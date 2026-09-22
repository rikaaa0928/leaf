use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use aho_corasick::{AhoCorasick, MatchKind};
use parking_lot::RwLock;
use tracing::{debug, info, warn};

use crate::app::router::Condition;
use crate::session::Session;

#[derive(Default, Debug)]
struct TrieNode {
    children: HashMap<String, TrieNode>,
    is_terminal: bool,
}

#[derive(Default, Debug)]
pub struct DomainSuffixTrie {
    root: TrieNode,
}

impl DomainSuffixTrie {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn insert(&mut self, suffix: &str) {
        let mut trimmed = suffix.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            return;
        }
        if trimmed.starts_with("*.") {
            trimmed = &trimmed[2..];
        } else if trimmed.starts_with('.') {
            trimmed = &trimmed[1..];
        }
        let trimmed = trimmed.trim_end_matches('.');
        if trimmed.is_empty() {
            return;
        }

        let mut curr = &mut self.root;
        for label in trimmed.split('.').rev() {
            if label.is_empty() {
                continue;
            }
            if curr.is_terminal {
                return;
            }
            let lower = label.to_ascii_lowercase();
            curr = curr.children.entry(lower).or_default();
        }
        curr.is_terminal = true;
        curr.children.clear();
    }

    pub fn matches(&self, domain: &str) -> bool {
        let trimmed = domain.trim().trim_end_matches('.');
        if trimmed.is_empty() {
            return false;
        }
        let lower = trimmed.to_ascii_lowercase();
        let mut curr = &self.root;
        for label in lower.split('.').rev() {
            if label.is_empty() {
                continue;
            }
            match curr.children.get(label) {
                Some(next) => {
                    if next.is_terminal {
                        return true;
                    }
                    curr = next;
                }
                None => return false,
            }
        }
        curr.is_terminal
    }
}

pub enum CompiledRemoteRule {
    Suffix(DomainSuffixTrie),
    Keyword(Option<AhoCorasick>),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RemoteRuleType {
    Suffix,
    Keyword,
}

pub struct RemoteDomainMatcher {
    url: String,
    #[allow(dead_code)]
    rule_type: RemoteRuleType,
    rules: Arc<RwLock<Option<CompiledRemoteRule>>>,
    initialized: Arc<AtomicBool>,
}

impl RemoteDomainMatcher {
    pub fn new(url: String, rule_type: RemoteRuleType, interval: Duration) -> Self {
        let rules = Arc::new(RwLock::new(None));
        let initialized = Arc::new(AtomicBool::new(false));

        let rules_clone = rules.clone();
        let init_clone = initialized.clone();
        let url_clone = url.clone();

        let task = async move {
            Self::fetch_and_update(&url_clone, rule_type, interval, &rules_clone, &init_clone)
                .await;

            let mut timer = tokio::time::interval(interval);
            timer.tick().await; // The first tick completes immediately; skip it since initial fetch is done

            loop {
                timer.tick().await;
                Self::fetch_and_update(&url_clone, rule_type, interval, &rules_clone, &init_clone)
                    .await;
            }
        };

        if let Ok(handle) = tokio::runtime::Handle::try_current() {
            handle.spawn(task);
        } else {
            std::thread::spawn(move || {
                if let Ok(rt) = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                {
                    rt.block_on(task);
                }
            });
        }

        Self {
            url,
            rule_type,
            rules,
            initialized,
        }
    }

    async fn fetch_source_single(url: &str) -> Result<String, String> {
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(15))
            .build()
            .map_err(|e| format!("failed to build client: {}", e))?;

        let resp = client.get(url).send().await.map_err(|e| format!("{}", e))?;
        if !resp.status().is_success() {
            return Err(format!("HTTP status {}", resp.status()));
        }
        resp.text().await.map_err(|e| format!("{}", e))
    }

    async fn fetch_source_with_retry(url: &str, interval: Duration) -> Result<String, String> {
        let mut last_err = String::new();
        for attempt in 0..=3 {
            if attempt > 0 {
                let base_delay = std::cmp::max(
                    Duration::from_millis(50),
                    std::cmp::min(Duration::from_secs(1), interval / 8),
                );
                let backoff = base_delay * (1 << (attempt - 1));
                tokio::time::sleep(backoff).await;
            }
            match Self::fetch_source_single(url).await {
                Ok(body) => return Ok(body),
                Err(e) => {
                    last_err = e;
                }
            }
        }
        Err(last_err)
    }

    async fn fetch_and_update(
        url: &str,
        rule_type: RemoteRuleType,
        interval: Duration,
        rules: &Arc<RwLock<Option<CompiledRemoteRule>>>,
        initialized: &Arc<AtomicBool>,
    ) {
        match Self::fetch_source_with_retry(url, interval).await {
            Ok(text) => {
                let (compiled, count) = Self::compile_rules(&text, rule_type);
                {
                    let mut guard = rules.write();
                    *guard = Some(compiled);
                }
                initialized.store(true, Ordering::Release);
                info!(
                    "remote rule [{}] loaded/refreshed with {} rules",
                    url, count
                );
            }
            Err(e) => {
                warn!(
                    "remote rule [{}] fetch failed after retries: {}, keeping existing data",
                    url, e
                );
            }
        }
    }

    fn compile_rules(text: &str, rule_type: RemoteRuleType) -> (CompiledRemoteRule, usize) {
        match rule_type {
            RemoteRuleType::Suffix => {
                let mut trie = DomainSuffixTrie::new();
                let mut count = 0;
                for line in text.lines() {
                    let s = line.trim();
                    if s.is_empty() || s.starts_with('#') {
                        continue;
                    }
                    trie.insert(s);
                    count += 1;
                }
                (CompiledRemoteRule::Suffix(trie), count)
            }
            RemoteRuleType::Keyword => {
                let mut keywords = Vec::new();
                for line in text.lines() {
                    let s = line.trim();
                    if s.is_empty() || s.starts_with('#') {
                        continue;
                    }
                    keywords.push(s.to_ascii_lowercase());
                }
                keywords.shrink_to_fit();
                let count = keywords.len();
                let ac = if keywords.is_empty() {
                    None
                } else {
                    AhoCorasick::builder()
                        .match_kind(MatchKind::Standard)
                        .build(&keywords)
                        .ok()
                };
                (CompiledRemoteRule::Keyword(ac), count)
            }
        }
    }
}

impl Condition for RemoteDomainMatcher {
    fn apply(&self, sess: &Session) -> bool {
        let destination = sess
            .destination_for_routing()
            .unwrap_or_else(|_| std::borrow::Cow::Borrowed(&sess.destination));
        if !destination.is_domain() {
            return false;
        }
        let domain = match destination.domain() {
            Some(d) => d,
            None => return false,
        };

        if !self.initialized.load(Ordering::Acquire) {
            warn!(
                "remote rule [{}] loading is not complete yet, treating as empty list",
                self.url
            );
            return false;
        }

        let guard = self.rules.read();
        match guard.as_ref() {
            Some(CompiledRemoteRule::Suffix(trie)) => {
                if trie.matches(domain) {
                    debug!(
                        "[{}] matches remote domain suffix in [{}]",
                        domain, self.url
                    );
                    return true;
                }
            }
            Some(CompiledRemoteRule::Keyword(Some(ac))) => {
                let lower = domain.trim().to_ascii_lowercase();
                if ac.find(&lower).is_some() {
                    debug!(
                        "[{}] matches remote domain keyword in [{}]",
                        domain, self.url
                    );
                    return true;
                }
            }
            _ => {}
        }
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::SocksAddr;
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[tokio::test]
    async fn test_remote_domain_matcher_lifecycle_and_error_handling() {
        let request_count = Arc::new(AtomicUsize::new(0));
        let count_clone = request_count.clone();

        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();

        std::thread::spawn(move || {
            for stream in listener.incoming() {
                if let Ok(mut stream) = stream {
                    let mut buf = [0u8; 1024];
                    let _ = stream.read(&mut buf);
                    let count = count_clone.fetch_add(1, Ordering::SeqCst);
                    if count < 4 {
                        // First 4 requests (1 initial + 3 retries) fail with 500
                        let response = "HTTP/1.1 500 Internal Server Error\r\nContent-Length: 0\r\nConnection: close\r\n\r\n";
                        let _ = stream.write_all(response.as_bytes());
                    } else if count == 4 {
                        // Next request succeeds
                        let body = "google.com\n.baidu.com\n";
                        let response = format!(
                            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                            body.len(),
                            body
                        );
                        let _ = stream.write_all(response.as_bytes());
                    } else {
                        // Subsequent requests fail with 502
                        let response = "HTTP/1.1 502 Bad Gateway\r\nContent-Length: 0\r\nConnection: close\r\n\r\n";
                        let _ = stream.write_all(response.as_bytes());
                    }
                }
            }
        });

        let url = format!("http://127.0.0.1:{}/suffix.txt", port);
        // Interval of 500ms
        let matcher =
            RemoteDomainMatcher::new(url, RemoteRuleType::Suffix, Duration::from_millis(500));

        let sess_google = Session {
            destination: SocksAddr::Domain("www.google.com".to_string(), 443),
            ..Default::default()
        };

        // 1. Immediately: initial fetch failed with 500 across all 3 retries, not initialized -> should return false (empty list)
        assert!(!matcher.apply(&sess_google));

        // 2. Wait for next tick with polling -> fetch succeeds, sets initialized = true
        let mut loaded = false;
        for _ in 0..25 {
            tokio::time::sleep(Duration::from_millis(100)).await;
            if matcher.apply(&sess_google) {
                loaded = true;
                break;
            }
        }
        assert!(
            loaded,
            "matcher should become active after retry on next interval"
        );
        assert!(request_count.load(Ordering::SeqCst) >= 5);

        let sess_other = Session {
            destination: SocksAddr::Domain("example.com".to_string(), 80),
            ..Default::default()
        };
        assert!(!matcher.apply(&sess_other));

        // 3. Wait for another tick -> update fails with 502 across retries -> existing data kept
        tokio::time::sleep(Duration::from_millis(700)).await;
        assert!(matcher.apply(&sess_google));
    }
}

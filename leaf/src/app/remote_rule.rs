use std::collections::HashMap;
use std::net::IpAddr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use aho_corasick::{AhoCorasick, MatchKind};
use cidr::IpCidr;
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
    Cidr(Vec<IpCidr>),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RemoteRuleType {
    Suffix,
    Keyword,
    Cidr,
}

fn spawn_fetch_loop(
    url: String,
    rule_type: RemoteRuleType,
    interval: Duration,
    rules: Arc<RwLock<Option<CompiledRemoteRule>>>,
    initialized: Arc<AtomicBool>,
) {
    let task = async move {
        fetch_and_update(&url, rule_type, interval, &rules, &initialized).await;

        let mut timer = tokio::time::interval(interval);
        timer.tick().await; // The first tick completes immediately; skip it since initial fetch is done

        loop {
            timer.tick().await;
            fetch_and_update(&url, rule_type, interval, &rules, &initialized).await;
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
        match fetch_source_single(url).await {
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
    match fetch_source_with_retry(url, interval).await {
        Ok(text) => {
            let (compiled, count) = compile_rules(&text, rule_type);
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
        RemoteRuleType::Cidr => {
            let mut cidrs = Vec::new();
            for line in text.lines() {
                let s = line.trim();
                if s.is_empty() || s.starts_with('#') {
                    continue;
                }
                if let Ok(cidr) = s.parse::<IpCidr>() {
                    cidrs.push(cidr);
                } else if let Ok(ip) = s.parse::<IpAddr>() {
                    cidrs.push(IpCidr::new_host(ip));
                } else {
                    debug!("parsing cidr {} failed", s);
                }
            }
            cidrs.shrink_to_fit();
            let count = cidrs.len();
            (CompiledRemoteRule::Cidr(cidrs), count)
        }
    }
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

        spawn_fetch_loop(
            url.clone(),
            rule_type,
            interval,
            rules.clone(),
            initialized.clone(),
        );

        Self {
            url,
            rule_type,
            rules,
            initialized,
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

pub struct RemoteIpCidrMatcher {
    url: String,
    rules: Arc<RwLock<Option<CompiledRemoteRule>>>,
    initialized: Arc<AtomicBool>,
}

impl RemoteIpCidrMatcher {
    pub fn new(url: String, interval: Duration) -> Self {
        let rules = Arc::new(RwLock::new(None));
        let initialized = Arc::new(AtomicBool::new(false));

        spawn_fetch_loop(
            url.clone(),
            RemoteRuleType::Cidr,
            interval,
            rules.clone(),
            initialized.clone(),
        );

        Self {
            url,
            rules,
            initialized,
        }
    }
}

impl Condition for RemoteIpCidrMatcher {
    fn apply(&self, sess: &Session) -> bool {
        let destination = sess
            .destination_for_routing()
            .unwrap_or_else(|_| std::borrow::Cow::Borrowed(&sess.destination));
        if destination.is_domain() {
            return false;
        }
        let ip = match destination.ip() {
            Some(i) => i,
            None => return false,
        };

        if !self.initialized.load(Ordering::Acquire) {
            warn!(
                "remote cidr rule [{}] loading is not complete yet, treating as empty list",
                self.url
            );
            return false;
        }

        let guard = self.rules.read();
        if let Some(CompiledRemoteRule::Cidr(cidrs)) = guard.as_ref() {
            for cidr in cidrs {
                if cidr.contains(&ip) {
                    debug!(
                        "[{}] matches remote ip-cidr [{}] in [{}]",
                        ip, cidr, self.url
                    );
                    return true;
                }
            }
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

    #[tokio::test]
    async fn test_remote_ip_cidr_matcher() {
        use std::net::SocketAddr;

        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();

        std::thread::spawn(move || {
            for stream in listener.incoming() {
                if let Ok(mut stream) = stream {
                    let mut buf = [0u8; 1024];
                    let _ = stream.read(&mut buf);
                    let body = "# CIDR list\n10.0.0.0/8\n192.168.1.0/24\n1.1.1.1\n";
                    let response = format!(
                        "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                        body.len(),
                        body
                    );
                    let _ = stream.write_all(response.as_bytes());
                }
            }
        });

        let url = format!("http://127.0.0.1:{}/cidr.txt", port);
        let matcher = RemoteIpCidrMatcher::new(url, Duration::from_millis(500));

        let sess_match_1 = Session {
            destination: SocksAddr::Ip("10.2.3.4:80".parse::<SocketAddr>().unwrap()),
            ..Default::default()
        };
        let sess_match_2 = Session {
            destination: SocksAddr::Ip("192.168.1.100:443".parse::<SocketAddr>().unwrap()),
            ..Default::default()
        };
        let sess_match_3 = Session {
            destination: SocksAddr::Ip("1.1.1.1:53".parse::<SocketAddr>().unwrap()),
            ..Default::default()
        };
        let sess_nomatch = Session {
            destination: SocksAddr::Ip("172.16.0.1:80".parse::<SocketAddr>().unwrap()),
            ..Default::default()
        };
        let sess_domain = Session {
            destination: SocksAddr::Domain("10.2.3.4".to_string(), 80),
            ..Default::default()
        };

        let mut loaded = false;
        for _ in 0..25 {
            tokio::time::sleep(Duration::from_millis(100)).await;
            if matcher.apply(&sess_match_1) {
                loaded = true;
                break;
            }
        }
        assert!(loaded, "cidr matcher should become active after fetch");
        assert!(matcher.apply(&sess_match_2));
        assert!(matcher.apply(&sess_match_3));
        assert!(!matcher.apply(&sess_nomatch));
        assert!(!matcher.apply(&sess_domain));
    }
}

use std::collections::HashMap;
use std::net::{IpAddr, Ipv4Addr};

use anyhow::{anyhow, Result};
use tokio::sync::RwLock;
use tracing::debug;
use trust_dns_proto::op::{
    header::MessageType, op_code::OpCode, response_code::ResponseCode, Message,
};
use trust_dns_proto::rr::{
    dns_class::DNSClass, rdata, record_data::RData, record_type::RecordType, resource::Record,
};

pub enum FakeDnsMode {
    Include,
    Exclude,
}

pub struct FakeDns(RwLock<FakeDnsImpl>);

impl FakeDns {
    pub fn new(mode: FakeDnsMode) -> Self {
        Self(RwLock::new(FakeDnsImpl::new(mode)))
    }

    pub async fn add_filter(&self, filter: String) {
        self.0.write().await.add_filter(filter)
    }

    pub async fn query_domain(&self, ip: &IpAddr) -> Option<String> {
        self.0.read().await.query_domain(ip)
    }

    pub async fn query_fake_ip(&self, domain: &str) -> Option<IpAddr> {
        self.0.read().await.query_fake_ip(domain)
    }

    pub async fn generate_fake_response(&self, request: &[u8]) -> Result<Vec<u8>> {
        self.0.write().await.generate_fake_response(request)
    }

    pub async fn is_fake_ip(&self, ip: &IpAddr) -> bool {
        self.0.read().await.is_fake_ip(ip)
    }
}

struct FakeDnsImpl {
    ip_to_domain: HashMap<u32, String>,
    domain_to_ip: HashMap<String, u32>,
    cursor: u32,
    min_cursor: u32,
    max_cursor: u32,
    ttl: u32,
    filters: Vec<String>,
    mode: FakeDnsMode,
}

impl FakeDnsImpl {
    pub(self) fn new(mode: FakeDnsMode) -> Self {
        let min_cursor = Self::ip_to_u32(&Ipv4Addr::new(198, 18, 0, 0));
        let max_cursor = Self::ip_to_u32(&Ipv4Addr::new(198, 18, 255, 255));
        Self {
            ip_to_domain: HashMap::new(),
            domain_to_ip: HashMap::new(),
            cursor: min_cursor,
            min_cursor,
            max_cursor,
            ttl: 1,
            filters: Vec::new(),
            mode,
        }
    }

    pub(self) fn add_filter(&mut self, filter: String) {
        self.filters.push(filter);
    }

    pub(self) fn query_domain(&self, ip: &IpAddr) -> Option<String> {
        let ip = match ip {
            IpAddr::V4(ip) => ip,
            _ => return None,
        };
        self.ip_to_domain.get(&Self::ip_to_u32(ip)).cloned()
    }

    pub(self) fn query_fake_ip(&self, domain: &str) -> Option<IpAddr> {
        self.domain_to_ip
            .get(domain)
            .map(|v| IpAddr::V4(Self::u32_to_ip(v.to_owned())))
    }

    pub(self) fn generate_fake_response(&mut self, request: &[u8]) -> Result<Vec<u8>> {
        tracing::error!("[FAKE-DNS] Processing DNS request: size={} bytes", request.len());
        let req = Message::from_vec(request).map_err(|e| {
            tracing::error!("[FAKE-DNS] Failed to parse DNS request: error={}", e);
            e
        })?;

        if req.queries().is_empty() {
            tracing::error!("[FAKE-DNS] DNS request contains no queries");
            return Err(anyhow!("no queries in this DNS request"));
        }

        let query = &req.queries()[0];
        tracing::error!("[FAKE-DNS] DNS query details: class={:?}, type={:?}", query.query_class(), query.query_type());
        
        if query.query_class() != DNSClass::IN {
            tracing::error!("[FAKE-DNS] Unsupported DNS query class: class={:?}", query.query_class());
            return Err(anyhow!("unsupported query class {}", query.query_class()));
        }

        let t = query.query_type();
        if t != RecordType::A && t != RecordType::AAAA && t != RecordType::HTTPS {
            tracing::error!("[FAKE-DNS] Unsupported DNS query type: type={:?}", query.query_type());
            return Err(anyhow!(
                "unsupported query record type {:?}",
                query.query_type()
            ));
        }

        let raw_name = query.name();

        // TODO check if a valid domain
        let domain = if raw_name.is_fqdn() {
            let fqdn = raw_name.to_ascii();
            fqdn[..fqdn.len() - 1].to_string()
        } else {
            raw_name.to_ascii()
        };

        tracing::error!("[FAKE-DNS] Extracted domain from query: domain={}", domain);

        if !self.accept(&domain) {
            tracing::error!("[FAKE-DNS] Domain not accepted by filter: domain={}", domain);
            return Err(anyhow!("domain {} not accepted", domain));
        }
        
        tracing::error!("[FAKE-DNS] Domain accepted by filter: domain={}", domain);

        let ip = if let Some(ip) = self.query_fake_ip(&domain) {
            tracing::error!("[FAKE-DNS] Found existing fake IP for domain: domain={}, ip={}", domain, ip);
            match ip {
                IpAddr::V4(a) => a,
                _ => {
                    tracing::error!("[FAKE-DNS] Unexpected IPv6 fake IP: domain={}, ip={}", domain, ip);
                    return Err(anyhow!("unexpected Ipv6 fake IP"));
                }
            }
        } else {
            tracing::error!("[FAKE-DNS] No existing fake IP found, allocating new one: domain={}", domain);
            let ip = self.allocate_ip(&domain)?;
            tracing::error!("[FAKE-DNS] Allocated new fake IP: domain={}, ip={}", domain, ip);
            ip
        };

        tracing::error!("[FAKE-DNS] Creating DNS response: domain={}, ip={}", domain, ip);
        let mut resp = Message::new();

        // sets the response according to request
        // https://github.com/miekg/dns/blob/f515aa579d28efa1af67d9a62cc57f2dfe59da76/defaults.go#L15
        resp.set_id(req.id())
            .set_message_type(MessageType::Response)
            .set_op_code(req.op_code());

        if resp.op_code() == OpCode::Query {
            resp.set_recursion_desired(req.recursion_desired())
                .set_checking_disabled(req.checking_disabled());
        }
        resp.set_response_code(ResponseCode::NoError);
        if !req.queries().is_empty() {
            resp.add_query(query.clone());
        }

        if query.query_type() == RecordType::A {
            tracing::error!("[FAKE-DNS] Adding A record to response: domain={}, ip={}, ttl={}", domain, ip, self.ttl);
            let mut ans = Record::new();
            ans.set_name(raw_name.clone())
                .set_rr_type(RecordType::A)
                .set_ttl(self.ttl)
                .set_dns_class(DNSClass::IN)
                .set_data(Some(RData::A(rdata::A(ip))));
            resp.add_answer(ans);
        }

        let response_bytes = resp.to_vec()?;
        tracing::error!("[FAKE-DNS] DNS response created successfully: domain={}, ip={}, response_size={} bytes", domain, ip, response_bytes.len());
        Ok(response_bytes)
    }

    pub(self) fn is_fake_ip(&self, ip: &IpAddr) -> bool {
        let ip = match ip {
            IpAddr::V4(ip) => ip,
            _ => return false,
        };
        let ip = Self::ip_to_u32(ip);
        ip >= self.min_cursor && ip <= self.max_cursor
    }

    fn allocate_ip(&mut self, domain: &str) -> Result<Ipv4Addr> {
        if let Some(prev_domain) = self.ip_to_domain.insert(self.cursor, domain.to_owned()) {
            // Remove the entry in the reverse map to make sure we won't have
            // multiple domains point to a same IP.
            self.domain_to_ip.remove(&prev_domain);
        }
        self.domain_to_ip.insert(domain.to_owned(), self.cursor);
        let ip = Self::u32_to_ip(self.cursor);
        self.prepare_next_cursor()?;
        Ok(ip)
    }

    // Make sure `self.cursor` is valid and can be used immediately for next fake IP.
    fn prepare_next_cursor(&mut self) -> Result<()> {
        for _ in 0..3 {
            self.cursor += 1;
            if self.cursor > self.max_cursor {
                self.cursor = self.min_cursor;
            }
            // avoid network and broadcast addresses
            match Self::u32_to_ip(self.cursor).octets()[3] {
                0 | 255 => {
                    continue;
                }
                _ => return Ok(()),
            }
        }
        Err(anyhow!("unable to prepare next cursor"))
    }

    fn accept(&self, domain: &str) -> bool {
        match self.mode {
            FakeDnsMode::Exclude => {
                for d in &self.filters {
                    if domain.contains(d) || d == "*" {
                        return false;
                    }
                }
                true
            }
            FakeDnsMode::Include => {
                for d in &self.filters {
                    if domain.contains(d) || d == "*" {
                        return true;
                    }
                }
                false
            }
        }
    }

    fn u32_to_ip(ip: u32) -> Ipv4Addr {
        Ipv4Addr::from(ip)
    }

    fn ip_to_u32(ip: &Ipv4Addr) -> u32 {
        u32::from_be_bytes(ip.octets())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::Ipv4Addr;

    #[test]
    fn test_u32_to_ip() {
        let ip1 = Ipv4Addr::new(127, 0, 0, 1);
        let ip2 = FakeDnsImpl::u32_to_ip(2130706433u32);
        assert_eq!(ip1, ip2);
    }

    #[test]
    fn test_ip_to_u32() {
        let ip = Ipv4Addr::new(127, 0, 0, 1);
        let ip1 = FakeDnsImpl::ip_to_u32(&ip);
        let ip2 = 2130706433u32;
        assert_eq!(ip1, ip2);
    }
}

#[cfg(feature = "outbound-rog")]
use std::io;

#[cfg(feature = "outbound-rog")]
use hyper_util::rt::TokioIo;
#[cfg(feature = "outbound-rog")]
use std::time::Duration;
#[cfg(feature = "outbound-rog")]
use tonic::transport::{Channel, Endpoint, Uri};
#[cfg(feature = "outbound-rog")]
use tower::service_fn;

#[cfg(feature = "outbound-rog")]
use crate::proxy::rog::protocol::rog::rog_service_client::RogServiceClient;
#[cfg(feature = "outbound-rog")]
use crate::proxy::*;

#[cfg(feature = "outbound-rog")]
const DEFAULT_KEEP_ALIVE_INTERVAL_SECS: u64 = 30;
#[cfg(feature = "outbound-rog")]
const DEFAULT_KEEP_ALIVE_TIMEOUT_SECS: u64 = 20;
#[cfg(feature = "outbound-rog")]
const DEFAULT_KEEP_ALIVE_WHILE_IDLE: bool = true;

#[cfg(feature = "outbound-rog")]
#[derive(Clone, Copy, Debug)]
pub struct ClientOptions {
    pub keep_alive: bool,
    pub keep_alive_interval_secs: u64,
    pub keep_alive_timeout_secs: u64,
    pub keep_alive_while_idle: bool,
}

#[cfg(feature = "outbound-rog")]
impl ClientOptions {
    pub fn new(
        keep_alive: bool,
        keep_alive_interval_secs: u32,
        keep_alive_timeout_secs: u32,
        keep_alive_while_idle: Option<bool>,
    ) -> Self {
        Self {
            keep_alive,
            keep_alive_interval_secs: nonzero_or_default(
                keep_alive_interval_secs,
                DEFAULT_KEEP_ALIVE_INTERVAL_SECS,
            ),
            keep_alive_timeout_secs: nonzero_or_default(
                keep_alive_timeout_secs,
                DEFAULT_KEEP_ALIVE_TIMEOUT_SECS,
            ),
            keep_alive_while_idle: keep_alive_while_idle.unwrap_or(DEFAULT_KEEP_ALIVE_WHILE_IDLE),
        }
    }
}

#[cfg(feature = "outbound-rog")]
impl Default for ClientOptions {
    fn default() -> Self {
        Self {
            keep_alive: false,
            keep_alive_interval_secs: DEFAULT_KEEP_ALIVE_INTERVAL_SECS,
            keep_alive_timeout_secs: DEFAULT_KEEP_ALIVE_TIMEOUT_SECS,
            keep_alive_while_idle: DEFAULT_KEEP_ALIVE_WHILE_IDLE,
        }
    }
}

#[cfg(feature = "outbound-rog")]
fn nonzero_or_default(value: u32, default_value: u64) -> u64 {
    if value == 0 {
        default_value
    } else {
        u64::from(value)
    }
}

#[cfg(feature = "outbound-rog")]
async fn connect_tcp(
    dns_client: SyncDnsClient,
    host: String,
    port: u16,
) -> Result<TokioIo<AnyStream>, std::io::Error> {
    let stream = new_tcp_stream(dns_client, &host, &port).await?;
    Ok(TokioIo::new(stream))
}

#[cfg(feature = "outbound-rog")]
pub async fn init_client(
    endpoint: String,
    dns_client: SyncDnsClient,
    port: u16,
    custom_connector: bool,
    client_options: ClientOptions,
) -> Result<RogServiceClient<Channel>, tonic::transport::Error> {
    let mut endpoint = Endpoint::new(endpoint)?;
    if client_options.keep_alive {
        endpoint = endpoint
            .http2_keep_alive_interval(Duration::from_secs(client_options.keep_alive_interval_secs))
            .keep_alive_timeout(Duration::from_secs(client_options.keep_alive_timeout_secs))
            .keep_alive_while_idle(client_options.keep_alive_while_idle);
    }
    let channel = if custom_connector {
        endpoint
            .connect_with_connector(service_fn(move |uri: Uri| {
                let dns_client = dns_client.clone();
                let host = uri.host().unwrap_or("localhost").to_string();
                let request_port = uri.port_u16().unwrap_or(port);
                connect_tcp(dns_client, host, request_port)
            }))
            .await?
    } else {
        endpoint.connect().await?
    };
    Ok(RogServiceClient::new(channel))
}

/// Parse address for ROG protocol
#[cfg(feature = "outbound-rog")]
pub fn parse_address(addr: &str) -> io::Result<(String, u16)> {
    // Use rsplit_once to handle IPv6 addresses with colons
    let (host, port) = addr
        .rsplit_once(':')
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "Invalid address format"))?;

    // Parse port
    let port: u16 = port
        .parse()
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "Invalid port number"))?;

    // Validate host is not empty
    if host.is_empty() {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "Empty host"));
    }

    Ok((host.to_string(), port))
}

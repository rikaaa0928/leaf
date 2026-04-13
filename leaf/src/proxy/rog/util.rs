#[cfg(feature = "outbound-rog")]
use std::io;

#[cfg(feature = "outbound-rog")]
use hyper_util::rt::TokioIo;
#[cfg(feature = "outbound-rog")]
use tonic::transport::{Channel, Endpoint, Uri};
#[cfg(feature = "outbound-rog")]
use tower::service_fn;

#[cfg(feature = "outbound-rog")]
use crate::proxy::rog::protocol::rog::rog_service_client::RogServiceClient;
#[cfg(feature = "outbound-rog")]
use crate::proxy::*;

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
) -> Result<RogServiceClient<Channel>, tonic::transport::Error> {
    let endpoint = Endpoint::new(endpoint)?;
    let channel = if custom_connector {
        endpoint.connect_with_connector(service_fn(move |uri: Uri| {
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

#[cfg(feature = "outbound-rog")]
use std::io;

#[cfg(feature = "outbound-rog")]
use crate::proxy::rog::protocol::rog::rog_service_client::RogServiceClient;
#[cfg(feature = "outbound-rog")]
use crate::proxy::rog::stream::RogStream;
#[cfg(feature = "outbound-rog")]
use crate::proxy::rog::util::init_client;
#[cfg(feature = "outbound-rog")]
use crate::{proxy::*, session::*};
#[cfg(feature = "outbound-rog")]
use async_trait::async_trait;
#[cfg(feature = "outbound-rog")]
use std::sync::Arc;
#[cfg(feature = "outbound-rog")]
use tonic::transport::Channel;
#[cfg(feature = "outbound-rog")]
pub struct Handler {
    pub address: String,
    pub port: u16,
    pub password: String,
    pub custom_connector: bool,
    pub keep_alive: bool,
    pub dns_client: SyncDnsClient,
    pub rog_client: Arc<tokio::sync::OnceCell<RogServiceClient<Channel>>>,
}

#[cfg(feature = "outbound-rog")]
#[async_trait]
impl OutboundStreamHandler for Handler {
    fn connect_addr(&self) -> OutboundConnect {
        OutboundConnect::Unknown
    }

    async fn handle<'a>(
        &'a self,
        sess: &'a Session,
        _lhs: Option<&mut AnyStream>,
        _stream: Option<AnyStream>,
    ) -> io::Result<AnyStream> {
        let schema = if self.port == 443 { "https" } else { "http" };
        let address = self.address.clone();
        let port = self.port;
        let endpoint = format!("{}://{}:{}", schema, address, port);
        let dns_client = self.dns_client.clone();
        let password = self.password.clone();

        let client = self
            .rog_client
            .get_or_try_init(|| async { init_client(endpoint, dns_client, port, self.custom_connector, self.keep_alive).await })
            .await
            .map_err(io::Error::other)?
            .clone();

        // Get destination information
        let (dst_addr, dst_port) = match &sess.destination {
            SocksAddr::Ip(socket_addr) => (socket_addr.ip().to_string(), socket_addr.port()),
            SocksAddr::Domain(domain, port) => (domain.clone(), *port),
        };

        // Create ROG stream with error handling
        let rog_stream = RogStream::new(client, dst_addr, dst_port, password)
            .await
            .map_err(|e| {
                tracing::error!("Failed to create ROG stream: {}", e);
                io::Error::new(
                    io::ErrorKind::Other,
                    format!("ROG stream creation failed: {}", e),
                )
            })?;

        tracing::debug!("ROG stream established to {}:{}", address, port);
        Ok(Box::new(rog_stream))
    }
}

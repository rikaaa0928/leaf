#[cfg(feature = "outbound-rog")]
use std::io;

#[cfg(feature = "outbound-rog")]
use async_trait::async_trait;
#[cfg(feature = "outbound-rog")]
use std::sync::Arc;
#[cfg(feature = "outbound-rog")]
use tonic::transport::Channel;

#[cfg(feature = "outbound-rog")]
use crate::proxy::rog::datagram::RogDatagram;
#[cfg(feature = "outbound-rog")]
use crate::proxy::rog::protocol::rog::rog_service_client::RogServiceClient;
#[cfg(feature = "outbound-rog")]
use crate::proxy::rog::util::init_client;
#[cfg(feature = "outbound-rog")]
use crate::{proxy::*, session::Session};

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
impl OutboundDatagramHandler for Handler {
    fn connect_addr(&self) -> OutboundConnect {
        OutboundConnect::Unknown
    }

    fn transport_type(&self) -> DatagramTransportType {
        DatagramTransportType::Reliable
    }

    async fn handle<'a>(
        &'a self,
        _sess: &'a Session,
        _transport: Option<OutboundTransport<AnyStream, AnyOutboundDatagram>>,
    ) -> io::Result<AnyOutboundDatagram> {
        let schema = if self.port == 443 { "https" } else { "http" };
        let endpoint = format!("{}://{}:{}", schema, self.address, self.port);
        let dns_client = self.dns_client.clone();
        let port = self.port;

        let client = self
            .rog_client
            .get_or_try_init(|| async { init_client(endpoint, dns_client, port, self.custom_connector, self.keep_alive).await })
            .await
            .map_err(io::Error::other)?
            .clone();

        // Create ROG datagram with error handling
        let rog_datagram = RogDatagram::new(client, self.password.clone())
            .await
            .map_err(|e| {
                tracing::error!("Failed to create ROG datagram: {}", e);
                io::Error::new(
                    io::ErrorKind::Other,
                    format!("ROG datagram creation failed: {}", e),
                )
            })?;

        tracing::debug!("ROG datagram established to {}:{}", self.address, self.port);
        Ok(Box::new(rog_datagram))
    }
}

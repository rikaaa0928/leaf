#[cfg(feature = "outbound-rog")]
use std::io;

#[cfg(feature = "outbound-rog")]
use async_trait::async_trait;

#[cfg(feature = "outbound-rog")]
use crate::proxy::rog::datagram::RogDatagram;
#[cfg(feature = "outbound-rog")]
use crate::proxy::rog::util::RogConnector;
#[cfg(feature = "outbound-rog")]
use crate::{proxy::*, session::Session};

#[cfg(feature = "outbound-rog")]
pub struct Handler {
    pub address: String,
    pub port: u16,
    pub password: String,
}

#[cfg(feature = "outbound-rog")]
#[async_trait]
impl OutboundDatagramHandler for Handler {
    fn connect_addr(&self) -> OutboundConnect {
        OutboundConnect::Proxy(Network::Tcp, self.address.clone(), self.port)
    }

    fn transport_type(&self) -> DatagramTransportType {
        DatagramTransportType::Reliable
    }

    async fn handle<'a>(
        &'a self,
        _sess: &'a Session,
        _transport: Option<OutboundTransport<AnyStream, AnyOutboundDatagram>>,
    ) -> io::Result<AnyOutboundDatagram> {
        // Create gRPC client connection for UDP with retry logic
        let connector = RogConnector::new(&self.address, self.port);
        let client = connector.connect().await?;

        // Create ROG datagram with error handling
        let rog_datagram = RogDatagram::new(client, self.password.clone())
            .await
            .map_err(|e| {
                tracing::error!("Failed to create ROG datagram: {}", e);
                io::Error::new(io::ErrorKind::Other, format!("ROG datagram creation failed: {}", e))
            })?;

        tracing::debug!("ROG datagram established to {}:{}", self.address, self.port);
        Ok(Box::new(rog_datagram))
    }
}
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
        tracing::error!("[ROG-OUTBOUND] Creating ROG datagram connection: target={}:{}", self.address, self.port);
        
        // Create gRPC client connection for UDP with retry logic
        let connector = RogConnector::new(&self.address, self.port);
        tracing::error!("[ROG-OUTBOUND] Connecting to ROG server: target={}:{}", self.address, self.port);
        let client = connector.connect().await.map_err(|e| {
            tracing::error!("[ROG-OUTBOUND] Failed to connect to ROG server: target={}:{}, error={}", self.address, self.port, e);
            e
        })?;
        tracing::error!("[ROG-OUTBOUND] Successfully connected to ROG server: target={}:{}", self.address, self.port);

        // Create ROG datagram with error handling
        tracing::error!("[ROG-OUTBOUND] Creating ROG datagram with authentication");
        let rog_datagram = RogDatagram::new(client, self.password.clone())
            .await
            .map_err(|e| {
                tracing::error!("[ROG-OUTBOUND] Failed to create ROG datagram: target={}:{}, error={}", self.address, self.port, e);
                io::Error::new(io::ErrorKind::Other, format!("ROG datagram creation failed: {}", e))
            })?;

        tracing::error!("[ROG-OUTBOUND] ROG datagram established successfully: target={}:{}", self.address, self.port);
        Ok(Box::new(rog_datagram))
    }
}
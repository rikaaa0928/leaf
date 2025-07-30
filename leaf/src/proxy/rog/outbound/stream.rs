#[cfg(feature = "outbound-rog")]
use std::io;

#[cfg(feature = "outbound-rog")]
use async_trait::async_trait;

#[cfg(feature = "outbound-rog")]
use crate::proxy::rog::stream::RogStream;
#[cfg(feature = "outbound-rog")]
use crate::proxy::rog::util::RogConnector;
#[cfg(feature = "outbound-rog")]
use crate::{proxy::*, session::*};

#[cfg(feature = "outbound-rog")]
pub struct Handler {
    pub address: String,
    pub port: u16,
    pub password: String,
}

#[cfg(feature = "outbound-rog")]
#[async_trait]
impl OutboundStreamHandler for Handler {
    fn connect_addr(&self) -> OutboundConnect {
        OutboundConnect::Proxy(Network::Tcp, self.address.clone(), self.port)
    }

    async fn handle<'a>(
        &'a self,
        sess: &'a Session,
        _lhs: Option<&mut AnyStream>,
        _stream: Option<AnyStream>,
    ) -> io::Result<AnyStream> {
        // Create gRPC client connection with retry logic
        let connector = RogConnector::new(&self.address, self.port);
        let client = connector.connect().await?;

        // Get destination information
        let (dst_addr, dst_port) = match &sess.destination {
            SocksAddr::Ip(socket_addr) => (socket_addr.ip().to_string(), socket_addr.port()),
            SocksAddr::Domain(domain, port) => (domain.clone(), *port),
        };

        // Create ROG stream with error handling
        let rog_stream = RogStream::new(client, dst_addr, dst_port, self.password.clone())
            .await
            .map_err(|e| {
                tracing::error!("Failed to create ROG stream: {}", e);
                io::Error::new(io::ErrorKind::Other, format!("ROG stream creation failed: {}", e))
            })?;

        tracing::debug!("ROG stream established to {}:{}", self.address, self.port);
        Ok(Box::new(rog_stream))
    }
}
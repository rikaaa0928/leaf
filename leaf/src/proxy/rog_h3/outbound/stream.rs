#[cfg(feature = "outbound-rog-h3")]
use std::io;
#[cfg(feature = "outbound-rog-h3")]
use std::sync::Arc;

#[cfg(feature = "outbound-rog-h3")]
use async_trait::async_trait;

#[cfg(feature = "outbound-rog-h3")]
use crate::proxy::rog_h3::stream::RogH3Stream;
#[cfg(feature = "outbound-rog-h3")]
use crate::proxy::rog_h3::RogH3Connector;
#[cfg(feature = "outbound-rog-h3")]
use crate::{proxy::*, session::*};

#[cfg(feature = "outbound-rog-h3")]
pub struct Handler {
    pub connector: Arc<RogH3Connector>,
}

#[cfg(feature = "outbound-rog-h3")]
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
        let (dst_addr, dst_port) = match &sess.destination {
            SocksAddr::Ip(socket_addr) => (socket_addr.ip().to_string(), socket_addr.port()),
            SocksAddr::Domain(domain, port) => (domain.clone(), *port),
        };

        let (recv, send) = self
            .connector
            .connect_tcp_stream(&dst_addr, dst_port)
            .await?;

        tracing::debug!("ROG-H3 stream established to {}:{}", dst_addr, dst_port);
        Ok(Box::new(RogH3Stream::new(recv, send)))
    }
}

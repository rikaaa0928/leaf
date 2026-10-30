#[cfg(feature = "outbound-rog-h3")]
use std::io;
#[cfg(feature = "outbound-rog-h3")]
use std::sync::Arc;

#[cfg(feature = "outbound-rog-h3")]
use async_trait::async_trait;

#[cfg(feature = "outbound-rog-h3")]
use crate::proxy::rog_h3::datagram::RogH3Datagram;
#[cfg(feature = "outbound-rog-h3")]
use crate::proxy::rog_h3::RogH3Connector;
#[cfg(feature = "outbound-rog-h3")]
use crate::{proxy::*, session::Session};

#[cfg(feature = "outbound-rog-h3")]
pub struct Handler {
    pub connector: Arc<RogH3Connector>,
}

#[cfg(feature = "outbound-rog-h3")]
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
        sess: &'a Session,
        _transport: Option<OutboundTransport<AnyStream, AnyOutboundDatagram>>,
    ) -> io::Result<AnyOutboundDatagram> {
        let src_host = sess.source.ip().to_string();
        let src_port = sess.source.port();

        let (recv, send) = self
            .connector
            .connect_udp_tunnel(&src_host, src_port)
            .await?;

        tracing::debug!("ROG-H3 datagram tunnel established for {}:{}", src_host, src_port);
        Ok(Box::new(RogH3Datagram::new(recv, send)))
    }
}

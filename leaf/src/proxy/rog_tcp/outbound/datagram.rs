#[cfg(feature = "outbound-rog-tcp")]
use std::io;

#[cfg(feature = "outbound-rog-tcp")]
use async_trait::async_trait;

#[cfg(feature = "outbound-rog-tcp")]
use crate::proxy::rog_tcp::datagram::RogTcpDatagram;
#[cfg(feature = "outbound-rog-tcp")]
use crate::proxy::rog_tcp::util::{write_conn_type, CONN_TYPE_UDP};
#[cfg(feature = "outbound-rog-tcp")]
use crate::{proxy::*, session::Session};

#[cfg(feature = "outbound-rog-tcp")]
pub struct Handler {
    pub address: String,
    pub port: u16,
    pub password: String,
}

#[cfg(feature = "outbound-rog-tcp")]
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
        transport: Option<AnyOutboundTransport>,
    ) -> io::Result<AnyOutboundDatagram> {
        let mut stream = if let Some(OutboundTransport::Stream(stream)) = transport {
            stream
        } else {
            return Err(io::Error::other("invalid input"));
        };

        write_conn_type(&mut stream, CONN_TYPE_UDP).await?;

        Ok(Box::new(RogTcpDatagram::new(
            stream,
            self.password.clone(),
        )))
    }
}

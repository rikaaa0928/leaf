#[cfg(feature = "outbound-rog-tcp")]
use std::io;

#[cfg(feature = "outbound-rog-tcp")]
use async_trait::async_trait;

#[cfg(feature = "outbound-rog-tcp")]
use crate::proxy::rog_tcp::protocol::rog::StreamReq;
#[cfg(feature = "outbound-rog-tcp")]
use crate::proxy::rog_tcp::stream::RogTcpStream;
#[cfg(feature = "outbound-rog-tcp")]
use crate::proxy::rog_tcp::util::{encrypt_field, write_conn_type, write_frame, CONN_TYPE_STREAM};
#[cfg(feature = "outbound-rog-tcp")]
use crate::{proxy::*, session::*};

#[cfg(feature = "outbound-rog-tcp")]
pub struct Handler {
    pub address: String,
    pub port: u16,
    pub password: String,
}

#[cfg(feature = "outbound-rog-tcp")]
#[async_trait]
impl OutboundStreamHandler for Handler {
    fn connect_addr(&self) -> OutboundConnect {
        OutboundConnect::Proxy(Network::Tcp, self.address.clone(), self.port)
    }

    async fn handle<'a>(
        &'a self,
        sess: &'a Session,
        _lhs: Option<&mut AnyStream>,
        stream: Option<AnyStream>,
    ) -> io::Result<AnyStream> {
        let mut stream = stream.ok_or_else(|| io::Error::other("invalid input"))?;
        write_conn_type(&mut stream, CONN_TYPE_STREAM).await?;

        let (dst_addr, dst_port) = match &sess.destination {
            SocksAddr::Ip(socket_addr) => (socket_addr.ip().to_string(), socket_addr.port()),
            SocksAddr::Domain(domain, port) => (domain.clone(), *port),
        };
        let password = &self.password;

        let auth_req = StreamReq {
            auth: encrypt_field(password, password)?,
            payload: None,
            dst_addr: Some(encrypt_field(&dst_addr, password)?),
            dst_port: Some(dst_port as u32),
        };
        write_frame(&mut stream, &auth_req).await?;

        Ok(Box::new(RogTcpStream::new(
            stream,
            self.password.clone(),
            dst_port != 443,
        )))
    }
}

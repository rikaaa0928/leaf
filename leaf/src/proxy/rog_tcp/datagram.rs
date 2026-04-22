#[cfg(feature = "outbound-rog-tcp")]
use std::io;

#[cfg(feature = "outbound-rog-tcp")]
use async_trait::async_trait;
#[cfg(feature = "outbound-rog-tcp")]
use tokio::io::{AsyncRead, AsyncWrite, AsyncWriteExt, ReadHalf, WriteHalf};

#[cfg(feature = "outbound-rog-tcp")]
use crate::proxy::rog_tcp::protocol::rog::{UdpReq, UdpRes};
#[cfg(feature = "outbound-rog-tcp")]
use crate::proxy::rog_tcp::util::{
    decrypt_bytes, decrypt_field, encrypt_bytes, encrypt_field, read_msg, write_frame,
};
#[cfg(feature = "outbound-rog-tcp")]
use crate::proxy::*;
#[cfg(feature = "outbound-rog-tcp")]
use crate::session::SocksAddr;

#[cfg(feature = "outbound-rog-tcp")]
pub struct RogTcpDatagram<T> {
    stream: T,
    password: String,
}

#[cfg(feature = "outbound-rog-tcp")]
impl<T> RogTcpDatagram<T> {
    pub fn new(stream: T, password: String) -> Self {
        Self { stream, password }
    }
}

#[cfg(feature = "outbound-rog-tcp")]
impl<T> OutboundDatagram for RogTcpDatagram<T>
where
    T: 'static + AsyncRead + AsyncWrite + Unpin + Send + Sync,
{
    fn split(
        self: Box<Self>,
    ) -> (
        Box<dyn OutboundDatagramRecvHalf>,
        Box<dyn OutboundDatagramSendHalf>,
    ) {
        let (reader, writer) = tokio::io::split(self.stream);
        (
            Box::new(RogTcpDatagramRecvHalf {
                reader,
                password: self.password.clone(),
            }),
            Box::new(RogTcpDatagramSendHalf {
                writer,
                password: self.password.clone(),
            }),
        )
    }
}

#[cfg(feature = "outbound-rog-tcp")]
pub struct RogTcpDatagramRecvHalf<T> {
    reader: ReadHalf<T>,
    password: String,
}

#[cfg(feature = "outbound-rog-tcp")]
#[async_trait]
impl<T> OutboundDatagramRecvHalf for RogTcpDatagramRecvHalf<T>
where
    T: AsyncRead + AsyncWrite + Send + Sync + Unpin,
{
    async fn recv_from(&mut self, buf: &mut [u8]) -> io::Result<(usize, SocksAddr)> {
        let mut res: UdpRes = read_msg(&mut self.reader).await?;
        if let Some(enc) = &res.dst_addr {
            res.dst_addr = Some(decrypt_field(enc, &self.password)?);
        }
        if let Some(enc) = &res.src_addr {
            res.src_addr = Some(decrypt_field(enc, &self.password)?);
        }
        res.payload = decrypt_bytes(&res.payload, &self.password)?;

        let payload_len = std::cmp::min(res.payload.len(), buf.len());
        buf[..payload_len].copy_from_slice(&res.payload[..payload_len]);

        let addr = match (res.dst_addr, res.dst_port) {
            (Some(addr), Some(port)) => match addr.parse() {
                Ok(ip) => SocksAddr::Ip(std::net::SocketAddr::new(ip, port as u16)),
                Err(_) => SocksAddr::Domain(addr, port as u16),
            },
            _ => {
                return Err(io::Error::other(
                    "rog_tcp udp response missing destination address",
                ));
            }
        };

        Ok((payload_len, addr))
    }
}

#[cfg(feature = "outbound-rog-tcp")]
pub struct RogTcpDatagramSendHalf<T> {
    writer: WriteHalf<T>,
    password: String,
}

#[cfg(feature = "outbound-rog-tcp")]
#[async_trait]
impl<T> OutboundDatagramSendHalf for RogTcpDatagramSendHalf<T>
where
    T: AsyncRead + AsyncWrite + Send + Sync + Unpin,
{
    async fn send_to(&mut self, buf: &[u8], dst_addr: &SocksAddr) -> io::Result<usize> {
        let (addr, port) = match dst_addr {
            SocksAddr::Ip(socket_addr) => (socket_addr.ip().to_string(), socket_addr.port() as u32),
            SocksAddr::Domain(domain, port) => (domain.clone(), *port as u32),
        };

        let mut req = UdpReq {
            auth: self.password.clone(),
            payload: Some(buf.to_vec()),
            dst_addr: Some(addr),
            dst_port: Some(port),
            src_addr: Some("0.0.0.0".to_string()),
            src_port: Some(0),
        };

        req.auth = encrypt_field(&req.auth, &self.password)?;
        if let Some(value) = &req.dst_addr {
            req.dst_addr = Some(encrypt_field(value, &self.password)?);
        }
        if let Some(value) = &req.src_addr {
            req.src_addr = Some(encrypt_field(value, &self.password)?);
        }
        if let Some(payload) = &req.payload {
            req.payload = Some(encrypt_bytes(payload, &self.password)?);
        }

        write_frame(&mut self.writer, &req).await?;
        Ok(buf.len())
    }

    async fn close(&mut self) -> io::Result<()> {
        self.writer.shutdown().await
    }
}

#[cfg(feature = "outbound-rog")]
use std::io;
#[cfg(feature = "outbound-rog")]
use std::sync::Arc;

#[cfg(feature = "outbound-rog")]
use async_trait::async_trait;
#[cfg(feature = "outbound-rog")]
use tokio::sync::{mpsc, Mutex, Notify};
#[cfg(feature = "outbound-rog")]
use tokio_stream::wrappers::ReceiverStream;
#[cfg(feature = "outbound-rog")]
use tonic::{Request, Streaming};

#[cfg(feature = "outbound-rog")]
use crate::proxy::rog::protocol::rog::{
    rog_service_client::RogServiceClient, UdpReq, UdpRes,
};
#[cfg(feature = "outbound-rog")]
use crate::proxy::*;
#[cfg(feature = "outbound-rog")]
use crate::session::SocksAddr;

#[cfg(feature = "outbound-rog")]
pub struct RogDatagramRecvHalf {
    rx: Arc<Mutex<Streaming<UdpRes>>>,
    shutdown: Arc<Notify>,
}

#[cfg(feature = "outbound-rog")]
pub struct RogDatagramSendHalf {
    tx: mpsc::Sender<UdpReq>,
    password: String,
    shutdown: Arc<Notify>,
}

#[cfg(feature = "outbound-rog")]
pub struct RogDatagram {
    recv_half: RogDatagramRecvHalf,
    send_half: RogDatagramSendHalf,
}

#[cfg(feature = "outbound-rog")]
impl RogDatagram {
    pub async fn new(
        mut client: RogServiceClient<tonic::transport::Channel>,
        password: String,
    ) -> io::Result<Self> {
        let (tx, rx) = mpsc::channel::<UdpReq>(32);
        let rx_stream = ReceiverStream::new(rx);
        let request = Request::new(rx_stream);

        let response = client
            .udp(request)
            .await
            .map_err(|e|{
                io::Error::new(io::ErrorKind::Other, e)
            })?;

        let response_stream = response.into_inner();
        let shutdown = Arc::new(Notify::new());

        Ok(RogDatagram {
            recv_half: RogDatagramRecvHalf {
                rx: Arc::new(Mutex::new(response_stream)),
                shutdown: shutdown.clone(),
            },
            send_half: RogDatagramSendHalf {
                tx: tx,
                password,
                shutdown,
            },
        })
    }
}

#[cfg(feature = "outbound-rog")]
impl OutboundDatagram for RogDatagram {
    fn split(
        self: Box<Self>,
    ) -> (
        Box<dyn OutboundDatagramRecvHalf>,
        Box<dyn OutboundDatagramSendHalf>,
    ) {
        (
            Box::new(self.recv_half),
            Box::new(self.send_half),
        )
    }
}

#[cfg(feature = "outbound-rog")]
#[async_trait]
impl OutboundDatagramRecvHalf for RogDatagramRecvHalf {
    async fn recv_from(&mut self, buf: &mut [u8]) -> io::Result<(usize, SocksAddr)> {
        let mut rx_guard = self.rx.lock().await;
        tokio::select! {
            _ = self.shutdown.notified() => {
                tracing::trace!("rog client udp downlink closed");
                return Err(io::Error::new(io::ErrorKind::BrokenPipe, "closed by sender"));
            }
            res = rx_guard.message() => {
                match res {
                    Ok(Some(response)) => {
                        let data_len = std::cmp::min(response.payload.len(), buf.len());
                        buf[..data_len].copy_from_slice(&response.payload[..data_len]);

                        let src_addr = if let (Some(addr), Some(port)) = (&response.src_addr, response.src_port) {
                            if let Ok(ip) = addr.parse() {
                                SocksAddr::Ip(std::net::SocketAddr::new(ip, port as u16))
                            } else {
                                SocksAddr::Domain(addr.clone(), port as u16)
                            }
                        } else {
                            return Err(io::Error::new(io::ErrorKind::Other, "Invalid response address"));
                        };

                        Ok((data_len, src_addr))
                    }
                    Ok(None) => Err(io::Error::new(io::ErrorKind::UnexpectedEof, "Stream ended")),
                    Err(e) => Err(io::Error::new(io::ErrorKind::Other, e)),
                }
            }
        }
    }
}

#[cfg(feature = "outbound-rog")]
#[async_trait]
impl OutboundDatagramSendHalf for RogDatagramSendHalf {
    async fn send_to(&mut self, buf: &[u8], dst_addr: &SocksAddr) -> io::Result<usize> {
        let (addr_str, port) = match dst_addr {
            SocksAddr::Ip(socket_addr) => (socket_addr.ip().to_string(), socket_addr.port() as u32),
            SocksAddr::Domain(domain, port) => (domain.clone(), *port as u32),
        };

        let req = UdpReq {
            auth: self.password.clone(),
            payload: Some(buf.to_vec()),
            dst_addr: Some(addr_str),
            dst_port: Some(port),
            src_addr: Some("0.0.0.0".to_string()), // Will be set by server
            src_port: Some(0), // Will be set by server
        };

        self.tx.send(req).await.map_err(|e| {
            io::Error::new(
                io::ErrorKind::Other,
                format!("Failed to send UDP packet: {}", e),
            )
        })?;
        Ok(buf.len())
    }

    async fn close(&mut self) -> io::Result<()> {
        self.shutdown.notify_waiters();
        tracing::trace!("rog client udp uplink closed");
        Ok(())
    }
}
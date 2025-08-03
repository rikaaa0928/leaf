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
        tracing::error!("[ROG-DATAGRAM] Creating UDP channel with buffer size 32");
        let (tx, rx) = mpsc::channel::<UdpReq>(32);
        let rx_stream = ReceiverStream::new(rx);
        let request = Request::new(rx_stream);

        tracing::error!("[ROG-DATAGRAM] Establishing UDP stream with ROG server");
        let response = client
            .udp(request)
            .await
            .map_err(|e|{
                tracing::error!("[ROG-DATAGRAM] Failed to establish UDP stream with ROG server: error={}", e);
                io::Error::new(io::ErrorKind::Other, e)
            })?;

        tracing::error!("[ROG-DATAGRAM] UDP stream established successfully with ROG server");
        let response_stream = response.into_inner();
        let shutdown = Arc::new(Notify::new());

        tracing::error!("[ROG-DATAGRAM] ROG datagram created successfully");
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
        tracing::error!("[ROG-RECV] Waiting for UDP response from ROG server");
        tokio::select! {
            _ = self.shutdown.notified() => {
                tracing::error!("[ROG-RECV] ROG client UDP downlink closed by shutdown signal");
                return Err(io::Error::new(io::ErrorKind::BrokenPipe, "closed by sender"));
            }
            res = rx_guard.message() => {
                match res {
                    Ok(Some(response)) => {
                        let data_len = std::cmp::min(response.payload.len(), buf.len());
                        tracing::error!("[ROG-RECV] Received UDP response from ROG server: payload_size={}, buffer_size={}, actual_size={}", response.payload.len(), buf.len(), data_len);
                        buf[..data_len].copy_from_slice(&response.payload[..data_len]);

                        let src_addr = if let (Some(addr), Some(port)) = (&response.src_addr, response.src_port) {
                            tracing::error!("[ROG-RECV] Parsing response source address: addr={}, port={}", addr, port);
                            if let Ok(ip) = addr.parse() {
                                let addr = SocksAddr::Ip(std::net::SocketAddr::new(ip, port as u16));
                                tracing::error!("[ROG-RECV] Source address parsed as IP: {}", addr);
                                addr
                            } else {
                                let addr = SocksAddr::Domain(addr.clone(), port as u16);
                                tracing::error!("[ROG-RECV] Source address parsed as domain: {}", addr);
                                addr
                            }
                        } else {
                            tracing::error!("[ROG-RECV] Invalid response address in UDP response: src_addr={:?}, src_port={:?}", response.src_addr, response.src_port);
                            return Err(io::Error::new(io::ErrorKind::Other, "Invalid response address"));
                        };

                        tracing::error!("[ROG-RECV] UDP response processed successfully: src={}, size={} bytes", src_addr, data_len);
                        Ok((data_len, src_addr))
                    }
                    Ok(None) => {
                        tracing::error!("[ROG-RECV] ROG server stream ended unexpectedly");
                        Err(io::Error::new(io::ErrorKind::UnexpectedEof, "Stream ended"))
                    },
                    Err(e) => {
                        tracing::error!("[ROG-RECV] Error receiving from ROG server stream: error={}", e);
                        Err(io::Error::new(io::ErrorKind::Other, e))
                    },
                }
            }
        }
    }
}

#[cfg(feature = "outbound-rog")]
#[async_trait]
impl OutboundDatagramSendHalf for RogDatagramSendHalf {
    async fn send_to(&mut self, buf: &[u8], dst_addr: &SocksAddr) -> io::Result<usize> {
        tracing::error!("[ROG-SEND] Preparing to send UDP packet: dst={}, size={} bytes", dst_addr, buf.len());
        
        let (addr_str, port) = match dst_addr {
            SocksAddr::Ip(socket_addr) => {
                let addr = socket_addr.ip().to_string();
                let port = socket_addr.port() as u32;
                tracing::error!("[ROG-SEND] Destination is IP address: {}:{}", addr, port);
                (addr, port)
            },
            SocksAddr::Domain(domain, port) => {
                let port = *port as u32;
                tracing::error!("[ROG-SEND] Destination is domain: {}:{}", domain, port);
                (domain.clone(), port)
            },
        };

        let req = UdpReq {
            auth: self.password.clone(),
            payload: Some(buf.to_vec()),
            dst_addr: Some(addr_str.clone()),
            dst_port: Some(port),
            src_addr: Some("0.0.0.0".to_string()), // Will be set by server
            src_port: Some(0), // Will be set by server
        };

        tracing::error!("[ROG-SEND] Sending UDP request to ROG server: dst={}:{}, size={} bytes", addr_str, port, buf.len());
        self.tx.send(req).await.map_err(|e| {
            tracing::error!("[ROG-SEND] Failed to send UDP packet to ROG server: dst={}:{}, error={}", addr_str, port, e);
            io::Error::new(
                io::ErrorKind::Other,
                format!("Failed to send UDP packet: {}", e),
            )
        })?;
        tracing::error!("[ROG-SEND] UDP packet sent successfully to ROG server: dst={}:{}, size={} bytes", addr_str, port, buf.len());
        Ok(buf.len())
    }

    async fn close(&mut self) -> io::Result<()> {
        tracing::error!("[ROG-SEND] Closing ROG UDP uplink");
        self.shutdown.notify_waiters();
        tracing::error!("[ROG-SEND] ROG UDP uplink closed successfully");
        Ok(())
    }
}
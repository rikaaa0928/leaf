#[cfg(feature = "outbound-rog-h3")]
use std::io::{self, ErrorKind};
#[cfg(feature = "outbound-rog-h3")]
use std::sync::Arc;

#[cfg(feature = "outbound-rog-h3")]
use async_trait::async_trait;
#[cfg(feature = "outbound-rog-h3")]
use tokio::sync::Notify;

#[cfg(feature = "outbound-rog-h3")]
use crate::proxy::rog_h3::client::{ClientRecvBody, ClientSendBody};
#[cfg(feature = "outbound-rog-h3")]
use crate::proxy::rog_h3::frame::{decode_udp_packet_frame, encode_udp_packet_frame, LpmDecoder};
#[cfg(feature = "outbound-rog-h3")]
use crate::proxy::*;
#[cfg(feature = "outbound-rog-h3")]
use crate::session::SocksAddr;

#[cfg(feature = "outbound-rog-h3")]
pub struct RogH3DatagramRecvHalf {
    recv: ClientRecvBody,
    decoder: LpmDecoder,
    last_dst: Option<(String, u16)>,
    shutdown: Arc<Notify>,
}

#[cfg(feature = "outbound-rog-h3")]
pub struct RogH3DatagramSendHalf {
    send: ClientSendBody,
    last_dst: Option<(String, u16)>,
    shutdown: Arc<Notify>,
}

#[cfg(feature = "outbound-rog-h3")]
pub struct RogH3Datagram {
    recv_half: RogH3DatagramRecvHalf,
    send_half: RogH3DatagramSendHalf,
}

#[cfg(feature = "outbound-rog-h3")]
impl RogH3Datagram {
    pub fn new(recv: ClientRecvBody, send: ClientSendBody) -> Self {
        let shutdown = Arc::new(Notify::new());
        Self {
            recv_half: RogH3DatagramRecvHalf {
                recv,
                decoder: LpmDecoder::new(),
                last_dst: None,
                shutdown: shutdown.clone(),
            },
            send_half: RogH3DatagramSendHalf {
                send,
                last_dst: None,
                shutdown,
            },
        }
    }
}

#[cfg(feature = "outbound-rog-h3")]
impl OutboundDatagram for RogH3Datagram {
    fn split(
        self: Box<Self>,
    ) -> (
        Box<dyn OutboundDatagramRecvHalf>,
        Box<dyn OutboundDatagramSendHalf>,
    ) {
        (Box::new(self.recv_half), Box::new(self.send_half))
    }
}

#[cfg(feature = "outbound-rog-h3")]
#[async_trait]
impl OutboundDatagramRecvHalf for RogH3DatagramRecvHalf {
    async fn recv_from(&mut self, buf: &mut [u8]) -> io::Result<(usize, SocksAddr)> {
        loop {
            if let Some(frame) = self.decoder.next_frame()? {
                let (src_addr, payload) = decode_udp_packet_frame(frame, &mut self.last_dst)?;
                let data_len = payload.len().min(buf.len());
                buf[..data_len].copy_from_slice(&payload[..data_len]);
                return Ok((data_len, src_addr));
            }

            tokio::select! {
                _ = self.shutdown.notified() => {
                    tracing::trace!("rog-h3 client udp downlink closed");
                    return Err(io::Error::new(ErrorKind::BrokenPipe, "closed by sender"));
                }
                res = self.recv.recv_chunk() => {
                    match res? {
                        Some(chunk) => {
                            if !chunk.is_empty() {
                                self.decoder.push(&chunk);
                            }
                        }
                        None => {
                            return Err(io::Error::new(
                                ErrorKind::UnexpectedEof,
                                "rog-h3 UDP stream ended",
                            ));
                        }
                    }
                }
            }
        }
    }
}

#[cfg(feature = "outbound-rog-h3")]
#[async_trait]
impl OutboundDatagramSendHalf for RogH3DatagramSendHalf {
    async fn send_to(&mut self, buf: &[u8], dst_addr: &SocksAddr) -> io::Result<usize> {
        let (dst_host, dst_port) = match dst_addr {
            SocksAddr::Ip(socket_addr) => (socket_addr.ip().to_string(), socket_addr.port()),
            SocksAddr::Domain(domain, port) => (domain.clone(), *port),
        };

        let frame = encode_udp_packet_frame(&dst_host, dst_port, &mut self.last_dst, buf)?;
        self.send.send_raw_bytes(frame).await?;
        Ok(buf.len())
    }

    async fn close(&mut self) -> io::Result<()> {
        let _ = self.send.finish().await;
        self.shutdown.notify_waiters();
        tracing::trace!("rog-h3 client udp uplink closed");
        Ok(())
    }
}

#[cfg(feature = "outbound-rog-tcp")]
use std::io;
#[cfg(feature = "outbound-rog-tcp")]
use std::pin::Pin;
#[cfg(feature = "outbound-rog-tcp")]
use std::task::{Context, Poll};

#[cfg(feature = "outbound-rog-tcp")]
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, DuplexStream, ReadBuf};

#[cfg(feature = "outbound-rog-tcp")]
use crate::proxy::rog_tcp::protocol::rog::{StreamReq, StreamRes};
#[cfg(feature = "outbound-rog-tcp")]
use crate::proxy::rog_tcp::util::{decrypt_bytes, encrypt_bytes, read_msg, write_frame};
#[cfg(feature = "outbound-rog-tcp")]
use crate::proxy::AnyStream;

#[cfg(feature = "outbound-rog-tcp")]
pub struct RogTcpStream {
    inner: DuplexStream,
}

#[cfg(feature = "outbound-rog-tcp")]
impl RogTcpStream {
    pub fn new(stream: AnyStream, password: String, encrypt: bool) -> Self {
        let (client_side, bridge_side) = tokio::io::duplex(64 * 1024);
        spawn_bridge(stream, bridge_side, password, encrypt);
        Self { inner: client_side }
    }
}

#[cfg(feature = "outbound-rog-tcp")]
fn spawn_bridge(stream: AnyStream, bridge: DuplexStream, password: String, encrypt: bool) {
    let (mut net_reader, mut net_writer) = tokio::io::split(stream);
    let (mut bridge_reader, mut bridge_writer) = tokio::io::split(bridge);
    let downlink_password = password.clone();

    tokio::spawn(async move {
        loop {
            let res: StreamRes = match read_msg(&mut net_reader).await {
                Ok(res) => res,
                Err(err) => {
                    tracing::debug!("rog_tcp downlink closed: {}", err);
                    break;
                }
            };
            let mut payload = res.payload;
            if payload.is_empty() {
                break;
            }
            if encrypt {
                payload = match decrypt_bytes(&payload, &downlink_password) {
                    Ok(payload) => payload,
                    Err(err) => {
                        tracing::debug!("rog_tcp downlink decrypt failed: {}", err);
                        break;
                    }
                };
            }
            if let Err(err) = bridge_writer.write_all(&payload).await {
                tracing::debug!("rog_tcp downlink write failed: {}", err);
                break;
            }
        }
        let _ = bridge_writer.shutdown().await;
    });

    tokio::spawn(async move {
        let mut buf = [0u8; 16 * 1024];
        loop {
            let n = match bridge_reader.read(&mut buf).await {
                Ok(n) => n,
                Err(err) => {
                    tracing::debug!("rog_tcp uplink read failed: {}", err);
                    break;
                }
            };
            if n == 0 {
                break;
            }
            let payload = if encrypt {
                match encrypt_bytes(&buf[..n], &password) {
                    Ok(payload) => payload,
                    Err(err) => {
                        tracing::debug!("rog_tcp uplink encrypt failed: {}", err);
                        break;
                    }
                }
            } else {
                buf[..n].to_vec()
            };
            let req = StreamReq {
                auth: String::new(),
                payload: Some(payload),
                dst_addr: None,
                dst_port: None,
            };
            if let Err(err) = write_frame(&mut net_writer, &req).await {
                tracing::debug!("rog_tcp uplink closed: {}", err);
                break;
            }
        }
        let _ = net_writer.shutdown().await;
    });
}

#[cfg(feature = "outbound-rog-tcp")]
impl AsyncRead for RogTcpStream {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        Pin::new(&mut self.inner).poll_read(cx, buf)
    }
}

#[cfg(feature = "outbound-rog-tcp")]
impl AsyncWrite for RogTcpStream {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<Result<usize, io::Error>> {
        Pin::new(&mut self.inner).poll_write(cx, buf)
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), io::Error>> {
        Pin::new(&mut self.inner).poll_flush(cx)
    }

    fn poll_shutdown(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Result<(), io::Error>> {
        Pin::new(&mut self.inner).poll_shutdown(cx)
    }
}

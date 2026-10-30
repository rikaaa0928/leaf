#[cfg(feature = "outbound-rog-h3")]
use std::io;
#[cfg(feature = "outbound-rog-h3")]
use std::pin::Pin;
#[cfg(feature = "outbound-rog-h3")]
use std::task::{Context, Poll};

#[cfg(feature = "outbound-rog-h3")]
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, DuplexStream, ReadBuf};

#[cfg(feature = "outbound-rog-h3")]
use crate::proxy::rog_h3::client::{ClientRecvBody, ClientSendBody};
#[cfg(feature = "outbound-rog-h3")]
use crate::proxy::rog_h3::frame::LpmDecoder;

#[cfg(feature = "outbound-rog-h3")]
pub struct RogH3Stream {
    inner: DuplexStream,
}

#[cfg(feature = "outbound-rog-h3")]
impl RogH3Stream {
    pub fn new(recv: ClientRecvBody, send: ClientSendBody) -> Self {
        let (client_side, bridge_side) = tokio::io::duplex(64 * 1024);
        spawn_bridge(recv, send, bridge_side);
        Self { inner: client_side }
    }
}

#[cfg(feature = "outbound-rog-h3")]
fn spawn_bridge(mut recv: ClientRecvBody, mut send: ClientSendBody, bridge: DuplexStream) {
    let (mut bridge_reader, mut bridge_writer) = tokio::io::split(bridge);

    // Downlink: H3/H2 LPM frames -> local stream reader
    tokio::spawn(async move {
        let mut decoder = LpmDecoder::new();
        'downlink: loop {
            loop {
                match decoder.next_frame() {
                    Ok(Some(frame)) => {
                        if frame.is_empty() {
                            continue;
                        }
                        if let Err(e) = bridge_writer.write_all(&frame).await {
                            tracing::trace!("rog-h3 downlink local write closed: {e}");
                            break 'downlink;
                        }
                    }
                    Ok(None) => break,
                    Err(e) => {
                        tracing::debug!("rog-h3 downlink LPM decode error: {e}");
                        break 'downlink;
                    }
                }
            }

            match recv.recv_chunk().await {
                Ok(Some(chunk)) => {
                    if !chunk.is_empty() {
                        decoder.push(&chunk);
                    }
                }
                Ok(None) => break,
                Err(e) => {
                    tracing::debug!("rog-h3 downlink closed: {e}");
                    break;
                }
            }
        }
        let _ = bridge_writer.shutdown().await;
    });

    // Uplink: local stream writer -> H3/H2 LPM frames
    tokio::spawn(async move {
        let mut buf = vec![0u8; 32 * 1024];
        loop {
            let n = match bridge_reader.read(&mut buf).await {
                Ok(n) => n,
                Err(e) => {
                    tracing::debug!("rog-h3 uplink local read error: {e}");
                    break;
                }
            };
            if n == 0 {
                break;
            }
            if let Err(e) = send.send_lpm_payload(&buf[..n]).await {
                tracing::debug!("rog-h3 uplink send error: {e}");
                break;
            }
        }
        let _ = send.finish().await;
    });
}

#[cfg(feature = "outbound-rog-h3")]
impl AsyncRead for RogH3Stream {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        Pin::new(&mut self.inner).poll_read(cx, buf)
    }
}

#[cfg(feature = "outbound-rog-h3")]
impl AsyncWrite for RogH3Stream {
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

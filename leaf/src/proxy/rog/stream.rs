#[cfg(feature = "outbound-rog")]
use std::io;
#[cfg(feature = "outbound-rog")]
use std::pin::Pin;
#[cfg(feature = "outbound-rog")]
use std::sync::Arc;
#[cfg(feature = "outbound-rog")]
use std::task::{Context, Poll};

#[cfg(feature = "outbound-rog")]
use futures::{Future, Stream};
#[cfg(feature = "outbound-rog")]
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
#[cfg(feature = "outbound-rog")]
use tokio::sync::{mpsc, Mutex};
#[cfg(feature = "outbound-rog")]
use tokio_stream::wrappers::ReceiverStream;
#[cfg(feature = "outbound-rog")]
use tonic::{Request, Streaming};

#[cfg(feature = "outbound-rog")]
use crate::proxy::rog::protocol::rog::{
    rog_service_client::RogServiceClient, StreamReq, StreamRes,
};

#[cfg(feature = "outbound-rog")]
pub struct RogStream {
    tx: mpsc::Sender<StreamReq>,
    rx: Arc<Mutex<Streaming<StreamRes>>>,
    read_buffer: Vec<u8>,
    read_pos: usize,
}

#[cfg(feature = "outbound-rog")]
impl RogStream {
    pub async fn new(
        mut client: RogServiceClient<tonic::transport::Channel>,
        dst_addr: String,
        dst_port: u16,
        password: String,
    ) -> io::Result<Self> {
        let (tx, rx) = mpsc::channel::<StreamReq>(32);
        let rx_stream = ReceiverStream::new(rx);
        let request = Request::new(rx_stream);

        let response = client
            .stream(request)
            .await
            .map_err(|e|{
                io::Error::new(io::ErrorKind::Other, e)
            })?;

        let response_stream = response.into_inner();

        // Send initial authentication request
        let auth_req = StreamReq {
            auth: password,
            payload: None,
            dst_addr: Some(dst_addr),
            dst_port: Some(dst_port as u32),
        };

        tx.send(auth_req).await.map_err(|e| {
            io::Error::new(io::ErrorKind::Other, format!("Failed to send auth: {}", e))
        })?;

        Ok(RogStream {
            tx,
            rx: Arc::new(Mutex::new(response_stream)),
            read_buffer: Vec::new(),
            read_pos: 0,
        })
    }
}

#[cfg(feature = "outbound-rog")]
impl AsyncRead for RogStream {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        // If we have data in our buffer, read from it first
        if self.read_pos < self.read_buffer.len() {
            let available = self.read_buffer.len() - self.read_pos;
            let to_read = std::cmp::min(available, buf.remaining());
            let end_pos = self.read_pos + to_read;
            buf.put_slice(&self.read_buffer[self.read_pos..end_pos]);
            self.read_pos = end_pos;

            // If we've consumed all buffered data, clear the buffer
            if self.read_pos >= self.read_buffer.len() {
                self.read_buffer.clear();
                self.read_pos = 0;
            }

            return Poll::Ready(Ok(()));
        }

        // No buffered data, need to read from the stream
        let rx = self.rx.clone();
        let mut rx_guard = match rx.try_lock() {
            Ok(guard) => guard,
            Err(_) => return Poll::Pending,
        };

        match Pin::new(&mut *rx_guard).poll_next(cx) {
            Poll::Ready(Some(Ok(response))) => {
                self.read_buffer = response.payload;
                self.read_pos = 0;

                if !self.read_buffer.is_empty() {
                    let to_read = std::cmp::min(self.read_buffer.len(), buf.remaining());
                    buf.put_slice(&self.read_buffer[..to_read]);
                    self.read_pos = to_read;

                    if self.read_pos >= self.read_buffer.len() {
                        self.read_buffer.clear();
                        self.read_pos = 0;
                    }
                }

                Poll::Ready(Ok(()))
            }
            Poll::Ready(Some(Err(e))) => {
                Poll::Ready(Err(io::Error::new(io::ErrorKind::Other, e)))
            }
            Poll::Ready(None) => Poll::Ready(Ok(())), // Stream ended
            Poll::Pending => Poll::Pending,
        }
    }
}

#[cfg(feature = "outbound-rog")]
impl AsyncWrite for RogStream {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<Result<usize, io::Error>> {
        let req = StreamReq {
            auth: String::new(), // Only needed for first request
            payload: Some(buf.to_vec()),
            dst_addr: None,
            dst_port: None,
        };

        let tx = &self.tx;
        let send_future = tx.send(req);
        tokio::pin!(send_future);

        match send_future.poll(cx) {
            Poll::Ready(Ok(())) => Poll::Ready(Ok(buf.len())),
            Poll::Ready(Err(e)) => {
                Poll::Ready(Err(io::Error::new(io::ErrorKind::Other, e)))
            }
            Poll::Pending => Poll::Pending,
        }
    }

    fn poll_flush(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<Result<(), io::Error>> {
        // gRPC handles flushing internally
        Poll::Ready(Ok(()))
    }

    fn poll_shutdown(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<Result<(), io::Error>> {
        // Close the sender to signal end of stream
        // The tx will be dropped when the struct is dropped
        Poll::Ready(Ok(()))
    }
}

// Streaming already implements Stream, so we don't need to implement it again
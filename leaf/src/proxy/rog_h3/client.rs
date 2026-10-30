#[cfg(feature = "outbound-rog-h3")]
use std::io::{self, ErrorKind};
#[cfg(feature = "outbound-rog-h3")]
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
#[cfg(feature = "outbound-rog-h3")]
use std::sync::atomic::{AtomicUsize, Ordering};
#[cfg(feature = "outbound-rog-h3")]
use std::sync::Arc;
#[cfg(feature = "outbound-rog-h3")]
use std::time::Duration;

#[cfg(feature = "outbound-rog-h3")]
use bytes::{Buf, Bytes};
#[cfg(feature = "outbound-rog-h3")]
use rustls::pki_types::ServerName;
#[cfg(feature = "outbound-rog-h3")]
use tokio::sync::Mutex;
#[cfg(feature = "outbound-rog-h3")]
use tokio::time::timeout;
#[cfg(feature = "outbound-rog-h3")]
use tracing::{debug, info, warn};

#[cfg(feature = "outbound-rog-h3")]
use crate::app::SyncDnsClient;
#[cfg(feature = "outbound-rog-h3")]
use crate::proxy::rog_h3::frame::{
    encode_lpm_frame, encode_tcp_handshake_frame, encode_udp_handshake_frame, H3_RAW_STREAM_PATH,
    H3_RAW_UDP_PATH,
};
#[cfg(feature = "outbound-rog-h3")]
use crate::proxy::*;

#[cfg(feature = "outbound-rog-h3")]
const DEFAULT_H3_STREAM_WINDOW: u32 = 8 * 1024 * 1024;
#[cfg(feature = "outbound-rog-h3")]
const DEFAULT_H3_CONN_WINDOW: u32 = 16 * 1024 * 1024;
#[cfg(feature = "outbound-rog-h3")]
const DEFAULT_H2_MAX_FRAME_SIZE: u32 = 64 * 1024;
#[cfg(feature = "outbound-rog-h3")]
const DEFAULT_KEEP_ALIVE_INTERVAL_SECS: u64 = 25;
#[cfg(feature = "outbound-rog-h3")]
const DEFAULT_IDLE_TIMEOUT_SECS: u64 = 45;

#[cfg(feature = "outbound-rog-h3")]
#[derive(Clone, Copy, Debug)]
pub struct ClientOptions {
    pub keep_alive: bool,
    pub keep_alive_interval_secs: u64,
    pub keep_alive_timeout_secs: u64,
    pub keep_alive_while_idle: bool,
}

#[cfg(feature = "outbound-rog-h3")]
impl ClientOptions {
    pub fn new(
        keep_alive: bool,
        keep_alive_interval_secs: u32,
        keep_alive_timeout_secs: u32,
        keep_alive_while_idle: Option<bool>,
    ) -> Self {
        let default_while_idle = !cfg!(any(target_os = "android", target_os = "ios"));
        Self {
            keep_alive,
            keep_alive_interval_secs: if keep_alive_interval_secs == 0 {
                DEFAULT_KEEP_ALIVE_INTERVAL_SECS
            } else {
                u64::from(keep_alive_interval_secs)
            },
            keep_alive_timeout_secs: if keep_alive_timeout_secs == 0 {
                DEFAULT_IDLE_TIMEOUT_SECS
            } else {
                u64::from(keep_alive_timeout_secs)
            },
            keep_alive_while_idle: keep_alive_while_idle.unwrap_or(default_while_idle),
        }
    }

    pub fn effective_keep_alive_interval(&self) -> Option<Duration> {
        if self.keep_alive && self.keep_alive_while_idle && self.keep_alive_interval_secs > 0 {
            Some(Duration::from_secs(self.keep_alive_interval_secs))
        } else {
            None
        }
    }

    pub fn effective_max_idle_timeout(&self) -> Duration {
        if self.effective_keep_alive_interval().is_some() {
            let min_for_keepalive = self.keep_alive_interval_secs.saturating_mul(3);
            Duration::from_secs(
                self.keep_alive_timeout_secs
                    .max(min_for_keepalive)
                    .max(60),
            )
        } else {
            Duration::from_secs(self.keep_alive_timeout_secs.max(DEFAULT_IDLE_TIMEOUT_SECS))
        }
    }
}

#[cfg(feature = "outbound-rog-h3")]
impl Default for ClientOptions {
    fn default() -> Self {
        Self::new(false, 0, 0, None)
    }
}

#[cfg(feature = "outbound-rog-h3")]
pub enum ClientSendBodyInner {
    H3(h3::client::RequestStream<h3_quinn::SendStream<Bytes>, Bytes>),
    H2(h2::SendStream<Bytes>),
}

#[cfg(feature = "outbound-rog-h3")]
pub struct ClientSendBody {
    inner: Option<ClientSendBodyInner>,
}

#[cfg(feature = "outbound-rog-h3")]
impl ClientSendBody {
    pub fn new_h3(stream: h3::client::RequestStream<h3_quinn::SendStream<Bytes>, Bytes>) -> Self {
        Self {
            inner: Some(ClientSendBodyInner::H3(stream)),
        }
    }

    pub fn new_h2(stream: h2::SendStream<Bytes>) -> Self {
        Self {
            inner: Some(ClientSendBodyInner::H2(stream)),
        }
    }

    pub async fn send_raw_bytes(&mut self, mut data: Bytes) -> io::Result<()> {
        if data.is_empty() {
            return Ok(());
        }
        match self.inner.as_mut() {
            Some(ClientSendBodyInner::H3(stream)) => stream.send_data(data).await.map_err(|e| {
                io::Error::new(ErrorKind::BrokenPipe, format!("rog-h3 send error: {e}"))
            }),
            Some(ClientSendBodyInner::H2(stream)) => {
                while !data.is_empty() {
                    let want = data.len();
                    stream.reserve_capacity(want);
                    match std::future::poll_fn(|cx| stream.poll_capacity(cx)).await {
                        Some(Ok(cap)) => {
                            let chunk = data.split_to(want.min(cap));
                            stream.send_data(chunk, false).map_err(|e| {
                                io::Error::new(
                                    ErrorKind::BrokenPipe,
                                    format!("rog-h3 (h2) send error: {e}"),
                                )
                            })?;
                        }
                        Some(Err(e)) => {
                            return Err(io::Error::new(
                                ErrorKind::BrokenPipe,
                                format!("rog-h3 (h2) poll_capacity error: {e}"),
                            ));
                        }
                        None => {
                            return Err(io::Error::new(
                                ErrorKind::BrokenPipe,
                                "rog-h3 (h2) send stream closed",
                            ));
                        }
                    }
                }
                Ok(())
            }
            None => Err(io::Error::new(
                ErrorKind::BrokenPipe,
                "rog-h3 client send stream already finished",
            )),
        }
    }

    pub async fn send_lpm_payload(&mut self, payload: &[u8]) -> io::Result<()> {
        if payload.is_empty() {
            return Ok(());
        }
        let frame = encode_lpm_frame(payload);
        self.send_raw_bytes(frame).await
    }

    pub async fn finish(&mut self) -> io::Result<()> {
        if let Some(inner) = self.inner.take() {
            match inner {
                ClientSendBodyInner::H3(mut stream) => {
                    let _ = stream.finish().await;
                }
                ClientSendBodyInner::H2(mut stream) => {
                    let _ = stream.send_data(Bytes::new(), true);
                }
            }
        }
        Ok(())
    }
}

#[cfg(feature = "outbound-rog-h3")]
impl Drop for ClientSendBody {
    fn drop(&mut self) {
        if let Some(inner) = self.inner.take() {
            match inner {
                ClientSendBodyInner::H3(mut stream) => {
                    if let Ok(handle) = tokio::runtime::Handle::try_current() {
                        handle.spawn(async move {
                            let _ = stream.finish().await;
                        });
                    }
                }
                ClientSendBodyInner::H2(mut stream) => {
                    let _ = stream.send_data(Bytes::new(), true);
                }
            }
        }
    }
}

#[cfg(feature = "outbound-rog-h3")]
pub enum ClientRecvBody {
    H3 {
        stream: h3::client::RequestStream<h3_quinn::RecvStream, Bytes>,
        headers_received: bool,
    },
    H2 {
        response_fut: Option<h2::client::ResponseFuture>,
        body: Option<h2::RecvStream>,
    },
}

#[cfg(feature = "outbound-rog-h3")]
impl Drop for ClientRecvBody {
    fn drop(&mut self) {
        if let ClientRecvBody::H2 { body, .. } = self {
            if let Some(mut stream) = body.take() {
                if !stream.is_end_stream() {
                    if let Ok(handle) = tokio::runtime::Handle::try_current() {
                        handle.spawn(async move {
                            let _ = timeout(Duration::from_secs(5), async move {
                                while let Some(Ok(chunk)) = stream.data().await {
                                    let _ = stream.flow_control().release_capacity(chunk.len());
                                }
                            })
                            .await;
                        });
                    }
                }
            }
        }
    }
}

#[cfg(feature = "outbound-rog-h3")]
impl ClientRecvBody {
    pub async fn recv_chunk(&mut self) -> io::Result<Option<Bytes>> {
        match self {
            ClientRecvBody::H3 {
                stream,
                headers_received,
            } => {
                if !*headers_received {
                    let resp = stream.recv_response().await.map_err(|e| {
                        io::Error::new(
                            ErrorKind::ConnectionAborted,
                            format!("rog-h3 recv_response error: {e}"),
                        )
                    })?;
                    if resp.status() != ::http::StatusCode::OK {
                        return Err(io::Error::new(
                            ErrorKind::PermissionDenied,
                            format!("rog-h3 server returned HTTP {}", resp.status()),
                        ));
                    }
                    *headers_received = true;
                }
                match stream.recv_data().await {
                    Ok(Some(mut buf)) => {
                        let len = buf.remaining();
                        if len == 0 {
                            Ok(Some(Bytes::new()))
                        } else {
                            Ok(Some(buf.copy_to_bytes(len)))
                        }
                    }
                    Ok(None) => Ok(None),
                    Err(e) => Err(io::Error::new(
                        ErrorKind::ConnectionReset,
                        format!("rog-h3 recv_data error: {e}"),
                    )),
                }
            }
            ClientRecvBody::H2 { response_fut, body } => {
                if body.is_none() {
                    let fut = response_fut
                        .take()
                        .ok_or_else(|| io::Error::other("missing h2 response future"))?;
                    let resp = fut.await.map_err(|e| {
                        io::Error::new(
                            ErrorKind::ConnectionAborted,
                            format!("rog-h3 (h2) response header error: {e}"),
                        )
                    })?;
                    if resp.status() != ::http::StatusCode::OK {
                        return Err(io::Error::new(
                            ErrorKind::PermissionDenied,
                            format!("rog-h3 (h2) server returned HTTP {}", resp.status()),
                        ));
                    }
                    *body = Some(resp.into_body());
                }
                let recv_stream = body.as_mut().unwrap();
                match recv_stream.data().await {
                    Some(Ok(chunk)) => {
                        let _ = recv_stream.flow_control().release_capacity(chunk.len());
                        Ok(Some(chunk))
                    }
                    Some(Err(e)) => Err(io::Error::new(
                        ErrorKind::ConnectionReset,
                        format!("rog-h3 (h2) recv data error: {e}"),
                    )),
                    None => Ok(None),
                }
            }
        }
    }
}

#[cfg(feature = "outbound-rog-h3")]
#[derive(Clone)]
enum ActiveClientConn {
    H3(h3::client::SendRequest<h3_quinn::OpenStreams, Bytes>),
    H2(h2::client::SendRequest<Bytes>),
}

#[cfg(feature = "outbound-rog-h3")]
pub struct RogH3Connector {
    host: String,
    port: u16,
    authority: String,
    password: String,
    custom_connector: bool,
    dns_client: SyncDnsClient,
    quic_client_config: quinn::ClientConfig,
    tls_client_config_h2: Arc<rustls::ClientConfig>,
    pool: Vec<Arc<Mutex<Option<ActiveClientConn>>>>,
    rr_index: AtomicUsize,
}

#[cfg(feature = "outbound-rog-h3")]
impl UdpConnector for RogH3Connector {}
#[cfg(feature = "outbound-rog-h3")]
impl TcpConnector for RogH3Connector {}

#[cfg(feature = "outbound-rog-h3")]
impl RogH3Connector {
    pub fn new(
        address: String,
        port: u16,
        password: String,
        custom_connector: bool,
        client_options: ClientOptions,
        dns_client: SyncDnsClient,
    ) -> io::Result<Self> {
        let host = address.trim_matches(['[', ']']).to_string();
        if host.is_empty() {
            return Err(io::Error::new(
                ErrorKind::InvalidInput,
                "rog-h3 address cannot be empty",
            ));
        }
        let authority = if host.contains(':') {
            format!("[{host}]:{port}")
        } else {
            format!("{host}:{port}")
        };

        let tls_h3 = Self::build_rustls_client_config(vec![b"h3".to_vec()])?;
        let tls_h2 = Arc::new(Self::build_rustls_client_config(vec![b"h2".to_vec()])?);

        let quic_tls = quinn::crypto::rustls::QuicClientConfig::try_from(tls_h3)
            .map_err(|e| io::Error::new(ErrorKind::InvalidInput, format!("quic tls error: {e}")))?;
        let mut quic_client_config = quinn::ClientConfig::new(Arc::new(quic_tls));

        let mut transport = quinn::TransportConfig::default();
        let idle_timeout = client_options.effective_max_idle_timeout();
        transport.max_idle_timeout(idle_timeout.try_into().ok());
        transport.keep_alive_interval(client_options.effective_keep_alive_interval());
        transport.receive_window(quinn::VarInt::from_u32(DEFAULT_H3_CONN_WINDOW));
        transport.stream_receive_window(quinn::VarInt::from_u32(DEFAULT_H3_STREAM_WINDOW));
        transport.send_window(u64::from(DEFAULT_H3_CONN_WINDOW));
        transport.congestion_controller_factory(Arc::new(quinn::congestion::BbrConfig::default()));
        quic_client_config.transport_config(Arc::new(transport));

        let pool_size = if cfg!(any(target_os = "android", target_os = "ios")) {
            1
        } else {
            2
        };
        let mut pool = Vec::with_capacity(pool_size);
        for _ in 0..pool_size {
            pool.push(Arc::new(Mutex::new(None)));
        }

        Ok(Self {
            host,
            port,
            authority,
            password,
            custom_connector,
            dns_client,
            quic_client_config,
            tls_client_config_h2: tls_h2,
            pool,
            rr_index: AtomicUsize::new(0),
        })
    }

    fn build_rustls_client_config(alpn: Vec<Vec<u8>>) -> io::Result<rustls::ClientConfig> {
        let mut roots = rustls::RootCertStore::empty();
        roots.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());

        #[cfg(feature = "rustls-tls-aws-lc")]
        let provider = Arc::new(rustls::crypto::aws_lc_rs::default_provider());
        #[cfg(not(feature = "rustls-tls-aws-lc"))]
        let provider = Arc::new(rustls::crypto::ring::default_provider());

        let mut cfg = rustls::ClientConfig::builder_with_provider(provider)
            .with_safe_default_protocol_versions()
            .map_err(|e| io::Error::new(ErrorKind::InvalidInput, e.to_string()))?
            .with_root_certificates(roots)
            .with_no_client_auth();
        cfg.alpn_protocols = alpn;
        Ok(cfg)
    }

    async fn resolve_target_addrs(&self) -> io::Result<Vec<SocketAddr>> {
        if let Ok(ip) = self.host.parse::<IpAddr>() {
            return Ok(vec![SocketAddr::new(ip, self.port)]);
        }
        if self.custom_connector {
            let ips = self
                .dns_client
                .read()
                .await
                .direct_lookup(&self.host)
                .await
                .map_err(|e| {
                    io::Error::new(
                        ErrorKind::NotFound,
                        format!("rog-h3 lookup '{}' failed: {e}", self.host),
                    )
                })?;
            if ips.is_empty() {
                return Err(io::Error::new(
                    ErrorKind::NotFound,
                    format!("rog-h3 resolved no IPs for '{}'", self.host),
                ));
            }
            Ok(ips
                .into_iter()
                .map(|ip| SocketAddr::new(ip, self.port))
                .collect())
        } else {
            let addrs: Vec<SocketAddr> =
                tokio::net::lookup_host((self.host.as_str(), self.port)).await?.collect();
            if addrs.is_empty() {
                return Err(io::Error::new(
                    ErrorKind::NotFound,
                    format!("rog-h3 system lookup failed for '{}'", self.host),
                ));
            }
            Ok(addrs)
        }
    }

    async fn establish_quinn_h3(&self, addrs: &[SocketAddr]) -> io::Result<ActiveClientConn> {
        let dial_timeout = Duration::from_secs(*crate::option::OUTBOUND_DIAL_TIMEOUT);
        let mut last_err = None;

        for &remote_addr in addrs {
            let bind_indicator = if remote_addr.is_ipv4() {
                SocketAddr::new(IpAddr::V4(Ipv4Addr::UNSPECIFIED), 0)
            } else {
                SocketAddr::new(IpAddr::V6(Ipv6Addr::UNSPECIFIED), 0)
            };

            let udp_socket = match self.new_udp_socket(&bind_indicator).await {
                Ok(s) => s,
                Err(e) => {
                    last_err = Some(e);
                    continue;
                }
            };
            let std_socket = match udp_socket.into_std() {
                Ok(s) => s,
                Err(e) => {
                    last_err = Some(e);
                    continue;
                }
            };

            let mut endpoint = match quinn::Endpoint::new(
                quinn::EndpointConfig::default(),
                None,
                std_socket,
                Arc::new(quinn::TokioRuntime),
            ) {
                Ok(ep) => ep,
                Err(e) => {
                    last_err = Some(io::Error::other(format!("quinn endpoint error: {e}")));
                    continue;
                }
            };
            endpoint.set_default_client_config(self.quic_client_config.clone());

            let connecting = match endpoint.connect(remote_addr, &self.host) {
                Ok(c) => c,
                Err(e) => {
                    last_err = Some(io::Error::other(format!("quinn connect init error: {e}")));
                    continue;
                }
            };

            match timeout(dial_timeout, connecting).await {
                Ok(Ok(quinn_conn)) => {
                    let h3_quinn_conn = h3_quinn::Connection::new(quinn_conn);
                    let (mut driver, send_request) =
                        match timeout(dial_timeout, h3::client::new(h3_quinn_conn)).await {
                            Ok(Ok(pair)) => pair,
                            Ok(Err(e)) => {
                                last_err = Some(io::Error::other(format!(
                                    "h3 client handshake error: {e}"
                                )));
                                continue;
                            }
                            Err(_) => {
                                last_err = Some(io::Error::new(
                                    ErrorKind::TimedOut,
                                    "h3 client handshake timed out",
                                ));
                                continue;
                            }
                        };

                    tokio::spawn(async move {
                        let _keep_endpoint_alive = endpoint;
                        let _ = std::future::poll_fn(|cx| driver.poll_close(cx)).await;
                    });

                    info!(
                        "rog-h3 (QUIC) connection established to {} ({})",
                        self.authority, remote_addr
                    );
                    return Ok(ActiveClientConn::H3(send_request));
                }
                Ok(Err(e)) => {
                    last_err = Some(io::Error::other(format!(
                        "quinn handshake to {} failed: {e}",
                        remote_addr
                    )));
                }
                Err(_) => {
                    last_err = Some(io::Error::new(
                        ErrorKind::TimedOut,
                        format!("quinn handshake to {} timed out", remote_addr),
                    ));
                }
            }
        }

        Err(last_err.unwrap_or_else(|| io::Error::other("no usable address for QUIC H3")))
    }

    async fn establish_h2(&self, addrs: &[SocketAddr]) -> io::Result<ActiveClientConn> {
        let dial_timeout = Duration::from_secs(*crate::option::OUTBOUND_DIAL_TIMEOUT);
        let tcp_stream: AnyStream = if self.custom_connector {
            self.new_tcp_stream(self.dns_client.clone(), &self.host, &self.port)
                .await?
        } else {
            let mut last_err = None;
            let mut connected = None;
            for &remote_addr in addrs {
                match timeout(dial_timeout, tokio::net::TcpStream::connect(remote_addr)).await {
                    Ok(Ok(s)) => {
                        let _ = s.set_nodelay(true);
                        connected = Some(Box::new(s) as AnyStream);
                        break;
                    }
                    Ok(Err(e)) => last_err = Some(e),
                    Err(_) => {
                        last_err = Some(io::Error::new(
                            ErrorKind::TimedOut,
                            format!("TCP connect to {} timed out", remote_addr),
                        ));
                    }
                }
            }
            match connected {
                Some(s) => s,
                None => {
                    return Err(
                        last_err.unwrap_or_else(|| io::Error::other("no usable address for H2"))
                    )
                }
            }
        };

        let mut h2_builder = h2::client::Builder::new();
        h2_builder
            .initial_window_size(DEFAULT_H3_STREAM_WINDOW)
            .initial_connection_window_size(DEFAULT_H3_CONN_WINDOW)
            .max_frame_size(DEFAULT_H2_MAX_FRAME_SIZE);

        let connector = tokio_rustls::TlsConnector::from(self.tls_client_config_h2.clone());
        let server_name = ServerName::try_from(self.host.clone())
            .map_err(|_| io::Error::new(ErrorKind::InvalidInput, "invalid TLS server name"))?;
        let tls_stream = timeout(dial_timeout, connector.connect(server_name, tcp_stream))
            .await
            .map_err(|_| io::Error::new(ErrorKind::TimedOut, "TLS handshake timeout"))?
            .map_err(|e| io::Error::other(format!("TLS handshake error: {e}")))?;

        let (sender, conn) = timeout(dial_timeout, h2_builder.handshake(tls_stream))
            .await
            .map_err(|_| io::Error::new(ErrorKind::TimedOut, "h2 handshake timeout"))?
            .map_err(|e| io::Error::other(format!("h2 handshake error: {e}")))?;

        tokio::spawn(async move {
            if let Err(e) = conn.await {
                debug!("rog-h3 H2 fallback connection closed: {e}");
            }
        });

        info!(
            "rog-h3 H2 (TLS fallback) connection established to {}",
            self.authority
        );
        Ok(ActiveClientConn::H2(sender))
    }

    async fn create_new_connection(&self) -> io::Result<ActiveClientConn> {
        let addrs = self.resolve_target_addrs().await?;
        match self.establish_quinn_h3(&addrs).await {
            Ok(conn) => Ok(conn),
            Err(h3_err) => {
                warn!(
                    "rog-h3 QUIC connection to {} failed ({}), falling back to HTTP/2 over TLS",
                    self.authority, h3_err
                );
                self.establish_h2(&addrs).await
            }
        }
    }

    fn build_post_request(&self, path: &str) -> io::Result<::http::Request<()>> {
        let uri = format!("https://{}{path}", self.authority);
        ::http::Request::builder()
            .method(::http::Method::POST)
            .uri(uri)
            .header(::http::header::CONTENT_TYPE, "application/grpc")
            .header(::http::header::TE, "trailers")
            .body(())
            .map_err(|e| io::Error::new(ErrorKind::InvalidInput, e.to_string()))
    }

    async fn try_open_on_conn(
        &self,
        conn: &mut ActiveClientConn,
        path: &str,
    ) -> io::Result<(ClientRecvBody, ClientSendBody)> {
        let req = self.build_post_request(path)?;
        match conn {
            ActiveClientConn::H3(sender) => {
                let stream = sender.send_request(req).await.map_err(|e| {
                    io::Error::new(
                        ErrorKind::ConnectionReset,
                        format!("h3 send_request failed: {e}"),
                    )
                })?;
                let (send, recv) = stream.split();
                Ok((
                    ClientRecvBody::H3 {
                        stream: recv,
                        headers_received: false,
                    },
                    ClientSendBody::new_h3(send),
                ))
            }
            ActiveClientConn::H2(sender) => {
                std::future::poll_fn(|cx| sender.poll_ready(cx))
                    .await
                    .map_err(|e| {
                        io::Error::new(
                            ErrorKind::ConnectionReset,
                            format!("h2 poll_ready failed: {e}"),
                        )
                    })?;
                let (resp_fut, send_stream) = sender.send_request(req, false).map_err(|e| {
                    io::Error::new(
                        ErrorKind::ConnectionReset,
                        format!("h2 send_request failed: {e}"),
                    )
                })?;
                Ok((
                    ClientRecvBody::H2 {
                        response_fut: Some(resp_fut),
                        body: None,
                    },
                    ClientSendBody::new_h2(send_stream),
                ))
            }
        }
    }

    pub async fn open_bidi_stream(&self, path: &str) -> io::Result<(ClientRecvBody, ClientSendBody)> {
        let idx = self.rr_index.fetch_add(1, Ordering::Relaxed) % self.pool.len();
        let slot = &self.pool[idx];

        let existing = { slot.lock().await.clone() };
        if let Some(mut conn) = existing {
            if let Ok(pair) = self.try_open_on_conn(&mut conn, path).await {
                return Ok(pair);
            }
        }

        let mut guard = slot.lock().await;
        if let Some(ref mut conn) = *guard {
            if let Ok(pair) = self.try_open_on_conn(conn, path).await {
                return Ok(pair);
            }
        }

        let mut fresh_conn = self.create_new_connection().await?;
        let pair = self.try_open_on_conn(&mut fresh_conn, path).await?;
        *guard = Some(fresh_conn);
        Ok(pair)
    }

    pub async fn connect_tcp_stream(
        &self,
        dst_host: &str,
        dst_port: u16,
    ) -> io::Result<(ClientRecvBody, ClientSendBody)> {
        let (recv, mut send) = self.open_bidi_stream(H3_RAW_STREAM_PATH).await?;
        let hs_frame = encode_tcp_handshake_frame(&self.password, dst_host, dst_port, &[])?;
        send.send_raw_bytes(hs_frame).await?;
        Ok((recv, send))
    }

    pub async fn connect_udp_tunnel(
        &self,
        src_host: &str,
        src_port: u16,
    ) -> io::Result<(ClientRecvBody, ClientSendBody)> {
        let (recv, mut send) = self.open_bidi_stream(H3_RAW_UDP_PATH).await?;
        let hs_frame = encode_udp_handshake_frame(&self.password, src_host, src_port)?;
        send.send_raw_bytes(hs_frame).await?;
        Ok((recv, send))
    }
}

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use futures::future::{abortable, BoxFuture};
use tokio::sync::{
    mpsc::{self, Sender},
    oneshot, Mutex, MutexGuard,
};
use tracing::{debug, error, trace};

use crate::app::dispatcher::Dispatcher;
use crate::option;
use crate::session::{DatagramSource, Network, Session, SocksAddr};

#[derive(Debug)]
pub struct UdpPacket {
    pub data: Vec<u8>,
    pub src_addr: SocksAddr,
    pub dst_addr: SocksAddr,
}

impl UdpPacket {
    pub fn new(data: Vec<u8>, src_addr: SocksAddr, dst_addr: SocksAddr) -> Self {
        Self {
            data,
            src_addr,
            dst_addr,
        }
    }
}

impl std::fmt::Display for UdpPacket {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        write!(
            f,
            "{} <-> {}, {} bytes",
            self.src_addr,
            self.dst_addr,
            self.data.len()
        )
    }
}

type SessionMap = HashMap<DatagramSource, (Sender<UdpPacket>, oneshot::Sender<bool>, Instant)>;

pub struct NatManager {
    sessions: Arc<Mutex<SessionMap>>,
    dispatcher: Arc<Dispatcher>,
    timeout_check_task: Mutex<Option<BoxFuture<'static, ()>>>,
}

impl NatManager {
    pub fn new(dispatcher: Arc<Dispatcher>) -> Self {
        let sessions: Arc<Mutex<SessionMap>> = Arc::new(Mutex::new(HashMap::new()));
        let sessions2 = sessions.clone();

        // The task is lazy, will not run until any sessions added.
        let timeout_check_task: BoxFuture<'static, ()> = Box::pin(async move {
            loop {
                let mut sessions = sessions2.lock().await;
                let n_total = sessions.len();
                let now = Instant::now();
                let mut to_be_remove = Vec::new();
                for (key, val) in sessions.iter() {
                    if now.duration_since(val.2).as_secs() >= *option::UDP_SESSION_TIMEOUT {
                        to_be_remove.push(key.to_owned());
                    }
                }
                for key in to_be_remove.iter() {
                    if let Some(sess) = sessions.remove(key) {
                        // Sends a signal to abort downlink task, uplink task will
                        // end automatically when we drop the channel's tx side upon
                        // session removal.
                        if let Err(e) = sess.1.send(true) {
                            debug!("failed to send abort signal on session {}: {}", key, e);
                        }
                        debug!("udp session {} ended", key);
                    }
                }
                drop(to_be_remove); // drop explicitly
                let n_remaining = sessions.len();
                let n_removed = n_total - n_remaining;
                drop(sessions); // release the lock
                if n_removed > 0 {
                    debug!(
                        "removed {} nat sessions, remaining {} sessions",
                        n_removed, n_remaining
                    );
                }
                tokio::time::sleep(Duration::from_secs(
                    *option::UDP_SESSION_TIMEOUT_CHECK_INTERVAL,
                ))
                .await;
            }
        });

        NatManager {
            sessions,
            dispatcher,
            timeout_check_task: Mutex::new(Some(timeout_check_task)),
        }
    }

    fn _send(&self, guard: &mut MutexGuard<'_, SessionMap>, key: &DatagramSource, pkt: UdpPacket) {
        if let Some(sess) = guard.get_mut(key) {
            if let Err(err) = sess.0.try_send(pkt) {
                trace!("send uplink packet failed {}", err);
            }
            sess.2 = Instant::now(); // activity update
        } else {
            error!("no nat association found");
        }
    }

    pub async fn send<'a>(
        &self,
        sess: Option<&Session>,
        dgram_src: &DatagramSource,
        inbound_tag: &str,
        client_ch_tx: &Sender<UdpPacket>,
        pkt: UdpPacket,
    ) {
        error!("[NAT-MANAGER] Received UDP packet to send: src={}, dst={}, inbound_tag={}, size={} bytes", 
               dgram_src, pkt.dst_addr, inbound_tag, pkt.data.len());
        
        let mut guard = self.sessions.lock().await;

        if guard.contains_key(dgram_src) {
            error!("[NAT-MANAGER] Found existing session for source: src={}, dst={}", dgram_src, pkt.dst_addr);
            self._send(&mut guard, dgram_src, pkt);
            return;
        }
        
        error!("[NAT-MANAGER] No existing session found, creating new session: src={}, dst={}", dgram_src, pkt.dst_addr);

        let mut sess = sess.cloned().unwrap_or(Session {
            network: Network::Udp,
            source: dgram_src.address,
            destination: pkt.dst_addr.clone(),
            inbound_tag: inbound_tag.to_string(),
            ..Default::default()
        });
        if sess.inbound_tag.is_empty() {
            sess.inbound_tag = inbound_tag.to_string();
        }

        error!("[NAT-MANAGER] Adding new session to session map: src={}, dst={}", dgram_src, pkt.dst_addr);
        self.add_session(sess, *dgram_src, client_ch_tx.clone(), &mut guard)
            .await;

        error!(
            "[NAT-MANAGER] Added UDP session successfully: {} -> {} (total_sessions={})",
            &dgram_src,
            &pkt.dst_addr,
            guard.len(),
        );

        error!("[NAT-MANAGER] Sending initial packet through new session: src={}, dst={}", dgram_src, pkt.dst_addr);
        self._send(&mut guard, dgram_src, pkt);

        drop(guard);
    }

    pub async fn add_session<'a>(
        &self,
        sess: Session,
        raddr: DatagramSource,
        client_ch_tx: Sender<UdpPacket>,
        guard: &mut MutexGuard<'a, SessionMap>,
    ) {
        // Runs the lazy task for session cleanup job, this task will run only once.
        if let Some(task) = self.timeout_check_task.lock().await.take() {
            tokio::spawn(task);
        }

        let (target_ch_tx, mut target_ch_rx) =
            mpsc::channel(*crate::option::UDP_UPLINK_CHANNEL_SIZE);
        let (downlink_abort_tx, downlink_abort_rx) = oneshot::channel();

        guard.insert(raddr, (target_ch_tx, downlink_abort_tx, Instant::now()));

        let dispatcher = self.dispatcher.clone();
        let sessions = self.sessions.clone();

        // Spawns a new task for dispatching to avoid blocking the current task,
        // because we have stream type transports for UDP traffic, establishing a
        // TCP stream would block the task.
        tokio::spawn(async move {
            error!("[NAT-MANAGER] Starting dispatch task for session: src={}", raddr);
            // new socket to communicate with the target.
            error!("[NAT-MANAGER] Dispatching datagram through outbound: src={}", raddr);
            let socket = match dispatcher.dispatch_datagram(sess).await {
                Ok(s) => {
                    error!("[NAT-MANAGER] Datagram dispatch successful: src={}", raddr);
                    s
                },
                Err(e) => {
                    error!("[NAT-MANAGER] Datagram dispatch failed: src={}, error={}", raddr, e);
                    sessions.lock().await.remove(&raddr);
                    return;
                }
            };

            let (mut target_sock_recv, mut target_sock_send) = socket.split();
            
            error!("[NAT-MANAGER] Starting downlink and uplink tasks for session: src={}", raddr);

            // downlink
            let downlink_task = async move {
                error!("[NAT-MANAGER] Downlink task started for session: src={}", raddr);
                let mut buf = vec![0u8; *crate::option::DATAGRAM_BUFFER_SIZE * 1024];
                loop {
                    match target_sock_recv.recv_from(&mut buf).await {
                        Err(err) => {
                            error!(
                                "[NAT-MANAGER] Failed to receive downlink packets on session {}: {}",
                                &raddr, err
                            );
                            break;
                        }
                        Ok((n, addr)) => {
                            error!("[NAT-MANAGER] Received downlink UDP packet: session={}, src={}, size={} bytes", raddr, addr, n);
                            let pkt = UdpPacket::new(
                                buf[..n].to_vec(),
                                addr.clone(),
                                SocksAddr::from(raddr.address),
                            );
                            error!("[NAT-MANAGER] Sending downlink packet to client: session={}, src={}, dst={}, size={} bytes", raddr, addr, pkt.dst_addr, n);
                            if let Err(err) = client_ch_tx.send(pkt).await {
                                error!(
                                    "[NAT-MANAGER] Failed to send downlink packets on session {} to {}: {}",
                                    &raddr, &addr, err
                                );
                                break;
                            } else {
                                error!("[NAT-MANAGER] Downlink packet sent to client successfully: session={}, src={}", raddr, addr);
                            }

                            // activity update
                            {
                                let mut sessions = sessions.lock().await;
                                if let Some(sess) = sessions.get_mut(&raddr) {
                                    if addr.port() == 53 {
                                        // If the destination port is 53, we assume it's a
                                        // DNS query and set a negative timeout so it will
                                        // be removed on next check.
                                        sess.2.checked_sub(Duration::from_secs(
                                            *option::UDP_SESSION_TIMEOUT,
                                        ));
                                    } else {
                                        sess.2 = Instant::now();
                                    }
                                }
                            }
                        }
                    }
                }
                sessions.lock().await.remove(&raddr);
            };

            let (downlink_task, downlink_task_handle) = abortable(downlink_task);
            tokio::spawn(downlink_task);

            // Runs a task to receive the abort signal.
            tokio::spawn(async move {
                let _ = downlink_abort_rx.await;
                downlink_task_handle.abort();
            });

            // uplink
            tokio::spawn(async move {
                error!("[NAT-MANAGER] Uplink task started for session: src={}", raddr);
                while let Some(pkt) = target_ch_rx.recv().await {
                    error!(
                        "[NAT-MANAGER] Received uplink UDP packet from client: session={}, dst={}, size={} bytes",
                        &raddr, &pkt.dst_addr, pkt.data.len()
                    );
                    error!("[NAT-MANAGER] Sending uplink packet to target: session={}, dst={}, size={} bytes", raddr, pkt.dst_addr, pkt.data.len());
                    if let Err(e) = target_sock_send.send_to(&pkt.data, &pkt.dst_addr).await {
                        error!(
                            "[NAT-MANAGER] Failed to send uplink packets on session {} to {}: {:?}",
                            &raddr, &pkt.dst_addr, e
                        );
                        break;
                    } else {
                        error!("[NAT-MANAGER] Uplink packet sent to target successfully: session={}, dst={}", raddr, pkt.dst_addr);
                    }
                }
                error!("[NAT-MANAGER] Closing uplink connection for session: src={}", raddr);
                if let Err(e) = target_sock_send.close().await {
                    error!("[NAT-MANAGER] Failed to close outbound datagram {}: {}", &raddr, e);
                } else {
                    error!("[NAT-MANAGER] Uplink connection closed successfully for session: src={}", raddr);
                }
            });
        });
    }
}

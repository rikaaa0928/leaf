use std::collections::HashMap;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
#[cfg(feature = "lifecycle-hooks")]
use std::process::Command;
use std::str::FromStr;
#[cfg(feature = "lifecycle-hooks")]
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use anyhow::{anyhow, Result};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::RwLock;
use tokio::time::timeout;
#[cfg(feature = "lifecycle-hooks")]
use tracing::{info, warn};

use crate::{
    app::{dns::DnsClient, outbound::manager::OutboundManager, SyncDnsClient},
    config::Config,
    proxy::*,
    session::*,
};

#[cfg(feature = "lifecycle-hooks")]
fn resolve_lifecycle_commands(
    config: &crate::Config,
) -> Result<crate::LifecycleCommands, crate::Error> {
    match config {
        crate::Config::File(path) => {
            crate::config::lifecycle_from_file(path).map_err(crate::Error::Config)
        }
        crate::Config::Str(content) => {
            crate::config::lifecycle_from_string(content).map_err(crate::Error::Config)
        }
        crate::Config::Internal(_) => Ok(crate::LifecycleCommands::default()),
    }
}

#[cfg(feature = "lifecycle-hooks")]
fn execute_lifecycle_command(stage: &str, command: &str) -> Result<(), crate::Error> {
    let command = command.trim();
    if command.is_empty() {
        return Ok(());
    }

    info!("running lifecycle {} command", stage);
    let output = if cfg!(windows) {
        Command::new("cmd").arg("/C").arg(command).output()
    } else {
        Command::new("sh").arg("-c").arg(command).output()
    }
    .map_err(crate::Error::Io)?;

    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();

    if !stdout.is_empty() {
        info!("lifecycle {} stdout: {}", stage, stdout);
    }
    if !stderr.is_empty() {
        info!("lifecycle {} stderr: {}", stage, stderr);
    }

    if output.status.success() {
        info!("lifecycle {} command completed", stage);
        return Ok(());
    }

    let status = output
        .status
        .code()
        .map(|code| code.to_string())
        .unwrap_or_else(|| "terminated by signal".to_string());
    Err(crate::Error::Io(std::io::Error::other(format!(
        "lifecycle {} command failed with status {}: {}",
        stage, status, command
    ))))
}

#[cfg(feature = "lifecycle-hooks")]
pub fn start_with_lifecycle(
    rt_id: crate::RuntimeId,
    opts: crate::StartOptions,
) -> Result<(), crate::Error> {
    let lifecycle = resolve_lifecycle_commands(&opts.config)?;
    if lifecycle.is_empty() {
        return crate::start(rt_id, opts);
    }

    let cancelled = Arc::new(AtomicBool::new(false));
    let post_start = lifecycle
        .post_start
        .as_deref()
        .map(str::trim)
        .filter(|command| !command.is_empty())
        .map(str::to_owned)
        .map(|command| {
            let cancelled = cancelled.clone();
            std::thread::spawn(move || {
                while !cancelled.load(Ordering::SeqCst) {
                    if crate::is_running(rt_id) {
                        if let Err(e) = execute_lifecycle_command("post-start", &command) {
                            warn!("running lifecycle post-start command failed: {}", e);
                        }
                        break;
                    }
                    std::thread::sleep(Duration::from_millis(50));
                }
            })
        });

    let result = crate::start(rt_id, opts);
    cancelled.store(true, Ordering::SeqCst);
    if let Some(handle) = post_start {
        let _ = handle.join();
    }

    if result.is_ok() {
        if let Some(command) = lifecycle.post_stop.as_deref() {
            if let Err(e) = execute_lifecycle_command("post-stop", command) {
                warn!("running lifecycle post-stop command failed: {}", e);
            }
        }
    }

    result
}

#[cfg(not(feature = "lifecycle-hooks"))]
pub fn start_with_lifecycle(
    rt_id: crate::RuntimeId,
    opts: crate::StartOptions,
) -> Result<(), crate::Error> {
    crate::start(rt_id, opts)
}

fn get_start_options(
    config_path: String,
    #[cfg(feature = "auto-reload")] auto_reload: bool,
    multi_thread: bool,
    auto_threads: bool,
    threads: usize,
    stack_size: usize,
) -> crate::StartOptions {
    if !multi_thread {
        return crate::StartOptions {
            config: crate::Config::File(config_path),
            #[cfg(feature = "auto-reload")]
            auto_reload,
            runtime_opt: crate::RuntimeOption::SingleThread,
            #[cfg(feature = "routing-history")]
            routing_history_enabled: false,
            #[cfg(feature = "routing-history")]
            routing_history_max_records: 0,
        };
    }
    if auto_threads {
        return crate::StartOptions {
            config: crate::Config::File(config_path),
            #[cfg(feature = "auto-reload")]
            auto_reload,
            runtime_opt: crate::RuntimeOption::MultiThreadAuto(stack_size),
            #[cfg(feature = "routing-history")]
            routing_history_enabled: false,
            #[cfg(feature = "routing-history")]
            routing_history_max_records: 0,
        };
    }
    crate::StartOptions {
        config: crate::Config::File(config_path),
        #[cfg(feature = "auto-reload")]
        auto_reload,
        runtime_opt: crate::RuntimeOption::MultiThread(threads, stack_size),
        #[cfg(feature = "routing-history")]
        routing_history_enabled: false,
        #[cfg(feature = "routing-history")]
        routing_history_max_records: 0,
    }
}

pub fn run_with_options(
    rt_id: crate::RuntimeId,
    config_path: String,
    #[cfg(feature = "auto-reload")] auto_reload: bool,
    multi_thread: bool,
    auto_threads: bool,
    threads: usize,
    stack_size: usize,
) -> Result<(), crate::Error> {
    let opts = get_start_options(
        config_path,
        #[cfg(feature = "auto-reload")]
        auto_reload,
        multi_thread,
        auto_threads,
        threads,
        stack_size,
    );
    start_with_lifecycle(rt_id, opts)
}

async fn test_tcp_outbound(
    dns_client: SyncDnsClient,
    handler: AnyOutboundHandler,
) -> Result<Duration> {
    let sess = Session {
        destination: SocksAddr::Domain("www.google.com".to_string(), 80),
        new_conn_once: true,
        ..Default::default()
    };
    let start = tokio::time::Instant::now();
    let stream = crate::proxy::connect_stream_outbound(&sess, dns_client, &handler).await?;
    let mut stream = handler.stream()?.handle(&sess, None, stream).await?;
    stream.write_all(b"HEAD / HTTP/1.1\r\n\r\n").await?;
    let mut buf = Vec::new();
    let n = stream.read_buf(&mut buf).await?;
    if n == 0 {
        Err(anyhow!("EOF"))
    } else {
        Ok(tokio::time::Instant::now().duration_since(start))
    }
}

async fn test_udp_outbound(
    dns_client: SyncDnsClient,
    handler: AnyOutboundHandler,
) -> Result<Duration> {
    use hickory_proto::{
        op::{header::MessageType, op_code::OpCode, query::Query, Message},
        rr::{record_type::RecordType, Name},
    };
    use rand::{rngs::StdRng, Rng, SeedableRng};
    let addr = SocksAddr::Ip(SocketAddr::new(IpAddr::V4(Ipv4Addr::new(8, 8, 8, 8)), 53));
    let sess = Session {
        destination: addr.clone(),
        new_conn_once: true,
        ..Default::default()
    };
    let start = tokio::time::Instant::now();
    let dgram = crate::proxy::connect_datagram_outbound(&sess, dns_client, &handler).await?;
    let dgram = handler.datagram()?.handle(&sess, dgram).await?;
    let mut msg = Message::new();
    let name = Name::from_str("www.google.com.")?;
    let query = Query::query(name, RecordType::A);
    msg.add_query(query);
    let mut rng = StdRng::from_entropy();
    let id: u16 = rng.gen();
    msg.set_id(id);
    msg.set_op_code(OpCode::Query);
    msg.set_message_type(MessageType::Query);
    msg.set_recursion_desired(true);
    let msg_buf = msg.to_vec()?;
    let (mut recv, mut send) = dgram.split();
    send.send_to(&msg_buf, &addr).await?;
    let mut buf = [0u8; 1500];
    let _ = recv.recv_from(&mut buf).await?;
    Ok(tokio::time::Instant::now().duration_since(start))
}

async fn test_healthcheck_tcp(
    dns_client: SyncDnsClient,
    handler: AnyOutboundHandler,
) -> Result<Duration> {
    crate::app::healthcheck::tcp(dns_client, handler).await
}

async fn test_healthcheck_udp(
    dns_client: SyncDnsClient,
    handler: AnyOutboundHandler,
) -> Result<Duration> {
    crate::app::healthcheck::udp(dns_client, handler).await
}

pub async fn test_outbound(
    tag: &str,
    config: &Config,
    to: Option<Duration>,
) -> Result<(Result<Duration>, Result<Duration>)> {
    let to = to.unwrap_or(Duration::from_secs(4));
    let dns_client = Arc::new(RwLock::new(DnsClient::new(&config.dns)?));
    let outbound_manager = OutboundManager::new(&config.outbounds, dns_client.clone())?;
    let handler = outbound_manager
        .get(tag)
        .ok_or_else(|| anyhow!("outbound {} not found", tag))?;
    let (tcp_res, udp_res) = futures::future::join(
        timeout(to, test_tcp_outbound(dns_client.clone(), handler.clone())),
        timeout(to, test_udp_outbound(dns_client, handler)),
    )
    .await;
    let tcp_res = match tcp_res.map_err(|e| e.into()) {
        Err(e) => Err(e),
        Ok(res) => match res {
            Err(e) => Err(e),
            Ok(duration) => Ok(duration),
        },
    };
    let udp_res = match udp_res.map_err(|e| e.into()) {
        Err(e) => Err(e),
        Ok(res) => match res {
            Err(e) => Err(e),
            Ok(duration) => Ok(duration),
        },
    };
    Ok((tcp_res, udp_res))
}

pub async fn test_outbounds(
    config: &Config,
    to: Option<Duration>,
    concurrency: usize,
) -> Result<HashMap<String, (Result<Duration>, Result<Duration>)>> {
    let to = to.unwrap_or(Duration::from_secs(4));
    let dns_client = Arc::new(RwLock::new(DnsClient::new(&config.dns)?));
    let outbound_manager = OutboundManager::new(&config.outbounds, dns_client.clone())?;

    let mut tasks = Vec::new();
    for handler in outbound_manager.handlers() {
        let tag = handler.tag().clone();
        let handler = handler.clone();
        let dns_client = dns_client.clone();
        tasks.push(async move {
            let (tcp_res, udp_res) = futures::future::join(
                timeout(to, test_tcp_outbound(dns_client.clone(), handler.clone())),
                timeout(to, test_udp_outbound(dns_client, handler)),
            )
            .await;
            let tcp_res = match tcp_res {
                Ok(res) => res,
                Err(_) => Err(anyhow!("timeout")),
            };
            let udp_res = match udp_res {
                Ok(res) => res,
                Err(_) => Err(anyhow!("timeout")),
            };
            (tag, (tcp_res, udp_res))
        });
    }

    use futures::StreamExt;
    let results = futures::stream::iter(tasks)
        .buffer_unordered(concurrency)
        .collect::<Vec<_>>()
        .await;

    let mut map = HashMap::new();
    for (tag, res) in results {
        map.insert(tag, res);
    }
    Ok(map)
}

pub async fn stream_outbounds_tests(
    config: &Config,
    to: Option<Duration>,
    concurrency: usize,
) -> Result<impl futures::Stream<Item = (String, (Result<Duration>, Result<Duration>))>> {
    let to = to.unwrap_or(Duration::from_secs(4));
    let dns_client = Arc::new(RwLock::new(DnsClient::new(&config.dns)?));
    let outbound_manager = OutboundManager::new(&config.outbounds, dns_client.clone())?;

    let mut tasks = Vec::new();
    for handler in outbound_manager.handlers() {
        let tag = handler.tag().clone();
        let handler = handler.clone();
        let dns_client = dns_client.clone();
        tasks.push(async move {
            let (tcp_res, udp_res) = futures::future::join(
                timeout(to, test_tcp_outbound(dns_client.clone(), handler.clone())),
                timeout(to, test_udp_outbound(dns_client, handler)),
            )
            .await;
            let tcp_res = match tcp_res {
                Ok(res) => res,
                Err(_) => Err(anyhow!("timeout")),
            };
            let udp_res = match udp_res {
                Ok(res) => res,
                Err(_) => Err(anyhow!("timeout")),
            };
            (tag, (tcp_res, udp_res))
        });
    }

    use futures::StreamExt;
    Ok(futures::stream::iter(tasks).buffer_unordered(concurrency))
}

pub async fn health_check_outbound(
    tag: &str,
    config: &Config,
    to: Option<Duration>,
) -> Result<(Result<Duration>, Result<Duration>)> {
    let to = to.unwrap_or(Duration::from_secs(4));
    let dns_client = Arc::new(RwLock::new(DnsClient::new(&config.dns)?));
    let outbound_manager = OutboundManager::new(&config.outbounds, dns_client.clone())?;
    let handler = outbound_manager
        .get(tag)
        .ok_or_else(|| anyhow!("outbound {} not found", tag))?;
    let (tcp_res, udp_res) = futures::future::join(
        timeout(
            to,
            test_healthcheck_tcp(dns_client.clone(), handler.clone()),
        ),
        timeout(to, test_healthcheck_udp(dns_client, handler)),
    )
    .await;
    let tcp_res = match tcp_res.map_err(|e| e.into()) {
        Err(e) => Err(e),
        Ok(res) => match res {
            Err(e) => Err(e),
            Ok(duration) => Ok(duration),
        },
    };
    let udp_res = match udp_res.map_err(|e| e.into()) {
        Err(e) => Err(e),
        Ok(res) => match res {
            Err(e) => Err(e),
            Ok(duration) => Ok(duration),
        },
    };
    Ok((tcp_res, udp_res))
}

#[cfg(test)]
mod tests {
    #[cfg(all(feature = "lifecycle-hooks", feature = "config-json", unix))]
    #[test]
    fn test_lifecycle_commands_run_on_shutdown() {
        use std::fs;
        use std::time::{Duration, SystemTime, UNIX_EPOCH};

        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let marker = std::env::temp_dir().join(format!("leaf-lifecycle-{}.txt", nanos));
        let marker_text = marker.to_string_lossy().replace('\'', "'\\''");

        let json = format!(
            r#"
{{
    "log": {{
        "level": "error"
    }},
    "lifecycle": {{
        "postStart": "printf start >> '{}'",
        "postStop": "printf stop >> '{}'"
    }},
    "dns": {{
        "servers": ["1.1.1.1"]
    }},
    "inbounds": [
        {{
            "tag": "socks",
            "address": "127.0.0.1",
            "port": 0,
            "protocol": "socks"
        }}
    ],
    "outbounds": [
        {{
            "protocol": "direct",
            "tag": "direct"
        }}
    ]
}}
"#,
            marker_text, marker_text
        );

        let opts = crate::StartOptions {
            config: crate::Config::Str(json),
            #[cfg(feature = "auto-reload")]
            auto_reload: false,
            runtime_opt: crate::RuntimeOption::SingleThread,
            #[cfg(feature = "routing-history")]
            routing_history_enabled: false,
            #[cfg(feature = "routing-history")]
            routing_history_max_records: 0,
        };

        let handle = std::thread::spawn(move || super::start_with_lifecycle(42, opts));

        for _ in 0..20 {
            std::thread::sleep(Duration::from_millis(100));
            if let Ok(content) = fs::read_to_string(&marker) {
                if content.contains("start") {
                    break;
                }
            }
        }

        assert!(crate::shutdown(42));
        assert!(handle.join().unwrap().is_ok());

        let content = fs::read_to_string(&marker).unwrap();
        assert_eq!(content, "startstop");
        let _ = fs::remove_file(&marker);
    }
}

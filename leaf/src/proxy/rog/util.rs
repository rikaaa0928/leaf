#[cfg(feature = "outbound-rog")]
use std::io;
#[cfg(feature = "outbound-rog")]
use std::time::Duration;

#[cfg(feature = "outbound-rog")]
use tokio::time::sleep;
#[cfg(feature = "outbound-rog")]
use tonic::transport::Channel;

#[cfg(feature = "outbound-rog")]
use crate::proxy::rog::protocol::rog::rog_service_client::RogServiceClient;

/// Connection helper with retry logic
#[cfg(feature = "outbound-rog")]
pub struct RogConnector {
    endpoint: String,
    max_retries: usize,
    retry_delay: Duration,
}

#[cfg(feature = "outbound-rog")]
impl RogConnector {
    pub fn new(address: &str, port: u16) -> Self {
        Self {
            endpoint: format!("https://{}:{}", address, port),
            max_retries: 3,
            retry_delay: Duration::from_millis(1000),
        }
    }

    pub async fn connect(&self) -> io::Result<RogServiceClient<Channel>> {
        let mut last_error = None;

        for attempt in 0..=self.max_retries {
            match RogServiceClient::connect(self.endpoint.clone()).await {
                Ok(client) => {
                    if attempt > 0 {
                        tracing::info!("ROG connection successful after {} retries", attempt);
                    }
                    return Ok(client);
                }
                Err(e) => {
                    last_error = Some(e);
                    if attempt < self.max_retries {
                        tracing::warn!(
                            "ROG connection attempt {} failed, retrying in {:?}: {}",
                            attempt + 1,
                            self.retry_delay,
                            last_error.as_ref().unwrap()
                        );
                        sleep(self.retry_delay).await;
                    }
                }
            }
        }

        Err(io::Error::new(
            io::ErrorKind::Other,
            format!("Failed to connect to ROG server after {} attempts: {}", 
                   self.max_retries + 1, 
                   last_error.unwrap())
        ))
    }
}

/// Parse address for ROG protocol
#[cfg(feature = "outbound-rog")]
pub fn parse_address(addr: &str) -> io::Result<(String, u16)> {
    // Use rsplit_once to handle IPv6 addresses with colons
    let (host, port) = addr
        .rsplit_once(':')
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "Invalid address format"))?;

    // Parse port
    let port: u16 = port
        .parse()
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "Invalid port number"))?;

    // Validate host is not empty
    if host.is_empty() {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "Empty host"));
    }

    Ok((host.to_string(), port))
}
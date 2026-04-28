use std::path::Path;

use anyhow::{anyhow, Result};

use crate::config::{common, internal};

pub use crate::config::common::{
    AMuxInboundSettings, AMuxOutboundSettings, CatInboundSettings, ChainInboundSettings,
    ChainOutboundSettings, Config, Dns, FailOverOutboundSettings, HcInboundSettings, Inbound,
    InboundSettings, Log, NfInboundSettings, ObfsOutboundSettings, Outbound, OutboundSettings,
    PluginOutboundSettings, QuicInboundSettings, QuicOutboundSettings, RealityOutboundSettings,
    RedirectOutboundSettings, Rule, SelectOutboundSettings, ShadowsocksInboundSettings,
    ShadowsocksOutboundSettings, SocksOutboundSettings, StaticOutboundSettings, TlsInboundSettings,
    TlsOutboundSettings, TrojanInboundSettings, TrojanOutboundSettings, TryAllOutboundSettings,
    TunInboundSettings, VMessOutboundSettings, VlessOutboundSettings, WebSocketInboundSettings,
    WebSocketOutboundSettings,
};

pub fn to_internal(config: Config) -> Result<internal::Config> {
    common::to_internal(config)
}

fn apply_env(config: &common::Config) {
    if let Some(env) = &config.env {
        for (k, v) in env {
            if !k.trim().is_empty() {
                std::env::set_var(k, v);
            }
        }
    }
}

pub fn json_from_string(config: &str) -> Result<common::Config> {
    let config: common::Config = serde_json::from_str(config)
        .map_err(|e| anyhow!("deserialize json config failed: {}", e))?;
    apply_env(&config);
    Ok(config)
}

#[cfg(feature = "lifecycle-hooks")]
#[derive(serde_derive::Deserialize)]
struct LifecycleOnlyConfig {
    #[serde(default)]
    lifecycle: crate::LifecycleCommands,
}

#[cfg(feature = "lifecycle-hooks")]
pub fn lifecycle_from_string(config: &str) -> Result<crate::LifecycleCommands> {
    let config: LifecycleOnlyConfig = serde_json::from_str(config)
        .map_err(|e| anyhow!("deserialize json config failed: {}", e))?;
    Ok(config.lifecycle)
}

pub fn from_string(s: &str) -> Result<internal::Config> {
    let config = json_from_string(s)?;
    common::to_internal(config)
}

#[cfg(feature = "lifecycle-hooks")]
pub fn lifecycle_from_file<P>(path: P) -> Result<crate::LifecycleCommands>
where
    P: AsRef<Path>,
{
    let config = std::fs::read_to_string(path)?;
    lifecycle_from_string(&config)
}

pub fn from_file<P>(path: P) -> Result<internal::Config>
where
    P: AsRef<Path>,
{
    let config = std::fs::read_to_string(path)?;
    let config = json_from_string(&config)?;
    common::to_internal(config)
}

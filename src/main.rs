mod config;
mod model;
mod mqtt;
mod smart;
mod zfs;

use crate::config::Config;
use crate::model::Snapshot;
use anyhow::{Context, Result};
use std::time::Duration;
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() -> Result<()> {
    dotenvy::dotenv().ok();
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .init();

    let cfg = Config::from_env()?;

    let (publisher, _client) = mqtt::Publisher::new(cfg.clone())?;

    if cfg.once {
        let snap = collect(&cfg)?;
        publisher.publish_snapshot(&snap).await?;
        publisher.disconnect().await;
        tracing::info!("published one snapshot; exiting");
        return Ok(());
    }

    tracing::info!(
        "starting ZFS->HA monitor for node '{}', polling every {}s",
        cfg.node_name,
        cfg.interval_secs
    );
    let mut ticker = tokio::time::interval(Duration::from_secs(cfg.interval_secs.max(1)));
    loop {
        match collect(&cfg) {
            Ok(snap) => {
                if let Err(e) = publisher.publish_snapshot(&snap).await {
                    tracing::error!("publish failed: {e}");
                }
            }
            Err(e) => tracing::error!("collect failed: {e:#}"),
        }
        ticker.tick().await;
    }
}

/// Collect a snapshot from live ZFS/SMART, or from a YAML fixture if configured.
fn collect(cfg: &Config) -> Result<Snapshot> {
    if let Some(path) = &cfg.failure_fixture {
        tracing::info!("loading failure fixture from {path}");
        let text = std::fs::read_to_string(path)
            .with_context(|| format!("cannot read fixture {path}"))?;
        let snap: Snapshot = serde_yaml::from_str(&text)
            .with_context(|| format!("bad fixture yaml in {path}"))?;
        Ok(snap)
    } else {
        collect_live(cfg)
    }
}

fn collect_live(cfg: &Config) -> Result<Snapshot> {
    let pools = zfs::collect_pools()?;
    if pools.is_empty() {
        anyhow::bail!("no zfs pools found (is ZFS installed / pools imported?)");
    }

    let mut disks = Vec::new();
    // Gather SMART for every leaf disk across pools, dedup by device.
    let mut seen = std::collections::HashSet::new();
    for pool in &pools {
        for leaf in pool.vdevs.iter().flat_map(|v| v.leaves()) {
            let dev = smart::whole_disk(&leaf.name);
            if dev.is_empty() || !seen.insert(dev.clone()) {
                continue;
            }
            match smart::smart_for(&dev) {
                Ok(d) => disks.push(d),
                Err(e) => tracing::warn!("smartctl failed for {dev}: {e}"),
            }
        }
    }

    Ok(Snapshot {
        node: cfg.node_name.clone(),
        pools,
        disks,
        collected_unix: std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0),
    })
}

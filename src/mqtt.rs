use crate::config::Config;
use crate::model::{DiskSmart, Pool, Snapshot};
use anyhow::Result;
use rumqttc::{AsyncClient, Event, MqttOptions, Packet, QoS, Transport};
use serde_json::json;
use std::time::Duration;

/// MQTT publisher owning the client + event loop task.
pub struct Publisher {
    client: AsyncClient,
    cfg: Config,
}

impl Publisher {
    pub fn new(cfg: Config) -> Result<(Self, AsyncClient)> {
        let url = url::Url::parse(&cfg.mqtt_url)
            .map_err(|e| anyhow::anyhow!("invalid MQTT_URL {}: {}", cfg.mqtt_url, e))?;
        let host = url.host_str().unwrap_or("127.0.0.1").to_string();
        let port = url.port().unwrap_or(1883);
        let user = if !url.username().is_empty() {
            Some(url.username().to_string())
        } else {
            cfg.user()
        };
        let pass = url.password().map(|p| p.to_string()).or_else(|| cfg.pass());

        let mut opts = MqttOptions::new(
            &format!("zfs-ha-monitor-{}", cfg.node_name),
            host,
            port,
        );
        opts.set_keep_alive(Duration::from_secs(30));
        opts.set_clean_session(true);
        if let Some(u) = user {
            opts.set_credentials(u, pass.unwrap_or_default());
        }
        if matches!(url.scheme(), "mqtts" | "ssl" | "tls") {
            opts.set_transport(Transport::tls_with_default_config());
        } else {
            opts.set_transport(Transport::Tcp);
        }

        let (client, eventloop) = AsyncClient::new(opts, 10);
        tokio::spawn(run_eventloop(eventloop));
        Ok((Publisher { client: client.clone(), cfg }, client))
    }

    fn device(&self, identifiers: &[&str], name: &str, model: &str) -> serde_json::Value {
        json!({
            "identifiers": identifiers,
            "name": name,
            "model": model,
            "manufacturer": "OpenZFS",
        })
    }

    /// Send a discovery config payload + initial state, then return the state topic.
    async fn publish_sensor(
        &self,
        component: &'static str,
        object_id: &str,
        unique_id: &str,
        name: &str,
        device: &serde_json::Value,
        state_topic: &str,
        state_value: String,
        unit: Option<&str>,
        device_class: Option<&str>,
        state_class: Option<&str>,
        extra: &serde_json::Value,
    ) -> Result<()> {
        let config_topic =
            format!("{}/{}/{}/{}/config", self.cfg.discovery_prefix, component, object_id, unique_id);
        let mut payload = json!({
            "state_topic": state_topic,
            "unique_id": unique_id,
            "name": name,
            "device": device,
        });
        if let Some(u) = unit {
            payload["unit_of_measurement"] = json!(u);
        }
        if let Some(dc) = device_class {
            payload["device_class"] = json!(dc);
        }
        if let Some(sc) = state_class {
            payload["state_class"] = json!(sc);
        }
        if let Some(aux) = extra.as_object() {
            for (k, v) in aux {
                payload[k] = v.clone();
            }
        }

        self.client
            .publish(config_topic, QoS::AtMostOnce, true, payload.to_string().into_bytes())
            .await?;
        self.client
            .publish(state_topic.to_string(), QoS::AtMostOnce, true, state_value.into_bytes())
            .await?;
        Ok(())
    }

    /// Publish all entities for a pool (pool-level + per-leaf-disk).
    pub async fn publish_pool(&self, pool: &Pool) -> Result<()> {
        let base = format!("{}/{}/{}", self.cfg.mqtt_topic, self.cfg.node_name, pool.name);
        let device = self.device(
            &[&format!("zfs_pool_{}_{}", self.cfg.node_name, pool.name)],
            &format!("ZFS Pool {} ({})", pool.name, self.cfg.node_name),
            "ZFS Pool",
        );

        // --- Binary sensor: pool problem state ---
        let st = format!("{base}/state");
        let problem = if pool.overall() == "ONLINE" { "no_problem" } else { "problem" };
        self.publish_sensor(
            "binary_sensor",
            &format!("{}_{}_problem", self.cfg.node_name, pool.name),
            &format!("{}_pool_{}_problem", self.cfg.node_name, pool.name),
            &format!("{} pool problem", pool.name),
            &device,
            &st,
            problem.to_string(),
            None,
            Some("problem"),
            None,
            &json!({"payload_on": "problem", "payload_off": "no_problem"}),
        )
        .await?;

        let frag = pool.frag_percent.unwrap_or(0) as f64;
        self.publish_sensor(
            "sensor",
            &format!("{}_{}_fragmentation", self.cfg.node_name, pool.name),
            &format!("{}_pool_{}_fragmentation", self.cfg.node_name, pool.name),
            &format!("{} fragmentation", pool.name),
            &device,
            &format!("{base}/fragmentation"),
            format!("{frag:.1}"),
            Some("%"),
            None,
            Some("measurement"),
            &json!({}),
        )
        .await?;

        let cap = pool.cap_percent.unwrap_or(0) as f64;
        self.publish_sensor(
            "sensor",
            &format!("{}_{}_capacity", self.cfg.node_name, pool.name),
            &format!("{}_pool_{}_capacity", self.cfg.node_name, pool.name),
            &format!("{} capacity", pool.name),
            &device,
            &format!("{base}/capacity"),
            format!("{cap:.1}"),
            Some("%"),
            None,
            Some("measurement"),
            &json!({}),
        )
        .await?;

        // free / alloc in GB
        let g = 1024f64 * 1024f64 * 1024f64;
        let free_gb = pool.free_bytes as f64 / g;
        let alloc_gb = pool.alloc_bytes as f64 / g;
        self.publish_sensor(
            "sensor",
            &format!("{}_{}_free", self.cfg.node_name, pool.name),
            &format!("{}_pool_{}_free", self.cfg.node_name, pool.name),
            &format!("{} free", pool.name),
            &device,
            &format!("{base}/free"),
            format!("{free_gb:.1}"),
            Some("GB"),
            None,
            Some("measurement"),
            &json!({}),
        )
        .await?;
        self.publish_sensor(
            "sensor",
            &format!("{}_{}_used", self.cfg.node_name, pool.name),
            &format!("{}_pool_{}_used", self.cfg.node_name, pool.name),
            &format!("{} used", pool.name),
            &device,
            &format!("{base}/used"),
            format!("{alloc_gb:.1}"),
            Some("GB"),
            None,
            Some("measurement"),
            &json!({}),
        )
        .await?;

        Ok(())
    }

    /// Publish per-disk SMART entities.
    pub async fn publish_disk(&self, pool: &Pool, disk: &DiskSmart) -> Result<()> {
        let dev = format!("zfs_disk_{}_{}_{}", self.cfg.node_name, pool.name, disk.name);
        let device = self.device(
            &[&dev],
            &format!("Disk {} ({})", disk.name, pool.name),
            "SATA Disk",
        );
        let base = format!(
            "{}/{}/{}/{}/{}",
            self.cfg.mqtt_topic, self.cfg.node_name, pool.name, "disk", disk.name
        );

        // temperature
        if let Some(t) = disk.temperature_c() {
            self.publish_sensor(
                "sensor",
                &format!("{}_{}_temperature", self.cfg.node_name, disk.name),
                &format!("{}_disk_{}_temperature", self.cfg.node_name, disk.name),
                &format!("{} temperature", disk.name),
                &device,
                &format!("{base}/temperature"),
                t.to_string(),
                Some("°C"),
                Some("temperature"),
                Some("measurement"),
                &json!({}),
            )
            .await?;
        }

        // SMART reliability counters. These are plain counts; there is no
        // valid HA SensorDeviceClass for them, so omit device_class entirely
        // and use "count" as the unit label.
        for attr in [
            "Reallocated_Sector_Ct",
            "Reported_Uncorrect",
            "Current_Pending_Sector",
            "Offline_Uncorrectable",
        ] {
            if let Some(v) = disk.raw(attr) {
                let key = attr.replace("_", "").to_lowercase();
                self.publish_sensor(
                    "sensor",
                    &format!("{}_{}_{}", self.cfg.node_name, disk.name, key),
                    &format!("{}_disk_{}_{}", self.cfg.node_name, disk.name, key),
                    &format!("{} {}", disk.name, key),
                    &device,
                    &format!("{base}/{key}"),
                    v.to_string(),
                    Some("count"),
                    None,
                    Some("measurement"),
                    &json!({}),
                )
                .await?;
            }
        }
        Ok(())
    }

    /// Publish a whole snapshot.
    pub async fn publish_snapshot(&self, snap: &Snapshot) -> Result<()> {
        for pool in &snap.pools {
            self.publish_pool(pool).await?;
            for leaf in pool.vdevs.iter().flat_map(|v| v.leaves()) {
                if let Some(d) = snap
                    .disks
                    .iter()
                    .find(|d| d.name == leaf_name_of(leaf))
                {
                    self.publish_disk(pool, d).await?;
                }
            }
        }
        Ok(())
    }

    pub async fn disconnect(&self) {
        let _ = self.client.disconnect().await;
    }
}

fn leaf_name_of(v: &crate::model::Vdev) -> String {
    crate::smart::short_name(&v.name)
}

/// Drive the rumqttc event loop, logging errors.
async fn run_eventloop(mut eventloop: rumqttc::EventLoop) {
    loop {
        match eventloop.poll().await {
            Ok(Event::Incoming(Packet::ConnAck(_))) => {
                tracing::info!("connected to MQTT broker");
                continue;
            }
            Ok(_) => continue,
            Err(e) => {
                tracing::warn!("MQTT event loop error: {e}");
                tokio::time::sleep(Duration::from_secs(5)).await;
            }
        }
    }
}

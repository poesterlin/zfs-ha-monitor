/// Runtime configuration loaded from environment / .env.
#[derive(Debug, Clone)]
pub struct Config {
    /// MQTT broker URL, e.g. mqtt://user:pass@host:1883
    pub mqtt_url: String,
    /// Optional explicit username (overrides any in the URL).
    pub mqtt_user: Option<String>,
    /// Optional explicit password (overrides any in the URL).
    pub mqtt_pass: Option<String>,
    /// Base topic for state, e.g. "zfs" -> zfs/<node>/...
    pub mqtt_topic: String,
    /// Poll interval seconds.
    pub interval_secs: u64,
    /// Short name of this node used in entity ids/device names.
    pub node_name: String,
    /// HA MQTT discovery prefix (default "homeassistant").
    pub discovery_prefix: String,
    /// Run one collect + publish and exit instead of looping.
    pub once: bool,
    /// If set, load a YAML fixture snapshot (e.g. a DEGRADED pool) for testing.
    pub failure_fixture: Option<String>,
}

impl Config {
    pub fn from_env() -> anyhow::Result<Self> {
        Ok(Config {
            mqtt_url: env_or("MQTT_URL", "mqtt://127.0.0.1:1883"),
            mqtt_user: optional_env("MQTT_USER"),
            mqtt_pass: optional_env("MQTT_PASS"),
            mqtt_topic: env_or("MQTT_TOPIC", "zfs"),
            interval_secs: env_or("INTERVAL_SECS", "60").parse()?,
            node_name: env_or("NODE_NAME", "pve"),
            discovery_prefix: env_or("MQTT_DISCOVERY_PREFIX", "homeassistant"),
            once: env_flag("ONCE"),
            failure_fixture: optional_env("FAILURE_FIXTURE"),
        })
    }

    pub fn user(&self) -> Option<String> {
        self.mqtt_user.clone()
    }

    pub fn pass(&self) -> Option<String> {
        self.mqtt_pass.clone()
    }
}

fn env_or(key: &str, default: &str) -> String {
    std::env::var(key).unwrap_or_else(|_| default.to_string())
}

fn optional_env(key: &str) -> Option<String> {
    std::env::var(key).ok().filter(|v| !v.is_empty())
}

fn env_flag(key: &str) -> bool {
    std::env::var(key)
        .map(|v| {
            let v = v.to_lowercase();
            matches!(v.as_str(), "1" | "true" | "yes" | "on")
        })
        .unwrap_or(false)
}

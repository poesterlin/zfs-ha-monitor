use serde::{Deserialize, Serialize};

/// Overall pool snapshot (the "happy path" of `zpool status` + `zpool list`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Pool {
    pub name: String,
    pub state: String,
    /// Raw configured size in bytes (0 if unknown).
    pub size_bytes: u64,
    pub alloc_bytes: u64,
    pub free_bytes: u64,
    /// Fragmentation percent shown in `zpool list`.
    pub frag_percent: Option<u64>,
    /// Capacity percent.
    pub cap_percent: Option<u64>,
    pub health: String,
    /// True if any vdev is DEGRADED/FAULTED/UNAVAIL.
    pub degraded: bool,
    pub vdevs: Vec<Vdev>,
}

impl Pool {
    /// Overall health string: ONLINE if healthy, else first bad vdev/pool state.
    pub fn overall(&self) -> &str {
        if self.degraded { "DEGRADED" } else { "ONLINE" }
    }
}

/// A top-level vdev (raidz/mirror/disk) parsed from `zpool status`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Vdev {
    /// ZFS name (pool name, raidz1-0, or device path).
    pub name: String,
    pub state: String,
    pub read_errors: u64,
    pub write_errors: u64,
    pub cksum_errors: u64,
    /// Children (for raidz/mirror) or empty for leaf disks.
    pub children: Vec<Vdev>,
    /// True if this node represents an individual physical disk.
    pub is_leaf: bool,
}

impl Vdev {
    pub fn leaves(&self) -> Vec<&Vdev> {
        let mut out = Vec::new();
        collect_leaves(self, &mut out);
        out
    }
}

fn collect_leaves<'a>(v: &'a Vdev, out: &mut Vec<&'a Vdev>) {
    if v.is_leaf {
        out.push(v);
    }
    for c in &v.children {
        collect_leaves(c, out);
    }
}

/// SMART attributes for one disk, keyed by attribute id/name.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiskSmart {
    pub device: String,
    /// friendly short name, e.g. "sda" or by-id tail
    pub name: String,
    pub model: Option<String>,
    pub serial: Option<String>,
    /// attribute name -> raw value
    pub attrs: Vec<Attr>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Attr {
    pub id: u64,
    pub name: String,
    /// Numeric raw value from smartctl (`raw.value`). For temperature this is an
    /// internal encoding (not degrees) — use `temperature_c()` instead.
    pub raw_value: u64,
    /// Human-readable raw string (`raw.string`), e.g. "48 (0 14 0 0 0)".
    pub raw_string: String,
    pub value: u64,
    pub worst: u64,
    pub thresh: u64,
    pub flags: String,
    pub when_failed: Option<String>,
}

impl DiskSmart {
    pub fn get(&self, name: &str) -> Option<&Attr> {
        self.attrs.iter().find(|a| a.name == name)
    }
    /// Fetch the numeric raw value by canonical attribute name.
    pub fn raw(&self, name: &str) -> Option<u64> {
        self.get(name).map(|a| a.raw_value)
    }
    pub fn temp_raw_string(&self) -> Option<&str> {
        self.get("Temperature_Celsius").map(|a| a.raw_string.as_str())
    }
    /// Temperature in degrees C: parsed from the leading integer of the raw string
    /// "48 (0 14 0 0 0)", since the numeric value field is smartctl-encoded.
    pub fn temperature_c(&self) -> Option<u64> {
        let s = self.temp_raw_string()?;
        s.split_whitespace()
            .next()
            .and_then(|p| p.parse().ok())
    }
}

/// Everything collected in one poll pass.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Snapshot {
    pub node: String,
    pub pools: Vec<Pool>,
    pub disks: Vec<DiskSmart>,
    pub collected_unix: i64,
}

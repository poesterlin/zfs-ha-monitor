use crate::model::{Attr, DiskSmart};
use anyhow::{Context, Result};
use serde::Deserialize;
use std::process::Command;

#[derive(Debug, Deserialize)]
struct SmartRoot {
    #[serde(default)]
    device: Option<DeviceInfo>,
    #[serde(rename = "ata_smart_attributes")]
    attrs: Option<AtaAttrs>,
}

#[derive(Debug, Deserialize)]
struct DeviceInfo {
    #[serde(default)]
    model_name: Option<String>,
    #[serde(default)]
    serial_number: Option<String>,
    #[serde(default)]
    info_name: String,
}

#[derive(Debug, Deserialize)]
struct AtaAttrs {
    #[serde(default)]
    table: Vec<AttrJson>,
}

#[derive(Debug, Deserialize)]
struct AttrJson {
    id: u64,
    name: String,
    #[serde(rename = "raw")]
    #[serde(default)]
    raw: RawValue,
    value: u64,
    worst: u64,
    #[serde(default)]
    thresh: u64,
    #[serde(default)]
    flags: serde_json::Value,
    #[serde(rename = "when_failed")]
    #[serde(default)]
    when_failed: Option<String>,
}

/// smartctl `-A -j` represents `raw` as `{"value": <int>, "string": "<readable>"}`.
#[derive(Debug, Deserialize)]
struct RawValue {
    #[serde(default)]
    value: u64,
    #[serde(default)]
    string: String,
}

impl Default for RawValue {
    fn default() -> Self {
        RawValue {
            value: 0,
            string: String::new(),
        }
    }
}

/// Query SMART attributes for a device via smartctl JSON.
pub fn smart_for(device: &str) -> Result<DiskSmart> {
    let out = Command::new("smartctl")
        .args(["-A", "-j", device])
        .output()
        .with_context(|| format!("smartctl failed for {device}"))?;
    let text = String::from_utf8_lossy(&out.stdout);
    parse_smart_json(&text, device)
}

/// Parse a smartctl `-A -j` blob into the model.
fn parse_smart_json(text: &str, device: &str) -> Result<DiskSmart> {
    let root: SmartRoot = serde_json::from_str(text)
        .with_context(|| format!("bad smartctl json for {device}"))?;

    let model = root
        .device
        .as_ref()
        .and_then(|d| d.model_name.clone())
        .or_else(|| root.device.as_ref().map(|d| d.info_name.clone()));
    let serial = root.device.as_ref().and_then(|d| d.serial_number.clone());

    let mut attrs = Vec::new();
    if let Some(a) = root.attrs {
        for t in a.table {
            attrs.push(Attr {
                id: t.id,
                name: t.name,
                raw_value: t.raw.value,
                raw_string: t.raw.string,
                value: t.value,
                worst: t.worst,
                thresh: t.thresh,
                flags: format!("{:?}", t.flags),
                when_failed: t.when_failed,
            });
        }
    }

    let name = short_name(device);
    Ok(DiskSmart {
        device: device.to_string(),
        name,
        model,
        serial,
        attrs,
    })
}

/// Normalize a vdev device reference to the whole-disk device (strip the
/// partition suffix). Handles the forms zpool reports: by-id `...-part1`,
/// SATA/SCSI `/dev/sdb1`, and NVMe `/dev/nvme0n1p2`.
pub fn whole_disk(device: &str) -> String {
    let d = device.trim();
    // by-id form: strip trailing "-part<digits>".
    if let Some(idx) = d.rfind("-part") {
        let tail = &d[idx + "-part".len()..];
        if !tail.is_empty() && tail.chars().all(|c| c.is_ascii_digit()) {
            return d[..idx].to_string();
        }
    }
    // NVMe form (whole disk `nvme0n1`, partition `nvme0n1p2`): strip `p<digits>`.
    if d.contains("/nvme") || d.starts_with("nvme") {
        if let Some(idx) = d.rfind('p') {
            let tail = &d[idx + 1..];
            if !tail.is_empty() && tail.chars().all(|c| c.is_ascii_digit()) {
                return d[..idx].to_string();
            }
        }
        return d.to_string();
    }
    // SATA/SCSI form (`/dev/sdb1`, `sda9`): strip trailing digits after sd[a-z].
    let split = d.rfind(|c: char| !c.is_ascii_digit());
    match split {
        Some(i) => {
            let (root, tail) = (&d[..=i], &d[i + 1..]);
            if !tail.is_empty() && root.ends_with(|c: char| c.is_ascii_alphabetic()) {
                d[..=i].to_string()
            } else {
                d.to_string()
            }
        }
        None => d.to_string(),
    }
}

/// Short human name for a device path, e.g. by-id tail or "/dev/sda".
pub fn short_name(device: &str) -> String {
    let trimmed = whole_disk(device);
    trimmed
        .rsplit('/')
        .next()
        .unwrap_or(&trimmed)
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn parses_real_smart_report() {
        let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures")
            .join("smart-report.json");
        let text = std::fs::read_to_string(path).expect("fixture present");
        let d = parse_smart_json(&text, "/dev/disk/by-id/ata-ST12000NM0558_ZHZ5X47S-part1")
            .expect("parse");

        assert_eq!(d.name, "ata-ST12000NM0558_ZHZ5X47S", "-part1 suffix trimmed");
        assert!(!d.attrs.is_empty(), "SMART attribute table parsed");

        // Key reliability attributes present on the disk.
        for want in [
            "Reallocated_Sector_Ct",
            "Reported_Uncorrect",
            "Current_Pending_Sector",
            "Offline_Uncorrectable",
            "Temperature_Celsius",
            "Power_On_Hours",
        ] {
            assert!(d.get(want).is_some(), "missing attribute {want}");
        }

        // The failing disk's signature values (real session data).
        assert_eq!(d.raw("Reallocated_Sector_Ct"), Some(32));
        assert_eq!(d.raw("Reported_Uncorrect"), Some(295));
        assert_eq!(d.raw("Current_Pending_Sector"), Some(61368));
        assert_eq!(d.raw("Offline_Uncorrectable"), Some(61368));
        // Temperature is parsed from the raw string's leading integer (48), not the
        // smartctl-encoded numeric raw value.
        assert_eq!(d.temperature_c(), Some(48));
    }

    #[test]
    fn short_name_trims_by_id_and_partition_suffix() {
        assert_eq!(
            short_name("/dev/disk/by-id/ata-ST12000VN0008-3MH101_ZZ30MHNX-part1"),
            "ata-ST12000VN0008-3MH101_ZZ30MHNX"
        );
        assert_eq!(short_name("/dev/sda"), "sda");
        assert_eq!(short_name("wwn-0x5000c500c48e90ca-part1"), "wwn-0x5000c500c48e90ca");
    }

    #[test]
    fn whole_disk_normalizes_both_partition_styles() {
        // by-id `-partN`
        assert_eq!(
            whole_disk("/dev/disk/by-id/ata-ST12000NM0127_ZJV57HHX-part1"),
            "/dev/disk/by-id/ata-ST12000NM0127_ZJV57HHX"
        );
        // kernel `sdXN` / `sdaN`
        assert_eq!(whole_disk("/dev/sdb1"), "/dev/sdb");
        assert_eq!(whole_disk("/dev/sdd"), "/dev/sdd");
        assert_eq!(whole_disk("sda9"), "sda");
        // no partition suffix -> unchanged
        assert_eq!(whole_disk("/dev/nvme0n1"), "/dev/nvme0n1");
    }
}


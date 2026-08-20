use crate::model::{Pool, Vdev};
use anyhow::{Context, Result};
use std::collections::HashMap;
use std::process::Command;

/// Run a host command, returning trimmed stdout.
pub fn run(cmd: &str, args: &[&str]) -> Result<String> {
    let out = Command::new(cmd)
        .args(args)
        .output()
        .with_context(|| format!("failed to spawn {cmd}"))?;
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

fn is_leaf_name(name: &str) -> bool {
    name.starts_with('/') || name.starts_with("wwn-") || name.starts_with("ata-")
}

/// Parse an error-count cell which may carry K/M/G/T suffixes, e.g. "1.34K", "1338", "0".
fn parse_count(s: &str) -> u64 {
    let s = s.trim();
    let (num, mult) = match s.chars().last() {
        Some('K') | Some('k') => (&s[..s.len() - 1], 1_000u64),
        Some('M') | Some('m') => (&s[..s.len() - 1], 1_000_000u64),
        Some('G') | Some('g') => (&s[..s.len() - 1], 1_000_000_000u64),
        Some('T') | Some('t') => (&s[..s.len() - 1], 1_000_000_000_000u64),
        _ => (s, 1u64),
    };
    match num.parse::<f64>() {
        Ok(v) => (v * mult as f64).round() as u64,
        Err(_) => num.parse::<u64>().unwrap_or(0) * mult,
    }
}

/// Build a Pool from `zpool status` text + raw `zpool list` sizes.
fn build_pool(
    status_text: &str,
    pool_name: &str,
    all_sizes: &HashMap<String, (u64, u64, u64, u64, u64, String)>,
) -> Pool {
    // Walk the table body and reconstruct the vdev tree from indentation depth.
    let mut raw_lines: Vec<(usize, String)> = Vec::new();
    let mut in_table = false;
    for raw in status_text.lines() {
        let line = raw.trim_end_matches('\r');
        if !in_table {
            if line.contains("STATE") && line.contains("CKSUM") {
                in_table = true;
            }
            continue;
        }
        let trimmed = line.trim();
        if trimmed.is_empty()
            || trimmed.starts_with("errors:")
            || trimmed.starts_with("config:")
        {
            continue;
        }
        let leading = line.len() - line.trim_start().len();
        raw_lines.push((leading, trimmed.to_string()));
    }

    if raw_lines.is_empty() {
        return Pool {
            name: pool_name.into(),
            state: "UNKNOWN".into(),
            size_bytes: 0,
            alloc_bytes: 0,
            free_bytes: 0,
            frag_percent: None,
            cap_percent: None,
            health: "UNKNOWN".into(),
            degraded: false,
            vdevs: Vec::new(),
        };
    }

    // Row 0 is the pool itself; its indent is the root depth. Top-level vdevs are
    // one level deeper (root+2 chars), their leaf disks deeper still.
    let root_depth = raw_lines[0].0;
    let pool_cols: Vec<&str> = raw_lines[0].1.split_whitespace().collect();
    let pool_state = pool_cols.get(1).copied().unwrap_or("UNKNOWN").to_string();

    let mut vdevs: Vec<Vdev> = Vec::new();
    let mut current_top: Option<usize> = None;

    for (i, (depth, line)) in raw_lines.iter().enumerate() {
        if i == 0 {
            continue; // pool row
        }
        let cols: Vec<&str> = line.split_whitespace().collect();
        if cols.len() < 5 {
            continue;
        }
        let name = cols[0].to_string();
        let state = cols[1].to_string();
        let read = parse_count(cols[2]);
        let write = parse_count(cols[3]);
        let cksum = parse_count(cols[4]);
        let leaf = is_leaf_name(&name);
        let node = Vdev {
            name,
            state,
            read_errors: read,
            write_errors: write,
            cksum_errors: cksum,
            children: Vec::new(),
            is_leaf: leaf,
        };

        if *depth == root_depth + 2 {
            vdevs.push(node);
            current_top = Some(vdevs.len() - 1);
        } else if *depth > root_depth + 2 {
            if let Some(ci) = current_top {
                vdevs[ci].children.push(node);
            }
        }
    }

    let degraded = pool_state != "ONLINE"
        || vdevs.iter().any(|v| v.state != "ONLINE")
        || vdevs
            .iter()
            .any(|v| v.children.iter().any(|c| c.state != "ONLINE"));

    let size = all_sizes.get(pool_name);
    let (size_bytes, alloc_bytes, free_bytes, frag, cap, health) = match size {
        Some(s) => s.clone(),
        None => (0, 0, 0, 0, 0, String::new()),
    };

    Pool {
        name: pool_name.into(),
        state: pool_state,
        size_bytes,
        alloc_bytes,
        free_bytes,
        frag_percent: if frag > 0 { Some(frag) } else { None },
        cap_percent: if cap > 0 { Some(cap) } else { None },
        health,
        degraded,
        vdevs,
    }
}

/// Run `zpool list -p` and return a map pool -> (size, alloc, free, frag, cap, health).
fn parse_zpool_list(text: &str) -> HashMap<String, (u64, u64, u64, u64, u64, String)> {
    let mut map = HashMap::new();
    let mut lines = text.lines();
    let _header = lines.next();
    for line in lines {
        let cols: Vec<&str> = line.split_whitespace().collect();
        if cols.len() < 11 {
            continue;
        }
        let name = cols[0].to_string();
        let size = cols[1].parse::<u64>().unwrap_or(0);
        let alloc = cols[2].parse::<u64>().unwrap_or(0);
        let free = cols[3].parse::<u64>().unwrap_or(0);
        let frag = cols[6].parse::<u64>().unwrap_or(0);
        let cap = cols[7].parse::<u64>().unwrap_or(0);
        let health = cols[9].to_string();
        map.insert(name, (size, alloc, free, frag, cap, health));
    }
    map
}

/// Collect all pools and their vdev topologies.
pub fn collect_pools() -> Result<Vec<Pool>> {
    let status = run("zpool", &["status", "-P"])?;
    let list = run("zpool", &["list", "-p"])?;
    let sizes = parse_zpool_list(&list);

    // Pool names come from `zpool list` (reliable); build each from the status table.
    let mut pool_names: Vec<String> = sizes.keys().cloned().collect();
    pool_names.sort();
    if pool_names.is_empty() {
        // Fall back to scanning the status table for a top-level pool row (tab-depth 8,
        // first table row after the header), e.g. `\tZFS  ONLINE  ...`.
        let mut in_table = false;
        for raw in status.lines() {
            let line = raw.trim_end_matches('\r');
            if !in_table {
                if line.contains("STATE") && line.contains("CKSUM") {
                    in_table = true;
                }
                continue;
            }
            let trimmed = line.trim();
            if trimmed.is_empty()
                || trimmed.starts_with("errors")
                || trimmed.starts_with("config:")
            {
                continue;
            }
            let leading = line.len() - line.trim_start().len();
            if leading == 8 {
                let name = trimmed.split_whitespace().next().unwrap_or("").to_string();
                if !name.is_empty() {
                    pool_names.push(name);
                }
                break;
            }
        }
    }

    let mut pools = Vec::new();
    for name in pool_names {
        pools.push(build_pool(&status, &name, &sizes));
    }
    Ok(pools)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn fixture(name: &str) -> String {
        let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures")
            .join(name);
        std::fs::read_to_string(path).expect("fixture file present")
    }

    #[test]
    fn parses_online_pool_happy_path() {
        let status = fixture("online-zpool-status.txt");
        let list = fixture("online-zpool-list.txt");
        let sizes = parse_zpool_list(&list);
        let pool = build_pool(&status, "ZFS", &sizes);

        assert_eq!(pool.name, "ZFS");
        assert_eq!(pool.state, "ONLINE");
        assert_eq!(pool.health, "ONLINE");
        assert!(!pool.degraded, "online pool must not be flagged degraded");
        assert_eq!(pool.size_bytes, 35991825940480);
        assert_eq!(pool.alloc_bytes, 17478336036864);
        assert_eq!(pool.free_bytes, 18513489903616);
        assert_eq!(pool.frag_percent, Some(26));
        assert_eq!(pool.cap_percent, Some(48));

        assert_eq!(pool.vdevs.len(), 1, "one top-level vdev (raidz1)");
        let raidz = &pool.vdevs[0];
        assert_eq!(raidz.name, "raidz1-0");
        assert_eq!(raidz.state, "ONLINE");
        assert_eq!(raidz.children.len(), 3, "raidz has 3 disk leaves");

        // Canonical device paths should register as leaves.
        let leaves: Vec<&Vdev> = raidz.children.iter().collect();
        assert!(leaves.iter().all(|l| l.is_leaf));
        assert_eq!(leaves[0].name, "/dev/disk/by-id/ata-ST12000VN0008-3MH101_ZZ30MHNX-part1");
        assert_eq!(leaves[0].read_errors, 0);
    }

    #[test]
    fn parses_degraded_pool_from_real_session_output() {
        let status = fixture("degraded-zpool-status.txt");
        let list = fixture("degraded-zpool-list.txt");
        let sizes = parse_zpool_list(&list);
        let pool = build_pool(&status, "ZFS", &sizes);

        assert_eq!(pool.state, "DEGRADED");
        assert_eq!(pool.health, "DEGRADED");
        assert!(pool.degraded, "pool with a failed vdev must be flagged degraded");

        let raidz = &pool.vdevs[0];
        assert_eq!(raidz.state, "DEGRADED");
        assert_eq!(raidz.children.len(), 3);

        // The degraded disk: 1.34K read errors parsed with suffix handling.
        let bad = &raidz.children[0];
        assert_eq!(bad.name, "ata-ST12000NM0127_ZJV57HHX");
        assert_eq!(bad.state, "DEGRADED");
        assert_eq!(bad.read_errors, 1340, "1.34K read errors -> 1340");

        // The two ONLINE disks keep their checksum counters.
        assert_eq!(raidz.children[1].name, "ata-ST12000NM0558_ZHZ5X47S");
        assert_eq!(raidz.children[1].cksum_errors, 28);
        assert_eq!(raidz.children[2].name, "wwn-0x5000c500c48e90ca");
        assert_eq!(raidz.children[2].cksum_errors, 23);

        // Overall pool degradation detection must walk the tree.
        assert!(pool.degraded);
    }

    #[test]
    fn parse_count_handles_suffixes_and_plain() {
        assert_eq!(parse_count("0"), 0);
        assert_eq!(parse_count("28"), 28);
        assert_eq!(parse_count("1.34K"), 1340);
        assert_eq!(parse_count("3.00M"), 3_000_000);
        assert_eq!(parse_count("206G"), 206_000_000_000);
        assert_eq!(parse_count("K"), 0, "bare suffix -> 0 (no number)");
    }

    #[test]
    fn collect_pools_detects_pool_name_from_list() {
        // Simulate the full two-column output (list keys feed the pool names).
        let list = fixture("online-zpool-list.txt");
        let sizes = parse_zpool_list(&list);
        assert!(sizes.contains_key("ZFS"), "pool name comes from zpool list");
    }
}


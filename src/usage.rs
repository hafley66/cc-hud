use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use std::path::PathBuf;
use std::io::{BufRead, Write};

use serde::{Deserialize, Serialize};

/// Where usage history is stored on disk.
fn history_path() -> PathBuf {
    let mut p = dirs();
    p.push("usage_history.jsonl");
    p
}

fn dirs() -> PathBuf {
    let mut p = PathBuf::from(std::env::var("HOME").unwrap_or_else(|_| "/tmp".into()));
    p.push(".cc-hud");
    std::fs::create_dir_all(&p).ok();
    p
}

/// A single usage snapshot from the API.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UsageSnapshot {
    pub ts: u64, // unix seconds
    pub five_hour: f64,   // utilization %
    pub seven_day: f64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub seven_day_opus: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub seven_day_sonnet: Option<f64>,
}

/// Time series of usage snapshots.
#[derive(Debug, Clone, Default)]
pub struct UsageData {
    pub snapshots: Vec<UsageSnapshot>,
    pub latest: Option<UsageSnapshot>,
    pub error: Option<String>,
}

#[derive(Deserialize)]
struct UsageWindow {
    utilization: Option<f64>,
    #[allow(dead_code)]
    resets_at: Option<String>,
}

#[derive(Deserialize)]
struct UsageResponse {
    five_hour: Option<UsageWindow>,
    seven_day: Option<UsageWindow>,
    seven_day_opus: Option<UsageWindow>,
    seven_day_sonnet: Option<UsageWindow>,
}

/// Read OAuth access token from macOS Keychain.
fn read_oauth_token() -> Option<String> {
    let output = std::process::Command::new("security")
        .args(["find-generic-password", "-s", "Claude Code-credentials", "-w"])
        .output()
        .ok()?;
    if !output.status.success() { return None; }
    let json_str = String::from_utf8(output.stdout).ok()?;
    let parsed: serde_json::Value = serde_json::from_str(json_str.trim()).ok()?;
    parsed.get("claudeAiOauth")?.get("accessToken")?.as_str().map(|s| s.to_string())
}

fn fetch_usage(token: &str) -> Result<UsageResponse, String> {
    let output = std::process::Command::new("curl")
        .args([
            "-s", "-f",
            "-H", &format!("Authorization: Bearer {}", token),
            "-H", "anthropic-beta: oauth-2025-04-20",
            "https://api.anthropic.com/api/oauth/usage",
        ])
        .output()
        .map_err(|e| format!("curl exec failed: {}", e))?;
    if !output.status.success() {
        return Err(format!("curl failed: {}", String::from_utf8_lossy(&output.stderr).trim()));
    }
    serde_json::from_slice::<UsageResponse>(&output.stdout)
        .map_err(|e| format!("parse failed: {}", e))
}

/// Load historical snapshots from disk (last 7 days).
fn load_history() -> Vec<UsageSnapshot> {
    let path = history_path();
    let file = match std::fs::File::open(&path) {
        Ok(f) => f,
        Err(_) => return vec![],
    };
    let cutoff = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
        .saturating_sub(7 * 24 * 3600);

    let reader = std::io::BufReader::new(file);
    let mut snaps = vec![];
    for line in reader.lines() {
        let Ok(line) = line else { continue };
        if let Ok(s) = serde_json::from_str::<UsageSnapshot>(&line) {
            if s.ts >= cutoff {
                snaps.push(s);
            }
        }
    }
    snaps
}

/// Append a snapshot to disk.
fn append_to_disk(snap: &UsageSnapshot) {
    let path = history_path();
    let mut file = match std::fs::OpenOptions::new().create(true).append(true).open(&path) {
        Ok(f) => f,
        Err(e) => {
            tracing::warn!("failed to open usage history file: {}", e);
            return;
        }
    };
    if let Ok(json) = serde_json::to_string(snap) {
        let _ = writeln!(file, "{}", json);
    }
}

/// Compact the history file: keep only last 7 days of data.
fn compact_history(snaps: &[UsageSnapshot]) {
    let path = history_path();
    let tmp = path.with_extension("jsonl.tmp");
    if let Ok(mut file) = std::fs::File::create(&tmp) {
        for s in snaps {
            if let Ok(json) = serde_json::to_string(s) {
                let _ = writeln!(file, "{}", json);
            }
        }
        let _ = std::fs::rename(&tmp, &path);
    }
}

pub fn poll_loop(data: Arc<Mutex<UsageData>>, interval: Duration) {
    let token = match read_oauth_token() {
        Some(t) => t,
        None => {
            tracing::warn!("no OAuth token found in keychain, usage polling disabled");
            let mut d = data.lock().unwrap();
            d.error = Some("no OAuth token".into());
            return;
        }
    };

    // Load history from disk
    let history = load_history();
    {
        let mut d = data.lock().unwrap();
        d.latest = history.last().cloned();
        d.snapshots = history;
    }

    let mut poll_count = 0u64;

    loop {
        let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_secs();

        match fetch_usage(&token) {
            Ok(resp) => {
                let snap = UsageSnapshot {
                    ts: now,
                    five_hour: resp.five_hour.as_ref().and_then(|w| w.utilization).unwrap_or(0.0),
                    seven_day: resp.seven_day.as_ref().and_then(|w| w.utilization).unwrap_or(0.0),
                    seven_day_opus: resp.seven_day_opus.as_ref().and_then(|w| w.utilization),
                    seven_day_sonnet: resp.seven_day_sonnet.as_ref().and_then(|w| w.utilization),
                };

                // Write to disk
                append_to_disk(&snap);

                let mut d = data.lock().unwrap();
                d.latest = Some(snap.clone());
                d.snapshots.push(snap);
                d.error = None;

                // Prune in-memory to 7 days
                let cutoff = now.saturating_sub(7 * 24 * 3600);
                d.snapshots.retain(|s| s.ts >= cutoff);

                poll_count += 1;
                // Compact disk file every ~100 polls (~2.5h at 90s interval)
                if poll_count % 100 == 0 {
                    compact_history(&d.snapshots);
                }
            }
            Err(e) => {
                tracing::warn!("usage poll failed: {}", e);
                let mut d = data.lock().unwrap();
                d.error = Some(e);
            }
        }

        std::thread::sleep(interval);
    }
}

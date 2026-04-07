use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use std::path::PathBuf;
use std::io::{BufRead, Write};

use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// Paths
// ---------------------------------------------------------------------------

fn hud_dir() -> PathBuf {
    let home = if cfg!(windows) {
        std::env::var("USERPROFILE").unwrap_or_else(|_| std::env::var("HOME").unwrap_or_else(|_| "/tmp".into()))
    } else {
        std::env::var("HOME").unwrap_or_else(|_| "/tmp".into())
    };
    let mut p = PathBuf::from(home);
    p.push(".cc-hud");
    std::fs::create_dir_all(&p).ok();
    p
}

fn history_path() -> PathBuf {
    let mut p = hud_dir();
    p.push("usage_history.jsonl");
    p
}

fn config_path() -> PathBuf {
    let mut p = hud_dir();
    p.push("config.toml");
    p
}

// ---------------------------------------------------------------------------
// Config
// ---------------------------------------------------------------------------

#[derive(Deserialize, Default)]
struct Config {
    /// Inline OAuth token.
    oauth_token: Option<String>,
    /// Path to a file containing the OAuth token (first line, trimmed).
    oauth_token_file: Option<String>,
    /// Poll interval in seconds (default 90).
    poll_interval_secs: Option<u64>,
}

fn load_config() -> Config {
    let path = config_path();
    match std::fs::read_to_string(&path) {
        Ok(text) => {
            // serde is already a dep; parse toml manually to avoid adding toml crate.
            // Format is simple key = "value" lines.
            parse_simple_toml(&text)
        }
        Err(_) => Config::default(),
    }
}

/// Minimal key="value" parser. Handles the three fields we care about.
fn parse_simple_toml(text: &str) -> Config {
    let mut cfg = Config::default();
    for line in text.lines() {
        let line = line.trim();
        if line.starts_with('#') || line.is_empty() { continue; }
        if let Some((key, val)) = line.split_once('=') {
            let key = key.trim();
            let val = val.trim().trim_matches('"').trim_matches('\'');
            match key {
                "oauth_token" => cfg.oauth_token = Some(val.to_string()),
                "oauth_token_file" => cfg.oauth_token_file = Some(val.to_string()),
                "poll_interval_secs" => cfg.poll_interval_secs = val.parse().ok(),
                _ => {}
            }
        }
    }
    cfg
}

// ---------------------------------------------------------------------------
// Token resolution
// ---------------------------------------------------------------------------

/// Resolve OAuth token. Priority:
/// 1. CC_HUD_OAUTH_TOKEN env var
/// 2. ~/.cc-hud/config.toml oauth_token or oauth_token_file
/// 3. Platform-native credential store (macOS Keychain)
fn read_oauth_token(cfg: &Config) -> Option<String> {
    // 1. Env var
    if let Ok(tok) = std::env::var("CC_HUD_OAUTH_TOKEN") {
        let tok = tok.trim().to_string();
        if !tok.is_empty() {
            tracing::info!("using OAuth token from CC_HUD_OAUTH_TOKEN env var");
            return Some(tok);
        }
    }

    // 2a. Config: inline token
    if let Some(tok) = &cfg.oauth_token {
        let tok = tok.trim().to_string();
        if !tok.is_empty() {
            tracing::info!("using OAuth token from config.toml oauth_token");
            return Some(tok);
        }
    }

    // 2b. Config: token file
    if let Some(path) = &cfg.oauth_token_file {
        let expanded = if path.starts_with('~') {
            let home = std::env::var("HOME").or_else(|_| std::env::var("USERPROFILE")).unwrap_or_default();
            path.replacen('~', &home, 1)
        } else {
            path.clone()
        };
        match std::fs::read_to_string(&expanded) {
            Ok(contents) => {
                let tok = contents.lines().next().unwrap_or("").trim().to_string();
                if !tok.is_empty() {
                    tracing::info!("using OAuth token from file: {}", expanded);
                    return Some(tok);
                }
            }
            Err(e) => {
                tracing::warn!("failed to read oauth_token_file {}: {}", expanded, e);
            }
        }
    }

    // 3. Platform-native
    read_platform_token()
}

#[cfg(target_os = "macos")]
fn read_platform_token() -> Option<String> {
    let output = std::process::Command::new("security")
        .args(["find-generic-password", "-s", "Claude Code-credentials", "-w"])
        .output()
        .ok()?;
    if !output.status.success() { return None; }
    let json_str = String::from_utf8(output.stdout).ok()?;
    let parsed: serde_json::Value = serde_json::from_str(json_str.trim()).ok()?;
    let tok = parsed.get("claudeAiOauth")?.get("accessToken")?.as_str()?.to_string();
    tracing::info!("using OAuth token from macOS Keychain");
    Some(tok)
}

#[cfg(target_os = "windows")]
fn read_platform_token() -> Option<String> {
    // Windows Credential Manager: Claude Code stores creds under "Claude Code-credentials".
    // Use cmdkey or PowerShell to read. For now, log guidance and return None.
    tracing::warn!(
        "Windows Credential Manager not yet supported. \
         Set CC_HUD_OAUTH_TOKEN env var or add oauth_token to ~/.cc-hud/config.toml"
    );
    None
}

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
fn read_platform_token() -> Option<String> {
    tracing::warn!(
        "no platform credential store implemented for this OS. \
         Set CC_HUD_OAUTH_TOKEN env var or add oauth_token to ~/.cc-hud/config.toml"
    );
    None
}

// ---------------------------------------------------------------------------
// Usage data types
// ---------------------------------------------------------------------------

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

// ---------------------------------------------------------------------------
// HTTP
// ---------------------------------------------------------------------------

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

// ---------------------------------------------------------------------------
// Disk persistence
// ---------------------------------------------------------------------------

/// Load historical snapshots from disk (last 7 days).
fn load_history() -> Vec<UsageSnapshot> {
    let path = history_path();
    let file = match std::fs::File::open(&path) {
        Ok(f) => f,
        Err(_) => return vec![],
    };
    let reader = std::io::BufReader::new(file);
    let mut snaps = vec![];
    for line in reader.lines() {
        let Ok(line) = line else { continue };
        if let Ok(s) = serde_json::from_str::<UsageSnapshot>(&line) {
            snaps.push(s);
        }
    }
    snaps
}

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

// ---------------------------------------------------------------------------
// Poll loop
// ---------------------------------------------------------------------------

pub fn poll_loop(data: Arc<Mutex<UsageData>>, default_interval: Duration) {
    let cfg = load_config();
    let interval = cfg.poll_interval_secs
        .map(|s| Duration::from_secs(s.max(10)))
        .unwrap_or(default_interval);

    let token = match read_oauth_token(&cfg) {
        Some(t) => t,
        None => {
            tracing::warn!("no OAuth token found, usage polling disabled");
            tracing::warn!("set CC_HUD_OAUTH_TOKEN env var or create ~/.cc-hud/config.toml");
            let mut d = data.lock().unwrap();
            d.error = Some("no OAuth token (see ~/.cc-hud/config.toml)".into());
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

                append_to_disk(&snap);

                let mut d = data.lock().unwrap();
                d.latest = Some(snap.clone());
                d.snapshots.push(snap);
                d.error = None;

                poll_count += 1;
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

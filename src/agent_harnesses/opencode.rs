/// OpenCode session harness.
///
/// Reads sessions and messages from the OpenCode SQLite database at
/// `~/.local/share/opencode/opencode.db`. Produces the same `SessionData`
/// and `Event` types as the Claude Code harness.
///
/// OpenCode stores message metadata as JSON in `message.data` with fields:
///   role, time.created, time.completed, modelID, providerID, cost,
///   tokens.{total, input, output, reasoning, cache.{read, write}}

use std::collections::HashMap;
use std::path::PathBuf;

use rusqlite::Connection;
use serde::Deserialize;

use super::claude_code::{Event, SessionData, HudData};
use crate::energy::{EnergyConfig, SessionEnergy};
use crate::model_registry;

fn db_path() -> PathBuf {
    let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".to_string());
    PathBuf::from(home).join(".local/share/opencode/opencode.db")
}

#[derive(Debug, Deserialize)]
struct MessageData {
    role: Option<String>,
    time: Option<TimeInfo>,
    #[serde(rename = "modelID")]
    model_id: Option<String>,
    #[serde(rename = "providerID")]
    provider_id: Option<String>,
    cost: Option<f64>,
    tokens: Option<TokenInfo>,
}

#[derive(Debug, Deserialize)]
struct TimeInfo {
    created: Option<u64>,
    completed: Option<u64>,
}

#[derive(Debug, Deserialize)]
struct TokenInfo {
    #[serde(default)]
    input: u64,
    #[serde(default)]
    output: u64,
    #[serde(default)]
    reasoning: u64,
    cache: Option<CacheInfo>,
}

#[derive(Debug, Deserialize)]
struct CacheInfo {
    #[serde(default)]
    read: u64,
    #[serde(default)]
    write: u64,
}

struct SessionRow {
    id: String,
    directory: String,
    title: String,
    time_created: u64,
    time_updated: u64,
}

struct MessageRow {
    session_id: String,
    time_created: u64,
    data: String,
}

/// Load all OpenCode sessions from the database.
/// Returns empty HudData if the DB doesn't exist or can't be read.
pub fn load_opencode_sessions(energy_config: &EnergyConfig) -> HudData {
    let path = db_path();
    if !path.exists() {
        return HudData::default();
    }

    let conn = match Connection::open_with_flags(&path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY) {
        Ok(c) => c,
        Err(e) => {
            tracing::warn!("Failed to open opencode DB: {e}");
            return HudData::default();
        }
    };

    let sessions = match load_sessions(&conn) {
        Ok(s) => s,
        Err(e) => {
            tracing::warn!("Failed to load opencode sessions: {e}");
            return HudData::default();
        }
    };

    let messages = match load_messages(&conn) {
        Ok(m) => m,
        Err(e) => {
            tracing::warn!("Failed to load opencode messages: {e}");
            return HudData::default();
        }
    };

    // Group messages by session_id
    let mut by_session: HashMap<String, Vec<MessageRow>> = HashMap::new();
    for msg in messages {
        by_session.entry(msg.session_id.clone()).or_default().push(msg);
    }

    let mut session_datas = Vec::new();

    for sess in sessions {
        let msgs = by_session.remove(&sess.id).unwrap_or_default();
        if let Some(sd) = build_session_data(sess, msgs, energy_config) {
            session_datas.push(sd);
        }
    }

    HudData { sessions: session_datas }
}

fn load_sessions(conn: &Connection) -> rusqlite::Result<Vec<SessionRow>> {
    let mut stmt = conn.prepare(
        "SELECT id, directory, title, time_created, time_updated FROM session ORDER BY time_updated DESC"
    )?;
    let rows = stmt.query_map([], |row| {
        Ok(SessionRow {
            id: row.get(0)?,
            directory: row.get(1)?,
            title: row.get(2)?,
            time_created: row.get::<_, i64>(3)? as u64,
            time_updated: row.get::<_, i64>(4)? as u64,
        })
    })?;
    rows.collect()
}

fn load_messages(conn: &Connection) -> rusqlite::Result<Vec<MessageRow>> {
    let mut stmt = conn.prepare(
        "SELECT session_id, time_created, data FROM message ORDER BY time_created ASC"
    )?;
    let rows = stmt.query_map([], |row| {
        Ok(MessageRow {
            session_id: row.get(0)?,
            time_created: row.get::<_, i64>(1)? as u64,
            data: row.get(2)?,
        })
    })?;
    rows.collect()
}

fn short_name(dir: &str) -> String {
    dir.rsplit('/').next().unwrap_or(dir).to_string()
}

fn build_session_data(
    sess: SessionRow,
    messages: Vec<MessageRow>,
    energy_config: &EnergyConfig,
) -> Option<SessionData> {
    let mut events = Vec::new();
    let mut seq: u32 = 0;
    let mut total_cost_usd = 0.0;
    let mut total_input_cost = 0.0;
    let mut total_output_cost = 0.0;
    let mut total_input: u64 = 0;
    let mut total_output: u64 = 0;
    let mut api_call_count: u32 = 0;
    let mut first_ts: u64 = 0;
    let mut last_ts: u64 = 0;
    let mut last_model = String::new();
    let mut last_input_tokens: u64 = 0;
    let mut energy = SessionEnergy::default();
    let mut cumulative_input_cost = 0.0;
    let mut cumulative_output_cost = 0.0;
    let tool_counts: HashMap<String, u32> = HashMap::new();

    for msg in &messages {
        let data: MessageData = match serde_json::from_str(&msg.data) {
            Ok(d) => d,
            Err(_) => continue,
        };

        // Only process assistant messages with token data
        if data.role.as_deref() != Some("assistant") {
            continue;
        }
        let tokens = match &data.tokens {
            Some(t) if t.input > 0 || t.output > 0 => t,
            _ => continue,
        };

        let model_id = data.model_id.as_deref().unwrap_or("unknown");
        let provider_id = data.provider_id.as_deref().unwrap_or("unknown");
        // Construct a combined model string for registry lookup
        let model_str = if model_id.contains('/') {
            // Already namespaced like "zai-org/GLM-5"
            model_id.to_string()
        } else {
            format!("{}/{}", provider_id, model_id)
        };

        let cache_read = tokens.cache.as_ref().map(|c| c.read).unwrap_or(0);
        let cache_write = tokens.cache.as_ref().map(|c| c.write).unwrap_or(0);

        // Use cost from OpenCode if available, otherwise compute from registry
        let profile = model_registry::lookup(&model_str);
        let (in_cost, out_cost) = if let Some(cost) = data.cost {
            // OpenCode provides total cost; split proportionally by token ratio
            // But we can also compute from registry for consistency
            let (reg_in, reg_out) = profile.pricing.cost_split(
                tokens.input, tokens.output, cache_read, cache_write,
            );
            let reg_total = reg_in + reg_out;
            if reg_total > 0.0 && cost > 0.0 {
                // Scale registry split to match opencode's reported total
                let scale = cost / reg_total;
                (reg_in * scale, reg_out * scale)
            } else {
                (reg_in, reg_out)
            }
        } else {
            profile.pricing.cost_split(tokens.input, tokens.output, cache_read, cache_write)
        };

        cumulative_input_cost += in_cost;
        cumulative_output_cost += out_cost;

        // Timestamp: opencode stores milliseconds
        let ts_secs = data.time.as_ref()
            .and_then(|t| t.created)
            .unwrap_or(msg.time_created) / 1000;

        if first_ts == 0 { first_ts = ts_secs; }
        last_ts = ts_secs;
        last_model = model_str.clone();
        last_input_tokens = tokens.input;

        events.push(Event::ApiCall {
            seq,
            timestamp_secs: ts_secs,
            input_tokens: tokens.input,
            output_tokens: tokens.output,
            cache_read_tokens: cache_read,
            cache_create_tokens: cache_write,
            input_cost_usd: in_cost,
            output_cost_usd: out_cost,
            cumulative_input_cost,
            cumulative_output_cost,
            model: model_str.clone(),
            has_thinking: tokens.reasoning > 0,
        });

        total_cost_usd += in_cost + out_cost;
        total_input_cost += in_cost;
        total_output_cost += out_cost;
        total_input += tokens.input + cache_read + cache_write;
        total_output += tokens.output;
        api_call_count += 1;

        energy.add_call(
            tokens.input, tokens.output, cache_read, cache_write,
            &model_str, in_cost + out_cost, energy_config,
        );

        seq += 1;
    }

    if api_call_count == 0 {
        return None;
    }

    Some(SessionData {
        session_id: sess.id,
        cwd: sess.directory.clone(),
        project: short_name(&sess.directory),
        pid: 0, // OpenCode doesn't expose PID
        events,
        total_cost_usd,
        total_input_cost,
        total_output_cost,
        total_input,
        total_output,
        tool_counts,
        skill_counts: HashMap::new(),
        read_counts: HashMap::new(),
        api_call_count,
        agent_count: 0,
        is_active: false, // No PID-based liveness check for opencode
        first_ts,
        last_ts,
        model: last_model,
        last_input_tokens,
        subagents: vec![],
        energy,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_message_data() {
        let json = r#"{
            "role": "assistant",
            "time": {"created": 1771778356375, "completed": 1771778363810},
            "modelID": "zai-org/GLM-5",
            "providerID": "togetherai",
            "cost": 0.0126174,
            "tokens": {"total": 11865, "input": 11523, "output": 342, "reasoning": 0, "cache": {"read": 0, "write": 0}}
        }"#;
        let data: MessageData = serde_json::from_str(json).unwrap();
        assert_eq!(data.model_id.as_deref(), Some("zai-org/GLM-5"));
        assert_eq!(data.provider_id.as_deref(), Some("togetherai"));
        assert!((data.cost.unwrap() - 0.0126174).abs() < 1e-8);
        let tokens = data.tokens.unwrap();
        assert_eq!(tokens.input, 11523);
        assert_eq!(tokens.output, 342);
    }

    #[test]
    fn loads_from_real_db_if_present() {
        let config = EnergyConfig::default();
        let hud = load_opencode_sessions(&config);
        // On machines with opencode data, this should produce sessions.
        // On CI without the DB, it returns empty. Both are valid.
        if db_path().exists() {
            assert!(!hud.sessions.is_empty(), "Expected sessions from opencode DB");
            let first = &hud.sessions[0];
            assert!(!first.session_id.is_empty());
            assert!(first.api_call_count > 0);
        }
    }
}

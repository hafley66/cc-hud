use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use crate::energy::{EnergyConfig, SessionEnergy};

// Pricing per million tokens (USD)
// cache_read = 0.1x input, cache_create_5m = 1.25x input
/// Context window size (input token limit) by model.
pub fn model_context_window(model: &str) -> u64 {
    match model {
        m if m.contains("opus-4-6") || m.contains("opus-4-5") => 1_000_000,
        m if m.contains("opus") => 200_000,
        m if m.contains("sonnet") => 200_000,
        m if m.contains("haiku") => 200_000,
        _ => 200_000,
    }
}

pub fn model_pricing(model: &str) -> (f64, f64, f64, f64) {
    // (input, output, cache_read, cache_create_5m) per 1M tokens
    match model {
        // Opus 4.6 / 4.5: $5 in, $25 out
        m if m.contains("opus-4-6") || m.contains("opus-4-5") => (5.0, 25.0, 0.50, 6.25),
        // Opus 4.1 / 4: $15 in, $75 out
        m if m.contains("opus") => (15.0, 75.0, 1.50, 18.75),
        // Sonnet 4.x: $3 in, $15 out
        m if m.contains("sonnet") => (3.0, 15.0, 0.30, 3.75),
        // Haiku 4.5: $1 in, $5 out
        m if m.contains("haiku-4-5") => (1.0, 5.0, 0.10, 1.25),
        // Haiku 3.5: $0.80 in, $4 out
        m if m.contains("haiku") => (0.80, 4.0, 0.08, 1.0),
        _ => (3.0, 15.0, 0.30, 3.75),
    }
}

/// Returns (input_cost, output_cost) separately.
fn compute_cost_split(model: &str, input: u64, output: u64, cache_read: u64, cache_create: u64) -> (f64, f64) {
    let (pi, po, pcr, pcc) = model_pricing(model);
    let in_cost = (input as f64 * pi + cache_read as f64 * pcr + cache_create as f64 * pcc) / 1_000_000.0;
    let out_cost = (output as f64 * po) / 1_000_000.0;
    (in_cost, out_cost)
}

/// One event in the session timeline.
#[derive(Debug, Clone, Serialize)]
pub enum Event {
    /// An assistant API call with token usage.
    ApiCall {
        seq: u32,
        timestamp_secs: u64,
        input_tokens: u64,
        output_tokens: u64,
        cache_read_tokens: u64,
        cache_create_tokens: u64,
        input_cost_usd: f64,
        output_cost_usd: f64,
        cumulative_input_cost: f64,
        cumulative_output_cost: f64,
        model: String,
        /// True when this API call included extended thinking content.
        has_thinking: bool,
    },
    /// A tool invocation.
    ToolUse {
        seq: u32,
        name: String,
    },
    /// A skill invocation (Skill tool with extracted skill name).
    SkillUse {
        seq: u32,
        skill: String,
    },
    /// A notable file read (CLAUDE.md, memory, etc.)
    ReadFile {
        seq: u32,
        category: String, // "CLAUDE.md", "memory", etc.
    },
    /// A subagent spawn.
    AgentSpawn {
        seq: u32,
        subagent_type: String,
        description: String,
    },
    /// Context compaction boundary.
    Compaction {
        seq: u32,
        timestamp_secs: u64,
    },
}

impl Event {
    pub fn seq(&self) -> u32 {
        match self {
            Event::ApiCall { seq, .. } => *seq,
            Event::ToolUse { seq, .. } => *seq,
            Event::SkillUse { seq, .. } => *seq,
            Event::ReadFile { seq, .. } => *seq,
            Event::AgentSpawn { seq, .. } => *seq,
            Event::Compaction { seq, .. } => *seq,
        }
    }
}

/// Per-subagent data.
#[derive(Debug, Clone, Serialize)]
pub struct SubagentData {
    pub agent_id: String,
    pub agent_type: String,
    pub description: String,
    pub model: String,
    pub total_cost_usd: f64,
    pub total_input: u64,
    pub total_output: u64,
    pub api_call_count: u32,
    pub tool_counts: HashMap<String, u32>,
    pub skill_counts: HashMap<String, u32>,
    pub read_counts: HashMap<String, u32>,
    pub first_ts: u64,
    pub last_ts: u64,
}

/// Per-session data.
#[derive(Debug, Clone, Serialize)]
pub struct SessionData {
    pub session_id: String,
    pub cwd: String,
    pub project: String,
    pub pid: i32,
    pub events: Vec<Event>,
    pub total_cost_usd: f64,
    pub total_input_cost: f64,
    pub total_output_cost: f64,
    pub total_input: u64,
    pub total_output: u64,
    pub tool_counts: HashMap<String, u32>,
    pub skill_counts: HashMap<String, u32>,
    pub read_counts: HashMap<String, u32>,
    pub api_call_count: u32,
    pub agent_count: u32,
    pub is_active: bool,
    pub first_ts: u64,   // unix seconds of first ApiCall (0 if none)
    pub last_ts: u64,    // unix seconds of last ApiCall  (0 if none)
    pub model: String,   // most recent model used
    pub last_input_tokens: u64, // input tokens of most recent API call (context fullness)
    pub subagents: Vec<SubagentData>,
    #[serde(skip)]
    pub energy: SessionEnergy,
}

/// All data the HUD renders from.
#[derive(Debug, Clone, Default, Serialize)]
pub struct HudData {
    pub sessions: Vec<SessionData>,
}

// --- internal types ---

#[derive(Debug, Deserialize)]
struct SessionFile {
    pid: i32,
    #[serde(rename = "sessionId")]
    session_id: String,
    cwd: String,
}

#[derive(Debug, Deserialize)]
struct JsonlLine {
    #[serde(rename = "type")]
    msg_type: Option<String>,
    #[serde(default)]
    subtype: Option<String>,
    message: Option<JsonlMessage>,
    timestamp: Option<String>,
}

/// Parse ISO 8601 UTC timestamp ("2026-03-18T19:38:00.000Z") -> unix seconds.
fn parse_iso_secs(s: &str) -> u64 {
    let b = s.as_bytes();
    if b.len() < 19 { return 0; }
    let n = |sl: &[u8]| -> u64 {
        std::str::from_utf8(sl).ok().and_then(|s| s.parse().ok()).unwrap_or(0)
    };
    let yr = n(&b[0..4]); let mo = n(&b[5..7]); let dy = n(&b[8..10]);
    let hr = n(&b[11..13]); let mn = n(&b[14..16]); let sc = n(&b[17..19]);
    let leap = |y: u64| y % 4 == 0 && (y % 100 != 0 || y % 400 == 0);
    let dim: [u64; 13] = [0,31,28,31,30,31,30,31,31,30,31,30,31];
    let mut days = 0u64;
    for y in 1970..yr { days += if leap(y) { 366 } else { 365 }; }
    for m in 1..mo { days += dim[m as usize] + if m == 2 && leap(yr) { 1 } else { 0 }; }
    days += dy.saturating_sub(1);
    days * 86400 + hr * 3600 + mn * 60 + sc
}

#[derive(Debug, Deserialize)]
struct JsonlMessage {
    id: Option<String>,
    model: Option<String>,
    usage: Option<JsonlUsage>,
    content: Option<Vec<ContentBlock>>,
    stop_reason: Option<String>,
}

#[derive(Debug, Deserialize)]
struct JsonlUsage {
    #[serde(default)]
    input_tokens: u64,
    #[serde(default)]
    output_tokens: u64,
    #[serde(default)]
    cache_read_input_tokens: u64,
    #[serde(default)]
    cache_creation_input_tokens: u64,
}

#[derive(Debug, Deserialize)]
struct ContentBlock {
    #[serde(rename = "type")]
    block_type: Option<String>,
    name: Option<String>,
    input: Option<serde_json::Value>,
}

#[derive(Debug, Deserialize)]
struct AgentMeta {
    #[serde(rename = "agentType", default)]
    agent_type: String,
    #[serde(default)]
    description: String,
}

struct TailState {
    offset: u64,
    seq: u32,
    cumulative_input_cost: f64,
    cumulative_output_cost: f64,
}

fn cwd_to_project_dir(cwd: &str) -> String {
    cwd.replace('/', "-")
}

fn short_name(cwd: &str) -> String {
    cwd.rsplit('/').next().unwrap_or(cwd).to_string()
}

/// Extract a readable project name from a project dir like "-Users-chris-projects-foo"
fn project_from_dir(dir_name: &str) -> String {
    // Take last 2 segments for context: "projects-foo" or "chris-projects"
    let parts: Vec<&str> = dir_name.split('-').filter(|s| !s.is_empty()).collect();
    if parts.len() >= 2 {
        parts[parts.len()-1].to_string()
    } else {
        dir_name.to_string()
    }
}

fn pid_alive(pid: i32) -> bool {
    unsafe { libc::kill(pid, 0) == 0 }
}

fn home_dir() -> String {
    std::env::var("HOME").unwrap_or_else(|_| "/tmp".to_string())
}

fn sessions_dir() -> String {
    format!("{}/.claude/sessions", home_dir())
}

fn projects_dir() -> String {
    format!("{}/.claude/projects", home_dir())
}

fn discover_active() -> Vec<SessionFile> {
    let dir = sessions_dir();
    let Ok(entries) = std::fs::read_dir(&dir) else { return vec![] };

    entries.flatten().filter_map(|entry| {
        let path = entry.path();
        if path.extension()?.to_str()? != "json" { return None; }
        let contents = std::fs::read_to_string(&path).ok()?;
        let sf: SessionFile = serde_json::from_str(&contents).ok()?;
        if pid_alive(sf.pid) { Some(sf) } else { None }
    }).collect()
}

/// Discover all JSONL session files across all projects.
/// Returns (session_id, project_dir_name, jsonl_path).
fn discover_all_jsonl() -> Vec<(String, String, String)> {
    let pdir = projects_dir();
    let Ok(projects) = std::fs::read_dir(&pdir) else { return vec![] };

    let mut result = Vec::new();
    for proj in projects.flatten() {
        let proj_path = proj.path();
        if !proj_path.is_dir() { continue; }
        let proj_name = proj.file_name().to_string_lossy().to_string();

        let Ok(files) = std::fs::read_dir(&proj_path) else { continue };
        for file in files.flatten() {
            let fpath = file.path();
            if fpath.extension().and_then(|e| e.to_str()) != Some("jsonl") { continue; }
            let sid = fpath.file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("")
                .to_string();
            if sid.is_empty() { continue; }
            result.push((sid, proj_name.clone(), fpath.to_string_lossy().to_string()));
        }
    }
    result
}

fn jsonl_path_for_session(sf: &SessionFile) -> String {
    let project_dir = cwd_to_project_dir(&sf.cwd);
    format!("{}/.claude/projects/{}/{}.jsonl", home_dir(), project_dir, sf.session_id)
}

/// Discover subagent dir for a session JSONL path.
/// Session JSONL is at `{projects}/{proj}/{sid}.jsonl`, subagents at `{projects}/{proj}/{sid}/subagents/`.
fn subagents_dir_for_jsonl(jsonl_path: &str) -> String {
    // Strip .jsonl extension to get session dir
    let base = jsonl_path.strip_suffix(".jsonl").unwrap_or(jsonl_path);
    format!("{}/subagents", base)
}

fn discover_subagents(jsonl_path: &str) -> Vec<SubagentData> {
    let dir = subagents_dir_for_jsonl(jsonl_path);
    let Ok(entries) = std::fs::read_dir(&dir) else { return vec![] };

    let mut meta_files: Vec<String> = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("").to_string();
        if name.ends_with(".meta.json") {
            meta_files.push(name);
        }
    }

    let mut subagents = Vec::new();
    for meta_name in &meta_files {
        let meta_path = format!("{}/{}", dir, meta_name);
        let agent_id = meta_name.strip_suffix(".meta.json").unwrap_or("").to_string();
        let jsonl_name = format!("{}.jsonl", agent_id);
        let jsonl_path = format!("{}/{}", dir, jsonl_name);

        let meta: AgentMeta = match std::fs::read_to_string(&meta_path)
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
        {
            Some(m) => m,
            None => continue,
        };

        // Parse the subagent JSONL for cost/token data
        let (events, _) = parse_jsonl_full(&jsonl_path);

        let mut total_input_cost = 0.0;
        let mut total_output_cost = 0.0;
        let mut total_input = 0u64;
        let mut total_output = 0u64;
        let mut api_calls = 0u32;
        let mut tool_counts: HashMap<String, u32> = HashMap::new();
        let mut skill_counts: HashMap<String, u32> = HashMap::new();
        let mut read_counts: HashMap<String, u32> = HashMap::new();
        let mut first_ts = 0u64;
        let mut last_ts = 0u64;
        let mut last_model = String::new();

        for ev in &events {
            match ev {
                Event::ApiCall { input_tokens, output_tokens, cumulative_input_cost, cumulative_output_cost, timestamp_secs, model, .. } => {
                    last_model = model.clone();
                    total_input_cost = *cumulative_input_cost;
                    total_output_cost = *cumulative_output_cost;
                    total_input += input_tokens;
                    total_output += output_tokens;
                    api_calls += 1;
                    if *timestamp_secs > 0 {
                        if first_ts == 0 { first_ts = *timestamp_secs; }
                        last_ts = *timestamp_secs;
                    }
                }
                Event::ToolUse { name, .. } => {
                    *tool_counts.entry(name.clone()).or_default() += 1;
                }
                Event::SkillUse { skill, .. } => {
                    *skill_counts.entry(skill.clone()).or_default() += 1;
                }
                Event::ReadFile { category, .. } => {
                    *read_counts.entry(category.clone()).or_default() += 1;
                }
                Event::AgentSpawn { .. } => {}
                Event::Compaction { .. } => {}
            }
        }

        subagents.push(SubagentData {
            agent_id,
            agent_type: meta.agent_type,
            description: meta.description,
            model: last_model,
            total_cost_usd: total_input_cost + total_output_cost,
            total_input,
            total_output,
            api_call_count: api_calls,
            tool_counts,
            skill_counts,
            read_counts,
            first_ts,
            last_ts,
        });
    }

    // Sort by first_ts so they appear in spawn order
    subagents.sort_by_key(|s| s.first_ts);
    subagents
}

fn parse_jsonl_full(path: &str) -> (Vec<Event>, TailState) {
    let mut state = TailState {
        offset: 0, seq: 0, cumulative_input_cost: 0.0, cumulative_output_cost: 0.0,
    };
    let events = tail_jsonl(path, &mut state);
    (events, state)
}

fn tail_jsonl(path: &str, state: &mut TailState) -> Vec<Event> {
    use std::io::{BufRead, BufReader, Seek, SeekFrom};

    let Ok(mut file) = std::fs::File::open(path) else { return vec![] };
    let file_len = file.metadata().map(|m| m.len()).unwrap_or(0);

    if file_len < state.offset {
        state.offset = 0;
        state.seq = 0;
        state.cumulative_input_cost = 0.0;
        state.cumulative_output_cost = 0.0;
    }
    if file_len <= state.offset { return vec![]; }
    if file.seek(SeekFrom::Start(state.offset)).is_err() { return vec![]; }

    let mut events = Vec::new();
    let reader = BufReader::new(&file);

    // Accumulate streaming partials per message ID.
    // Claude Code writes multiple JSONL entries for a single API call:
    //   - thinking partial (stop_reason=null, stale usage)
    //   - text partial (stop_reason=null, stale usage)
    //   - final entry (stop_reason!=null, real cumulative usage)
    // We collect tool/thinking data from all partials and emit events
    // only when we see the final entry with real usage.
    struct Pending {
        has_thinking: bool,
        tool_events: Vec<Event>,
        timestamp_secs: u64,
    }
    let mut pending: std::collections::HashMap<String, Pending> = std::collections::HashMap::new();

    for line in reader.lines() {
        let Ok(line) = line else { break };
        if line.trim().is_empty() { continue; }
        let Ok(parsed) = serde_json::from_str::<JsonlLine>(&line) else { continue };

        // Handle compact_boundary system events
        if parsed.msg_type.as_deref() == Some("system")
            && parsed.subtype.as_deref() == Some("compact_boundary")
        {
            state.seq += 1;
            let timestamp_secs = parsed.timestamp.as_deref().map(parse_iso_secs).unwrap_or(0);
            events.push(Event::Compaction { seq: state.seq, timestamp_secs });
            continue;
        }

        if parsed.msg_type.as_deref() != Some("assistant") { continue; }
        let Some(msg) = parsed.message else { continue; };

        let msg_id = msg.id.clone().unwrap_or_default();
        let is_final = msg.stop_reason.is_some();
        let timestamp_secs = parsed.timestamp.as_deref().map(parse_iso_secs).unwrap_or(0);

        // Get or create pending state for this message ID
        let p = pending.entry(msg_id.clone()).or_insert_with(|| Pending {
            has_thinking: false,
            tool_events: Vec::new(),
            timestamp_secs,
        });

        // Accumulate content block data from this partial
        if let Some(content) = &msg.content {
            for block in content {
                if block.block_type.as_deref() == Some("thinking") {
                    p.has_thinking = true;
                }
                if block.block_type.as_deref() == Some("tool_use") {
                    let name = block.name.clone().unwrap_or_else(|| "?".to_string());
                    state.seq += 1;
                    if name == "Agent" {
                        let subagent_type = block.input.as_ref()
                            .and_then(|v| v.get("subagent_type"))
                            .and_then(|v| v.as_str())
                            .unwrap_or("general-purpose")
                            .to_string();
                        let description = block.input.as_ref()
                            .and_then(|v| v.get("description"))
                            .and_then(|v| v.as_str())
                            .unwrap_or("")
                            .to_string();
                        p.tool_events.push(Event::AgentSpawn { seq: state.seq, subagent_type, description });
                    } else if name == "Skill" {
                        let skill = block.input.as_ref()
                            .and_then(|v| v.get("skill"))
                            .and_then(|v| v.as_str())
                            .unwrap_or("?")
                            .to_string();
                        p.tool_events.push(Event::SkillUse { seq: state.seq, skill });
                    } else if name == "Read" {
                        let file_path = block.input.as_ref()
                            .and_then(|v| v.get("file_path"))
                            .and_then(|v| v.as_str())
                            .unwrap_or("");
                        let category = if file_path.contains("CLAUDE.md") {
                            Some("CLAUDE.md")
                        } else if file_path.contains("/memory/") || file_path.contains("MEMORY.md") {
                            Some("memory")
                        } else {
                            None
                        };
                        p.tool_events.push(Event::ToolUse { seq: state.seq, name });
                        if let Some(cat) = category {
                            p.tool_events.push(Event::ReadFile { seq: state.seq, category: cat.to_string() });
                        }
                    } else {
                        p.tool_events.push(Event::ToolUse { seq: state.seq, name });
                    }
                }
            }
        }

        // Only emit ApiCall on the final entry (real usage) or if no message ID
        if !is_final && !msg_id.is_empty() { continue; }

        let p = pending.remove(&msg_id).unwrap_or(Pending {
            has_thinking: false, tool_events: Vec::new(), timestamp_secs,
        });

        // Flush accumulated tool events
        events.extend(p.tool_events);

        if let Some(usage) = msg.usage {
            let model = msg.model.unwrap_or_default();
            let (in_cost, out_cost) = compute_cost_split(
                &model,
                usage.input_tokens,
                usage.output_tokens,
                usage.cache_read_input_tokens,
                usage.cache_creation_input_tokens,
            );
            state.cumulative_input_cost += in_cost;
            state.cumulative_output_cost += out_cost;
            state.seq += 1;

            events.push(Event::ApiCall {
                seq: state.seq,
                timestamp_secs: p.timestamp_secs,
                input_tokens: usage.input_tokens,
                output_tokens: usage.output_tokens,
                cache_read_tokens: usage.cache_read_input_tokens,
                cache_create_tokens: usage.cache_creation_input_tokens,
                input_cost_usd: in_cost,
                output_cost_usd: out_cost,
                cumulative_input_cost: state.cumulative_input_cost,
                cumulative_output_cost: state.cumulative_output_cost,
                model,
                has_thinking: p.has_thinking,
            });
        }
    }

    state.offset = file_len;
    events
}

fn build_session_data(
    session_id: &str,
    cwd: &str,
    project: &str,
    pid: i32,
    events: Vec<Event>,
    is_active: bool,
    subagents: Vec<SubagentData>,
) -> SessionData {
    let energy_config = EnergyConfig::default();
    let mut total_input_cost = 0.0;
    let mut total_output_cost = 0.0;
    let mut total_input = 0u64;
    let mut total_output = 0u64;
    let mut tool_counts: HashMap<String, u32> = HashMap::new();
    let mut skill_counts: HashMap<String, u32> = HashMap::new();
    let mut read_counts: HashMap<String, u32> = HashMap::new();
    let mut api_calls = 0u32;
    let mut agents = 0u32;
    let mut first_ts = 0u64;
    let mut last_ts = 0u64;
    let mut last_model = String::new();
    let mut last_input_tokens = 0u64;
    let mut session_energy = SessionEnergy::default();

    for ev in &events {
        match ev {
            Event::ApiCall { input_tokens, output_tokens, cache_read_tokens, cache_create_tokens, input_cost_usd, output_cost_usd, cumulative_input_cost, cumulative_output_cost, timestamp_secs, model, .. } => {
                last_model = model.clone();
                last_input_tokens = input_tokens + cache_read_tokens + cache_create_tokens;
                total_input_cost = *cumulative_input_cost;
                total_output_cost = *cumulative_output_cost;
                total_input += input_tokens;
                total_output += output_tokens;
                api_calls += 1;
                if *timestamp_secs > 0 {
                    if first_ts == 0 { first_ts = *timestamp_secs; }
                    last_ts = *timestamp_secs;
                }
                session_energy.add_call(
                    *input_tokens, *output_tokens, *cache_read_tokens, *cache_create_tokens,
                    model, input_cost_usd + output_cost_usd, &energy_config,
                );
            }
            Event::ToolUse { name, .. } => {
                *tool_counts.entry(name.clone()).or_default() += 1;
            }
            Event::SkillUse { skill, .. } => {
                *skill_counts.entry(skill.clone()).or_default() += 1;
            }
            Event::ReadFile { category, .. } => {
                *read_counts.entry(category.clone()).or_default() += 1;
            }
            Event::AgentSpawn { .. } => { agents += 1; }
            Event::Compaction { .. } => {}
        }
    }

    SessionData {
        session_id: session_id.to_string(),
        cwd: cwd.to_string(),
        project: project.to_string(),
        pid,
        events,
        total_cost_usd: total_input_cost + total_output_cost,
        total_input_cost,
        total_output_cost,
        total_input,
        total_output,
        tool_counts,
        skill_counts,
        read_counts,
        api_call_count: api_calls,
        agent_count: agents,
        is_active,
        first_ts,
        last_ts,
        model: last_model,
        last_input_tokens,
        subagents,
        energy: session_energy,
    }
}

pub fn poll_loop(data: Arc<Mutex<HudData>>, show_history: bool) {
    let mut tail_states: HashMap<String, TailState> = HashMap::new();
    let mut known_sessions: HashMap<String, SessionFile> = HashMap::new();
    let mut history_loaded = false;

    loop {
        // Load history once on first tick
        if show_history && !history_loaded {
            history_loaded = true;
            let active = discover_active();
            let active_ids: Vec<String> = active.iter().map(|s| s.session_id.clone()).collect();

            let all_jsonl = discover_all_jsonl();
            tracing::info!(count = all_jsonl.len(), "scanning historical sessions");

            let mut sessions = Vec::new();
            for (sid, proj_dir, path) in &all_jsonl {
                let is_active = active_ids.contains(sid);
                let (events, tail_state) = parse_jsonl_full(path);
                if events.is_empty() { continue; }

                let project = project_from_dir(proj_dir);
                let subs = discover_subagents(path);
                if !subs.is_empty() {
                    tracing::info!(session = %sid, count = subs.len(), "discovered subagents");
                }
                let sd = build_session_data(sid, &project, &project, 0, events, is_active, subs);
                sessions.push(sd);

                // Keep tail state for active sessions so we can append later
                if is_active {
                    tail_states.insert(sid.clone(), tail_state);
                }
            }

            // Sort by total cost descending
            sessions.sort_by(|a, b| b.total_cost_usd.partial_cmp(&a.total_cost_usd).unwrap_or(std::cmp::Ordering::Equal));

            {
                let mut d = data.lock().unwrap();
                d.sessions = sessions;
            }

            // Register active sessions for tailing
            for sf in active {
                known_sessions.insert(sf.session_id.clone(), sf);
            }

            tracing::info!("history loaded");
            std::thread::sleep(std::time::Duration::from_secs(2));
            continue;
        }

        // Normal active-only polling
        let active = discover_active();
        let active_ids: Vec<String> = active.iter().map(|s| s.session_id.clone()).collect();

        for sf in active {
            known_sessions.entry(sf.session_id.clone()).or_insert(sf);
        }
        known_sessions.retain(|id, _| active_ids.contains(id));

        if !show_history {
            tail_states.retain(|id, _| active_ids.contains(id));
        }

        let mut updated = false;

        for (sid, sf) in &known_sessions {
            let path = jsonl_path_for_session(sf);
            let state = tail_states.entry(sid.clone()).or_insert(TailState {
                offset: 0, seq: 0, cumulative_input_cost: 0.0, cumulative_output_cost: 0.0,
            });

            let new_events = tail_jsonl(&path, state);
            if new_events.is_empty() { continue; }
            updated = true;

            let mut d = data.lock().unwrap();
            if let Some(existing) = d.sessions.iter_mut().find(|s| s.session_id == *sid) {
                existing.events.extend(new_events);
                // Recompute aggregates (re-discover subagents too, new ones may have spawned)
                let subs = discover_subagents(&path);
                *existing = build_session_data(
                    &existing.session_id,
                    &existing.cwd,
                    &existing.project,
                    existing.pid,
                    existing.events.clone(),
                    true,
                    subs,
                );
            } else {
                // New active session not in history
                let events = new_events;
                let subs = discover_subagents(&path);
                let sd = build_session_data(sid, &short_name(&sf.cwd), &short_name(&sf.cwd), sf.pid, events, true, subs);
                d.sessions.insert(0, sd);
            }
        }

        // Refresh is_active on all sessions every poll cycle
        {
            let mut d = data.lock().unwrap();
            for s in d.sessions.iter_mut() {
                s.is_active = active_ids.contains(&s.session_id);
            }
        }

        if !show_history && !updated {
            // Rebuild session list for active-only mode
            let mut all_sessions = Vec::new();
            for (sid, sf) in &known_sessions {
                let d = data.lock().unwrap();
                if let Some(s) = d.sessions.iter().find(|s| s.session_id == *sid) {
                    all_sessions.push(s.clone());
                } else {
                    drop(d);
                    let path = jsonl_path_for_session(sf);
                    let (events, ts) = parse_jsonl_full(&path);
                    tail_states.insert(sid.clone(), ts);
                    let subs = discover_subagents(&path);
                    all_sessions.push(build_session_data(sid, &short_name(&sf.cwd), &short_name(&sf.cwd), sf.pid, events, true, subs));
                }
            }
            if !all_sessions.is_empty() {
                data.lock().unwrap().sessions = all_sessions;
            }
        }

        std::thread::sleep(std::time::Duration::from_secs(2));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture_path() -> String {
        format!("{}/src/agent_harnesses/fixtures/sample_session.jsonl", env!("CARGO_MANIFEST_DIR"))
    }

    #[test]
    fn parse_sample_session_events() {
        let (events, _) = parse_jsonl_full(&fixture_path());
        insta::assert_yaml_snapshot!("events", events);
    }

    #[test]
    fn build_sample_session() {
        let (events, _) = parse_jsonl_full(&fixture_path());
        let sd = build_session_data("test-session-001", "cc-hud", "cc-hud", 12345, events, true, vec![]);

        let mut tool_counts: Vec<_> = sd.tool_counts.into_iter().collect();
        tool_counts.sort_by(|a, b| a.0.cmp(&b.0));
        let mut skill_counts: Vec<_> = sd.skill_counts.into_iter().collect();
        skill_counts.sort_by(|a, b| a.0.cmp(&b.0));
        let mut read_counts: Vec<_> = sd.read_counts.into_iter().collect();
        read_counts.sort_by(|a, b| a.0.cmp(&b.0));

        insta::assert_yaml_snapshot!("session_aggregates", &serde_json::json!({
            "session_id": sd.session_id,
            "total_cost_usd": (sd.total_cost_usd * 100000.0).round() / 100000.0,
            "total_input_cost": (sd.total_input_cost * 100000.0).round() / 100000.0,
            "total_output_cost": (sd.total_output_cost * 100000.0).round() / 100000.0,
            "total_input": sd.total_input,
            "total_output": sd.total_output,
            "api_call_count": sd.api_call_count,
            "agent_count": sd.agent_count,
            "tool_counts": tool_counts,
            "skill_counts": skill_counts,
            "read_counts": read_counts,
            "model": sd.model,
            "is_active": sd.is_active,
        }));
    }

    #[test]
    fn event_type_coverage() {
        let (events, _) = parse_jsonl_full(&fixture_path());

        let mut has_api = false;
        let mut has_tool = false;
        let mut has_skill = false;
        let mut has_read_file = false;
        let mut has_agent = false;
        let mut has_compaction = false;
        let mut has_thinking = false;

        for ev in &events {
            match ev {
                Event::ApiCall { has_thinking: t, .. } => {
                    has_api = true;
                    if *t { has_thinking = true; }
                }
                Event::ToolUse { .. } => has_tool = true,
                Event::SkillUse { .. } => has_skill = true,
                Event::ReadFile { .. } => has_read_file = true,
                Event::AgentSpawn { .. } => has_agent = true,
                Event::Compaction { .. } => has_compaction = true,
            }
        }

        assert!(has_api, "fixture must contain ApiCall events");
        assert!(has_tool, "fixture must contain ToolUse events");
        assert!(has_skill, "fixture must contain SkillUse events");
        assert!(has_read_file, "fixture must contain ReadFile events");
        assert!(has_agent, "fixture must contain AgentSpawn events");
        assert!(has_compaction, "fixture must contain Compaction events");
        assert!(has_thinking, "fixture must contain ApiCall with has_thinking=true");
    }
}

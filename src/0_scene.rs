/// Framework-agnostic scene tree. Pure functions produce `Vec<Node>`,
/// a backend (egui, dioxus, etc.) walks and renders synchronously.

/// Downsample a line to at most `max_pts` points, preserving first and last.
/// Uses uniform index sampling -- adequate for monotonic cumulative lines.
fn downsample_line(pts: &[[f64; 2]], max_pts: usize) -> Vec<[f64; 2]> {
    if pts.len() <= max_pts { return pts.to_vec(); }
    let mut out = Vec::with_capacity(max_pts);
    out.push(pts[0]);
    let step = (pts.len() - 1) as f64 / (max_pts - 1) as f64;
    for i in 1..max_pts - 1 {
        out.push(pts[(i as f64 * step) as usize]);
    }
    out.push(*pts.last().unwrap());
    out
}

// ---- shared formatting utilities ----

pub fn short_model_label(model: &str) -> &'static str {
    if model.contains("opus-4-6") { "opus4.6" }
    else if model.contains("opus-4-5") { "opus4.5" }
    else if model.contains("opus-4-1") { "opus4.1" }
    else if model.contains("opus-4-0") || model.contains("opus-4-2") { "opus4" }
    else if model.contains("opus") { "opus3" }
    else if model.contains("sonnet-4-6") { "sonnet4.6" }
    else if model.contains("sonnet-4-5") { "sonnet4.5" }
    else if model.contains("sonnet-4-1") { "sonnet4.1" }
    else if model.contains("sonnet-4-0") { "sonnet4" }
    else if model.contains("sonnet") { "sonnet" }
    else if model.contains("haiku-4-5") { "haiku4.5" }
    else if model.contains("haiku") { "haiku" }
    else if model.is_empty() { "?" }
    else { "other" }
}

// ---- geometry & styling primitives ----

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Color(pub u8, pub u8, pub u8, pub u8);

impl Color {
    pub const TRANSPARENT: Color = Color(0, 0, 0, 0);

    pub const fn rgb(r: u8, g: u8, b: u8) -> Self { Color(r, g, b, 255) }
    pub const fn rgba(r: u8, g: u8, b: u8, a: u8) -> Self { Color(r, g, b, a) }
    pub fn r(self) -> u8 { self.0 }
    pub fn g(self) -> u8 { self.1 }
    pub fn b(self) -> u8 { self.2 }
    pub fn a(self) -> u8 { self.3 }
}

#[derive(Clone, Copy, Debug)]
pub struct Stroke {
    pub color: Color,
    pub width: f32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Anchor {
    LeftCenter,
    LeftTop,
    RightCenter,
    CenterCenter,
}

// ---- chart data types ----

#[derive(Clone, Debug)]
pub struct BarData {
    pub x: f64,
    pub height: f64,
    pub width: f64,
    pub color: Color,
    pub session_idx: usize,
}

#[derive(Clone, Debug)]
pub struct LineSeries {
    pub points: Vec<[f64; 2]>,
    pub color: Color,
    pub width: f32,
    pub dashed: bool,
    pub name: String,
}

#[derive(Clone, Debug)]
pub struct Marker {
    pub x: f64,
    pub color: Color,
    pub width: f32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum YFormat {
    Cost,
    CostAbs,
    Tokens,
    Percent,
    None,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum XFormat {
    Time,
    Turn,
    Hidden,
}

#[derive(Clone, Debug)]
pub struct AxisConfig {
    pub show_x: bool,
    pub show_y: bool,
    pub show_grid: bool,
    pub y_fmt: YFormat,
    pub x_fmt: XFormat,
    pub y_min: Option<f64>,
    pub y_max: Option<f64>,
    pub x_min: Option<f64>,
    pub x_max: Option<f64>,
}

impl Default for AxisConfig {
    fn default() -> Self {
        Self {
            show_x: false,
            show_y: false,
            show_grid: false,
            y_fmt: YFormat::None,
            x_fmt: XFormat::Hidden,
            y_min: None,
            y_max: None,
            x_min: None,
            x_max: None,
        }
    }
}

// ---- tooltip ----

#[derive(Clone, Debug)]
pub struct TooltipLine {
    pub color: Option<Color>,
    pub text: String,
}

// ---- legend ----

#[derive(Clone, Debug)]
pub struct TimelineBar {
    pub start_frac: f32,
    pub end_frac: f32,
    pub color: Color,
    pub active: bool,
}

// ---- scene node ----

#[derive(Clone, Debug)]
pub enum Node {
    // Containers
    Panel { children: Vec<Node> },
    Scroll { id: String, children: Vec<Node> },
    Clip { children: Vec<Node> },

    // Charts
    BarChart {
        id: String,
        up_bars: Vec<BarData>,
        down_bars: Vec<BarData>,
        markers: Vec<Marker>,
        axis: AxisConfig,
    },
    LineChart {
        id: String,
        lines: Vec<LineSeries>,
        markers: Vec<Marker>,
        axis: AxisConfig,
    },

    // Composite widgets
    BarRow {
        label: String,
        count: u32,
        max: f32,
        color: Color,
        highlighted: bool,
        /// If set, renderer reports this key when row is hovered (for cross-widget highlighting).
        hover_key: Option<String>,
    },
    SectionLabel { text: String },
    ChartLabel {
        title: String,
        left_sub: String,
        right_sub: String,
    },
    LegendRow {
        color: Color,
        name: String,
        stats: String,
        timeline: Option<TimelineBar>,
    },
    SubagentRow {
        indent: f32,
        type_label: String,
        cost: String,
        color: Color,
    },
    Tooltip { lines: Vec<TooltipLine> },
    HLine { y: f64, color: Color, width: f32 },

    // Primitives (escape hatch)
    Rect { rounding: f32, color: Color },
    Text {
        anchor: Anchor,
        text: String,
        font_size: f32,
        color: Color,
    },
    Circle { radius: f32, color: Color },
    Line {
        a: (f32, f32),
        b: (f32, f32),
        stroke: Stroke,
    },
}

/// Convenience: build markers from agent/skill x-positions with highlight awareness.
pub fn build_markers(
    agent_xs: &[(f64, String)],
    skill_xs: &[(f64, String)],
    compaction_xs: &[f64],
    highlight_key: &str,
) -> Vec<Marker> {
    let has_hl = !highlight_key.is_empty();
    let mut out = Vec::with_capacity(agent_xs.len() + skill_xs.len() + compaction_xs.len());

    for (x, atype) in agent_xs {
        let is_hl = has_hl && highlight_key == format!("agent:{}", atype);
        let (color, width) = if is_hl {
            (Color::rgba(220, 80, 80, 180), 2.0)
        } else if has_hl {
            (Color::rgba(180, 60, 60, 20), 0.3)
        } else {
            (Color::rgba(180, 60, 60, 60), 0.5)
        };
        out.push(Marker { x: *x, color, width });
    }

    for (x, sname) in skill_xs {
        let is_hl = has_hl && highlight_key == format!("skill:{}", sname);
        let (color, width) = if is_hl {
            (Color::rgba(80, 220, 140, 200), 2.0)
        } else if has_hl {
            (Color::rgba(60, 180, 120, 20), 0.3)
        } else {
            (Color::rgba(60, 180, 120, 60), 0.5)
        };
        out.push(Marker { x: *x, color, width });
    }

    // Compaction boundaries: yellow dashed-style
    for x in compaction_xs {
        let (color, width) = if has_hl {
            (Color::rgba(220, 200, 60, 30), 0.5)
        } else {
            (Color::rgba(220, 200, 60, 100), 1.0)
        };
        out.push(Marker { x: *x, color, width });
    }

    out
}

/// Build the tool/skill/agent/reads panel as a node tree.
pub fn build_tool_panel(
    skill_list: &[(String, u32)],
    agent_list: &[(String, u32)],
    read_list: &[(String, u32)],
    tool_list: &[(String, u32)],
    highlight_key: &str,
) -> Vec<Node> {
    let mut children = Vec::new();

    // Skills (top priority)
    if !skill_list.is_empty() {
        children.push(Node::SectionLabel { text: "skills".into() });
        let sk_max = skill_list[0].1.max(1) as f32;
        for (name, count) in skill_list {
            let short = name.rsplit(':').next().unwrap_or(name);
            let key = format!("skill:{}", name);
            let is_hl = highlight_key == key;
            children.push(Node::BarRow {
                label: short.to_string(),
                count: *count,
                max: sk_max,
                color: Color::rgba(60, 180, 120, 60),
                highlighted: is_hl,
                hover_key: Some(key),
            });
        }
    }

    // Agents
    if !agent_list.is_empty() {
        if !skill_list.is_empty() { children.push(Node::SectionLabel { text: String::new() }); }
        children.push(Node::SectionLabel { text: "agents".into() });
        let ag_max = agent_list[0].1.max(1) as f32;
        for (name, count) in agent_list {
            let key = format!("agent:{}", name);
            let is_hl = highlight_key == key;
            children.push(Node::BarRow {
                label: name.clone(),
                count: *count,
                max: ag_max,
                color: Color::rgba(180, 60, 60, 60),
                highlighted: is_hl,
                hover_key: Some(key),
            });
        }
    }

    // Reads
    if !read_list.is_empty() {
        if !skill_list.is_empty() || !agent_list.is_empty() {
            children.push(Node::SectionLabel { text: String::new() });
        }
        children.push(Node::SectionLabel { text: "reads".into() });
        let rd_max = read_list[0].1.max(1) as f32;
        for (name, count) in read_list {
            children.push(Node::BarRow {
                label: name.clone(),
                count: *count,
                max: rd_max,
                color: Color::rgba(100, 160, 220, 180),
                highlighted: false,
                hover_key: None,
            });
        }
    }

    // Tool calls (bottom)
    if !tool_list.is_empty() {
        children.push(Node::SectionLabel { text: String::new() });
        children.push(Node::SectionLabel { text: "tool calls".into() });
        let max_count = tool_list[0].1.max(1) as f32;
        for (name, count) in tool_list {
            children.push(Node::BarRow {
                label: name.clone(),
                count: *count,
                max: max_count,
                color: Color::rgba(71, 77, 88, 160),
                highlighted: false,
                hover_key: None,
            });
        }
    }

    vec![Node::Scroll {
        id: "tool_panel_scroll".into(),
        children,
    }]
}

// ---- shared palette & formatting ----

pub const SESSION_COLORS: &[(u8, u8, u8)] = &[
    (190, 120, 20),   // amber
    (80, 180, 120),   // green
    (100, 140, 220),  // blue
    (200, 80, 80),    // red
    (180, 130, 200),  // purple
    (200, 180, 80),   // gold
];

pub fn session_color(i: usize) -> Color {
    let (r, g, b) = SESSION_COLORS[i % SESSION_COLORS.len()];
    Color::rgb(r, g, b)
}

pub fn format_cost(usd: f64) -> String {
    if usd < 0.001 { format!("${:.5}", usd) }
    else if usd < 0.01 { format!("${:.4}", usd) }
    else if usd < 1.0 { format!("${:.3}", usd) }
    else { format!("${:.2}", usd) }
}

pub fn format_tokens(n: u64) -> String {
    if n >= 1_000_000 { format!("{:.1}M", n as f64 / 1_000_000.0) }
    else if n >= 1_000 { format!("{}k", n / 1_000) }
    else { format!("{}", n) }
}

// ---- chart data (framework-agnostic) ----

use crate::agent_harnesses::claude_code::{self, Event, HudData};
use crate::energy::{self, EnergyConfig, EnergyEstimate, TokenCounts};
use std::collections::{HashMap, HashSet};

#[derive(Clone, Default)]
pub struct TurnInfo {
    pub x: f64,
    /// Timestamp as minutes-from-epoch (always set, regardless of axis mode).
    pub ts_x: f64,
    pub in_cost: f64,
    pub out_cost: f64,
    pub cost_change: f64,
    pub in_tok: u64,
    pub out_tok: u64,
    pub cache_read_tok: u64,
    pub cache_create_tok: u64,
    /// Cost from fresh (uncached) input tokens only.
    pub fresh_input_cost: f64,
    /// Cost from cache read tokens.
    pub cache_read_cost: f64,
    /// Cost from cache creation tokens.
    pub cache_create_cost: f64,
    pub total_cost: f64,
    pub total_in_tok: u64,
    pub total_out_tok: u64,
    pub model_short: String,
    pub has_thinking: bool,
    /// Total context tokens this turn (input + cache_read + cache_create)
    pub context_tokens: u64,
    /// Context window limit for this model
    pub context_limit: u64,
    /// True if context_tokens dropped from previous turn (compaction happened)
    pub is_reset: bool,
    /// Per-turn energy estimate (midpoint values)
    pub energy: EnergyEstimate,
    /// Cumulative energy up to this turn
    pub cumulative_energy: EnergyEstimate,
}

pub struct ChartData {
    pub in_cost_bars: Vec<BarData>,
    /// Stacked breakdown of input cost: fresh (uncached) portion.
    pub in_cost_fresh_bars: Vec<BarData>,
    /// Stacked breakdown: cache read portion (stacked on fresh).
    pub in_cost_cache_read_bars: Vec<BarData>,
    /// Stacked breakdown: cache create portion (stacked on fresh + read).
    pub in_cost_cache_create_bars: Vec<BarData>,
    pub out_cost_bars: Vec<BarData>,
    pub in_tok_bars: Vec<BarData>,
    /// Stacked token breakdown: fresh input tokens.
    pub in_tok_fresh_bars: Vec<BarData>,
    /// Stacked token breakdown: cache read tokens.
    pub in_tok_cache_read_bars: Vec<BarData>,
    /// Stacked token breakdown: cache create tokens.
    pub in_tok_cache_create_bars: Vec<BarData>,
    pub out_tok_bars: Vec<BarData>,
    pub agent_xs: Vec<(f64, String)>,
    pub skill_xs: Vec<(f64, String)>,
    pub per_turn_in_cost_max: f64,
    pub per_turn_out_cost_max: f64,
    pub in_max: f64,
    pub out_max: f64,
    pub tool_list: Vec<(String, u32)>,
    pub skill_list: Vec<(String, u32)>,
    pub read_list: Vec<(String, u32)>,
    pub agent_list: Vec<(String, u32)>,
    pub total_cost_lines: Vec<(Color, Vec<[f64; 2]>)>,
    pub total_cost_max: f64,
    pub total_tok_lines: Vec<(Color, Vec<[f64; 2]>, Vec<[f64; 2]>)>,
    pub total_tok_max: f64,
    pub combined_cost_pts: Vec<[f64; 2]>,
    pub combined_cost_max: f64,
    pub cost_rate_pts: Vec<[f64; 2]>,
    pub cost_rate_max: f64,
    /// Combined cost points always in time-based x (minutes-from-epoch), for budget chart.
    pub budget_cost_pts: Vec<[f64; 2]>,
    pub budget_cost_max: f64,
    pub compaction_xs: Vec<f64>,
    pub session_turns: Vec<(String, Color, Vec<TurnInfo>)>,
    /// Per-turn energy bars (Wh midpoint)
    pub energy_wh_bars: Vec<BarData>,
    /// Per-turn water bars (mL midpoint)
    pub water_ml_bars: Vec<BarData>,
    /// Per-turn local cost bars (USD)
    pub local_cost_bars: Vec<BarData>,
    pub energy_wh_max: f64,
    pub water_ml_max: f64,
    pub local_cost_max: f64,
    /// Cumulative energy lines per session: (color, Wh_mid points)
    pub total_energy_lines: Vec<(Color, Vec<[f64; 2]>)>,
    /// Cumulative water lines per session: (color, mL_mid points)
    pub total_water_lines: Vec<(Color, Vec<[f64; 2]>)>,
    pub total_energy_max: f64,
    pub total_water_max: f64,
}

pub fn build_chart_data(data: &HudData, hidden: &HashSet<String>, time_axis: bool) -> ChartData {
    let mut per_turn_in_cost_max = 0.001_f64;
    let mut per_turn_out_cost_max = 0.001_f64;
    let mut in_max = 100.0_f64;
    let mut out_max = 100.0_f64;
    let mut agg_tools: HashMap<String, u32> = HashMap::new();
    let mut agg_skills: HashMap<String, u32> = HashMap::new();
    let mut agg_reads: HashMap<String, u32> = HashMap::new();
    let mut agg_agents: HashMap<String, u32> = HashMap::new();
    let mut in_cost_bars: Vec<BarData> = vec![];
    let mut in_cost_fresh_bars: Vec<BarData> = vec![];
    let mut in_cost_cache_read_bars: Vec<BarData> = vec![];
    let mut in_cost_cache_create_bars: Vec<BarData> = vec![];
    let mut out_cost_bars: Vec<BarData> = vec![];
    let mut in_tok_bars: Vec<BarData> = vec![];
    let mut in_tok_fresh_bars: Vec<BarData> = vec![];
    let mut in_tok_cache_read_bars: Vec<BarData> = vec![];
    let mut in_tok_cache_create_bars: Vec<BarData> = vec![];
    let mut out_tok_bars: Vec<BarData> = vec![];
    let mut agent_xs: Vec<(f64, String)> = vec![];
    let mut skill_xs: Vec<(f64, String)> = vec![];
    let mut compaction_xs: Vec<f64> = vec![];
    let mut session_turns: Vec<(String, Color, Vec<TurnInfo>)> = vec![];
    let mut energy_wh_bars: Vec<BarData> = vec![];
    let mut water_ml_bars: Vec<BarData> = vec![];
    let mut local_cost_bars: Vec<BarData> = vec![];
    let mut energy_wh_max = 0.001_f64;
    let mut water_ml_max = 0.001_f64;
    let mut local_cost_max = 0.0001_f64;
    let energy_config = EnergyConfig::default();

    let mut total_api_calls = 0usize;
    let mut visible_session_count = 0usize;
    let (mut ts_min, mut ts_max) = (u64::MAX, 0u64);
    for session in &data.sessions {
        if hidden.contains(&session.session_id) { continue; }
        visible_session_count += 1;
        for ev in &session.events {
            if let Event::ApiCall { timestamp_secs, .. } = ev {
                if *timestamp_secs > 0 {
                    ts_min = ts_min.min(*timestamp_secs);
                    ts_max = ts_max.max(*timestamp_secs);
                }
                total_api_calls += 1;
            }
        }
    }
    let time_span_min = if ts_max > ts_min { (ts_max - ts_min) as f64 / 60.0 } else { 60.0 };
    // Bar width in minutes: thin enough that bars don't bloat when zoomed in.
    // Use median inter-event gap as the base, capped at a small fraction of total span.
    let time_bar_w = (time_span_min / total_api_calls.max(1) as f64 * 0.4)
        .max(0.5)             // at least 30 seconds wide
        .min(time_span_min * 0.002); // never more than 0.2% of total span

    // Downsampling: bucket into time intervals so TOTAL bars across all sessions stays bounded.
    // Each session can produce up to (time_span / bucket_minutes) bars, so bucket_minutes must
    // account for session count to keep the grand total under max_bars.
    let max_bars_total = 600usize;
    let max_bars_per_session = (max_bars_total / visible_session_count.max(1)).max(4);
    let downsample = time_axis && total_api_calls > max_bars_total;
    let bucket_minutes = if downsample {
        time_span_min / max_bars_per_session as f64
    } else {
        0.0
    };
    // Bar width for downsampled bars: fill the bucket but cap to prevent absurdly fat bars.
    // Max width ~0.5% of total span so bars stay visually distinct.
    let bucket_w = if downsample {
        (bucket_minutes * 0.85).min(time_span_min * 0.005)
    } else { 0.0 };

    // Pre-pass: count visible sessions per project name so we can index duplicates
    let mut project_name_counts: HashMap<String, usize> = HashMap::new();
    for session in &data.sessions {
        if hidden.contains(&session.session_id) { continue; }
        *project_name_counts.entry(session.project.clone()).or_default() += 1;
    }
    let mut project_name_idx: HashMap<String, usize> = HashMap::new();

    for (si, session) in data.sessions.iter().enumerate() {
        if hidden.contains(&session.session_id) { continue; }

        for (name, count) in &session.tool_counts { *agg_tools.entry(name.clone()).or_default() += count; }
        for (name, count) in &session.skill_counts { *agg_skills.entry(name.clone()).or_default() += count; }
        for (name, count) in &session.read_counts { *agg_reads.entry(name.clone()).or_default() += count; }
        for agent in &session.subagents { *agg_agents.entry(agent.agent_type.clone()).or_default() += 1; }

        let mut turns: Vec<TurnInfo> = vec![];
        let mut total_in_cost = 0.0f64;
        let mut total_out_cost = 0.0f64;
        let mut prev_total_cost = 0.0f64;
        let mut total_in_tok = 0u64;
        let mut total_out_tok = 0u64;
        let mut api_idx = 0usize;
        let mut last_x = 0f64;
        let mut cum_energy = EnergyEstimate::default();

        // Bucket accumulator for downsampled bars (only used when downsample == true)
        struct Bucket {
            x: f64, // bucket center x
            in_cost: f64, out_cost: f64,
            fresh_cost: f64, cr_cost: f64, cc_cost: f64,
            in_tok: f64, in_tok_fresh: f64, in_tok_cr: f64, in_tok_cc: f64,
            out_tok: f64,
            wh: f64, wml: f64, lc: f64,
            count: u32,
        }
        impl Bucket {
            fn empty(x: f64) -> Self {
                Bucket { x, in_cost: 0.0, out_cost: 0.0, fresh_cost: 0.0, cr_cost: 0.0, cc_cost: 0.0,
                    in_tok: 0.0, in_tok_fresh: 0.0, in_tok_cr: 0.0, in_tok_cc: 0.0,
                    out_tok: 0.0, wh: 0.0, wml: 0.0, lc: 0.0, count: 0 }
            }
        }
        let mut cur_bucket: Option<Bucket> = None;
        let mut cur_bucket_id: i64 = i64::MIN;

        // Flush accumulated bucket into bar arrays
        let flush_bucket = |bkt: &Bucket, si: usize,
            in_cost_bars: &mut Vec<BarData>, out_cost_bars: &mut Vec<BarData>,
            in_cost_fresh_bars: &mut Vec<BarData>, in_cost_cache_read_bars: &mut Vec<BarData>, in_cost_cache_create_bars: &mut Vec<BarData>,
            in_tok_bars: &mut Vec<BarData>, in_tok_fresh_bars: &mut Vec<BarData>, in_tok_cache_read_bars: &mut Vec<BarData>, in_tok_cache_create_bars: &mut Vec<BarData>,
            out_tok_bars: &mut Vec<BarData>,
            energy_wh_bars: &mut Vec<BarData>, water_ml_bars: &mut Vec<BarData>, local_cost_bars: &mut Vec<BarData>,
            energy_wh_max: &mut f64, water_ml_max: &mut f64, local_cost_max: &mut f64,
            per_turn_in_cost_max: &mut f64, per_turn_out_cost_max: &mut f64, in_max: &mut f64, out_max: &mut f64,
            bw: f64| {
            if bkt.count == 0 { return; }
            let x = bkt.x;
            *per_turn_in_cost_max = per_turn_in_cost_max.max(bkt.in_cost);
            *per_turn_out_cost_max = per_turn_out_cost_max.max(bkt.out_cost);
            *in_max = in_max.max(bkt.in_tok);
            *out_max = out_max.max(bkt.out_tok.abs());
            in_cost_bars.push(BarData { x, height: bkt.in_cost, width: bw, color: Color::rgba(100, 160, 220, 180), session_idx: si });
            out_cost_bars.push(BarData { x, height: -(bkt.out_cost), width: bw, color: Color::rgba(220, 160, 60, 180), session_idx: si });
            in_cost_fresh_bars.push(BarData { x, height: bkt.fresh_cost, width: bw, color: Color::rgba(60, 120, 200, 220), session_idx: si });
            in_cost_cache_read_bars.push(BarData { x, height: bkt.cr_cost, width: bw, color: Color::rgba(80, 180, 100, 160), session_idx: si });
            in_cost_cache_create_bars.push(BarData { x, height: bkt.cc_cost, width: bw, color: Color::rgba(220, 160, 60, 160), session_idx: si });
            in_tok_bars.push(BarData { x, height: bkt.in_tok, width: bw, color: Color::rgba(100, 160, 220, 180), session_idx: si });
            in_tok_fresh_bars.push(BarData { x, height: bkt.in_tok_fresh, width: bw, color: Color::rgba(60, 120, 200, 220), session_idx: si });
            in_tok_cache_read_bars.push(BarData { x, height: bkt.in_tok_cr, width: bw, color: Color::rgba(80, 180, 100, 160), session_idx: si });
            in_tok_cache_create_bars.push(BarData { x, height: bkt.in_tok_cc, width: bw, color: Color::rgba(220, 160, 60, 160), session_idx: si });
            out_tok_bars.push(BarData { x, height: -(bkt.out_tok), width: bw, color: Color::rgba(220, 160, 60, 180), session_idx: si });
            energy_wh_bars.push(BarData { x, height: bkt.wh, width: bw, color: Color::rgba(120, 200, 80, 200), session_idx: si });
            water_ml_bars.push(BarData { x, height: bkt.wml, width: bw, color: Color::rgba(100, 160, 220, 200), session_idx: si });
            local_cost_bars.push(BarData { x, height: bkt.lc, width: bw, color: Color::rgba(220, 180, 80, 180), session_idx: si });
            *energy_wh_max = energy_wh_max.max(bkt.wh);
            *water_ml_max = water_ml_max.max(bkt.wml);
            *local_cost_max = local_cost_max.max(bkt.lc);
        };

        for ev in &session.events {
            match ev {
                Event::ApiCall { input_cost_usd, output_cost_usd, input_tokens, output_tokens,
                                 cache_read_tokens, cache_create_tokens, timestamp_secs, model, has_thinking, .. } => {
                    let x = if time_axis { *timestamp_secs as f64 / 60.0 } else { api_idx as f64 };
                    let bar_w = if time_axis { time_bar_w } else { 0.8 };

                    // Compute per-component input cost breakdown
                    let (pi, _po, pcr, pcc) = claude_code::model_pricing(model);
                    let fresh_cost = (*input_tokens as f64 * pi) / 1_000_000.0;
                    let cr_cost = (*cache_read_tokens as f64 * pcr) / 1_000_000.0;
                    let cc_cost = (*cache_create_tokens as f64 * pcc) / 1_000_000.0;

                    let total_in_tokens = input_tokens + cache_read_tokens + cache_create_tokens;

                    // Per-turn energy estimate (always computed for cumulative tracking)
                    let turn_tokens = TokenCounts {
                        input_tokens: *input_tokens,
                        output_tokens: *output_tokens,
                        cache_read_tokens: *cache_read_tokens,
                        cache_create_tokens: *cache_create_tokens,
                    };
                    let api_cost = input_cost_usd + output_cost_usd;
                    let turn_energy = energy::estimate_for_model(&turn_tokens, model, api_cost, &energy_config);
                    let wh = turn_energy.facility_kwh.mid * 1000.0;
                    let wml = turn_energy.water_total_ml.mid;
                    let lc = turn_energy.local_cost_usd;

                    if downsample {
                        // Bucket bars by time interval
                        let bid = (x / bucket_minutes) as i64;
                        if bid != cur_bucket_id {
                            // Flush previous bucket
                            if let Some(bkt) = cur_bucket.take() {
                                flush_bucket(&bkt, si,
                                    &mut in_cost_bars, &mut out_cost_bars,
                                    &mut in_cost_fresh_bars, &mut in_cost_cache_read_bars, &mut in_cost_cache_create_bars,
                                    &mut in_tok_bars, &mut in_tok_fresh_bars, &mut in_tok_cache_read_bars, &mut in_tok_cache_create_bars,
                                    &mut out_tok_bars,
                                    &mut energy_wh_bars, &mut water_ml_bars, &mut local_cost_bars,
                                    &mut energy_wh_max, &mut water_ml_max, &mut local_cost_max,
                                    &mut per_turn_in_cost_max, &mut per_turn_out_cost_max, &mut in_max, &mut out_max,
                                    bucket_w);
                            }
                            cur_bucket_id = bid;
                            cur_bucket = Some(Bucket::empty((bid as f64 + 0.5) * bucket_minutes));
                        }
                        let bkt = cur_bucket.as_mut().unwrap();
                        bkt.in_cost += *input_cost_usd;
                        bkt.out_cost += *output_cost_usd;
                        bkt.fresh_cost += fresh_cost;
                        bkt.cr_cost += cr_cost;
                        bkt.cc_cost += cc_cost;
                        bkt.in_tok += total_in_tokens as f64;
                        bkt.in_tok_fresh += *input_tokens as f64;
                        bkt.in_tok_cr += *cache_read_tokens as f64;
                        bkt.in_tok_cc += *cache_create_tokens as f64;
                        bkt.out_tok += *output_tokens as f64;
                        bkt.wh += wh;
                        bkt.wml += wml;
                        bkt.lc += lc;
                        bkt.count += 1;
                    } else {
                        // Original per-bar rendering
                        per_turn_in_cost_max = per_turn_in_cost_max.max(*input_cost_usd);
                        per_turn_out_cost_max = per_turn_out_cost_max.max(*output_cost_usd);
                        in_max = in_max.max(total_in_tokens as f64);
                        out_max = out_max.max(*output_tokens as f64);

                        in_cost_bars.push(BarData { x, height: *input_cost_usd, width: bar_w, color: Color::rgba(100, 160, 220, 180), session_idx: si });
                        let out_cost_color = if *has_thinking { Color::rgba(180, 80, 200, 200) } else { Color::rgba(220, 160, 60, 180) };
                        out_cost_bars.push(BarData { x, height: -(*output_cost_usd), width: bar_w, color: out_cost_color, session_idx: si });
                        in_cost_fresh_bars.push(BarData { x, height: fresh_cost, width: bar_w, color: Color::rgba(60, 120, 200, 220), session_idx: si });
                        in_cost_cache_read_bars.push(BarData { x, height: cr_cost, width: bar_w, color: Color::rgba(80, 180, 100, 160), session_idx: si });
                        in_cost_cache_create_bars.push(BarData { x, height: cc_cost, width: bar_w, color: Color::rgba(220, 160, 60, 160), session_idx: si });
                        in_tok_bars.push(BarData { x, height: total_in_tokens as f64, width: bar_w, color: Color::rgba(100, 160, 220, 180), session_idx: si });
                        in_tok_fresh_bars.push(BarData { x, height: *input_tokens as f64, width: bar_w, color: Color::rgba(60, 120, 200, 220), session_idx: si });
                        in_tok_cache_read_bars.push(BarData { x, height: *cache_read_tokens as f64, width: bar_w, color: Color::rgba(80, 180, 100, 160), session_idx: si });
                        in_tok_cache_create_bars.push(BarData { x, height: *cache_create_tokens as f64, width: bar_w, color: Color::rgba(220, 160, 60, 160), session_idx: si });
                        let out_color = if *has_thinking { Color::rgba(180, 80, 200, 200) } else { Color::rgba(220, 160, 60, 180) };
                        out_tok_bars.push(BarData { x, height: -(*output_tokens as f64), width: bar_w, color: out_color, session_idx: si });
                        energy_wh_bars.push(BarData { x, height: wh, width: bar_w, color: Color::rgba(120, 200, 80, 200), session_idx: si });
                        water_ml_bars.push(BarData { x, height: wml, width: bar_w, color: Color::rgba(100, 160, 220, 200), session_idx: si });
                        local_cost_bars.push(BarData { x, height: lc, width: bar_w, color: Color::rgba(220, 180, 80, 180), session_idx: si });
                        energy_wh_max = energy_wh_max.max(wh);
                        water_ml_max = water_ml_max.max(wml);
                        local_cost_max = local_cost_max.max(lc);
                    }

                    cum_energy.accumulate(&turn_energy);

                    total_in_cost += input_cost_usd;
                    total_out_cost += output_cost_usd;
                    total_in_tok += total_in_tokens;
                    total_out_tok += output_tokens;

                    let cur_total = total_in_cost + total_out_cost;
                    let context_tokens = total_in_tokens;
                    let context_limit = claude_code::model_context_window(model);
                    let prev_context = turns.last().map(|t| t.context_tokens).unwrap_or(0);
                    let is_reset = context_tokens < prev_context.saturating_sub(prev_context / 4);

                    turns.push(TurnInfo {
                        x,
                        ts_x: *timestamp_secs as f64 / 60.0,
                        in_cost: *input_cost_usd,
                        out_cost: *output_cost_usd,
                        cost_change: cur_total - prev_total_cost,
                        in_tok: *input_tokens,
                        out_tok: *output_tokens,
                        cache_read_tok: *cache_read_tokens,
                        cache_create_tok: *cache_create_tokens,
                        fresh_input_cost: fresh_cost,
                        cache_read_cost: cr_cost,
                        cache_create_cost: cc_cost,
                        total_cost: cur_total,
                        total_in_tok,
                        total_out_tok,
                        model_short: short_model_label(model).to_string(),
                        has_thinking: *has_thinking,
                        context_tokens,
                        context_limit,
                        is_reset,
                        energy: turn_energy,
                        cumulative_energy: cum_energy.clone(),
                    });
                    prev_total_cost = cur_total;
                    last_x = x;
                    api_idx += 1;
                }
                Event::AgentSpawn { subagent_type, .. } => { agent_xs.push((last_x + 0.15, subagent_type.clone())); }
                Event::SkillUse { skill, .. } => { skill_xs.push((last_x + 0.10, skill.clone())); }
                Event::Compaction { timestamp_secs, .. } => {
                    let x = if time_axis { *timestamp_secs as f64 / 60.0 } else { api_idx as f64 };
                    compaction_xs.push(x);
                }
                _ => {}
            }
        }

        // Flush final bucket if downsampling
        if let Some(bkt) = cur_bucket.take() {
            flush_bucket(&bkt, si,
                &mut in_cost_bars, &mut out_cost_bars,
                &mut in_cost_fresh_bars, &mut in_cost_cache_read_bars, &mut in_cost_cache_create_bars,
                &mut in_tok_bars, &mut in_tok_fresh_bars, &mut in_tok_cache_read_bars, &mut in_tok_cache_create_bars,
                &mut out_tok_bars,
                &mut energy_wh_bars, &mut water_ml_bars, &mut local_cost_bars,
                &mut energy_wh_max, &mut water_ml_max, &mut local_cost_max,
                &mut per_turn_in_cost_max, &mut per_turn_out_cost_max, &mut in_max, &mut out_max,
                bucket_w);
        }

        let display_name = {
            let count = project_name_counts.get(&session.project).copied().unwrap_or(1);
            if count > 1 {
                let idx = project_name_idx.entry(session.project.clone()).or_default();
                *idx += 1;
                format!("{}(#{})", session.project, idx)
            } else {
                session.project.clone()
            }
        };
        session_turns.push((display_name, session_color(si), turns));
    }

    let sort_desc = |v: &mut Vec<(String, u32)>| v.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    let mut tool_list: Vec<(String, u32)> = agg_tools.into_iter().collect(); sort_desc(&mut tool_list);
    let mut skill_list: Vec<(String, u32)> = agg_skills.into_iter().collect(); sort_desc(&mut skill_list);
    let mut read_list: Vec<(String, u32)> = agg_reads.into_iter().collect(); sort_desc(&mut read_list);
    let mut agent_list: Vec<(String, u32)> = agg_agents.into_iter().collect(); sort_desc(&mut agent_list);

    let mut total_cost_lines: Vec<(Color, Vec<[f64; 2]>)> = vec![];
    let mut total_cost_max = 0.001_f64;
    let mut total_tok_lines: Vec<(Color, Vec<[f64; 2]>, Vec<[f64; 2]>)> = vec![];
    let mut total_tok_max = 1.0_f64;

    // Max points per line series -- more than enough for screen resolution,
    // prevents egui_plot from choking on thousands of points * hundreds of sessions.
    let max_line_pts = 300;

    // Sort helper: ensure line points are x-monotonic (timestamps can be non-monotonic
    // within a session due to streaming partials, subagent interleaving, etc.)
    fn sort_by_x(mut pts: Vec<[f64; 2]>) -> Vec<[f64; 2]> {
        pts.sort_by(|a, b| a[0].partial_cmp(&b[0]).unwrap_or(std::cmp::Ordering::Equal));
        pts
    }

    for (_, color, turns) in &session_turns {
        if turns.is_empty() { continue; }
        let cost_pts = sort_by_x(turns.iter().map(|t| [t.x, t.total_cost]).collect());
        let in_tok_pts = sort_by_x(turns.iter().map(|t| [t.x, t.total_in_tok as f64]).collect());
        let out_tok_pts = sort_by_x(turns.iter().map(|t| [t.x, t.total_out_tok as f64]).collect());
        if let Some(last) = turns.last() {
            total_cost_max = total_cost_max.max(last.total_cost);
            total_tok_max = total_tok_max.max(last.total_in_tok as f64);
        }
        total_cost_lines.push((*color, downsample_line(&cost_pts, max_line_pts)));
        total_tok_lines.push((*color, downsample_line(&in_tok_pts, max_line_pts), downsample_line(&out_tok_pts, max_line_pts)));
    }

    let mut total_energy_lines: Vec<(Color, Vec<[f64; 2]>)> = vec![];
    let mut total_water_lines: Vec<(Color, Vec<[f64; 2]>)> = vec![];
    let mut total_energy_max = 0.001_f64;
    let mut total_water_max = 0.001_f64;
    for (_, color, turns) in &session_turns {
        if turns.is_empty() { continue; }
        let energy_pts = sort_by_x(turns.iter()
            .map(|t| [t.x, t.cumulative_energy.facility_kwh.mid * 1000.0])
            .collect());
        let water_pts = sort_by_x(turns.iter()
            .map(|t| [t.x, t.cumulative_energy.water_total_ml.mid])
            .collect());
        if let Some(last) = turns.last() {
            total_energy_max = total_energy_max.max(last.cumulative_energy.facility_kwh.mid * 1000.0);
            total_water_max = total_water_max.max(last.cumulative_energy.water_total_ml.mid);
        }
        total_energy_lines.push((*color, downsample_line(&energy_pts, max_line_pts)));
        total_water_lines.push((*color, downsample_line(&water_pts, max_line_pts)));
    }

    let mut all_cost_events: Vec<(f64, f64)> = vec![];
    for (_, _, turns) in &session_turns {
        for t in turns { all_cost_events.push((t.x, t.cost_change)); }
    }
    all_cost_events.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));

    let mut running = 0.0f64;
    let mut combined_cost_pts: Vec<[f64; 2]> = Vec::with_capacity(all_cost_events.len() + 1);
    let mut cost_rate_pts: Vec<[f64; 2]> = Vec::with_capacity(all_cost_events.len());
    if let Some((first_x, _)) = all_cost_events.first() {
        combined_cost_pts.push([*first_x, 0.0]);
    }
    for (x, dc) in &all_cost_events {
        running += dc;
        combined_cost_pts.push([*x, running]);
        cost_rate_pts.push([*x, *dc]);
    }
    let combined_cost_max = running.max(0.001);
    let cost_rate_max = all_cost_events.iter().map(|(_, c)| *c).fold(0.001_f64, f64::max);
    let combined_cost_pts = downsample_line(&combined_cost_pts, max_line_pts);
    let cost_rate_pts = downsample_line(&cost_rate_pts, max_line_pts);

    // Budget cost points: always time-based x (ts_x), for the budget chart in any axis mode
    let mut budget_events: Vec<(f64, f64)> = vec![];
    for (_, _, turns) in &session_turns {
        for t in turns { budget_events.push((t.ts_x, t.cost_change)); }
    }
    budget_events.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
    let mut budget_running = 0.0f64;
    let mut budget_cost_pts: Vec<[f64; 2]> = Vec::with_capacity(budget_events.len() + 1);
    if let Some((first_x, _)) = budget_events.first() {
        budget_cost_pts.push([*first_x, 0.0]);
    }
    for (x, dc) in &budget_events {
        budget_running += dc;
        budget_cost_pts.push([*x, budget_running]);
    }
    let budget_cost_max = budget_running.max(0.001);
    let budget_cost_pts = downsample_line(&budget_cost_pts, max_line_pts);

    ChartData {
        in_cost_bars, in_cost_fresh_bars, in_cost_cache_read_bars, in_cost_cache_create_bars,
        out_cost_bars, in_tok_bars, in_tok_fresh_bars, in_tok_cache_read_bars, in_tok_cache_create_bars,
        out_tok_bars, agent_xs, skill_xs,
        per_turn_in_cost_max, per_turn_out_cost_max, in_max, out_max, tool_list, skill_list, read_list, agent_list,
        total_cost_lines, total_cost_max, total_tok_lines, total_tok_max,
        combined_cost_pts, combined_cost_max, cost_rate_pts, cost_rate_max,
        budget_cost_pts, budget_cost_max,
        compaction_xs, session_turns,
        energy_wh_bars, water_ml_bars, local_cost_bars,
        energy_wh_max, water_ml_max, local_cost_max,
        total_energy_lines, total_water_lines, total_energy_max, total_water_max,
    }
}

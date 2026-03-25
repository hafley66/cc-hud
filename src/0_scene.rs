/// Framework-agnostic scene tree. Pure functions produce `Vec<Node>`,
/// a backend (egui, dioxus, etc.) walks and renders synchronously.

// ---- shared formatting utilities ----

pub fn short_model_label(model: &str) -> &'static str {
    if model.contains("opus-4-6") || model.contains("opus-4-5") { "opus4" }
    else if model.contains("opus") { "opus3" }
    else if model.contains("sonnet") { "sonnet" }
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
use crate::energy::{self, EnergyConfig, EnergyEstimate, ModelTier, TokenCounts};
use std::collections::{HashMap, HashSet};

#[derive(Clone, Default)]
pub struct TurnInfo {
    pub x: f64,
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
    let (mut ts_min, mut ts_max) = (u64::MAX, 0u64);
    for session in &data.sessions {
        if hidden.contains(&session.session_id) { continue; }
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
    let time_bar_w = (time_span_min / total_api_calls.max(1) as f64 * 0.6).max(time_span_min / 300.0).min(time_span_min / 60.0);

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

                    per_turn_in_cost_max = per_turn_in_cost_max.max(*input_cost_usd);
                    per_turn_out_cost_max = per_turn_out_cost_max.max(*output_cost_usd);
                    let total_in_tokens = input_tokens + cache_read_tokens + cache_create_tokens;
                    in_max = in_max.max(total_in_tokens as f64);
                    out_max = out_max.max(*output_tokens as f64);

                    // Total input cost bar (composite color)
                    in_cost_bars.push(BarData { x, height: *input_cost_usd, width: bar_w, color: Color::rgba(100, 160, 220, 180) });
                    let out_cost_color = if *has_thinking { Color::rgba(180, 80, 200, 200) } else { Color::rgba(220, 160, 60, 180) };
                    out_cost_bars.push(BarData { x, height: -(*output_cost_usd), width: bar_w, color: out_cost_color });

                    // Stacked input cost breakdown: fresh (bottom), cache_read (middle), cache_create (top)
                    in_cost_fresh_bars.push(BarData { x, height: fresh_cost, width: bar_w, color: Color::rgba(60, 120, 200, 220) });
                    in_cost_cache_read_bars.push(BarData { x, height: cr_cost, width: bar_w, color: Color::rgba(80, 180, 100, 160) });
                    in_cost_cache_create_bars.push(BarData { x, height: cc_cost, width: bar_w, color: Color::rgba(220, 160, 60, 160) });

                    // Token bars: total input (fresh + cached), not just fresh
                    in_tok_bars.push(BarData { x, height: total_in_tokens as f64, width: bar_w, color: Color::rgba(100, 160, 220, 180) });
                    in_tok_fresh_bars.push(BarData { x, height: *input_tokens as f64, width: bar_w, color: Color::rgba(60, 120, 200, 220) });
                    in_tok_cache_read_bars.push(BarData { x, height: *cache_read_tokens as f64, width: bar_w, color: Color::rgba(80, 180, 100, 160) });
                    in_tok_cache_create_bars.push(BarData { x, height: *cache_create_tokens as f64, width: bar_w, color: Color::rgba(220, 160, 60, 160) });
                    let out_color = if *has_thinking { Color::rgba(180, 80, 200, 200) } else { Color::rgba(220, 160, 60, 180) };
                    out_tok_bars.push(BarData { x, height: -(*output_tokens as f64), width: bar_w, color: out_color });

                    // Per-turn energy estimate
                    let turn_tokens = TokenCounts {
                        input_tokens: *input_tokens,
                        output_tokens: *output_tokens,
                        cache_read_tokens: *cache_read_tokens,
                        cache_create_tokens: *cache_create_tokens,
                    };
                    let tier = ModelTier::from_model_str(model);
                    let api_cost = input_cost_usd + output_cost_usd;
                    let turn_energy = energy::estimate(&turn_tokens, tier, api_cost, &energy_config);

                    // Energy bars: Wh (not kWh) for readable per-turn values
                    let wh = turn_energy.facility_kwh.mid * 1000.0;
                    let wml = turn_energy.water_total_ml.mid;
                    let lc = turn_energy.local_cost_usd;
                    energy_wh_bars.push(BarData { x, height: wh, width: bar_w, color: Color::rgba(120, 200, 80, 200) });
                    water_ml_bars.push(BarData { x, height: wml, width: bar_w, color: Color::rgba(100, 160, 220, 200) });
                    local_cost_bars.push(BarData { x, height: lc, width: bar_w, color: Color::rgba(220, 180, 80, 180) });
                    energy_wh_max = energy_wh_max.max(wh);
                    water_ml_max = water_ml_max.max(wml);
                    local_cost_max = local_cost_max.max(lc);

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

        session_turns.push((session.project.clone(), session_color(si), turns));
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

    for (_, color, turns) in &session_turns {
        if turns.is_empty() { continue; }
        let cost_pts: Vec<[f64; 2]> = turns.iter().map(|t| [t.x, t.total_cost]).collect();
        let in_tok_pts: Vec<[f64; 2]> = turns.iter().map(|t| [t.x, t.total_in_tok as f64]).collect();
        let out_tok_pts: Vec<[f64; 2]> = turns.iter().map(|t| [t.x, t.total_out_tok as f64]).collect();
        if let Some(last) = turns.last() {
            total_cost_max = total_cost_max.max(last.total_cost);
            total_tok_max = total_tok_max.max(last.total_in_tok as f64);
        }
        total_cost_lines.push((*color, cost_pts));
        total_tok_lines.push((*color, in_tok_pts, out_tok_pts));
    }

    let mut total_energy_lines: Vec<(Color, Vec<[f64; 2]>)> = vec![];
    let mut total_water_lines: Vec<(Color, Vec<[f64; 2]>)> = vec![];
    let mut total_energy_max = 0.001_f64;
    let mut total_water_max = 0.001_f64;
    for (_, color, turns) in &session_turns {
        if turns.is_empty() { continue; }
        let energy_pts: Vec<[f64; 2]> = turns.iter()
            .map(|t| [t.x, t.cumulative_energy.facility_kwh.mid * 1000.0]) // Wh
            .collect();
        let water_pts: Vec<[f64; 2]> = turns.iter()
            .map(|t| [t.x, t.cumulative_energy.water_total_ml.mid])
            .collect();
        if let Some(last) = turns.last() {
            total_energy_max = total_energy_max.max(last.cumulative_energy.facility_kwh.mid * 1000.0);
            total_water_max = total_water_max.max(last.cumulative_energy.water_total_ml.mid);
        }
        total_energy_lines.push((*color, energy_pts));
        total_water_lines.push((*color, water_pts));
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

    ChartData {
        in_cost_bars, in_cost_fresh_bars, in_cost_cache_read_bars, in_cost_cache_create_bars,
        out_cost_bars, in_tok_bars, in_tok_fresh_bars, in_tok_cache_read_bars, in_tok_cache_create_bars,
        out_tok_bars, agent_xs, skill_xs,
        per_turn_in_cost_max, per_turn_out_cost_max, in_max, out_max, tool_list, skill_list, read_list, agent_list,
        total_cost_lines, total_cost_max, total_tok_lines, total_tok_max,
        combined_cost_pts, combined_cost_max, cost_rate_pts, cost_rate_max,
        compaction_xs, session_turns,
        energy_wh_bars, water_ml_bars, local_cost_bars,
        energy_wh_max, water_ml_max, local_cost_max,
        total_energy_lines, total_water_lines, total_energy_max, total_water_max,
    }
}

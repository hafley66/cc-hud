#![allow(dead_code)]

mod geometry;
mod anchors;
mod agent_harnesses;
mod usage;

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use egui_overlay::EguiOverlay;
use egui_overlay::egui_render_wgpu::WgpuBackend as DefaultGfxBackend;
use egui_overlay::egui_window_glfw_passthrough::GlfwBackend;
use egui_plot::{Bar, BarChart, Plot, VLine};

use geometry::PixelRect;
use agent_harnesses::claude_code::{Event, HudData, SessionData};

const SESSION_COLORS: &[(u8, u8, u8)] = &[
    (190, 120, 20),   // amber
    (80, 180, 120),   // green
    (100, 140, 220),  // blue
    (200, 80, 80),    // red
    (180, 130, 200),  // purple
    (200, 180, 80),   // gold
];

fn main() {
    use tracing_subscriber::{fmt, prelude::*, EnvFilter};
    let log_file = std::fs::File::create("/tmp/cc-hud.log").expect("could not create log file");
    tracing_subscriber::registry()
        .with(fmt::layer().with_writer(log_file))
        .with(
            EnvFilter::try_from_default_env()
                .unwrap_or(EnvFilter::new("info,wgpu=warn,naga=warn")),
        )
        .init();

    let args: Vec<String> = std::env::args().collect();
    let show_history = args.iter().any(|a| a == "--history" || a == "-H");
    let big_mode = args.iter().any(|a| a == "--big" || a == "-b");

    let (state, poll_target) = if big_mode {
        // Fixed floating window — no tmux anchoring
        let r = Arc::new(Mutex::new(PixelRect { x: 100, y: 80, w: 960, h: 460 }));
        (r, None)
    } else {
        let target = match args.get(1).filter(|a| !a.starts_with('-')) {
            Some(arg) => anchors::tmux::TmuxTarget::parse(arg),
            None => match std::env::var("TMUX_PANE") {
                Ok(pane) => anchors::tmux::TmuxTarget::PaneId(pane),
                Err(_) => {
                    let session = std::process::Command::new("tmux")
                        .args(["display-message", "-p", "#{session_name}"])
                        .output()
                        .ok()
                        .and_then(|o| String::from_utf8(o.stdout).ok())
                        .map(|s| s.trim().to_string())
                        .unwrap_or_else(|| "0".to_string());
                    anchors::tmux::TmuxTarget::Session(session)
                }
            },
        };
        let initial = compute_pane_rect(&target).unwrap_or(PixelRect { x: 0, y: 0, w: 800, h: 60 });
        let r = Arc::new(Mutex::new(initial));
        (r, Some(target))
    };

    let visible = Arc::new(std::sync::atomic::AtomicBool::new(true));
    let hud_data = Arc::new(Mutex::new(HudData::default()));
    let usage_data = Arc::new(Mutex::new(usage::UsageData::default()));

    if let Some(target) = poll_target {
        let poll_state = state.clone();
        let poll_visible = visible.clone();
        std::thread::spawn(move || { pane_poll_loop(target, poll_state, poll_visible); });
    }

    let feed_data = hud_data.clone();
    std::thread::spawn(move || {
        agent_harnesses::claude_code::poll_loop(feed_data, show_history);
    });

    let feed_usage = usage_data.clone();
    std::thread::spawn(move || {
        usage::poll_loop(feed_usage, Duration::from_secs(90));
    });

    start_overlay(Hud { first_frame: true, state, visible, hud_data, usage_data, big_mode, hidden_sessions: HashSet::new(), show_active_only: true, time_axis: false, autofit: true, nav_view: None, expanded_groups: HashSet::new(), small_mode_session: None, pre_small_window_size: None });
}

fn compute_pane_rect(target: &anchors::tmux::TmuxTarget) -> Option<PixelRect> {
    let term_pid = anchors::terminal::terminal_pid()?;
    let pane = anchors::tmux::find_pane(target)?;
    let tty = pane.tty.as_deref()?;
    let metrics = anchors::terminal::cell_metrics_from_tty(tty)?;
    let origin = anchors::terminal::terminal_window_origin(term_pid)?;
    let insets = geometry::TerminalInsets::iterm2_default();
    Some(geometry::compute_overlay_rect(&pane.cell_rect, &metrics, &origin, &insets, 3, 1))
}

fn pane_poll_loop(
    target: anchors::tmux::TmuxTarget,
    state: Arc<Mutex<PixelRect>>,
    _visible: Arc<std::sync::atomic::AtomicBool>,
) {
    loop {
        if let Some(rect) = compute_pane_rect(&target) {
            let mut s = state.lock().unwrap();
            if *s != rect { *s = rect; }
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

struct Hud {
    first_frame: bool,
    state: Arc<Mutex<PixelRect>>,
    visible: Arc<std::sync::atomic::AtomicBool>,
    hud_data: Arc<Mutex<HudData>>,
    usage_data: Arc<Mutex<usage::UsageData>>,
    big_mode: bool,
    hidden_sessions: HashSet<String>,
    show_active_only: bool,
    time_axis: bool,
    autofit: bool,
    /// Chart viewport x-range in minutes-from-epoch. None = auto-fit to all data.
    nav_view: Option<(f64, f64)>,
    /// Which cwd groups have their session list expanded.
    expanded_groups: HashSet<String>,
    /// When Some, show small mode for this session id
    small_mode_session: Option<String>,
    /// Saved window size from before entering small mode
    pre_small_window_size: Option<(i32, i32)>,
}

// --- colors ---
struct Palette;
impl Palette {
    const BG: egui::Color32 = egui::Color32::from_rgba_premultiplied(12, 10, 7, 220);
    const BG_PANEL: egui::Color32 = egui::Color32::from_rgba_premultiplied(18, 15, 10, 220);
    const GRID: egui::Color32 = egui::Color32::from_rgba_premultiplied(50, 45, 35, 80);
    const TEXT: egui::Color32 = egui::Color32::from_rgba_premultiplied(200, 190, 165, 230);
    const TEXT_DIM: egui::Color32 = egui::Color32::from_rgba_premultiplied(130, 120, 100, 180);
    const TEXT_BRIGHT: egui::Color32 = egui::Color32::from_rgba_premultiplied(240, 230, 200, 255);
    const AGENT_MARKER: egui::Color32 = egui::Color32::from_rgba_premultiplied(180, 60, 60, 60);
    const INPUT_TINT: egui::Color32 = egui::Color32::from_rgba_premultiplied(100, 160, 220, 180);
    const OUTPUT_TINT: egui::Color32 = egui::Color32::from_rgba_premultiplied(220, 160, 60, 180);
    const TOOL_BAR: egui::Color32 = egui::Color32::from_rgba_premultiplied(71, 77, 88, 160);
    const SEPARATOR: egui::Color32 = egui::Color32::from_rgba_premultiplied(60, 55, 42, 120);
}

/// Which chart region the hover originated from.
#[derive(Clone, Copy, PartialEq, Eq)]
enum HoverSource { Cost, Tokens, TotalCost, TotalTokens, WeeklyCost, WeeklyRate }

/// Session time ranges highlighted from legend hover (stored as minutes-from-epoch).
#[derive(Clone, Default)]
struct LegendHighlight {
    /// (first_ts_min, last_ts_min) for each hovered session
    ranges: Vec<(f64, f64)>,
}

/// Wrapper for storing hover state in egui temp storage.
#[derive(Clone, Copy)]
struct HoverState { x: f64, source: HoverSource }

fn session_color(i: usize) -> egui::Color32 {
    let (r, g, b) = SESSION_COLORS[i % SESSION_COLORS.len()];
    egui::Color32::from_rgb(r, g, b)
}

fn format_cost(usd: f64) -> String {
    if usd < 0.001 { format!("${:.5}", usd) }
    else if usd < 0.01 { format!("${:.4}", usd) }
    else if usd < 1.0 { format!("${:.3}", usd) }
    else { format!("${:.2}", usd) }
}

/// Format epoch seconds as "YYYY/MM/DD HH:MM" in local time (24h).
fn format_epoch_local(epoch_secs: u64) -> String {
    let ts = epoch_secs as libc::time_t;
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    unsafe { libc::localtime_r(&ts, &mut tm); }
    format!("{:04}/{:02}/{:02} {:02}:{:02}",
        tm.tm_year + 1900, tm.tm_mon + 1, tm.tm_mday,
        tm.tm_hour, tm.tm_min)
}

fn short_model(model: &str) -> String {
    if model.contains("opus-4-6") || model.contains("opus-4-5") { "opus4".into() }
    else if model.contains("opus") { "opus3".into() }
    else if model.contains("sonnet") { "sonnet".into() }
    else if model.contains("haiku") { "haiku".into() }
    else if model.is_empty() { "?".into() }
    else { model.split('-').next().unwrap_or("?").to_string() }
}

fn format_tokens(n: u64) -> String {
    if n >= 1_000_000 { format!("{:.1}M", n as f64 / 1_000_000.0) }
    else if n >= 1_000 { format!("{}k", n / 1_000) }
    else { format!("{}", n) }
}

// ---------------------------------------------------------------------------
// Shared chart data built once per frame
// ---------------------------------------------------------------------------

/// Per-turn values for hover tooltip, one entry per api call within a session.
#[derive(Clone, Default)]
struct TurnInfo {
    x: f64,
    in_cost: f64,
    out_cost: f64,
    cost_change: f64,
    in_tok: u64,
    out_tok: u64,
    total_cost: f64,
    total_in_tok: u64,
    total_out_tok: u64,
    model_short: String,
}

struct ChartData {
    in_cost_bars: Vec<Bar>,
    out_cost_bars: Vec<Bar>,
    in_tok_bars: Vec<Bar>,
    out_tok_bars: Vec<Bar>,
    agent_xs: Vec<f64>,
    per_turn_in_cost_max: f64,
    per_turn_out_cost_max: f64,
    in_max: f64,
    out_max: f64,
    tool_list: Vec<(String, u32)>,
    /// Per-session cumulative cost lines: (color, points)
    total_cost_lines: Vec<(egui::Color32, Vec<[f64; 2]>)>,
    total_cost_max: f64,
    /// Per-session running token lines: (color, in_points, out_points)
    total_tok_lines: Vec<(egui::Color32, Vec<[f64; 2]>, Vec<[f64; 2]>)>,
    total_tok_max: f64,
    /// Combined running cost across all visible sessions, sorted by x
    combined_cost_pts: Vec<[f64; 2]>,
    combined_cost_max: f64,
    /// Per-turn cost (cost_change) at each x, sorted by x
    cost_rate_pts: Vec<[f64; 2]>,
    cost_rate_max: f64,
    /// Per-session turn data for tooltips: (display_name, session_color, turns)
    session_turns: Vec<(String, egui::Color32, Vec<TurnInfo>)>,
}


fn build_chart_data(data: &HudData, hidden: &HashSet<String>, time_axis: bool) -> ChartData {
    let mut per_turn_in_cost_max = 0.001_f64;
    let mut per_turn_out_cost_max = 0.001_f64;
    let mut in_max = 100.0_f64;
    let mut out_max = 100.0_f64;
    let mut agg_tools: HashMap<String, u32> = HashMap::new();
    let mut in_cost_bars: Vec<Bar> = vec![];
    let mut out_cost_bars: Vec<Bar> = vec![];
    let mut in_tok_bars: Vec<Bar> = vec![];
    let mut out_tok_bars: Vec<Bar> = vec![];
    let mut agent_xs: Vec<f64> = vec![];
    let mut session_turns: Vec<(String, egui::Color32, Vec<TurnInfo>)> = vec![];

    // Pre-compute time span + total api calls for adaptive bar width
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
    // Bar width: fraction of total span so bars are visible. Target ~200 bars filling the view.
    let time_bar_w = (time_span_min / total_api_calls.max(1) as f64).max(time_span_min / 300.0).min(time_span_min / 10.0);

    for (si, session) in data.sessions.iter().enumerate() {
        if hidden.contains(&session.session_id) { continue; }

        for (name, count) in &session.tool_counts {
            *agg_tools.entry(name.clone()).or_default() += count;
        }

        let mut turns: Vec<TurnInfo> = vec![];
        let mut total_in_cost = 0.0f64;
        let mut total_out_cost = 0.0f64;
        let mut prev_total_cost = 0.0f64;
        let mut total_in_tok = 0u64;
        let mut total_out_tok = 0u64;
        let mut api_idx = 0usize;
        let mut last_x = 0f64;

        for ev in &session.events {
            match ev {
                Event::ApiCall { input_cost_usd, output_cost_usd, input_tokens, output_tokens,
                                 cache_read_tokens, cache_create_tokens, timestamp_secs, model, .. } => {
                    let _total_input = input_tokens + cache_read_tokens + cache_create_tokens;
                    let x = if time_axis {
                        *timestamp_secs as f64 / 60.0
                    } else {
                        api_idx as f64
                    };
                    let bar_w = if time_axis { time_bar_w } else { 0.8 };

                    per_turn_in_cost_max = per_turn_in_cost_max.max(*input_cost_usd);
                    per_turn_out_cost_max = per_turn_out_cost_max.max(*output_cost_usd);
                    in_max = in_max.max(*input_tokens as f64);
                    out_max = out_max.max(*output_tokens as f64);

                    in_cost_bars.push(Bar::new(x, *input_cost_usd).width(bar_w).fill(Palette::INPUT_TINT));
                    out_cost_bars.push(Bar::new(x, -(*output_cost_usd)).width(bar_w).fill(Palette::OUTPUT_TINT));
                    in_tok_bars.push(Bar::new(x, *input_tokens as f64).width(bar_w).fill(Palette::INPUT_TINT));
                    out_tok_bars.push(Bar::new(x, -(*output_tokens as f64)).width(bar_w).fill(Palette::OUTPUT_TINT));

                    total_in_cost += input_cost_usd;
                    total_out_cost += output_cost_usd;
                    total_in_tok += input_tokens;
                    total_out_tok += output_tokens;

                    let cur_total = total_in_cost + total_out_cost;
                    turns.push(TurnInfo {
                        x,
                        in_cost: *input_cost_usd,
                        out_cost: *output_cost_usd,
                        cost_change: cur_total - prev_total_cost,
                        in_tok: *input_tokens,
                        out_tok: *output_tokens,
                        total_cost: cur_total,
                        total_in_tok,
                        total_out_tok,
                        model_short: short_model(model),
                    });
                    prev_total_cost = cur_total;

                    last_x = x;
                    api_idx += 1;
                }
                Event::AgentSpawn { .. } => { agent_xs.push(last_x + 0.15); }
                _ => {}
            }
        }

        session_turns.push((session.project.clone(), session_color(si), turns));
    }

    let mut tool_list: Vec<(String, u32)> = agg_tools.into_iter().collect();
    tool_list.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));

    // Per-session cumulative lines
    let mut total_cost_lines: Vec<(egui::Color32, Vec<[f64; 2]>)> = vec![];
    let mut total_cost_max = 0.001_f64;
    let mut total_tok_lines: Vec<(egui::Color32, Vec<[f64; 2]>, Vec<[f64; 2]>)> = vec![];
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

    // Combined running cost + per-turn rate, merged across all sessions, sorted by x
    let mut all_cost_events: Vec<(f64, f64)> = vec![]; // (x, cost_change)
    for (_, _, turns) in &session_turns {
        for t in turns {
            all_cost_events.push((t.x, t.cost_change));
        }
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
        in_cost_bars, out_cost_bars, in_tok_bars, out_tok_bars, agent_xs,
        per_turn_in_cost_max, per_turn_out_cost_max, in_max, out_max, tool_list,
        total_cost_lines, total_cost_max, total_tok_lines, total_tok_max,
        combined_cost_pts, combined_cost_max, cost_rate_pts, cost_rate_max,
        session_turns,
    }
}

// ---------------------------------------------------------------------------
// Shared plot factory — all interactive behaviors off, transparent bg
// ---------------------------------------------------------------------------

fn base_plot(id: &str) -> Plot<'_> {
    Plot::new(id)
        .show_axes([false, false])
        .show_grid(false)
        .allow_zoom(false)
        .allow_drag(false)
        .allow_scroll(false)
        .show_background(false)
        .set_margin_fraction(egui::Vec2::ZERO)
        .label_formatter(|_, _| String::new())
        .auto_bounds(egui::Vec2b::new(true, true))
}

fn make_tooltip<F: Fn(&TurnInfo) -> String>(
    session_turns: &[(String, egui::Color32, Vec<TurnInfo>)],
    turn: usize,
    fmt: F,
) -> Option<String> {
    let mut lines = vec![format!("turn {}", turn + 1)];
    for (name, _, turns) in session_turns {
        if let Some(t) = turns.get(turn) {
            lines.push(format!("{}: {}", name, fmt(t)));
        }
    }
    if lines.len() > 1 { Some(lines.join("\n")) } else { None }
}

fn panel_frame() -> egui::Frame {
    egui::Frame::none()
        .fill(Palette::BG_PANEL)
        .stroke(egui::Stroke::new(0.5, Palette::SEPARATOR))
        .rounding(4.0)
        .inner_margin(egui::Margin::same(6.0))
}

// ---------------------------------------------------------------------------
// Legend row drawing helper
// ---------------------------------------------------------------------------

fn draw_legend_row(
    ui: &egui::Ui,
    row_rect: egui::Rect,
    row_h: f32,
    timeline_w: f32,
    week_start_secs: u64,
    week_span: f32,
    name: &str,
    swatch_col: egui::Color32,
    name_col: egui::Color32,
    dim_col: egui::Color32,
    is_active: bool,
    _is_hidden: bool,
    total_input: u64,
    cost: f64,
    model: &str,
    // Some((active_cost, group_total_cost)) for active groups, None otherwise
    active_group_costs: Option<(f64, f64)>,
    // Sessions to draw in the mini timeline
    timeline_sessions: &[(&agent_harnesses::claude_code::SessionData, egui::Color32)],
    effective_hidden: &HashSet<String>,
    indent: Option<f32>,
) {
    let painter = ui.painter();
    let indent_px = indent.unwrap_or(0.0);
    let bar_x = row_rect.left() + 2.0 + indent_px;
    let bar_top_y = row_rect.top() + (row_h * 0.1).max(2.0);
    let bar_h_px = row_h - (row_h * 0.2).max(4.0);
    let bar_w = 8.0_f32;

    // Color swatch -- active: fill proportional to context usage; inactive: solid full bar
    if is_active {
        let faded_col = egui::Color32::from_rgba_unmultiplied(swatch_col.r(), swatch_col.g(), swatch_col.b(), 35);
        painter.rect_filled(
            egui::Rect::from_min_size(egui::pos2(bar_x, bar_top_y), egui::vec2(bar_w, bar_h_px)),
            2.0, faded_col,
        );
        let ctx_frac = (total_input as f32 / 200_000.0).clamp(0.02, 1.0);
        let swatch_h = (bar_h_px * ctx_frac).max(3.0);
        let swatch_top = bar_top_y + bar_h_px - swatch_h;
        painter.rect_filled(
            egui::Rect::from_min_size(egui::pos2(bar_x, swatch_top), egui::vec2(bar_w, swatch_h)),
            2.0, swatch_col,
        );
    } else {
        painter.rect_filled(
            egui::Rect::from_min_size(egui::pos2(bar_x, bar_top_y), egui::vec2(bar_w, bar_h_px)),
            2.0, swatch_col,
        );
    }

    // Active dot
    if is_active {
        painter.circle_filled(
            egui::pos2(bar_x + bar_w + 5.0, bar_top_y + bar_h_px * 0.25),
            2.5, egui::Color32::from_rgba_unmultiplied(80, 220, 120, 200),
        );
    } else {
        painter.circle_filled(
            egui::pos2(bar_x + bar_w + 5.0, bar_top_y + bar_h_px * 0.25),
            2.0, egui::Color32::from_rgba_unmultiplied(80, 75, 65, 100),
        );
    }

    // Mini timeline (right side)
    let tl_right = row_rect.right() - 4.0;
    let tl_left = tl_right - timeline_w;
    let tl_rect = egui::Rect::from_min_size(
        egui::pos2(tl_left, bar_top_y),
        egui::vec2(timeline_w, bar_h_px),
    );

    // Text area
    let text_x = bar_x + 16.0;
    let text_max_x = tl_left - 6.0;
    let cy = row_rect.center().y;
    let font_name = egui::FontId::monospace((row_h * 0.35).clamp(9.0, 13.0));
    let font_stat = egui::FontId::monospace((row_h * 0.27).clamp(8.0, 10.0));

    let text_clip = egui::Rect::from_min_max(
        egui::pos2(row_rect.left(), row_rect.top()),
        egui::pos2(text_max_x, row_rect.bottom()),
    );
    let text_painter = ui.painter().with_clip_rect(text_clip);

    // Name (primary line) -- calls count first, cost secondary
    text_painter.text(egui::pos2(text_x, cy - row_h * 0.12), egui::Align2::LEFT_CENTER,
        name, font_name, name_col);

    // Stats (secondary line)
    if row_h >= 22.0 {
        let model_tag = if model.is_empty() { String::new() } else { format!("  {}", short_model(model)) };
        let stat_str = if let Some((active_cost, group_cost)) = active_group_costs {
            // Active group: show active session stats first, then group total
            let ctx_pct = (total_input as f64 / 200_000.0 * 100.0).min(999.0);
            format!(
                "{:.0}% ctx  {}{}  |  {} total",
                ctx_pct,
                format_cost(active_cost),
                model_tag,
                format_cost(group_cost),
            )
        } else if is_active {
            // Flat active session
            let ctx_pct = (total_input as f64 / 200_000.0 * 100.0).min(999.0);
            format!("{:.0}% ctx  {}{}", ctx_pct, format_cost(cost), model_tag)
        } else {
            // Inactive: no context %, just cost
            format!("{}{}", format_cost(cost), model_tag)
        };
        text_painter.text(egui::pos2(text_x, cy + row_h * 0.18), egui::Align2::LEFT_CENTER,
            &stat_str, font_stat, dim_col);
    }

    // Timeline bg
    painter.rect_filled(tl_rect, 2.0,
        egui::Color32::from_rgba_unmultiplied(swatch_col.r(), swatch_col.g(), swatch_col.b(), 18));

    // Per-session lanes in timeline
    let n_sessions = timeline_sessions.len();
    let seg_h = (bar_h_px / n_sessions.max(1) as f32).max(2.0);

    for (lane, (s, col)) in timeline_sessions.iter().enumerate() {
        let seg_alpha = if effective_hidden.contains(&s.session_id) {
            30u8
        } else if s.is_active {
            220u8
        } else {
            80u8
        };
        let seg_col_a = egui::Color32::from_rgba_unmultiplied(col.r(), col.g(), col.b(), seg_alpha);

        let seg_top = tl_rect.top() + lane as f32 * seg_h;
        let seg_bot = (seg_top + seg_h).min(tl_rect.bottom());

        if s.first_ts > 0 {
            let x0f = ((s.first_ts.saturating_sub(week_start_secs)) as f32 / week_span).clamp(0.0, 1.0);
            let x1f = ((s.last_ts.saturating_sub(week_start_secs)) as f32 / week_span).clamp(0.0, 1.0);
            let px0 = tl_rect.left() + x0f * timeline_w;
            let px1 = (tl_rect.left() + x1f * timeline_w).max(px0 + 3.0).min(tl_rect.right());
            painter.rect_filled(
                egui::Rect::from_min_max(egui::pos2(px0, seg_top), egui::pos2(px1, seg_bot)),
                1.0, seg_col_a,
            );
            if s.is_active {
                painter.rect_filled(
                    egui::Rect::from_min_max(
                        egui::pos2(px1 - 2.0, seg_top),
                        egui::pos2(px1, seg_bot),
                    ),
                    0.0, egui::Color32::from_rgba_unmultiplied(80, 220, 120, 160),
                );
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Small mode (compact 3-row overlay for a single session)
// ---------------------------------------------------------------------------

fn draw_small(ui: &mut egui::Ui, data: &HudData, _cd: &ChartData, usage: &usage::UsageData, session_id: &str, hidden: &HashSet<String>, show_active_only: &mut bool, time_axis: &mut bool, autofit: &mut bool, nav_view: &mut Option<(f64, f64)>, small_mode_session: &mut Option<String>) {
    let area = ui.available_rect_before_wrap();
    let pad = 4.0;
    let gap = 3.0;

    let w = area.width() - pad * 2.0;
    let h = area.height() - pad * 2.0;
    let x0 = area.left() + pad;
    let y0 = area.top() + pad;
    let is_time = *time_axis;

    let now_secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let now_min = now_secs as f64 / 60.0;
    let week_start_secs = now_secs.saturating_sub(7 * 24 * 3600);
    let week_span = (now_secs - week_start_secs).max(1) as f32;

    // Responsive layout: sections collapse as height shrinks
    let controls_h = 14.0_f32;
    let nav_h_full = 10.0_f32;
    let legend_h_full = 32.0_f32;
    let min_chart_h = 20.0_f32;

    // Budget after controls
    let budget = h - controls_h;
    let show_nav = budget >= nav_h_full + gap;
    let nav_budget = if show_nav { nav_h_full + gap } else { 0.0 };
    let show_legend = budget - nav_budget >= legend_h_full + gap;
    let legend_budget = if show_legend { legend_h_full + gap } else { 0.0 };
    let charts_budget = budget - nav_budget - legend_budget;
    let show_charts = charts_budget >= min_chart_h;

    let controls_rect = egui::Rect::from_min_size(egui::pos2(x0, y0), egui::vec2(w, controls_h));

    let nav_y = y0 + controls_h + gap;
    let nav_rect = egui::Rect::from_min_size(egui::pos2(x0, nav_y), egui::vec2(w, nav_h_full));

    let legend_y = nav_y + if show_nav { nav_h_full + gap } else { 0.0 };
    let legend_rect = egui::Rect::from_min_size(egui::pos2(x0, legend_y), egui::vec2(w, legend_h_full));

    let charts_y = legend_y + if show_legend { legend_h_full + gap } else { 0.0 };
    let charts_h = if show_charts { charts_budget } else { 0.0 };
    let chart_w = (w - gap) / 2.0;
    let cost_rect = egui::Rect::from_min_size(egui::pos2(x0, charts_y), egui::vec2(chart_w, charts_h));
    let tok_rect = egui::Rect::from_min_size(egui::pos2(x0 + chart_w + gap, charts_y), egui::vec2(chart_w, charts_h));

    let eye_w = 20.0_f32;
    let timeline_w = 60.0_f32;

    // Find the selected session
    let selected_session = data.sessions.iter().find(|s| s.session_id == session_id);
    let Some(session) = selected_session else { return; };

    // --- Row 0: Controls + usage bars ---
    ui.allocate_new_ui(egui::UiBuilder::new().max_rect(controls_rect), |ui| {
        panel_frame().show(ui, |ui| {
            let inner = ui.available_rect_before_wrap();
            let cy = inner.center().y;
            let painter = ui.painter();
            let btn_h = controls_h - 2.0;

            // [▲ back]
            let btn_size = egui::vec2(46.0, btn_h);
            let btn_rect = egui::Rect::from_min_size(egui::pos2(inner.left() + 2.0, cy - btn_h / 2.0), btn_size);
            let btn_resp = ui.interact(btn_rect, egui::Id::new("small_back"), egui::Sense::click());
            if btn_resp.clicked() { *small_mode_session = None; }
            if btn_resp.hovered() {
                painter.rect_filled(btn_rect, 3.0, egui::Color32::from_rgba_unmultiplied(255,255,255,12));
            }
            painter.text(btn_rect.center(), egui::Align2::CENTER_CENTER,
                "▲ back", egui::FontId::monospace(8.0), Palette::TEXT_DIM);

            // "active only" toggle
            let ao_label = if *show_active_only { "● active" } else { "○ active" };
            let ao_col = if *show_active_only { Palette::TEXT_BRIGHT } else { Palette::TEXT_DIM };
            let ao_rect = egui::Rect::from_min_size(egui::pos2(btn_rect.right() + 4.0, cy - btn_h / 2.0), egui::vec2(50.0, btn_h));
            let ao_resp = ui.interact(ao_rect, egui::Id::new("small_active_only"), egui::Sense::click());
            if ao_resp.clicked() { *show_active_only = !*show_active_only; }
            if ao_resp.hovered() {
                painter.rect_filled(ao_rect, 3.0, egui::Color32::from_rgba_unmultiplied(255,255,255,12));
            }
            painter.text(ao_rect.center(), egui::Align2::CENTER_CENTER,
                ao_label, egui::FontId::monospace(8.0), ao_col);

            // "time" toggle
            let ta_label = if *time_axis { "● time" } else { "○ time" };
            let ta_col = if *time_axis { Palette::TEXT_BRIGHT } else { Palette::TEXT_DIM };
            let ta_rect = egui::Rect::from_min_size(egui::pos2(ao_rect.right() + 4.0, cy - btn_h / 2.0), egui::vec2(46.0, btn_h));
            let ta_resp = ui.interact(ta_rect, egui::Id::new("small_time_axis"), egui::Sense::click());
            if ta_resp.clicked() {
                *time_axis = !*time_axis;
                if *time_axis { *autofit = true; *nav_view = None; }
            }
            if ta_resp.hovered() {
                painter.rect_filled(ta_rect, 3.0, egui::Color32::from_rgba_unmultiplied(255,255,255,12));
            }
            painter.text(ta_rect.center(), egui::Align2::CENTER_CENTER,
                ta_label, egui::FontId::monospace(8.0), ta_col);

            // "fit" button (time mode only)
            if *time_axis {
                let fit_rect = egui::Rect::from_min_size(egui::pos2(ta_rect.right() + 4.0, cy - btn_h / 2.0), egui::vec2(30.0, btn_h));
                let fit_resp = ui.interact(fit_rect, egui::Id::new("small_fit"), egui::Sense::click());
                if fit_resp.clicked() { *autofit = true; }
                if fit_resp.hovered() {
                    painter.rect_filled(fit_rect, 3.0, egui::Color32::from_rgba_unmultiplied(255,255,255,12));
                }
                painter.text(fit_rect.center(), egui::Align2::CENTER_CENTER,
                    "fit", egui::FontId::monospace(8.0), Palette::TEXT_DIM);
            }

            // Usage bars at right side of controls
            if let Some(latest) = &usage.latest {
                let bar_w = 70.0_f32;
                let bar_h = 4.0_f32;
                let label_w = 46.0_f32; // fixed width for "5h  9%" / "7d 36%"
                let bar_x = inner.right() - bar_w - 4.0;
                let label_x = bar_x - 2.0;
                let usage_x = label_x - label_w - 2.0;

                // "usage" label
                painter.text(egui::pos2(usage_x, cy), egui::Align2::RIGHT_CENTER,
                    "usage", egui::FontId::monospace(7.0), Palette::TEXT_DIM);

                // 5h bar
                let fill_5h = (latest.five_hour as f32 / 100.0).clamp(0.0, 1.0);
                let bar_5h = egui::Rect::from_min_size(egui::pos2(bar_x, cy - bar_h - 1.0), egui::vec2(bar_w, bar_h));
                painter.rect_filled(bar_5h, 1.0, egui::Color32::from_rgba_unmultiplied(60, 55, 45, 100));
                painter.rect_filled(egui::Rect::from_min_size(bar_5h.left_top(), egui::vec2(bar_w * fill_5h, bar_h)),
                    1.0, egui::Color32::from_rgb(220, 160, 60));
                painter.text(egui::pos2(label_x, bar_5h.center().y), egui::Align2::RIGHT_CENTER,
                    &format!("5h {:>3.0}%", latest.five_hour), egui::FontId::monospace(7.0), egui::Color32::from_rgb(220, 160, 60));

                // 7d bar
                let fill_7d = (latest.seven_day as f32 / 100.0).clamp(0.0, 1.0);
                let bar_7d = egui::Rect::from_min_size(egui::pos2(bar_x, cy + 1.0), egui::vec2(bar_w, bar_h));
                painter.rect_filled(bar_7d, 1.0, egui::Color32::from_rgba_unmultiplied(60, 55, 45, 100));
                painter.rect_filled(egui::Rect::from_min_size(bar_7d.left_top(), egui::vec2(bar_w * fill_7d, bar_h)),
                    1.0, egui::Color32::from_rgb(100, 160, 220));
                painter.text(egui::pos2(label_x, bar_7d.center().y), egui::Align2::RIGHT_CENTER,
                    &format!("7d {:>3.0}%", latest.seven_day), egui::FontId::monospace(7.0), egui::Color32::from_rgb(100, 160, 220));
            }
        });
    });

    // --- Row 1: Time Navigator ---
    let mut all_x_min = f64::MAX;
    let mut all_x_max = f64::MIN;
    let mut nav_dots: Vec<(f64, egui::Color32)> = vec![];
    let sess_col = egui::Color32::from_rgb(190, 120, 20);
    for ev in &session.events {
        if let Event::ApiCall { timestamp_secs, .. } = ev {
            if *timestamp_secs > 0 {
                let x = *timestamp_secs as f64 / 60.0;
                all_x_min = all_x_min.min(x);
                all_x_max = all_x_max.max(x);
                nav_dots.push((x, sess_col));
            }
        }
    }
    if all_x_min > all_x_max { all_x_min = now_min - 60.0; all_x_max = now_min; }
    let data_span = (all_x_max - all_x_min).max(1.0);
    let full_min = all_x_min - data_span * 0.02;
    let full_max = all_x_max + data_span * 0.02;
    let full_span = full_max - full_min;

    // Autofit
    if *autofit {
        if session.first_ts > 0 {
            let fit_min = session.first_ts as f64 / 60.0;
            let fit_max = session.last_ts as f64 / 60.0;
            let span = (fit_max - fit_min).max(1.0);
            *nav_view = Some((fit_min - span * 0.02, fit_max + span * 0.02));
        }
        *autofit = false;
    }

    if show_nav {
        ui.allocate_new_ui(egui::UiBuilder::new().max_rect(nav_rect), |ui| {
            let bar = ui.available_rect_before_wrap();
            let resp = ui.interact(bar, egui::Id::new("small_nav_bar"), egui::Sense::click_and_drag());
            let painter = ui.painter();

            painter.rect_filled(bar, 1.0, egui::Color32::from_rgba_unmultiplied(18, 15, 10, 200));

            let bar_w = bar.width();
            let cy = bar.center().y;
            for (x, col) in &nav_dots {
                let frac = ((x - full_min) / full_span) as f32;
                let px = bar.left() + frac * bar_w;
                painter.circle_filled(egui::pos2(px, cy), 1.0, *col);
            }

            if is_time {
                let (vmin, vmax) = nav_view.unwrap_or((full_min, full_max));
                let v0 = ((vmin - full_min) / full_span) as f32;
                let v1 = ((vmax - full_min) / full_span) as f32;
                let vp_left  = bar.left() + v0.clamp(0.0, 1.0) * bar_w;
                let vp_right = bar.left() + v1.clamp(0.0, 1.0) * bar_w;

                if vp_left > bar.left() {
                    painter.rect_filled(
                        egui::Rect::from_min_max(bar.left_top(), egui::pos2(vp_left, bar.bottom())),
                        0.0, egui::Color32::from_rgba_unmultiplied(0, 0, 0, 120));
                }
                if vp_right < bar.right() {
                    painter.rect_filled(
                        egui::Rect::from_min_max(egui::pos2(vp_right, bar.top()), bar.right_bottom()),
                        0.0, egui::Color32::from_rgba_unmultiplied(0, 0, 0, 120));
                }
                painter.rect_stroke(
                    egui::Rect::from_min_max(egui::pos2(vp_left, bar.top()), egui::pos2(vp_right, bar.bottom())),
                    1.0, egui::Stroke::new(0.5, Palette::TEXT_DIM));

                // Drag to pan
                if resp.dragged() {
                    let dx_px = resp.drag_delta().x;
                    let dx_min = (dx_px / bar_w) as f64 * full_span;
                    let (mut vmin, mut vmax) = nav_view.unwrap_or((full_min, full_max));
                    let vspan = vmax - vmin;
                    vmin += dx_min; vmax += dx_min;
                    if vmin < full_min { vmin = full_min; vmax = vmin + vspan; }
                    if vmax > full_max { vmax = full_max; vmin = vmax - vspan; }
                    *nav_view = Some((vmin, vmax));
                }

                // Scroll to zoom
                let scroll = ui.input(|i| i.smooth_scroll_delta.y + i.smooth_scroll_delta.x);
                if resp.hovered() && scroll.abs() > 0.1 {
                    let zoom_factor = 1.0 - (scroll as f64 * 0.003);
                    let (vmin, vmax) = nav_view.unwrap_or((full_min, full_max));
                    let vspan = vmax - vmin;
                    let mouse_frac = ui.input(|i| i.pointer.hover_pos())
                        .map(|p| ((p.x - bar.left()) / bar_w).clamp(0.0, 1.0) as f64)
                        .unwrap_or(0.5);
                    let anchor = full_min + mouse_frac * full_span;
                    let t = ((anchor - vmin) / vspan).clamp(0.0, 1.0);
                    let new_span = (vspan * zoom_factor).clamp(1.0, full_span);
                    let new_min = (anchor - t * new_span).max(full_min);
                    let new_max = (new_min + new_span).min(full_max);
                    let new_min = (new_max - new_span).max(full_min);
                    *nav_view = Some((new_min, new_max));
                }
            }
        });
    }

    // --- Row 2: Legend ---
    if show_legend {
        ui.allocate_new_ui(egui::UiBuilder::new().max_rect(legend_rect), |ui| {
            panel_frame().show(ui, |ui| {
                let name_col = Palette::TEXT_BRIGHT;
                let dim_col = Palette::TEXT_DIM;

                let inner = ui.available_rect_before_wrap();
                draw_legend_row(ui, inner, legend_h_full, timeline_w, week_start_secs, week_span,
                    &session.project, sess_col, name_col, dim_col,
                    session.is_active, hidden.contains(&session.session_id),
                    session.last_input_tokens, session.total_cost_usd, &session.model,
                    None,
                    &[(session, sess_col)], hidden, Some(eye_w));
            });
        });
    }

    // --- Row 3: Two charts (only if space) ---
    if !show_charts { return; }
    struct TurnData { x: f64, cost: f64, total_cost: f64, toks: u64, total_toks: u64, idx: usize }
    let mut turns: Vec<TurnData> = vec![];
    let mut cum_cost = 0.0f64;
    let mut cum_toks = 0u64;
    let mut api_idx = 0usize;
    for ev in &session.events {
        if let Event::ApiCall { input_cost_usd, output_cost_usd, input_tokens, output_tokens, timestamp_secs, .. } = ev {
            let x = if is_time { *timestamp_secs as f64 / 60.0 } else { api_idx as f64 };
            let cost = input_cost_usd + output_cost_usd;
            let toks = input_tokens + output_tokens;
            cum_cost += cost;
            cum_toks += toks;
            turns.push(TurnData { x, cost, total_cost: cum_cost, toks, total_toks: cum_toks, idx: api_idx });
            api_idx += 1;
        }
    }

    let cursor_id = egui::Id::new("small_charts_cursor");
    let bar_w = if is_time { 0.5 } else { 0.8 };
    let line_style = if is_time { egui_plot::LineStyle::Dashed { length: 8.0 } } else { egui_plot::LineStyle::Solid };
    let hovered_data_x = std::cell::Cell::new(None::<f64>);

    // Cost chart
    ui.allocate_new_ui(egui::UiBuilder::new().max_rect(cost_rect), |ui| {
        panel_frame().show(ui, |ui| {
            let cost_bars: Vec<Bar> = turns.iter()
                .map(|t| Bar::new(t.x, t.cost).width(bar_w).fill(Palette::INPUT_TINT))
                .collect();
            let cost_line: Vec<[f64; 2]> = turns.iter().map(|t| [t.x, t.total_cost]).collect();

            let mut p = base_plot("small_cost")
                .link_cursor(cursor_id, true, false)
                .include_y(0.0)
                .include_y(cum_cost * 1.05)
                .show_axes([false, false])
                .show_grid(false);
            if let Some((vmin, vmax)) = if is_time { *nav_view } else { None } {
                p = p.include_x(vmin).include_x(vmax).auto_bounds(egui::Vec2b::new(false, true));
            }
            let plot_resp = p.show(ui, |pui| {
                pui.bar_chart(BarChart::new(cost_bars).color(Palette::INPUT_TINT));
                if !cost_line.is_empty() {
                    pui.line(egui_plot::Line::new(cost_line).color(Palette::OUTPUT_TINT).width(1.5).style(line_style));
                }
            });

            ui.painter().text(
                egui::pos2(cost_rect.right() - 4.0, cost_rect.top() + 2.0),
                egui::Align2::RIGHT_TOP, &format_cost(cum_cost),
                egui::FontId::monospace(10.0), Palette::TEXT_DIM);

            // Capture hover x from this chart
            if let Some(hover_pos) = ui.ctx().input(|i| i.pointer.hover_pos()) {
                if cost_rect.contains(hover_pos) {
                    if let Some(px) = plot_resp.response.hover_pos()
                        .map(|p| plot_resp.transform.value_from_position(p).x) {
                        hovered_data_x.set(Some(px));
                    }
                }
            }
        });
    });

    // Token chart
    ui.allocate_new_ui(egui::UiBuilder::new().max_rect(tok_rect), |ui| {
        panel_frame().show(ui, |ui| {
            let tok_bars: Vec<Bar> = turns.iter()
                .map(|t| Bar::new(t.x, t.toks as f64).width(bar_w).fill(Palette::OUTPUT_TINT))
                .collect();
            let tok_line: Vec<[f64; 2]> = turns.iter().map(|t| [t.x, t.total_toks as f64]).collect();

            let mut p = base_plot("small_tok")
                .link_cursor(cursor_id, true, false)
                .include_y(0.0)
                .include_y(cum_toks as f64 * 1.05)
                .show_axes([false, false])
                .show_grid(false);
            if let Some((vmin, vmax)) = if is_time { *nav_view } else { None } {
                p = p.include_x(vmin).include_x(vmax).auto_bounds(egui::Vec2b::new(false, true));
            }
            let plot_resp = p.show(ui, |pui| {
                pui.bar_chart(BarChart::new(tok_bars).color(Palette::OUTPUT_TINT));
                if !tok_line.is_empty() {
                    pui.line(egui_plot::Line::new(tok_line).color(Palette::INPUT_TINT).width(1.5).style(line_style));
                }
            });

            ui.painter().text(
                egui::pos2(tok_rect.right() - 4.0, tok_rect.top() + 2.0),
                egui::Align2::RIGHT_TOP, &format_tokens(cum_toks),
                egui::FontId::monospace(10.0), Palette::TEXT_DIM);

            // Capture hover x from this chart
            if let Some(hover_pos) = ui.ctx().input(|i| i.pointer.hover_pos()) {
                if tok_rect.contains(hover_pos) {
                    if let Some(px) = plot_resp.response.hover_pos()
                        .map(|p| plot_resp.transform.value_from_position(p).x) {
                        hovered_data_x.set(Some(px));
                    }
                }
            }
        });
    });

    // Both tooltips + navigator cursor (triggered by hovering either chart)
    if let Some(data_x) = hovered_data_x.get() {
        if let Some(t) = turns.iter().min_by(|a, b|
            (a.x - data_x).abs().partial_cmp(&(b.x - data_x).abs()).unwrap())
        {
            let font = egui::FontId::monospace(9.0);
            let tip_bg = egui::Color32::from_rgb(20, 18, 14);

            // Cost tooltip
            let cost_tip = format!("t{}  {}  total {}", t.idx + 1, format_cost(t.cost), format_cost(t.total_cost));
            let cost_galley = ui.painter().layout_no_wrap(cost_tip.clone(), font.clone(), Palette::TEXT_BRIGHT);
            let cost_tip_rect = egui::Rect::from_min_size(
                egui::pos2(cost_rect.left() + 2.0, cost_rect.top() + 2.0),
                cost_galley.size() + egui::vec2(8.0, 4.0));
            ui.painter().rect_filled(cost_tip_rect, 2.0, tip_bg);
            ui.painter().text(cost_tip_rect.min + egui::vec2(4.0, 2.0), egui::Align2::LEFT_TOP,
                &cost_tip, font.clone(), Palette::TEXT_BRIGHT);

            // Token tooltip
            let tok_tip = format!("t{}  {}  total {}", t.idx + 1, format_tokens(t.toks), format_tokens(t.total_toks));
            let tok_galley = ui.painter().layout_no_wrap(tok_tip.clone(), font.clone(), Palette::TEXT_BRIGHT);
            let tok_tip_rect = egui::Rect::from_min_size(
                egui::pos2(tok_rect.left() + 2.0, tok_rect.top() + 2.0),
                tok_galley.size() + egui::vec2(8.0, 4.0));
            ui.painter().rect_filled(tok_tip_rect, 2.0, tip_bg);
            ui.painter().text(tok_tip_rect.min + egui::vec2(4.0, 2.0), egui::Align2::LEFT_TOP,
                &tok_tip, font, Palette::TEXT_BRIGHT);
        }

        // Cursor line on navigator
        let frac = ((data_x - full_min) / full_span) as f32;
        let px = nav_rect.left() + frac * nav_rect.width();
        if px >= nav_rect.left() && px <= nav_rect.right() {
            ui.painter().line_segment(
                [egui::pos2(px, nav_rect.top()), egui::pos2(px, nav_rect.bottom())],
                egui::Stroke::new(1.0, Palette::TEXT_BRIGHT));

            // Timestamp label under cursor line
            if is_time {
                let epoch_secs = (data_x * 60.0) as u64;
                let label = format_epoch_local(epoch_secs);
                let font = egui::FontId::monospace(7.0);
                let galley = ui.painter().layout_no_wrap(label.clone(), font.clone(), Palette::TEXT_DIM);
                let label_w = galley.size().x;
                let label_x = px.clamp(nav_rect.left() + label_w * 0.5 + 1.0, nav_rect.right() - label_w * 0.5 - 1.0);
                ui.painter().text(egui::pos2(label_x, nav_rect.bottom() + 1.0),
                    egui::Align2::CENTER_TOP, &label, font, Palette::TEXT_DIM);
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Big dashboard layout
// ---------------------------------------------------------------------------

fn draw_big(ui: &mut egui::Ui, data: &HudData, cd: &ChartData, usage: &usage::UsageData, hidden: &mut HashSet<String>, effective_hidden: &HashSet<String>, show_active_only: &mut bool, time_axis: &mut bool, autofit: &mut bool, nav_view: &mut Option<(f64, f64)>, expanded_groups: &mut HashSet<String>, small_mode_session: &mut Option<String>) {
    let area = ui.available_rect_before_wrap();
    let pad = 8.0;
    let gap = 8.0;

    let w = area.width() - pad * 2.0;
    let h = area.height() - pad * 2.0;
    let x0 = area.left() + pad;
    let y0 = area.top() + pad;

    // Row 0: controls strip
    let controls_h = 22.0_f32;
    let controls_rect = egui::Rect::from_min_size(egui::pos2(x0, y0), egui::vec2(w, controls_h));

    // Row 0.5: time navigator strip (always visible, compact)
    let nav_h = 16.0_f32;
    let nav_rect = egui::Rect::from_min_size(
        egui::pos2(x0, y0 + controls_h + gap),
        egui::vec2(w, nav_h),
    );
    let after_nav_y = y0 + controls_h + gap + nav_h + gap;

    // Row 1: legend strip -- group sessions by cwd, one row per group
    let mut groups: Vec<(String, Vec<(usize, usize)>)> = vec![];
    for (si, session) in data.sessions.iter().enumerate() {
        if let Some(g) = groups.iter_mut().find(|(cwd, _)| *cwd == session.cwd) {
            let local = g.1.len();
            g.1.push((si, local));
        } else {
            groups.push((session.cwd.clone(), vec![(si, 0)]));
        }
    }
    let legend_row_h = 42.0_f32;
    let row_gap = 3.0_f32;
    let legend_h = (h * 0.30).clamp(100.0, 220.0);
    // Row 3: weekly strip (thin)
    let weekly_h = (h * 0.17).max(50.0);
    // Row 2: per-turn + running total charts
    let chart_h = h - controls_h - gap - nav_h - gap - legend_h - weekly_h - gap * 2.0;

    let legend_rect = egui::Rect::from_min_size(egui::pos2(x0, after_nav_y), egui::vec2(w, legend_h));

    let cost_w  = w * 0.50;
    let tok_w   = w * 0.26;
    let tool_w  = w - cost_w - tok_w - gap * 2.0;
    let chart_y = after_nav_y + legend_h + gap;

    // Cost column: per-turn (top 60%) + running total (bottom 40%), gap between
    let per_turn_h = (chart_h * 0.60).floor();
    let stacked_h = chart_h - per_turn_h - gap;

    let cost_rect     = egui::Rect::from_min_size(egui::pos2(x0, chart_y), egui::vec2(cost_w, per_turn_h));
    let total_cost_rect = egui::Rect::from_min_size(egui::pos2(x0, chart_y + per_turn_h + gap), egui::vec2(cost_w, stacked_h));
    let tok_rect     = egui::Rect::from_min_size(egui::pos2(x0 + cost_w + gap, chart_y), egui::vec2(tok_w, per_turn_h));
    let total_tok_rect = egui::Rect::from_min_size(egui::pos2(x0 + cost_w + gap, chart_y + per_turn_h + gap), egui::vec2(tok_w, stacked_h));
    let usage_chart_h = (chart_h * 0.45).floor();
    let tool_h = chart_h - usage_chart_h - gap;
    let right_x = x0 + cost_w + tok_w + gap * 2.0;
    let usage_rect   = egui::Rect::from_min_size(egui::pos2(right_x, chart_y), egui::vec2(tool_w, usage_chart_h));
    let tool_rect    = egui::Rect::from_min_size(egui::pos2(right_x, chart_y + usage_chart_h + gap), egui::vec2(tool_w, tool_h));
    let weekly_y     = chart_y + chart_h + gap;
    let weekly_half  = (w - gap) / 2.0;
    let weekly_total_rect  = egui::Rect::from_min_size(egui::pos2(x0, weekly_y), egui::vec2(weekly_half, weekly_h));
    let weekly_rate_rect = egui::Rect::from_min_size(egui::pos2(x0 + weekly_half + gap, weekly_y), egui::vec2(weekly_half, weekly_h));

    // Context window size (tokens). Could be made configurable.
    const CTX_WINDOW: u64 = 200_000;

    let now_secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let week_start_secs = now_secs.saturating_sub(7 * 24 * 3600);
    let week_span = (now_secs - week_start_secs).max(1) as f32;
    let now_min = now_secs as f64 / 60.0;
    let is_time = *time_axis;

    // Formatter for x-axis in time mode: minutes-from-epoch -> relative label
    let time_x_fmt = move |v: egui_plot::GridMark, _range: &std::ops::RangeInclusive<f64>| -> String {
        let ago_min = now_min - v.value;
        if ago_min < 0.5 { return "now".into(); }
        if ago_min < 60.0 { return format!("{}m", ago_min.round() as i64); }
        let ago_h = ago_min / 60.0;
        if ago_h < 24.0 { return format!("{:.0}h", ago_h); }
        let ago_d = ago_h / 24.0;
        format!("{:.0}d", ago_d)
    };

    // --- controls strip ---
    ui.allocate_new_ui(egui::UiBuilder::new().max_rect(controls_rect), |ui| {
        panel_frame().show(ui, |ui| {
            let inner = ui.available_rect_before_wrap();
            let cy = inner.center().y;
            let painter = ui.painter();

            // "active only" toggle button
            let btn_label = if *show_active_only { "● active" } else { "○ active" };
            let btn_col = if *show_active_only { Palette::TEXT_BRIGHT } else { Palette::TEXT_DIM };
            let btn_size = egui::vec2(70.0, controls_h - 6.0);
            let btn_rect = egui::Rect::from_min_size(egui::pos2(inner.left() + 2.0, cy - btn_size.y / 2.0), btn_size);
            let btn_resp = ui.interact(btn_rect, egui::Id::new("ctrl_active_only"), egui::Sense::click());
            if btn_resp.clicked() { *show_active_only = !*show_active_only; }
            if btn_resp.hovered() {
                painter.rect_filled(btn_rect, 3.0, egui::Color32::from_rgba_unmultiplied(255,255,255,12));
            }
            painter.text(btn_rect.center(), egui::Align2::CENTER_CENTER,
                btn_label, egui::FontId::monospace(10.0), btn_col);

            // "time axis" toggle button
            let ta_label = if *time_axis { "● time" } else { "○ time" };
            let ta_col = if *time_axis { Palette::TEXT_BRIGHT } else { Palette::TEXT_DIM };
            let ta_rect = egui::Rect::from_min_size(egui::pos2(btn_rect.right() + 8.0, cy - btn_size.y / 2.0), egui::vec2(60.0, btn_size.y));
            let ta_resp = ui.interact(ta_rect, egui::Id::new("ctrl_time_axis"), egui::Sense::click());
            if ta_resp.clicked() {
                *time_axis = !*time_axis;
                if *time_axis {
                    *autofit = true;
                    *nav_view = None;
                }
            }
            if ta_resp.hovered() {
                painter.rect_filled(ta_rect, 3.0, egui::Color32::from_rgba_unmultiplied(255,255,255,12));
            }
            painter.text(ta_rect.center(), egui::Align2::CENTER_CENTER,
                ta_label, egui::FontId::monospace(10.0), ta_col);

            // "fit" button -- resets navigator zoom in time mode
            if *time_axis {
                let af_rect = egui::Rect::from_min_size(egui::pos2(ta_rect.right() + 8.0, cy - btn_size.y / 2.0), egui::vec2(50.0, btn_size.y));
                let af_resp = ui.interact(af_rect, egui::Id::new("ctrl_autofit"), egui::Sense::click());
                if af_resp.clicked() { *autofit = true; }
                if af_resp.hovered() {
                    painter.rect_filled(af_rect, 3.0, egui::Color32::from_rgba_unmultiplied(255,255,255,12));
                }
                painter.text(af_rect.center(), egui::Align2::CENTER_CENTER,
                    "fit", egui::FontId::monospace(10.0), Palette::TEXT_DIM);
            }
        });
    });

    // --- time navigator strip (always visible; pan/zoom only in time mode) ---
    // --- time navigator: custom-painted bar with viewport window ---
    // Compute full data range (all visible sessions)
    let mut all_x_min = f64::MAX;
    let mut all_x_max = f64::MIN;
    let mut all_dots: Vec<(f64, egui::Color32)> = vec![];
    for (si, session) in data.sessions.iter().enumerate() {
        if effective_hidden.contains(&session.session_id) { continue; }
        let col = session_color(si);
        let alpha = if session.is_active { 200u8 } else { 50u8 };
        let dot_col = egui::Color32::from_rgba_unmultiplied(col.r(), col.g(), col.b(), alpha);
        for ev in &session.events {
            if let Event::ApiCall { timestamp_secs, .. } = ev {
                if *timestamp_secs > 0 {
                    let x = *timestamp_secs as f64 / 60.0;
                    all_x_min = all_x_min.min(x);
                    all_x_max = all_x_max.max(x);
                    all_dots.push((x, dot_col));
                }
            }
        }
    }
    if all_x_min > all_x_max { all_x_min = now_min - 60.0; all_x_max = now_min; }
    let data_span = (all_x_max - all_x_min).max(1.0);
    // Pad 2% on each side
    let full_min = all_x_min - data_span * 0.02;
    let full_max = all_x_max + data_span * 0.02;
    let full_span = full_max - full_min;

    // Autofit: snap viewport to effectively visible sessions' time range
    if *autofit {
        let mut fit_min = f64::MAX;
        let mut fit_max = f64::MIN;
        for session in &data.sessions {
            if effective_hidden.contains(&session.session_id) { continue; }
            if session.first_ts > 0 {
                fit_min = fit_min.min(session.first_ts as f64 / 60.0);
                fit_max = fit_max.max(session.last_ts as f64 / 60.0);
            }
        }
        if fit_min < fit_max {
            let span = (fit_max - fit_min).max(1.0);
            *nav_view = Some((fit_min - span * 0.02, fit_max + span * 0.02));
        }
        *autofit = false;
    }

    ui.allocate_new_ui(egui::UiBuilder::new().max_rect(nav_rect), |ui| {
        let bar = ui.available_rect_before_wrap();
        let resp = ui.interact(bar, egui::Id::new("nav_bar"), egui::Sense::click_and_drag());
        let painter = ui.painter();

        // Bottom separator
        painter.line_segment(
            [egui::pos2(bar.left(), bar.bottom()), egui::pos2(bar.right(), bar.bottom())],
            egui::Stroke::new(0.5, Palette::SEPARATOR),
        );

        // Draw dots on the full range
        let bar_w = bar.width();
        let cy = bar.center().y;
        for (x, col) in &all_dots {
            let frac = ((x - full_min) / full_span) as f32;
            let px = bar.left() + frac * bar_w;
            painter.circle_filled(egui::pos2(px, cy), 1.5, *col);
        }

        // Viewport highlight (only in time mode with a nav_view set)
        if is_time {
            let (vmin, vmax) = nav_view.unwrap_or((full_min, full_max));
            let v0 = ((vmin - full_min) / full_span) as f32;
            let v1 = ((vmax - full_min) / full_span) as f32;
            let vp_left  = bar.left() + v0.clamp(0.0, 1.0) * bar_w;
            let vp_right = bar.left() + v1.clamp(0.0, 1.0) * bar_w;

            // Dim areas outside viewport
            if vp_left > bar.left() {
                painter.rect_filled(
                    egui::Rect::from_min_max(bar.left_top(), egui::pos2(vp_left, bar.bottom())),
                    0.0, egui::Color32::from_rgba_unmultiplied(0, 0, 0, 120),
                );
            }
            if vp_right < bar.right() {
                painter.rect_filled(
                    egui::Rect::from_min_max(egui::pos2(vp_right, bar.top()), bar.right_bottom()),
                    0.0, egui::Color32::from_rgba_unmultiplied(0, 0, 0, 120),
                );
            }
            // Viewport border
            painter.rect_stroke(
                egui::Rect::from_min_max(egui::pos2(vp_left, bar.top()), egui::pos2(vp_right, bar.bottom())),
                1.0, egui::Stroke::new(1.0, Palette::TEXT_DIM),
            );

            // Handle drag: pan the viewport
            if resp.dragged() {
                let dx_px = resp.drag_delta().x;
                let dx_min = (dx_px / bar_w) as f64 * full_span;
                let (mut vmin, mut vmax) = nav_view.unwrap_or((full_min, full_max));
                let vspan = vmax - vmin;
                vmin += dx_min;
                vmax += dx_min;
                // Clamp to full range
                if vmin < full_min { vmin = full_min; vmax = vmin + vspan; }
                if vmax > full_max { vmax = full_max; vmin = vmax - vspan; }
                *nav_view = Some((vmin, vmax));
            }

            // Handle scroll: zoom anchored to mouse position
            let scroll = ui.input(|i| i.smooth_scroll_delta.y + i.smooth_scroll_delta.x);
            if resp.hovered() && scroll.abs() > 0.1 {
                let zoom_factor = 1.0 - (scroll as f64 * 0.003);
                let (vmin, vmax) = nav_view.unwrap_or((full_min, full_max));
                let vspan = vmax - vmin;

                // Anchor point: mouse x position mapped to time domain
                let mouse_frac = ui.input(|i| i.pointer.hover_pos())
                    .map(|p| ((p.x - bar.left()) / bar_w).clamp(0.0, 1.0) as f64)
                    .unwrap_or(0.5);
                let anchor = full_min + mouse_frac * full_span;
                // How far anchor is into current viewport (0..1)
                let t = ((anchor - vmin) / vspan).clamp(0.0, 1.0);

                let new_span = (vspan * zoom_factor).clamp(1.0, full_span);
                let new_min = (anchor - t * new_span).max(full_min);
                let new_max = (new_min + new_span).min(full_max);
                let new_min = (new_max - new_span).max(full_min);
                *nav_view = Some((new_min, new_max));
            }
        }

        // Time labels along bottom
        let label_font = egui::FontId::monospace(8.0);
        let n_labels = (bar_w / 80.0).floor().max(2.0) as usize;
        for i in 0..=n_labels {
            let frac = i as f64 / n_labels as f64;
            let t = full_min + frac * full_span;
            let ago_min = now_min - t;
            let label = if ago_min.abs() < 1.0 { "now".into() }
                else if ago_min < 60.0 { format!("{}m", ago_min.round() as i64) }
                else if ago_min < 24.0 * 60.0 { format!("{:.0}h", ago_min / 60.0) }
                else { format!("{:.0}d", ago_min / (24.0 * 60.0)) };
            let px = bar.left() + frac as f32 * bar_w;
            painter.text(egui::pos2(px, bar.bottom() - 1.0), egui::Align2::CENTER_BOTTOM,
                &label, label_font.clone(), Palette::TEXT_DIM);
        }
    });

    // --- legend (grouped by cwd) ---
    // Sort: groups with active sessions first, then by cost descending.
    groups.sort_by(|a, b| {
        let a_active = a.1.iter().any(|(si, _)| data.sessions[*si].is_active);
        let b_active = b.1.iter().any(|(si, _)| data.sessions[*si].is_active);
        b_active.cmp(&a_active)
            .then_with(|| {
                let a_cost: f64 = a.1.iter().map(|(si, _)| data.sessions[*si].total_cost_usd).sum();
                let b_cost: f64 = b.1.iter().map(|(si, _)| data.sessions[*si].total_cost_usd).sum();
                b_cost.partial_cmp(&a_cost).unwrap_or(std::cmp::Ordering::Equal)
            })
    });
    // Within each group, put active sessions first
    for (_, members) in &mut groups {
        members.sort_by(|a, b| {
            let a_active = data.sessions[a.0].is_active;
            let b_active = data.sessions[b.0].is_active;
            b_active.cmp(&a_active)
        });
    }

    // Groups with <=2 sessions: flat rows, click toggles visibility.
    // Groups with 3+: collapsible header, click expands/collapses sub-session list.
    // Sub-session rows: click toggles individual session visibility.
    let row_h = legend_row_h;
    let timeline_w = 120.0_f32;

    let eye_w = 20.0_f32;

    // Collect toggle actions to apply after rendering
    let mut toggle_ids: Vec<String> = vec![];
    let mut group_toggle: Option<(String, Vec<String>)> = None; // (cwd, all member ids)
    let mut toggle_expand: Option<String> = None;
    let mut enter_small_mode: Option<String> = None;
    let legend_hl_id = egui::Id::new("legend_highlight");
    // Clear highlight each frame; legend hover will re-set it
    ui.ctx().data_mut(|d| d.insert_temp(legend_hl_id, LegendHighlight::default()));

    ui.allocate_new_ui(egui::UiBuilder::new().max_rect(legend_rect), |ui| {
        panel_frame().show(ui, |ui| {
            // Boost scroll speed 4x: read raw delta, apply extra offset after ScrollArea
            let extra_scroll = ui.input(|i| i.smooth_scroll_delta.y) * 3.0; // 3x extra = 4x total
            let legend_scroll_id = egui::Id::new("legend_scroll");
            let scroll_out = egui::ScrollArea::vertical()
                .auto_shrink(false)
                .id_salt(legend_scroll_id)
                .show(ui, |ui| {
            let inner = ui.available_rect_before_wrap();
            let mut row_idx = 0usize;

            for (gi, (cwd, members)) in groups.iter().enumerate() {
                let group_col = session_color(members[0].0);
                let is_flat = members.len() <= 2;
                let is_expanded = expanded_groups.contains(cwd);

                // Aggregate stats for group header
                let mut g_cost = 0.0f64;
                let mut g_last_input = 0u64;
                let mut g_active_cost = 0.0f64;
                let mut any_active = false;
                let mut active_count = 0u32;
                let mut all_hidden = true;
                let mut g_model = String::new();

                for (si, _) in members {
                    let s = &data.sessions[*si];
                    g_cost    += s.total_cost_usd;
                    if s.is_active {
                        any_active = true;
                        active_count += 1;
                        g_model = s.model.clone();
                        g_last_input = s.last_input_tokens;
                        g_active_cost += s.total_cost_usd;
                    }
                    if !effective_hidden.contains(&s.session_id) { all_hidden = false; }
                }
                // If no active session, use the first member (already sorted most-recent-first)
                if !any_active {
                    if let Some((si, _)) = members.first() {
                        let s = &data.sessions[*si];
                        g_last_input = s.last_input_tokens;
                        g_model = s.model.clone();
                    }
                }

                if is_flat {
                    // Flat: render each session as its own row
                    for (si, _) in members {
                        let s = &data.sessions[*si];
                        let sess_col = session_color(*si);
                        let is_hidden = effective_hidden.contains(&s.session_id);
                        let manually_hidden = hidden.contains(&s.session_id);
                        let force_masked = *show_active_only && !s.is_active;
                        let text_alpha = if is_hidden { 80u8 } else { 230u8 };
                        let name_col = egui::Color32::from_rgba_unmultiplied(240, 230, 200, text_alpha);
                        let dim_col = egui::Color32::from_rgba_unmultiplied(130, 120, 100, text_alpha / 2);

                        let row_top = inner.top() + row_idx as f32 * (row_h + row_gap);
                        let row_rect = egui::Rect::from_min_size(
                            egui::pos2(inner.left(), row_top),
                            egui::vec2(inner.width(), row_h),
                        );
                        let resp = ui.interact(row_rect, egui::Id::new(("legend_flat", gi, *si)), egui::Sense::click());
                        if resp.clicked() && !force_masked { toggle_ids.push(s.session_id.clone()); }
                        ui.painter().rect_filled(row_rect, 2.0,
                            egui::Color32::from_rgba_unmultiplied(255, 255, 255, 4));
                        if resp.hovered() {
                            ui.painter().rect_filled(row_rect, 2.0,
                                egui::Color32::from_rgba_unmultiplied(255, 255, 255, 12));
                            if s.first_ts > 0 {
                                ui.ctx().data_mut(|d| d.get_temp_mut_or_default::<LegendHighlight>(legend_hl_id)
                                    .ranges.push((s.first_ts as f64 / 60.0, s.last_ts as f64 / 60.0)));
                            }
                        }

                        // Eye icon: filled circle = visible, outline = hidden, gray = force-masked
                        let eye_cx = row_rect.left() + eye_w * 0.5 + 2.0;
                        let eye_cy = row_rect.center().y;
                        let eye_r = 4.0;
                        if force_masked {
                            ui.painter().circle_stroke(egui::pos2(eye_cx, eye_cy), eye_r,
                                egui::Stroke::new(1.0, egui::Color32::from_rgba_unmultiplied(80, 75, 65, 60)));
                        } else if manually_hidden {
                            ui.painter().circle_stroke(egui::pos2(eye_cx, eye_cy), eye_r,
                                egui::Stroke::new(1.0, egui::Color32::from_rgba_unmultiplied(180, 170, 150, 120)));
                        } else {
                            ui.painter().circle_filled(egui::pos2(eye_cx, eye_cy), eye_r,
                                egui::Color32::from_rgba_unmultiplied(200, 190, 170, 180));
                        }

                        // [→] button to enter small mode (left of timeline)
                        let btn_w = 36.0;
                        let btn_h = row_h - 4.0;
                        let btn_rect = egui::Rect::from_min_size(
                            egui::pos2(row_rect.right() - timeline_w - btn_w - 6.0, row_rect.top() + 2.0),
                            egui::vec2(btn_w, btn_h),
                        );
                        let btn_resp = ui.interact(btn_rect, egui::Id::new(("legend_small_btn", gi, *si)), egui::Sense::click());
                        if btn_resp.clicked() { enter_small_mode = Some(s.session_id.clone()); }
                        if btn_resp.hovered() {
                            ui.painter().rect_filled(btn_rect, 2.0, egui::Color32::from_rgba_unmultiplied(255, 255, 255, 20));
                        }
                        ui.painter().text(egui::pos2(btn_rect.center().x, btn_rect.center().y - 4.0),
                            egui::Align2::CENTER_CENTER, "→",
                            egui::FontId::monospace(18.0), Palette::TEXT_DIM);
                        ui.painter().text(egui::pos2(btn_rect.center().x, btn_rect.center().y + 10.0),
                            egui::Align2::CENTER_CENTER, "detail",
                            egui::FontId::monospace(7.0), Palette::TEXT_DIM);

                        draw_legend_row(ui, row_rect, row_h, timeline_w, week_start_secs, week_span,
                            &s.project, sess_col, name_col, dim_col,
                            s.is_active, is_hidden,
                            s.last_input_tokens, s.total_cost_usd, &s.model,
                            None,
                            &[(s, sess_col)], effective_hidden, Some(eye_w));

                        row_idx += 1;
                    }
                } else {
                    // Group header row
                    let text_alpha = if all_hidden { 80u8 } else { 230u8 };
                    let bar_col = egui::Color32::from_rgba_unmultiplied(group_col.r(), group_col.g(), group_col.b(), text_alpha);
                    let name_col = egui::Color32::from_rgba_unmultiplied(240, 230, 200, text_alpha);
                    let dim_col = egui::Color32::from_rgba_unmultiplied(130, 120, 100, text_alpha / 2);

                    let row_top = inner.top() + row_idx as f32 * (row_h + row_gap);
                    let row_rect = egui::Rect::from_min_size(
                        egui::pos2(inner.left(), row_top),
                        egui::vec2(inner.width(), row_h),
                    );

                    // Eye zone (left): toggles all members' visibility
                    let eye_rect = egui::Rect::from_min_size(
                        row_rect.left_top(),
                        egui::vec2(eye_w + 4.0, row_h),
                    );
                    let eye_resp = ui.interact(eye_rect, egui::Id::new(("legend_group_eye", gi)), egui::Sense::click());
                    if eye_resp.clicked() {
                        let member_ids: Vec<String> = members.iter()
                            .map(|(si, _)| data.sessions[*si].session_id.clone())
                            .collect();
                        group_toggle = Some((cwd.clone(), member_ids));
                    }

                    // Main zone (right of eye): expand/collapse
                    let main_rect = egui::Rect::from_min_max(
                        egui::pos2(row_rect.left() + eye_w + 4.0, row_rect.top()),
                        row_rect.right_bottom(),
                    );
                    let main_resp = ui.interact(main_rect, egui::Id::new(("legend_group", gi)), egui::Sense::click());
                    if main_resp.clicked() { toggle_expand = Some(cwd.clone()); }

                    // Row bg
                    ui.painter().rect_filled(row_rect, 2.0,
                        egui::Color32::from_rgba_unmultiplied(255, 255, 255, 4));
                    if main_resp.hovered() || eye_resp.hovered() {
                        ui.painter().rect_filled(row_rect, 2.0,
                            egui::Color32::from_rgba_unmultiplied(255, 255, 255, 12));
                        ui.ctx().data_mut(|d| {
                            let hl = d.get_temp_mut_or_default::<LegendHighlight>(legend_hl_id);
                            for (si, _) in members {
                                let s = &data.sessions[*si];
                                if s.first_ts > 0 {
                                    hl.ranges.push((s.first_ts as f64 / 60.0, s.last_ts as f64 / 60.0));
                                }
                            }
                        });
                    }

                    // Eye icon for group: filled = some visible, outline = all hidden, half = mixed
                    let eye_cx = row_rect.left() + eye_w * 0.5 + 2.0;
                    let eye_cy = row_rect.center().y;
                    let eye_r = 4.5;
                    let none_hidden = members.iter().all(|(si, _)| !hidden.contains(&data.sessions[*si].session_id));
                    if all_hidden {
                        ui.painter().circle_stroke(egui::pos2(eye_cx, eye_cy), eye_r,
                            egui::Stroke::new(1.0, egui::Color32::from_rgba_unmultiplied(180, 170, 150, 120)));
                    } else if none_hidden {
                        ui.painter().circle_filled(egui::pos2(eye_cx, eye_cy), eye_r,
                            egui::Color32::from_rgba_unmultiplied(200, 190, 170, 180));
                    } else {
                        // Mixed: half-filled -- outline + smaller filled inner
                        ui.painter().circle_stroke(egui::pos2(eye_cx, eye_cy), eye_r,
                            egui::Stroke::new(1.0, egui::Color32::from_rgba_unmultiplied(200, 190, 170, 150)));
                        ui.painter().circle_filled(egui::pos2(eye_cx, eye_cy), eye_r * 0.5,
                            egui::Color32::from_rgba_unmultiplied(200, 190, 170, 150));
                    }
                    // Hover highlight on eye zone
                    if eye_resp.hovered() {
                        ui.painter().circle_stroke(egui::pos2(eye_cx, eye_cy), eye_r + 2.0,
                            egui::Stroke::new(1.0, egui::Color32::from_rgba_unmultiplied(255, 255, 255, 40)));
                    }

                    // Build session refs for timeline
                    let sess_refs: Vec<(&SessionData, egui::Color32)> = members.iter()
                        .map(|(si, _)| (&data.sessions[*si], session_color(*si)))
                        .collect();

                    let badge = if active_count > 0 {
                        format!("{} x{} ({} active)", cwd, members.len(), active_count)
                    } else {
                        format!("{} x{}", cwd, members.len())
                    };
                    let arrow = if is_expanded { "\u{25be}" } else { "\u{25b8}" };
                    let header_name = format!("{} {}", arrow, badge);

                    let agc = if any_active { Some((g_active_cost, g_cost)) } else { None };

                    // [→] button to enter small mode (picks active session, or first)
                    let small_target = members.iter()
                        .find(|(si, _)| data.sessions[*si].is_active)
                        .or(members.first())
                        .map(|(si, _)| data.sessions[*si].session_id.clone());
                    if let Some(ref target_id) = small_target {
                        let btn_w = 36.0;
                        let btn_h = row_h - 4.0;
                        let btn_rect = egui::Rect::from_min_size(
                            egui::pos2(row_rect.right() - timeline_w - btn_w - 6.0, row_rect.top() + 2.0),
                            egui::vec2(btn_w, btn_h),
                        );
                        let btn_resp = ui.interact(btn_rect, egui::Id::new(("legend_small_grp", gi)), egui::Sense::click());
                        if btn_resp.clicked() { enter_small_mode = Some(target_id.clone()); }
                        if btn_resp.hovered() {
                            ui.painter().rect_filled(btn_rect, 2.0, egui::Color32::from_rgba_unmultiplied(255, 255, 255, 20));
                        }
                        ui.painter().text(egui::pos2(btn_rect.center().x, btn_rect.center().y - 4.0),
                            egui::Align2::CENTER_CENTER, "→",
                            egui::FontId::monospace(18.0), Palette::TEXT_DIM);
                        ui.painter().text(egui::pos2(btn_rect.center().x, btn_rect.center().y + 10.0),
                            egui::Align2::CENTER_CENTER, "detail",
                            egui::FontId::monospace(7.0), Palette::TEXT_DIM);
                    }

                    draw_legend_row(ui, row_rect, row_h, timeline_w, week_start_secs, week_span,
                        &header_name, bar_col, name_col, dim_col,
                        any_active, all_hidden,
                        g_last_input, g_cost, &g_model,
                        agc,
                        &sess_refs, effective_hidden, Some(eye_w));

                    row_idx += 1;

                    // Expanded sub-rows
                    if is_expanded {
                        for (si, _) in members {
                            let s = &data.sessions[*si];
                            let sess_col = session_color(*si);
                            let is_hidden = effective_hidden.contains(&s.session_id);
                            let manually_hidden = hidden.contains(&s.session_id);
                            let force_masked = *show_active_only && !s.is_active;
                            let sub_alpha = if is_hidden { 60u8 } else { 230u8 };
                            let sub_name_col = egui::Color32::from_rgba_unmultiplied(240, 230, 200, sub_alpha);
                            let sub_dim = egui::Color32::from_rgba_unmultiplied(110, 105, 90, sub_alpha / 2);

                            let sub_top = inner.top() + row_idx as f32 * (row_h + row_gap);
                            let sub_rect = egui::Rect::from_min_size(
                                egui::pos2(inner.left(), sub_top),
                                egui::vec2(inner.width(), row_h),
                            );
                            let sub_resp = ui.interact(sub_rect, egui::Id::new(("legend_sub", gi, *si)), egui::Sense::click());
                            if sub_resp.clicked() && !force_masked { toggle_ids.push(s.session_id.clone()); }
                            ui.painter().rect_filled(sub_rect, 2.0,
                                egui::Color32::from_rgba_unmultiplied(255, 255, 255, 3));
                            if sub_resp.hovered() {
                                ui.painter().rect_filled(sub_rect, 2.0,
                                    egui::Color32::from_rgba_unmultiplied(255, 255, 255, 10));
                                if s.first_ts > 0 {
                                    ui.ctx().data_mut(|d| d.get_temp_mut_or_default::<LegendHighlight>(legend_hl_id)
                                        .ranges.push((s.first_ts as f64 / 60.0, s.last_ts as f64 / 60.0)));
                                }
                            }

                            // Eye icon for sub-row
                            let eye_cx = sub_rect.left() + eye_w * 0.5 + 2.0 + 16.0; // extra indent for sub-rows
                            let eye_cy = sub_rect.center().y;
                            let eye_r = 3.5;
                            if force_masked {
                                ui.painter().circle_stroke(egui::pos2(eye_cx, eye_cy), eye_r,
                                    egui::Stroke::new(1.0, egui::Color32::from_rgba_unmultiplied(80, 75, 65, 60)));
                            } else if manually_hidden {
                                ui.painter().circle_stroke(egui::pos2(eye_cx, eye_cy), eye_r,
                                    egui::Stroke::new(1.0, egui::Color32::from_rgba_unmultiplied(180, 170, 150, 120)));
                            } else {
                                ui.painter().circle_filled(egui::pos2(eye_cx, eye_cy), eye_r,
                                    egui::Color32::from_rgba_unmultiplied(200, 190, 170, 180));
                            }

                            // [→] button to enter small mode (left of timeline)
                            let btn_w = 36.0;
                            let btn_h = row_h - 4.0;
                            let btn_rect = egui::Rect::from_min_size(
                                egui::pos2(sub_rect.right() - timeline_w - btn_w - 6.0, sub_rect.top() + 2.0),
                                egui::vec2(btn_w, btn_h),
                            );
                            let btn_resp = ui.interact(btn_rect, egui::Id::new(("legend_small_btn_sub", gi, *si)), egui::Sense::click());
                            if btn_resp.clicked() { enter_small_mode = Some(s.session_id.clone()); }
                            if btn_resp.hovered() {
                                ui.painter().rect_filled(btn_rect, 2.0, egui::Color32::from_rgba_unmultiplied(255, 255, 255, 20));
                            }
                            ui.painter().text(egui::pos2(btn_rect.center().x, btn_rect.center().y - 4.0),
                                egui::Align2::CENTER_CENTER, "→",
                                egui::FontId::monospace(18.0), Palette::TEXT_DIM);
                            ui.painter().text(egui::pos2(btn_rect.center().x, btn_rect.center().y + 10.0),
                                egui::Align2::CENTER_CENTER, "detail",
                                egui::FontId::monospace(7.0), Palette::TEXT_DIM);

                            // Session label: short id + active marker
                            let sid_short = if s.session_id.len() > 8 { &s.session_id[..8] } else { &s.session_id };
                            let sub_label = if s.is_active {
                                format!("  {} (active)", sid_short)
                            } else {
                                format!("  {}", sid_short)
                            };

                            draw_legend_row(ui, sub_rect, row_h, timeline_w, week_start_secs, week_span,
                                &sub_label, sess_col, sub_name_col, sub_dim,
                                s.is_active, is_hidden,
                                s.last_input_tokens, s.total_cost_usd, &s.model,
                                None,
                                &[(s, sess_col)], effective_hidden, Some(16.0 + eye_w));

                            row_idx += 1;
                        }
                    }
                }
            }
            // Tell ScrollArea the total content height so scrolling works
            let total_h = row_idx as f32 * (row_h + row_gap);
            ui.allocate_space(egui::vec2(ui.available_width(), total_h));
            }); // ScrollArea
            // Apply boosted scroll offset
            if extra_scroll.abs() > 0.1 {
                let mut state = scroll_out.state;
                state.offset.y = (state.offset.y - extra_scroll).max(0.0);
                state.store(ui.ctx(), legend_scroll_id);
            }
        });
    });

    // Apply expand/collapse
    if let Some(cwd) = toggle_expand {
        if expanded_groups.contains(&cwd) {
            expanded_groups.remove(&cwd);
        } else {
            expanded_groups.insert(cwd);
        }
    }
    // Apply group toggle: if any member is visible, hide all; otherwise show all
    if let Some((_cwd, member_ids)) = group_toggle {
        let any_visible = member_ids.iter().any(|id| !hidden.contains(id));
        if any_visible {
            for id in member_ids { hidden.insert(id); }
        } else {
            for id in &member_ids { hidden.remove(id); }
        }
    }
    // Apply individual session visibility toggles
    for id in toggle_ids {
        if hidden.contains(&id) {
            hidden.remove(&id);
        } else {
            hidden.insert(id);
        }
    }
    // Apply small mode entry
    if let Some(session_id) = enter_small_mode {
        *small_mode_session = Some(session_id);
    }

    let cursor_id = egui::Id::new("all_charts_cursor");
    let hover_id = egui::Id::new("hud_hover_turn");

    // Don't clear hover state every frame -- let it persist so tooltip stays visible
    // when mouse is stationary. Charts overwrite when cursor is inside them.
    // Clear only if pointer left the window entirely.
    if ui.ctx().input(|i| i.pointer.hover_pos()).is_none() {
        ui.ctx().data_mut(|d| d.remove::<HoverState>(hover_id));
    }

    // Screen-space containment check + source tracking.
    let update_hover_src = |pui: &egui_plot::PlotUi, source: HoverSource| {
        let Some(hover_pos) = pui.ctx().input(|i| i.pointer.hover_pos()) else { return };
        let b = pui.plot_bounds();
        let s_min = pui.screen_from_plot(egui_plot::PlotPoint::new(b.min()[0], b.min()[1]));
        let s_max = pui.screen_from_plot(egui_plot::PlotPoint::new(b.max()[0], b.max()[1]));
        if !egui::Rect::from_two_pos(s_min, s_max).contains(hover_pos) { return; }
        let x = pui.plot_from_screen(hover_pos).x;
        pui.ctx().data_mut(|d| d.insert_temp(hover_id, HoverState { x, source }));
    };

    // Read legend highlight for drawing VLines on charts
    let _legend_hl: LegendHighlight = ui.ctx().data(|d| d.get_temp(legend_hl_id).unwrap_or_default());
    let hl_color = egui::Color32::from_rgba_unmultiplied(220, 60, 60, 140);
    let _draw_legend_hl = |pui: &mut egui_plot::PlotUi, hl: &LegendHighlight| {
        for (start, end) in &hl.ranges {
            pui.vline(VLine::new(*start).color(hl_color).width(1.0));
            if (end - start).abs() > 0.5 {
                pui.vline(VLine::new(*end).color(hl_color).width(1.0));
            }
        }
    };

    // --- cost per-turn chart (ctx cost up / gen cost down) ---
    ui.allocate_new_ui(egui::UiBuilder::new().max_rect(cost_rect), |ui| {
        panel_frame().show(ui, |ui| {
            draw_chart_label(ui, "cost / turn", "ctx$", "gen$");
            let mut p = base_plot("cost_big")
                .link_cursor(cursor_id, true, false)
                .include_y(cd.per_turn_in_cost_max)
                .include_y(-cd.per_turn_out_cost_max)
                .y_axis_formatter(move |v, _| {
                    let abs = v.value.abs();
                    if abs < 1e-9 { String::new() } else { format_cost(abs) }
                })
                .show_axes([false, true])
                .show_grid(true);
            if let Some((vmin, vmax)) = if is_time { *nav_view } else { None } {
                p = p.include_x(vmin).include_x(vmax).auto_bounds(egui::Vec2b::new(false, true));
            }
            p.show(ui, |pui| {
                    pui.bar_chart(BarChart::new(cd.in_cost_bars.clone()).color(Palette::INPUT_TINT).name("ctx$"));
                    pui.bar_chart(BarChart::new(cd.out_cost_bars.clone()).color(Palette::OUTPUT_TINT).name("gen$"));
                    for x in &cd.agent_xs {
                        pui.vline(VLine::new(*x).color(Palette::AGENT_MARKER).width(0.5).name("agent"));
                    }
                    update_hover_src(pui, HoverSource::Cost);
                    // draw_legend_hl(pui, &legend_hl); // disabled: causes chart rescale on hover
                });
        });
    });

    // --- total cost chart ---
    ui.allocate_new_ui(egui::UiBuilder::new().max_rect(total_cost_rect), |ui| {
        panel_frame().show(ui, |ui| {
            draw_chart_label(ui, "total cost", "", "");
            let mut p = base_plot("total_cost_big")
                .link_cursor(cursor_id, true, false)
                .include_y(0.0)
                .include_y(cd.total_cost_max)
                .y_axis_formatter(move |v, _| {
                    if v.value < 1e-9 { String::new() } else { format_cost(v.value) }
                })
                .show_axes([is_time, true])
                .show_grid(true);
            if is_time { p = p.x_axis_formatter(time_x_fmt); }
            if let Some((vmin, vmax)) = if is_time { *nav_view } else { None } {
                p = p.include_x(vmin).include_x(vmax).auto_bounds(egui::Vec2b::new(false, true));
            }
            p
                .show(ui, |pui| {
                    for (color, points) in &cd.total_cost_lines {
                        pui.line(egui_plot::Line::new(points.clone()).color(*color).width(2.0));
                    }
                    for x in &cd.agent_xs {
                        pui.vline(VLine::new(*x).color(Palette::AGENT_MARKER).width(0.5));
                    }
                    update_hover_src(pui, HoverSource::TotalCost);
                    // draw_legend_hl(pui, &legend_hl); // disabled: causes chart rescale on hover
                });
        });
    });

    // --- token per-turn chart ---
    ui.allocate_new_ui(egui::UiBuilder::new().max_rect(tok_rect), |ui| {
        panel_frame().show(ui, |ui| {
            draw_chart_label(ui, "tokens / turn", "in", "out");
            let mut p = base_plot("tok_big")
                .link_cursor(cursor_id, true, false)
                .include_y(cd.in_max)
                .include_y(-cd.out_max)
                .y_axis_formatter(move |v, _| {
                    let abs = v.value.abs();
                    if abs < 0.5 { return String::new(); }
                    format_tokens(abs.round() as u64)
                })
                .show_axes([false, true])
                .show_grid(true);
            if let Some((vmin, vmax)) = if is_time { *nav_view } else { None } {
                p = p.include_x(vmin).include_x(vmax).auto_bounds(egui::Vec2b::new(false, true));
            }
            p.show(ui, |pui| {
                    pui.bar_chart(BarChart::new(cd.in_tok_bars.clone()).color(Palette::INPUT_TINT).name("in"));
                    pui.bar_chart(BarChart::new(cd.out_tok_bars.clone()).color(Palette::OUTPUT_TINT).name("out"));
                    update_hover_src(pui, HoverSource::Tokens);
                    // draw_legend_hl(pui, &legend_hl); // disabled: causes chart rescale on hover
                });
        });
    });

    // --- running total token chart ---
    ui.allocate_new_ui(egui::UiBuilder::new().max_rect(total_tok_rect), |ui| {
        panel_frame().show(ui, |ui| {
            draw_chart_label(ui, "total tokens", "", "");
            let mut p = base_plot("total_tok_big")
                .link_cursor(cursor_id, true, false)
                .include_y(0.0)
                .include_y(cd.total_tok_max)
                .y_axis_formatter(move |v, _| {
                    if v.value < 0.5 { String::new() } else { format_tokens(v.value.round() as u64) }
                })
                .show_axes([is_time, true])
                .show_grid(true);
            if is_time { p = p.x_axis_formatter(time_x_fmt); }
            if let Some((vmin, vmax)) = if is_time { *nav_view } else { None } {
                p = p.include_x(vmin).include_x(vmax).auto_bounds(egui::Vec2b::new(false, true));
            }
            p
                .show(ui, |pui| {
                    for (color, in_pts, out_pts) in &cd.total_tok_lines {
                        pui.line(egui_plot::Line::new(in_pts.clone()).color(*color).width(2.0).name("in"));
                        pui.line(egui_plot::Line::new(out_pts.clone()).color(*color).width(1.5)
                            .style(egui_plot::LineStyle::Dashed { length: 8.0 }).name("out"));
                    }
                    update_hover_src(pui, HoverSource::TotalTokens);
                    // draw_legend_hl(pui, &legend_hl); // disabled: causes chart rescale on hover
                });
        });
    });

    // --- usage chart (5h + 7d utilization over time) ---
    // Clear session hover tooltip when pointer is over usage chart
    if let Some(pos) = ui.ctx().input(|i| i.pointer.hover_pos()) {
        if usage_rect.contains(pos) {
            ui.ctx().data_mut(|d| d.remove::<HoverState>(hover_id));
        }
    }
    ui.allocate_new_ui(egui::UiBuilder::new().max_rect(usage_rect), |ui| {
        panel_frame().show(ui, |ui| {
            draw_chart_label(ui, "usage %", "5h", "7d");

            if let Some(latest) = &usage.latest {
                // Show current values as text
                let inner = ui.available_rect_before_wrap();
                let painter = ui.painter();
                let font = egui::FontId::monospace(9.0);
                painter.text(
                    egui::pos2(inner.right() - 60.0, inner.top()),
                    egui::Align2::LEFT_TOP,
                    format!("{}%  {}%", latest.five_hour as u32, latest.seven_day as u32),
                    font,
                    Palette::TEXT_DIM,
                );
            }

            if usage.snapshots.len() >= 2 {
                let five_h_pts: Vec<[f64; 2]> = usage.snapshots.iter()
                    .map(|s| [s.ts as f64 / 60.0, s.five_hour])
                    .collect();
                let seven_d_pts: Vec<[f64; 2]> = usage.snapshots.iter()
                    .map(|s| [s.ts as f64 / 60.0, s.seven_day])
                    .collect();

                let usage_time_fmt = move |v: egui_plot::GridMark, _: &std::ops::RangeInclusive<f64>| -> String {
                    let ago_min = now_min - v.value;
                    if ago_min < 0.5 { "now".into() }
                    else if ago_min < 60.0 { format!("{}m", ago_min.round() as i64) }
                    else if ago_min < 24.0 * 60.0 { format!("{:.0}h", ago_min / 60.0) }
                    else { format!("{:.0}d", ago_min / (24.0 * 60.0)) }
                };

                // Clone snapshots for the label formatter closure
                let snap_for_tip = usage.snapshots.clone();
                let tip_fmt = move |_name: &str, point: &egui_plot::PlotPoint| -> String {
                    let hx = point.x;
                    // Find nearest snapshot
                    let nearest = snap_for_tip.iter().min_by(|a, b| {
                        let ax = a.ts as f64 / 60.0;
                        let bx = b.ts as f64 / 60.0;
                        (ax - hx).abs().partial_cmp(&(bx - hx).abs()).unwrap()
                    });
                    if let Some(s) = nearest {
                        let ago_min = now_min - (s.ts as f64 / 60.0);
                        let ago = if ago_min < 1.0 { "now".into() }
                            else if ago_min < 60.0 { format!("{}m ago", ago_min.round() as i64) }
                            else if ago_min < 24.0 * 60.0 { format!("{:.1}h ago", ago_min / 60.0) }
                            else { format!("{:.1}d ago", ago_min / (24.0 * 60.0)) };
                        let mut tip = format!("{}\n5h: {:.0}%\n7d: {:.0}%", ago, s.five_hour, s.seven_day);
                        if let Some(opus) = s.seven_day_opus { tip += &format!("\nopus 7d: {:.0}%", opus); }
                        if let Some(sonnet) = s.seven_day_sonnet { tip += &format!("\nsonnet 7d: {:.0}%", sonnet); }
                        tip
                    } else {
                        String::new()
                    }
                };

                Plot::new("usage_pct")
                    .show_axes([true, true])
                    .show_grid(true)
                    .allow_zoom(false)
                    .allow_drag(false)
                    .allow_scroll(false)
                    .show_background(false)
                    .set_margin_fraction(egui::Vec2::ZERO)
                    .auto_bounds(egui::Vec2b::new(true, true))
                    .include_y(0.0)
                    .include_y(100.0)
                    .y_axis_formatter(|v, _| {
                        if v.value < 0.5 { String::new() } else { format!("{}%", v.value as u32) }
                    })
                    .x_axis_formatter(usage_time_fmt)
                    .label_formatter(tip_fmt)
                    .show(ui, |pui| {
                        pui.line(egui_plot::Line::new(five_h_pts)
                            .color(egui::Color32::from_rgb(220, 160, 60)).width(1.5).name("5h"));
                        pui.line(egui_plot::Line::new(seven_d_pts)
                            .color(egui::Color32::from_rgb(100, 160, 220)).width(1.5).name("7d"));
                        pui.hline(egui_plot::HLine::new(100.0)
                            .color(egui::Color32::from_rgba_unmultiplied(200, 60, 60, 80))
                            .width(0.5));
                    });
            } else if let Some(e) = &usage.error {
                let inner = ui.available_rect_before_wrap();
                ui.painter().text(inner.center(), egui::Align2::CENTER_CENTER,
                    e, egui::FontId::monospace(9.0), Palette::TEXT_DIM);
            } else {
                let inner = ui.available_rect_before_wrap();
                ui.painter().text(inner.center(), egui::Align2::CENTER_CENTER,
                    "polling...", egui::FontId::monospace(9.0), Palette::TEXT_DIM);
            }
        });
    });

    // Clear session hover tooltip when pointer is over tool panel
    if let Some(pos) = ui.ctx().input(|i| i.pointer.hover_pos()) {
        if tool_rect.contains(pos) {
            ui.ctx().data_mut(|d| d.remove::<HoverState>(hover_id));
        }
    }
    // --- tool breakdown ---
    ui.allocate_new_ui(egui::UiBuilder::new().max_rect(tool_rect), |ui| {
        panel_frame().show(ui, |ui| {
            let inner = ui.available_rect_before_wrap();
            let painter = ui.painter();

            painter.text(
                egui::pos2(inner.left(), inner.top()),
                egui::Align2::LEFT_TOP,
                "tool calls",
                egui::FontId::monospace(10.0),
                Palette::TEXT_DIM,
            );

            if cd.tool_list.is_empty() { return; }
            let max_count = cd.tool_list[0].1.max(1) as f32;
            let n = cd.tool_list.len();
            let list_top = inner.top() + 16.0;
            let row_h = ((inner.height() - 16.0) / n as f32).min(26.0);
            let name_w = 44.0_f32;
            let count_w = 28.0_f32;
            let bar_max_w = inner.width() - name_w - count_w - 4.0;

            for (i, (name, count)) in cd.tool_list.iter().enumerate() {
                let y = list_top + i as f32 * row_h;
                let cy = y + row_h * 0.5;

                painter.text(
                    egui::pos2(inner.left(), cy),
                    egui::Align2::LEFT_CENTER,
                    name,
                    egui::FontId::monospace(10.5),
                    Palette::TEXT,
                );

                let bar_w = (*count as f32 / max_count) * bar_max_w;
                if bar_w > 0.5 {
                    painter.rect_filled(
                        egui::Rect::from_min_size(egui::pos2(inner.left() + name_w, y + row_h * 0.25), egui::vec2(bar_w, row_h * 0.5)),
                        2.0, Palette::TOOL_BAR,
                    );
                }

                painter.text(
                    egui::pos2(inner.left() + name_w + bar_max_w + 4.0, cy),
                    egui::Align2::LEFT_CENTER,
                    &count.to_string(),
                    egui::FontId::monospace(10.0),
                    Palette::TEXT_DIM,
                );
            }
        });
    });

    // --- bottom row: combined cost + cost rate (from ChartData, same x-domain) ---
    {
        let rate_label = if is_time { "cost/hr" } else { "cost/turn" };

        ui.allocate_new_ui(egui::UiBuilder::new().max_rect(weekly_total_rect), |ui| {
            panel_frame().show(ui, |ui| {
                draw_chart_label(ui, "combined cost", "", "");
                let mut p = base_plot("combined_cost")
                    .link_cursor(cursor_id, true, false)
                    .include_y(0.0)
                    .include_y(cd.combined_cost_max)
                    .y_axis_formatter(move |v, _| {
                        if v.value < 1e-9 { String::new() } else { format_cost(v.value) }
                    })
                    .show_axes([is_time, true])
                    .show_grid(true);
                if is_time { p = p.x_axis_formatter(time_x_fmt); }
                if let Some((vmin, vmax)) = if is_time { *nav_view } else { None } {
                    p = p.include_x(vmin).include_x(vmax).auto_bounds(egui::Vec2b::new(false, true));
                }
                p.show(ui, |pui| {
                    pui.line(egui_plot::Line::new(cd.combined_cost_pts.clone())
                        .color(Palette::INPUT_TINT).width(1.0)
                        .style(egui_plot::LineStyle::Dashed { length: 4.0 }));
                    pui.points(egui_plot::Points::new(cd.combined_cost_pts.clone())
                        .color(Palette::INPUT_TINT).radius(2.0));
                    update_hover_src(pui, HoverSource::WeeklyCost);
                    // draw_legend_hl(pui, &legend_hl); // disabled: causes chart rescale on hover
                });
            });
        });

        ui.allocate_new_ui(egui::UiBuilder::new().max_rect(weekly_rate_rect), |ui| {
            panel_frame().show(ui, |ui| {
                draw_chart_label(ui, rate_label, "", "");
                let mut p = base_plot("cost_rate")
                    .link_cursor(cursor_id, true, false)
                    .include_y(0.0)
                    .include_y(cd.cost_rate_max)
                    .y_axis_formatter(move |v, _| {
                        if v.value < 1e-9 { String::new() } else { format_cost(v.value) }
                    })
                    .show_axes([is_time, true])
                    .show_grid(true);
                if is_time { p = p.x_axis_formatter(time_x_fmt); }
                if let Some((vmin, vmax)) = if is_time { *nav_view } else { None } {
                    p = p.include_x(vmin).include_x(vmax).auto_bounds(egui::Vec2b::new(false, true));
                }
                p.show(ui, |pui| {
                    pui.line(egui_plot::Line::new(cd.cost_rate_pts.clone())
                        .color(Palette::OUTPUT_TINT).width(1.0)
                        .style(egui_plot::LineStyle::Dashed { length: 4.0 }));
                    pui.points(egui_plot::Points::new(cd.cost_rate_pts.clone())
                        .color(Palette::OUTPUT_TINT).radius(2.0));
                    update_hover_src(pui, HoverSource::WeeklyRate);
                    // draw_legend_hl(pui, &legend_hl); // disabled: causes chart rescale on hover
                });
            });
        });
    }

    // --- floating hover tooltip (all sessions, context-aware) ---
    // Placed after all charts so hover state from bottom charts is available same-frame.
    let hover_state: Option<HoverState> = ui.ctx().data(|d| d.get_temp(hover_id));
    if let Some(hs) = hover_state {
        let hx = hs.x;
        if let Some(cursor) = ui.ctx().input(|i| i.pointer.hover_pos()) {
            let mut entries: Vec<(String, String)> = vec![];
            for (name, _, turns) in &cd.session_turns {
                let nearest = turns.iter().enumerate().min_by(|(_, a), (_, b)| {
                    (a.x - hx).abs().partial_cmp(&(b.x - hx).abs()).unwrap()
                });
                if let Some((idx, t)) = nearest {
                    let snap = if turns.len() > 1 {
                        let span = turns.last().unwrap().x - turns.first().unwrap().x;
                        (span / turns.len() as f64).max(0.5)
                    } else { 1.0 };
                    if (t.x - hx).abs() <= snap {
                        let detail = match hs.source {
                            HoverSource::Cost => format!(
                                "  t{} [{}] ctx {}  gen {}  (+{})",
                                idx + 1, t.model_short,
                                format_cost(t.in_cost), format_cost(t.out_cost),
                                format_cost(t.cost_change),
                            ),
                            HoverSource::TotalCost => format!(
                                "  t{} total {}  (+{})",
                                idx + 1,
                                format_cost(t.total_cost),
                                format_cost(t.cost_change),
                            ),
                            HoverSource::Tokens => format!(
                                "  t{} [{}] in {}  out {}",
                                idx + 1, t.model_short,
                                format_tokens(t.in_tok), format_tokens(t.out_tok),
                            ),
                            HoverSource::TotalTokens => format!(
                                "  t{} total in {}  out {}",
                                idx + 1,
                                format_tokens(t.total_in_tok), format_tokens(t.total_out_tok),
                            ),
                            HoverSource::WeeklyCost | HoverSource::WeeklyRate => format!(
                                "  t{} [{}] +{}  total {}",
                                idx + 1, t.model_short,
                                format_cost(t.cost_change),
                                format_cost(t.total_cost),
                            ),
                        };
                        entries.push((name.clone(), detail));
                    }
                }
            }
            if !entries.is_empty() {
                let win_rect = ui.ctx().screen_rect();
                let row_h = 15.0_f32;
                let tip_h = row_h * (1.0 + entries.len() as f32 * 2.0) + 16.0;
                let mut tip_pos = cursor + egui::vec2(14.0, -tip_h - 8.0);
                tip_pos.y = tip_pos.y.max(win_rect.top() + 4.0);

                egui::Area::new(egui::Id::new("hud_float_tip"))
                    .fixed_pos(tip_pos)
                    .order(egui::Order::Tooltip)
                    .show(ui.ctx(), |ui| {
                        egui::Frame::none()
                            .fill(egui::Color32::from_rgba_premultiplied(20, 18, 14, 240))
                            .stroke(egui::Stroke::new(0.5, Palette::SEPARATOR))
                            .rounding(5.0)
                            .inner_margin(egui::Margin::same(8.0))
                            .show(ui, |ui| {
                                ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Extend);
                                for (name, data) in &entries {
                                    ui.add(egui::Label::new(
                                        egui::RichText::new(name).monospace().size(12.0).color(Palette::TEXT_BRIGHT)
                                    ));
                                    ui.add(egui::Label::new(
                                        egui::RichText::new(data).monospace().size(11.0).color(Palette::TEXT_DIM)
                                    ));
                                }
                            });
                    });
            }
        }
    }
}

fn draw_chart_label(ui: &mut egui::Ui, title: &str, top_label: &str, bot_label: &str) {
    let rect = ui.available_rect_before_wrap();
    let p = ui.painter();
    // Inset from edges to avoid colliding with y-axis tick labels
    let y_inset = 12.0; // clear top/bottom axis ticks
    let x_inset = 4.0;
    p.text(egui::pos2(rect.left() + x_inset, rect.top() + y_inset), egui::Align2::LEFT_TOP, title, egui::FontId::monospace(10.0), Palette::TEXT_DIM);
    p.text(egui::pos2(rect.right() - x_inset, rect.top() + y_inset), egui::Align2::RIGHT_TOP, top_label, egui::FontId::monospace(9.0), Palette::INPUT_TINT);
    p.text(egui::pos2(rect.right() - x_inset, rect.bottom() - y_inset), egui::Align2::RIGHT_BOTTOM, bot_label, egui::FontId::monospace(9.0), Palette::OUTPUT_TINT);
}

// ---------------------------------------------------------------------------
// Strip layout (original compact HUD)
// ---------------------------------------------------------------------------

fn draw_strip(ui: &mut egui::Ui, data: &HudData, cd: &ChartData) {
    let area = ui.available_rect_before_wrap();
    let pad = 2.0;
    let gap = 2.0;
    let w = area.width() - pad * 2.0;
    let h = area.height() - pad * 2.0;
    let x0 = area.left() + pad;
    let y0 = area.top() + pad;

    let cost_w   = w * 0.38;
    let token_w  = w * 0.17;
    let tool_w   = w * 0.20;
    let legend_w = w - cost_w - token_w - tool_w - gap * 3.0;

    let cost_rect   = egui::Rect::from_min_size(egui::pos2(x0, y0), egui::vec2(cost_w, h));
    let token_rect  = egui::Rect::from_min_size(egui::pos2(x0 + cost_w + gap, y0), egui::vec2(token_w, h));
    let tool_rect   = egui::Rect::from_min_size(egui::pos2(x0 + cost_w + token_w + gap * 2.0, y0), egui::vec2(tool_w, h));
    let legend_rect = egui::Rect::from_min_size(egui::pos2(x0 + cost_w + token_w + tool_w + gap * 3.0, y0), egui::vec2(legend_w, h));

    let strip_frame = egui::Frame::none()
        .fill(Palette::BG)
        .stroke(egui::Stroke::new(0.5, Palette::GRID))
        .rounding(3.0)
        .inner_margin(egui::Margin::same(2.0));

    ui.allocate_new_ui(egui::UiBuilder::new().max_rect(cost_rect), |ui| {
        strip_frame.show(ui, |ui| {
            base_plot("cost_strip")
                .include_y(0.0)
                .include_y(cd.per_turn_in_cost_max * 1.1)
                .include_y(-cd.per_turn_out_cost_max * 1.1)
                .show(ui, |pui| {
                    pui.bar_chart(BarChart::new(cd.in_cost_bars.clone()).color(Palette::INPUT_TINT).name("in$"));
                    pui.bar_chart(BarChart::new(cd.out_cost_bars.clone()).color(Palette::OUTPUT_TINT).name("out$"));
                    for x in &cd.agent_xs {
                        pui.vline(VLine::new(*x).color(Palette::AGENT_MARKER).width(0.5));
                    }
                });
        });
    });

    ui.allocate_new_ui(egui::UiBuilder::new().max_rect(token_rect), |ui| {
        strip_frame.show(ui, |ui| {
            base_plot("tok_strip")
                .include_y(0.0)
                .include_y(cd.in_max * 1.1)
                .include_y(-cd.out_max * 1.1)
                .show(ui, |pui| {
                    pui.bar_chart(BarChart::new(cd.in_tok_bars.clone()).color(Palette::INPUT_TINT).name("in"));
                    pui.bar_chart(BarChart::new(cd.out_tok_bars.clone()).color(Palette::OUTPUT_TINT).name("out"));
                });
        });
    });

    ui.allocate_new_ui(egui::UiBuilder::new().max_rect(tool_rect), |ui| {
        strip_frame.show(ui, |ui| {
            let inner = ui.available_rect_before_wrap();
            draw_tool_strip(ui.painter(), inner, &cd.tool_list);
        });
    });

    ui.allocate_new_ui(egui::UiBuilder::new().max_rect(legend_rect), |ui| {
        strip_frame.show(ui, |ui| {
            let inner = ui.available_rect_before_wrap();
            let painter = ui.painter();
            let n = data.sessions.len();
            let row_h = (inner.height() / n.max(1) as f32).min(20.0);
            let font = egui::FontId::monospace((row_h * 0.55).min(9.0));

            for (si, session) in data.sessions.iter().enumerate() {
                let row_y = inner.top() + si as f32 * row_h;
                painter.circle_filled(egui::pos2(inner.left() + 4.0, row_y + row_h * 0.5), 2.5, session_color(si));
                painter.text(egui::pos2(inner.left() + 10.0, row_y + row_h * 0.3), egui::Align2::LEFT_CENTER, &session.cwd, font.clone(), Palette::TEXT);
                let stats = format!("{} (in:{} out:{}){}", format_cost(session.total_cost_usd),
                    format_cost(session.total_input_cost), format_cost(session.total_output_cost),
                    if session.agent_count > 0 { format!(" {}agt", session.agent_count) } else { String::new() });
                painter.text(egui::pos2(inner.left() + 10.0, row_y + row_h * 0.75), egui::Align2::LEFT_CENTER, &stats,
                    egui::FontId::monospace((row_h * 0.4).min(7.5)), Palette::TEXT_DIM);
            }

            if data.sessions.iter().any(|s| s.agent_count > 0) {
                painter.line_segment([egui::pos2(inner.right() - 30.0, inner.bottom() - 4.0), egui::pos2(inner.right() - 30.0, inner.bottom() - 8.0)], egui::Stroke::new(1.5, Palette::AGENT_MARKER));
                painter.text(egui::pos2(inner.right() - 27.0, inner.bottom() - 6.0), egui::Align2::LEFT_CENTER, "agt", egui::FontId::monospace(6.0), Palette::AGENT_MARKER);
            }
        });
    });
}

fn draw_tool_strip(painter: &egui::Painter, area: egui::Rect, tool_list: &[(String, u32)]) {
    if tool_list.is_empty() { return; }
    let max_count = tool_list[0].1.max(1) as f32;
    let n = tool_list.len();
    let row_h = (area.height() / n as f32).min(14.0);
    let font = egui::FontId::monospace((row_h * 0.52).clamp(5.5, 8.0));
    let name_w = 28.0_f32;
    let count_w = 16.0_f32;
    let bar_max_w = area.width() - name_w - count_w - 2.0;
    for (i, (name, count)) in tool_list.iter().enumerate() {
        let y = area.top() + i as f32 * row_h;
        let cy = y + row_h * 0.5;
        painter.text(egui::pos2(area.left(), cy), egui::Align2::LEFT_CENTER, name, font.clone(), Palette::TEXT_DIM);
        let bar_w = (*count as f32 / max_count) * bar_max_w;
        if bar_w > 0.5 {
            painter.rect_filled(egui::Rect::from_min_size(egui::pos2(area.left() + name_w, y + row_h * 0.25), egui::vec2(bar_w, row_h * 0.5)), 1.0, Palette::TOOL_BAR);
        }
        painter.text(egui::pos2(area.left() + name_w + bar_max_w + 2.0, cy), egui::Align2::LEFT_CENTER, &count.to_string(), font.clone(), Palette::TEXT_DIM);
    }
}

// ---------------------------------------------------------------------------
// EguiOverlay impl
// ---------------------------------------------------------------------------

impl EguiOverlay for Hud {
    fn gui_run(
        &mut self,
        egui_context: &egui::Context,
        _default_gfx_backend: &mut DefaultGfxBackend,
        glfw_backend: &mut GlfwBackend,
    ) {
        let rect = *self.state.lock().unwrap();
        let is_visible = self.visible.load(std::sync::atomic::Ordering::Relaxed);
        let big_mode = self.big_mode;

        if !big_mode {
            glfw_backend.set_passthrough(true);
        }

        if !is_visible {
            glfw_backend.window.set_pos(-9999, -9999);
            glfw_backend.set_window_size([1.0, 1.0]);
            egui_context.request_repaint();
            return;
        }

        if self.first_frame {
            self.first_frame = false;
            glfw_backend.window.set_pos(rect.x, rect.y);
            glfw_backend.set_window_size([rect.w as f32, rect.h as f32]);
            glfw_backend.window.show();
        } else if !big_mode {
            glfw_backend.window.set_pos(rect.x, rect.y);
            glfw_backend.set_window_size([rect.w as f32, rect.h as f32]);
        }

        let data = self.hud_data.lock().unwrap().clone();
        let big_mode = self.big_mode;

        // Validate small mode session still exists
        if let Some(ref sid) = self.small_mode_session {
            if data.sessions.iter().all(|s| s.session_id != *sid) {
                self.small_mode_session = None;
            }
        }

        // Resize window for small mode: shrink to 160px on entry, then user can resize freely
        if big_mode && self.small_mode_session.is_some() {
            if self.pre_small_window_size.is_none() {
                let (win_w, cur_h) = glfw_backend.window.get_size();
                self.pre_small_window_size = Some((win_w, cur_h));
                glfw_backend.set_window_size([win_w as f32, 160.0]);
            }
        } else if big_mode && self.small_mode_session.is_none() {
            if let Some((sw, sh)) = self.pre_small_window_size.take() {
                let (cur_w, cur_h) = glfw_backend.window.get_size();
                if cur_h != sh || cur_w != sw {
                    glfw_backend.set_window_size([sw as f32, sh as f32]);
                }
            }
        }

        let bg = if big_mode { egui::Color32::from_rgb(14, 12, 9) } else { Palette::BG };
        egui::CentralPanel::default()
            .frame(egui::Frame::none().fill(bg))
            .show(egui_context, |ui| {
                if data.sessions.is_empty() {
                    let area = ui.available_rect_before_wrap();
                    ui.painter().text(area.center(), egui::Align2::CENTER_CENTER, "no active sessions",
                        egui::FontId::monospace(if big_mode { 16.0 } else { area.height() * 0.3 }), Palette::TEXT_DIM);
                    egui_context.request_repaint();
                    return;
                }

                // Effective hidden: user-toggled set + inactive sessions (when filter is on)
                let effective_hidden: HashSet<String> = if self.show_active_only {
                    let mut h = self.hidden_sessions.clone();
                    for s in &data.sessions {
                        if !s.is_active { h.insert(s.session_id.clone()); }
                    }
                    h
                } else {
                    self.hidden_sessions.clone()
                };

                let cd = build_chart_data(&data, &effective_hidden, self.time_axis);
                let usage = self.usage_data.lock().unwrap().clone();

                if big_mode {
                    if let Some(sid) = self.small_mode_session.clone() {
                        draw_small(ui, &data, &cd, &usage, &sid, &self.hidden_sessions, &mut self.show_active_only, &mut self.time_axis, &mut self.autofit, &mut self.nav_view, &mut self.small_mode_session);
                    } else {
                        draw_big(ui, &data, &cd, &usage, &mut self.hidden_sessions, &effective_hidden, &mut self.show_active_only, &mut self.time_axis, &mut self.autofit, &mut self.nav_view, &mut self.expanded_groups, &mut self.small_mode_session);
                    }
                } else {
                    draw_strip(ui, &data, &cd);
                }
            });

        egui_context.request_repaint();
    }
}

fn start_overlay(user_data: Hud) {
    use egui_overlay::egui_window_glfw_passthrough::{GlfwConfig, glfw};
    use egui_overlay::egui_render_wgpu::{WgpuBackend, WgpuConfig};
    use egui_overlay::OverlayApp;

    let big_mode = user_data.big_mode;

    if !big_mode {
        hide_dock_icon();
    }

    let mut glfw_backend = GlfwBackend::new(GlfwConfig {
        glfw_callback: Box::new(move |gtx| {
            (GlfwConfig::default().glfw_callback)(gtx);
            gtx.window_hint(glfw::WindowHint::ScaleToMonitor(true));
            gtx.window_hint(glfw::WindowHint::FocusOnShow(false));
            gtx.window_hint(glfw::WindowHint::Visible(false));
        }),
        opengl_window: Some(false),
        transparent_window: Some(!big_mode),
        ..Default::default()
    });
    if !big_mode {
        glfw_backend.window.set_floating(true);
        glfw_backend.window.set_decorated(false);
    } else {
        glfw_backend.window.set_resizable(true);
    }

    #[cfg(target_os = "macos")]
    {
        use objc2::rc::Retained;
        use objc2_app_kit::{NSWindow, NSWindowCollectionBehavior};
        let raw = glfw_backend.window.get_cocoa_window() as *mut objc2::runtime::AnyObject;
        let ns_window: Retained<NSWindow> = unsafe { Retained::retain(raw.cast()).unwrap() };
        ns_window.setCollectionBehavior(
            NSWindowCollectionBehavior::CanJoinAllSpaces
                | NSWindowCollectionBehavior::FullScreenAuxiliary,
        );
    }

    let latest_size = glfw_backend.window.get_framebuffer_size();
    let latest_size = [latest_size.0 as _, latest_size.1 as _];

    let mut wgpu_config = WgpuConfig::default();
    wgpu_config.device_descriptor.required_limits.max_texture_dimension_2d = 8192;

    let default_gfx_backend = WgpuBackend::new(
        wgpu_config,
        Some(Box::new(glfw_backend.window.render_context())),
        latest_size,
    );

    OverlayApp {
        user_data,
        egui_context: Default::default(),
        default_gfx_backend,
        glfw_backend,
    }.enter_event_loop();
}

fn hide_dock_icon() {
    #[cfg(target_os = "macos")]
    {
        use objc2::MainThreadMarker;
        use objc2_app_kit::NSApplication;
        use objc2_app_kit::NSApplicationActivationPolicy;
        let mtm = unsafe { MainThreadMarker::new_unchecked() };
        let app = NSApplication::sharedApplication(mtm);
        app.setActivationPolicy(NSApplicationActivationPolicy::Accessory);
    }
}

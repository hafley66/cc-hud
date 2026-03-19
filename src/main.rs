#![allow(dead_code)]

mod geometry;
mod anchors;
mod agent_harnesses;

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use egui_overlay::EguiOverlay;
use egui_overlay::egui_render_wgpu::WgpuBackend as DefaultGfxBackend;
use egui_overlay::egui_window_glfw_passthrough::GlfwBackend;
use egui_plot::{Bar, BarChart, Plot, VLine};

use geometry::PixelRect;
use agent_harnesses::claude_code::{Event, HudData};

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

    if let Some(target) = poll_target {
        let poll_state = state.clone();
        let poll_visible = visible.clone();
        std::thread::spawn(move || { pane_poll_loop(target, poll_state, poll_visible); });
    }

    let feed_data = hud_data.clone();
    std::thread::spawn(move || {
        agent_harnesses::claude_code::poll_loop(feed_data, show_history);
    });

    start_overlay(Hud { first_frame: true, state, visible, hud_data, big_mode, hidden_sessions: HashSet::new(), show_active_only: false, time_axis: false, autofit: true, nav_view: None });
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
    big_mode: bool,
    hidden_sessions: HashSet<String>,
    show_active_only: bool,
    time_axis: bool,
    autofit: bool,
    /// Chart viewport x-range in minutes-from-epoch. None = auto-fit to all data.
    nav_view: Option<(f64, f64)>,
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

/// Wrapper for storing an f64 in egui temp storage (needs Clone + Send + Sync + 'static).
#[derive(Clone, Copy)]
struct HoverX(f64);

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
    /// Per-session total cost lines: (color, points)
    total_cost_lines: Vec<(egui::Color32, Vec<[f64; 2]>)>,
    total_cost_max: f64,
    /// Per-session running token lines: (color, in_points, out_points)
    total_tok_lines: Vec<(egui::Color32, Vec<[f64; 2]>, Vec<[f64; 2]>)>,
    total_tok_max: f64,
    /// Per-session turn data for tooltips: (display_name, session_color, turns)
    session_turns: Vec<(String, egui::Color32, Vec<TurnInfo>)>,
}

/// Combined time-series across all visible sessions, in minutes-from-epoch coordinates
/// (same x-axis as all other time-mode charts).
struct WeeklyData {
    /// (minutes_from_epoch, running_cost)
    total_pts: Vec<[f64; 2]>,
    total_max: f64,
    /// (minutes_from_epoch, cost_in_that_hour_bucket)
    rate_pts: Vec<[f64; 2]>,
    rate_max: f64,
}

fn build_weekly_data(data: &HudData, hidden: &HashSet<String>) -> Option<WeeklyData> {
    let now_secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs()).unwrap_or(0);

    // Collect all visible API calls (no 7d filter -- viewport handles clipping)
    let mut raw: Vec<(u64, f64)> = vec![];
    for session in &data.sessions {
        if hidden.contains(&session.session_id) { continue; }
        for ev in &session.events {
            if let Event::ApiCall { timestamp_secs, input_cost_usd, output_cost_usd, .. } = ev {
                if *timestamp_secs == 0 { continue; }
                raw.push((*timestamp_secs, input_cost_usd + output_cost_usd));
            }
        }
    }
    if raw.is_empty() { return None; }
    raw.sort_by_key(|(t, _)| *t);

    let first_secs = raw.first().unwrap().0;
    let now_min = now_secs as f64 / 60.0;

    // Running total line in minutes-from-epoch
    let mut running = 0.0f64;
    let mut total_pts: Vec<[f64; 2]> = vec![[first_secs as f64 / 60.0, 0.0]];
    for (t, cost) in &raw {
        running += cost;
        total_pts.push([*t as f64 / 60.0, running]);
    }
    total_pts.push([now_min, running]);
    let total_max = running.max(0.001);

    // Hourly rate buckets, keyed by minutes-from-epoch (bucket center)
    let first_hour = first_secs / 3600;
    let now_hour = now_secs / 3600;
    let n_buckets = ((now_hour - first_hour) as usize + 1).min(10_000);
    let mut buckets = vec![0.0f64; n_buckets + 1];
    for (t, cost) in &raw {
        let idx = ((t / 3600).saturating_sub(first_hour)) as usize;
        buckets[idx.min(n_buckets)] += cost;
    }
    let rate_pts: Vec<[f64; 2]> = buckets.iter().enumerate().map(|(i, &c)| {
        let hour_secs = (first_hour + i as u64) * 3600;
        [hour_secs as f64 / 60.0, c]
    }).collect();
    let rate_max = buckets.iter().cloned().fold(0.001_f64, f64::max);

    Some(WeeklyData { total_pts, total_max, rate_pts, rate_max })
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

    // Derive running total lines from session_turns
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

    ChartData {
        in_cost_bars, out_cost_bars, in_tok_bars, out_tok_bars, agent_xs,
        per_turn_in_cost_max, per_turn_out_cost_max, in_max, out_max, tool_list,
        total_cost_lines, total_cost_max, total_tok_lines, total_tok_max, session_turns,
    }
}

// ---------------------------------------------------------------------------
// Shared plot factory — all interactive behaviors off, transparent bg
// ---------------------------------------------------------------------------

fn base_plot(id: &str) -> Plot {
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
// Big dashboard layout
// ---------------------------------------------------------------------------

fn draw_big(ui: &mut egui::Ui, data: &HudData, cd: &ChartData, hidden: &mut HashSet<String>, effective_hidden: &HashSet<String>, show_active_only: &mut bool, time_axis: &mut bool, autofit: &mut bool, nav_view: &mut Option<(f64, f64)>) {
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
    let n_groups = groups.len().max(1);
    let ideal_row_h = 38.0_f32;
    let max_legend_h = h * 0.30;
    let legend_h = (ideal_row_h * n_groups as f32).min(max_legend_h).max(40.0);
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
    let tool_rect    = egui::Rect::from_min_size(egui::pos2(x0 + cost_w + tok_w + gap * 2.0, chart_y), egui::vec2(tool_w, chart_h));
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
            if ta_resp.clicked() { *time_axis = !*time_axis; }
            if ta_resp.hovered() {
                painter.rect_filled(ta_rect, 3.0, egui::Color32::from_rgba_unmultiplied(255,255,255,12));
            }
            painter.text(ta_rect.center(), egui::Align2::CENTER_CENTER,
                ta_label, egui::FontId::monospace(10.0), ta_col);

            // "autofit" button
            let af_label = if *autofit { "● fit" } else { "○ fit" };
            let af_col = if *autofit { Palette::TEXT_BRIGHT } else { Palette::TEXT_DIM };
            let af_rect = egui::Rect::from_min_size(egui::pos2(ta_rect.right() + 8.0, cy - btn_size.y / 2.0), egui::vec2(50.0, btn_size.y));
            let af_resp = ui.interact(af_rect, egui::Id::new("ctrl_autofit"), egui::Sense::click());
            if af_resp.clicked() { *autofit = !*autofit; }
            if af_resp.hovered() {
                painter.rect_filled(af_rect, 3.0, egui::Color32::from_rgba_unmultiplied(255,255,255,12));
            }
            painter.text(af_rect.center(), egui::Align2::CENTER_CENTER,
                af_label, egui::FontId::monospace(10.0), af_col);
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

    // --- legend (grouped by cwd, one row per project) ---
    // Click toggles entire group. Active sessions bright in timeline, inactive dimmed.
    let mut toggle_group: Option<Vec<String>> = None;
    let row_h = (legend_h / n_groups as f32).clamp(14.0, 36.0);
    let timeline_w = 120.0_f32;

    ui.allocate_new_ui(egui::UiBuilder::new().max_rect(legend_rect), |ui| {
        panel_frame().show(ui, |ui| {
            let inner = ui.available_rect_before_wrap();

            for (gi, (cwd, members)) in groups.iter().enumerate() {
                let group_col = session_color(members[0].0);

                // Aggregate stats
                let mut g_cost = 0.0f64;
                let mut g_in_cost = 0.0f64;
                let mut g_out_cost = 0.0f64;
                let mut g_calls = 0u32;
                let mut g_agents = 0u32;
                let mut any_active = false;
                let mut active_count = 0u32;
                let mut all_hidden = true;
                let group_ids: Vec<String> = members.iter()
                    .map(|(si, _)| data.sessions[*si].session_id.clone())
                    .collect();

                for (si, _) in members {
                    let s = &data.sessions[*si];
                    g_cost    += s.total_cost_usd;
                    g_in_cost += s.total_input_cost;
                    g_out_cost+= s.total_output_cost;
                    g_calls   += s.api_call_count;
                    g_agents  += s.agent_count;
                    if s.is_active { any_active = true; active_count += 1; }
                    if !effective_hidden.contains(&s.session_id) { all_hidden = false; }
                }

                let text_alpha = if all_hidden { 80u8 } else { 230u8 };
                let bar_col  = egui::Color32::from_rgba_unmultiplied(group_col.r(), group_col.g(), group_col.b(), text_alpha);
                let name_col = if any_active {
                    egui::Color32::from_rgba_unmultiplied(240, 230, 200, text_alpha)
                } else {
                    egui::Color32::from_rgba_unmultiplied(160, 150, 130, text_alpha)
                };
                let dim_col  = egui::Color32::from_rgba_unmultiplied(130, 120, 100, text_alpha / 2);

                let row_top = inner.top() + gi as f32 * row_h;
                let row_rect = egui::Rect::from_min_size(
                    egui::pos2(inner.left(), row_top),
                    egui::vec2(inner.width(), row_h),
                );
                let resp = ui.interact(row_rect, egui::Id::new(("legend_group", gi)), egui::Sense::click());
                if resp.clicked() { toggle_group = Some(group_ids.clone()); }
                if resp.hovered() {
                    ui.painter().rect_filled(row_rect, 2.0,
                        egui::Color32::from_rgba_unmultiplied(255, 255, 255, 10));
                }

                let painter = ui.painter();
                let bar_x     = row_rect.left() + 2.0;
                let bar_top_y = row_rect.top() + (row_h * 0.1).max(2.0);
                let bar_h_px  = row_h - (row_h * 0.2).max(4.0);
                let bar_w     = 5.0_f32;

                // Color swatch
                painter.rect_filled(
                    egui::Rect::from_min_size(egui::pos2(bar_x, bar_top_y), egui::vec2(bar_w, bar_h_px)),
                    1.5, bar_col,
                );

                // Active dot (green if any active, gray otherwise)
                if any_active {
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

                // --- Mini timeline (right side) ---
                let tl_right = row_rect.right() - 4.0;
                let tl_left  = tl_right - timeline_w;
                let tl_rect = egui::Rect::from_min_size(
                    egui::pos2(tl_left, bar_top_y),
                    egui::vec2(timeline_w, bar_h_px),
                );

                // Text area clipped before timeline
                let text_x = row_rect.left() + 18.0;
                let text_max_x = tl_left - 6.0;
                let cy     = row_rect.center().y;
                let font_name = egui::FontId::monospace((row_h * 0.35).clamp(9.0, 13.0));
                let font_stat = egui::FontId::monospace((row_h * 0.27).clamp(8.0, 10.0));

                let text_clip = egui::Rect::from_min_max(
                    egui::pos2(row_rect.left(), row_rect.top()),
                    egui::pos2(text_max_x, row_rect.bottom()),
                );
                let text_painter = ui.painter().with_clip_rect(text_clip);

                // Name + count badge + active indicator
                let name_str = if members.len() > 1 {
                    let badge = if active_count > 0 {
                        format!("{} x{} ({} active)", cwd, members.len(), active_count)
                    } else {
                        format!("{} x{}", cwd, members.len())
                    };
                    badge
                } else {
                    cwd.clone()
                };
                text_painter.text(egui::pos2(text_x, cy - row_h * 0.12), egui::Align2::LEFT_CENTER,
                    &name_str, font_name, name_col);

                if row_h >= 22.0 {
                    let cost_str = format!(
                        "{}  in {}  out {}  |  {} calls{}",
                        format_cost(g_cost),
                        format_cost(g_in_cost),
                        format_cost(g_out_cost),
                        g_calls,
                        if g_agents > 0 { format!("  {} agt", g_agents) } else { String::new() },
                    );
                    text_painter.text(egui::pos2(text_x, cy + row_h * 0.18), egui::Align2::LEFT_CENTER,
                        &cost_str, font_stat, dim_col);
                }

                // Timeline bg
                painter.rect_filled(tl_rect, 2.0,
                    egui::Color32::from_rgba_unmultiplied(group_col.r(), group_col.g(), group_col.b(), 18));

                // Per-session lanes in timeline: active sessions bright, inactive dim
                let n_members = members.len();
                let seg_h = (bar_h_px / n_members as f32).max(2.0);

                for (lane, (si, _)) in members.iter().enumerate() {
                    let s = &data.sessions[*si];
                    let seg_col = session_color(*si);
                    let seg_alpha = if effective_hidden.contains(&s.session_id) {
                        30u8
                    } else if s.is_active {
                        220u8
                    } else {
                        80u8
                    };
                    let seg_col_a = egui::Color32::from_rgba_unmultiplied(seg_col.r(), seg_col.g(), seg_col.b(), seg_alpha);

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
                        // Active sessions get a bright left-edge accent
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
        });
    });
    if let Some(ids) = toggle_group {
        // When active-only filter is on, only toggle active sessions
        let toggleable: Vec<String> = if *show_active_only {
            ids.into_iter().filter(|id| {
                data.sessions.iter().any(|s| s.session_id == *id && s.is_active)
            }).collect()
        } else {
            ids
        };
        if !toggleable.is_empty() {
            let all_hidden = toggleable.iter().all(|id| hidden.contains(id));
            if all_hidden {
                for id in toggleable { hidden.remove(&id); }
            } else {
                for id in toggleable { hidden.insert(id); }
            }
        }
    }

    let cursor_id = egui::Id::new("all_charts_cursor");
    let hover_id = egui::Id::new("hud_hover_turn");

    // Clear at frame start -- charts set it only if cursor is physically over them.
    // Store hover x as OrderedFloat wrapper so it satisfies Any + Clone + Send + Sync.
    ui.ctx().data_mut(|d| d.remove::<HoverX>(hover_id));

    // Screen-space containment check. pointer_coordinate() always returns Some regardless
    // of where the cursor is, so we convert plot-bound corners to screen space and test
    // the actual pixel rect before writing.
    let update_hover = |pui: &egui_plot::PlotUi| {
        let Some(hover_pos) = pui.ctx().input(|i| i.pointer.hover_pos()) else { return };
        let b = pui.plot_bounds();
        let s_min = pui.screen_from_plot(egui_plot::PlotPoint::new(b.min()[0], b.min()[1]));
        let s_max = pui.screen_from_plot(egui_plot::PlotPoint::new(b.max()[0], b.max()[1]));
        if !egui::Rect::from_two_pos(s_min, s_max).contains(hover_pos) { return; }
        let x = pui.plot_from_screen(hover_pos).x;
        pui.ctx().data_mut(|d| d.insert_temp(hover_id, HoverX(x)));
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
                    update_hover(pui);
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
                    update_hover(pui);
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
                    update_hover(pui);
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
                    update_hover(pui);
                });
        });
    });

    // --- floating hover tooltip (all sessions, all dims) ---
    // Read after all charts have had a chance to write hover_id this frame.
    let hover_x: Option<HoverX> = ui.ctx().data(|d| d.get_temp(hover_id));
    if let Some(HoverX(hx)) = hover_x {
        if let Some(cursor) = ui.ctx().input(|i| i.pointer.hover_pos()) {
            // Find nearest turn per session by x distance
            let mut entries: Vec<(String, String)> = vec![];
            for (name, _, turns) in &cd.session_turns {
                let nearest = turns.iter().enumerate().min_by(|(_, a), (_, b)| {
                    (a.x - hx).abs().partial_cmp(&(b.x - hx).abs()).unwrap()
                });
                if let Some((idx, t)) = nearest {
                    // Only show if within reasonable snap distance
                    let snap = if turns.len() > 1 {
                        // Half the average gap between turns
                        let span = turns.last().unwrap().x - turns.first().unwrap().x;
                        (span / turns.len() as f64).max(0.5)
                    } else { 1.0 };
                    if (t.x - hx).abs() <= snap {
                        entries.push((
                            name.clone(),
                            format!(
                                "  t{} [{}] +{}  ctx {}  gen {}  total {}",
                                idx + 1, t.model_short,
                                format_cost(t.cost_change),
                                format_cost(t.in_cost), format_cost(t.out_cost),
                                format_cost(t.total_cost),
                            ),
                        ));
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

    // --- weekly row (same x-axis as main charts, responds to nav_view) ---
    if let Some(wd) = build_weekly_data(data, hidden) {
        ui.allocate_new_ui(egui::UiBuilder::new().max_rect(weekly_total_rect), |ui| {
            panel_frame().show(ui, |ui| {
                draw_chart_label(ui, "total cost", "", "");
                let mut p = base_plot("weekly_total")
                    .link_cursor(cursor_id, true, false)
                    .include_y(0.0)
                    .include_y(wd.total_max)
                    .y_axis_formatter(move |v, _| {
                        if v.value < 1e-9 { String::new() } else { format_cost(v.value) }
                    })
                    .show_axes([true, true])
                    .show_grid(true)
                    .x_axis_formatter(time_x_fmt);
                if let Some((vmin, vmax)) = if is_time { *nav_view } else { None } {
                    p = p.include_x(vmin).include_x(vmax).auto_bounds(egui::Vec2b::new(false, true));
                }
                p.show(ui, |pui| {
                    pui.line(egui_plot::Line::new(wd.total_pts.clone())
                        .color(Palette::INPUT_TINT).width(2.0).fill(0.0));
                    update_hover(pui);
                });
            });
        });

        ui.allocate_new_ui(egui::UiBuilder::new().max_rect(weekly_rate_rect), |ui| {
            panel_frame().show(ui, |ui| {
                draw_chart_label(ui, "cost/hr", "", "");
                let mut p = base_plot("weekly_rate")
                    .link_cursor(cursor_id, true, false)
                    .include_y(0.0)
                    .include_y(wd.rate_max)
                    .y_axis_formatter(move |v, _| {
                        if v.value < 1e-9 { String::new() } else { format_cost(v.value) }
                    })
                    .show_axes([true, true])
                    .show_grid(true)
                    .x_axis_formatter(time_x_fmt);
                if let Some((vmin, vmax)) = if is_time { *nav_view } else { None } {
                    p = p.include_x(vmin).include_x(vmax).auto_bounds(egui::Vec2b::new(false, true));
                }
                p.show(ui, |pui| {
                    pui.line(egui_plot::Line::new(wd.rate_pts.clone())
                        .color(Palette::OUTPUT_TINT).width(1.5).fill(0.0));
                    update_hover(pui);
                });
            });
        });
    }
}

fn draw_chart_label(ui: &mut egui::Ui, title: &str, top_label: &str, bot_label: &str) {
    let rect = ui.available_rect_before_wrap();
    let p = ui.painter();
    p.text(egui::pos2(rect.left(), rect.top()), egui::Align2::LEFT_TOP, title, egui::FontId::monospace(10.0), Palette::TEXT_DIM);
    p.text(egui::pos2(rect.right(), rect.top()), egui::Align2::RIGHT_TOP, top_label, egui::FontId::monospace(9.0), Palette::INPUT_TINT);
    p.text(egui::pos2(rect.right(), rect.bottom()), egui::Align2::RIGHT_BOTTOM, bot_label, egui::FontId::monospace(9.0), Palette::OUTPUT_TINT);
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

                if big_mode {
                    draw_big(ui, &data, &cd, &mut self.hidden_sessions, &effective_hidden, &mut self.show_active_only, &mut self.time_axis, &mut self.autofit, &mut self.nav_view);
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

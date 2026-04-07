#![allow(dead_code)]

mod geometry;
mod anchors;
mod agent_harnesses;
mod usage;
#[path = "2_model_registry.rs"]
mod model_registry;
mod energy;
#[path = "0_scene.rs"]
mod scene;
#[path = "1_render_egui.rs"]
mod render_egui;

use std::collections::HashSet;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use egui_overlay::EguiOverlay;
use egui_overlay::egui_render_wgpu::WgpuBackend as DefaultGfxBackend;
use egui_overlay::egui_window_glfw_passthrough::GlfwBackend;
use egui_plot::{Bar, BarChart, Plot, VLine};

use geometry::PixelRect;
use agent_harnesses::claude_code::{Event, HudData, SessionData};

use scene::{ChartData, TurnInfo};

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

    start_overlay(Hud { first_frame: true, state, visible, hud_data, usage_data, big_mode, exclude_set: HashSet::new(), include_set: HashSet::new(), filter_mode: FilterMode::Exclude, show_active_only: false, show_bars: true, time_axis: false, autofit: true, nav_view: None, expanded_groups: HashSet::new(), expanded_sessions: HashSet::new(), small_mode_session: None, pre_small_window_size: None, chart_vis: ChartVisibility::default(), show_budget: false, billing: BillingConfig::load(), cached_chart: None });
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

/// Which chart sections are visible in big mode.
#[derive(Clone)]
struct ChartVisibility {
    cost: bool,
    tokens: bool,
    energy: bool,
    water: bool,
    totals: bool,
}

impl Default for ChartVisibility {
    fn default() -> Self {
        Self { cost: true, tokens: true, energy: true, water: true, totals: true }
    }
}

/// Billing period configuration for budget tracking.
#[derive(Clone)]
struct BillingConfig {
    /// Day of month the billing period resets (1-28).
    reset_day: u8,
    /// Hour of day the reset happens (0-23, default 0 = midnight).
    reset_hour: u8,
    /// Budget limit in USD for the period.
    limit_usd: f64,
    /// User-reported total from web dashboard: (amount_usd, epoch_secs).
    /// Entered manually since the web API is no longer scrapable.
    web_reported: Option<(f64, u64)>,
    /// Text buffer for web reported $ input
    web_input_buf: String,
    /// Text buffer for limit $ input
    limit_input_buf: String,
}

impl Default for BillingConfig {
    fn default() -> Self {
        Self { reset_day: 1, reset_hour: 0, limit_usd: 100.0, web_reported: None, web_input_buf: String::new(), limit_input_buf: "100".into() }
    }
}

impl BillingConfig {
    fn config_path() -> std::path::PathBuf {
        let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".into());
        let mut p = std::path::PathBuf::from(home);
        p.push(".config");
        p.push("cc-hud");
        p.push("billing.json");
        p
    }

    fn save(&self) {
        let path = Self::config_path();
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let json = serde_json::json!({
            "reset_day": self.reset_day,
            "reset_hour": self.reset_hour,
            "limit_usd": self.limit_usd,
            "web_reported": self.web_reported,
        });
        let _ = std::fs::write(&path, serde_json::to_string_pretty(&json).unwrap_or_default());
    }

    fn load() -> Self {
        let path = Self::config_path();
        let mut cfg = BillingConfig::default();
        if let Ok(data) = std::fs::read_to_string(&path) {
            if let Ok(v) = serde_json::from_str::<serde_json::Value>(&data) {
                if let Some(d) = v["reset_day"].as_u64() { cfg.reset_day = d.min(28).max(1) as u8; }
                if let Some(h) = v["reset_hour"].as_u64() { cfg.reset_hour = h.min(23) as u8; }
                if let Some(l) = v["limit_usd"].as_f64() { cfg.limit_usd = l.max(1.0); cfg.limit_input_buf = format!("{:.0}", l); }
                if let Some(arr) = v["web_reported"].as_array() {
                    if arr.len() == 2 {
                        if let (Some(val), Some(ts)) = (arr[0].as_f64(), arr[1].as_u64()) {
                            cfg.web_reported = Some((val, ts));
                        }
                    }
                }
            }
        }
        cfg
    }
}

impl BillingConfig {
    /// Compute the start of the current billing period as epoch seconds.
    fn period_start_epoch(&self) -> u64 {
        let now_secs = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH).unwrap().as_secs() as libc::time_t;
        let mut tm: libc::tm = unsafe { std::mem::zeroed() };
        unsafe { libc::localtime_r(&now_secs, &mut tm); }

        // Set to reset_day at reset_hour:00:00
        tm.tm_mday = self.reset_day as i32;
        tm.tm_hour = self.reset_hour as i32;
        tm.tm_min = 0;
        tm.tm_sec = 0;
        tm.tm_isdst = -1; // let mktime figure it out

        let candidate = unsafe { libc::mktime(&mut tm) };
        // If candidate is in the future, go back one month
        if candidate > now_secs {
            tm.tm_mon -= 1;
            if tm.tm_mon < 0 { tm.tm_mon = 11; tm.tm_year -= 1; }
            tm.tm_isdst = -1;
            let prev = unsafe { libc::mktime(&mut tm) };
            prev as u64
        } else {
            candidate as u64
        }
    }

    /// Period start as x-coordinate in minutes-from-epoch (matching chart x-axis).
    fn period_start_x(&self) -> f64 {
        self.period_start_epoch() as f64 / 60.0
    }
}

#[derive(Clone, Copy, PartialEq)]
enum FilterMode { Include, Exclude }

struct Hud {
    first_frame: bool,
    state: Arc<Mutex<PixelRect>>,
    visible: Arc<std::sync::atomic::AtomicBool>,
    hud_data: Arc<Mutex<HudData>>,
    usage_data: Arc<Mutex<usage::UsageData>>,
    big_mode: bool,
    /// Sessions to hide when in Exclude mode.
    exclude_set: HashSet<String>,
    /// Sessions to show when in Include mode.
    include_set: HashSet<String>,
    filter_mode: FilterMode,
    show_active_only: bool,
    show_bars: bool,
    time_axis: bool,
    autofit: bool,
    /// Chart viewport x-range in minutes-from-epoch. None = auto-fit to all data.
    nav_view: Option<(f64, f64)>,
    /// Which cwd groups have their session list expanded.
    expanded_groups: HashSet<String>,
    /// Which sessions have their subagent tree expanded.
    expanded_sessions: HashSet<String>,
    /// When Some, show small mode for this session id
    small_mode_session: Option<String>,
    /// Saved window size from before entering small mode
    pre_small_window_size: Option<(i32, i32)>,
    chart_vis: ChartVisibility,
    /// When true, usage chart slot shows billing period budget instead.
    show_budget: bool,
    billing: BillingConfig,
    /// Cached chart data to avoid rebuilding every frame.
    cached_chart: Option<(u64, HashSet<String>, bool, scene::ChartData)>,
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
    const SKILL_MARKER: egui::Color32 = egui::Color32::from_rgba_premultiplied(60, 180, 120, 60);
    const INPUT_TINT: egui::Color32 = egui::Color32::from_rgba_premultiplied(100, 160, 220, 180);
    const OUTPUT_TINT: egui::Color32 = egui::Color32::from_rgba_premultiplied(220, 160, 60, 180);
    const TOOL_BAR: egui::Color32 = egui::Color32::from_rgba_premultiplied(71, 77, 88, 160);
    const SEPARATOR: egui::Color32 = egui::Color32::from_rgba_premultiplied(60, 55, 42, 120);
}

/// Which chart region the hover originated from.
#[derive(Clone, Copy, PartialEq, Eq)]
enum HoverSource { Cost, Tokens, TotalCost, TotalTokens, WeeklyCost, WeeklyRate, Energy, Water }

/// Session time ranges highlighted from legend hover (stored as minutes-from-epoch).
#[derive(Clone, Default)]
struct LegendHighlight {
    /// (first_ts_min, last_ts_min) for each hovered session
    ranges: Vec<(f64, f64)>,
}

/// Wrapper for storing hover state in egui temp storage.
#[derive(Clone, Copy)]
struct HoverState { x: f64, source: HoverSource }

/// Which panel row is hovered, used to highlight corresponding vlines on charts.
#[derive(Clone, Default)]
struct PanelHighlight {
    /// "skill:<name>" or "agent:<type>" -- empty means nothing highlighted
    key: String,
}

fn scene_to_egui(c: scene::Color) -> egui::Color32 {
    egui::Color32::from_rgba_unmultiplied(c.0, c.1, c.2, c.3)
}

fn bars_to_egui(bars: &[scene::BarData]) -> Vec<Bar> {
    bars.iter().map(|b| Bar::new(b.x, b.height).width(b.width).fill(scene_to_egui(b.color))).collect()
}

fn bars_to_egui_hl(bars: &[scene::BarData], hl: &[bool]) -> Vec<Bar> {
    bars.iter().map(|b| {
        let col = if hl.is_empty() || hl.get(b.session_idx).copied().unwrap_or(true) {
            scene_to_egui(b.color)
        } else {
            scene_to_egui(b.color).gamma_multiply(0.12)
        };
        Bar::new(b.x, b.height).width(b.width).fill(col)
    }).collect()
}

use scene::{format_cost, format_tokens, session_color};

/// Convert a cumulative line series to a step function.
/// Each point stays flat at the previous value until the next x, then steps up.
/// This correctly represents discrete events (turns) rather than continuous accumulation.
fn step_pts(pts: &[[f64; 2]]) -> Vec<[f64; 2]> {
    if pts.len() < 2 { return pts.to_vec(); }
    let mut out = Vec::with_capacity(pts.len() * 2 - 1);
    out.push(pts[0]);
    for w in pts.windows(2) {
        out.push([w[1][0], w[0][1]]); // horizontal at prev y up to next x
        out.push(w[1]);               // vertical step up
    }
    out
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

use scene::build_chart_data;



// ---------------------------------------------------------------------------
// Grid spacer for token axes: picks 1-2-5 steps (e.g. 1k, 2k, 5k, 10k, 50k)
// ---------------------------------------------------------------------------

fn token_grid_spacer(input: egui_plot::GridInput) -> Vec<egui_plot::GridMark> {
    let range = (input.bounds.1 - input.bounds.0).abs();
    if range < 1.0 { return vec![]; }

    // Target ~4-6 grid lines in the visible range
    let raw_step = range / 5.0;

    // Round to nearest 1-2-5 sequence
    let mag = 10.0_f64.powf(raw_step.log10().floor());
    let norm = raw_step / mag;
    let step = if norm <= 1.5 { mag } else if norm <= 3.5 { 2.0 * mag } else if norm <= 7.5 { 5.0 * mag } else { 10.0 * mag };

    // Sub-steps for thinner lines (half and fifth of main step)
    let sub_step = step / 2.0;

    let lo = input.bounds.0.min(input.bounds.1);
    let hi = input.bounds.0.max(input.bounds.1);

    let mut marks = vec![];
    // Start from a multiple of step below lo
    let start = (lo / sub_step).floor() as i64;
    let end = (hi / sub_step).ceil() as i64;
    for i in start..=end {
        let value = i as f64 * sub_step;
        if value < lo - sub_step || value > hi + sub_step { continue; }
        let is_major = (value / step).round() * step == value || (value - (value / step).round() * step).abs() < step * 0.01;
        marks.push(egui_plot::GridMark {
            value,
            step_size: if is_major { step } else { sub_step },
        });
    }
    marks
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

/// Handle scroll/drag on an egui_plot chart, updating the shared nav_view.
/// Also handles click-to-pin: clicking stores the hover x in pinned_x_id temp storage.
/// Call this after p.show() when is_time is true.
fn handle_chart_nav(
    ctx: &egui::Context,
    resp: &egui::Response,
    bounds: &egui_plot::PlotBounds,
    nav_view: &mut Option<(f64, f64)>,
    full_min: f64,
    full_max: f64,
    autofit: &mut bool,
) {
    let x_min = bounds.min()[0];
    let x_max = bounds.max()[0];
    let x_span = (x_max - x_min).max(1e-10);
    let rect_w = resp.rect.width().max(1.0) as f64;
    let full_span = full_max - full_min;

    if resp.dragged() {
        *autofit = false;
        let dx_time = resp.drag_delta().x as f64 * x_span / rect_w;
        let (mut vmin, mut vmax) = nav_view.unwrap_or((full_min, full_max));
        let vspan = vmax - vmin;
        vmin -= dx_time;
        vmax -= dx_time;
        if vmin < full_min { vmin = full_min; vmax = vmin + vspan; }
        if vmax > full_max { vmax = full_max; vmin = vmax - vspan; }
        *nav_view = Some((vmin, vmax));
    }

    // Vertical scroll = zoom anchored at cursor
    let scroll_y = ctx.input(|i| i.smooth_scroll_delta.y);
    if resp.hovered() && scroll_y.abs() > 0.1 {
        *autofit = false;
        let zoom_factor = 1.0 - (scroll_y as f64 * 0.003);
        let (vmin, vmax) = nav_view.unwrap_or((full_min, full_max));
        let vspan = vmax - vmin;
        let anchor = ctx.input(|i| i.pointer.hover_pos())
            .map(|p| {
                let frac = ((p.x - resp.rect.left()) / resp.rect.width()).clamp(0.0, 1.0) as f64;
                x_min + frac * x_span
            })
            .unwrap_or((x_min + x_max) / 2.0);
        let t = ((anchor - vmin) / vspan).clamp(0.0, 1.0);
        let new_span = (vspan * zoom_factor).clamp(1.0, full_span);
        let new_min = (anchor - t * new_span).max(full_min);
        let new_max = (new_min + new_span).min(full_max);
        let new_min = (new_max - new_span).max(full_min);
        *nav_view = Some((new_min, new_max));
    }

    // Horizontal scroll = pan
    let scroll_x = ctx.input(|i| i.smooth_scroll_delta.x);
    if resp.hovered() && scroll_x.abs() > 0.1 {
        *autofit = false;
        let dx_time = scroll_x as f64 * x_span / rect_w;
        let (mut vmin, mut vmax) = nav_view.unwrap_or((full_min, full_max));
        let vspan = vmax - vmin;
        vmin -= dx_time;
        vmax -= dx_time;
        if vmin < full_min { vmin = full_min; vmax = vmin + vspan; }
        if vmax > full_max { vmax = full_max; vmin = vmax - vspan; }
        *nav_view = Some((vmin, vmax));
    }
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
// Subagent tree rendering helper
// ---------------------------------------------------------------------------

/// Format duration between two epoch-second timestamps as compact string.
fn format_duration_secs(first: u64, last: u64) -> String {
    if first == 0 || last == 0 || last < first { return String::new(); }
    let secs = last - first;
    if secs < 60 { format!("{}s", secs) }
    else if secs < 3600 { format!("{}m", secs / 60) }
    else { format!("{:.1}h", secs as f64 / 3600.0) }
}

/// Render subagent toggle inline on parent row + expanded tree rows below.
fn draw_subagent_tree(
    ui: &egui::Ui,
    inner: egui::Rect,
    row_h: f32,
    row_gap: f32,
    timeline_w: f32,
    row_idx: &mut usize,
    parent_rect: egui::Rect,
    indent: f32,
    session: &agent_harnesses::claude_code::SessionData,
    is_expanded: bool,
    toggle_out: &mut Vec<String>,
    gi: usize,
    si: usize,
) {
    if session.subagents.is_empty() { return; }

    // Inline toggle: right side of parent row, left of timeline area
    let arrow = if is_expanded { "\u{25be}" } else { "\u{25b8}" };
    let agent_cost_sum: f64 = session.subagents.iter().map(|a| a.total_cost_usd).sum();
    let tog_label = format!("{} {}ag {}", arrow, session.subagents.len(), format_cost(agent_cost_sum));

    let tog_w = 110.0_f32;
    let tog_rect = egui::Rect::from_min_size(
        egui::pos2(parent_rect.right() - timeline_w - tog_w - 44.0, parent_rect.top()),
        egui::vec2(tog_w, row_h),
    );
    let tog_resp = ui.interact(tog_rect, egui::Id::new(("agent_toggle", gi, si)), egui::Sense::click());
    if tog_resp.clicked() {
        toggle_out.push(session.session_id.clone());
    }

    if tog_resp.hovered() {
        ui.painter().rect_filled(tog_rect, 2.0,
            egui::Color32::from_rgba_unmultiplied(255, 255, 255, 10));
    }

    let tog_col = if is_expanded {
        Palette::TEXT_BRIGHT
    } else if tog_resp.hovered() {
        egui::Color32::from_rgba_unmultiplied(200, 195, 180, 200)
    } else {
        Palette::TEXT_DIM
    };
    ui.painter().text(
        egui::pos2(tog_rect.left() + 2.0, tog_rect.center().y),
        egui::Align2::LEFT_CENTER, &tog_label,
        egui::FontId::monospace(10.0), tog_col,
    );

    if !is_expanded { return; }

    let tree_x = parent_rect.left() + indent + 6.0;
    let text_x = tree_x + 14.0;
    let tree_col = egui::Color32::from_rgba_unmultiplied(90, 85, 75, 140);
    let name_col = egui::Color32::from_rgba_unmultiplied(200, 195, 180, 200);
    let stat_col = egui::Color32::from_rgba_unmultiplied(140, 135, 120, 170);

    for (ai, agent) in session.subagents.iter().enumerate() {
        let a_top = inner.top() + *row_idx as f32 * (row_h + row_gap);
        let a_rect = egui::Rect::from_min_size(
            egui::pos2(inner.left(), a_top),
            egui::vec2(inner.width(), row_h),
        );

        // Background
        ui.painter().rect_filled(a_rect, 2.0,
            egui::Color32::from_rgba_unmultiplied(255, 255, 255, 3));

        // Tree connector
        let is_last = ai == session.subagents.len() - 1;
        let connector = if is_last { "\u{2514}" } else { "\u{251c}" };
        ui.painter().text(
            egui::pos2(tree_x, a_rect.center().y),
            egui::Align2::CENTER_CENTER, connector,
            egui::FontId::monospace(12.0), tree_col,
        );

        // Vertical tree line for non-last items
        if !is_last {
            ui.painter().line_segment(
                [egui::pos2(tree_x, a_rect.bottom()), egui::pos2(tree_x, a_rect.bottom() + row_gap)],
                egui::Stroke::new(1.0, tree_col),
            );
        }

        // Agent type badge
        let type_short = match agent.agent_type.as_str() {
            "general-purpose" => "agent",
            "Explore" => "explore",
            "Plan" => "plan",
            other => other,
        };

        // Description - allow more chars since text is bigger
        let desc_trunc = if agent.description.len() > 40 {
            format!("{}...", &agent.description[..37])
        } else {
            agent.description.clone()
        };

        // Name line: type + description
        let name_font = egui::FontId::monospace(11.0);
        let agent_label = format!("{} {}", type_short, desc_trunc);

        // Clip text to avoid overlap with right-side stats
        let text_clip = egui::Rect::from_min_max(
            egui::pos2(text_x, a_rect.top()),
            egui::pos2(a_rect.right() - timeline_w - 8.0, a_rect.bottom()),
        );
        let clip_painter = ui.painter().with_clip_rect(text_clip);
        clip_painter.text(
            egui::pos2(text_x, a_rect.center().y - row_h * 0.12),
            egui::Align2::LEFT_CENTER, &agent_label,
            name_font, name_col,
        );

        // Stats line: model + cost + duration
        let model_short = scene::short_model_label(&agent.model);
        let duration = format_duration_secs(agent.first_ts, agent.last_ts);
        let total_tok = format_tokens(agent.total_input + agent.total_output);
        let stat_str = format!("{}  {}  {} tok  {}  {}calls", model_short, format_cost(agent.total_cost_usd), total_tok, duration, agent.api_call_count);
        let stat_font = egui::FontId::monospace(9.0);
        clip_painter.text(
            egui::pos2(text_x, a_rect.center().y + row_h * 0.18),
            egui::Align2::LEFT_CENTER, &stat_str,
            stat_font, stat_col,
        );

        *row_idx += 1;
    }
}

// ---------------------------------------------------------------------------
// Legend row drawing helper
// ---------------------------------------------------------------------------

/// Stats for legend row display.
struct LegendStats {
    cost: f64,
    last_input: u64,
    total_tokens: u64,      // input + output across entire session/group
    session_count: u32,     // 1 for flat rows, N for group headers
    api_call_count: u32,
}

impl LegendStats {
    fn cost_per_token(&self) -> f64 {
        if self.total_tokens > 0 { self.cost / self.total_tokens as f64 * 1_000_000.0 } else { 0.0 }
    }
    fn avg_tokens_per_session(&self) -> u64 {
        if self.session_count > 0 { self.total_tokens / self.session_count as u64 } else { 0 }
    }
    fn avg_cost_per_session(&self) -> f64 {
        if self.session_count > 0 { self.cost / self.session_count as f64 } else { 0.0 }
    }
}

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
    stats: &LegendStats,
    model: &str,
    // Some((active_cost, group_total_cost)) for active groups, None otherwise
    _active_group_costs: Option<(f64, f64)>,
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
        let ctx_frac = (stats.last_input as f32 / 200_000.0).clamp(0.02, 1.0);
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

    // Stats (secondary line) -- monospace fixed-width fields for column alignment
    if row_h >= 22.0 {
        let sy = cy + row_h * 0.18;
        let stat_str = if stats.session_count > 1 {
            let avg_cost = format_cost(stats.avg_cost_per_session());
            let avg_tok = format_tokens(stats.avg_tokens_per_session());
            if is_active {
                let ctx_pct = (stats.last_input as f64 / 200_000.0 * 100.0).min(999.0);
                format!("{:>3.0}% ctx  {:>8}  {:>6}  avg {}/sesh  {}/sesh",
                    ctx_pct, format_cost(stats.cost), format_tokens(stats.total_tokens), avg_cost, avg_tok)
            } else {
                format!("{:>8}  {:>6}  avg {}/sesh  {}/sesh",
                    format_cost(stats.cost), format_tokens(stats.total_tokens), avg_cost, avg_tok)
            }
        } else if is_active {
            let model_tag = if model.is_empty() { "" } else { scene::short_model_label(model) };
            let ctx_pct = (stats.last_input as f64 / 200_000.0 * 100.0).min(999.0);
            format!("{:>3.0}% ctx  {:>8}  {:>6}  {}",
                ctx_pct, format_cost(stats.cost), format_tokens(stats.total_tokens), model_tag)
        } else {
            let model_tag = if model.is_empty() { "" } else { scene::short_model_label(model) };
            format!("{:>8}  {:>6}  {}",
                format_cost(stats.cost), format_tokens(stats.total_tokens), model_tag)
        };
        text_painter.text(egui::pos2(text_x, sy), egui::Align2::LEFT_CENTER,
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

fn draw_small(ui: &mut egui::Ui, data: &HudData, _cd: &ChartData, usage: &usage::UsageData, session_id: &str, _filter_set: &HashSet<String>, _filter_mode: &mut FilterMode, time_axis: &mut bool, autofit: &mut bool, nav_view: &mut Option<(f64, f64)>, small_mode_session: &mut Option<String>) {
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

            // "time" toggle
            let ta_label = if *time_axis { "● time" } else { "○ time" };
            let ta_col = if *time_axis { Palette::TEXT_BRIGHT } else { Palette::TEXT_DIM };
            let ta_rect = egui::Rect::from_min_size(egui::pos2(btn_rect.right() + 4.0, cy - btn_h / 2.0), egui::vec2(46.0, btn_h));
            let ta_resp = ui.interact(ta_rect, egui::Id::new("small_time_axis"), egui::Sense::click());
            if ta_resp.clicked() {
                *time_axis = !*time_axis;
                *autofit = true; *nav_view = None;
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
                if fit_resp.clicked() { *autofit = !*autofit; }
                if *autofit || fit_resp.hovered() {
                    painter.rect_filled(fit_rect, 3.0, egui::Color32::from_rgba_unmultiplied(255,255,255,12));
                }
                let fit_col = if *autofit { Palette::TEXT_BRIGHT } else { Palette::TEXT_DIM };
                painter.text(fit_rect.center(), egui::Align2::CENTER_CENTER,
                    "fit", egui::FontId::monospace(8.0), fit_col);
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

    // Autofit: recompute each frame while active (tracks latest data)
    if *autofit {
        if session.first_ts > 0 {
            let fit_min = session.first_ts as f64 / 60.0;
            let fit_max = session.last_ts as f64 / 60.0;
            let span = (fit_max - fit_min).max(1.0);
            *nav_view = Some((fit_min - span * 0.02, fit_max + span * 0.02));
        }
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
                    *autofit = false;
                    let dx_px = resp.drag_delta().x;
                    let dx_min = (dx_px / bar_w) as f64 * full_span;
                    let (mut vmin, mut vmax) = nav_view.unwrap_or((full_min, full_max));
                    let vspan = vmax - vmin;
                    vmin += dx_min; vmax += dx_min;
                    if vmin < full_min { vmin = full_min; vmax = vmin + vspan; }
                    if vmax > full_max { vmax = full_max; vmin = vmax - vspan; }
                    *nav_view = Some((vmin, vmax));
                }

                // Vertical scroll = zoom
                let scroll_y = ui.input(|i| i.smooth_scroll_delta.y);
                if resp.hovered() && scroll_y.abs() > 0.1 {
                    *autofit = false;
                    let zoom_factor = 1.0 - (scroll_y as f64 * 0.003);
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

                // Horizontal scroll = pan
                let scroll_x = ui.input(|i| i.smooth_scroll_delta.x);
                if resp.hovered() && scroll_x.abs() > 0.1 {
                    *autofit = false;
                    let dx_min = -(scroll_x as f64 / bar_w as f64) * full_span;
                    let (mut vmin, mut vmax) = nav_view.unwrap_or((full_min, full_max));
                    let vspan = vmax - vmin;
                    vmin += dx_min; vmax += dx_min;
                    if vmin < full_min { vmin = full_min; vmax = vmin + vspan; }
                    if vmax > full_max { vmax = full_max; vmin = vmax - vspan; }
                    *nav_view = Some((vmin, vmax));
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
                let no_hidden = HashSet::new();
                draw_legend_row(ui, inner, legend_h_full, timeline_w, week_start_secs, week_span,
                    &session.project, sess_col, name_col, dim_col,
                    session.is_active, false,
                    &LegendStats { cost: session.total_cost_usd, last_input: session.last_input_tokens,
                        total_tokens: session.total_input + session.total_output, session_count: 1, api_call_count: session.api_call_count },
                    &session.model, None,
                    &[(session, sess_col)], &no_hidden, Some(eye_w));
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
            if is_time { handle_chart_nav(ui.ctx(), &plot_resp.response, plot_resp.transform.bounds(), nav_view, full_min, full_max, autofit); }

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
            if is_time { handle_chart_nav(ui.ctx(), &plot_resp.response, plot_resp.transform.bounds(), nav_view, full_min, full_max, autofit); }

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

fn draw_big(ui: &mut egui::Ui, data: &HudData, cd: &ChartData, usage: &usage::UsageData, filter_set: &mut HashSet<String>, filter_mode: &mut FilterMode, show_active_only: &mut bool, show_bars: &mut bool, effective_hidden: &HashSet<String>, time_axis: &mut bool, autofit: &mut bool, nav_view: &mut Option<(f64, f64)>, expanded_groups: &mut HashSet<String>, expanded_sessions: &mut HashSet<String>, small_mode_session: &mut Option<String>, chart_vis: &mut ChartVisibility, show_budget: &mut bool, billing: &mut BillingConfig) {
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

    // Dynamic layout: hidden sections give their space to the main chart row
    let show_cost_tok = chart_vis.cost || chart_vis.tokens;
    let show_energy_water = chart_vis.energy || chart_vis.water;
    let show_totals = chart_vis.totals;

    let base_energy_h = (h * 0.14).clamp(60.0, 130.0);
    let base_weekly_h = (h * 0.12).max(45.0);
    let energy_row_h = if show_energy_water { base_energy_h } else { 0.0 };
    let weekly_h = if show_totals { base_weekly_h } else { 0.0 };
    let energy_gap = if show_energy_water { gap } else { 0.0 };
    let weekly_gap = if show_totals { gap } else { 0.0 };

    let fixed_overhead = controls_h + gap + nav_h + gap + legend_h + energy_row_h + energy_gap + weekly_h + weekly_gap;
    let chart_h = if show_cost_tok { (h - fixed_overhead - gap).max(40.0) } else { 0.0 };
    let chart_gap = if show_cost_tok { gap } else { 0.0 };

    let legend_rect = egui::Rect::from_min_size(egui::pos2(x0, after_nav_y), egui::vec2(w, legend_h));

    let cost_w  = w * 0.50;
    let tok_w   = w * 0.26;
    let tool_w  = w - cost_w - tok_w - gap * 2.0;
    let chart_y = after_nav_y + legend_h + chart_gap;

    let cost_rect     = egui::Rect::from_min_size(egui::pos2(x0, chart_y), egui::vec2(cost_w, chart_h));
    let tok_rect     = egui::Rect::from_min_size(egui::pos2(x0 + cost_w + gap, chart_y), egui::vec2(tok_w, chart_h));
    let usage_chart_h = (chart_h * 0.45).floor();
    let tool_h = chart_h - usage_chart_h - gap;
    let right_x = x0 + cost_w + tok_w + gap * 2.0;
    let usage_rect   = egui::Rect::from_min_size(egui::pos2(right_x, chart_y), egui::vec2(tool_w, usage_chart_h));
    let tool_rect    = egui::Rect::from_min_size(egui::pos2(right_x, chart_y + usage_chart_h + gap), egui::vec2(tool_w, tool_h));
    // Row 3: per-turn energy (Wh) + per-turn water (mL), each with cumulative overlay
    let energy_y = chart_y + chart_h + energy_gap;
    let energy_half = (w - gap) / 2.0;
    let energy_wh_rect = egui::Rect::from_min_size(egui::pos2(x0, energy_y), egui::vec2(energy_half, energy_row_h));
    let water_ml_rect = egui::Rect::from_min_size(egui::pos2(x0 + energy_half + gap, energy_y), egui::vec2(energy_half, energy_row_h));

    let totals_y = energy_y + energy_row_h + weekly_gap;
    let totals_rect = egui::Rect::from_min_size(egui::pos2(x0, totals_y), egui::vec2(w, weekly_h));

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

            // Include/Exclude mode toggle + clear button
            let mode_label = match *filter_mode {
                FilterMode::Include => "include",
                FilterMode::Exclude => "exclude",
            };
            let mode_col = if filter_set.is_empty() { Palette::TEXT_DIM } else { Palette::TEXT_BRIGHT };
            let btn_size = egui::vec2(70.0, controls_h - 6.0);
            let btn_rect = egui::Rect::from_min_size(egui::pos2(inner.left() + 2.0, cy - btn_size.y / 2.0), btn_size);
            let btn_resp = ui.interact(btn_rect, egui::Id::new("ctrl_filter_mode"), egui::Sense::click());
            if btn_resp.clicked() {
                *filter_mode = match *filter_mode {
                    FilterMode::Include => FilterMode::Exclude,
                    FilterMode::Exclude => FilterMode::Include,
                };
                // Don't clear -- each mode has its own independent set
            }
            if btn_resp.hovered() {
                painter.rect_filled(btn_rect, 3.0, egui::Color32::from_rgba_unmultiplied(255,255,255,12));
            }
            painter.text(btn_rect.center(), egui::Align2::CENTER_CENTER,
                mode_label, egui::FontId::monospace(10.0), mode_col);
            // Clear filter set button
            if !filter_set.is_empty() {
                let clr_rect = egui::Rect::from_min_size(egui::pos2(btn_rect.right() + 4.0, cy - btn_size.y / 2.0), egui::vec2(20.0, btn_size.y));
                let clr_resp = ui.interact(clr_rect, egui::Id::new("ctrl_filter_clear"), egui::Sense::click());
                if clr_resp.clicked() { filter_set.clear(); }
                if clr_resp.hovered() {
                    painter.rect_filled(clr_rect, 3.0, egui::Color32::from_rgba_unmultiplied(255,255,255,12));
                }
                painter.text(clr_rect.center(), egui::Align2::CENTER_CENTER,
                    "x", egui::FontId::monospace(10.0), Palette::TEXT_DIM);
            }

            // Next button x-cursor (advances after each button)
            let mut bx = btn_rect.right() + if filter_set.is_empty() { 8.0 } else { 30.0 };

            // "active" toggle
            let ao_label = if *show_active_only { "● active" } else { "○ active" };
            let ao_col = if *show_active_only { Palette::TEXT_BRIGHT } else { Palette::TEXT_DIM };
            let ao_rect = egui::Rect::from_min_size(egui::pos2(bx, cy - btn_size.y / 2.0), egui::vec2(60.0, btn_size.y));
            let ao_resp = ui.interact(ao_rect, egui::Id::new("ctrl_active_only"), egui::Sense::click());
            if ao_resp.clicked() { *show_active_only = !*show_active_only; }
            if ao_resp.hovered() { painter.rect_filled(ao_rect, 3.0, egui::Color32::from_rgba_unmultiplied(255,255,255,12)); }
            painter.text(ao_rect.center(), egui::Align2::CENTER_CENTER, ao_label, egui::FontId::monospace(10.0), ao_col);
            bx = ao_rect.right() + 4.0;

            // "bars" toggle -- hide per-turn bars, show only cumulative lines
            let bars_label = if *show_bars { "● bars" } else { "○ bars" };
            let bars_col = if *show_bars { Palette::TEXT_BRIGHT } else { Palette::TEXT_DIM };
            let bars_rect = egui::Rect::from_min_size(egui::pos2(bx, cy - btn_size.y / 2.0), egui::vec2(50.0, btn_size.y));
            let bars_resp = ui.interact(bars_rect, egui::Id::new("ctrl_show_bars"), egui::Sense::click());
            if bars_resp.clicked() { *show_bars = !*show_bars; }
            if bars_resp.hovered() { painter.rect_filled(bars_rect, 3.0, egui::Color32::from_rgba_unmultiplied(255,255,255,12)); }
            painter.text(bars_rect.center(), egui::Align2::CENTER_CENTER, bars_label, egui::FontId::monospace(10.0), bars_col);
            bx = bars_rect.right() + 4.0;

            // "budget" toggle -- swap usage chart with billing period budget chart
            let bdg_label = if *show_budget { "● budget" } else { "○ budget" };
            let bdg_col = if *show_budget { Palette::TEXT_BRIGHT } else { Palette::TEXT_DIM };
            let bdg_rect = egui::Rect::from_min_size(egui::pos2(bx, cy - btn_size.y / 2.0), egui::vec2(60.0, btn_size.y));
            let bdg_resp = ui.interact(bdg_rect, egui::Id::new("ctrl_show_budget"), egui::Sense::click());
            if bdg_resp.clicked() { *show_budget = !*show_budget; }
            if bdg_resp.hovered() { painter.rect_filled(bdg_rect, 3.0, egui::Color32::from_rgba_unmultiplied(255,255,255,12)); }
            painter.text(bdg_rect.center(), egui::Align2::CENTER_CENTER, bdg_label, egui::FontId::monospace(10.0), bdg_col);
            bx = bdg_rect.right() + 4.0;

            // "time axis" toggle button
            let ta_label = if *time_axis { "● time" } else { "○ time" };
            let ta_col = if *time_axis { Palette::TEXT_BRIGHT } else { Palette::TEXT_DIM };
            let ta_rect = egui::Rect::from_min_size(egui::pos2(bx, cy - btn_size.y / 2.0), egui::vec2(60.0, btn_size.y));
            let ta_resp = ui.interact(ta_rect, egui::Id::new("ctrl_time_axis"), egui::Sense::click());
            if ta_resp.clicked() {
                *time_axis = !*time_axis;
                if *time_axis { *show_bars = false; }
                *autofit = true;
                *nav_view = None;
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
                if af_resp.clicked() { *autofit = !*autofit; }
                if *autofit || af_resp.hovered() {
                    painter.rect_filled(af_rect, 3.0, egui::Color32::from_rgba_unmultiplied(255,255,255,12));
                }
                let fit_col = if *autofit { Palette::TEXT_BRIGHT } else { Palette::TEXT_DIM };
                painter.text(af_rect.center(), egui::Align2::CENTER_CENTER,
                    "fit", egui::FontId::monospace(10.0), fit_col);
            }

            // Chart section visibility toggles (right-aligned)
            let vis_toggles: &[(&str, fn(&ChartVisibility) -> bool, fn(&mut ChartVisibility))] = &[
                ("$", |v| v.cost, |v| v.cost = !v.cost),
                ("tok", |v| v.tokens, |v| v.tokens = !v.tokens),
                ("nrg", |v| v.energy, |v| v.energy = !v.energy),
                ("H₂O", |v| v.water, |v| v.water = !v.water),
                ("Σ", |v| v.totals, |v| v.totals = !v.totals),
            ];
            let vtog_w = 32.0_f32;
            let vtog_gap = 4.0_f32;
            let vtog_total_w = vis_toggles.len() as f32 * (vtog_w + vtog_gap) - vtog_gap;
            let mut vtog_x = inner.right() - vtog_total_w - 4.0;
            for (label, getter, toggler) in vis_toggles {
                let on = getter(chart_vis);
                let vrect = egui::Rect::from_min_size(egui::pos2(vtog_x, cy - btn_size.y / 2.0), egui::vec2(vtog_w, btn_size.y));
                let vresp = ui.interact(vrect, egui::Id::new(("vis_toggle", *label)), egui::Sense::click());
                if vresp.clicked() { toggler(chart_vis); }
                if vresp.hovered() {
                    painter.rect_filled(vrect, 3.0, egui::Color32::from_rgba_unmultiplied(255,255,255,12));
                }
                let vcol = if on { Palette::TEXT_BRIGHT } else { egui::Color32::from_rgba_unmultiplied(100, 90, 75, 120) };
                painter.text(vrect.center(), egui::Align2::CENTER_CENTER,
                    *label, egui::FontId::monospace(9.0), vcol);
                vtog_x += vtog_w + vtog_gap;
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
        let col = scene_to_egui(session_color(si));
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

    // Autofit: recompute each frame while active (tracks latest data)
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
                *autofit = false;
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

            // Vertical scroll = zoom anchored to mouse position
            let scroll_y = ui.input(|i| i.smooth_scroll_delta.y);
            if resp.hovered() && scroll_y.abs() > 0.1 {
                *autofit = false;
                let zoom_factor = 1.0 - (scroll_y as f64 * 0.003);
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

            // Horizontal scroll = pan
            let scroll_x = ui.input(|i| i.smooth_scroll_delta.x);
            if resp.hovered() && scroll_x.abs() > 0.1 {
                *autofit = false;
                let dx_min = -(scroll_x as f64 / bar_w as f64) * full_span;
                let (mut vmin, mut vmax) = nav_view.unwrap_or((full_min, full_max));
                let vspan = vmax - vmin;
                vmin += dx_min; vmax += dx_min;
                if vmin < full_min { vmin = full_min; vmax = vmin + vspan; }
                if vmax > full_max { vmax = full_max; vmin = vmax - vspan; }
                *nav_view = Some((vmin, vmax));
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
    let mut toggle_session_agents: Vec<String> = vec![];
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
                let group_col = scene_to_egui(session_color(members[0].0));
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
                let mut g_total_tokens = 0u64;
                let mut g_api_calls = 0u32;

                for (si, _) in members {
                    let s = &data.sessions[*si];
                    g_cost    += s.total_cost_usd;
                    g_total_tokens += s.total_input + s.total_output;
                    g_api_calls += s.api_call_count;
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
                        let sess_col = scene_to_egui(session_color(*si));
                        let is_hidden = effective_hidden.contains(&s.session_id);
                        let in_filter = filter_set.contains(&s.session_id);
                        let text_alpha = if is_hidden { 80u8 } else { 230u8 };
                        let name_col = egui::Color32::from_rgba_unmultiplied(240, 230, 200, text_alpha);
                        let dim_col = egui::Color32::from_rgba_unmultiplied(170, 160, 140, (text_alpha as u16 * 3 / 4) as u8);

                        let row_top = inner.top() + row_idx as f32 * (row_h + row_gap);
                        let row_rect = egui::Rect::from_min_size(
                            egui::pos2(inner.left(), row_top),
                            egui::vec2(inner.width(), row_h),
                        );
                        let resp = ui.interact(row_rect, egui::Id::new(("legend_flat", gi, *si)), egui::Sense::click());
                        if resp.clicked() { toggle_ids.push(s.session_id.clone()); }
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

                        // Eye icon: filled = in filter set, outline = not in set
                        let eye_cx = row_rect.left() + eye_w * 0.5 + 2.0;
                        let eye_cy = row_rect.center().y;
                        let eye_r = 4.0;
                        if in_filter {
                            ui.painter().circle_filled(egui::pos2(eye_cx, eye_cy), eye_r,
                                egui::Color32::from_rgba_unmultiplied(200, 190, 170, 180));
                        } else {
                            ui.painter().circle_stroke(egui::pos2(eye_cx, eye_cy), eye_r,
                                egui::Stroke::new(1.0, egui::Color32::from_rgba_unmultiplied(120, 110, 95, 120)));
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
                            &LegendStats { cost: s.total_cost_usd, last_input: s.last_input_tokens,
                                total_tokens: s.total_input + s.total_output, session_count: 1, api_call_count: s.api_call_count },
                            &s.model, None,
                            &[(s, sess_col)], effective_hidden, Some(eye_w));

                        row_idx += 1;

                        // Subagent tree (toggle + rows)
                        draw_subagent_tree(
                            ui, inner, row_h, row_gap, timeline_w,
                            &mut row_idx, row_rect, eye_w,
                            s, expanded_sessions.contains(&s.session_id),
                            &mut toggle_session_agents, gi, *si,
                        );
                    }
                } else {
                    // Group header row
                    let text_alpha = if all_hidden { 80u8 } else { 230u8 };
                    let bar_col = egui::Color32::from_rgba_unmultiplied(group_col.r(), group_col.g(), group_col.b(), text_alpha);
                    let name_col = egui::Color32::from_rgba_unmultiplied(240, 230, 200, text_alpha);
                    let dim_col = egui::Color32::from_rgba_unmultiplied(170, 160, 140, (text_alpha as u16 * 3 / 4) as u8);

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
                    let none_hidden = members.iter().all(|(si, _)| !effective_hidden.contains(&data.sessions[*si].session_id));
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
                        .map(|(si, _)| (&data.sessions[*si], scene_to_egui(session_color(*si))))
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
                        &LegendStats { cost: g_cost, last_input: g_last_input,
                            total_tokens: g_total_tokens, session_count: members.len() as u32, api_call_count: g_api_calls },
                        &g_model, agc,
                        &sess_refs, effective_hidden, Some(eye_w));

                    row_idx += 1;

                    // Expanded sub-rows
                    if is_expanded {
                        for (si, _) in members {
                            let s = &data.sessions[*si];
                            let sess_col = scene_to_egui(session_color(*si));
                            let is_hidden = effective_hidden.contains(&s.session_id);
                            let in_filter = filter_set.contains(&s.session_id);
                            let sub_alpha = if is_hidden { 60u8 } else { 230u8 };
                            let sub_name_col = egui::Color32::from_rgba_unmultiplied(240, 230, 200, sub_alpha);
                            let sub_dim = egui::Color32::from_rgba_unmultiplied(155, 145, 130, (sub_alpha as u16 * 3 / 4) as u8);

                            let sub_top = inner.top() + row_idx as f32 * (row_h + row_gap);
                            let sub_rect = egui::Rect::from_min_size(
                                egui::pos2(inner.left(), sub_top),
                                egui::vec2(inner.width(), row_h),
                            );
                            let sub_resp = ui.interact(sub_rect, egui::Id::new(("legend_sub", gi, *si)), egui::Sense::click());
                            if sub_resp.clicked() { toggle_ids.push(s.session_id.clone()); }
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
                            let eye_cx = sub_rect.left() + eye_w * 0.5 + 2.0 + 16.0;
                            let eye_cy = sub_rect.center().y;
                            let eye_r = 3.5;
                            if in_filter {
                                ui.painter().circle_filled(egui::pos2(eye_cx, eye_cy), eye_r,
                                    egui::Color32::from_rgba_unmultiplied(200, 190, 170, 180));
                            } else {
                                ui.painter().circle_stroke(egui::pos2(eye_cx, eye_cy), eye_r,
                                    egui::Stroke::new(1.0, egui::Color32::from_rgba_unmultiplied(120, 110, 95, 120)));
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
                                &LegendStats { cost: s.total_cost_usd, last_input: s.last_input_tokens,
                                    total_tokens: s.total_input + s.total_output, session_count: 1, api_call_count: s.api_call_count },
                                &s.model, None,
                                &[(s, sess_col)], effective_hidden, Some(16.0 + eye_w));

                            row_idx += 1;

                            // Subagent tree (toggle + rows)
                            draw_subagent_tree(
                                ui, inner, row_h, row_gap, timeline_w,
                                &mut row_idx, sub_rect, 16.0 + eye_w,
                                s, expanded_sessions.contains(&s.session_id),
                                &mut toggle_session_agents, gi, *si,
                            );
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
    // Apply session agent tree toggles
    for sid in toggle_session_agents {
        if expanded_sessions.contains(&sid) {
            expanded_sessions.remove(&sid);
        } else {
            expanded_sessions.insert(sid);
        }
    }
    // Apply group toggle: toggle all members in/out of filter_set
    if let Some((_cwd, member_ids)) = group_toggle {
        let any_in = member_ids.iter().any(|id| filter_set.contains(id));
        if any_in {
            for id in &member_ids { filter_set.remove(id); }
        } else {
            for id in member_ids { filter_set.insert(id); }
        }
    }
    // Apply individual session toggles
    for id in toggle_ids {
        if filter_set.contains(&id) {
            filter_set.remove(&id);
        } else {
            filter_set.insert(id);
        }
    }
    // Apply small mode entry
    if let Some(session_id) = enter_small_mode {
        *small_mode_session = Some(session_id);
    }

    let cursor_id = egui::Id::new("all_charts_cursor");
    let hover_id = egui::Id::new("hud_hover_turn");
    let panel_hl_id = egui::Id::new("panel_highlight");
    let panel_hl: PanelHighlight = ui.ctx().data(|d| d.get_temp(panel_hl_id).unwrap_or_default());

    // Clear hover state when pointer is outside chart areas or left the window.
    let mut all_charts_rect = egui::Rect::NOTHING;
    if chart_vis.cost { all_charts_rect = all_charts_rect.union(cost_rect); }
    if chart_vis.tokens { all_charts_rect = all_charts_rect.union(tok_rect); }
    if chart_vis.energy { all_charts_rect = all_charts_rect.union(energy_wh_rect); }
    if chart_vis.water { all_charts_rect = all_charts_rect.union(water_ml_rect); }
    if chart_vis.totals { all_charts_rect = all_charts_rect.union(totals_rect); }
    match ui.ctx().input(|i| i.pointer.hover_pos()) {
        None => { ui.ctx().data_mut(|d| d.remove::<HoverState>(hover_id)); }
        Some(pos) if !all_charts_rect.contains(pos) => { ui.ctx().data_mut(|d| d.remove::<HoverState>(hover_id)); }
        _ => {}
    }

    // Previous frame's hover x for drawing highlight VLine across all charts.
    let prev_hover: Option<HoverState> = ui.ctx().data(|d| d.get_temp(hover_id));
    let hover_vline_color = egui::Color32::from_rgba_unmultiplied(200, 190, 165, 35);

    // Click-to-pin: stores the x that was clicked on so highlighting persists until Esc/another click.
    let pinned_x_id = egui::Id::new("hud_pinned_hover_x");
    let pinned_pos_id = egui::Id::new("hud_pinned_cursor_pos");
    let pinned_hover_id = egui::Id::new("hud_pinned_hover_state");
    let mut pinned_x: Option<f64> = ui.ctx().data(|d| d.get_temp(pinned_x_id));
    let pinned_cursor_pos: Option<egui::Pos2> = ui.ctx().data(|d| d.get_temp(pinned_pos_id));
    let pinned_hover: Option<HoverState> = ui.ctx().data(|d| d.get_temp(pinned_hover_id));
    // Escape clears pin
    if ui.ctx().input(|i| i.key_pressed(egui::Key::Escape)) {
        pinned_x = None;
        ui.ctx().data_mut(|d| {
            d.remove::<f64>(pinned_x_id);
            d.remove::<egui::Pos2>(pinned_pos_id);
            d.remove::<HoverState>(pinned_hover_id);
        });
    }

    // Effective hover x: pinned takes priority over live hover
    let effective_hover_x: Option<f64> = pinned_x.or_else(|| prev_hover.as_ref().map(|hs| hs.x));

    // Sessions within threshold of effective_hover_x are "highlighted"; others dimmed.
    let visible_span = match *nav_view {
        Some((vmin, vmax)) => vmax - vmin,
        None => full_span,
    };
    let hl_threshold = (visible_span * 0.03).max(if is_time { 2.0 } else { 0.6 });
    let hovered_sessions: Vec<bool> = if let Some(hx) = effective_hover_x {
        cd.session_turns.iter().map(|(_, _, turns)| {
            turns.iter().any(|t| (t.x - hx).abs() < hl_threshold)
        }).collect()
    } else {
        vec![]
    };

    // Pin toggle is handled per-chart via plot_resp.response.clicked() after each show().

    // Screen-space containment check + source tracking + highlight VLine.
    // When pinned, draw the VLine at pinned x but don't update hover state (data stays frozen).
    let is_pinned = pinned_x.is_some();
    let update_hover_src = move |pui: &mut egui_plot::PlotUi, source: HoverSource| {
        // Draw highlight VLine at effective position (pinned or live)
        let vline_x = if is_pinned {
            pinned_x
        } else {
            prev_hover.as_ref().map(|hs| hs.x)
        };
        if let Some(x) = vline_x {
            pui.vline(VLine::new(x).color(hover_vline_color).width(1.0));
        }
        // When pinned, don't update hover state -- tooltip data stays frozen
        if is_pinned { return; }
        let Some(hover_pos) = pui.ctx().input(|i| i.pointer.hover_pos()) else { return };
        let b = pui.plot_bounds();
        let s_min = pui.screen_from_plot(egui_plot::PlotPoint::new(b.min()[0], b.min()[1]));
        let s_max = pui.screen_from_plot(egui_plot::PlotPoint::new(b.max()[0], b.max()[1]));
        if !egui::Rect::from_two_pos(s_min, s_max).contains(hover_pos) { return; }
        let x = pui.plot_from_screen(hover_pos).x;
        pui.ctx().data_mut(|d| d.insert_temp(hover_id, HoverState { x, source }));
    };

    // Called after each chart's show(): if clicked and something is hovered, toggle pin.
    // Stores data x, screen cursor position, and hover state so tooltip is fully frozen.
    let try_pin = |resp: &egui::Response| {
        if resp.clicked() {
            if let Some(hs) = &prev_hover {
                if pinned_x.is_some() {
                    resp.ctx.data_mut(|d| {
                        d.remove::<f64>(pinned_x_id);
                        d.remove::<egui::Pos2>(pinned_pos_id);
                        d.remove::<HoverState>(pinned_hover_id);
                    });
                } else {
                    let cursor = resp.ctx.input(|i| i.pointer.hover_pos()).unwrap_or_default();
                    resp.ctx.data_mut(|d| {
                        d.insert_temp(pinned_x_id, hs.x);
                        d.insert_temp(pinned_pos_id, cursor);
                        d.insert_temp(pinned_hover_id, hs.clone());
                    });
                }
            } else {
                resp.ctx.data_mut(|d| {
                    d.remove::<f64>(pinned_x_id);
                    d.remove::<egui::Pos2>(pinned_pos_id);
                    d.remove::<HoverState>(pinned_hover_id);
                });
            }
        }
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

    // --- cost per-turn chart with total cost overlay (right Y) ---
    if chart_vis.cost { ui.allocate_new_ui(egui::UiBuilder::new().max_rect(cost_rect), |ui| {
        panel_frame().show(ui, |ui| {
            let total_label = if let Some(last) = cd.combined_cost_pts.last() {
                format!("total {}", format_cost(last[1]))
            } else { String::new() };
            draw_chart_label(ui, "cost / turn", "ctx$  gen$", &total_label);
            // Scale cumulative lines into bar Y-range: map [0..total_cost_max] -> [0..per_turn_in_cost_max]
            let cost_scale = if cd.total_cost_max > 0.0 { cd.per_turn_in_cost_max / cd.total_cost_max } else { 1.0 };
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
            let plot_resp = p.show(ui, |pui| {
                    if *show_bars {
                        let fresh = BarChart::new(bars_to_egui_hl(&cd.in_cost_fresh_bars, &hovered_sessions)).name("fresh$");
                        let read = BarChart::new(bars_to_egui_hl(&cd.in_cost_cache_read_bars, &hovered_sessions)).name("read$").stack_on(&[&fresh]);
                        let create = BarChart::new(bars_to_egui_hl(&cd.in_cost_cache_create_bars, &hovered_sessions)).name("create$").stack_on(&[&fresh, &read]);
                        pui.bar_chart(fresh);
                        pui.bar_chart(read);
                        pui.bar_chart(create);
                        pui.bar_chart(BarChart::new(bars_to_egui_hl(&cd.out_cost_bars, &hovered_sessions)).name("gen$"));
                    }
                    // Overlay: total cost lines scaled into bar coordinate space
                    for (si, (color, points)) in cd.total_cost_lines.iter().enumerate() {
                        let (alpha, w) = if hovered_sessions.is_empty() {
                            if is_time { (0.3f32, 1.5f32) } else { (0.75f32, 2.0f32) }
                        } else if hovered_sessions.get(si).copied().unwrap_or(false) {
                            (1.0, 2.5)
                        } else {
                            (0.12, 1.0)
                        };
                        let scaled: Vec<[f64; 2]> = points.iter().map(|[x, y]| [*x, y * cost_scale]).collect();
                        pui.line(egui_plot::Line::new(scaled)
                            .color(scene_to_egui(*color).gamma_multiply(alpha)).width(w));
                    }
                    if !is_time || !panel_hl.key.is_empty() {
                        render_egui::render_markers(pui, &scene::build_markers(&cd.agent_xs, &cd.skill_xs, &cd.compaction_xs, &panel_hl.key));
                    }
                    update_hover_src(pui, HoverSource::Cost);
                });
            if is_time { handle_chart_nav(ui.ctx(), &plot_resp.response, plot_resp.transform.bounds(), nav_view, full_min, full_max, autofit); }
            try_pin(&plot_resp.response);
        });
    }); }

    // --- token per-turn chart with total token overlay (right Y) ---
    if chart_vis.tokens { ui.allocate_new_ui(egui::UiBuilder::new().max_rect(tok_rect), |ui| {
        panel_frame().show(ui, |ui| {
            let total_tok_label = if cd.total_tok_max > 0.0 {
                format!("total {}", format_tokens(cd.total_tok_max as u64))
            } else { String::new() };
            draw_chart_label(ui, "tokens / turn", "in  out", &total_tok_label);
            // Scale total token lines into bar Y-range
            let tok_scale = if cd.total_tok_max > 0.0 { cd.in_max / cd.total_tok_max } else { 1.0 };
            let mut p = base_plot("tok_big")
                .link_cursor(cursor_id, true, false)
                .auto_bounds(egui::Vec2b::new(true, true))
                .y_axis_formatter(move |v, _| {
                    let abs = v.value.abs();
                    if abs < 0.5 { return String::new(); }
                    format_tokens(abs.round() as u64)
                })
                .y_grid_spacer(token_grid_spacer)
                .show_axes([false, true])
                .show_grid(true);
            if let Some((vmin, vmax)) = if is_time { *nav_view } else { None } {
                p = p.include_x(vmin).include_x(vmax).auto_bounds(egui::Vec2b::new(false, true));
            }
            let plot_resp = p.show(ui, |pui| {
                    if *show_bars {
                        let fresh = BarChart::new(bars_to_egui_hl(&cd.in_tok_fresh_bars, &hovered_sessions)).name("fresh");
                        let read = BarChart::new(bars_to_egui_hl(&cd.in_tok_cache_read_bars, &hovered_sessions)).name("cached");
                        let create = BarChart::new(bars_to_egui_hl(&cd.in_tok_cache_create_bars, &hovered_sessions)).name("create");
                        let read = read.stack_on(&[&fresh]);
                        let create = create.stack_on(&[&fresh, &read]);
                        pui.bar_chart(fresh);
                        pui.bar_chart(read);
                        pui.bar_chart(create);
                        pui.bar_chart(BarChart::new(bars_to_egui_hl(&cd.out_tok_bars, &hovered_sessions)).name("out"));
                    }
                    // Overlay: total token lines (input solid, output dashed) scaled into bar space
                    for (si, (color, in_pts, out_pts)) in cd.total_tok_lines.iter().enumerate() {
                        let (alpha, w) = if hovered_sessions.is_empty() {
                            if is_time { (0.3f32, 1.5f32) } else { (0.75f32, 2.0f32) }
                        } else if hovered_sessions.get(si).copied().unwrap_or(false) {
                            (1.0, 2.5)
                        } else {
                            (0.12, 1.0)
                        };
                        let scaled_in: Vec<[f64; 2]> = in_pts.iter().map(|[x, y]| [*x, y * tok_scale]).collect();
                        let scaled_out: Vec<[f64; 2]> = out_pts.iter().map(|[x, y]| [*x, -y * tok_scale]).collect();
                        pui.line(egui_plot::Line::new(scaled_in)
                            .color(scene_to_egui(*color).gamma_multiply(alpha)).width(w));
                        pui.line(egui_plot::Line::new(scaled_out)
                            .color(scene_to_egui(*color).gamma_multiply(alpha * 0.7)).width(w * 0.6)
                            .style(egui_plot::LineStyle::Dashed { length: 6.0 }));
                    }
                    update_hover_src(pui, HoverSource::Tokens);
                });
            if is_time { handle_chart_nav(ui.ctx(), &plot_resp.response, plot_resp.transform.bounds(), nav_view, full_min, full_max, autofit); }
            try_pin(&plot_resp.response);
        });
    }); }

    // --- usage / budget chart (mutually exclusive, share the same slot) ---
    // Clear session hover tooltip when pointer is over this panel
    if let Some(pos) = ui.ctx().input(|i| i.pointer.hover_pos()) {
        if usage_rect.contains(pos) {
            ui.ctx().data_mut(|d| d.remove::<HoverState>(hover_id));
        }
    }
    ui.allocate_new_ui(egui::UiBuilder::new().max_rect(usage_rect), |ui| {
        panel_frame().show(ui, |ui| {
          if *show_budget {
            // --- budget chart: cumulative cost within billing period vs limit ---
            let period_start_x = billing.period_start_x();
            let period_cost_pts: Vec<[f64; 2]> = cd.budget_cost_pts.iter()
                .filter(|p| p[0] >= period_start_x)
                .copied()
                .collect();
            let period_spent = period_cost_pts.last().map(|p| p[1]).unwrap_or(0.0)
                - period_cost_pts.first().map(|p| p[1]).unwrap_or(0.0);
            let pct = if billing.limit_usd > 0.0 { (period_spent / billing.limit_usd * 100.0).min(999.0) } else { 0.0 };
            let pct_color = if pct > 90.0 { egui::Color32::from_rgb(220, 60, 60) }
                else if pct > 70.0 { egui::Color32::from_rgb(220, 160, 60) }
                else { Palette::TEXT_BRIGHT };

            let label = format!("{} / {} ({:.0}%)", format_cost(period_spent), format_cost(billing.limit_usd), pct);
            draw_chart_label(ui, "budget", &label, "");

            // Clickable config row: [day] [limit] controls
            let config_h = 14.0;
            let (config_rect, _) = ui.allocate_exact_size(egui::vec2(ui.available_width(), config_h), egui::Sense::hover());
            let painter = ui.painter();
            let font = egui::FontId::monospace(9.0);
            let cy = config_rect.center().y;

            // Reset day control: < day >
            let day_label = format!("reset day {}", billing.reset_day);
            let day_rect = egui::Rect::from_min_size(config_rect.min, egui::vec2(90.0, config_h));
            painter.text(day_rect.center(), egui::Align2::CENTER_CENTER, &day_label, font.clone(), Palette::TEXT_DIM);
            let dec_rect = egui::Rect::from_min_size(egui::pos2(day_rect.left(), cy - 6.0), egui::vec2(12.0, 12.0));
            let inc_rect = egui::Rect::from_min_size(egui::pos2(day_rect.right() - 12.0, cy - 6.0), egui::vec2(12.0, 12.0));
            if ui.interact(dec_rect, egui::Id::new("budget_day_dec"), egui::Sense::click()).clicked() {
                billing.reset_day = if billing.reset_day <= 1 { 28 } else { billing.reset_day - 1 };
                billing.save();
            }
            if ui.interact(inc_rect, egui::Id::new("budget_day_inc"), egui::Sense::click()).clicked() {
                billing.reset_day = if billing.reset_day >= 28 { 1 } else { billing.reset_day + 1 };
                billing.save();
            }
            painter.text(dec_rect.center(), egui::Align2::CENTER_CENTER, "<", font.clone(), Palette::TEXT_BRIGHT);
            painter.text(inc_rect.center(), egui::Align2::CENTER_CENTER, ">", font.clone(), Palette::TEXT_BRIGHT);

            // (painter ref ends here -- mutable ui calls below need exclusive access)

            // Limit input row
            let input_h = 20.0;
            let (lim_row, _) = ui.allocate_exact_size(egui::vec2(ui.available_width(), input_h), egui::Sense::hover());
            let lim_cy = lim_row.center().y;
            let lim_input_rect = egui::Rect::from_min_size(egui::pos2(lim_row.left() + 40.0, lim_row.top() + 1.0), egui::vec2(80.0, input_h - 2.0));
            let mut lim_buf = billing.limit_input_buf.clone();
            let lim_te = egui::TextEdit::singleline(&mut lim_buf)
                .font(font.clone())
                .desired_width(70.0)
                .text_color(Palette::TEXT_BRIGHT);
            let lim_te_resp = ui.put(lim_input_rect, lim_te);
            billing.limit_input_buf = lim_buf.clone();
            if lim_te_resp.lost_focus() && ui.ctx().input(|i| i.key_pressed(egui::Key::Enter)) {
                let clean = lim_buf.trim().trim_start_matches('$');
                if let Ok(val) = clean.parse::<f64>() {
                    billing.limit_usd = val.max(1.0);
                    billing.save();
                }
            }
            ui.painter().text(egui::pos2(lim_row.left() + 2.0, lim_cy), egui::Align2::LEFT_CENTER,
                "limit", font.clone(), Palette::TEXT_DIM);

            // Web-reported total row
            let web_h = 20.0;
            let (web_row, _) = ui.allocate_exact_size(egui::vec2(ui.available_width(), web_h), egui::Sense::hover());
            let web_cy = web_row.center().y;
            let input_rect = egui::Rect::from_min_size(egui::pos2(web_row.left() + 40.0, web_row.top() + 1.0), egui::vec2(80.0, web_h - 2.0));
            let mut buf = billing.web_input_buf.clone();
            let te = egui::TextEdit::singleline(&mut buf)
                .font(font.clone())
                .desired_width(60.0)
                .text_color(egui::Color32::from_rgb(180, 140, 220));
            let te_resp = ui.put(input_rect, te);
            billing.web_input_buf = buf.clone();
            if te_resp.lost_focus() && ui.ctx().input(|i| i.key_pressed(egui::Key::Enter)) {
                let clean = buf.trim().trim_start_matches('$');
                if let Ok(val) = clean.parse::<f64>() {
                    let now_epoch = std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH).unwrap().as_secs();
                    billing.web_reported = Some((val, now_epoch));
                    billing.save();
                }
            }
            // Clear button interaction (must happen before re-borrowing painter)
            let x_rect = egui::Rect::from_min_size(egui::pos2(web_row.right() - 16.0, web_cy - 6.0), egui::vec2(12.0, 12.0));
            let clear_clicked = if billing.web_reported.is_some() {
                ui.interact(x_rect, egui::Id::new("budget_web_clear"), egui::Sense::click()).clicked()
            } else { false };
            if clear_clicked { billing.web_reported = None; billing.web_input_buf.clear(); billing.save(); }
            // Now paint text (re-borrow painter)
            let painter = ui.painter();
            let web_color = egui::Color32::from_rgb(180, 140, 220);
            painter.text(egui::pos2(web_row.left() + 2.0, web_cy), egui::Align2::LEFT_CENTER, "web $", font.clone(), web_color);
            let info_x = web_row.left() + 118.0;
            if let Some((val, ts)) = billing.web_reported {
                let ago_min = (std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH).unwrap().as_secs() as f64 - ts as f64) / 60.0;
                let ago = if ago_min < 1.0 { "just now".into() }
                    else if ago_min < 60.0 { format!("{:.0}m ago", ago_min) }
                    else if ago_min < 24.0 * 60.0 { format!("{:.1}h ago", ago_min / 60.0) }
                    else { format!("{:.1}d ago", ago_min / (24.0 * 60.0)) };
                painter.text(egui::pos2(info_x, web_cy), egui::Align2::LEFT_CENTER,
                    &format!("{} as of {}", format_cost(val), ago), font.clone(), web_color);
                painter.text(x_rect.center(), egui::Align2::CENTER_CENTER, "x", font.clone(), Palette::TEXT_DIM);
            } else {
                painter.text(egui::pos2(info_x, web_cy), egui::Align2::LEFT_CENTER,
                    "enter total from web, press enter", font.clone(), Palette::TEXT_DIM);
            }

            // The chart: cumulative cost within period, zeroed at period start
            if !period_cost_pts.is_empty() {
                let base_cost = period_cost_pts.first().map(|p| p[1]).unwrap_or(0.0);
                let zeroed: Vec<[f64; 2]> = period_cost_pts.iter()
                    .map(|[x, y]| [*x, y - base_cost])
                    .collect();

                let limit_usd = billing.limit_usd;
                let web_reported = billing.web_reported;
                let budget_y_fmt = move |v: egui_plot::GridMark, _: &std::ops::RangeInclusive<f64>| -> String {
                    if v.value < 0.001 { String::new() } else { format_cost(v.value) }
                };
                let budget_tip = move |_name: &str, point: &egui_plot::PlotPoint| -> String {
                    let mut tip = format!("{} ({:.0}% of {})", format_cost(point.y), point.y / limit_usd * 100.0, format_cost(limit_usd));
                    if let Some((web_val, _)) = web_reported {
                        let diff = web_val - point.y;
                        if diff > 0.001 {
                            tip += &format!("\nnon-CLI: ~{}", format_cost(diff));
                        }
                    }
                    tip
                };

                let mut p = Plot::new("budget_period")
                    .show_axes([true, true])
                    .show_grid(true)
                    .allow_zoom(false)
                    .allow_drag(false)
                    .allow_scroll(false)
                    .show_background(false)
                    .set_margin_fraction(egui::Vec2::ZERO)
                    .auto_bounds(egui::Vec2b::new(true, true))
                    .include_y(0.0)
                    .include_y(billing.limit_usd * 1.05)
                    .y_axis_formatter(budget_y_fmt)
                    .x_axis_formatter(time_x_fmt)
                    .label_formatter(budget_tip);
                // In time mode, share viewport with other charts; otherwise auto-fit to billing period
                if is_time {
                    p = p.link_cursor(cursor_id, true, false);
                    if let Some((vmin, vmax)) = *nav_view {
                        p = p.include_x(vmin).include_x(vmax).auto_bounds(egui::Vec2b::new(false, true));
                    }
                }
                // If web reported total > CLI total, include it in Y range
                if let Some((web_val, _)) = billing.web_reported {
                    p = p.include_y(web_val * 1.05);
                }
                let web_reported_for_chart = billing.web_reported;
                let period_start_for_chart = period_start_x;
                let plot_resp = p.show(ui, |pui| {
                    let pts = step_pts(&zeroed); // budget always uses time-based x
                    pui.line(egui_plot::Line::new(pts)
                        .color(pct_color).width(2.0).fill(0.0).name("CLI spent"));
                    // Budget limit line
                    pui.hline(egui_plot::HLine::new(billing.limit_usd)
                        .color(egui::Color32::from_rgba_unmultiplied(200, 60, 60, 120))
                        .width(1.0));
                    // Period start marker
                    pui.vline(VLine::new(period_start_for_chart)
                        .color(egui::Color32::from_rgba_unmultiplied(100, 180, 100, 60))
                        .width(0.5));
                    // Web-reported marker: horizontal line at reported value + point marker at report time
                    if let Some((web_val, web_ts)) = web_reported_for_chart {
                        let web_x = web_ts as f64 / 60.0;
                        let web_color = egui::Color32::from_rgb(180, 140, 220); // purple
                        // Horizontal line at web-reported value
                        pui.hline(egui_plot::HLine::new(web_val)
                            .color(egui::Color32::from_rgba_unmultiplied(180, 140, 220, 80))
                            .width(1.0));
                        // Point marker at the time it was reported
                        pui.points(egui_plot::Points::new(vec![[web_x, web_val]])
                            .color(web_color).radius(4.0).name("web reported"));
                        // Vertical line at report time
                        pui.vline(VLine::new(web_x)
                            .color(egui::Color32::from_rgba_unmultiplied(180, 140, 220, 40))
                            .width(0.5));
                        // If CLI cost at that time is lower, find the CLI value at web_x and show the gap
                        let cli_at_web_x = zeroed.iter()
                            .filter(|p| p[0] <= web_x)
                            .last()
                            .map(|p| p[1])
                            .unwrap_or(0.0);
                        if web_val > cli_at_web_x + 0.01 {
                            // Gap shading: a small rectangle between CLI line and web line at report time
                            let gap_pts = vec![
                                [web_x - 0.5, cli_at_web_x],
                                [web_x + 0.5, cli_at_web_x],
                                [web_x + 0.5, web_val],
                                [web_x - 0.5, web_val],
                            ];
                            pui.polygon(egui_plot::Polygon::new(gap_pts)
                                .fill_color(egui::Color32::from_rgba_unmultiplied(180, 140, 220, 30))
                                .name(format!("non-CLI: ~{}", format_cost(web_val - cli_at_web_x))));
                        }
                    }
                });
                if is_time { handle_chart_nav(ui.ctx(), &plot_resp.response, plot_resp.transform.bounds(), nav_view, full_min, full_max, autofit); }
            } else {
                let inner = ui.available_rect_before_wrap();
                let msg = "no data in billing period";
                ui.painter().text(inner.center(), egui::Align2::CENTER_CENTER,
                    msg, egui::FontId::monospace(9.0), Palette::TEXT_DIM);
            }
          } else {
            // --- usage chart: 5h + 7d utilization over time ---
            let usage_now_label = usage.latest.as_ref().map(|l|
                format!("5h {}%  7d {}%", l.five_hour as u32, l.seven_day as u32)
            ).unwrap_or_default();
            draw_chart_label(ui, "usage %", &usage_now_label, "");

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

                let mut p = Plot::new("usage_pct")
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
                    .label_formatter(tip_fmt);
                if let Some((vmin, vmax)) = if is_time { *nav_view } else { None } {
                    p = p.include_x(vmin).include_x(vmax).auto_bounds(egui::Vec2b::new(false, true));
                }
                let plot_resp = p.show(ui, |pui| {
                        pui.line(egui_plot::Line::new(five_h_pts)
                            .color(egui::Color32::from_rgb(220, 160, 60)).width(1.5).name("5h"));
                        pui.line(egui_plot::Line::new(seven_d_pts)
                            .color(egui::Color32::from_rgb(100, 160, 220)).width(1.5).name("7d"));
                        pui.hline(egui_plot::HLine::new(100.0)
                            .color(egui::Color32::from_rgba_unmultiplied(200, 60, 60, 80))
                            .width(0.5));
                    });
                if is_time { handle_chart_nav(ui.ctx(), &plot_resp.response, plot_resp.transform.bounds(), nav_view, full_min, full_max, autofit); }
            } else if let Some(e) = &usage.error {
                let inner = ui.available_rect_before_wrap();
                ui.painter().text(inner.center(), egui::Align2::CENTER_CENTER,
                    e, egui::FontId::monospace(9.0), Palette::TEXT_DIM);
            } else {
                let inner = ui.available_rect_before_wrap();
                ui.painter().text(inner.center(), egui::Align2::CENTER_CENTER,
                    "polling...", egui::FontId::monospace(9.0), Palette::TEXT_DIM);
            }
          }
        });
    });

    // Clear session hover tooltip when pointer is over tool panel
    if let Some(pos) = ui.ctx().input(|i| i.pointer.hover_pos()) {
        if tool_rect.contains(pos) {
            ui.ctx().data_mut(|d| d.remove::<HoverState>(hover_id));
        }
    }
    // --- skill / agent / reads / tool breakdown (scrollable, via scene tree) ---
    let panel_hl_id = egui::Id::new("panel_highlight");
    ui.ctx().data_mut(|d| d.insert_temp(panel_hl_id, PanelHighlight::default()));
    let panel_nodes = scene::build_tool_panel(
        &cd.skill_list, &cd.agent_list, &cd.read_list, &cd.tool_list,
        &panel_hl.key,
    );
    ui.allocate_new_ui(egui::UiBuilder::new().max_rect(tool_rect), |ui| {
        panel_frame().show(ui, |ui| {
            ui.style_mut().visuals.override_text_color = Some(Palette::TEXT);
            ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Truncate);
            let hovered_key = render_egui::render(ui, &panel_nodes);
            if !hovered_key.is_empty() {
                ui.ctx().data_mut(|d| d.get_temp_mut_or_default::<PanelHighlight>(panel_hl_id)
                    .key = hovered_key);
            }
        });
    });

    // --- Row 3: per-turn energy (Wh) + per-turn water (mL), each with cumulative overlay ---
    let total_wh: f64 = cd.total_energy_lines.iter()
        .filter_map(|(_, pts)| pts.last().map(|p| p[1])).sum();
    let total_water_ml: f64 = cd.total_water_lines.iter()
        .filter_map(|(_, pts)| pts.last().map(|p| p[1])).sum();

    // Silly unit equivalences
    let wh_silly = if total_wh > 12.0 { format!("{:.1} phone charges", total_wh / 12.0) }
        else if total_wh > 0.01 { format!("{:.1} LED-bulb hrs", total_wh / 10.0) }
        else { String::new() };
    // 1 sip of water ~30 mL, 1 gulp ~60 mL, 1 cup = 237 mL
    let water_silly = if total_water_ml > 237.0 { format!("{:.1} cups", total_water_ml / 237.0) }
        else if total_water_ml > 30.0 { format!("{:.0} sips", total_water_ml / 30.0) }
        else if total_water_ml > 0.01 { format!("{:.1} drops", total_water_ml / 0.05) }
        else { String::new() };

    if chart_vis.energy { ui.allocate_new_ui(egui::UiBuilder::new().max_rect(energy_wh_rect), |ui| {
        panel_frame().show(ui, |ui| {
            let wh_label = if total_wh > 0.001 { format!("{:.2} Wh total", total_wh) } else { String::new() };
            draw_chart_label(ui, "energy / turn", &wh_label, &wh_silly);
            let energy_scale = if cd.total_energy_max > 0.0 { cd.energy_wh_max / cd.total_energy_max } else { 1.0 };
            let mut p = base_plot("energy_wh")
                .link_cursor(cursor_id, true, false)
                .include_y(0.0)
                .include_y(cd.energy_wh_max)
                .y_axis_formatter(move |v, _| {
                    if v.value < 1e-9 { String::new() } else { format!("{:.2}", v.value) }
                })
                .show_axes([false, true])
                .show_grid(true);
            if let Some((vmin, vmax)) = if is_time { *nav_view } else { None } {
                p = p.include_x(vmin).include_x(vmax).auto_bounds(egui::Vec2b::new(false, true));
            }
            let plot_resp = p.show(ui, |pui| {
                if *show_bars { pui.bar_chart(BarChart::new(bars_to_egui_hl(&cd.energy_wh_bars, &hovered_sessions)).name("Wh")); }
                for (si, (color, pts)) in cd.total_energy_lines.iter().enumerate() {
                    let (alpha, w) = if hovered_sessions.is_empty() {
                        if is_time { (0.3f32, 1.5f32) } else { (0.75f32, 2.0f32) }
                    } else if hovered_sessions.get(si).copied().unwrap_or(false) {
                        (1.0, 2.5)
                    } else {
                        (0.12, 1.0)
                    };
                    let scaled: Vec<[f64; 2]> = pts.iter().map(|[x, y]| [*x, y * energy_scale]).collect();
                    pui.line(egui_plot::Line::new(scaled)
                        .color(scene_to_egui(*color).gamma_multiply(alpha)).width(w));
                }
                update_hover_src(pui, HoverSource::Energy);
            });
            if is_time { handle_chart_nav(ui.ctx(), &plot_resp.response, plot_resp.transform.bounds(), nav_view, full_min, full_max, autofit); }
            try_pin(&plot_resp.response);
        });
    }); }

    if chart_vis.water { ui.allocate_new_ui(egui::UiBuilder::new().max_rect(water_ml_rect), |ui| {
        panel_frame().show(ui, |ui| {
            let water_label = if total_water_ml > 0.001 { format!("{:.1} mL total", total_water_ml) } else { String::new() };
            draw_chart_label(ui, "water / turn", &water_label, &water_silly);
            let water_scale = if cd.total_water_max > 0.0 { cd.water_ml_max / cd.total_water_max } else { 1.0 };
            let mut p = base_plot("water_ml")
                .link_cursor(cursor_id, true, false)
                .include_y(0.0)
                .include_y(cd.water_ml_max)
                .y_axis_formatter(move |v, _| {
                    if v.value < 1e-9 { String::new() } else { format!("{:.2}", v.value) }
                })
                .show_axes([false, true])
                .show_grid(true);
            if let Some((vmin, vmax)) = if is_time { *nav_view } else { None } {
                p = p.include_x(vmin).include_x(vmax).auto_bounds(egui::Vec2b::new(false, true));
            }
            let plot_resp = p.show(ui, |pui| {
                if *show_bars { pui.bar_chart(BarChart::new(bars_to_egui_hl(&cd.water_ml_bars, &hovered_sessions)).name("mL")); }
                for (si, (color, pts)) in cd.total_water_lines.iter().enumerate() {
                    let (alpha, w) = if hovered_sessions.is_empty() {
                        if is_time { (0.3f32, 1.5f32) } else { (0.75f32, 2.0f32) }
                    } else if hovered_sessions.get(si).copied().unwrap_or(false) {
                        (1.0, 2.5)
                    } else {
                        (0.12, 1.0)
                    };
                    let scaled: Vec<[f64; 2]> = pts.iter().map(|[x, y]| [*x, y * water_scale]).collect();
                    pui.line(egui_plot::Line::new(scaled)
                        .color(scene_to_egui(*color).gamma_multiply(alpha)).width(w));
                }
                update_hover_src(pui, HoverSource::Water);
            });
            if is_time { handle_chart_nav(ui.ctx(), &plot_resp.response, plot_resp.transform.bounds(), nav_view, full_min, full_max, autofit); }
            try_pin(&plot_resp.response);
        });
    }); }

    // --- bottom row: unified totals (cost + tokens + energy + water, all normalized) ---
    if chart_vis.totals { ui.allocate_new_ui(egui::UiBuilder::new().max_rect(totals_rect), |ui| {
        panel_frame().show(ui, |ui| {
            // Current values for labels
            let cur_cost = cd.combined_cost_pts.last().map(|p| p[1]).unwrap_or(0.0);
            let cur_tok = cd.total_tok_max;
            let cur_wh = total_wh;
            let cur_water = total_water_ml;
            let label = format!(
                "{}  {}tok  {:.1}Wh  {:.0}mL",
                format_cost(cur_cost), format_tokens(cur_tok as u64), cur_wh, cur_water
            );
            draw_chart_label(ui, "totals", &label, "");

            // Normalize all series to [0..1] range, then plot in [0..1] Y space
            let mut p = base_plot("totals_combined")
                .link_cursor(cursor_id, true, false)
                .include_y(0.0)
                .include_y(1.05)
                .y_axis_formatter(move |v, _| {
                    // Left Y shows cost scale
                    if v.value < 1e-9 { String::new() }
                    else { format_cost(v.value * cur_cost) }
                })
                .show_axes([is_time, true])
                .show_grid(true);
            if is_time { p = p.x_axis_formatter(time_x_fmt); }
            if let Some((vmin, vmax)) = if is_time { *nav_view } else { None } {
                p = p.include_x(vmin).include_x(vmax).auto_bounds(egui::Vec2b::new(false, true));
            }
            let cost_color = egui::Color32::from_rgb(220, 180, 60);   // gold
            let tok_color = egui::Color32::from_rgb(100, 160, 220);   // blue
            let energy_color = egui::Color32::from_rgb(120, 200, 80); // green
            let water_color = egui::Color32::from_rgb(80, 180, 220);  // cyan

            let plot_resp = p.show(ui, |pui| {
                // Cost line (normalized)
                if cd.combined_cost_max > 0.0 {
                    let norm: Vec<[f64; 2]> = cd.combined_cost_pts.iter()
                        .map(|[x, y]| [*x, y / cd.combined_cost_max]).collect();
                    pui.line(egui_plot::Line::new(norm).color(cost_color).width(2.0).name("cost"));
                }
                // Total tokens line (input, normalized) -- per session
                if cd.total_tok_max > 0.0 {
                    for (si, (_, in_pts, _)) in cd.total_tok_lines.iter().enumerate() {
                        let (alpha, w) = if hovered_sessions.is_empty() {
                            if is_time { (0.35f32, 1.2f32) } else { (0.8f32, 1.5f32) }
                        } else if hovered_sessions.get(si).copied().unwrap_or(false) {
                            (1.0, 2.5)
                        } else {
                            (0.12, 1.0)
                        };
                        let norm: Vec<[f64; 2]> = in_pts.iter()
                            .map(|[x, y]| [*x, y / cd.total_tok_max]).collect();
                        let norm = norm;
                        pui.line(egui_plot::Line::new(norm)
                            .color(tok_color.gamma_multiply(alpha)).width(w).name("tokens"));
                    }
                }
                // Total energy line (normalized) -- per session
                if cd.total_energy_max > 0.0 {
                    for (si, (_, pts)) in cd.total_energy_lines.iter().enumerate() {
                        let (alpha, w) = if hovered_sessions.is_empty() {
                            if is_time { (0.35f32, 1.2f32) } else { (0.8f32, 1.5f32) }
                        } else if hovered_sessions.get(si).copied().unwrap_or(false) {
                            (1.0, 2.5)
                        } else {
                            (0.12, 1.0)
                        };
                        let norm: Vec<[f64; 2]> = pts.iter()
                            .map(|[x, y]| [*x, y / cd.total_energy_max]).collect();
                        let norm = norm;
                        pui.line(egui_plot::Line::new(norm)
                            .color(energy_color.gamma_multiply(alpha)).width(w).name("energy"));
                    }
                }
                // Total water line (normalized) -- per session
                if cd.total_water_max > 0.0 {
                    for (si, (_, pts)) in cd.total_water_lines.iter().enumerate() {
                        let (alpha, w) = if hovered_sessions.is_empty() {
                            if is_time { (0.35f32, 1.2f32) } else { (0.8f32, 1.5f32) }
                        } else if hovered_sessions.get(si).copied().unwrap_or(false) {
                            (1.0, 2.5)
                        } else {
                            (0.12, 1.0)
                        };
                        let norm: Vec<[f64; 2]> = pts.iter()
                            .map(|[x, y]| [*x, y / cd.total_water_max]).collect();
                        let norm = norm;
                        pui.line(egui_plot::Line::new(norm)
                            .color(water_color.gamma_multiply(alpha)).width(w).name("water"));
                    }
                }
                update_hover_src(pui, HoverSource::WeeklyCost);
            });
            if is_time { handle_chart_nav(ui.ctx(), &plot_resp.response, plot_resp.transform.bounds(), nav_view, full_min, full_max, autofit); }
            try_pin(&plot_resp.response);
        });
    }); }

    // --- floating hover tooltip (all sessions, context-aware) ---
    // When pinned, use the frozen hover state and cursor position from pin time.
    let hover_state: Option<HoverState> = if pinned_hover.is_some() {
        pinned_hover.clone()
    } else {
        ui.ctx().data(|d| d.get_temp(hover_id))
    };
    if let Some(hs) = hover_state {
        let hx = hs.x;
        let is_pinned = pinned_x.is_some();
        let cursor_opt = if is_pinned { pinned_cursor_pos } else { ui.ctx().input(|i| i.pointer.hover_pos()) };
        if let Some(cursor) = cursor_opt {
            // (session_name, session_color, detail_text, optional breakdown fracs, sort_key)
            let mut entries: Vec<(String, egui::Color32, String, Option<[f32; 3]>, f64)> = vec![];
            // Context info from nearest turn (for footer): (context_tokens, context_limit, burn_rate_per_turn, is_reset)
            let mut context_footer: Option<(u64, u64, f64, bool)> = None;
            for (name, sess_color, turns) in &cd.session_turns {
                if turns.is_empty() { continue; }
                // Skip sessions where hover x is outside the session's data range (with small margin)
                let first_x = turns.first().unwrap().x;
                let last_x = turns.last().unwrap().x;
                let span = (last_x - first_x).max(1.0);
                let margin = span * 0.05; // 5% margin at edges
                if hx < first_x - margin || hx > last_x + margin { continue; }

                let nearest = turns.iter().enumerate().min_by(|(_, a), (_, b)| {
                    (a.x - hx).abs().partial_cmp(&(b.x - hx).abs()).unwrap()
                });
                if let Some((idx, t)) = nearest {
                    // Compute context burn rate: avg context growth per turn over recent window
                    let window = 5;
                    let start = idx.saturating_sub(window);
                    let burn_rate = if idx > start {
                        let ctx_start = turns[start].context_tokens;
                        let ctx_end = t.context_tokens;
                        if ctx_end > ctx_start {
                            (ctx_end - ctx_start) as f64 / (idx - start) as f64
                        } else {
                            0.0
                        }
                    } else {
                        0.0
                    };
                    context_footer = Some((t.context_tokens, t.context_limit, burn_rate, t.is_reset));

                    let (detail, breakdown) = match hs.source {
                        HoverSource::Cost => {
                            let total_in = t.in_cost;
                            let frac = |v: f64| if total_in > 0.0 { (v / total_in) as f32 } else { 0.0 };
                            let pct = |v: f64| (frac(v) * 100.0).round() as u32;
                            let thinking_tag = if t.has_thinking { " [thinking]" } else { "" };
                            let fracs = [frac(t.fresh_input_cost), frac(t.cache_read_cost), frac(t.cache_create_cost)];
                            (format!(
                                "  t{} [{}]{} ctx {} (fresh {}% read {}% create {}%)  gen {}  (+{})",
                                idx + 1, t.model_short, thinking_tag,
                                format_cost(t.in_cost),
                                pct(t.fresh_input_cost), pct(t.cache_read_cost), pct(t.cache_create_cost),
                                format_cost(t.out_cost),
                                format_cost(t.cost_change),
                            ), Some(fracs))
                        }
                        HoverSource::Tokens => {
                            let total_in = (t.in_tok + t.cache_read_tok + t.cache_create_tok) as f64;
                            let frac = |v: u64| if total_in > 0.0 { (v as f64 / total_in) as f32 } else { 0.0 };
                            let fracs = [frac(t.in_tok), frac(t.cache_read_tok), frac(t.cache_create_tok)];
                            (format!(
                                "  t{} [{}] ctx {} (fresh {} read {} create {})  out {}",
                                idx + 1, t.model_short,
                                format_tokens(total_in as u64),
                                format_tokens(t.in_tok), format_tokens(t.cache_read_tok), format_tokens(t.cache_create_tok),
                                format_tokens(t.out_tok),
                            ), Some(fracs))
                        }
                        HoverSource::TotalCost => (format!(
                            "  t{} total {}  (+{})",
                            idx + 1, format_cost(t.total_cost), format_cost(t.cost_change),
                        ), None),
                        HoverSource::TotalTokens => (format!(
                            "  t{} total in {}  out {}",
                            idx + 1, format_tokens(t.total_in_tok), format_tokens(t.total_out_tok),
                        ), None),
                        HoverSource::WeeklyCost | HoverSource::WeeklyRate => (format!(
                            "  t{} [{}] +{}  total {}",
                            idx + 1, t.model_short, format_cost(t.cost_change), format_cost(t.total_cost),
                        ), None),
                        HoverSource::Energy => {
                            let wh = t.energy.facility_kwh.mid * 1000.0;
                            (format!(
                                "  t{} [{}] {:.2} Wh  ({:.2}..{:.2})",
                                idx + 1, t.model_short, wh,
                                t.energy.facility_kwh.low * 1000.0,
                                t.energy.facility_kwh.high * 1000.0,
                            ), None)
                        }
                        HoverSource::Water => {
                            let wml = t.energy.water_total_ml.mid;
                            (format!(
                                "  t{} [{}] {:.2} mL  ({:.2}..{:.2})",
                                idx + 1, t.model_short, wml,
                                t.energy.water_total_ml.low,
                                t.energy.water_total_ml.high,
                            ), None)
                        }
                    };
                    let sc = scene_to_egui(*sess_color);
                    entries.push((name.clone(), sc, detail, breakdown, t.cost_change));
                }
            }
            // Sort by cost descending, cap at 8 entries so the tooltip stays readable
            entries.sort_by(|a, b| b.4.partial_cmp(&a.4).unwrap_or(std::cmp::Ordering::Equal));
            // Deduplicate entries with same name + detail (multiple sessions from same project dir)
            {
                let mut seen = HashSet::new();
                entries.retain(|e| seen.insert((e.0.clone(), e.2.clone())));
            }
            let omitted = entries.len().saturating_sub(8);
            entries.truncate(8);

            // Find nearby skill invocations
            let mut nearby_skills: Vec<&str> = cd.skill_xs.iter()
                .filter(|(x, _)| {
                    let snap = if entries.len() > 1 { 1.5 } else { 0.5 };
                    (x - hx).abs() <= snap
                })
                .map(|(_, name)| name.as_str())
                .collect();
            nearby_skills.dedup();

            // For WeeklyCost, compute cumulative combined cost at hovered x
            let running_total_header: Option<String> = if matches!(hs.source, HoverSource::WeeklyCost) {
                let pts = &cd.combined_cost_pts;
                let idx = pts.partition_point(|p| p[0] <= hx);
                let total = if idx > 0 { pts[idx - 1][1] } else if !pts.is_empty() { pts[0][1] } else { 0.0 };
                Some(format!("total {}", format_cost(total)))
            } else { None };

            if !entries.is_empty() {
                let win_rect = ui.ctx().screen_rect();
                let tip_w = 420.0_f32;
                let row_count = entries.len() + if running_total_header.is_some() { 1 } else { 0 }
                    + nearby_skills.len() + if context_footer.is_some() { 1 } else { 0 }
                    + if omitted > 0 { 1 } else { 0 } + 1; // +1 for header
                let tip_h = row_count as f32 * 16.0 + 20.0;
                let offset = 14.0;
                let x_offset = if cursor.x + tip_w + offset > win_rect.right() {
                    -tip_w - offset
                } else {
                    offset
                };
                let mut tip_pos = cursor + egui::vec2(x_offset, -tip_h - 8.0);
                tip_pos.y = tip_pos.y.max(win_rect.top() + 4.0);

                egui::Area::new(egui::Id::new("hud_float_tip"))
                    .fixed_pos(tip_pos)
                    .order(egui::Order::Tooltip)
                    .interactable(false)
                    .show(ui.ctx(), |ui| {
                        let frame_stroke = if is_pinned {
                            egui::Stroke::new(1.5, egui::Color32::from_rgb(180, 160, 100))
                        } else {
                            egui::Stroke::new(0.5, Palette::SEPARATOR)
                        };
                        egui::Frame::none()
                            .fill(egui::Color32::from_rgb(20, 18, 14))
                            .stroke(frame_stroke)
                            .rounding(5.0)
                            .inner_margin(egui::Margin::same(8.0))
                            .show(ui, |ui| {
                                ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Extend);
                                let font = egui::FontId::monospace(10.0);
                                let hdr_col = Palette::TEXT_DIM;

                                if let Some(hdr) = &running_total_header {
                                    ui.add(egui::Label::new(
                                        egui::RichText::new(hdr).monospace().size(11.0).color(Palette::INPUT_TINT)
                                    ));
                                }

                                // Table header with column colors matching bar segments
                                {
                                    let mono = egui::FontId::monospace(11.0);
                                    let mut hdr = egui::text::LayoutJob::default();
                                    let ha = |job: &mut egui::text::LayoutJob, text: &str, color: egui::Color32| {
                                        job.append(text, 0.0, egui::TextFormat {
                                            font_id: mono.clone(), color, ..Default::default()
                                        });
                                    };
                                    let ci = egui::Color32::from_rgb(100, 160, 220); // input blue
                                    let co = egui::Color32::from_rgb(220, 160, 60);  // output gold
                                    let cg = egui::Color32::from_rgb(80, 180, 100);  // cache green
                                    ha(&mut hdr, "session          turn  model   ", hdr_col);
                                    match hs.source {
                                        HoverSource::Cost => {
                                            ha(&mut hdr, "    ctx$", ci);
                                            ha(&mut hdr, "      gen$", co);
                                            ha(&mut hdr, "    +total", Palette::TEXT_BRIGHT);
                                        }
                                        HoverSource::Tokens => {
                                            ha(&mut hdr, "      in", ci);
                                            ha(&mut hdr, "       out", co);
                                            ha(&mut hdr, "    cached", cg);
                                        }
                                        HoverSource::Energy => {
                                            ha(&mut hdr, "      Wh", egui::Color32::from_rgb(120, 200, 80));
                                        }
                                        HoverSource::Water => {
                                            ha(&mut hdr, "      mL", egui::Color32::from_rgb(80, 180, 220));
                                        }
                                        _ => {
                                            ha(&mut hdr, "    cost", ci);
                                            ha(&mut hdr, "    +delta", Palette::TEXT_BRIGHT);
                                            ha(&mut hdr, "    total", hdr_col);
                                        }
                                    }
                                    ui.add(egui::Label::new(hdr));
                                }
                                ui.add_space(2.0);

                                for (name, sess_color, _detail, _breakdown, _sort_key) in &entries {
                                    // Re-find the nearest turn for structured rendering
                                    let nearest = cd.session_turns.iter()
                                        .find(|(n, _, _)| n == name)
                                        .and_then(|(_, _, turns)| {
                                            turns.iter().enumerate().min_by(|(_, a), (_, b)| {
                                                (a.x - hx).abs().partial_cmp(&(b.x - hx).abs()).unwrap()
                                            })
                                        });
                                    let Some((idx, t)) = nearest else { continue };

                                    let short_name = if name.len() > 16 { &name[..16] } else { name };

                                    // Colors matching bar segments
                                    let c_input = egui::Color32::from_rgb(100, 160, 220);  // input cost (blue)
                                    let c_output = if t.has_thinking {
                                        egui::Color32::from_rgb(180, 80, 200)  // thinking output (purple)
                                    } else {
                                        egui::Color32::from_rgb(220, 160, 60)  // normal output (gold)
                                    };
                                    let c_total = Palette::TEXT_BRIGHT;
                                    let c_meta = Palette::TEXT_DIM;
                                    let c_cache_read = egui::Color32::from_rgb(80, 180, 100); // green
                                    let _c_cache_create = egui::Color32::from_rgb(220, 160, 60); // gold

                                    // Build a multi-colored row via LayoutJob
                                    let mut job = egui::text::LayoutJob::default();
                                    let mono = egui::FontId::monospace(11.0);
                                    let append = |job: &mut egui::text::LayoutJob, text: &str, color: egui::Color32| {
                                        job.append(text, 0.0, egui::TextFormat {
                                            font_id: mono.clone(),
                                            color,
                                            ..Default::default()
                                        });
                                    };

                                    // Session name in session color
                                    append(&mut job, &format!("{:<16}", short_name), *sess_color);
                                    // Turn + model in dim
                                    let think = if t.has_thinking { "*" } else { " " };
                                    append(&mut job, &format!(" t{:<4}{:<8}{}", idx + 1, t.model_short, think), c_meta);

                                    match hs.source {
                                        HoverSource::Cost => {
                                            append(&mut job, &format!("{:>8}", format_cost(t.in_cost)), c_input);
                                            append(&mut job, &format!("  {:>8}", format_cost(t.out_cost)), c_output);
                                            append(&mut job, &format!("  +{}", format_cost(t.cost_change)), c_total);
                                        }
                                        HoverSource::Tokens => {
                                            let total_in = t.in_tok + t.cache_read_tok + t.cache_create_tok;
                                            append(&mut job, &format!("{:>8}", format_tokens(total_in)), c_input);
                                            append(&mut job, &format!("  {:>8}", format_tokens(t.out_tok)), c_output);
                                            append(&mut job, &format!("  {:>8}", format_tokens(t.cache_read_tok)), c_cache_read);
                                        }
                                        HoverSource::Energy => {
                                            let wh = t.energy.facility_kwh.mid * 1000.0;
                                            append(&mut job, &format!("{:>7.2} Wh", wh), egui::Color32::from_rgb(120, 200, 80));
                                        }
                                        HoverSource::Water => {
                                            let wml = t.energy.water_total_ml.mid;
                                            append(&mut job, &format!("{:>7.1} mL", wml), egui::Color32::from_rgb(80, 180, 220));
                                        }
                                        _ => {
                                            append(&mut job, &format!("{:>8}", format_cost(t.in_cost + t.out_cost)), c_input);
                                            append(&mut job, &format!("  +{:<7}", format_cost(t.cost_change)), c_total);
                                            append(&mut job, &format!("  {}", format_cost(t.total_cost)), c_meta);
                                        }
                                    }

                                    ui.add(egui::Label::new(job));
                                }
                                if omitted > 0 {
                                    ui.add(egui::Label::new(
                                        egui::RichText::new(format!("+{} more", omitted)).font(font.clone()).color(Palette::TEXT_DIM)
                                    ));
                                }
                                for sk in &nearby_skills {
                                    let short = sk.rsplit(':').next().unwrap_or(sk);
                                    ui.add(egui::Label::new(
                                        egui::RichText::new(format!("skill: {}", short)).font(font.clone()).color(Palette::SKILL_MARKER)
                                    ));
                                }
                                if let Some((ctx_tok, ctx_limit, burn_rate, is_reset)) = context_footer {
                                    let pct = if ctx_limit > 0 { (ctx_tok as f64 / ctx_limit as f64 * 100.0).round() as u32 } else { 0 };
                                    let remaining = ctx_limit.saturating_sub(ctx_tok);
                                    let countdown = if burn_rate > 0.0 {
                                        format!("  ~{} turns til compact", (remaining as f64 / burn_rate).round() as u64)
                                    } else { String::new() };
                                    let reset_tag = if is_reset { " [RESET]" } else { "" };
                                    let ctx_line = format!("ctx {}% {}/{}  rem {}{}{}",
                                        pct, format_tokens(ctx_tok), format_tokens(ctx_limit),
                                        format_tokens(remaining), countdown, reset_tag);
                                    let ctx_color = if pct >= 80 { egui::Color32::from_rgb(220, 80, 60) }
                                        else if pct >= 60 { egui::Color32::from_rgb(220, 180, 60) }
                                        else { Palette::TEXT_DIM };
                                    ui.add(egui::Label::new(
                                        egui::RichText::new(ctx_line).font(font.clone()).color(ctx_color)
                                    ));
                                }
                                if is_pinned {
                                    ui.add(egui::Label::new(
                                        egui::RichText::new("pinned (Esc to clear)").font(font).color(
                                            egui::Color32::from_rgba_unmultiplied(180, 160, 100, 140))
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

fn draw_strip(ui: &mut egui::Ui, data: &HudData, cd: &ChartData, panel_hl: &PanelHighlight) {
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
                    let fresh = BarChart::new(bars_to_egui(&cd.in_cost_fresh_bars)).color(egui::Color32::from_rgb(60, 120, 200)).name("fresh$");
                    let read = BarChart::new(bars_to_egui(&cd.in_cost_cache_read_bars)).color(egui::Color32::from_rgb(80, 180, 100)).name("read$").stack_on(&[&fresh]);
                    let create = BarChart::new(bars_to_egui(&cd.in_cost_cache_create_bars)).color(egui::Color32::from_rgb(220, 160, 60)).name("create$").stack_on(&[&fresh, &read]);
                    pui.bar_chart(fresh);
                    pui.bar_chart(read);
                    pui.bar_chart(create);
                    pui.bar_chart(BarChart::new(bars_to_egui(&cd.out_cost_bars)).name("out$"));
                    render_egui::render_markers(pui, &scene::build_markers(&cd.agent_xs, &cd.skill_xs, &cd.compaction_xs, &panel_hl.key));
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
                    let fresh = BarChart::new(bars_to_egui(&cd.in_tok_fresh_bars)).color(egui::Color32::from_rgb(60, 120, 200)).name("fresh");
                    let read = BarChart::new(bars_to_egui(&cd.in_tok_cache_read_bars)).color(egui::Color32::from_rgb(80, 180, 100)).name("cached").stack_on(&[&fresh]);
                    let create = BarChart::new(bars_to_egui(&cd.in_tok_cache_create_bars)).color(egui::Color32::from_rgb(220, 160, 60)).name("create").stack_on(&[&fresh, &read]);
                    pui.bar_chart(fresh);
                    pui.bar_chart(read);
                    pui.bar_chart(create);
                    pui.bar_chart(BarChart::new(bars_to_egui(&cd.out_tok_bars)).name("out"));
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
                painter.circle_filled(egui::pos2(inner.left() + 4.0, row_y + row_h * 0.5), 2.5, scene_to_egui(session_color(si)));
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
            if data.sessions.iter().any(|s| !s.skill_counts.is_empty()) {
                painter.line_segment([egui::pos2(inner.right() - 60.0, inner.bottom() - 4.0), egui::pos2(inner.right() - 60.0, inner.bottom() - 8.0)], egui::Stroke::new(1.5, Palette::SKILL_MARKER));
                painter.text(egui::pos2(inner.right() - 57.0, inner.bottom() - 6.0), egui::Align2::LEFT_CENTER, "skill", egui::FontId::monospace(6.0), Palette::SKILL_MARKER);
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

                // Active filter set depends on current mode
                let filter_set = match self.filter_mode {
                    FilterMode::Exclude => &mut self.exclude_set,
                    FilterMode::Include => &mut self.include_set,
                };
                // Compute effective hidden set from active filter + mode
                let mut effective_hidden: HashSet<String> = match self.filter_mode {
                    FilterMode::Exclude => filter_set.clone(),
                    FilterMode::Include => {
                        data.sessions.iter()
                            .map(|s| s.session_id.clone())
                            .filter(|id| !filter_set.contains(id))
                            .collect()
                    }
                };
                if self.show_active_only {
                    for s in &data.sessions {
                        if !s.is_active { effective_hidden.insert(s.session_id.clone()); }
                    }
                }

                // Cache chart data: only rebuild when data generation, hidden set, or time_axis changes
                let data_gen = data.generation;
                let cache_hit = self.cached_chart.as_ref().map_or(false, |(g, h, t, _)| {
                    *g == data_gen && *h == effective_hidden && *t == self.time_axis
                });
                if !cache_hit {
                    let cd = build_chart_data(&data, &effective_hidden, self.time_axis);
                    self.cached_chart = Some((data_gen, effective_hidden.clone(), self.time_axis, cd));
                }
                let cd = &self.cached_chart.as_ref().unwrap().3;
                let usage = self.usage_data.lock().unwrap().clone();

                if big_mode {
                    if let Some(sid) = self.small_mode_session.clone() {
                        draw_small(ui, &data, &cd, &usage, &sid, filter_set, &mut self.filter_mode, &mut self.time_axis, &mut self.autofit, &mut self.nav_view, &mut self.small_mode_session);
                    } else {
                        draw_big(ui, &data, &cd, &usage, filter_set, &mut self.filter_mode, &mut self.show_active_only, &mut self.show_bars, &effective_hidden, &mut self.time_axis, &mut self.autofit, &mut self.nav_view, &mut self.expanded_groups, &mut self.expanded_sessions, &mut self.small_mode_session, &mut self.chart_vis, &mut self.show_budget, &mut self.billing);
                    }
                } else {
                    let strip_hl: PanelHighlight = ui.ctx().data(|d| d.get_temp(egui::Id::new("panel_highlight")).unwrap_or_default());
                    draw_strip(ui, &data, &cd, &strip_hl);
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

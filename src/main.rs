#![allow(dead_code)]

mod agent_harnesses;
mod energy;
#[path = "3_legend_table.rs"]
mod legend;
#[path = "2_model_registry.rs"]
mod model_registry;
#[path = "1_render_egui.rs"]
mod render_egui;
#[path = "0_scene.rs"]
mod scene;
mod usage;

use std::collections::HashSet;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use eframe::{App, CreationContext};
use egui_plot::{Bar, BarChart, Plot, PlotPoint, VLine};

use agent_harnesses::claude_code::{Event, HudData};

use scene::{ChartData, TurnInfo};

fn main() -> eframe::Result {
    use tracing_subscriber::{fmt, prelude::*, EnvFilter};
    let log_file = std::fs::File::create("/tmp/cc-hud.log").expect("could not create log file");
    tracing_subscriber::registry()
        .with(fmt::layer().with_writer(log_file))
        .with(
            EnvFilter::try_from_default_env().unwrap_or(EnvFilter::new("info,wgpu=warn,naga=warn")),
        )
        .init();

    let args: Vec<String> = std::env::args().collect();
    let show_history = !args.iter().any(|a| a == "--no-history" || a == "-N");
    let do_backup = args.iter().any(|a| a == "--backup");

    // Optional additive-only rsync backup (only if rsync exists)
    if do_backup {
        std::thread::spawn(|| {
            if std::process::Command::new("which")
                .arg("rsync")
                .output()
                .map(|o| o.status.success())
                .unwrap_or(false)
            {
                let dest = format!(
                    "{}/.cc-hud-backup/projects",
                    std::env::var("HOME").unwrap_or_default()
                );
                let _ = std::fs::create_dir_all(&dest);
                let src = format!(
                    "{}/.claude/projects/",
                    std::env::var("HOME").unwrap_or_default()
                );
                match std::process::Command::new("rsync")
                    .args(["-a", "--ignore-existing", &src, &dest])
                    .status()
                {
                    Ok(s) if s.success() => tracing::info!(dest, "backup complete"),
                    Ok(s) => tracing::warn!(code = ?s.code(), "rsync exited non-zero"),
                    Err(e) => tracing::warn!(%e, "rsync failed"),
                }
            } else {
                tracing::warn!("--backup: rsync not found, skipping");
            }
        });
    }

    let hud_data = Arc::new(Mutex::new(HudData::default()));
    let usage_data = Arc::new(Mutex::new(usage::UsageData::default()));

    let feed_data = hud_data.clone();
    std::thread::spawn(move || {
        agent_harnesses::claude_code::poll_loop(feed_data, show_history);
    });

    let feed_usage = usage_data.clone();
    std::thread::spawn(move || {
        usage::poll_loop(feed_usage, Duration::from_secs(90));
    });

    let native_options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default().with_inner_size([960.0, 460.0]),
        ..Default::default()
    };

    eframe::run_native(
        "cc-hud",
        native_options,
        Box::new(|cc| Ok(Box::new(Hud::new(cc, hud_data, usage_data)))),
    )
}

/// Which chart sections are visible in big mode.
#[derive(Clone)]
struct ChartVisibility {
    cost: bool,
    tokens: bool,
    energy: bool,
    water: bool,
    totals: bool,
    tool: bool,
}

impl Default for ChartVisibility {
    fn default() -> Self {
        Self {
            cost: true,
            tokens: true,
            energy: true,
            water: true,
            totals: true,
            tool: true,
        }
    }
}

/// Billing period configuration for budget tracking.
#[derive(Clone)]
struct BillingConfig {
    /// Day of month the billing period resets (1-28).
    reset_day: u8,
    /// Hour of day the reset happens (0-23, default 0 = midnight).
    reset_hour: u8,
    /// Minute of the reset hour (0-59, in 15-min steps: 0/15/30/45).
    reset_minute: u8,
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
        Self {
            reset_day: 1,
            reset_hour: 0,
            reset_minute: 0,
            limit_usd: 100.0,
            web_reported: None,
            web_input_buf: String::new(),
            limit_input_buf: "100".into(),
        }
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
            "reset_minute": self.reset_minute,
            "limit_usd": self.limit_usd,
            "web_reported": self.web_reported,
        });
        let _ = std::fs::write(
            &path,
            serde_json::to_string_pretty(&json).unwrap_or_default(),
        );
    }

    fn load() -> Self {
        let path = Self::config_path();
        let mut cfg = BillingConfig::default();
        if let Ok(data) = std::fs::read_to_string(&path) {
            if let Ok(v) = serde_json::from_str::<serde_json::Value>(&data) {
                if let Some(d) = v["reset_day"].as_u64() {
                    cfg.reset_day = d.min(28).max(1) as u8;
                }
                if let Some(h) = v["reset_hour"].as_u64() {
                    cfg.reset_hour = h.min(23) as u8;
                }
                if let Some(m) = v["reset_minute"].as_u64() {
                    cfg.reset_minute = (m.min(59) / 15 * 15) as u8; // snap to 15-min grid
                }
                if let Some(l) = v["limit_usd"].as_f64() {
                    cfg.limit_usd = l.max(1.0);
                    cfg.limit_input_buf = format!("{:.0}", l);
                }
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

    fn save_to_file(&self) {
        let path = Self::config_path();
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let json = serde_json::json!({
            "reset_day": self.reset_day,
            "reset_hour": self.reset_hour,
            "reset_minute": self.reset_minute,
            "limit_usd": self.limit_usd,
            "web_reported": self.web_reported,
        });
        let _ = std::fs::write(
            &path,
            serde_json::to_string_pretty(&json).unwrap_or_default(),
        );
    }
}

impl BillingConfig {
    /// Compute the start of the current billing period as epoch seconds.
    fn period_start_epoch(&self) -> u64 {
        let now_secs = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs() as libc::time_t;
        let mut tm: libc::tm = unsafe { std::mem::zeroed() };
        unsafe {
            libc::localtime_r(&now_secs, &mut tm);
        }

        // Set to reset_day at reset_hour:reset_minute:00
        tm.tm_mday = self.reset_day as i32;
        tm.tm_hour = self.reset_hour as i32;
        tm.tm_min = self.reset_minute as i32;
        tm.tm_sec = 0;
        tm.tm_isdst = -1; // let mktime figure it out

        let candidate = unsafe { libc::mktime(&mut tm) };
        // If candidate is in the future, go back one month
        if candidate > now_secs {
            tm.tm_mon -= 1;
            if tm.tm_mon < 0 {
                tm.tm_mon = 11;
                tm.tm_year -= 1;
            }
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
enum FilterMode {
    Include,
    Exclude,
}

struct Hud {
    hud_data: Arc<Mutex<HudData>>,
    usage_data: Arc<Mutex<usage::UsageData>>,
    exclude_set: HashSet<String>,
    include_set: HashSet<String>,
    filter_mode: FilterMode,
    show_active_only: bool,
    show_bars: bool,
    time_axis: bool,
    compaction_plus: bool,
    autofit: bool,
    nav_view: Option<(f64, f64)>,
    turn_view: Option<(f64, f64)>,
    expanded_groups: HashSet<String>,
    expanded_sessions: HashSet<String>,
    chart_vis: ChartVisibility,
    show_budget: bool,
    billing: BillingConfig,
    focused: Option<String>,
    viewer: Option<TurnViewerState>,
    cached_chart: Option<(usize, HashSet<String>, bool, bool, ChartData)>,
    cached_plot: Option<PlotCache>,
    last_seen_gen: usize,
    chart_tree: egui_tiles::Tree<ChartPane>,
}

#[derive(Copy, Clone, Eq, PartialEq, Hash, Debug)]
enum ChartPane {
    Cost,
    Tokens,
    Usage,
    Tool,
    Energy,
    Water,
    Totals,
}

struct ChartBehavior {
    rects: std::collections::HashMap<ChartPane, egui::Rect>,
}

impl egui_tiles::Behavior<ChartPane> for ChartBehavior {
    fn tab_title_for_pane(&mut self, pane: &ChartPane) -> egui::WidgetText {
        format!("{:?}", pane).into()
    }
    fn pane_ui(
        &mut self,
        ui: &mut egui::Ui,
        _tile_id: egui_tiles::TileId,
        pane: &mut ChartPane,
    ) -> egui_tiles::UiResponse {
        self.rects.insert(*pane, ui.available_rect_before_wrap().shrink(3.0));
        egui_tiles::UiResponse::None
    }
    fn simplification_options(&self) -> egui_tiles::SimplificationOptions {
        egui_tiles::SimplificationOptions {
            all_panes_must_have_tabs: false,
            ..Default::default()
        }
    }
    fn tab_bar_height(&self, _style: &egui::Style) -> f32 { 0.0 }
    fn gap_width(&self, _style: &egui::Style) -> f32 { 6.0 }
}

fn build_chart_tree(show_tool: bool) -> egui_tiles::Tree<ChartPane> {
    let mut tiles = egui_tiles::Tiles::default();
    let cost = tiles.insert_pane(ChartPane::Cost);
    let tokens = tiles.insert_pane(ChartPane::Tokens);
    let usage = tiles.insert_pane(ChartPane::Usage);
    let energy = tiles.insert_pane(ChartPane::Energy);
    let water = tiles.insert_pane(ChartPane::Water);
    let totals = tiles.insert_pane(ChartPane::Totals);
    let top_row = if show_tool {
        let tool = tiles.insert_pane(ChartPane::Tool);
        let right_col = tiles.insert_vertical_tile(vec![usage, tool]);
        tiles.insert_horizontal_tile(vec![cost, tokens, right_col])
    } else {
        tiles.insert_horizontal_tile(vec![cost, tokens, usage])
    };
    let mid_row = tiles.insert_horizontal_tile(vec![energy, water]);
    let root = tiles.insert_vertical_tile(vec![top_row, mid_row, totals]);
    egui_tiles::Tree::new("chart_tree", root, tiles)
}


impl Hud {
    fn new(
        _cc: &CreationContext<'_>,
        hud_data: Arc<Mutex<HudData>>,
        usage_data: Arc<Mutex<usage::UsageData>>,
    ) -> Self {
        Self {
            hud_data,
            usage_data,
            exclude_set: HashSet::new(),
            include_set: HashSet::new(),
            filter_mode: FilterMode::Exclude,
            show_active_only: false,
            show_bars: true,
            time_axis: false,
            compaction_plus: false,
            autofit: true,
            nav_view: None,
            turn_view: None,
            expanded_groups: HashSet::new(),
            expanded_sessions: HashSet::new(),
            chart_vis: ChartVisibility::default(),
            show_budget: false,
            billing: BillingConfig::load(),
            focused: None,
            viewer: None,
            cached_chart: None,
            cached_plot: None,
            last_seen_gen: 0,
            chart_tree: build_chart_tree(true),
        }
    }
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
enum HoverSource {
    Cost,
    Tokens,
    TotalCost,
    TotalTokens,
    WeeklyCost,
    WeeklyRate,
    Energy,
    Water,
    Budget,
}

use legend::LegendHighlight;

/// Wrapper for storing hover state in egui temp storage.
#[derive(Clone, Copy)]
struct HoverState {
    x: f64,
    source: HoverSource,
}

/// Open-viewer state. Payload is loaded lazily on first frame after open.
#[derive(Clone)]
struct TurnViewerState {
    session_id: String,
    cwd: String,
    display_name: String,
    api_call_idx: u32,
    payload: Option<agent_harnesses::claude_code::TurnPayload>,
}

/// Which panel row is hovered, used to highlight corresponding vlines on charts.
#[derive(Clone, Default)]
struct PanelHighlight {
    /// "skill:<name>" or "agent:<type>" -- empty means nothing highlighted
    key: String,
}

fn scene_to_egui(c: scene::Color) -> egui::Color32 {
    egui::Color32::from_rgba_unmultiplied(c.0, c.1, c.2, c.3)
}

fn decimate(pts: &[[f64; 2]], max: usize) -> Vec<[f64; 2]> {
    if pts.len() <= max {
        return pts.to_vec();
    }
    let stride = pts.len() as f64 / max as f64;
    let mut out = Vec::with_capacity(max + 1);
    let mut i = 0.0;
    while (i as usize) < pts.len() {
        out.push(pts[i as usize]);
        i += stride;
    }
    if let Some(last) = pts.last() {
        if out.last() != Some(last) {
            out.push(*last);
        }
    }
    out
}

use scene::{format_cost, format_tokens, session_color};

/// Convert a cumulative line series to a step function.
/// Each point stays flat at the previous value until the next x, then steps up.
/// This correctly represents discrete events (turns) rather than continuous accumulation.
fn step_pts(pts: &[[f64; 2]]) -> Vec<[f64; 2]> {
    if pts.len() < 2 {
        return pts.to_vec();
    }
    let mut out = Vec::with_capacity(pts.len() * 2 - 1);
    out.push(pts[0]);
    for w in pts.windows(2) {
        out.push([w[1][0], w[0][1]]); // horizontal at prev y up to next x
        out.push(w[1]); // vertical step up
    }
    out
}

/// Format epoch seconds as "YYYY/MM/DD HH:MM" in local time (24h).
fn format_epoch_local(epoch_secs: u64) -> String {
    let ts = epoch_secs as libc::time_t;
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    unsafe {
        libc::localtime_r(&ts, &mut tm);
    }
    format!(
        "{:04}/{:02}/{:02} {:02}:{:02}",
        tm.tm_year + 1900,
        tm.tm_mon + 1,
        tm.tm_mday,
        tm.tm_hour,
        tm.tm_min
    )
}

use scene::build_chart_data;

const MAX_LINE_PTS: usize = 400;

struct PlotCache {
    cost_lines: Vec<Vec<PlotPoint>>,
    tok_in_lines: Vec<Vec<PlotPoint>>,
    tok_out_lines: Vec<Vec<PlotPoint>>,
    energy_lines: Vec<Vec<PlotPoint>>,
    water_lines: Vec<Vec<PlotPoint>>,
    totals_cost: Vec<PlotPoint>,
    totals_tok: Vec<Vec<PlotPoint>>,
    totals_energy: Vec<Vec<PlotPoint>>,
    totals_water: Vec<Vec<PlotPoint>>,
}

fn bars_culled(bars: &[scene::BarData], hl: &[bool], xmin: f64, xmax: f64) -> Vec<Bar> {
    let finite = xmin.is_finite() && xmax.is_finite();
    let margin = if finite { (xmax - xmin) * 0.02 } else { 0.0 };
    let lo = if finite { xmin - margin } else { f64::NEG_INFINITY };
    let hi = if finite { xmax + margin } else { f64::INFINITY };
    bars.iter()
        .filter(|b| b.x >= lo && b.x <= hi)
        .map(|b| {
            let col = if hl.is_empty() || hl.get(b.session_idx).copied().unwrap_or(true) {
                scene_to_egui(b.color)
            } else {
                scene_to_egui(b.color).gamma_multiply(0.12)
            };
            Bar::new(b.x, b.height).width(b.width).fill(col)
        })
        .collect()
}

fn build_plot_cache(cd: &ChartData) -> PlotCache {
    let cost_scale = if cd.total_cost_max > 0.0 {
        cd.per_turn_in_cost_max / cd.total_cost_max
    } else {
        1.0
    };
    let tok_scale = if cd.total_tok_max > 0.0 {
        cd.in_max / cd.total_tok_max
    } else {
        1.0
    };
    let energy_scale = if cd.total_energy_max > 0.0 {
        cd.energy_wh_max / cd.total_energy_max
    } else {
        1.0
    };
    let water_scale = if cd.total_water_max > 0.0 {
        cd.water_ml_max / cd.total_water_max
    } else {
        1.0
    };

    let to_pp = |v: Vec<[f64; 2]>| -> Vec<PlotPoint> {
        v.into_iter().map(|[x, y]| PlotPoint::new(x, y)).collect()
    };
    let scale_decimate = |pts: &[[f64; 2]], s: f64| -> Vec<PlotPoint> {
        to_pp(decimate(
            &pts.iter().map(|[x, y]| [*x, y * s]).collect::<Vec<_>>(),
            MAX_LINE_PTS,
        ))
    };

    let cost_lines = cd
        .total_cost_lines
        .iter()
        .map(|(_, pts)| scale_decimate(pts, cost_scale))
        .collect();
    let tok_in_lines = cd
        .total_tok_lines
        .iter()
        .map(|(_, in_pts, _)| scale_decimate(in_pts, tok_scale))
        .collect();
    let tok_out_lines = cd
        .total_tok_lines
        .iter()
        .map(|(_, _, out_pts)| scale_decimate(out_pts, -tok_scale))
        .collect();
    let energy_lines = cd
        .total_energy_lines
        .iter()
        .map(|(_, pts)| scale_decimate(pts, energy_scale))
        .collect();
    let water_lines = cd
        .total_water_lines
        .iter()
        .map(|(_, pts)| scale_decimate(pts, water_scale))
        .collect();

    let norm_decimate = |pts: &[[f64; 2]], max: f64| -> Vec<PlotPoint> {
        if max > 0.0 {
            to_pp(decimate(
                &pts.iter().map(|[x, y]| [*x, y / max]).collect::<Vec<_>>(),
                MAX_LINE_PTS,
            ))
        } else {
            vec![]
        }
    };
    let totals_cost = norm_decimate(&cd.combined_cost_pts, cd.combined_cost_max);
    let totals_tok = cd
        .total_tok_lines
        .iter()
        .map(|(_, in_pts, _)| norm_decimate(in_pts, cd.total_tok_max))
        .collect();
    let totals_energy = cd
        .total_energy_lines
        .iter()
        .map(|(_, pts)| norm_decimate(pts, cd.total_energy_max))
        .collect();
    let totals_water = cd
        .total_water_lines
        .iter()
        .map(|(_, pts)| norm_decimate(pts, cd.total_water_max))
        .collect();

    PlotCache {
        cost_lines,
        tok_in_lines,
        tok_out_lines,
        energy_lines,
        water_lines,
        totals_cost,
        totals_tok,
        totals_energy,
        totals_water,
    }
}

// ---------------------------------------------------------------------------
// Grid spacer for token axes: picks 1-2-5 steps (e.g. 1k, 2k, 5k, 10k, 50k)
// ---------------------------------------------------------------------------

fn token_grid_spacer(input: egui_plot::GridInput) -> Vec<egui_plot::GridMark> {
    let range = (input.bounds.1 - input.bounds.0).abs();
    if range < 1.0 {
        return vec![];
    }

    // Target ~4-6 grid lines in the visible range
    let raw_step = range / 5.0;

    // Round to nearest 1-2-5 sequence
    let mag = 10.0_f64.powf(raw_step.log10().floor());
    let norm = raw_step / mag;
    let step = if norm <= 1.5 {
        mag
    } else if norm <= 3.5 {
        2.0 * mag
    } else if norm <= 7.5 {
        5.0 * mag
    } else {
        10.0 * mag
    };

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
        if value < lo - sub_step || value > hi + sub_step {
            continue;
        }
        let is_major = (value / step).round() * step == value
            || (value - (value / step).round() * step).abs() < step * 0.01;
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
        .auto_bounds([true, true])
}

/// Handle scroll/drag on an egui_plot chart, updating the shared nav_view.
/// Also handles click-to-pin: clicking stores the hover x in pinned_x_id temp storage.
/// Call this after p.show() when is_time is true.
fn handle_chart_nav(
    ctx: &egui::Context,
    resp: &egui::Response,
    bounds: &egui_plot::PlotBounds,
    view: &mut Option<(f64, f64)>,
    full_min: f64,
    full_max: f64,
    min_span: f64,
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
        let (mut vmin, mut vmax) = view.unwrap_or((full_min, full_max));
        let vspan = vmax - vmin;
        vmin -= dx_time;
        vmax -= dx_time;
        if vmin < full_min {
            vmin = full_min;
            vmax = vmin + vspan;
        }
        if vmax > full_max {
            vmax = full_max;
            vmin = vmax - vspan;
        }
        *view = Some((vmin, vmax));
    }

    // Vertical scroll = zoom anchored at cursor
    let scroll_y = ctx.input(|i| i.smooth_scroll_delta.y);
    if resp.hovered() && scroll_y.abs() > 0.1 {
        *autofit = false;
        let zoom_factor = (1.0 - (scroll_y as f64 * 0.0008)).clamp(0.85, 1.18);
        let (vmin, vmax) = view.unwrap_or((full_min, full_max));
        let vspan = vmax - vmin;
        let anchor = ctx
            .input(|i| i.pointer.hover_pos())
            .map(|p| {
                let frac = ((p.x - resp.rect.left()) / resp.rect.width()).clamp(0.0, 1.0) as f64;
                x_min + frac * x_span
            })
            .unwrap_or((x_min + x_max) / 2.0);
        let t = ((anchor - vmin) / vspan).clamp(0.0, 1.0);
        let upper = full_span.max(min_span);
        let new_span = (vspan * zoom_factor).clamp(min_span, upper);
        let new_min = (anchor - t * new_span).max(full_min);
        let new_max = (new_min + new_span).min(full_max);
        let new_min = (new_max - new_span).max(full_min);
        *view = Some((new_min, new_max));
    }

    // Horizontal scroll = pan
    let scroll_x = ctx.input(|i| i.smooth_scroll_delta.x);
    if resp.hovered() && scroll_x.abs() > 0.1 {
        *autofit = false;
        let dx_time = scroll_x as f64 * x_span / rect_w;
        let (mut vmin, mut vmax) = view.unwrap_or((full_min, full_max));
        let vspan = vmax - vmin;
        vmin -= dx_time;
        vmax -= dx_time;
        if vmin < full_min {
            vmin = full_min;
            vmax = vmin + vspan;
        }
        if vmax > full_max {
            vmax = full_max;
            vmin = vmax - vspan;
        }
        *view = Some((vmin, vmax));
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
    if lines.len() > 1 {
        Some(lines.join("\n"))
    } else {
        None
    }
}

fn draw_nav_strip(
    ui: &mut egui::Ui,
    rect: egui::Rect,
    id: &str,
    view: &mut Option<(f64, f64)>,
    full_min: f64,
    full_max: f64,
    min_span: f64,
    autofit: &mut bool,
    dots: &[(f64, egui::Color32)],
    label_fmt: &dyn Fn(f64) -> String,
) {
    let full_span = (full_max - full_min).max(1e-9);
    ui.allocate_new_ui(egui::UiBuilder::new().max_rect(rect), |ui| {
        let bar = ui.available_rect_before_wrap();
        let resp = ui.interact(bar, egui::Id::new(id), egui::Sense::click_and_drag());
        let painter = ui.painter();
        painter.line_segment(
            [egui::pos2(bar.left(), bar.bottom()), egui::pos2(bar.right(), bar.bottom())],
            egui::Stroke::new(0.5, Palette::SEPARATOR),
        );
        let bar_w = bar.width();
        let cy = bar.center().y;
        for (x, col) in dots {
            let frac = ((x - full_min) / full_span) as f32;
            let px = bar.left() + frac * bar_w;
            painter.circle_filled(egui::pos2(px, cy), 1.5, *col);
        }
        let (vmin, vmax) = view.unwrap_or((full_min, full_max));
        let v0 = ((vmin - full_min) / full_span) as f32;
        let v1 = ((vmax - full_min) / full_span) as f32;
        let vp_left = bar.left() + v0.clamp(0.0, 1.0) * bar_w;
        let vp_right = bar.left() + v1.clamp(0.0, 1.0) * bar_w;
        if vp_left > bar.left() {
            painter.rect_filled(
                egui::Rect::from_min_max(bar.left_top(), egui::pos2(vp_left, bar.bottom())),
                0.0,
                egui::Color32::from_rgba_unmultiplied(0, 0, 0, 120),
            );
        }
        if vp_right < bar.right() {
            painter.rect_filled(
                egui::Rect::from_min_max(egui::pos2(vp_right, bar.top()), bar.right_bottom()),
                0.0,
                egui::Color32::from_rgba_unmultiplied(0, 0, 0, 120),
            );
        }
        painter.rect_stroke(
            egui::Rect::from_min_max(
                egui::pos2(vp_left, bar.top()),
                egui::pos2(vp_right, bar.bottom()),
            ),
            1.0,
            egui::Stroke::new(1.0, Palette::TEXT_DIM),
            egui::StrokeKind::Outside,
        );
        if resp.dragged() {
            *autofit = false;
            let dx = (resp.drag_delta().x / bar_w) as f64 * full_span;
            let (mut vmin, mut vmax) = view.unwrap_or((full_min, full_max));
            let vspan = vmax - vmin;
            vmin += dx;
            vmax += dx;
            if vmin < full_min { vmin = full_min; vmax = vmin + vspan; }
            if vmax > full_max { vmax = full_max; vmin = vmax - vspan; }
            *view = Some((vmin, vmax));
        }
        let scroll_y = ui.input(|i| i.smooth_scroll_delta.y);
        if resp.hovered() && scroll_y.abs() > 0.1 {
            *autofit = false;
            let zoom_factor = (1.0 - (scroll_y as f64 * 0.0008)).clamp(0.85, 1.18);
            let (vmin, vmax) = view.unwrap_or((full_min, full_max));
            let vspan = vmax - vmin;
            let mouse_frac = ui.input(|i| i.pointer.hover_pos())
                .map(|p| ((p.x - bar.left()) / bar_w).clamp(0.0, 1.0) as f64)
                .unwrap_or(0.5);
            let anchor = full_min + mouse_frac * full_span;
            let t = ((anchor - vmin) / vspan).clamp(0.0, 1.0);
            let new_span = (vspan * zoom_factor).clamp(min_span, full_span);
            let new_min = (anchor - t * new_span).max(full_min);
            let new_max = (new_min + new_span).min(full_max);
            let new_min = (new_max - new_span).max(full_min);
            *view = Some((new_min, new_max));
        }
        let scroll_x = ui.input(|i| i.smooth_scroll_delta.x);
        if resp.hovered() && scroll_x.abs() > 0.1 {
            *autofit = false;
            let dx = -(scroll_x as f64 / bar_w as f64) * full_span;
            let (mut vmin, mut vmax) = view.unwrap_or((full_min, full_max));
            let vspan = vmax - vmin;
            vmin += dx;
            vmax += dx;
            if vmin < full_min { vmin = full_min; vmax = vmin + vspan; }
            if vmax > full_max { vmax = full_max; vmin = vmax - vspan; }
            *view = Some((vmin, vmax));
        }
        let label_font = egui::FontId::monospace(8.0);
        let n_labels = (bar_w / 80.0).floor().max(2.0) as usize;
        for i in 0..=n_labels {
            let frac = i as f64 / n_labels as f64;
            let t = full_min + frac * full_span;
            let px = bar.left() + frac as f32 * bar_w;
            painter.text(
                egui::pos2(px, bar.bottom() - 1.0),
                egui::Align2::CENTER_BOTTOM,
                &label_fmt(t),
                label_font.clone(),
                Palette::TEXT_DIM,
            );
        }
    });
}

fn panel_frame() -> egui::Frame {
    egui::Frame::NONE
        .fill(Palette::BG_PANEL)
        .stroke(egui::Stroke::new(0.5, Palette::SEPARATOR))
        .corner_radius(4.0)
        .inner_margin(egui::Margin::same(6))
}

// (draw_subagent_tree, draw_legend_row, LegendStats moved to 2_legend.rs)

// ---------------------------------------------------------------------------
// Small mode (compact 3-row overlay for a single session)
// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// Big dashboard layout
// ---------------------------------------------------------------------------

fn draw_big(
    ui: &mut egui::Ui,
    data: &HudData,
    cd: &ChartData,
    pc: &PlotCache,
    usage: &usage::UsageData,
    filter_set: &mut HashSet<String>,
    filter_mode: &mut FilterMode,
    show_active_only: &mut bool,
    show_bars: &mut bool,
    effective_hidden: &HashSet<String>,
    time_axis: &mut bool,
    compaction_plus: &mut bool,
    autofit: &mut bool,
    time_view: &mut Option<(f64, f64)>,
    turn_view: &mut Option<(f64, f64)>,
    expanded_groups: &mut HashSet<String>,
    expanded_sessions: &mut HashSet<String>,
    chart_vis: &mut ChartVisibility,
    show_budget: &mut bool,
    billing: &mut BillingConfig,
    focused: &mut Option<String>,
    viewer: &mut Option<TurnViewerState>,
    chart_tree: &mut egui_tiles::Tree<ChartPane>,
) {
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

    // Row 0.5: nav strips. Time strip always shown. Turn strip stacked below in non-time mode.
    let is_time = *time_axis;
    let nav_h = 16.0_f32;
    let nav_rect =
        egui::Rect::from_min_size(egui::pos2(x0, y0 + controls_h + gap), egui::vec2(w, nav_h));
    let turn_nav_rect = if !is_time {
        Some(egui::Rect::from_min_size(
            egui::pos2(x0, nav_rect.bottom() + 2.0),
            egui::vec2(w, nav_h),
        ))
    } else {
        None
    };
    let after_nav_y = turn_nav_rect.map_or(nav_rect.bottom(), |r| r.bottom()) + gap;

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
    let legend_h = (h * 0.30).clamp(100.0, 220.0);

    // Dynamic layout: hidden sections give their space to the main chart row
    let show_cost_tok = chart_vis.cost || chart_vis.tokens;
    let show_energy_water = chart_vis.energy || chart_vis.water;
    let show_totals = chart_vis.totals;

    let base_energy_h = (h * 0.14).clamp(60.0, 130.0);
    let base_weekly_h = (h * 0.12).max(45.0);
    let energy_row_h = if show_energy_water {
        base_energy_h
    } else {
        0.0
    };
    let weekly_h = if show_totals { base_weekly_h } else { 0.0 };
    let energy_gap = if show_energy_water { gap } else { 0.0 };
    let weekly_gap = if show_totals { gap } else { 0.0 };

    let fixed_overhead = controls_h
        + gap
        + nav_h
        + gap
        + legend_h
        + energy_row_h
        + energy_gap
        + weekly_h
        + weekly_gap;
    let chart_h = if show_cost_tok {
        (h - fixed_overhead - gap).max(40.0)
    } else {
        0.0
    };
    let chart_gap = if show_cost_tok { gap } else { 0.0 };

    let legend_rect =
        egui::Rect::from_min_size(egui::pos2(x0, after_nav_y), egui::vec2(w, legend_h));

    let chart_y = after_nav_y + legend_h + chart_gap;
    let charts_bottom = energy_row_h + weekly_gap + weekly_h + energy_gap;
    let charts_total_h = chart_h + charts_bottom;
    let charts_area = egui::Rect::from_min_size(egui::pos2(x0, chart_y), egui::vec2(w, charts_total_h));

    let (cost_rect, tok_rect, usage_rect, tool_rect, energy_wh_rect, water_ml_rect, totals_rect) = {
        let mut behavior = ChartBehavior { rects: std::collections::HashMap::new() };
        ui.allocate_new_ui(egui::UiBuilder::new().max_rect(charts_area), |ui| {
            chart_tree.ui(&mut behavior, ui);
        });
        let zero = egui::Rect::NOTHING;
        let g = |p: ChartPane| behavior.rects.get(&p).copied().unwrap_or(zero);
        (
            g(ChartPane::Cost),
            g(ChartPane::Tokens),
            g(ChartPane::Usage),
            g(ChartPane::Tool),
            g(ChartPane::Energy),
            g(ChartPane::Water),
            g(ChartPane::Totals),
        )
    };

    // Context window size (tokens). Could be made configurable.
    const CTX_WINDOW: u64 = 200_000;

    let now_secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let week_start_secs = now_secs.saturating_sub(7 * 24 * 3600);
    let week_span = (now_secs - week_start_secs).max(1) as f32;
    let now_min = now_secs as f64 / 60.0;

    // Formatter for x-axis in time mode: minutes-from-epoch -> relative label
    let time_x_fmt =
        move |v: egui_plot::GridMark, _range: &std::ops::RangeInclusive<f64>| -> String {
            let ago_min = now_min - v.value;
            if ago_min < 0.5 {
                return "now".into();
            }
            if ago_min < 60.0 {
                return format!("{}m", ago_min.round() as i64);
            }
            let ago_h = ago_min / 60.0;
            if ago_h < 24.0 {
                return format!("{:.0}h", ago_h);
            }
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
            let mode_col = if filter_set.is_empty() {
                Palette::TEXT_DIM
            } else {
                Palette::TEXT_BRIGHT
            };
            let btn_size = egui::vec2(70.0, controls_h - 6.0);
            let btn_rect = egui::Rect::from_min_size(
                egui::pos2(inner.left() + 2.0, cy - btn_size.y / 2.0),
                btn_size,
            );
            let btn_resp = ui.interact(
                btn_rect,
                egui::Id::new("ctrl_filter_mode"),
                egui::Sense::click(),
            );
            if btn_resp.clicked() {
                *filter_mode = match *filter_mode {
                    FilterMode::Include => FilterMode::Exclude,
                    FilterMode::Exclude => FilterMode::Include,
                };
                // Don't clear -- each mode has its own independent set
            }
            if btn_resp.hovered() {
                painter.rect_filled(
                    btn_rect,
                    3.0,
                    egui::Color32::from_rgba_unmultiplied(255, 255, 255, 12),
                );
            }
            painter.text(
                btn_rect.center(),
                egui::Align2::CENTER_CENTER,
                mode_label,
                egui::FontId::monospace(10.0),
                mode_col,
            );
            // Clear filter set button
            if !filter_set.is_empty() {
                let clr_rect = egui::Rect::from_min_size(
                    egui::pos2(btn_rect.right() + 4.0, cy - btn_size.y / 2.0),
                    egui::vec2(20.0, btn_size.y),
                );
                let clr_resp = ui.interact(
                    clr_rect,
                    egui::Id::new("ctrl_filter_clear"),
                    egui::Sense::click(),
                );
                if clr_resp.clicked() {
                    filter_set.clear();
                }
                if clr_resp.hovered() {
                    painter.rect_filled(
                        clr_rect,
                        3.0,
                        egui::Color32::from_rgba_unmultiplied(255, 255, 255, 12),
                    );
                }
                painter.text(
                    clr_rect.center(),
                    egui::Align2::CENTER_CENTER,
                    "x",
                    egui::FontId::monospace(10.0),
                    Palette::TEXT_DIM,
                );
            }

            // Next button x-cursor (advances after each button)
            let mut bx = btn_rect.right() + if filter_set.is_empty() { 8.0 } else { 30.0 };

            // "active" toggle
            let ao_label = if *show_active_only {
                "● active"
            } else {
                "○ active"
            };
            let ao_col = if *show_active_only {
                Palette::TEXT_BRIGHT
            } else {
                Palette::TEXT_DIM
            };
            let ao_rect = egui::Rect::from_min_size(
                egui::pos2(bx, cy - btn_size.y / 2.0),
                egui::vec2(60.0, btn_size.y),
            );
            let ao_resp = ui.interact(
                ao_rect,
                egui::Id::new("ctrl_active_only"),
                egui::Sense::click(),
            );
            if ao_resp.clicked() {
                *show_active_only = !*show_active_only;
            }
            if ao_resp.hovered() {
                painter.rect_filled(
                    ao_rect,
                    3.0,
                    egui::Color32::from_rgba_unmultiplied(255, 255, 255, 12),
                );
            }
            painter.text(
                ao_rect.center(),
                egui::Align2::CENTER_CENTER,
                ao_label,
                egui::FontId::monospace(10.0),
                ao_col,
            );
            bx = ao_rect.right() + 4.0;

            // "bars" toggle -- hide per-turn bars, show only cumulative lines
            let bars_label = if *show_bars { "● bars" } else { "○ bars" };
            let bars_col = if *show_bars {
                Palette::TEXT_BRIGHT
            } else {
                Palette::TEXT_DIM
            };
            let bars_rect = egui::Rect::from_min_size(
                egui::pos2(bx, cy - btn_size.y / 2.0),
                egui::vec2(50.0, btn_size.y),
            );
            let bars_resp = ui.interact(
                bars_rect,
                egui::Id::new("ctrl_show_bars"),
                egui::Sense::click(),
            );
            if bars_resp.clicked() {
                *show_bars = !*show_bars;
            }
            if bars_resp.hovered() {
                painter.rect_filled(
                    bars_rect,
                    3.0,
                    egui::Color32::from_rgba_unmultiplied(255, 255, 255, 12),
                );
            }
            painter.text(
                bars_rect.center(),
                egui::Align2::CENTER_CENTER,
                bars_label,
                egui::FontId::monospace(10.0),
                bars_col,
            );
            bx = bars_rect.right() + 4.0;

            // "budget" toggle -- swap usage chart with billing period budget chart
            let bdg_label = if *show_budget {
                "● budget"
            } else {
                "○ budget"
            };
            let bdg_col = if *show_budget {
                Palette::TEXT_BRIGHT
            } else {
                Palette::TEXT_DIM
            };
            let bdg_rect = egui::Rect::from_min_size(
                egui::pos2(bx, cy - btn_size.y / 2.0),
                egui::vec2(60.0, btn_size.y),
            );
            let bdg_resp = ui.interact(
                bdg_rect,
                egui::Id::new("ctrl_show_budget"),
                egui::Sense::click(),
            );
            if bdg_resp.clicked() {
                *show_budget = !*show_budget;
            }
            if bdg_resp.hovered() {
                painter.rect_filled(
                    bdg_rect,
                    3.0,
                    egui::Color32::from_rgba_unmultiplied(255, 255, 255, 12),
                );
            }
            painter.text(
                bdg_rect.center(),
                egui::Align2::CENTER_CENTER,
                bdg_label,
                egui::FontId::monospace(10.0),
                bdg_col,
            );
            bx = bdg_rect.right() + 4.0;

            // "time axis" toggle button
            let ta_label = if *time_axis { "● time" } else { "○ time" };
            let ta_col = if *time_axis {
                Palette::TEXT_BRIGHT
            } else {
                Palette::TEXT_DIM
            };
            let ta_rect = egui::Rect::from_min_size(
                egui::pos2(bx, cy - btn_size.y / 2.0),
                egui::vec2(60.0, btn_size.y),
            );
            let ta_resp = ui.interact(
                ta_rect,
                egui::Id::new("ctrl_time_axis"),
                egui::Sense::click(),
            );
            if ta_resp.clicked() {
                *time_axis = !*time_axis;
                *autofit = true;
                *time_view = None;
                *turn_view = None;
            }
            if ta_resp.hovered() {
                painter.rect_filled(
                    ta_rect,
                    3.0,
                    egui::Color32::from_rgba_unmultiplied(255, 255, 255, 12),
                );
            }
            painter.text(
                ta_rect.center(),
                egui::Align2::CENTER_CENTER,
                ta_label,
                egui::FontId::monospace(10.0),
                ta_col,
            );

            // "compaction+" toggle -- segment turn axis at compactions (non-time mode only)
            if !*time_axis {
                let cp_label = if *compaction_plus {
                    "● compaction+"
                } else {
                    "○ compaction+"
                };
                let cp_col = if *compaction_plus {
                    Palette::TEXT_BRIGHT
                } else {
                    Palette::TEXT_DIM
                };
                let cp_rect = egui::Rect::from_min_size(
                    egui::pos2(ta_rect.right() + 8.0, cy - btn_size.y / 2.0),
                    egui::vec2(100.0, btn_size.y),
                );
                let cp_resp = ui.interact(
                    cp_rect,
                    egui::Id::new("ctrl_compaction_plus"),
                    egui::Sense::click(),
                );
                if cp_resp.clicked() {
                    *compaction_plus = !*compaction_plus;
                    *autofit = true;
                    // Fit turn view to the longest segment (compaction+ on) or longest
                    // session (compaction+ off). Data not yet rebuilt this frame, so
                    // compute directly from session events.
                    let max_x = if *compaction_plus {
                        // Max api calls within a single compaction segment.
                        let mut mx = 1.0_f64;
                        for s in &data.sessions {
                            if effective_hidden.contains(&s.session_id) {
                                continue;
                            }
                            let mut seg_len = 0usize;
                            for ev in &s.events {
                                match ev {
                                    Event::ApiCall { .. } => seg_len += 1,
                                    Event::Compaction { .. } => {
                                        mx = mx.max(seg_len as f64);
                                        seg_len = 0;
                                    }
                                    _ => {}
                                }
                            }
                            mx = mx.max(seg_len as f64);
                        }
                        mx
                    } else {
                        // Max total api calls in any single session.
                        data.sessions
                            .iter()
                            .filter(|s| !effective_hidden.contains(&s.session_id))
                            .map(|s| {
                                s.events
                                    .iter()
                                    .filter(|e| matches!(e, Event::ApiCall { .. }))
                                    .count() as f64
                            })
                            .fold(1.0_f64, f64::max)
                    };
                    *turn_view = Some((-0.5, max_x + 0.5));
                }
                if cp_resp.hovered() {
                    painter.rect_filled(
                        cp_rect,
                        3.0,
                        egui::Color32::from_rgba_unmultiplied(255, 255, 255, 12),
                    );
                }
                painter.text(
                    cp_rect.center(),
                    egui::Align2::CENTER_CENTER,
                    cp_label,
                    egui::FontId::monospace(10.0),
                    cp_col,
                );
            }

            // "fit" button -- resets navigator zoom in time mode
            if *time_axis {
                let af_rect = egui::Rect::from_min_size(
                    egui::pos2(ta_rect.right() + 8.0, cy - btn_size.y / 2.0),
                    egui::vec2(50.0, btn_size.y),
                );
                let af_resp =
                    ui.interact(af_rect, egui::Id::new("ctrl_autofit"), egui::Sense::click());
                if af_resp.clicked() {
                    *autofit = !*autofit;
                }
                if *autofit || af_resp.hovered() {
                    painter.rect_filled(
                        af_rect,
                        3.0,
                        egui::Color32::from_rgba_unmultiplied(255, 255, 255, 12),
                    );
                }
                let fit_col = if *autofit {
                    Palette::TEXT_BRIGHT
                } else {
                    Palette::TEXT_DIM
                };
                painter.text(
                    af_rect.center(),
                    egui::Align2::CENTER_CENTER,
                    "fit",
                    egui::FontId::monospace(10.0),
                    fit_col,
                );
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
            // +1 for the "skl" toggle rendered separately after the loop
            let vtog_total_w = (vis_toggles.len() + 1) as f32 * (vtog_w + vtog_gap) - vtog_gap;
            let mut vtog_x = inner.right() - vtog_total_w - 4.0;
            for (label, getter, toggler) in vis_toggles {
                let on = getter(chart_vis);
                let vrect = egui::Rect::from_min_size(
                    egui::pos2(vtog_x, cy - btn_size.y / 2.0),
                    egui::vec2(vtog_w, btn_size.y),
                );
                let vresp = ui.interact(
                    vrect,
                    egui::Id::new(("vis_toggle", *label)),
                    egui::Sense::click(),
                );
                if vresp.clicked() {
                    toggler(chart_vis);
                }
                if vresp.hovered() {
                    painter.rect_filled(
                        vrect,
                        3.0,
                        egui::Color32::from_rgba_unmultiplied(255, 255, 255, 12),
                    );
                }
                let vcol = if on {
                    Palette::TEXT_BRIGHT
                } else {
                    egui::Color32::from_rgba_unmultiplied(100, 90, 75, 120)
                };
                painter.text(
                    vrect.center(),
                    egui::Align2::CENTER_CENTER,
                    *label,
                    egui::FontId::monospace(9.0),
                    vcol,
                );
                vtog_x += vtog_w + vtog_gap;
            }
            // "skl" toggle — separate because clicking it also rebuilds the chart tree
            {
                let skl_rect = egui::Rect::from_min_size(
                    egui::pos2(vtog_x, cy - btn_size.y / 2.0),
                    egui::vec2(vtog_w, btn_size.y),
                );
                let skl_resp = ui.interact(
                    skl_rect,
                    egui::Id::new("vis_toggle_skl"),
                    egui::Sense::click(),
                );
                if skl_resp.clicked() {
                    chart_vis.tool = !chart_vis.tool;
                    *chart_tree = build_chart_tree(chart_vis.tool);
                }
                if skl_resp.hovered() {
                    painter.rect_filled(
                        skl_rect,
                        3.0,
                        egui::Color32::from_rgba_unmultiplied(255, 255, 255, 12),
                    );
                }
                let skl_col = if chart_vis.tool {
                    Palette::TEXT_BRIGHT
                } else {
                    egui::Color32::from_rgba_unmultiplied(100, 90, 75, 120)
                };
                painter.text(
                    skl_rect.center(),
                    egui::Align2::CENTER_CENTER,
                    "skl",
                    egui::FontId::monospace(9.0),
                    skl_col,
                );
            }
        });
    });

    // --- time navigator strip (always visible; pan/zoom only in time mode) ---
    // --- time navigator: custom-painted bar with viewport window ---
    // Compute full data range (all visible sessions)
    let mut all_x_min = f64::MAX;
    let mut all_x_max = f64::MIN;
    let mut all_dots: Vec<(f64, egui::Color32)> = vec![];
    // Nav strip covers the full dataset. Don't apply effective_hidden here --
    // in non-time mode the nav window itself drives hiding, so filtering here
    // would collapse full_min/full_max as the user zooms in (one-way shrink).
    // Still skip sessions removed by the user's explicit filter set so that
    // include/exclude lists narrow the navigator.
    let nav_hidden: HashSet<String> = match *filter_mode {
        FilterMode::Exclude => filter_set.clone(),
        FilterMode::Include => data
            .sessions
            .iter()
            .map(|s| s.session_id.clone())
            .filter(|id| !filter_set.contains(id))
            .collect(),
    };
    for (si, session) in data.sessions.iter().enumerate() {
        if nav_hidden.contains(&session.session_id) {
            continue;
        }
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
    if all_x_min > all_x_max {
        all_x_min = now_min - 60.0;
        all_x_max = now_min;
    }
    let data_span = (all_x_max - all_x_min).max(1.0);
    // Pad 2% on each side
    let time_full_min = all_x_min - data_span * 0.02;
    let time_full_max = all_x_max + data_span * 0.02;

    // Max turns across ALL sessions (unfiltered) so turn-axis full range is stable.
    let max_turns: f64 = data
        .sessions
        .iter()
        .map(|s| {
            s.events
                .iter()
                .filter(|e| matches!(e, Event::ApiCall { .. }))
                .count() as f64
        })
        .fold(1.0_f64, f64::max);

    let turn_full_min = -0.5_f64;
    let turn_full_max = max_turns + 0.5;

    // Chart x-range state: time_view in time mode, turn_view in non-time mode.
    let (full_min, full_max, view_min_span): (f64, f64, f64) = if is_time {
        (time_full_min, time_full_max, 5.0)
    } else {
        (turn_full_min, turn_full_max, 2.0)
    };
    let full_span = full_max - full_min;

    // Default turn_view to first 200 turns so initial zoom is readable.
    if turn_view.is_none() {
        *turn_view = Some((turn_full_min, 200.0_f64.min(turn_full_max)));
    }

    // Autofit: recompute each frame to track latest data.
    if *autofit && is_time {
        let mut fit_min = f64::MAX;
        let mut fit_max = f64::MIN;
        for session in &data.sessions {
            if effective_hidden.contains(&session.session_id) {
                continue;
            }
            if session.first_ts > 0 {
                fit_min = fit_min.min(session.first_ts as f64 / 60.0);
                fit_max = fit_max.max(session.last_ts as f64 / 60.0);
            }
        }
        if fit_min < fit_max {
            let span = (fit_max - fit_min).max(1.0);
            *time_view = Some((fit_min - span * 0.02, fit_max + span * 0.02));
        }
    }
    // Turn-mode autofit while a session is pinned: expand right edge as new turns arrive.
    if *autofit && !is_time {
        if let Some(fid) = focused.as_deref() {
            if let Some(session) = data.sessions.iter().find(|s| s.session_id == fid) {
                let api_count = session
                    .events
                    .iter()
                    .filter(|e| matches!(e, Event::ApiCall { .. }))
                    .count() as f64;
                *turn_view = Some((-0.5_f64, (api_count + 0.5).max(0.5)));
            }
        }
    }

    // Time nav strip: always visible, drives time_view.
    // In turn mode it filters visible sessions (last_ts ∈ window, applied in Hud::ui).
    let nm = now_min;
    let time_label = move |t: f64| -> String {
        let ago_min = nm - t;
        if ago_min.abs() < 1.0 { "now".into() }
        else if ago_min < 60.0 { format!("{}m", ago_min.round() as i64) }
        else if ago_min < 24.0 * 60.0 { format!("{:.0}h", ago_min / 60.0) }
        else { format!("{:.0}d", ago_min / (24.0 * 60.0)) }
    };
    let turn_label = |t: f64| -> String { format!("{}", t.max(0.0).round() as i64) };
    draw_nav_strip(
        ui,
        nav_rect,
        "nav_bar_time",
        time_view,
        time_full_min,
        time_full_max,
        5.0,
        autofit,
        &all_dots,
        &time_label,
    );
    if let Some(trect) = turn_nav_rect {
        draw_nav_strip(
            ui,
            trect,
            "nav_bar_turn",
            turn_view,
            turn_full_min,
            turn_full_max,
            2.0,
            autofit,
            &[],
            &turn_label,
        );
    }

    // Snapshot time_view value before reborrowing into active_view so usage/budget
    // (which are inherently time-based) can always drive their X off the time window.
    let time_view_snap: Option<(f64, f64)> = *time_view;
    let (time_vis_xmin, time_vis_xmax) = time_view_snap
        .unwrap_or((time_full_min, time_full_max));

    // Raw pointers to the two views so the focus handler can write to the
    // INACTIVE view while active_view holds the mutable borrow of the other.
    // Safety invariant: in each branch of the `if is_time` inside the focus
    // handler below, only the pointer to the non-active view is dereferenced.
    let time_view_ptr: *mut Option<(f64, f64)> = time_view as *mut _;
    let turn_view_ptr: *mut Option<(f64, f64)> = turn_view as *mut _;
    let active_view: &mut Option<(f64, f64)> = if is_time { time_view } else { turn_view };

    let _ = full_span;
    let _ = view_min_span;

    // --- legend (grouped by cwd) ---
    // Sort: groups with active sessions first, then by most-recent start time descending.
    groups.sort_by(|a, b| {
        let a_active = a.1.iter().any(|(si, _)| data.sessions[*si].is_active);
        let b_active = b.1.iter().any(|(si, _)| data.sessions[*si].is_active);
        let a_latest = a
            .1
            .iter()
            .map(|(si, _)| data.sessions[*si].first_ts)
            .max()
            .unwrap_or(0);
        let b_latest = b
            .1
            .iter()
            .map(|(si, _)| data.sessions[*si].first_ts)
            .max()
            .unwrap_or(0);
        b_active
            .cmp(&a_active)
            .then_with(|| b_latest.cmp(&a_latest))
    });
    // Within each group: active first, then by start time descending (most recent first).
    for (_, members) in &mut groups {
        members.sort_by(|a, b| {
            let sa = &data.sessions[a.0];
            let sb = &data.sessions[b.0];
            sb.is_active
                .cmp(&sa.is_active)
                .then_with(|| sb.first_ts.cmp(&sa.first_ts))
        });
    }

    // Legend panel (StripBuilder-based, see 2_legend.rs)
    let actions = legend::draw_legend_panel(
        ui,
        legend_rect,
        &data,
        &groups,
        filter_set,
        effective_hidden,
        expanded_groups,
        expanded_sessions,
        focused.as_deref(),
        week_start_secs,
        week_span,
    );
    // Extract focus_toggle before apply() consumes the struct.
    let focus_click = actions.focus_toggle.clone();
    actions.apply(filter_set, expanded_groups, expanded_sessions);
    if let Some(fid) = focus_click {
        // Toggle: clicking the currently-focused row clears focus.
        if focused.as_deref() == Some(&fid) {
            *focused = None;
            *active_view = None;
        } else {
            *focused = Some(fid.clone());

            // Fit both axes to the focused session immediately so the very
            // first frame after pinning shows the session in full.
            if let Some(session) = data.sessions.iter().find(|s| s.session_id == fid) {
                let api_count = session
                    .events
                    .iter()
                    .filter(|e| matches!(e, Event::ApiCall { .. }))
                    .count() as f64;
                let turn_fit = (-0.5_f64, (api_count + 0.5).max(0.5));

                let time_fit = if session.first_ts > 0 {
                    let tmin = session.first_ts as f64 / 60.0;
                    let tmax = session.last_ts as f64 / 60.0;
                    let span = (tmax - tmin).max(1.0);
                    Some((tmin - span * 0.02, tmax + span * 0.02))
                } else {
                    None
                };

                // active_view borrows exactly one of time_view / turn_view.
                // Use the raw pointers for the INACTIVE view (the one not
                // borrowed by active_view), which is always safe here.
                if is_time {
                    // active_view == &mut *time_view; turn_view_ptr is inactive.
                    if let Some(tf) = time_fit {
                        *active_view = Some(tf);
                    }
                    unsafe { *turn_view_ptr = Some(turn_fit) };
                } else {
                    // active_view == &mut *turn_view; time_view_ptr is inactive.
                    *active_view = Some(turn_fit);
                    if let Some(tf) = time_fit {
                        unsafe { *time_view_ptr = Some(tf) };
                    }
                }
            } else {
                *active_view = None;
            }
        }
        // autofit keeps time mode live-tracking while pinned (autofit block already
        // filters to visible=focused sessions only via effective_hidden).
        *autofit = true;
    }

    let cursor_id = egui::Id::new("all_charts_cursor");
    let hover_id = egui::Id::new("hud_hover_turn");
    let panel_hl_id = egui::Id::new("panel_highlight");
    let panel_hl: PanelHighlight = ui
        .ctx()
        .data(|d| d.get_temp(panel_hl_id).unwrap_or_default());

    // Clear hover state when pointer is outside chart areas or left the window.
    let mut all_charts_rect = egui::Rect::NOTHING;
    if chart_vis.cost {
        all_charts_rect = all_charts_rect.union(cost_rect);
    }
    if chart_vis.tokens {
        all_charts_rect = all_charts_rect.union(tok_rect);
    }
    if chart_vis.energy {
        all_charts_rect = all_charts_rect.union(energy_wh_rect);
    }
    if chart_vis.water {
        all_charts_rect = all_charts_rect.union(water_ml_rect);
    }
    if chart_vis.totals {
        all_charts_rect = all_charts_rect.union(totals_rect);
    }
    all_charts_rect = all_charts_rect.union(usage_rect); // usage/budget slot
    match ui.ctx().input(|i| i.pointer.hover_pos()) {
        None => {
            ui.ctx().data_mut(|d| d.remove::<HoverState>(hover_id));
        }
        Some(pos) if !all_charts_rect.contains(pos) => {
            ui.ctx().data_mut(|d| d.remove::<HoverState>(hover_id));
        }
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

    let visible_span = match *active_view {
        Some((vmin, vmax)) => vmax - vmin,
        None => full_span,
    };
    let hl_threshold = (visible_span * 0.03).max(if is_time { 2.0 } else { 0.6 });
    // Quantize hover x to hl_threshold steps so bar/line highlight only rebuilds
    // when the cursor crosses into a new session's range, not every pixel.
    let quantized_hx = effective_hover_x.map(|hx| (hx / hl_threshold).round() * hl_threshold);
    let hovered_sessions: Vec<bool> = if let Some(hx) = quantized_hx {
        cd.session_turns
            .iter()
            .map(|(_, _, turns)| turns.iter().any(|t| (t.x - hx).abs() < hl_threshold))
            .collect()
    } else {
        vec![]
    };

    // Time-mode visual parameters: defined once, used at every chart render site.
    // In time mode with many overlapping sessions, reduce line opacity/width to keep charts readable.
    // Hovered sessions always render at full brightness regardless.
    let line_alpha_default: f32 = 0.75;
    let line_width_default: f32 = 2.0;
    let totals_alpha_default: f32 = 0.80;
    let totals_width_default: f32 = 1.5;
    let show_markers = !is_time || !panel_hl.key.is_empty();

    let (vis_xmin, vis_xmax) = match *active_view {
        Some((vmin, vmax)) => (vmin, vmax),
        None => (full_min, full_max),
    };
    // Pin toggle is handled per-chart via plot_resp.response.clicked() after each show().

    // Screen-space containment check + source tracking + highlight VLine.
    // When pinned, draw the VLine at pinned x but don't update hover state (data stays frozen).
    let is_pinned = pinned_x.is_some();
    let update_hover_src = move |pui: &mut egui_plot::PlotUi, source: HoverSource| {
        // Draw highlight VLine at effective position (pinned or live).
        // In non-time mode, Budget uses time-x while other charts use turn-x.
        // Skip VLine when coordinate systems don't match to avoid auto-bounds explosion.
        let coords_match = is_time
            || match (prev_hover.as_ref().map(|hs| hs.source), source) {
                (Some(HoverSource::Budget), HoverSource::Budget) => true,
                (Some(HoverSource::Budget), _) | (_, HoverSource::Budget) => false,
                _ => true,
            };
        let vline_x = if is_pinned {
            pinned_x
        } else if coords_match {
            prev_hover.as_ref().map(|hs| hs.x)
        } else {
            None
        };
        if let Some(x) = vline_x {
            pui.vline(VLine::new("", x).color(hover_vline_color).width(1.0));
        }
        // When pinned, don't update hover state -- tooltip data stays frozen
        if is_pinned {
            return;
        }
        let Some(hover_pos) = pui.ctx().input(|i| i.pointer.hover_pos()) else {
            return;
        };
        let b = pui.plot_bounds();
        let s_min = pui.screen_from_plot(egui_plot::PlotPoint::new(b.min()[0], b.min()[1]));
        let s_max = pui.screen_from_plot(egui_plot::PlotPoint::new(b.max()[0], b.max()[1]));
        if !egui::Rect::from_two_pos(s_min, s_max).contains(hover_pos) {
            return;
        }
        let x = pui.plot_from_screen(hover_pos).x;
        pui.ctx()
            .data_mut(|d| d.insert_temp(hover_id, HoverState { x, source }));
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
                    let cursor = resp
                        .ctx
                        .input(|i| i.pointer.hover_pos())
                        .unwrap_or_default();
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
    let legend_hl_id = egui::Id::new("legend_highlight");
    let legend_hl: LegendHighlight = ui
        .ctx()
        .data(|d| d.get_temp(legend_hl_id).unwrap_or_default());
    let hl_color = egui::Color32::from_rgba_unmultiplied(220, 60, 60, 140);
    let draw_legend_hl = |pui: &mut egui_plot::PlotUi, hl: &LegendHighlight| {
        for (start, end) in &hl.ranges {
            pui.vline(VLine::new("", *start).color(hl_color).width(1.0));
            if (end - start).abs() > 0.5 {
                pui.vline(VLine::new("", *end).color(hl_color).width(1.0));
            }
        }
    };

    // --- cost per-turn chart with total cost overlay (right Y) ---
    if chart_vis.cost {
        ui.allocate_new_ui(egui::UiBuilder::new().max_rect(cost_rect), |ui| {
            panel_frame().show(ui, |ui| {
                let total_label = if let Some(last) = cd.combined_cost_pts.last() {
                    format!("total {}", format_cost(last[1]))
                } else {
                    String::new()
                };
                draw_chart_label(ui, "cost / turn", "input  cached  output", &total_label);
                let mut p = base_plot("cost_big")
                    .link_cursor(cursor_id, [true, false])
                    .include_y(cd.per_turn_in_cost_max)
                    .include_y(-cd.per_turn_out_cost_max)
                    .y_axis_formatter(move |v, _| {
                        let abs = v.value.abs();
                        if abs < 1e-9 {
                            String::new()
                        } else {
                            format_cost(abs)
                        }
                    })
                    .show_axes([false, true])
                    .show_grid(true);
                if let Some((vmin, vmax)) = *active_view {
                    p = p.include_x(vmin).include_x(vmax).auto_bounds([false, true]);
                }
                let plot_resp = p.show(ui, |pui| {
                    pui.set_plot_bounds_x(vis_xmin..=vis_xmax);
                    pui.set_auto_bounds([false, true]);
                    if *show_bars {
                        let fresh = BarChart::new(
                            "fresh$",
                            bars_culled(&cd.in_cost_fresh_bars, &hovered_sessions, vis_xmin, vis_xmax),
                        );
                        let read = BarChart::new(
                            "read$",
                            bars_culled(&cd.in_cost_cache_read_bars, &hovered_sessions, vis_xmin, vis_xmax),
                        )
                        .stack_on(&[&fresh]);
                        let create = BarChart::new(
                            "create$",
                            bars_culled(&cd.in_cost_cache_create_bars, &hovered_sessions, vis_xmin, vis_xmax),
                        )
                        .stack_on(&[&fresh, &read]);
                        pui.bar_chart(fresh);
                        pui.bar_chart(read);
                        pui.bar_chart(create);
                        pui.bar_chart(BarChart::new(
                            "gen$",
                            bars_culled(&cd.out_cost_bars, &hovered_sessions, vis_xmin, vis_xmax),
                        ));
                    }
                    for (si, (color, _)) in cd.total_cost_lines.iter().enumerate() {
                        let (alpha, w) = if hovered_sessions.is_empty() {
                            (line_alpha_default, line_width_default)
                        } else if hovered_sessions.get(si).copied().unwrap_or(false) {
                            (1.0, 2.5)
                        } else {
                            (0.12, 1.0)
                        };
                        let pts = &pc.cost_lines[si];
                        let c = scene_to_egui(*color).gamma_multiply(alpha);
                        pui.line(egui_plot::Line::new("", pts.as_slice()).color(c).width(w));
                        if pts.len() <= MAX_LINE_PTS {
                            pui.points(egui_plot::Points::new("", pts.as_slice()).color(c).radius(2.5).filled(true));
                        }
                    }
                    if show_markers {
                        render_egui::render_markers(
                            pui,
                            &scene::build_markers(
                                &cd.agent_xs,
                                &cd.skill_xs,
                                &cd.compaction_xs,
                                &panel_hl.key,
                            ),
                        );
                    }
                    if is_time { draw_legend_hl(pui, &legend_hl); }
                    update_hover_src(pui, HoverSource::Cost);
                });
                handle_chart_nav(
                    ui.ctx(),
                    &plot_resp.response,
                    plot_resp.transform.bounds(),
                    active_view,
                    full_min,
                    full_max,
                    view_min_span,
                    autofit,
                );
                try_pin(&plot_resp.response);
            });
        });
    }

    // --- token per-turn chart with total token overlay (right Y) ---
    if chart_vis.tokens {
        ui.allocate_new_ui(egui::UiBuilder::new().max_rect(tok_rect), |ui| {
            panel_frame().show(ui, |ui| {
                let total_tok_label = if cd.total_tok_max > 0.0 {
                    format!("total {}", format_tokens(cd.total_tok_max as u64))
                } else {
                    String::new()
                };
                draw_chart_label(ui, "tokens / turn", "in  out", &total_tok_label);
                let mut p = base_plot("tok_big")
                    .link_cursor(cursor_id, [true, false])
                    .auto_bounds([true, true])
                    .y_axis_formatter(move |v, _| {
                        let abs = v.value.abs();
                        if abs < 0.5 {
                            return String::new();
                        }
                        format_tokens(abs.round() as u64)
                    })
                    .y_grid_spacer(token_grid_spacer)
                    .show_axes([false, true])
                    .show_grid(true);
                if let Some((vmin, vmax)) = *active_view {
                    p = p.include_x(vmin).include_x(vmax).auto_bounds([false, true]);
                }
                let plot_resp = p.show(ui, |pui| {
                    pui.set_plot_bounds_x(vis_xmin..=vis_xmax);
                    pui.set_auto_bounds([false, true]);
                    if *show_bars {
                        let fresh = BarChart::new(
                            "fresh",
                            bars_culled(&cd.in_tok_fresh_bars, &hovered_sessions, vis_xmin, vis_xmax),
                        );
                        let read = BarChart::new(
                            "cached",
                            bars_culled(&cd.in_tok_cache_read_bars, &hovered_sessions, vis_xmin, vis_xmax),
                        );
                        let create = BarChart::new(
                            "create",
                            bars_culled(&cd.in_tok_cache_create_bars, &hovered_sessions, vis_xmin, vis_xmax),
                        );
                        let read = read.stack_on(&[&fresh]);
                        let create = create.stack_on(&[&fresh, &read]);
                        pui.bar_chart(fresh);
                        pui.bar_chart(read);
                        pui.bar_chart(create);
                        pui.bar_chart(BarChart::new(
                            "out",
                            bars_culled(&cd.out_tok_bars, &hovered_sessions, vis_xmin, vis_xmax),
                        ));
                    }
                    for (si, (color, _, _)) in cd.total_tok_lines.iter().enumerate() {
                        let (alpha, w) = if hovered_sessions.is_empty() {
                            (line_alpha_default, line_width_default)
                        } else if hovered_sessions.get(si).copied().unwrap_or(false) {
                            (1.0, 2.5)
                        } else {
                            (0.12, 1.0)
                        };
                        let in_pts = &pc.tok_in_lines[si];
                        let out_pts = &pc.tok_out_lines[si];
                        let c = scene_to_egui(*color).gamma_multiply(alpha);
                        let c_out = scene_to_egui(*color).gamma_multiply(alpha * 0.7);
                        pui.line(egui_plot::Line::new("in", in_pts.as_slice()).color(c).width(w));
                        if in_pts.len() <= MAX_LINE_PTS {
                            pui.points(egui_plot::Points::new("", in_pts.as_slice()).color(c).radius(2.5).filled(true));
                        }
                        pui.line(egui_plot::Line::new("out", out_pts.as_slice()).color(c_out).width(w * 0.6)
                            .style(egui_plot::LineStyle::Dashed { length: 6.0 }));
                        if out_pts.len() <= MAX_LINE_PTS {
                            pui.points(egui_plot::Points::new("", out_pts.as_slice()).color(c_out).radius(2.5).filled(true));
                        }
                    }
                    if is_time { draw_legend_hl(pui, &legend_hl); }
                    update_hover_src(pui, HoverSource::Tokens);
                });
                handle_chart_nav(
                    ui.ctx(),
                    &plot_resp.response,
                    plot_resp.transform.bounds(),
                    active_view,
                    full_min,
                    full_max,
                    view_min_span,
                    autofit,
                );
                try_pin(&plot_resp.response);
            });
        });
    }

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
                let period_cost_pts: Vec<[f64; 2]> = cd
                    .budget_cost_pts
                    .iter()
                    .filter(|p| p[0] >= period_start_x)
                    .copied()
                    .collect();
                let period_spent = period_cost_pts.last().map(|p| p[1]).unwrap_or(0.0)
                    - period_cost_pts.first().map(|p| p[1]).unwrap_or(0.0);
                let pct = if billing.limit_usd > 0.0 {
                    (period_spent / billing.limit_usd * 100.0).min(999.0)
                } else {
                    0.0
                };
                let pct_color = if pct > 90.0 {
                    egui::Color32::from_rgb(220, 60, 60)
                } else if pct > 70.0 {
                    egui::Color32::from_rgb(220, 160, 60)
                } else {
                    Palette::TEXT_BRIGHT
                };

                let label = format!(
                    "{} / {} ({:.0}%)",
                    format_cost(period_spent),
                    format_cost(billing.limit_usd),
                    pct
                );
                draw_chart_label(ui, "budget", &label, "");

                // Clickable config row: [day] [limit] controls
                let config_h = 14.0;
                let (config_rect, _) = ui.allocate_exact_size(
                    egui::vec2(ui.available_width(), config_h),
                    egui::Sense::hover(),
                );
                let painter = ui.painter();
                let font = egui::FontId::monospace(9.0);
                let cy = config_rect.center().y;

                // Reset controls: < day N > < HH > : < MM >
                // Day control
                let day_label = format!("day {}", billing.reset_day);
                let day_rect =
                    egui::Rect::from_min_size(config_rect.min, egui::vec2(68.0, config_h));
                painter.text(
                    day_rect.center(),
                    egui::Align2::CENTER_CENTER,
                    &day_label,
                    font.clone(),
                    Palette::TEXT_DIM,
                );
                let day_dec = egui::Rect::from_min_size(
                    egui::pos2(day_rect.left(), cy - 6.0),
                    egui::vec2(10.0, 12.0),
                );
                let day_inc = egui::Rect::from_min_size(
                    egui::pos2(day_rect.right() - 10.0, cy - 6.0),
                    egui::vec2(10.0, 12.0),
                );
                if ui.interact(day_dec, egui::Id::new("budget_day_dec"), egui::Sense::click()).clicked() {
                    billing.reset_day = if billing.reset_day <= 1 { 28 } else { billing.reset_day - 1 };
                    billing.save();
                }
                if ui.interact(day_inc, egui::Id::new("budget_day_inc"), egui::Sense::click()).clicked() {
                    billing.reset_day = if billing.reset_day >= 28 { 1 } else { billing.reset_day + 1 };
                    billing.save();
                }
                painter.text(day_dec.center(), egui::Align2::CENTER_CENTER, "<", font.clone(), Palette::TEXT_BRIGHT);
                painter.text(day_inc.center(), egui::Align2::CENTER_CENTER, ">", font.clone(), Palette::TEXT_BRIGHT);

                // Hour control
                let hour_label = format!("{:02}", billing.reset_hour);
                let hr_x = day_rect.right() + 4.0;
                let hr_rect = egui::Rect::from_min_size(egui::pos2(hr_x, config_rect.top()), egui::vec2(44.0, config_h));
                painter.text(hr_rect.center(), egui::Align2::CENTER_CENTER, &hour_label, font.clone(), Palette::TEXT_DIM);
                let hr_dec = egui::Rect::from_min_size(egui::pos2(hr_rect.left(), cy - 6.0), egui::vec2(10.0, 12.0));
                let hr_inc = egui::Rect::from_min_size(egui::pos2(hr_rect.right() - 10.0, cy - 6.0), egui::vec2(10.0, 12.0));
                if ui.interact(hr_dec, egui::Id::new("budget_hr_dec"), egui::Sense::click()).clicked() {
                    billing.reset_hour = if billing.reset_hour == 0 { 23 } else { billing.reset_hour - 1 };
                    billing.save();
                }
                if ui.interact(hr_inc, egui::Id::new("budget_hr_inc"), egui::Sense::click()).clicked() {
                    billing.reset_hour = if billing.reset_hour >= 23 { 0 } else { billing.reset_hour + 1 };
                    billing.save();
                }
                painter.text(hr_dec.center(), egui::Align2::CENTER_CENTER, "<", font.clone(), Palette::TEXT_BRIGHT);
                painter.text(hr_inc.center(), egui::Align2::CENTER_CENTER, ">", font.clone(), Palette::TEXT_BRIGHT);

                // Colon separator
                let colon_x = hr_rect.right() + 1.0;
                painter.text(egui::pos2(colon_x + 3.0, cy), egui::Align2::LEFT_CENTER, ":", font.clone(), Palette::TEXT_DIM);

                // Minute control (15-min steps)
                let min_label = format!("{:02}", billing.reset_minute);
                let mn_x = colon_x + 8.0;
                let mn_rect = egui::Rect::from_min_size(egui::pos2(mn_x, config_rect.top()), egui::vec2(44.0, config_h));
                painter.text(mn_rect.center(), egui::Align2::CENTER_CENTER, &min_label, font.clone(), Palette::TEXT_DIM);
                let mn_dec = egui::Rect::from_min_size(egui::pos2(mn_rect.left(), cy - 6.0), egui::vec2(10.0, 12.0));
                let mn_inc = egui::Rect::from_min_size(egui::pos2(mn_rect.right() - 10.0, cy - 6.0), egui::vec2(10.0, 12.0));
                if ui.interact(mn_dec, egui::Id::new("budget_mn_dec"), egui::Sense::click()).clicked() {
                    billing.reset_minute = if billing.reset_minute < 15 { 45 } else { billing.reset_minute - 15 };
                    billing.save();
                }
                if ui.interact(mn_inc, egui::Id::new("budget_mn_inc"), egui::Sense::click()).clicked() {
                    billing.reset_minute = if billing.reset_minute >= 45 { 0 } else { billing.reset_minute + 15 };
                    billing.save();
                }
                painter.text(mn_dec.center(), egui::Align2::CENTER_CENTER, "<", font.clone(), Palette::TEXT_BRIGHT);
                painter.text(mn_inc.center(), egui::Align2::CENTER_CENTER, ">", font.clone(), Palette::TEXT_BRIGHT);

                // (painter ref ends here -- mutable ui calls below need exclusive access)

                // Limit input row
                let input_h = 20.0;
                let (lim_row, _) = ui.allocate_exact_size(
                    egui::vec2(ui.available_width(), input_h),
                    egui::Sense::hover(),
                );
                let lim_cy = lim_row.center().y;
                let lim_input_rect = egui::Rect::from_min_size(
                    egui::pos2(lim_row.left() + 40.0, lim_row.top() + 1.0),
                    egui::vec2(80.0, input_h - 2.0),
                );
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
                ui.painter().text(
                    egui::pos2(lim_row.left() + 2.0, lim_cy),
                    egui::Align2::LEFT_CENTER,
                    "limit",
                    font.clone(),
                    Palette::TEXT_DIM,
                );

                // Web-reported total row
                let web_h = 20.0;
                let (web_row, _) = ui.allocate_exact_size(
                    egui::vec2(ui.available_width(), web_h),
                    egui::Sense::hover(),
                );
                let web_cy = web_row.center().y;
                let input_rect = egui::Rect::from_min_size(
                    egui::pos2(web_row.left() + 40.0, web_row.top() + 1.0),
                    egui::vec2(80.0, web_h - 2.0),
                );
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
                            .duration_since(std::time::UNIX_EPOCH)
                            .unwrap()
                            .as_secs();
                        billing.web_reported = Some((val, now_epoch));
                        billing.save();
                    }
                }
                // Clear button interaction (must happen before re-borrowing painter)
                let x_rect = egui::Rect::from_min_size(
                    egui::pos2(web_row.right() - 16.0, web_cy - 6.0),
                    egui::vec2(12.0, 12.0),
                );
                let clear_clicked = if billing.web_reported.is_some() {
                    ui.interact(
                        x_rect,
                        egui::Id::new("budget_web_clear"),
                        egui::Sense::click(),
                    )
                    .clicked()
                } else {
                    false
                };
                if clear_clicked {
                    billing.web_reported = None;
                    billing.web_input_buf.clear();
                    billing.save();
                }
                // Now paint text (re-borrow painter)
                let web_label_rect = egui::Rect::from_min_size(
                    egui::pos2(web_row.left(), web_row.top()),
                    egui::vec2(38.0, web_h),
                );
                let painter = ui.painter();
                let web_color = egui::Color32::from_rgb(180, 140, 220);
                painter.text(
                    egui::pos2(web_row.left() + 2.0, web_cy),
                    egui::Align2::LEFT_CENTER,
                    "web $",
                    font.clone(),
                    web_color,
                );
                let info_x = web_row.left() + 118.0;
                if let Some((val, ts)) = billing.web_reported {
                    let ago_min = (std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .unwrap()
                        .as_secs() as f64
                        - ts as f64)
                        / 60.0;
                    let ago = if ago_min < 1.0 {
                        "just now".into()
                    } else if ago_min < 60.0 {
                        format!("{:.0}m ago", ago_min)
                    } else if ago_min < 24.0 * 60.0 {
                        format!("{:.1}h ago", ago_min / 60.0)
                    } else {
                        format!("{:.1}d ago", ago_min / (24.0 * 60.0))
                    };
                    painter.text(
                        egui::pos2(info_x, web_cy),
                        egui::Align2::LEFT_CENTER,
                        &format!("{} as of {}", format_cost(val), ago),
                        font.clone(),
                        web_color,
                    );
                    painter.text(
                        x_rect.center(),
                        egui::Align2::CENTER_CENTER,
                        "x",
                        font.clone(),
                        Palette::TEXT_DIM,
                    );
                } else {
                    painter.text(
                        egui::pos2(info_x, web_cy),
                        egui::Align2::LEFT_CENTER,
                        "enter total from web, press enter",
                        font.clone(),
                        Palette::TEXT_DIM,
                    );
                }
                ui.interact(web_label_rect, egui::Id::new("web_label_tip"), egui::Sense::hover())
                    .on_hover_text(
                        "Current total as reported by claude.ai/usage for this billing period.\n\
                         Claude Code only sees API calls made locally; the web total includes usage\n\
                         across the web app, API keys, and integrations. Enter this value manually\n\
                         and press Enter to anchor the budget chart to the correct baseline.",
                    );

                // The chart: cumulative cost within period, zeroed at period start
                if !period_cost_pts.is_empty() {
                    let base_cost = period_cost_pts.first().map(|p| p[1]).unwrap_or(0.0);
                    let zeroed: Vec<[f64; 2]> = period_cost_pts
                        .iter()
                        .map(|[x, y]| [*x, y - base_cost])
                        .collect();

                    let budget_y_fmt = move |v: egui_plot::GridMark,
                                             _: &std::ops::RangeInclusive<f64>|
                          -> String {
                        if v.value < 0.001 {
                            String::new()
                        } else {
                            format_cost(v.value)
                        }
                    };

                    let mut p = Plot::new("budget_period")
                        .show_axes([true, true])
                        .show_grid(true)
                        .allow_zoom(false)
                        .allow_drag(false)
                        .allow_scroll(false)
                        .show_background(false)
                        .set_margin_fraction(egui::Vec2::ZERO)
                        .auto_bounds([true, true])
                        .include_y(0.0)
                        .include_y(billing.limit_usd * 1.05)
                        .y_axis_formatter(budget_y_fmt)
                        .x_axis_formatter(time_x_fmt);
                    // Budget is always time-based: share the time window regardless of mode.
                    p = p.link_cursor(cursor_id, [true, false]);
                    p = p.include_x(time_vis_xmin).include_x(time_vis_xmax).auto_bounds([false, true]);
                    // If web reported total > CLI total, include it in Y range
                    if let Some((web_val, _)) = billing.web_reported {
                        p = p.include_y(web_val * 1.05);
                    }
                    let web_reported_for_chart = billing.web_reported;
                    let period_start_for_chart = period_start_x;
                    let plot_resp = p.show(ui, |pui| {
                    pui.set_plot_bounds_x(time_vis_xmin..=time_vis_xmax);
                    pui.set_auto_bounds([false, true]);
                        let pts = step_pts(&zeroed); // budget always uses time-based x
                        pui.line(
                            egui_plot::Line::new("CLI spent", pts)
                                .color(pct_color)
                                .width(2.0)
                                .fill(0.0),
                        );
                        // Budget limit line
                        pui.hline(
                            egui_plot::HLine::new("", billing.limit_usd)
                                .color(egui::Color32::from_rgba_unmultiplied(200, 60, 60, 120))
                                .width(1.0),
                        );
                        // Period start marker
                        pui.vline(
                            VLine::new("", period_start_for_chart)
                                .color(egui::Color32::from_rgba_unmultiplied(100, 180, 100, 60))
                                .width(0.5),
                        );
                        // Web-reported marker: horizontal line at reported value + point marker at report time
                        if let Some((web_val, web_ts)) = web_reported_for_chart {
                            let web_x = web_ts as f64 / 60.0;
                            let web_color = egui::Color32::from_rgb(180, 140, 220); // purple
                                                                                    // Horizontal line at web-reported value
                            pui.hline(
                                egui_plot::HLine::new("", web_val)
                                    .color(egui::Color32::from_rgba_unmultiplied(180, 140, 220, 80))
                                    .width(1.0),
                            );
                            // Point marker at the time it was reported
                            pui.points(
                                egui_plot::Points::new("", vec![[web_x, web_val]])
                                    .color(web_color)
                                    .radius(4.0)
                                    .name("web reported"),
                            );
                            // Vertical line at report time
                            pui.vline(
                                VLine::new("", web_x)
                                    .color(egui::Color32::from_rgba_unmultiplied(180, 140, 220, 40))
                                    .width(0.5),
                            );
                            // If CLI cost at that time is lower, find the CLI value at web_x and show the gap
                            let cli_at_web_x = zeroed
                                .iter()
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
                                pui.polygon(
                                    egui_plot::Polygon::new("", gap_pts)
                                        .fill_color(egui::Color32::from_rgba_unmultiplied(
                                            180, 140, 220, 30,
                                        ))
                                        .name(format!(
                                            "non-CLI: ~{}",
                                            format_cost(web_val - cli_at_web_x)
                                        )),
                                );
                            }
                        }
                        draw_legend_hl(pui, &legend_hl);
                        update_hover_src(pui, HoverSource::Budget);
                    });
                    try_pin(&plot_resp.response);
                    // Budget is time-only: drive time_view directly via active_view only when is_time.
                    // In non-time mode, use the always-visible time nav strip instead.
                    if is_time {
                        handle_chart_nav(
                            ui.ctx(),
                            &plot_resp.response,
                            plot_resp.transform.bounds(),
                            active_view,
                            time_full_min,
                            time_full_max,
                            5.0,
                            autofit,
                        );
                    }
                } else {
                    let inner = ui.available_rect_before_wrap();
                    let msg = "no data in billing period";
                    ui.painter().text(
                        inner.center(),
                        egui::Align2::CENTER_CENTER,
                        msg,
                        egui::FontId::monospace(9.0),
                        Palette::TEXT_DIM,
                    );
                }
            } else {
                // --- usage chart: 5h + 7d utilization over time ---
                let usage_now_label = usage
                    .latest
                    .as_ref()
                    .map(|l| format!("5h {}%  7d {}%", l.five_hour as u32, l.seven_day as u32))
                    .unwrap_or_default();
                draw_chart_label(ui, "usage %", &usage_now_label, "");

                if usage.snapshots.len() >= 2 {
                    let five_h_pts: Vec<[f64; 2]> = usage
                        .snapshots
                        .iter()
                        .map(|s| [s.ts as f64 / 60.0, s.five_hour])
                        .collect();
                    let seven_d_pts: Vec<[f64; 2]> = usage
                        .snapshots
                        .iter()
                        .map(|s| [s.ts as f64 / 60.0, s.seven_day])
                        .collect();

                    let usage_time_fmt = move |v: egui_plot::GridMark,
                                               _: &std::ops::RangeInclusive<f64>|
                          -> String {
                        let ago_min = now_min - v.value;
                        if ago_min < 0.5 {
                            "now".into()
                        } else if ago_min < 60.0 {
                            format!("{}m", ago_min.round() as i64)
                        } else if ago_min < 24.0 * 60.0 {
                            format!("{:.0}h", ago_min / 60.0)
                        } else {
                            format!("{:.0}d", ago_min / (24.0 * 60.0))
                        }
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
                            let ago = if ago_min < 1.0 {
                                "now".into()
                            } else if ago_min < 60.0 {
                                format!("{}m ago", ago_min.round() as i64)
                            } else if ago_min < 24.0 * 60.0 {
                                format!("{:.1}h ago", ago_min / 60.0)
                            } else {
                                format!("{:.1}d ago", ago_min / (24.0 * 60.0))
                            };
                            let mut tip = format!(
                                "{}\n5h: {:.0}%\n7d: {:.0}%",
                                ago, s.five_hour, s.seven_day
                            );
                            if let Some(opus) = s.seven_day_opus {
                                tip += &format!("\nopus 7d: {:.0}%", opus);
                            }
                            if let Some(sonnet) = s.seven_day_sonnet {
                                tip += &format!("\nsonnet 7d: {:.0}%", sonnet);
                            }
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
                        .auto_bounds([true, true])
                        .include_y(0.0)
                        .include_y(100.0)
                        .y_axis_formatter(|v, _| {
                            if v.value < 0.5 {
                                String::new()
                            } else {
                                format!("{}%", v.value as u32)
                            }
                        })
                        .x_axis_formatter(usage_time_fmt)
                        .label_formatter(tip_fmt);
                    // Usage is always time-based: use the time window regardless of mode.
                    p = p.include_x(time_vis_xmin).include_x(time_vis_xmax).auto_bounds([false, true]);
                    let plot_resp = p.show(ui, |pui| {
                    pui.set_plot_bounds_x(time_vis_xmin..=time_vis_xmax);
                    pui.set_auto_bounds([false, true]);
                        pui.line(
                            egui_plot::Line::new("5h", five_h_pts)
                                .color(egui::Color32::from_rgb(220, 160, 60))
                                .width(1.5),
                        );
                        pui.line(
                            egui_plot::Line::new("7d", seven_d_pts)
                                .color(egui::Color32::from_rgb(100, 160, 220))
                                .width(1.5),
                        );
                        pui.hline(
                            egui_plot::HLine::new("", 100.0)
                                .color(egui::Color32::from_rgba_unmultiplied(200, 60, 60, 80))
                                .width(0.5),
                        );
                    });
                    // Usage is time-only: drive active_view (which is time_view) only in time mode.
                    if is_time {
                        handle_chart_nav(
                            ui.ctx(),
                            &plot_resp.response,
                            plot_resp.transform.bounds(),
                            active_view,
                            time_full_min,
                            time_full_max,
                            5.0,
                            autofit,
                        );
                    }
                } else if let Some(e) = &usage.error {
                    let inner = ui.available_rect_before_wrap();
                    ui.painter().text(
                        inner.center(),
                        egui::Align2::CENTER_CENTER,
                        e,
                        egui::FontId::monospace(9.0),
                        Palette::TEXT_DIM,
                    );
                } else {
                    let inner = ui.available_rect_before_wrap();
                    ui.painter().text(
                        inner.center(),
                        egui::Align2::CENTER_CENTER,
                        "polling...",
                        egui::FontId::monospace(9.0),
                        Palette::TEXT_DIM,
                    );
                }
            }
        });
    });

    // --- skill / agent / reads / tool breakdown (scrollable, via scene tree) ---
    if chart_vis.tool {
        // Clear session hover tooltip when pointer is over tool panel
        if let Some(pos) = ui.ctx().input(|i| i.pointer.hover_pos()) {
            if tool_rect.contains(pos) {
                ui.ctx().data_mut(|d| d.remove::<HoverState>(hover_id));
            }
        }
        let panel_hl_id = egui::Id::new("panel_highlight");
        ui.ctx()
            .data_mut(|d| d.insert_temp(panel_hl_id, PanelHighlight::default()));
        let panel_nodes = scene::build_tool_panel(
            &cd.skill_list,
            &cd.agent_list,
            &cd.read_list,
            &cd.tool_list,
            &panel_hl.key,
        );
        ui.allocate_new_ui(egui::UiBuilder::new().max_rect(tool_rect), |ui| {
            panel_frame().show(ui, |ui| {
                ui.style_mut().visuals.override_text_color = Some(Palette::TEXT);
                ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Truncate);
                let hovered_key = render_egui::render(ui, &panel_nodes);
                if !hovered_key.is_empty() {
                    ui.ctx().data_mut(|d| {
                        d.get_temp_mut_or_default::<PanelHighlight>(panel_hl_id).key = hovered_key
                    });
                }
            });
        });
    }

    // --- Row 3: per-turn energy (Wh) + per-turn water (mL), each with cumulative overlay ---
    let total_wh: f64 = cd
        .total_energy_lines
        .iter()
        .filter_map(|(_, pts)| pts.last().map(|p| p[1]))
        .sum();
    let total_water_ml: f64 = cd
        .total_water_lines
        .iter()
        .filter_map(|(_, pts)| pts.last().map(|p| p[1]))
        .sum();

    // Silly unit equivalences
    let wh_silly = if total_wh > 12.0 {
        format!("{:.1} phone charges", total_wh / 12.0)
    } else if total_wh > 0.01 {
        format!("{:.1} LED-bulb hrs", total_wh / 10.0)
    } else {
        String::new()
    };
    // 1 sip of water ~30 mL, 1 gulp ~60 mL, 1 cup = 237 mL
    let water_silly = if total_water_ml > 237.0 {
        format!("{:.1} cups", total_water_ml / 237.0)
    } else if total_water_ml > 30.0 {
        format!("{:.0} sips", total_water_ml / 30.0)
    } else if total_water_ml > 0.01 {
        format!("{:.1} drops", total_water_ml / 0.05)
    } else {
        String::new()
    };

    if chart_vis.energy {
        ui.allocate_new_ui(egui::UiBuilder::new().max_rect(energy_wh_rect), |ui| {
            panel_frame().show(ui, |ui| {
                let wh_label = if total_wh > 0.001 {
                    format!("{:.2} Wh total", total_wh)
                } else {
                    String::new()
                };
                draw_chart_label(ui, "energy / turn", &wh_label, &wh_silly);
                let mut p = base_plot("energy_wh")
                    .link_cursor(cursor_id, [true, false])
                    .include_y(0.0)
                    .include_y(cd.energy_wh_max)
                    .y_axis_formatter(move |v, _| {
                        if v.value < 1e-9 {
                            String::new()
                        } else {
                            format!("{:.2}", v.value)
                        }
                    })
                    .show_axes([false, true])
                    .show_grid(true);
                if let Some((vmin, vmax)) = *active_view {
                    p = p.include_x(vmin).include_x(vmax).auto_bounds([false, true]);
                }
                let plot_resp = p.show(ui, |pui| {
                    pui.set_plot_bounds_x(vis_xmin..=vis_xmax);
                    pui.set_auto_bounds([false, true]);
                    if *show_bars {
                        pui.bar_chart(BarChart::new(
                            "Wh",
                            bars_culled(&cd.energy_wh_bars, &hovered_sessions, vis_xmin, vis_xmax),
                        ));
                    }
                    for (si, (color, _)) in cd.total_energy_lines.iter().enumerate() {
                        let (alpha, w) = if hovered_sessions.is_empty() {
                            (line_alpha_default, line_width_default)
                        } else if hovered_sessions.get(si).copied().unwrap_or(false) {
                            (1.0, 2.5)
                        } else {
                            (0.12, 1.0)
                        };
                        let pts = &pc.energy_lines[si];
                        let c = scene_to_egui(*color).gamma_multiply(alpha);
                        pui.line(egui_plot::Line::new("", pts.as_slice()).color(c).width(w));
                        if pts.len() <= MAX_LINE_PTS {
                            pui.points(egui_plot::Points::new("", pts.as_slice()).color(c).radius(2.5).filled(true));
                        }
                    }
                    if is_time { draw_legend_hl(pui, &legend_hl); }
                    update_hover_src(pui, HoverSource::Energy);
                });
                handle_chart_nav(
                    ui.ctx(),
                    &plot_resp.response,
                    plot_resp.transform.bounds(),
                    active_view,
                    full_min,
                    full_max,
                    view_min_span,
                    autofit,
                );
                try_pin(&plot_resp.response);
            });
        });
    }

    if chart_vis.water {
        ui.allocate_new_ui(egui::UiBuilder::new().max_rect(water_ml_rect), |ui| {
            panel_frame().show(ui, |ui| {
                let water_label = if total_water_ml > 0.001 {
                    format!("{:.1} mL total", total_water_ml)
                } else {
                    String::new()
                };
                draw_chart_label(ui, "water / turn", &water_label, &water_silly);
                let mut p = base_plot("water_ml")
                    .link_cursor(cursor_id, [true, false])
                    .include_y(0.0)
                    .include_y(cd.water_ml_max)
                    .y_axis_formatter(move |v, _| {
                        if v.value < 1e-9 {
                            String::new()
                        } else {
                            format!("{:.2}", v.value)
                        }
                    })
                    .show_axes([false, true])
                    .show_grid(true);
                if let Some((vmin, vmax)) = *active_view {
                    p = p.include_x(vmin).include_x(vmax).auto_bounds([false, true]);
                }
                let plot_resp = p.show(ui, |pui| {
                    pui.set_plot_bounds_x(vis_xmin..=vis_xmax);
                    pui.set_auto_bounds([false, true]);
                    if *show_bars {
                        pui.bar_chart(BarChart::new(
                            "mL",
                            bars_culled(&cd.water_ml_bars, &hovered_sessions, vis_xmin, vis_xmax),
                        ));
                    }
                    for (si, (color, _)) in cd.total_water_lines.iter().enumerate() {
                        let (alpha, w) = if hovered_sessions.is_empty() {
                            (line_alpha_default, line_width_default)
                        } else if hovered_sessions.get(si).copied().unwrap_or(false) {
                            (1.0, 2.5)
                        } else {
                            (0.12, 1.0)
                        };
                        let pts = &pc.water_lines[si];
                        let c = scene_to_egui(*color).gamma_multiply(alpha);
                        pui.line(egui_plot::Line::new("", pts.as_slice()).color(c).width(w));
                        if pts.len() <= MAX_LINE_PTS {
                            pui.points(egui_plot::Points::new("", pts.as_slice()).color(c).radius(2.5).filled(true));
                        }
                    }
                    if is_time { draw_legend_hl(pui, &legend_hl); }
                    update_hover_src(pui, HoverSource::Water);
                });
                handle_chart_nav(
                    ui.ctx(),
                    &plot_resp.response,
                    plot_resp.transform.bounds(),
                    active_view,
                    full_min,
                    full_max,
                    view_min_span,
                    autofit,
                );
                try_pin(&plot_resp.response);
            });
        });
    }

    // --- bottom row: unified totals (cost + tokens + energy + water, all normalized) ---
    if chart_vis.totals {
        ui.allocate_new_ui(egui::UiBuilder::new().max_rect(totals_rect), |ui| {
            panel_frame().show(ui, |ui| {
                // Current values for labels
                let cur_cost = cd.combined_cost_pts.last().map(|p| p[1]).unwrap_or(0.0);
                let cur_tok = cd.total_tok_max;
                let cur_wh = total_wh;
                let cur_water = total_water_ml;
                let label = format!(
                    "{}  {}tok  {:.1}Wh  {:.0}mL",
                    format_cost(cur_cost),
                    format_tokens(cur_tok as u64),
                    cur_wh,
                    cur_water
                );
                draw_chart_label(ui, "totals", &label, "");

                // Normalize all series to [0..1] range, then plot in [0..1] Y space
                let mut p = base_plot("totals_combined")
                    .link_cursor(cursor_id, [true, false])
                    .include_y(0.0)
                    .include_y(1.05)
                    .y_axis_formatter(move |v, _| {
                        // Left Y shows cost scale
                        if v.value < 1e-9 {
                            String::new()
                        } else {
                            format_cost(v.value * cur_cost)
                        }
                    })
                    .show_axes([is_time, true])
                    .show_grid(true);
                if is_time {
                    p = p.x_axis_formatter(time_x_fmt);
                }
                if let Some((vmin, vmax)) = *active_view {
                    p = p.include_x(vmin).include_x(vmax).auto_bounds([false, true]);
                }
                let cost_color = egui::Color32::from_rgb(220, 180, 60); // gold
                let tok_color = egui::Color32::from_rgb(100, 160, 220); // blue
                let energy_color = egui::Color32::from_rgb(120, 200, 80); // green
                let water_color = egui::Color32::from_rgb(80, 180, 220); // cyan

                let plot_resp = p.show(ui, |pui| {
                    pui.set_plot_bounds_x(vis_xmin..=vis_xmax);
                    pui.set_auto_bounds([false, true]);
                    if !pc.totals_cost.is_empty() {
                        pui.line(egui_plot::Line::new("cost", pc.totals_cost.as_slice()).color(cost_color).width(2.0));
                        if pc.totals_cost.len() <= MAX_LINE_PTS {
                            pui.points(egui_plot::Points::new("", pc.totals_cost.as_slice()).color(cost_color).radius(2.0).filled(true));
                        }
                    }
                    for (si, _) in cd.total_tok_lines.iter().enumerate() {
                        let (alpha, w) = if hovered_sessions.is_empty() {
                            (totals_alpha_default, totals_width_default)
                        } else if hovered_sessions.get(si).copied().unwrap_or(false) {
                            (1.0, 2.5)
                        } else {
                            (0.12, 1.0)
                        };
                        let pts = &pc.totals_tok[si];
                        if !pts.is_empty() {
                            let c = tok_color.gamma_multiply(alpha);
                            pui.line(egui_plot::Line::new("tokens", pts.as_slice()).color(c).width(w));
                            if pts.len() <= MAX_LINE_PTS {
                                pui.points(egui_plot::Points::new("", pts.as_slice()).color(c).radius(2.0).filled(true));
                            }
                        }
                    }
                    for (si, _) in cd.total_energy_lines.iter().enumerate() {
                        let (alpha, w) = if hovered_sessions.is_empty() {
                            (totals_alpha_default, totals_width_default)
                        } else if hovered_sessions.get(si).copied().unwrap_or(false) {
                            (1.0, 2.5)
                        } else {
                            (0.12, 1.0)
                        };
                        let pts = &pc.totals_energy[si];
                        if !pts.is_empty() {
                            let c = energy_color.gamma_multiply(alpha);
                            pui.line(egui_plot::Line::new("energy", pts.as_slice()).color(c).width(w));
                            if pts.len() <= MAX_LINE_PTS {
                                pui.points(egui_plot::Points::new("", pts.as_slice()).color(c).radius(2.0).filled(true));
                            }
                        }
                    }
                    for (si, _) in cd.total_water_lines.iter().enumerate() {
                        let (alpha, w) = if hovered_sessions.is_empty() {
                            (totals_alpha_default, totals_width_default)
                        } else if hovered_sessions.get(si).copied().unwrap_or(false) {
                            (1.0, 2.5)
                        } else {
                            (0.12, 1.0)
                        };
                        let pts = &pc.totals_water[si];
                        if !pts.is_empty() {
                            let c = water_color.gamma_multiply(alpha);
                            pui.line(egui_plot::Line::new("water", pts.as_slice()).color(c).width(w));
                            if pts.len() <= MAX_LINE_PTS {
                                pui.points(egui_plot::Points::new("", pts.as_slice()).color(c).radius(2.0).filled(true));
                            }
                        }
                    }
                    if is_time { draw_legend_hl(pui, &legend_hl); }
                    update_hover_src(pui, HoverSource::WeeklyCost);
                });
                handle_chart_nav(
                    ui.ctx(),
                    &plot_resp.response,
                    plot_resp.transform.bounds(),
                    active_view,
                    full_min,
                    full_max,
                    view_min_span,
                    autofit,
                );
                try_pin(&plot_resp.response);
            });
        });
    }

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
        let cursor_opt = if is_pinned {
            pinned_cursor_pos
        } else {
            ui.ctx().input(|i| i.pointer.hover_pos())
        };
        if let Some(cursor) = cursor_opt {
            // (session_name, session_color, detail_text, optional breakdown fracs, sort_key)
            let mut entries: Vec<(String, egui::Color32, String, Option<[f32; 3]>, f64)> = vec![];
            // Context info from nearest turn (for footer): (context_tokens, context_limit, burn_rate_per_turn, is_reset)
            let mut context_footer: Option<(u64, u64, f64, bool)> = None;
            // Budget always uses time-based x, even in non-time mode
            let use_ts_x = matches!(hs.source, HoverSource::Budget) && !is_time;
            // Closest turn across all sessions — used by the V-key turn viewer shortcut.
            // (meta_idx, api_call_idx, display_name, distance).
            let mut viewer_hit: Option<(usize, u32, String, f64)> = None;
            for (meta_idx, (name, sess_color, turns)) in cd.session_turns.iter().enumerate() {
                if turns.is_empty() {
                    continue;
                }
                // Skip sessions where hover x is outside the session's data range (with small margin)
                let tx = |t: &TurnInfo| if use_ts_x { t.ts_x } else { t.x };
                let first_x = tx(turns.first().unwrap());
                let last_x = tx(turns.last().unwrap());
                let span = (last_x - first_x).max(1.0);
                let margin = span * 0.05; // 5% margin at edges
                if hx < first_x - margin || hx > last_x + margin {
                    continue;
                }

                let nearest = turns.iter().enumerate().min_by(|(_, a), (_, b)| {
                    (tx(a) - hx).abs().partial_cmp(&(tx(b) - hx).abs()).unwrap()
                });
                if let Some((idx, t)) = nearest {
                    let dist = (tx(t) - hx).abs();
                    let beat = viewer_hit
                        .as_ref()
                        .map(|(_, _, _, d)| dist < *d)
                        .unwrap_or(true);
                    if beat {
                        viewer_hit =
                            Some((meta_idx, t.api_call_idx, name.clone(), dist));
                    }
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
                    context_footer =
                        Some((t.context_tokens, t.context_limit, burn_rate, t.is_reset));

                    let (detail, breakdown) = match hs.source {
                        HoverSource::Cost => {
                            let total_in = t.in_cost;
                            let frac = |v: f64| {
                                if total_in > 0.0 {
                                    (v / total_in) as f32
                                } else {
                                    0.0
                                }
                            };
                            let pct = |v: f64| (frac(v) * 100.0).round() as u32;
                            let thinking_tag = if t.has_thinking { " [thinking]" } else { "" };
                            let fracs = [
                                frac(t.fresh_input_cost),
                                frac(t.cache_read_cost),
                                frac(t.cache_create_cost),
                            ];
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
                            let total_in =
                                (t.in_tok + t.cache_read_tok + t.cache_create_tok) as f64;
                            let frac = |v: u64| {
                                if total_in > 0.0 {
                                    (v as f64 / total_in) as f32
                                } else {
                                    0.0
                                }
                            };
                            let fracs = [
                                frac(t.in_tok),
                                frac(t.cache_read_tok),
                                frac(t.cache_create_tok),
                            ];
                            (
                                format!(
                                    "  t{} [{}] ctx {} (fresh {} read {} create {})  out {}",
                                    idx + 1,
                                    t.model_short,
                                    format_tokens(total_in as u64),
                                    format_tokens(t.in_tok),
                                    format_tokens(t.cache_read_tok),
                                    format_tokens(t.cache_create_tok),
                                    format_tokens(t.out_tok),
                                ),
                                Some(fracs),
                            )
                        }
                        HoverSource::TotalCost => (
                            format!(
                                "  t{} total {}  (+{})",
                                idx + 1,
                                format_cost(t.total_cost),
                                format_cost(t.cost_change),
                            ),
                            None,
                        ),
                        HoverSource::TotalTokens => (
                            format!(
                                "  t{} total in {}  out {}",
                                idx + 1,
                                format_tokens(t.total_in_tok),
                                format_tokens(t.total_out_tok),
                            ),
                            None,
                        ),
                        HoverSource::WeeklyCost | HoverSource::WeeklyRate => (
                            format!(
                                "  t{} [{}] +{}  total {}",
                                idx + 1,
                                t.model_short,
                                format_cost(t.cost_change),
                                format_cost(t.total_cost),
                            ),
                            None,
                        ),
                        HoverSource::Energy => {
                            let wh = t.energy.facility_kwh.mid * 1000.0;
                            (
                                format!(
                                    "  t{} [{}] {:.2} Wh  ({:.2}..{:.2})",
                                    idx + 1,
                                    t.model_short,
                                    wh,
                                    t.energy.facility_kwh.low * 1000.0,
                                    t.energy.facility_kwh.high * 1000.0,
                                ),
                                None,
                            )
                        }
                        HoverSource::Water => {
                            let wml = t.energy.water_total_ml.mid;
                            (
                                format!(
                                    "  t{} [{}] {:.2} mL  ({:.2}..{:.2})",
                                    idx + 1,
                                    t.model_short,
                                    wml,
                                    t.energy.water_total_ml.low,
                                    t.energy.water_total_ml.high,
                                ),
                                None,
                            )
                        }
                        HoverSource::Budget => (
                            format!(
                                "  t{} [{}] +{}  total {}",
                                idx + 1,
                                t.model_short,
                                format_cost(t.cost_change),
                                format_cost(t.total_cost),
                            ),
                            None,
                        ),
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
            let mut nearby_skills: Vec<&str> = cd
                .skill_xs
                .iter()
                .filter(|(x, _)| {
                    let snap = if entries.len() > 1 { 1.5 } else { 0.5 };
                    (x - hx).abs() <= snap
                })
                .map(|(_, name)| name.as_str())
                .collect();
            nearby_skills.dedup();

            // For WeeklyCost/Budget, compute cumulative combined cost at hovered x
            let running_total_header: Option<String> =
                if matches!(hs.source, HoverSource::WeeklyCost) {
                    let pts = &cd.combined_cost_pts;
                    let idx = pts.partition_point(|p| p[0] <= hx);
                    let total = if idx > 0 {
                        pts[idx - 1][1]
                    } else if !pts.is_empty() {
                        pts[0][1]
                    } else {
                        0.0
                    };
                    Some(format!("total {}", format_cost(total)))
                } else if matches!(hs.source, HoverSource::Budget) {
                    let period_start_x = billing.period_start_x();
                    let base_cost = cd
                        .budget_cost_pts
                        .iter()
                        .find(|p| p[0] >= period_start_x)
                        .map(|p| p[1])
                        .unwrap_or(0.0);
                    let idx = cd.budget_cost_pts.partition_point(|p| p[0] <= hx);
                    let raw = if idx > 0 {
                        cd.budget_cost_pts[idx - 1][1]
                    } else {
                        0.0
                    };
                    let spent = (raw - base_cost).max(0.0);
                    let remaining = (billing.limit_usd - spent).max(0.0);
                    let pct = if billing.limit_usd > 0.0 {
                        spent / billing.limit_usd * 100.0
                    } else {
                        0.0
                    };
                    let elapsed_days = (hx - period_start_x) / (60.0 * 24.0);
                    let rate_line = if elapsed_days > 0.1 {
                        let per_day = spent / elapsed_days;
                        format!(
                            "  {}/day  proj {}/mo",
                            format_cost(per_day),
                            format_cost(per_day * 30.0)
                        )
                    } else {
                        String::new()
                    };
                    // Format date from hx
                    let secs = (hx * 60.0) as libc::time_t;
                    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
                    unsafe {
                        libc::localtime_r(&secs, &mut tm);
                    }
                    let months = [
                        "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct",
                        "Nov", "Dec",
                    ];
                    let month = months.get(tm.tm_mon as usize).copied().unwrap_or("?");
                    Some(format!(
                        "{} {} {:02}:{:02}  spent {} ({:.0}%)  remaining {}{}",
                        month,
                        tm.tm_mday,
                        tm.tm_hour,
                        tm.tm_min,
                        format_cost(spent),
                        pct,
                        format_cost(remaining),
                        rate_line,
                    ))
                } else {
                    None
                };

            if !entries.is_empty() {
                let win_rect = ui.ctx().viewport_rect();
                let tip_w = 420.0_f32;
                let row_count = entries.len()
                    + if running_total_header.is_some() { 1 } else { 0 }
                    + nearby_skills.len()
                    + if context_footer.is_some() { 1 } else { 0 }
                    + if omitted > 0 { 1 } else { 0 }
                    + 1; // +1 for header
                let tip_h = row_count as f32 * 16.0 + 20.0;
                let offset = 14.0;
                let x_offset = if cursor.x + tip_w + offset > win_rect.right() {
                    -tip_w - offset
                } else {
                    offset
                };
                let is_budget_hover = matches!(hs.source, HoverSource::Budget);
                let tip_pos = if is_budget_hover {
                    // Tooltip right edge aligns to budget chart left edge
                    egui::pos2(usage_rect.left() - tip_w - 4.0, usage_rect.top() + 4.0)
                } else {
                    let tip_y = if cursor.y - tip_h - 8.0 >= win_rect.top() + 4.0 {
                        cursor.y - tip_h - 8.0
                    } else {
                        cursor.y + 20.0
                    };
                    egui::pos2(cursor.x + x_offset, tip_y)
                };

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
                        egui::Frame::NONE
                            .fill(egui::Color32::from_rgb(20, 18, 14))
                            .stroke(frame_stroke)
                            .corner_radius(5.0)
                            .inner_margin(egui::Margin::same(8))
                            .show(ui, |ui| {
                                ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Extend);
                                let font = egui::FontId::monospace(10.0);
                                let hdr_col = Palette::TEXT_DIM;

                                if let Some(hdr) = &running_total_header {
                                    ui.add(egui::Label::new(
                                        egui::RichText::new(hdr)
                                            .monospace()
                                            .size(11.0)
                                            .color(Palette::INPUT_TINT),
                                    ));
                                }

                                // Column colors matching bar segments
                                let c_input = egui::Color32::from_rgb(60, 120, 200); // fresh input (blue) — matches fresh$ bars
                                let c_cache_read = egui::Color32::from_rgb(80, 180, 100); // cache read (green) — matches read$ bars
                                let c_cache_create = egui::Color32::from_rgb(220, 160, 60); // cache create (gold) — matches create$ bars
                                let c_output = egui::Color32::from_rgb(220, 160, 60); // output (gold)
                                let c_think = egui::Color32::from_rgb(180, 80, 200); // output thinking (purple)
                                let c_total = Palette::TEXT_BRIGHT;
                                let c_meta = Palette::TEXT_DIM;
                                let c_pct =
                                    egui::Color32::from_rgba_unmultiplied(180, 180, 180, 100);
                                let mono = egui::FontId::monospace(11.0);
                                let mono_sm = egui::FontId::monospace(9.0);

                                // Right-aligned colored cell helper
                                let cell =
                                    |ui: &mut egui::Ui,
                                     text: &str,
                                     color: egui::Color32,
                                     f: &egui::FontId| {
                                        ui.with_layout(
                                            egui::Layout::right_to_left(egui::Align::Center),
                                            |ui| {
                                                ui.add(egui::Label::new(
                                                    egui::RichText::new(text)
                                                        .font(f.clone())
                                                        .color(color),
                                                ));
                                            },
                                        );
                                    };
                                // Header cell with dotted underline and tooltip
                                let header_cell =
                                    |ui: &mut egui::Ui,
                                     text: &str,
                                     color: egui::Color32,
                                     f: &egui::FontId,
                                     tooltip: &str| {
                                        // Draw text
                                        let galley = ui.painter().layout_no_wrap(
                                            text.to_string(),
                                            f.clone(),
                                            color,
                                        );
                                        let size = galley.size();
                                        let (rect, resp) =
                                            ui.allocate_exact_size(size, egui::Sense::hover());
                                        ui.painter().galley(rect.min, galley, color);

                                        // Draw dotted underline
                                        let y = rect.bottom() + 1.0;
                                        let mut x = rect.left();
                                        while x < rect.right() {
                                            let seg_end = (x + 2.0).min(rect.right());
                                            ui.painter().line_segment(
                                                [egui::pos2(x, y), egui::pos2(seg_end, y)],
                                                egui::Stroke::new(1.0, color.gamma_multiply(0.5)),
                                            );
                                            x += 4.0;
                                        }

                                        // Show tooltip on hover
                                        if resp.hovered() {
                                            if let Some(pos) = ui.input(|i| i.pointer.hover_pos()) {
                                                egui::Area::new(egui::Id::new((
                                                    "header_tooltip",
                                                    text,
                                                )))
                                                .order(egui::Order::Tooltip)
                                                .fixed_pos(egui::pos2(pos.x, pos.y + 20.0))
                                                .show(ui.ctx(), |ui| {
                                                    egui::Frame::NONE
                                                        .fill(egui::Color32::from_rgb(30, 28, 20))
                                                        .stroke(egui::Stroke::new(
                                                            0.5,
                                                            egui::Color32::from_rgb(100, 95, 80),
                                                        ))
                                                        .corner_radius(3.0)
                                                        .inner_margin(egui::Margin::same(4))
                                                        .show(ui, |ui| {
                                                            ui.label(tooltip);
                                                        });
                                                });
                                            }
                                        }
                                    };
                                // Left-aligned colored cell helper
                                let lcell =
                                    |ui: &mut egui::Ui,
                                     text: &str,
                                     color: egui::Color32,
                                     f: &egui::FontId| {
                                        ui.add(egui::Label::new(
                                            egui::RichText::new(text).font(f.clone()).color(color),
                                        ));
                                    };

                                let fmt_pct = |v: f64, total: f64| -> String {
                                    if total < 1e-9 {
                                        return String::new();
                                    }
                                    let p = v / total * 100.0;
                                    if p < 0.1 {
                                        String::new()
                                    } else {
                                        format!("{:.0}%", p)
                                    }
                                };

                                // Pre-resolve all turns so we can iterate twice (header + grid)
                                let resolved: Vec<_> = entries
                                    .iter()
                                    .filter_map(|(name, sess_color, _, _, _)| {
                                        let nearest = cd
                                            .session_turns
                                            .iter()
                                            .find(|(n, _, _)| n == name)
                                            .and_then(|(_, _, turns)| {
                                                turns.iter().enumerate().min_by(|(_, a), (_, b)| {
                                                    let ax = if use_ts_x { a.ts_x } else { a.x };
                                                    let bx = if use_ts_x { b.ts_x } else { b.x };
                                                    (ax - hx)
                                                        .abs()
                                                        .partial_cmp(&(bx - hx).abs())
                                                        .unwrap()
                                                })
                                            });
                                        nearest.map(|(idx, t)| (name.as_str(), sess_color, idx, t))
                                    })
                                    .collect();

                                ui.spacing_mut().item_spacing = egui::vec2(10.0, 1.0);
                                egui::Grid::new("tooltip_grid")
                                    .min_col_width(0.0)
                                    .spacing(egui::vec2(10.0, 1.0))
                                    .show(ui, |ui| {
                                        // Header row
                                        lcell(ui, "session", hdr_col, &mono);
                                        lcell(ui, "turn", hdr_col, &mono);
                                        lcell(ui, "model", hdr_col, &mono);
                                        match hs.source {
                                            HoverSource::Cost => {
                                                header_cell(
                                                    ui,
                                                    "in",
                                                    c_input,
                                                    &mono,
                                                    "Fresh input tokens — context not in cache.\nCharged at base input rate (e.g. $3/MTok Sonnet).\nApplies to the portion of the prompt not covered by a cache hit.",
                                                );
                                                header_cell(
                                                    ui,
                                                    "in(read)",
                                                    c_cache_read,
                                                    &mono,
                                                    "Cache-read tokens — prompt served from the 1-hour cache.\nCharged at 0.1× input rate (e.g. $0.30/MTok Sonnet).\nAppears when a prior cache-write turn's context is reused.",
                                                );
                                                header_cell(
                                                    ui,
                                                    "in(write)",
                                                    c_cache_create,
                                                    &mono,
                                                    "Cache-write tokens — storing context into the 1-hour prompt cache.\nCharged at 2× input rate (e.g. $6/MTok Sonnet).\nAppears on turns that establish or refresh a large context block.",
                                                );
                                                header_cell(
                                                    ui,
                                                    "out",
                                                    c_output,
                                                    &mono,
                                                    "Output tokens — generated response text.\nCharged at output rate (e.g. $15/MTok Sonnet).\nDoes not include thinking tokens.",
                                                );
                                                header_cell(
                                                    ui,
                                                    "out(think)",
                                                    c_think,
                                                    &mono,
                                                    "Thinking/reasoning tokens — internal chain-of-thought.\nCharged at the same output rate as regular output.\nOnly present on models with extended thinking enabled.",
                                                );
                                                header_cell(
                                                    ui,
                                                    "turn",
                                                    c_total,
                                                    &mono,
                                                    "Cost change this turn",
                                                );
                                                header_cell(
                                                    ui,
                                                    "cumul",
                                                    c_meta,
                                                    &mono,
                                                    "Cumulative session total",
                                                );
                                            }
                                            HoverSource::Tokens => {
                                                header_cell(
                                                    ui,
                                                    "in",
                                                    c_input,
                                                    &mono,
                                                    "Fresh input tokens — context not in cache.\nCharged at base input rate.",
                                                );
                                                header_cell(
                                                    ui,
                                                    "in(read)",
                                                    c_cache_read,
                                                    &mono,
                                                    "Cache-read tokens — prompt served from the 1-hour cache.\nCheap: 0.1× input rate.",
                                                );
                                                header_cell(
                                                    ui,
                                                    "in(write)",
                                                    c_cache_create,
                                                    &mono,
                                                    "Cache-write tokens — storing context into the 1-hour prompt cache.\nExpensive: 2× input rate.",
                                                );
                                                header_cell(
                                                    ui,
                                                    "out",
                                                    c_output,
                                                    &mono,
                                                    "Output tokens — generated response text.",
                                                );
                                                header_cell(
                                                    ui,
                                                    "out(think)",
                                                    c_think,
                                                    &mono,
                                                    "Thinking tokens — internal chain-of-thought.\nSame rate as output. Only on extended-thinking models.",
                                                );
                                                header_cell(
                                                    ui,
                                                    "cumul",
                                                    c_meta,
                                                    &mono,
                                                    "Cumulative tokens",
                                                );
                                            }
                                            HoverSource::Energy => {
                                                header_cell(
                                                    ui,
                                                    "Wh",
                                                    egui::Color32::from_rgb(120, 200, 80),
                                                    &mono,
                                                    "Facility energy (Wh)",
                                                );
                                            }
                                            HoverSource::Water => {
                                                header_cell(
                                                    ui,
                                                    "mL",
                                                    egui::Color32::from_rgb(80, 180, 220),
                                                    &mono,
                                                    "Total water (mL)",
                                                );
                                            }
                                            _ => {
                                                header_cell(
                                                    ui,
                                                    "cost",
                                                    c_input,
                                                    &mono,
                                                    "Turn cost",
                                                );
                                                header_cell(
                                                    ui,
                                                    "+delta",
                                                    c_total,
                                                    &mono,
                                                    "Cost change",
                                                );
                                                header_cell(
                                                    ui,
                                                    "total",
                                                    hdr_col,
                                                    &mono,
                                                    "Cumulative total",
                                                );
                                            }
                                        }
                                        ui.end_row();

                                        // Data rows: value row + percent row per session
                                        for (name, sess_color, idx, t) in &resolved {
                                            let short_name =
                                                if name.len() > 16 { &name[..16] } else { name };
                                            let think_mark = if t.has_thinking { "*" } else { "" };

                                            // Value row
                                            lcell(ui, short_name, **sess_color, &mono);
                                            cell(ui, &format!("t{}", idx + 1), c_meta, &mono);
                                            lcell(
                                                ui,
                                                &format!("{}{}", t.model_short, think_mark),
                                                c_meta,
                                                &mono,
                                            );
                                            match hs.source {
                                                HoverSource::Cost => {
                                                    let (out_reg, out_think) = if t.has_thinking {
                                                        (0.0, t.out_cost)
                                                    } else {
                                                        (t.out_cost, 0.0)
                                                    };
                                                    cell(
                                                        ui,
                                                        &format_cost(t.fresh_input_cost),
                                                        c_input,
                                                        &mono,
                                                    );
                                                    cell(
                                                        ui,
                                                        &format_cost(t.cache_read_cost),
                                                        c_cache_read,
                                                        &mono,
                                                    );
                                                    cell(
                                                        ui,
                                                        &format_cost(t.cache_create_cost),
                                                        c_cache_create,
                                                        &mono,
                                                    );
                                                    cell(
                                                        ui,
                                                        &format_cost(out_reg),
                                                        c_output,
                                                        &mono,
                                                    );
                                                    cell(
                                                        ui,
                                                        &format_cost(out_think),
                                                        c_think,
                                                        &mono,
                                                    );
                                                    cell(
                                                        ui,
                                                        &format_cost(t.cost_change),
                                                        c_total,
                                                        &mono,
                                                    );
                                                    cell(
                                                        ui,
                                                        &format_cost(t.total_cost),
                                                        c_meta,
                                                        &mono,
                                                    );
                                                }
                                                HoverSource::Tokens => {
                                                    let (out_reg, out_think) = if t.has_thinking {
                                                        (0, t.out_tok)
                                                    } else {
                                                        (t.out_tok, 0)
                                                    };
                                                    cell(
                                                        ui,
                                                        &format_tokens(t.in_tok),
                                                        c_input,
                                                        &mono,
                                                    );
                                                    cell(
                                                        ui,
                                                        &format_tokens(t.cache_read_tok),
                                                        c_cache_read,
                                                        &mono,
                                                    );
                                                    cell(
                                                        ui,
                                                        &format_tokens(t.cache_create_tok),
                                                        c_cache_create,
                                                        &mono,
                                                    );
                                                    cell(
                                                        ui,
                                                        &format_tokens(out_reg),
                                                        c_output,
                                                        &mono,
                                                    );
                                                    cell(
                                                        ui,
                                                        &format_tokens(out_think),
                                                        c_think,
                                                        &mono,
                                                    );
                                                    let cumul_tok =
                                                        t.total_in_tok + t.total_out_tok;
                                                    cell(
                                                        ui,
                                                        &format_tokens(cumul_tok),
                                                        c_meta,
                                                        &mono,
                                                    );
                                                }
                                                HoverSource::Energy => {
                                                    let wh = t.energy.facility_kwh.mid * 1000.0;
                                                    cell(
                                                        ui,
                                                        &format!("{:.2} Wh", wh),
                                                        egui::Color32::from_rgb(120, 200, 80),
                                                        &mono,
                                                    );
                                                }
                                                HoverSource::Water => {
                                                    let wml = t.energy.water_total_ml.mid;
                                                    cell(
                                                        ui,
                                                        &format!("{:.1} mL", wml),
                                                        egui::Color32::from_rgb(80, 180, 220),
                                                        &mono,
                                                    );
                                                }
                                                _ => {
                                                    cell(
                                                        ui,
                                                        &format_cost(t.in_cost + t.out_cost),
                                                        c_input,
                                                        &mono,
                                                    );
                                                    cell(
                                                        ui,
                                                        &format!("+{}", format_cost(t.cost_change)),
                                                        c_total,
                                                        &mono,
                                                    );
                                                    cell(
                                                        ui,
                                                        &format_cost(t.total_cost),
                                                        c_meta,
                                                        &mono,
                                                    );
                                                }
                                            }
                                            ui.end_row();

                                            // Percent row (smaller font, dimmer)
                                            ui.label(""); // session
                                            ui.label(""); // turn
                                            ui.label(""); // model
                                            match hs.source {
                                                HoverSource::Cost => {
                                                    let turn_total = t.cost_change.max(1e-9);
                                                    let (out_reg, out_think) = if t.has_thinking {
                                                        (0.0, t.out_cost)
                                                    } else {
                                                        (t.out_cost, 0.0)
                                                    };
                                                    cell(
                                                        ui,
                                                        &fmt_pct(t.fresh_input_cost, turn_total),
                                                        c_pct,
                                                        &mono_sm,
                                                    );
                                                    cell(
                                                        ui,
                                                        &fmt_pct(t.cache_read_cost, turn_total),
                                                        c_pct,
                                                        &mono_sm,
                                                    );
                                                    cell(
                                                        ui,
                                                        &fmt_pct(t.cache_create_cost, turn_total),
                                                        c_pct,
                                                        &mono_sm,
                                                    );
                                                    cell(
                                                        ui,
                                                        &fmt_pct(out_reg, turn_total),
                                                        c_pct,
                                                        &mono_sm,
                                                    );
                                                    cell(
                                                        ui,
                                                        &fmt_pct(out_think, turn_total),
                                                        c_pct,
                                                        &mono_sm,
                                                    );
                                                    cell(
                                                        ui,
                                                        &fmt_pct(t.cost_change, t.total_cost),
                                                        c_pct,
                                                        &mono_sm,
                                                    );
                                                    ui.label(""); // cumul has no %
                                                }
                                                HoverSource::Tokens => {
                                                    let turn_total = (t.in_tok
                                                        + t.cache_read_tok
                                                        + t.cache_create_tok
                                                        + t.out_tok)
                                                        as f64;
                                                    let (out_reg, out_think) = if t.has_thinking {
                                                        (0.0, t.out_tok as f64)
                                                    } else {
                                                        (t.out_tok as f64, 0.0)
                                                    };
                                                    let cumul_tok =
                                                        (t.total_in_tok + t.total_out_tok) as f64;
                                                    cell(
                                                        ui,
                                                        &fmt_pct(t.in_tok as f64, turn_total),
                                                        c_pct,
                                                        &mono_sm,
                                                    );
                                                    cell(
                                                        ui,
                                                        &fmt_pct(
                                                            t.cache_read_tok as f64,
                                                            turn_total,
                                                        ),
                                                        c_pct,
                                                        &mono_sm,
                                                    );
                                                    cell(
                                                        ui,
                                                        &fmt_pct(
                                                            t.cache_create_tok as f64,
                                                            turn_total,
                                                        ),
                                                        c_pct,
                                                        &mono_sm,
                                                    );
                                                    cell(
                                                        ui,
                                                        &fmt_pct(out_reg, turn_total),
                                                        c_pct,
                                                        &mono_sm,
                                                    );
                                                    cell(
                                                        ui,
                                                        &fmt_pct(out_think, turn_total),
                                                        c_pct,
                                                        &mono_sm,
                                                    );
                                                    cell(
                                                        ui,
                                                        &fmt_pct(turn_total, cumul_tok),
                                                        c_pct,
                                                        &mono_sm,
                                                    );
                                                }
                                                _ => {}
                                            }
                                            ui.end_row();
                                        }
                                    });
                                if omitted > 0 {
                                    ui.add(egui::Label::new(
                                        egui::RichText::new(format!("+{} more", omitted))
                                            .font(font.clone())
                                            .color(Palette::TEXT_DIM),
                                    ));
                                }
                                for sk in &nearby_skills {
                                    let short = sk.rsplit(':').next().unwrap_or(sk);
                                    ui.add(egui::Label::new(
                                        egui::RichText::new(format!("skill: {}", short))
                                            .font(font.clone())
                                            .color(Palette::SKILL_MARKER),
                                    ));
                                }
                                if let Some((ctx_tok, ctx_limit, burn_rate, is_reset)) =
                                    context_footer
                                {
                                    let pct = if ctx_limit > 0 {
                                        (ctx_tok as f64 / ctx_limit as f64 * 100.0).round() as u32
                                    } else {
                                        0
                                    };
                                    let remaining = ctx_limit.saturating_sub(ctx_tok);
                                    let countdown = if burn_rate > 0.0 {
                                        format!(
                                            "  ~{} turns til compact",
                                            (remaining as f64 / burn_rate).round() as u64
                                        )
                                    } else {
                                        String::new()
                                    };
                                    let reset_tag = if is_reset { " [RESET]" } else { "" };
                                    let ctx_line = format!(
                                        "ctx {}% {}/{}  rem {}{}{}",
                                        pct,
                                        format_tokens(ctx_tok),
                                        format_tokens(ctx_limit),
                                        format_tokens(remaining),
                                        countdown,
                                        reset_tag
                                    );
                                    let ctx_color = if pct >= 80 {
                                        egui::Color32::from_rgb(220, 80, 60)
                                    } else if pct >= 60 {
                                        egui::Color32::from_rgb(220, 180, 60)
                                    } else {
                                        Palette::TEXT_DIM
                                    };
                                    ui.add(egui::Label::new(
                                        egui::RichText::new(ctx_line)
                                            .font(font.clone())
                                            .color(ctx_color),
                                    ));
                                }
                                if is_pinned {
                                    ui.add(egui::Label::new(
                                        egui::RichText::new("pinned (Esc to clear)")
                                            .font(font)
                                            .color(egui::Color32::from_rgba_unmultiplied(
                                                180, 160, 100, 140,
                                            )),
                                    ));
                                }
                            });
                    });
            }

            // Press V while hovering a turn to open the turn viewer popup.
            // Uses the globally-nearest turn across all sessions under cursor.
            if let Some((meta_idx, api_call_idx, display_name, _)) = viewer_hit {
                let pressed_v = ui
                    .ctx()
                    .input(|i| i.key_pressed(egui::Key::V));
                if pressed_v {
                    if let Some((sid, cwd)) = cd.session_meta.get(meta_idx).cloned() {
                        *viewer = Some(TurnViewerState {
                            session_id: sid,
                            cwd,
                            display_name,
                            api_call_idx,
                            payload: None,
                        });
                    }
                }
            }
        }
    }

    // --- Turn viewer popup ---
    if let Some(v) = viewer.as_mut() {
        if v.payload.is_none() {
            v.payload = agent_harnesses::claude_code::read_turn_payload(
                &v.cwd,
                &v.session_id,
                v.api_call_idx,
            );
        }
    }
    let mut keep_open = viewer.is_some();
    if let Some(v) = viewer.as_ref() {
        let title = format!("turn {} — {}", v.api_call_idx + 1, v.display_name);
        egui::Window::new(title)
            .id(egui::Id::new("hud_turn_viewer"))
            .open(&mut keep_open)
            .default_size([640.0, 520.0])
            .resizable(true)
            .show(ui.ctx(), |ui| {
                egui::ScrollArea::vertical().show(ui, |ui| match &v.payload {
                    Some(p) => {
                        ui.label(
                            egui::RichText::new("user")
                                .color(Palette::TEXT_DIM)
                                .monospace(),
                        );
                        let mut user_buf = p.user_text.as_str();
                        ui.add(
                            egui::TextEdit::multiline(&mut user_buf)
                                .desired_width(f32::INFINITY)
                                .code_editor(),
                        );
                        ui.separator();
                        ui.label(
                            egui::RichText::new("assistant")
                                .color(Palette::TEXT_DIM)
                                .monospace(),
                        );
                        let mut asst_buf = p.assistant_text.as_str();
                        ui.add(
                            egui::TextEdit::multiline(&mut asst_buf)
                                .desired_width(f32::INFINITY)
                                .code_editor(),
                        );
                        if !p.tool_uses.is_empty() {
                            ui.separator();
                            ui.collapsing(
                                format!("tool calls ({})", p.tool_uses.len()),
                                |ui| {
                                    for (name, input) in &p.tool_uses {
                                        ui.label(
                                            egui::RichText::new(name)
                                                .color(Palette::TEXT_BRIGHT)
                                                .monospace(),
                                        );
                                        let mut input_buf = input.as_str();
                                        ui.add(
                                            egui::TextEdit::multiline(&mut input_buf)
                                                .desired_width(f32::INFINITY)
                                                .code_editor(),
                                        );
                                    }
                                },
                            );
                        }
                    }
                    None => {
                        ui.label("(no payload — JSONL line not found for this turn)");
                    }
                });
            });
    }
    if !keep_open {
        *viewer = None;
    } else if ui.ctx().input(|i| i.key_pressed(egui::Key::Escape)) {
        *viewer = None;
    }
}

fn draw_chart_label(ui: &mut egui::Ui, title: &str, top_label: &str, bot_label: &str) {
    let rect = ui.available_rect_before_wrap();
    let p = ui.painter();
    // Inset from edges to avoid colliding with y-axis tick labels
    let y_inset = 12.0; // clear top/bottom axis ticks
    let x_inset = 4.0;
    p.text(
        egui::pos2(rect.left() + x_inset, rect.top() + y_inset),
        egui::Align2::LEFT_TOP,
        title,
        egui::FontId::monospace(10.0),
        Palette::TEXT_DIM,
    );
    p.text(
        egui::pos2(rect.right() - x_inset, rect.top() + y_inset),
        egui::Align2::RIGHT_TOP,
        top_label,
        egui::FontId::monospace(9.0),
        Palette::INPUT_TINT,
    );
    p.text(
        egui::pos2(rect.right() - x_inset, rect.bottom() - y_inset),
        egui::Align2::RIGHT_BOTTOM,
        bot_label,
        egui::FontId::monospace(9.0),
        Palette::OUTPUT_TINT,
    );
}

// ---------------------------------------------------------------------------
// Strip layout (original compact HUD)
// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// eframe App impl
// ---------------------------------------------------------------------------

impl App for Hud {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let data = self.hud_data.lock().unwrap().clone();

        let bg = egui::Color32::from_rgb(14, 12, 9);

        egui::CentralPanel::default()
            .frame(egui::Frame::NONE.fill(bg))
            .show_inside(ui, |ui| {
                if data.sessions.is_empty() {
                    let area = ui.available_rect_before_wrap();
                    ui.painter().text(
                        area.center(),
                        egui::Align2::CENTER_CENTER,
                        "no active sessions",
                        egui::FontId::monospace(16.0),
                        Palette::TEXT_DIM,
                    );
                    ui.ctx().request_repaint_after(std::time::Duration::from_secs(1));
                    return;
                }

                // Active filter set depends on current mode
                let filter_set = match self.filter_mode {
                    FilterMode::Exclude => &mut self.exclude_set,
                    FilterMode::Include => &mut self.include_set,
                };
                // Compute effective hidden set from active filter + mode.
                // Focus overrides everything: hide all sessions except the focused one.
                let mut effective_hidden: HashSet<String> = if let Some(fid) = &self.focused {
                    if data.sessions.iter().any(|s| &s.session_id == fid) {
                        data.sessions
                            .iter()
                            .map(|s| s.session_id.clone())
                            .filter(|id| id != fid)
                            .collect()
                    } else {
                        self.focused = None;
                        match self.filter_mode {
                            FilterMode::Exclude => filter_set.clone(),
                            FilterMode::Include => data
                                .sessions
                                .iter()
                                .map(|s| s.session_id.clone())
                                .filter(|id| !filter_set.contains(id))
                                .collect(),
                        }
                    }
                } else {
                    match self.filter_mode {
                        FilterMode::Exclude => filter_set.clone(),
                        FilterMode::Include => data
                            .sessions
                            .iter()
                            .map(|s| s.session_id.clone())
                            .filter(|id| !filter_set.contains(id))
                            .collect(),
                    }
                };
                if self.focused.is_none() && self.show_active_only {
                    for s in &data.sessions {
                        if !s.is_active {
                            effective_hidden.insert(s.session_id.clone());
                        }
                    }
                }

                // Default nav_view to last 24h on first data load (minutes-from-epoch)
                if self.nav_view.is_none() && !data.sessions.is_empty() {
                    let max_last = data
                        .sessions
                        .iter()
                        .map(|s| s.last_ts)
                        .max()
                        .unwrap_or(0) as f64
                        / 60.0;
                    if max_last > 0.0 {
                        self.nav_view = Some((max_last - 24.0 * 60.0, max_last));
                    }
                }

                // In non-time mode, nav_view range filters sessions by last_ts ∈ window.
                // Quantize window to 1-minute boundaries so small scroll deltas don't
                // flip the hidden set (and invalidate the cache) every frame.
                if !self.time_axis {
                    if let Some((vmin, vmax)) = self.nav_view {
                        let qmin = vmin.floor();
                        let qmax = vmax.ceil();
                        for s in &data.sessions {
                            let last_m = s.last_ts as f64 / 60.0;
                            if last_m < qmin || last_m > qmax {
                                effective_hidden.insert(s.session_id.clone());
                            }
                        }
                    }
                }

                // Cache chart data: only rebuild when data generation, hidden set, or time_axis changes
                let data_gen = data.generation as usize;
                let cache_hit = self.cached_chart.as_ref().map_or(false, |(g, h, t, cp, _)| {
                    *g == data_gen
                        && *h == effective_hidden
                        && *t == self.time_axis
                        && *cp == self.compaction_plus
                });
                if !cache_hit {
                    let cd = build_chart_data(
                        &data,
                        &effective_hidden,
                        self.time_axis,
                        self.compaction_plus,
                    );
                    self.cached_plot = Some(build_plot_cache(&cd));
                    self.cached_chart = Some((
                        data_gen,
                        effective_hidden.clone(),
                        self.time_axis,
                        self.compaction_plus,
                        cd,
                    ));
                }
                let cd = &self.cached_chart.as_ref().unwrap().4;
                let pc = self.cached_plot.as_ref().unwrap();
                let usage = self.usage_data.lock().unwrap().clone();

                draw_big(
                    ui,
                    &data,
                    &cd,
                    pc,
                    &usage,
                    filter_set,
                    &mut self.filter_mode,
                    &mut self.show_active_only,
                    &mut self.show_bars,
                    &effective_hidden,
                    &mut self.time_axis,
                    &mut self.compaction_plus,
                    &mut self.autofit,
                    &mut self.nav_view,
                    &mut self.turn_view,
                    &mut self.expanded_groups,
                    &mut self.expanded_sessions,
                    &mut self.chart_vis,
                    &mut self.show_budget,
                    &mut self.billing,
                    &mut self.focused,
                    &mut self.viewer,
                    &mut self.chart_tree,
                );
            });

        let data_gen = data.generation as usize;
        if data_gen != self.last_seen_gen {
            self.last_seen_gen = data_gen;
            ui.ctx().request_repaint();
        } else if data.sessions.iter().any(|s| s.is_active) {
            ui.ctx().request_repaint_after(std::time::Duration::from_millis(250));
        }
    }

    fn save(&mut self, _storage: &mut dyn eframe::Storage) {
        self.billing.save_to_file();
    }

    fn on_exit(&mut self) {}

    fn clear_color(&self, _visuals: &egui::Visuals) -> [f32; 4] {
        egui::Color32::from_rgb(14, 12, 9).to_normalized_gamma_f32()
    }
}

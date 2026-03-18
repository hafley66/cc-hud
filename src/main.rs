#![allow(dead_code)]

mod geometry;
mod anchors;
mod agent_harnesses;

use std::sync::{Arc, Mutex};
use std::time::Duration;

use egui_overlay::EguiOverlay;
use egui_overlay::egui_render_wgpu::WgpuBackend as DefaultGfxBackend;
use egui_overlay::egui_window_glfw_passthrough::GlfwBackend;

use geometry::PixelRect;

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

    // Debug: log environment for diagnosing launch issues
    tracing::info!(
        tmux = std::env::var("TMUX").unwrap_or_default(),
        tmux_pane = std::env::var("TMUX_PANE").unwrap_or_default(),
        term_program = std::env::var("TERM_PROGRAM").unwrap_or_default(),
        pid = std::process::id(),
        args = ?std::env::args().collect::<Vec<_>>(),
        "cc-hud starting"
    );

    // Target: session name, global pane id (%N), or default to first pane in current session
    let target = match std::env::args().nth(1) {
        Some(arg) => anchors::tmux::TmuxTarget::parse(&arg),
        None => {
            // Try $TMUX_PANE first, then fall back to current session's first pane
            match std::env::var("TMUX_PANE") {
                Ok(pane) => anchors::tmux::TmuxTarget::PaneId(pane),
                Err(_) => {
                    // Get current session name from tmux
                    let session = std::process::Command::new("tmux")
                        .args(["display-message", "-p", "#{session_name}"])
                        .output()
                        .ok()
                        .and_then(|o| String::from_utf8(o.stdout).ok())
                        .map(|s| s.trim().to_string())
                        .unwrap_or_else(|| "0".to_string());
                    anchors::tmux::TmuxTarget::Session(session)
                }
            }
        }
    };
    tracing::info!(?target, "tmux target");

    // Compute initial rect synchronously before opening the window
    let initial_rect = compute_pane_rect(&target).unwrap_or_else(|| {
        tracing::warn!("could not compute initial pane rect, using fallback");
        PixelRect { x: 0, y: 0, w: 800, h: 60 }
    });
    tracing::info!(?initial_rect, "initial overlay position");

    let state = Arc::new(Mutex::new(initial_rect));
    let visible = Arc::new(std::sync::atomic::AtomicBool::new(true));

    // Spawn pane tracking thread for ongoing updates
    let poll_state = state.clone();
    let poll_visible = visible.clone();
    std::thread::spawn(move || {
        pane_poll_loop(target, poll_state, poll_visible);
    });

    start_overlay(Hud {
        first_frame: true,
        shown: false,
        state,
        visible,
    });
}

fn compute_pane_rect(target: &anchors::tmux::TmuxTarget) -> Option<PixelRect> {
    let term_pid = match anchors::terminal::terminal_pid() {
        Some(p) => { tracing::debug!(term_pid = p, "found terminal"); p }
        None => { tracing::warn!("terminal_pid() returned None"); return None; }
    };
    let pane = match anchors::tmux::find_pane(target) {
        Some(p) => { tracing::debug!(?p.cell_rect, tty = ?p.tty, "found pane"); p }
        None => { tracing::warn!(?target, "find_pane() returned None"); return None; }
    };
    let tty = match pane.tty.as_deref() {
        Some(t) => { tracing::debug!(tty = t, "pane tty"); t }
        None => { tracing::warn!("pane has no tty"); return None; }
    };
    let metrics = match anchors::terminal::cell_metrics_from_tty(tty) {
        Some(m) => { tracing::debug!(?m, "cell metrics"); m }
        None => { tracing::warn!(tty, "cell_metrics_from_tty() returned None"); return None; }
    };
    let origin = match anchors::terminal::terminal_window_origin(term_pid) {
        Some(o) => { tracing::debug!(?o, "window origin"); o }
        None => { tracing::warn!(term_pid, "terminal_window_origin() returned None"); return None; }
    };

    let insets = geometry::TerminalInsets::iterm2_default();
    let rect = geometry::compute_overlay_rect(&pane.cell_rect, &metrics, &origin, &insets, 3, 1);
    tracing::info!(?rect, "computed overlay rect");
    Some(rect)
}

fn pane_poll_loop(
    target: anchors::tmux::TmuxTarget,
    state: Arc<Mutex<PixelRect>>,
    visible: Arc<std::sync::atomic::AtomicBool>,
) {
    use std::sync::atomic::Ordering;

    // Resolve the client tty (iTerm's pty, not tmux's internal pane pty)
    let tty = anchors::tmux::client_tty();
    tracing::info!(?tty, "resolved client tty for tab detection");

    let mut tick = 1u32; // start at 1 so first tab check is delayed, not immediate
    loop {
        // TODO: tab detection disabled until debugged
        // Check tab visibility every ~1s (every 10th tick), not every 100ms
        // AppleScript IPC is slow and would block geometry updates
        // if tick % 10 == 0 {
        //     let tab_active = match &tty {
        //         Some(t) => anchors::terminal::is_iterm_tab_active(t),
        //         None => true,
        //     };
        //     visible.store(tab_active, Ordering::Relaxed);
        // }
        tick = tick.wrapping_add(1);

        if visible.load(Ordering::Relaxed) {
            if let Some(rect) = compute_pane_rect(&target) {
                let mut s = state.lock().unwrap();
                if *s != rect {
                    tracing::debug!(?rect, "pane rect updated");
                    *s = rect;
                }
            }
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

struct Hud {
    first_frame: bool,
    shown: bool,
    state: Arc<Mutex<PixelRect>>,
    visible: Arc<std::sync::atomic::AtomicBool>,
}

impl EguiOverlay for Hud {
    fn gui_run(
        &mut self,
        egui_context: &egui::Context,
        _default_gfx_backend: &mut DefaultGfxBackend,
        glfw_backend: &mut GlfwBackend,
    ) {
        let rect = *self.state.lock().unwrap();
        let is_visible = self.visible.load(std::sync::atomic::Ordering::Relaxed);

        glfw_backend.set_passthrough(true);

        if !is_visible {
            // Move offscreen instead of hide() to avoid focus steal on show()
            glfw_backend.window.set_pos(-9999, -9999);
            glfw_backend.set_window_size([1.0, 1.0]);
            egui_context.request_repaint();
            return;
        }

        glfw_backend.window.set_pos(rect.x, rect.y);
        glfw_backend.set_window_size([rect.w as f32, rect.h as f32]);

        if self.first_frame {
            self.first_frame = false;
            glfw_backend.window.show();
        }

        egui::CentralPanel::default()
            .frame(egui::Frame::none().fill(egui::Color32::TRANSPARENT))
            .show(egui_context, |ui| {
                let area = ui.available_rect_before_wrap();
                let painter = ui.painter();

                // AC color palette
                let bronze = egui::Color32::from_rgb(180, 140, 60);
                let bronze_dim = egui::Color32::from_rgba_unmultiplied(140, 110, 45, 180);
                let bronze_bright = egui::Color32::from_rgb(220, 175, 80);
                let bar_bg = egui::Color32::from_rgba_unmultiplied(15, 13, 8, 200);
                let fill_color = egui::Color32::from_rgb(190, 120, 20);
                let red_low = egui::Color32::from_rgb(180, 40, 30);

                // Mock data
                let fill_frac = 0.62_f32;

                let pad = 2.0;
                let outer_h = area.height() - pad * 2.0;
                let outer_y = area.top() + pad;
                let rounding = 4.0;
                let stroke_w = 2.5;
                let inner_margin = 4.0;

                let label_box_w = outer_h * 0.7; // narrower to hug the letter
                let label_bg = egui::Color32::from_rgba_unmultiplied(140, 110, 45, 40);

                // One continuous outer rect spanning full width
                let outer_rect = egui::Rect::from_min_size(
                    egui::pos2(area.left() + pad, outer_y),
                    egui::vec2(area.width() - pad * 2.0, outer_h),
                );
                painter.rect_filled(outer_rect, rounding, bar_bg);
                painter.rect_stroke(outer_rect, rounding, egui::Stroke::new(stroke_w, bronze_dim));

                // E label area (left end, inside the outer rect)
                // Only round the left corners (it's part of the outer rect)
                let e_rect = egui::Rect::from_min_size(
                    egui::pos2(outer_rect.left() + stroke_w * 0.5, outer_rect.top() + stroke_w * 0.5),
                    egui::vec2(label_box_w, outer_h - stroke_w),
                );
                let e_rounding = egui::Rounding { nw: rounding, sw: rounding, ne: 0.0, se: 0.0 };
                painter.rect_filled(e_rect, e_rounding, label_bg);
                let label_font = egui::FontId::monospace(outer_h * 0.85);
                // Fake bold: draw twice with 1px horizontal offset
                for dx in [0.0, 1.0] {
                    painter.text(
                        e_rect.center() + egui::vec2(dx, 0.0),
                        egui::Align2::CENTER_CENTER,
                        "E",
                        label_font.clone(),
                        fill_color,
                    );
                }

                // F label area (right end, inside the outer rect)
                let f_rect = egui::Rect::from_min_size(
                    egui::pos2(outer_rect.right() - stroke_w * 0.5 - label_box_w, outer_rect.top() + stroke_w * 0.5),
                    egui::vec2(label_box_w, outer_h - stroke_w),
                );
                let f_rounding = egui::Rounding { nw: 0.0, sw: 0.0, ne: rounding, se: rounding };
                painter.rect_filled(f_rect, f_rounding, label_bg);
                for dx in [0.0, 1.0] {
                    painter.text(
                        f_rect.center() + egui::vec2(dx, 0.0),
                        egui::Align2::CENTER_CENTER,
                        "F",
                        label_font.clone(),
                        fill_color,
                    );
                }

                // Inner gauge area (between E and F, with margin from outer)
                let gauge_rect = egui::Rect::from_min_max(
                    egui::pos2(e_rect.right() + inner_margin, outer_rect.top() + inner_margin),
                    egui::pos2(f_rect.left() - inner_margin, outer_rect.bottom() - inner_margin),
                );

                // Fill (not flush -- sits inside the gauge area)
                let fill_w = gauge_rect.width() * fill_frac;
                let fill_rect = egui::Rect::from_min_size(
                    gauge_rect.left_top(),
                    egui::vec2(fill_w, gauge_rect.height()),
                );
                let current_fill = if fill_frac < 0.2 { red_low } else { fill_color };
                painter.rect_filled(fill_rect, 2.0, current_fill);
            });

        egui_context.request_repaint();
    }
}

/// Inlined from egui_overlay::start() to set max_texture_dimension_2d for Retina displays.
fn start_overlay(user_data: Hud) {
    use egui_overlay::egui_window_glfw_passthrough::{GlfwConfig, glfw};
    use egui_overlay::egui_render_wgpu::{WgpuBackend, WgpuConfig};
    use egui_overlay::OverlayApp;

    // Set activation policy BEFORE GLFW init to prevent Space-switching.
    // GLFW's glfw::init() calls [NSApp run] which triggers activate if policy is Regular.
    hide_dock_icon();

    let mut glfw_backend = GlfwBackend::new(GlfwConfig {
        glfw_callback: Box::new(|gtx| {
            (GlfwConfig::default().glfw_callback)(gtx);
            gtx.window_hint(glfw::WindowHint::ScaleToMonitor(true));
            gtx.window_hint(glfw::WindowHint::FocusOnShow(false));
            gtx.window_hint(glfw::WindowHint::Visible(false)); // start hidden, show after positioning
        }),
        opengl_window: Some(false),
        transparent_window: Some(true),
        ..Default::default()
    });
    glfw_backend.window.set_floating(true);
    glfw_backend.window.set_decorated(false);

    let latest_size = glfw_backend.window.get_framebuffer_size();
    let latest_size = [latest_size.0 as _, latest_size.1 as _];

    let mut wgpu_config = WgpuConfig::default();
    wgpu_config.device_descriptor.required_limits.max_texture_dimension_2d = 8192;
    tracing::info!(
        max_tex = wgpu_config.device_descriptor.required_limits.max_texture_dimension_2d,
        "wgpu config limits"
    );

    let default_gfx_backend = WgpuBackend::new(
        wgpu_config,
        Some(Box::new(glfw_backend.window.render_context())),
        latest_size,
    );

    let overlap_app = OverlayApp {
        user_data,
        egui_context: Default::default(),
        default_gfx_backend,
        glfw_backend,
    };
    overlap_app.enter_event_loop();
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

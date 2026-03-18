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
    tracing_subscriber::registry()
        .with(fmt::layer())
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

    // Spawn pane tracking thread for ongoing updates
    let poll_state = state.clone();
    std::thread::spawn(move || {
        pane_poll_loop(target, poll_state);
    });

    start_overlay(Hud {
        first_frame: true,
        state,
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

    let rect = geometry::compute_overlay_rect(&pane.cell_rect, &metrics, &origin, 3);
    tracing::info!(?rect, "computed overlay rect");
    Some(rect)
}

fn pane_poll_loop(target: anchors::tmux::TmuxTarget, state: Arc<Mutex<PixelRect>>) {
    loop {
        if let Some(rect) = compute_pane_rect(&target) {
            let mut s = state.lock().unwrap();
            if *s != rect {
                tracing::debug!(?rect, "pane rect updated");
                *s = rect;
            }
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

struct Hud {
    first_frame: bool,
    state: Arc<Mutex<PixelRect>>,
}

impl EguiOverlay for Hud {
    fn gui_run(
        &mut self,
        egui_context: &egui::Context,
        _default_gfx_backend: &mut DefaultGfxBackend,
        glfw_backend: &mut GlfwBackend,
    ) {
        let rect = *self.state.lock().unwrap();

        glfw_backend.window.set_pos(rect.x, rect.y);
        glfw_backend.set_window_size([rect.w as f32, rect.h as f32]);
        glfw_backend.set_passthrough(true);

        if self.first_frame {
            self.first_frame = false;
            // Show window only after positioning, so it doesn't flash at default location
            glfw_backend.window.show();
        }

        egui::CentralPanel::default()
            .frame(egui::Frame::none().fill(egui::Color32::from_black_alpha(140)))
            .show(egui_context, |ui| {
                let area = ui.available_rect_before_wrap();
                let painter = ui.painter();

                let bar_count = 30;
                let bar_w = area.width() / bar_count as f32;
                let time = ui.input(|i| i.time);

                for i in 0..bar_count {
                    let phase = i as f64 * 0.3 + time * 1.5;
                    let height_frac = (phase.sin() * 0.4 + 0.5) as f32;
                    let bar_h = area.height() * 0.8 * height_frac;

                    let bar_rect = egui::Rect::from_min_size(
                        egui::pos2(
                            area.left() + i as f32 * bar_w + 1.0,
                            area.bottom() - bar_h,
                        ),
                        egui::vec2(bar_w - 2.0, bar_h),
                    );

                    let r = (height_frac * 255.0) as u8;
                    let g = ((1.0 - height_frac) * 200.0) as u8;
                    let color = egui::Color32::from_rgba_unmultiplied(r, g, 40, 200);
                    painter.rect_filled(bar_rect, 2.0, color);
                }

                painter.text(
                    area.left_top() + egui::vec2(6.0, 4.0),
                    egui::Align2::LEFT_TOP,
                    "$0.00 total | 0 turns | cc-hud POC",
                    egui::FontId::monospace(10.0),
                    egui::Color32::from_white_alpha(180),
                );
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

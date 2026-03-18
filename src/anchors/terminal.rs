use crate::geometry::{CellMetrics, WindowOrigin};

/// Get cell pixel dimensions from a tty via TIOCGWINSZ ioctl.
/// Returns physical pixels. CellMetrics includes scale_factor for conversion.
#[cfg(unix)]
pub fn cell_metrics_from_tty(tty_path: &str) -> Option<CellMetrics> {
    use std::fs::OpenOptions;
    use std::os::unix::io::AsRawFd;

    let file = OpenOptions::new().read(true).open(tty_path).ok()?;
    let fd = file.as_raw_fd();

    let mut ws: libc::winsize = unsafe { std::mem::zeroed() };
    let ret = unsafe { libc::ioctl(fd, libc::TIOCGWINSZ, &mut ws) };

    if ret != 0 || ws.ws_xpixel == 0 || ws.ws_ypixel == 0 {
        return None;
    }

    let cell_w = ws.ws_xpixel as u32 / ws.ws_col as u32;
    let cell_h = ws.ws_ypixel as u32 / ws.ws_row as u32;

    // Heuristic: Retina displays yield cell_w > 14 for standard monospace fonts
    let scale = if cell_w > 14 { 2 } else { 1 };

    Some(CellMetrics { cell_w, cell_h, scale_factor: scale })
}

/// Get terminal emulator window origin via CGWindowListCopyWindowInfo.
/// Returns logical points (what the window server and GLFW use).
#[cfg(target_os = "macos")]
pub fn terminal_window_origin(terminal_pid: i32) -> Option<WindowOrigin> {
    use core_foundation::dictionary::CFDictionaryRef;
    use core_graphics::window::{
        CGWindowListCopyWindowInfo,
        kCGWindowListOptionOnScreenOnly,
        kCGWindowListExcludeDesktopElements,
        kCGNullWindowID,
    };

    let windows_ptr = unsafe {
        CGWindowListCopyWindowInfo(
            kCGWindowListOptionOnScreenOnly | kCGWindowListExcludeDesktopElements,
            kCGNullWindowID,
        )
    };
    if windows_ptr.is_null() { return None; }

    let count = unsafe { core_foundation::array::CFArrayGetCount(windows_ptr as _) };

    // Find the tallest window for this PID (skips tab bars, toolbars)
    let mut best: Option<WindowOrigin> = None;
    let mut best_h: i32 = 0;

    for i in 0..count {
        let dict_ref: CFDictionaryRef = unsafe {
            core_foundation::array::CFArrayGetValueAtIndex(windows_ptr as _, i) as CFDictionaryRef
        };

        let pid = match cf_dict_get_i32(dict_ref, "kCGWindowOwnerPID") {
            Some(p) => p,
            None => continue,
        };
        if pid != terminal_pid { continue; }

        let layer = cf_dict_get_i32(dict_ref, "kCGWindowLayer").unwrap_or(-1);
        if layer != 0 { continue; }

        let bounds_ref = match cf_dict_get_raw(dict_ref, "kCGWindowBounds") {
            Some(b) => b,
            None => continue,
        };
        let x = cf_dict_f64(bounds_ref as CFDictionaryRef, "X") as i32;
        let y = cf_dict_f64(bounds_ref as CFDictionaryRef, "Y") as i32;
        let h = cf_dict_f64(bounds_ref as CFDictionaryRef, "Height") as i32;

        if h < 100 { continue; }

        if h > best_h {
            // Fullscreen heuristic: y==0 means no menu bar, so no titlebar
            let titlebar_h = if y == 0 { 0 } else { 28 };
            best = Some(WindowOrigin { x, y, titlebar_h });
            best_h = h;
        }
    }
    best
}

#[cfg(target_os = "macos")]
fn cf_dict_get_i32(dict: core_foundation::dictionary::CFDictionaryRef, key: &str) -> Option<i32> {
    use core_foundation::base::TCFType;
    use core_foundation::dictionary::CFDictionaryGetValue;
    use core_foundation::number::{CFNumber, CFNumberRef};
    use core_foundation::string::CFString;

    unsafe {
        let cf_key = CFString::new(key);
        let val = CFDictionaryGetValue(dict, cf_key.as_CFTypeRef() as *const _);
        if val.is_null() { return None; }
        let cf_num: CFNumber = TCFType::wrap_under_get_rule(val as CFNumberRef);
        cf_num.to_i32()
    }
}

#[cfg(target_os = "macos")]
fn cf_dict_get_raw(
    dict: core_foundation::dictionary::CFDictionaryRef,
    key: &str,
) -> Option<*const std::ffi::c_void> {
    use core_foundation::base::TCFType;
    use core_foundation::dictionary::CFDictionaryGetValue;
    use core_foundation::string::CFString;

    unsafe {
        let cf_key = CFString::new(key);
        let val = CFDictionaryGetValue(dict, cf_key.as_CFTypeRef() as *const _);
        if val.is_null() { None } else { Some(val) }
    }
}

#[cfg(target_os = "macos")]
fn cf_dict_f64(dict: core_foundation::dictionary::CFDictionaryRef, key: &str) -> f64 {
    use core_foundation::base::TCFType;
    use core_foundation::dictionary::CFDictionaryGetValue;
    use core_foundation::number::{CFNumber, CFNumberRef};
    use core_foundation::string::CFString;

    unsafe {
        let cf_key = CFString::new(key);
        let val = CFDictionaryGetValue(dict, cf_key.as_CFTypeRef() as *const _);
        if val.is_null() { return 0.0; }
        let cf_num: CFNumber = TCFType::wrap_under_get_rule(val as CFNumberRef);
        cf_num.to_f64().unwrap_or(0.0)
    }
}

const KNOWN_TERMINALS: &[&str] = &[
    "iTerm2", "Terminal", "wezterm-gui", "alacritty", "kitty", "WezTerm",
];

/// Find the PID of the terminal emulator running this process.
/// Strategy 1: Walk ppid chain (works when not in tmux).
/// Strategy 2: TERM_PROGRAM env var + CGWindowList (works inside tmux).
pub fn terminal_pid() -> Option<i32> {
    // Strategy 1: ppid walk
    let mut pid = std::process::id() as i32;
    for _ in 0..10 {
        pid = match ppid_of(pid) {
            Some(p) if p > 1 => p,
            _ => break,
        };
        if let Some(name) = proc_name(pid) {
            if KNOWN_TERMINALS.iter().any(|t| *t == name) {
                tracing::debug!(pid, name, "found terminal via ppid walk");
                return Some(pid);
            }
        }
    }

    // Strategy 2: TERM_PROGRAM env var
    let term = std::env::var("TERM_PROGRAM").ok()?;

    // Inside tmux, TERM_PROGRAM="tmux". Walk the tmux CLIENT's ppid chain
    // to find the real terminal. $TMUX = "/path/socket,SERVER_PID,session_index"
    if term == "tmux" || term == "screen" {
        if let Ok(tmux_var) = std::env::var("TMUX") {
            // Parse server PID from $TMUX
            if let Some(server_pid_str) = tmux_var.split(',').nth(1) {
                if let Ok(server_pid) = server_pid_str.parse::<i32>() {
                    // Walk up from tmux server: server → client → terminal
                    // Actually, tmux server is a daemon, its ppid is 1 (launchd)
                    // We need the tmux CLIENT pid. List all tmux processes,
                    // find the client (child of the terminal, parent of server? no...)
                    // Simpler: scan all processes for known terminal binaries
                    tracing::debug!(server_pid, "inside tmux, scanning for real terminal");
                }
            }
        }
        // Fall through to scan for known terminal binaries
        return find_terminal_by_scan();
    }

    let proc_name_hint = match term.as_str() {
        "iTerm.app" => "iTerm2",
        "WezTerm" => "wezterm-gui",
        "Apple_Terminal" => "Terminal",
        "Alacritty" => "alacritty",
        "kitty" => "kitty",
        other => other,
    };

    // Find PID by scanning running processes
    tracing::debug!(proc_name_hint, "searching for terminal process");
    let output = std::process::Command::new("ps")
        .args(["-eo", "pid,comm"])
        .output()
        .ok()?;
    let stdout = String::from_utf8(output.stdout).ok()?;
    let mut candidates: Vec<i32> = Vec::new();
    for line in stdout.lines() {
        let trimmed = line.trim();
        let mut parts = trimmed.splitn(2, char::is_whitespace);
        let pid_str = parts.next().unwrap_or("");
        let comm = parts.next().unwrap_or("").trim();
        let binary = comm.rsplit('/').next().unwrap_or(comm);
        if binary == proc_name_hint {
            if let Ok(pid) = pid_str.parse::<i32>() {
                tracing::debug!(pid, comm, "terminal candidate");
                candidates.push(pid);
            }
        }
    }

    if let Some(&pid) = candidates.first() {
        tracing::info!(pid, candidates = ?candidates, "found terminal via TERM_PROGRAM + ps");
        return Some(pid);
    }

    tracing::warn!(term, "could not find terminal PID");
    None
}

/// Scan all processes for known terminal emulator binaries.
/// Used when TERM_PROGRAM is "tmux" or "screen" and we need the real terminal.
fn find_terminal_by_scan() -> Option<i32> {
    let output = std::process::Command::new("ps")
        .args(["-eo", "pid,comm"])
        .output()
        .ok()?;
    let stdout = String::from_utf8(output.stdout).ok()?;

    for line in stdout.lines() {
        let trimmed = line.trim();
        let mut parts = trimmed.splitn(2, char::is_whitespace);
        let pid_str = parts.next().unwrap_or("");
        let comm = parts.next().unwrap_or("").trim();
        let binary = comm.rsplit('/').next().unwrap_or(comm);
        if KNOWN_TERMINALS.iter().any(|t| *t == binary) {
            if let Ok(pid) = pid_str.parse::<i32>() {
                tracing::info!(pid, binary, "found terminal via process scan");
                return Some(pid);
            }
        }
    }
    tracing::warn!("no known terminal found in process list");
    None
}

fn ppid_of(pid: i32) -> Option<i32> {
    let output = std::process::Command::new("ps")
        .args(["-o", "ppid=", "-p", &pid.to_string()])
        .output()
        .ok()?;
    let stdout = String::from_utf8(output.stdout).ok()?;
    stdout.trim().parse().ok()
}

fn proc_name(pid: i32) -> Option<String> {
    let output = std::process::Command::new("ps")
        .args(["-o", "comm=", "-p", &pid.to_string()])
        .output()
        .ok()?;
    let stdout = String::from_utf8(output.stdout).ok()?;
    let path = stdout.trim();
    // ps returns full path, we want just the binary name
    path.rsplit('/').next().map(|s| s.to_string())
}

#[cfg(test)]
mod tests {
    #[test]
    #[ignore] // needs real tty
    fn cell_metrics_from_real_tty() {
        let m = super::cell_metrics_from_tty("/dev/ttys009").unwrap();
        assert!(m.cell_w > 0);
        assert!(m.cell_h > 0);
        assert!(m.scale_factor >= 1);
    }

    #[test]
    #[ignore] // needs real display
    fn finds_terminal_pid() {
        let pid = super::terminal_pid().unwrap();
        assert!(pid > 0);
    }

    #[test]
    #[ignore] // needs real display + terminal
    fn finds_window_origin() {
        let pid = super::terminal_pid().unwrap();
        let origin = super::terminal_window_origin(pid).unwrap();
        assert!(origin.x >= 0);
        assert!(origin.y >= 0);
    }
}

use crate::geometry::CellRect;

/// A tmux target: either a named session (with optional window/pane) or a global pane ID.
#[derive(Debug, Clone)]
pub enum TmuxTarget {
    /// Global pane id, e.g. "%5". Always unambiguous.
    PaneId(String),
    /// Session target, e.g. "claude:main.0". Passed to `tmux list-panes -t`.
    Session(String),
}

impl TmuxTarget {
    /// Parse from CLI arg. Starts with % = global pane id, otherwise session target.
    pub fn parse(s: &str) -> Self {
        if s.starts_with('%') {
            TmuxTarget::PaneId(s.to_string())
        } else {
            TmuxTarget::Session(s.to_string())
        }
    }

    /// Generate a unique session name for a new cc-hud managed session.
    pub fn new_session() -> String {
        let uuid = uuid_v4_simple();
        format!("cc-hud-{uuid}")
    }
}

/// Parse output of `tmux list-panes -F '#{pane_id}:#{pane_top}:#{pane_left}:#{pane_width}:#{pane_height}:#{pane_tty}'`
pub fn parse_pane_geometry(stdout: &str) -> Vec<PaneInfo> {
    stdout
        .lines()
        .filter_map(|line| {
            let parts: Vec<&str> = line.split(':').collect();
            if parts.len() < 5 {
                return None;
            }
            let raw_id = parts[0].to_string();
            let id_num = raw_id.trim_start_matches('%').parse().ok()?;
            Some(PaneInfo {
                pane_id: raw_id,
                cell_rect: CellRect {
                    id: id_num,
                    top: parts[1].parse().ok()?,
                    left: parts[2].parse().ok()?,
                    width: parts[3].parse().ok()?,
                    height: parts[4].parse().ok()?,
                },
                tty: parts.get(5).map(|s| s.to_string()),
            })
        })
        .collect()
}

pub struct PaneInfo {
    pub pane_id: String,
    pub cell_rect: CellRect,
    pub tty: Option<String>,
}

/// Query panes for a specific target.
pub fn query_panes(target: &TmuxTarget) -> Option<Vec<PaneInfo>> {
    let mut cmd = std::process::Command::new("tmux");
    cmd.args([
        "list-panes",
        "-F",
        "#{pane_id}:#{pane_top}:#{pane_left}:#{pane_width}:#{pane_height}:#{pane_tty}",
    ]);

    match target {
        TmuxTarget::Session(session) => {
            cmd.args(["-t", session]);
        }
        TmuxTarget::PaneId(_) => {
            // list all panes across all sessions so we can find by global id
            cmd.arg("-a");
        }
    }

    let output = cmd.output().ok()?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        tracing::warn!(%stderr, "tmux list-panes failed");
        return None;
    }
    let stdout = String::from_utf8(output.stdout).ok()?;
    tracing::debug!(%stdout, "tmux list-panes output");
    Some(parse_pane_geometry(&stdout))
}

/// Find the target pane. For PaneId, searches globally. For Session, returns first pane.
pub fn find_pane(target: &TmuxTarget) -> Option<PaneInfo> {
    let panes = query_panes(target)?;
    tracing::debug!(
        pane_count = panes.len(),
        pane_ids = ?panes.iter().map(|p| p.pane_id.as_str()).collect::<Vec<_>>(),
        ?target,
        "find_pane candidates"
    );
    match target {
        TmuxTarget::PaneId(id) => panes.into_iter().find(|p| p.pane_id == *id),
        TmuxTarget::Session(_) => panes.into_iter().next(),
    }
}

/// Get the client tty for the current tmux session.
/// Uses the TMUX env var (always set inside tmux) to get the session,
/// then queries list-clients for the terminal's pty.
pub fn client_tty() -> Option<String> {
    // Get session name from current tmux environment
    let session = std::process::Command::new("tmux")
        .args(["display-message", "-p", "#{session_name}"])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| s.trim().to_string())?;
    tracing::info!(%session, "resolved tmux session");

    let output = std::process::Command::new("tmux")
        .args(["list-clients", "-t", &session, "-F", "#{client_tty}"])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .and_then(|o| String::from_utf8(o.stdout).ok())?;
    let tty = output.lines().next().map(|s| s.trim().to_string());
    tracing::info!(?tty, "client tty");
    tty
}

/// Check if the pane's window is the active window in an attached session.
/// Returns false if the session is detached or the window isn't active.
pub fn is_pane_visible(target: &TmuxTarget) -> bool {
    let args = match target {
        TmuxTarget::Session(session) => {
            vec![
                "list-panes",
                "-t",
                session,
                "-F",
                "#{window_active}:#{client_session}",
            ]
        }
        TmuxTarget::PaneId(_) => {
            vec![
                "list-panes",
                "-a",
                "-F",
                "#{pane_id}:#{window_active}:#{session_name}",
            ]
        }
    };

    let output = match std::process::Command::new("tmux").args(&args).output() {
        Ok(o) if o.status.success() => String::from_utf8_lossy(&o.stdout).to_string(),
        _ => return false,
    };

    match target {
        TmuxTarget::Session(_) => {
            // Any line with window_active=1 means visible
            output.lines().any(|l| l.starts_with('1'))
        }
        TmuxTarget::PaneId(id) => {
            // Find the line for our pane, check window_active
            output.lines().any(|l| {
                let parts: Vec<&str> = l.split(':').collect();
                parts.len() >= 2 && parts[0] == id && parts[1] == "1"
            })
        }
    }
}

/// Simple v4-ish UUID from random bytes. No external dep.
fn uuid_v4_simple() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let seed = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    // Mix PID in for uniqueness across rapid launches
    let pid = std::process::id() as u128;
    let mixed = seed ^ (pid << 32) ^ (pid >> 16);
    format!("{:016x}", mixed & 0xFFFF_FFFF_FFFF_FFFF)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_single_pane() {
        let stdout = "%0:0:0:154:40:/dev/ttys009";
        let panes = parse_pane_geometry(stdout);
        assert_eq!(panes.len(), 1);
        assert_eq!(panes[0].pane_id, "%0");
        assert_eq!(panes[0].cell_rect.id, 0);
        assert_eq!(panes[0].cell_rect.width, 154);
        assert_eq!(panes[0].cell_rect.height, 40);
        assert_eq!(panes[0].tty.as_deref(), Some("/dev/ttys009"));
    }

    #[test]
    fn parse_horizontal_split() {
        let stdout = "%0:0:0:154:20:/dev/ttys009\n%1:20:0:154:20:/dev/ttys010";
        let panes = parse_pane_geometry(stdout);
        assert_eq!(panes.len(), 2);
        assert_eq!(panes[0].cell_rect.height, 20);
        assert_eq!(panes[1].cell_rect.top, 20);
        assert_eq!(panes[1].pane_id, "%1");
    }

    #[test]
    fn parse_vertical_split() {
        let stdout = "%0:0:0:77:40:/dev/ttys009\n%1:0:77:77:40:/dev/ttys010";
        let panes = parse_pane_geometry(stdout);
        assert_eq!(panes[1].cell_rect.left, 77);
        assert_eq!(panes[1].cell_rect.width, 77);
    }

    #[test]
    fn parse_no_tty() {
        let stdout = "%0:0:0:80:24";
        let panes = parse_pane_geometry(stdout);
        assert_eq!(panes.len(), 1);
        assert_eq!(panes[0].tty, None);
    }

    #[test]
    fn parse_empty() {
        assert_eq!(parse_pane_geometry("").len(), 0);
    }

    #[test]
    fn parse_garbage() {
        assert_eq!(parse_pane_geometry("not:valid:data").len(), 0);
    }

    #[test]
    fn target_parse_pane_id() {
        let t = TmuxTarget::parse("%5");
        assert!(matches!(t, TmuxTarget::PaneId(ref s) if s == "%5"));
    }

    #[test]
    fn target_parse_session() {
        let t = TmuxTarget::parse("claude:main");
        assert!(matches!(t, TmuxTarget::Session(ref s) if s == "claude:main"));
    }

    #[test]
    fn new_session_name_unique() {
        let a = TmuxTarget::new_session();
        let b = TmuxTarget::new_session();
        assert!(a.starts_with("cc-hud-"));
        // Not guaranteed different in same nanosecond, but practically always
        assert_ne!(a, b);
    }
}

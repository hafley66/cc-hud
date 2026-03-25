# cc-hud

Real-time overlay dashboard for Claude Code sessions. Reads JSONL transcripts from `~/.claude/projects/`, computes per-turn and cumulative cost/token/energy/water metrics, renders as a transparent always-on-top egui overlay via GLFW+wgpu.

## Prerequisites

### Install Rust

If you've never used Rust, install it via [rustup](https://rustup.rs/):

```bash
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
```

Follow the prompts (defaults are fine), then restart your shell or run:

```bash
source "$HOME/.cargo/env"
```

Verify:

```bash
rustc --version   # should print something like: rustc 1.8x.x
cargo --version
```

### System dependencies

**macOS** (primary target): Xcode Command Line Tools required for the linker.

```bash
xcode-select --install
```

No other system dependencies needed. GLFW and wgpu are built from source via Cargo.

**Linux**: You'll need X11/Wayland dev headers and a GPU driver. On Ubuntu/Debian:

```bash
sudo apt install libx11-dev libxrandr-dev libxinerama-dev libxcursor-dev libxi-dev libgl-dev
```

## Build and run

```bash
# Debug build (faster compilation, slower runtime)
cargo run

# Release build (slower compilation, faster runtime, what you want for daily use)
cargo run --release

# Or build first, then run the binary directly
cargo build --release
./target/release/cc-hud
```

The overlay window appears as a transparent always-on-top window. It reads Claude Code session data from `~/.claude/projects/` automatically.

### With tmux (recommended)

The launcher script sets up a tmux session with the overlay tracking your main pane:

```bash
bin/launch              # new tmux session with overlay
bin/launch my-session   # named session
```

If you're already inside tmux, `bin/launch` starts the overlay for your current pane.

### Status line integration

Pipe Claude Code's status line output to the HUD feed:

```jsonc
// ~/.claude/settings.json
{
  "statusLine": {
    "type": "command",
    "command": "~/projects/cc-hud/bin/cc-hud-status.sh"
  }
}
```

### Thinking token capture (optional)

To capture extended thinking tokens (not in the standard JSONL), run a mitmproxy sidecar:

```bash
pip install mitmproxy
mitmdump -s proxy/thinking_capture.py --listen-port 8080 --mode regular

# In another terminal:
HTTPS_PROXY=http://localhost:8080 \
NODE_EXTRA_CA_CERTS=~/.mitmproxy/mitmproxy-ca-cert.pem \
claude
```

## What it shows

- Per-turn cost bars (input breakdown: fresh/cache-read/cache-create) with cumulative cost line overlay
- Per-turn token bars with cumulative token line overlay
- Per-turn energy (Wh) and water (mL) bars with cumulative line overlays
- Unified totals chart: cost + tokens + energy + water normalized on one axis
- Multi-session legend with color coding, click to toggle visibility
- Context window burn rate, compaction detection
- Energy/water estimation with confidence bands (research-backed, see `src/energy.rs`)
- API usage % polling (5h/7d utilization)
- Time-axis mode with nav bar zoom/pan

## Architecture

```
src/
  0_scene.rs          -- Scene tree: HudData -> ChartData (pure functions, no UI)
  1_render_egui.rs    -- Walks scene nodes, emits egui widgets
  energy.rs           -- Energy/carbon/water estimation (pure math, cited research)
  geometry.rs         -- Terminal/window geometry detection
  usage.rs            -- API usage endpoint polling
  main.rs             -- egui overlay: layout, charts, tooltips, hover sync
  main_iced.rs        -- Iced backend (experimental)
  agent_harnesses/    -- JSONL transcript parsers
  anchors/            -- Terminal pane geometry (tmux, macOS window APIs)

bin/
  launch              -- tmux launcher script
  cc-hud-status.sh    -- Status line feed script

proxy/
  thinking_capture.py -- mitmproxy addon for thinking token capture
```

## Running tests

```bash
cargo test
```

Snapshot tests use [insta](https://insta.rs/). To update snapshots after intentional changes:

```bash
cargo install cargo-insta   # one-time
cargo insta test
cargo insta review          # interactive review
# or: cargo insta accept --all
```

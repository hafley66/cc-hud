# AGENTS.md

## Build & Run

```bash
cargo build                    # debug (default egui backend)
cargo build --release          # release
cargo run                      # debug run
cargo run --release            # release run
cargo run -- --big             # big mode (fixed floating window, no tmux)
cargo run -- --history         # load historical sessions
cargo run -- --backup          # additive rsync backup on startup
cargo run --features iced-backend  # experimental iced backend
```

## Test

```bash
cargo test                     # all tests (46 tests, 3 ignored)
cargo insta test               # run snapshot tests
cargo insta review             # interactive snapshot review
cargo insta accept --all       # accept all snapshot changes
```

Snapshot tests use [insta](https://insta.rs/) with YAML format. Snapshots live in `src/snapshots/`. After intentional changes that alter output, update snapshots with `cargo insta accept --all`.

## Project Overview

Real-time transparent overlay dashboard for Claude Code sessions. Reads JSONL transcripts from `~/.claude/projects/`, computes per-turn cost/token/energy/water metrics, renders as an always-on-top egui overlay via GLFW+wgpu.

Primary target: **macOS**. Linux supported with X11/Wayland dev headers.

Rust edition **2024**. MSRV: whatever stable supports edition 2024.

## Architecture

```
src/
  main.rs              # egui overlay entry, all big-mode/small-mode UI, chart rendering (~2300 lines)
  main_iced.rs         # experimental iced backend (not actively used)
  lib.rs               # module declarations, #[path] remapping for numbered files
  0_scene.rs           # Scene tree (framework-agnostic): Node enum, ChartData, build_chart_data()
  1_render_egui.rs     # Walks scene Node tree → egui painter calls
  2_legend.rs          # Legend panel (StripBuilder-based), session rows grouped by cwd
  2_model_registry.rs  # Model profiles: pricing, energy coefficients, context windows. Single source of truth.
  energy.rs            # Energy/carbon/water estimation (pure math, research-backed)
  geometry.rs          # Terminal/window geometry detection, overlay rect computation
  usage.rs             # API usage % polling (5h/7d utilization)
  agent_harnesses/
    mod.rs             # re-exports claude_code + opencode modules
    claude_code.rs     # JSONL transcript parser, HudData, poll_loop, Event types, model_pricing()
    opencode.rs        # OpenCode harness (SQLite-based)
  anchors/
    mod.rs, terminal.rs, tmux.rs  # Terminal pane geometry (tmux, macOS window APIs)
  snapshots/           # insta snapshot files

proxy/
  thinking_capture.py  # mitmproxy addon for thinking token capture

cc-hud-functions.sh    # Shell helpers: cc-cost, cc-cost-all, cc-cost-per-session, cc-files, cc-backup
```

## Key Patterns

### Numbered file naming with #[path]
Files like `0_scene.rs`, `1_render_egui.rs`, `2_legend.rs`, `2_model_registry.rs` use `#[path = "..."]` attributes in `lib.rs`/`main.rs` to map to module names (`scene`, `render_egui`, `legend`, `model_registry`). The numbers indicate load/render layer ordering.

### Scene tree pattern
`0_scene.rs` defines a framework-agnostic `Node` enum. Pure functions produce `Vec<Node>`. Backend-specific code (egui) walks and renders. This separates data preparation from rendering.

### Model registry is the single source of truth
`2_model_registry.rs` contains all model-specific constants: pricing, energy coefficients, context windows. Use `lookup(model_str)` to get a `ModelProfile`. Do not add pricing/energy constants elsewhere.

### Thread architecture
Three background threads: pane geometry polling (100ms), JSONL transcript polling, API usage polling (90s). Data shared via `Arc<Mutex<T>>`.

### Cargo features
Two mutually exclusive backend features: `egui-backend` (default) and `iced-backend` (experimental).

## Code Style

- Rust edition 2024
- `#![allow(dead_code)]` at crate root
- Monospace font IDs throughout UI (`egui::FontId::monospace(...)`)
- Colors defined as const structs (e.g., `Palette` in main.rs, module-level consts in legend/render)
- `egui_extras::StripBuilder` for row layouts (legend panel, bar rows)
- Snapshot tests for energy model validation — each test produces a human-readable multi-line string snapshot
- UI state stored in egui temp storage (`ui.ctx().data_mut(|d| d.insert_temp(...))`) and `egui::Id` keys

## Important Gotchas

- The overlay window is transparent and always-on-top — testing requires a display
- macOS-specific APIs use `objc2`, `core-foundation`, `core-graphics`, `libc` directly
- Billing config persisted at `~/.config/cc-hud/billing.json` — manual JSON, no CLI args for it
- Session data read from `~/.claude/projects/` — the app is read-only on those files
- `main.rs` is ~2300 lines and contains all chart rendering, controls, tooltips, navigation — the largest single file
- Some tests are ignored (e.g., `loads_from_real_db_if_present`) — they depend on local data

## Shell Helpers

`cc-hud-functions.sh` provides CLI cost analysis:
- `cc-cost` / `cc-cost-all` — cost breakdown by model from session JSONL files
- `cc-cost-per-session` — per-session cost sorted descending
- `cc-files` — JSONL file counts
- `cc-backup` — additive-only rsync backup (never overwrites)

Source it: `source cc-hud-functions.sh`

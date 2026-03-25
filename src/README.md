# src/

Numbered files follow dependency/reading order (0 = no deps, higher = more deps).

| File | Purpose |
|------|---------|
| `0_scene.rs` | Framework-agnostic scene tree. Pure functions: `HudData` -> `ChartData`. No UI imports. All chart bar/line/marker data computed here. |
| `1_render_egui.rs` | Walks scene nodes, emits egui widgets. Stateless translator. |
| `energy.rs` | Energy/carbon/water estimation from token counts. Pure math, no UI. Research-backed coefficients with full citations in doc comments. |
| `geometry.rs` | Terminal/window geometry: cell metrics, pane detection, insets. macOS-specific APIs. |
| `usage.rs` | Polls Claude API usage endpoint (`/usage`), tracks 5h/7d utilization snapshots over time. |
| `main.rs` | egui overlay entry point. Layout geometry, chart rendering (egui_plot), tooltip system, hover state sync across charts. |
| `main_iced.rs` | Iced backend (experimental, not actively maintained). |
| `lib.rs` | Re-exports for test visibility. |

## Subdirectories

- `agent_harnesses/` -- Data source parsers (Claude Code JSONL transcripts)
- `anchors/` -- Terminal geometry detection (tmux panes, macOS window position)
- `snapshots/` -- insta test snapshots (auto-generated, do not edit by hand)

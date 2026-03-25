# anchors/

Terminal window geometry detection for overlay positioning.

| File | Purpose |
|------|---------|
| `terminal.rs` | Cell metrics (pixel size per character), window rect, scale factor. macOS implementation uses `CGWindowListCopyWindowInfo` + `NSScreen`. |
| `tmux.rs` | Tmux pane geometry: parses `tmux display-message` output to get the active pane's rect within the terminal window. Converts cell coordinates to pixels using cell metrics from `terminal.rs`. |
| `mod.rs` | Module re-exports. |

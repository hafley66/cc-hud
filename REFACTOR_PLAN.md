# Refactor: Completed

## Changes Made

### 1. Fixed Build Errors
- ✅ Fixed `last_ctx` undefined in `opencode.rs:316` - changed to `last_input_tokens`
- ✅ Fixed `is_active: sess.is_active` in `opencode.rs:312` - set to `false` for opencode
- ✅ Removed unused `last_model` variable in `opencode.rs`

### 2. Added Context Structs to Reduce Function Cardinality

**Added to `2_legend.rs`:**
- `SessionCellTheme` - Theme colors for session cells
- `SubagentToggle<'a>` - Subagent toggle state
- `SessionCellCtx<'a>` - Complete context for rendering session name/stats cells

### 3. Refactored `cell_name_stats()`

**Before:** 11 parameters
```rust
fn cell_name_stats(
    ui: &mut egui::Ui,
    name: &str,
    stats: &LegendStats,
    model: &str,
    harness: &str,
    name_col: egui::Color32,
    dim_col: egui::Color32,
    is_active: bool,
    row_h: f32,
    subagent_toggle: Option<(&SessionData, bool, egui::Id)>,
) -> Option<String>
```

**After:** 1 parameter (context struct)
```rust
fn cell_name_stats(ctx: SessionCellCtx) -> Option<String>
```

### 4. Updated Call Sites

Updated 2 call sites to build `SessionCellCtx` before calling `cell_name_stats()`:
- Line 1174: Flat session rows
- Line 1387: Group header rows

### 5. Harness Name Display

Added harness tags in session stats:
- `[cc]` for Claude Code sessions
- `[oc]` for OpenCode sessions
- Displayed before model tag in stats line

## Benefits

- Reduced function cardinality from 11 to 1 parameter
- Improved type safety (subagent toggle is now a named struct)
- Easier to add new fields - just add to context struct
- No more parameter reordering risks
- User can now distinguish between claude and opencode sessions in the UI

## Test Results

```
test result: ok. 46 passed; 0 failed; 3 ignored
```

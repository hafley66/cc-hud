# Egui 0.29 → 0.33 Migration Plan

## ⚠️ MIGRATION BLOCKED - VERSION CONFLICT WITH egui_overlay

**Status:** Migration cannot proceed to egui 0.32 or 0.33 due to incompatibility with `egui_overlay v0.9`.

**Problem:** `egui_overlay v0.9` depends on `egui v0.29.1` internally, which creates version conflicts when trying to use egui 0.32/0.33. The conflict manifests as:
- Multiple versions of `ecolor` and `emath` in dependency tree
- Type mismatches between different egui versions (Ui, Response, Color32)
- API changes in egui_plot (Line::new requires name parameter)

**Solution:** Wait for `egui_overlay` to release a version compatible with egui 0.32+ before proceeding.

---

## Current State Analysis

Your codebase uses **egui 0.29** with extensive API usage:
- 42+ painter operations
- 27 Stroke usages
- 167+ Color32 operations  
- 12 Frame constructions
- 100+ text rendering calls
- 200+ geometry operations

---

## Critical Breaking Changes by Version

### **egui 0.30.0** (Dec 2024)
**Minimal impact for your codebase:**
- ❗ **MSRV bump to 1.80** (you need Rust 1.80+)
- ✅ All your APIs remain compatible

### **egui 0.31.0** (Feb 2025) ⚠️ **MAJOR BREAKING CHANGES**

#### 1. **Stroke Construction** (CRITICAL - affects 27 locations)
```rust
// BEFORE (0.29):
egui::Stroke::new(1.0, color)

// AFTER (0.31+):
egui::Stroke::new(1.0, color, egui::StrokeKind::Middle)  // For lines
egui::Stroke::new(1.0, color, egui::StrokeKind::Inside) // For rects
```

#### 2. **Rounding → CornerRadius Rename**
```rust
// You use: .rounding(4.0)  
// This f32 method should still work, but egui::Rounding type is now egui::CornerRadius
```

#### 3. **Type Size Reductions**
```rust
// CornerRadius, Margin, Shadow changed from f32 to i8/u8
// Your f32 values will be automatically cast/saturated
```

#### 4. **Frame Sizing Change**
```rust
// Frame now includes stroke width in its sizing
// Check all Frame layouts for visual issues
```

### **egui 0.32.0** (Jul 2025)
**New features, minimal breaking changes:**
- ✅ Adds Atoms, better popups
- ✅ Old tooltip API still works (ported to new API)
- ✅ Minimal impact on your codebase

### **egui 0.33.0** (Oct 2025)
**Plugin system improvements:**
- ✅ New `egui::Plugin` trait (replaces `on_begin_pass`/`on_end_pass`)
- ✅ Better kerning
- ❗ **MSRV bump to 1.88**
- ❗ **`screen_rect` → `content_rect`** deprecation

---

## Migration Checklist

### **Step 1: Pre-Migration Requirements**
```bash
# Check Rust version
rustc --version  # Must be >= 1.88

# If not, update Rust
rustup update stable
rustup default stable
```

### **Step 2: Update Cargo.toml**
```toml
[dependencies]
egui = { version = "0.33", optional = true }
egui_plot = { version = "0.33", optional = true }  # May need update
egui_extras = { version = "0.33", optional = true }  # May need update
egui_overlay = { version = "0.9", default-features = false, features = ["egui_default", "glfw_default", "wgpu"], optional = true }
egui_table = { version = "0.7", optional = true }  # Now compatible!
```

### **Step 3: Critical Code Changes**

#### 3a. Fix Stroke Construction (27 locations)
**Pattern 1: Line operations** (8 matches)
```rust
// BEFORE:
painter.line_segment([p1, p2], egui::Stroke::new(0.5, SEPARATOR))

// AFTER:
painter.line_segment([p1, p2], egui::Stroke::new(0.5, SEPARATOR, egui::StrokeKind::Middle))
```

**Pattern 2: Rect outlines** (1 match)
```rust
// BEFORE:
painter.rect_stroke(rect, 1.0, egui::Stroke::new(1.0, TEXT_DIM))

// AFTER:
painter.rect_stroke(rect, 1.0, egui::Stroke::new(1.0, TEXT_DIM, egui::StrokeKind::Inside))
```

**Pattern 3: Frame strokes** (4 matches)
```rust
// BEFORE:
.stroke(egui::Stroke::new(0.5, SEPARATOR))

// AFTER:
.stroke(egui::Stroke::new(0.5, SEPARATOR, egui::StrokeKind::Inside))
```

**Pattern 4: Other Stroke usages** (14 matches in various contexts)
```rust
// Search and replace all remaining Stroke::new calls:
// Add egui::StrokeKind::Inside for rectangular shapes
// Add egui::StrokeKind::Middle for lines
```

#### 3b. Verify Frame.rounding() (8 matches)
```rust
// Check if these still work or need type change:
.rounding(4.0)
.rounding(5.0)
.rounding(3.0)

// May need to become:
.rounding(egui::CornerRadius::same(4))
```

#### 3c. Check screen_rect usage (2 matches)
```rust
// Search for:
ui.ctx().screen_rect()

// Replace with:
ui.ctx().content_rect()  // for drawing in safe area
// or keep screen_rect if you want to draw in unsafe areas
```

### **Step 4: Build and Test**
```bash
# Clean build
cargo clean

# Build with detailed output
cargo build 2>&1 | tee build_output.txt

# Check for:
# 1. Missing StrokeKind arguments
# 2. Type mismatch errors
# 3. Deprecated warnings
```

### **Step 5: Visual Testing**
```bash
# Run the application and check:
# 1. Frame sizes (should include stroke width)
# 2. Stroke rendering (should look the same)
# 3. Text kerning (should be improved)
# 4. Layout alignment (no regressions)
```

---

## Estimated Effort

| Task | Complexity | Time Estimate |
|------|-----------|---------------|
| Pre-migration checks | Low | 5 min |
| Update Cargo.toml | Low | 2 min |
| Fix Stroke construction | Medium | 30-45 min |
| Verify Frame.rounding() | Low | 10 min |
| Check screen_rect | Low | 5 min |
| Build & fix errors | Medium | 30-60 min |
| Visual testing | Medium | 15-30 min |
| **Total** | | **~1.5-2.5 hours** |

---

## Risk Assessment

### **High Risk:**
- ❌ Stroke construction changes (27 locations, critical for rendering)
- ❌ Frame sizing changes (visual regressions possible)

### **Medium Risk:**
- ⚠️ MSRV bumps (need newer Rust)
- ⚠️ egui_plot/egui_extras version compatibility

### **Low Risk:**
- ✅ Most APIs remain stable
- ✅ Painter operations mostly unchanged
- ✅ Layout/geometry operations stable

---

## Benefits After Migration

✅ **Access to egui_table 0.7** (virtual scrolling, collapsible rows)
✅ **Better text kerning** (improved readability)
✅ **Plugin system** (easier custom functionality)
✅ **Better popup/modal support**
✅ **Improved rendering quality**

---

## Rollback Plan

If migration fails:
```bash
# Revert Cargo.toml changes
git checkout Cargo.toml

# Clean build cache
cargo clean

# Restore egui 0.29
cargo build
```

---

## API Usage Analysis Results

### Files requiring changes:
- `src/main.rs` - Primary UI, charts, rendering
- `src/2_legend.rs` - Legend panel with StripBuilder
- `src/1_render_egui.rs` - Scene tree rendering
- `src/3_legend_table.rs` - Failed table attempt (may delete)

### Specific Stroke locations to fix:

**Line operations (8):**
- `src/main.rs` - Multiple line_segment calls for separators, chart grid lines
- `src/2_legend.rs` - Timeline vertical lines
- `src/1_render_egui.rs` - Scene node connections

**Rect outlines (1):**
- `src/main.rs:1176` - Bar chart outline

**Frame strokes (4):**
- `src/main.rs` - Panel frames
- `src/2_legend.rs` - Legend panel frame
- Multiple other UI frames

**Other Stroke usages (14):**
- Various chart annotations
- Visual indicators
- Grid lines

### Frame.rounding() locations (8):
- Multiple panel frames throughout codebase
- Window decorations
- Control panels

### screen_rect usage (2):
- Need to verify context usage
- Likely safe area handling

---

## Migration Status & Blockers

### Attempted Actions
- ✅ Updated Cargo.toml to egui 0.33
- ✅ Fixed all Stroke::new calls (removed StrokeKind parameter - not needed in 0.33)
- ✅ Fixed Frame::none() → Frame::NONE deprecations
- ✅ Fixed Frame.rounding() → Frame::corner_radius() deprecations
- ✅ Fixed Margin type changes (f32 → i8)
- ❌ **BLOCKED:** Version conflicts with egui_overlay

### Root Cause
```toml
# Current dependencies causing conflict:
egui_overlay = "0.9"  # Uses egui 0.29.1 internally
egui = "0.32" or "0.33"  # Desired version
```

This creates multiple versions in the dependency tree:
- egui 0.29.1 (from egui_overlay)
- egui 0.31.1 (from other deps)
- egui 0.32.3 (requested)

### Error Manifestations
1. **Type mismatches:** `ecolor::Color32` from different versions cannot be converted
2. **API changes:** egui_plot::Line::new() signature changed (requires name parameter)
3. **Trait bound errors:** Response types from different egui versions incompatible

### Resolution Path
1. Monitor `egui_overlay` repository for releases supporting egui 0.32+
2. Consider contributing to egui_overlay if upstream is inactive
3. Alternative: Implement overlay window manually without egui_overlay
4. Alternative: Downgrade other dependencies to match egui 0.29.1 (not recommended)

---

## Next Steps (When egui_overlay becomes compatible)

1. ✅ **Create migration branch** (DONE)
2. ✅ **Save this plan** (DONE)
3. ⏸️ **Wait for egui_overlay 0.10+ with egui 0.32+ support**
4. 🔄 **Re-attempt migration after dependency resolution**
5. 🔄 **Execute Step 1-5 from checklist** (adjusted for actual API changes)
6. 🔄 **Test and verify**
7. 🔄 **Commit changes**
8. 🔄 **Implement egui_table features**

---

## Notes

- Current branch: `egui-0.33-migration`
- Base commit: HEAD of main (6 commits ahead of origin)
- Untracked files will be ignored unless needed
- `.sem/` directory exists for session management

---

## Resources

- egui changelog: https://github.com/emilk/egui/releases
- Migration guides in each release notes
- egui_table documentation: https://docs.rs/egui_table

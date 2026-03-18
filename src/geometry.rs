#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PixelRect {
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CellRect {
    pub id: u32,
    pub top: u32,
    pub left: u32,
    pub width: u32,
    pub height: u32,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CellMetrics {
    pub cell_w: u32,
    pub cell_h: u32,
    pub scale_factor: u32,
}

impl CellMetrics {
    /// Returns logical cell size (physical / scale_factor)
    pub fn logical_cell_w(&self) -> u32 {
        self.cell_w / self.scale_factor
    }

    pub fn logical_cell_h(&self) -> u32 {
        self.cell_h / self.scale_factor
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct WindowOrigin {
    pub x: i32,
    pub y: i32,
    pub titlebar_h: i32,
}

impl WindowOrigin {
    pub fn content_y(&self) -> i32 {
        self.y + self.titlebar_h
    }
}

/// Compute the overlay PixelRect anchored to the bottom `hud_lines` of a pane.
/// All values in logical points (what the window server and GLFW use).
pub fn compute_overlay_rect(
    cell: &CellRect,
    metrics: &CellMetrics,
    origin: &WindowOrigin,
    hud_lines: u32,
) -> PixelRect {
    let cw = metrics.logical_cell_w();
    let ch = metrics.logical_cell_h();

    let pane_x = origin.x + (cell.left * cw) as i32;
    let pane_y = origin.content_y() + (cell.top * ch) as i32;
    let pane_w = (cell.width * cw) as i32;
    let pane_h = (cell.height * ch) as i32;
    let hud_h = (hud_lines * ch) as i32;

    PixelRect {
        x: pane_x,
        y: pane_y + pane_h - hud_h,
        w: pane_w,
        h: hud_h,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn metrics(cell_w: u32, cell_h: u32, scale: u32) -> CellMetrics {
        CellMetrics { cell_w, cell_h, scale_factor: scale }
    }

    fn origin(x: i32, y: i32, titlebar: i32) -> WindowOrigin {
        WindowOrigin { x, y, titlebar_h: titlebar }
    }

    #[test]
    fn single_pane_fullscreen_no_titlebar() {
        let cell = CellRect { id: 0, top: 0, left: 0, width: 154, height: 40 };
        let rect = compute_overlay_rect(&cell, &metrics(22, 50, 2), &origin(0, 0, 0), 3);
        // logical cell: 11x25
        // pane: 0,0 -> 1694x1000
        // hud: bottom 3 lines = 75px tall
        assert_eq!(rect, PixelRect { x: 0, y: 1000 - 75, w: 154 * 11, h: 75 });
    }

    #[test]
    fn with_titlebar_and_window_offset() {
        let cell = CellRect { id: 0, top: 0, left: 0, width: 100, height: 40 };
        let rect = compute_overlay_rect(&cell, &metrics(22, 50, 2), &origin(50, 62, 28), 3);
        // content_y = 62 + 28 = 90
        // pane bottom = 90 + 40*25 = 1090
        // hud top = 1090 - 75 = 1015
        assert_eq!(rect.x, 50);
        assert_eq!(rect.y, 1015);
        assert_eq!(rect.h, 75);
    }

    #[test]
    fn bottom_pane_in_horizontal_split() {
        let cell = CellRect { id: 1, top: 20, left: 0, width: 100, height: 20 };
        let rect = compute_overlay_rect(&cell, &metrics(22, 50, 2), &origin(0, 62, 0), 3);
        // pane_y = 62 + 20*25 = 562
        // pane_bottom = 562 + 20*25 = 1062
        // hud = 1062 - 75 = 987
        assert_eq!(rect.y, 987);
        assert_eq!(rect.w, 100 * 11);
    }

    #[test]
    fn right_pane_in_vertical_split() {
        let cell = CellRect { id: 1, top: 0, left: 77, width: 77, height: 40 };
        let rect = compute_overlay_rect(&cell, &metrics(22, 50, 2), &origin(0, 0, 0), 3);
        assert_eq!(rect.x, 77 * 11);
        assert_eq!(rect.w, 77 * 11);
    }

    #[test]
    fn scale_factor_1x() {
        let cell = CellRect { id: 0, top: 0, left: 0, width: 80, height: 24 };
        let rect = compute_overlay_rect(&cell, &metrics(8, 16, 1), &origin(0, 0, 0), 3);
        assert_eq!(rect.w, 80 * 8);
        assert_eq!(rect.h, 3 * 16);
    }

    #[test]
    fn cell_metrics_logical() {
        let m = metrics(22, 50, 2);
        assert_eq!(m.logical_cell_w(), 11);
        assert_eq!(m.logical_cell_h(), 25);
    }
}

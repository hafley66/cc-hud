/// Synchronous egui executor for the scene tree.
/// Walks `&[Node]` and emits egui painter/widget calls. No logic, just translation.

use crate::scene::{self, Anchor, Color, Marker, Node};

// ---- color conversion ----

fn color(c: Color) -> egui::Color32 {
    egui::Color32::from_rgba_unmultiplied(c.0, c.1, c.2, c.3)
}

fn stroke(s: &scene::Stroke) -> egui::Stroke {
    egui::Stroke::new(s.width, color(s.color))
}

// ---- palette constants (mirror main.rs Palette, will consolidate later) ----

const TEXT: egui::Color32 = egui::Color32::from_rgba_premultiplied(200, 190, 165, 230);
const TEXT_DIM: egui::Color32 = egui::Color32::from_rgba_premultiplied(130, 120, 100, 180);
const TEXT_BRIGHT: egui::Color32 = egui::Color32::from_rgba_premultiplied(240, 230, 200, 255);

// ---- rendering ----

/// Render a list of nodes into the given ui region.
/// Returns the hover_key of any BarRow that is currently hovered (empty string if none).
pub fn render(ui: &mut egui::Ui, nodes: &[Node]) -> String {
    let mut hovered_key = String::new();
    for node in nodes {
        render_node(ui, node, &mut hovered_key);
    }
    hovered_key
}

fn render_node(ui: &mut egui::Ui, node: &Node, hovered_key: &mut String) {
    match node {
        Node::Panel { children } => {
            for child in children {
                render_node(ui, child, hovered_key);
            }
        }

        Node::Scroll { id, children } => {
            egui::ScrollArea::vertical()
                .id_salt(id.as_str())
                .auto_shrink(false)
                .show(ui, |ui| {
                    for child in children {
                        render_node(ui, child, hovered_key);
                    }
                });
        }

        Node::Clip { children } => {
            for child in children {
                render_node(ui, child, hovered_key);
            }
        }

        Node::SectionLabel { text } => {
            if text.is_empty() {
                ui.add_space(6.0);
            } else {
                ui.add(egui::Label::new(
                    egui::RichText::new(text.as_str())
                        .monospace()
                        .size(10.0)
                        .color(TEXT_DIM),
                ));
            }
        }

        Node::BarRow { label, count, max, color: col, highlighted, hover_key: hk } => {
            let row_h = 16.0;
            let avail_w = ui.available_width();

            let (rect, resp) = ui.allocate_exact_size(
                egui::vec2(avail_w, row_h),
                egui::Sense::hover(),
            );
            let is_hovered = resp.hovered();
            if is_hovered {
                if let Some(k) = hk {
                    *hovered_key = k.clone();
                }
            }
            let show_highlight = is_hovered || *highlighted;

            if show_highlight {
                ui.painter().rect_filled(rect, 2.0, egui::Color32::from_rgba_unmultiplied(255, 255, 255, 12));
            }

            let mut child = ui.new_child(egui::UiBuilder::new().max_rect(rect));
            egui_extras::StripBuilder::new(&mut child)
                .size(egui_extras::Size::exact(60.0))
                .size(egui_extras::Size::remainder())
                .size(egui_extras::Size::exact(28.0))
                .horizontal(|mut strip| {
                    strip.cell(|ui| {
                        ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
                            ui.add(egui::Label::new(
                                egui::RichText::new(label.as_str())
                                    .monospace().size(10.5)
                                    .color(if show_highlight { TEXT_BRIGHT } else { TEXT })
                            ));
                        });
                    });
                    strip.cell(|ui| {
                        let bar_rect = ui.max_rect();
                        let bar_max_w = bar_rect.width();
                        let bar_w = (*count as f32 / max) * bar_max_w;
                        if bar_w > 0.5 {
                            ui.painter().rect_filled(
                                egui::Rect::from_min_size(
                                    egui::pos2(bar_rect.left(), bar_rect.top() + row_h * 0.25),
                                    egui::vec2(bar_w, row_h * 0.5),
                                ),
                                2.0,
                                color(*col),
                            );
                        }
                    });
                    strip.cell(|ui| {
                        ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
                            ui.add(egui::Label::new(
                                egui::RichText::new(&count.to_string())
                                    .monospace().size(10.0)
                                    .color(TEXT_DIM)
                            ));
                        });
                    });
                });
        }

        Node::ChartLabel { title, left_sub, right_sub } => {
            let avail = ui.available_rect_before_wrap();
            let painter = ui.painter();
            painter.text(
                egui::pos2(avail.left() + 2.0, avail.top()),
                egui::Align2::LEFT_TOP,
                title.as_str(),
                egui::FontId::monospace(9.0),
                TEXT_DIM,
            );
            if !left_sub.is_empty() {
                painter.text(
                    egui::pos2(avail.right() - 40.0, avail.top()),
                    egui::Align2::LEFT_TOP,
                    left_sub.as_str(),
                    egui::FontId::monospace(7.5),
                    TEXT_DIM,
                );
            }
            if !right_sub.is_empty() {
                painter.text(
                    egui::pos2(avail.right() - 2.0, avail.top()),
                    egui::Align2::RIGHT_TOP,
                    right_sub.as_str(),
                    egui::FontId::monospace(7.5),
                    TEXT_DIM,
                );
            }
            ui.add_space(12.0);
        }

        Node::Text { anchor, text, font_size, color: col } => {
            let avail = ui.available_rect_before_wrap();
            let align = match anchor {
                Anchor::LeftCenter => egui::Align2::LEFT_CENTER,
                Anchor::LeftTop => egui::Align2::LEFT_TOP,
                Anchor::RightCenter => egui::Align2::RIGHT_CENTER,
                Anchor::CenterCenter => egui::Align2::CENTER_CENTER,
            };
            ui.painter().text(
                avail.center(),
                align,
                text.as_str(),
                egui::FontId::monospace(*font_size),
                color(*col),
            );
        }

        Node::Rect { rounding, color: col } => {
            let rect = ui.available_rect_before_wrap();
            ui.painter().rect_filled(rect, *rounding, color(*col));
        }

        Node::Circle { radius, color: col } => {
            let rect = ui.available_rect_before_wrap();
            ui.painter().circle_filled(rect.center(), *radius, color(*col));
        }

        Node::Line { a, b, stroke: s } => {
            ui.painter().line_segment(
                [egui::pos2(a.0, a.1), egui::pos2(b.0, b.1)],
                stroke(s),
            );
        }

        Node::Tooltip { lines } => {
            // Tooltip rendering handled by caller positioning -- this just emits text
            for line in lines {
                let col = line.color.map(color).unwrap_or(TEXT);
                ui.colored_label(col, &line.text);
            }
        }

        // Chart nodes are placeholders -- these need egui_plot integration.
        // For now they're no-ops; will be implemented when charts are migrated.
        Node::BarChart { .. } | Node::LineChart { .. } | Node::HLine { .. } => {}

        Node::LegendRow { .. } | Node::SubagentRow { .. } => {
            // Will be implemented when legend is migrated
        }
    }
}

/// Render marker vlines into an egui_plot PlotUi.
/// This bridges the scene Marker type into the egui_plot API directly,
/// since plot widgets need special handling.
pub fn render_markers(pui: &mut egui_plot::PlotUi, markers: &[Marker]) {
    use egui_plot::VLine;
    for m in markers {
        pui.vline(VLine::new(m.x).color(color(m.color)).width(m.width));
    }
}

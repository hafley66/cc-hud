use cc_hud::agent_harnesses::claude_code::{self, HudData};
use cc_hud::scene::{self, Node, ChartData, Color, format_cost, session_color};

use iced::widget::{canvas, column, container, scrollable, text};
use iced::{Element, Length, Settings, Theme, Size, Rectangle, Renderer};
use iced::widget::canvas::{Cache, Frame, Geometry, Path, Stroke as IcedStroke};

use std::collections::HashSet;
use std::sync::{Arc, Mutex};
use std::time::Duration;

fn scene_to_iced(c: Color) -> iced::Color {
    iced::Color::from_rgba8(c.0, c.1, c.2, c.3 as f32 / 255.0)
}

fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or(tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    let args: Vec<String> = std::env::args().collect();
    let show_history = args.iter().any(|a| a == "--history" || a == "-H");
    let big_mode = args.iter().any(|a| a == "--big" || a == "-b");

    let hud_data = Arc::new(Mutex::new(HudData::default()));

    let feed_data = hud_data.clone();
    std::thread::spawn(move || {
        claude_code::poll_loop(feed_data, show_history);
    });

    let win_size = if big_mode {
        Size::new(1280.0, 720.0)
    } else {
        Size::new(960.0, 520.0)
    };

    let settings = Settings {
        antialiasing: true,
        ..Settings::default()
    };

    iced::application("cc-hud (iced)", App::update, App::view)
        .settings(settings)
        .window_size(win_size)
        .theme(|_| Theme::Dark)
        .subscription(App::subscription)
        .run_with(move || {
            (App {
                hud_data: hud_data.clone(),
                cache: Cache::new(),
            }, iced::Task::none())
        })
        .expect("failed to run iced app");
}

struct App {
    hud_data: Arc<Mutex<HudData>>,
    cache: Cache,
}

#[derive(Debug, Clone)]
enum Message {
    Tick,
}

impl App {
    fn update(&mut self, message: Message) {
        match message {
            Message::Tick => {
                self.cache.clear();
            }
        }
    }

    fn view(&self) -> Element<'_, Message> {
        let data = self.hud_data.lock().unwrap().clone();
        let cd = scene::build_chart_data(&data, &HashSet::new(), false);

        // Build scene tree from data (pure, framework-agnostic)
        let panel_nodes = scene::build_tool_panel(
            &cd.skill_list,
            &cd.agent_list,
            &cd.read_list,
            &cd.tool_list,
            "",
        );

        // Session summary text
        let session_count = data.sessions.len();
        let total_cost: f64 = data.sessions.iter().map(|s| s.total_cost_usd).sum();
        let active_count = data.sessions.iter().filter(|s| s.is_active).count();

        let header = text(format!(
            "{} sessions ({} active)  total: {}",
            session_count, active_count, format_cost(total_cost)
        ))
        .size(14);

        // Render the scene tree panel via iced widgets
        let panel_view: Element<Message> = render_panel_nodes(&panel_nodes);

        // Session legend
        let mut session_col = column![].spacing(4);
        for (i, s) in data.sessions.iter().enumerate() {
            let status = if s.is_active { "[active]" } else { "" };
            let col = session_color(i);
            session_col = session_col.push(
                text(format!(
                    "{} {} {}  api:{} agents:{} {}",
                    s.project,
                    scene::short_model_label(&s.model),
                    format_cost(s.total_cost_usd),
                    s.api_call_count,
                    s.agent_count,
                    status,
                ))
                .size(11)
                .color(scene_to_iced(col)),
            );
        }

        // Chart area: two charts stacked (per-turn cost bars top, total cost lines bottom)
        let chart_area = canvas(HudCanvas { cd })
            .width(Length::Fill)
            .height(Length::Fixed(340.0));

        let content = column![
            header,
            chart_area,
            iced::widget::row![
                scrollable(session_col).width(Length::FillPortion(3)),
                scrollable(panel_view).width(Length::FillPortion(2)),
            ]
            .spacing(8)
        ]
        .spacing(8)
        .padding(12);

        container(content)
            .width(Length::Fill)
            .height(Length::Fill)
            .into()
    }

    fn subscription(&self) -> iced::Subscription<Message> {
        iced::time::every(Duration::from_secs(2)).map(|_| Message::Tick)
    }
}

// ---------------------------------------------------------------------------
// Scene tree -> iced widget translation
// ---------------------------------------------------------------------------

fn render_panel_nodes<'a>(nodes: &[Node]) -> Element<'a, Message> {
    let mut col = column![].spacing(2);

    for node in nodes {
        match node {
            Node::Scroll { children, .. } => {
                return render_panel_nodes(children);
            }
            Node::Panel { children } => {
                return render_panel_nodes(children);
            }
            Node::SectionLabel { text: t } => {
                if t.is_empty() {
                    col = col.push(iced::widget::vertical_space().height(4));
                } else {
                    col = col.push(
                        text(t.clone())
                            .size(10)
                            .color(iced::Color::from_rgb(0.5, 0.47, 0.39)),
                    );
                }
            }
            Node::BarRow { label, count, max, color, highlighted, .. } => {
                let frac = if *max > 0.0 { *count as f32 / max } else { 0.0 };
                let bar_color = scene_to_iced(*color);
                let label_color = if *highlighted {
                    iced::Color::from_rgb(0.94, 0.90, 0.78)
                } else {
                    iced::Color::from_rgb(0.78, 0.74, 0.65)
                };

                col = col.push(
                    iced::widget::row![
                        text(label.clone()).size(11).width(Length::Fixed(70.0)).color(label_color),
                        canvas(BarCanvas { frac, color: bar_color })
                            .width(Length::Fill)
                            .height(Length::Fixed(12.0)),
                        text(format!("{}", count)).size(10).width(Length::Fixed(28.0))
                            .color(iced::Color::from_rgb(0.5, 0.47, 0.39)),
                    ]
                    .spacing(4)
                    .align_y(iced::Alignment::Center),
                );
            }
            _ => {}
        }
    }

    col.into()
}

// ---------------------------------------------------------------------------
// Canvas widgets for custom drawing
// ---------------------------------------------------------------------------

struct BarCanvas {
    frac: f32,
    color: iced::Color,
}

impl<Message> canvas::Program<Message> for BarCanvas {
    type State = ();

    fn draw(
        &self,
        _state: &(),
        renderer: &Renderer,
        _theme: &Theme,
        bounds: Rectangle,
        _cursor: iced::mouse::Cursor,
    ) -> Vec<Geometry> {
        let mut frame = Frame::new(renderer, bounds.size());
        let w = bounds.width * self.frac;
        if w > 0.5 {
            frame.fill_rectangle(
                iced::Point::ORIGIN,
                Size::new(w, bounds.height),
                self.color,
            );
        }
        vec![frame.into_geometry()]
    }
}

struct HudCanvas {
    cd: ChartData,
}

impl<Message> canvas::Program<Message> for HudCanvas {
    type State = ();

    fn draw(
        &self,
        _state: &(),
        renderer: &Renderer,
        _theme: &Theme,
        bounds: Rectangle,
        _cursor: iced::mouse::Cursor,
    ) -> Vec<Geometry> {
        let mut frame = Frame::new(renderer, bounds.size());
        let cd = &self.cd;

        if cd.in_cost_bars.is_empty() && cd.combined_cost_pts.is_empty() {
            return vec![frame.into_geometry()];
        }

        let pad = 24.0_f32;
        let gap = 12.0_f32;
        let label_w = 50.0_f32;
        let full_w = bounds.width - pad * 2.0;
        let full_h = bounds.height - pad * 2.0;

        // Layout: left 60% = per-turn cost bars, right 40% = total cost lines
        let left_w = (full_w * 0.58).floor();
        let right_w = full_w - left_w - gap;
        let chart_h = full_h;

        // --- Left chart: per-turn cost bars (bidirectional) ---
        let left_x = pad + label_w;
        let left_chart_w = left_w - label_w;
        let mid_y = pad + chart_h * 0.5; // zero line

        // Find x range and y range
        let x_min = cd.in_cost_bars.first().map(|b| b.x).unwrap_or(0.0);
        let x_max = cd.in_cost_bars.last().map(|b| b.x).unwrap_or(1.0);
        let x_span = (x_max - x_min).max(1.0);
        let y_max = cd.per_turn_in_cost_max.max(cd.per_turn_out_cost_max).max(0.001);

        // Axes
        let axis_color = iced::Color::from_rgba(0.4, 0.37, 0.3, 0.6);
        // zero line
        frame.stroke(
            &Path::line(
                iced::Point::new(left_x, mid_y),
                iced::Point::new(left_x + left_chart_w, mid_y),
            ),
            IcedStroke::default().with_color(axis_color).with_width(0.5),
        );
        // left axis
        frame.stroke(
            &Path::line(
                iced::Point::new(left_x, pad),
                iced::Point::new(left_x, pad + chart_h),
            ),
            IcedStroke::default().with_color(axis_color).with_width(0.5),
        );

        let half_h = chart_h * 0.5;

        // Draw input cost bars (upward from midline)
        let in_color = iced::Color::from_rgba8(100, 160, 220, 180.0 / 255.0);
        for bar in &cd.in_cost_bars {
            let bx = left_x + ((bar.x - x_min) / x_span) as f32 * left_chart_w;
            let bh = (bar.height / y_max) as f32 * half_h;
            let bw = (bar.width / x_span) as f32 * left_chart_w;
            if bh > 0.3 {
                frame.fill_rectangle(
                    iced::Point::new(bx - bw * 0.5, mid_y - bh),
                    Size::new(bw.max(1.0), bh),
                    in_color,
                );
            }
        }

        // Draw output cost bars (downward from midline)
        let out_color = iced::Color::from_rgba8(220, 160, 60, 180.0 / 255.0);
        for bar in &cd.out_cost_bars {
            let bx = left_x + ((bar.x - x_min) / x_span) as f32 * left_chart_w;
            let bh = (bar.height.abs() / y_max) as f32 * half_h;
            let bw = (bar.width / x_span) as f32 * left_chart_w;
            if bh > 0.3 {
                frame.fill_rectangle(
                    iced::Point::new(bx - bw * 0.5, mid_y),
                    Size::new(bw.max(1.0), bh),
                    out_color,
                );
            }
        }

        // Draw agent/skill markers as vertical lines
        let marker_nodes = scene::build_markers(&cd.agent_xs, &cd.skill_xs, &cd.compaction_xs, "");
        for m in &marker_nodes {
            let mx = left_x + ((m.x - x_min) / x_span) as f32 * left_chart_w;
            frame.stroke(
                &Path::line(
                    iced::Point::new(mx, pad),
                    iced::Point::new(mx, pad + chart_h),
                ),
                IcedStroke::default()
                    .with_color(scene_to_iced(m.color))
                    .with_width(m.width),
            );
        }

        // Y-axis labels for per-turn chart
        let label_color = iced::Color::from_rgb(0.5, 0.47, 0.39);
        frame.fill_text(canvas::Text {
            content: format_cost(y_max),
            position: iced::Point::new(pad, pad + 2.0),
            color: label_color,
            size: iced::Pixels(9.0),
            ..Default::default()
        });
        frame.fill_text(canvas::Text {
            content: format!("-{}", format_cost(y_max)),
            position: iced::Point::new(pad, pad + chart_h - 12.0),
            color: label_color,
            size: iced::Pixels(9.0),
            ..Default::default()
        });

        // Chart title
        frame.fill_text(canvas::Text {
            content: "cost/turn".into(),
            position: iced::Point::new(left_x + 4.0, pad + 2.0),
            color: iced::Color::from_rgb(0.78, 0.74, 0.65),
            size: iced::Pixels(10.0),
            ..Default::default()
        });
        // Legend labels
        frame.fill_text(canvas::Text {
            content: "in".into(),
            position: iced::Point::new(left_x + left_chart_w - 40.0, pad + 2.0),
            color: in_color,
            size: iced::Pixels(9.0),
            ..Default::default()
        });
        frame.fill_text(canvas::Text {
            content: "out".into(),
            position: iced::Point::new(left_x + left_chart_w - 20.0, pad + 2.0),
            color: out_color,
            size: iced::Pixels(9.0),
            ..Default::default()
        });

        // --- Right chart: cumulative cost per session ---
        let right_x = pad + left_w + gap + label_w;
        let right_chart_w = right_w - label_w;

        // Axes
        frame.stroke(
            &Path::line(
                iced::Point::new(right_x, pad),
                iced::Point::new(right_x, pad + chart_h),
            ),
            IcedStroke::default().with_color(axis_color).with_width(0.5),
        );
        frame.stroke(
            &Path::line(
                iced::Point::new(right_x, pad + chart_h),
                iced::Point::new(right_x + right_chart_w, pad + chart_h),
            ),
            IcedStroke::default().with_color(axis_color).with_width(0.5),
        );

        // Per-session total cost lines
        for (color, pts) in &cd.total_cost_lines {
            if pts.len() < 2 { continue; }
            let line_color = scene_to_iced(*color);
            let path = Path::new(|builder| {
                for (i, pt) in pts.iter().enumerate() {
                    let px = right_x + ((pt[0] - x_min) / x_span) as f32 * right_chart_w;
                    let py = pad + chart_h - (pt[1] / cd.total_cost_max) as f32 * chart_h;
                    if i == 0 {
                        builder.move_to(iced::Point::new(px, py));
                    } else {
                        builder.line_to(iced::Point::new(px, py));
                    }
                }
            });
            frame.stroke(&path, IcedStroke::default().with_color(line_color).with_width(1.5));
        }

        // Combined cost line (dashed-ish, thicker, white-ish)
        if cd.combined_cost_pts.len() >= 2 {
            let combined_color = iced::Color::from_rgba(0.9, 0.87, 0.78, 0.5);
            let path = Path::new(|builder| {
                for (i, pt) in cd.combined_cost_pts.iter().enumerate() {
                    let px = right_x + ((pt[0] - x_min) / x_span) as f32 * right_chart_w;
                    let py = pad + chart_h - (pt[1] / cd.combined_cost_max) as f32 * chart_h;
                    if i == 0 {
                        builder.move_to(iced::Point::new(px, py));
                    } else {
                        builder.line_to(iced::Point::new(px, py));
                    }
                }
            });
            frame.stroke(&path, IcedStroke::default().with_color(combined_color).with_width(2.0));
        }

        // Y-axis labels for total cost
        frame.fill_text(canvas::Text {
            content: format_cost(cd.total_cost_max),
            position: iced::Point::new(pad + left_w + gap, pad + 2.0),
            color: label_color,
            size: iced::Pixels(9.0),
            ..Default::default()
        });

        // Chart title
        frame.fill_text(canvas::Text {
            content: "total cost".into(),
            position: iced::Point::new(right_x + 4.0, pad + 2.0),
            color: iced::Color::from_rgb(0.78, 0.74, 0.65),
            size: iced::Pixels(10.0),
            ..Default::default()
        });

        // Total cost label (top-right)
        frame.fill_text(canvas::Text {
            content: format_cost(cd.combined_cost_max),
            position: iced::Point::new(right_x + right_chart_w - 60.0, pad + 2.0),
            color: iced::Color::from_rgb(0.94, 0.90, 0.78),
            size: iced::Pixels(11.0),
            ..Default::default()
        });

        vec![frame.into_geometry()]
    }
}

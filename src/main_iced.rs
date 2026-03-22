use cc_hud::agent_harnesses::claude_code::{self, HudData};
use cc_hud::scene::{self, Node};

use iced::widget::{canvas, column, container, scrollable, text};
use iced::{Element, Length, Settings, Theme, Size, Rectangle, Renderer};
use iced::widget::canvas::{Cache, Frame, Geometry, Path, Stroke as IcedStroke};

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

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

        // Build scene tree from data (pure, framework-agnostic)
        let panel_nodes = scene::build_tool_panel(
            &aggregate_skills(&data),
            &aggregate_agents(&data),
            &aggregate_reads(&data),
            &aggregate_tools(&data),
            "",
        );

        // Session summary text
        let session_count = data.sessions.len();
        let total_cost: f64 = data.sessions.iter().map(|s| s.total_cost_usd).sum();
        let active_count = data.sessions.iter().filter(|s| s.is_active).count();

        let header = text(format!(
            "{} sessions ({} active)  total: ${:.4}",
            session_count, active_count, total_cost
        ))
        .size(14);

        // Render the scene tree panel via iced widgets
        let panel_view: Element<Message> = render_panel_nodes(&panel_nodes);

        // Session list
        let mut session_col = column![].spacing(4);
        for s in &data.sessions {
            let status = if s.is_active { "[active]" } else { "" };
            session_col = session_col.push(
                text(format!(
                    "{} {} ${:.4}  api:{} agents:{} {}",
                    s.project,
                    cc_hud::scene::short_model_label(&s.model),
                    s.total_cost_usd,
                    s.api_call_count,
                    s.agent_count,
                    status,
                ))
                .size(11),
            );
        }

        let chart_area = canvas(HudCanvas {
            data: data.clone(),
        })
        .width(Length::Fill)
        .height(Length::Fixed(300.0));

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
                let bar_color = iced::Color::from_rgba8(color.0, color.1, color.2, color.3 as f32 / 255.0);
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
    data: HudData,
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

        if self.data.sessions.is_empty() {
            return vec![frame.into_geometry()];
        }

        // Collect all API call costs for a simple cost-over-time chart
        let mut points: Vec<(f64, f64)> = Vec::new();
        let mut running = 0.0f64;
        for session in &self.data.sessions {
            for ev in &session.events {
                if let claude_code::Event::ApiCall { input_cost_usd, output_cost_usd, .. } = ev {
                    running += input_cost_usd + output_cost_usd;
                    points.push((points.len() as f64, running));
                }
            }
        }

        if points.len() < 2 {
            return vec![frame.into_geometry()];
        }

        let max_y = running.max(0.001);
        let max_x = points.len() as f64;
        let pad = 20.0;
        let chart_w = bounds.width - pad * 2.0;
        let chart_h = bounds.height - pad * 2.0;

        // Draw axes
        let axis_color = iced::Color::from_rgba(0.4, 0.37, 0.3, 0.6);
        frame.stroke(
            &Path::line(
                iced::Point::new(pad, pad),
                iced::Point::new(pad, pad + chart_h),
            ),
            IcedStroke::default().with_color(axis_color).with_width(1.0),
        );
        frame.stroke(
            &Path::line(
                iced::Point::new(pad, pad + chart_h),
                iced::Point::new(pad + chart_w, pad + chart_h),
            ),
            IcedStroke::default().with_color(axis_color).with_width(1.0),
        );

        // Draw cost line
        let line_color = iced::Color::from_rgba(0.39, 0.63, 0.86, 0.8);
        let path = Path::new(|builder| {
            for (i, (x, y)) in points.iter().enumerate() {
                let px = pad + (*x / max_x) as f32 * chart_w;
                let py = pad + chart_h - (*y / max_y) as f32 * chart_h;
                if i == 0 {
                    builder.move_to(iced::Point::new(px, py));
                } else {
                    builder.line_to(iced::Point::new(px, py));
                }
            }
        });
        frame.stroke(&path, IcedStroke::default().with_color(line_color).with_width(1.5));

        // Draw agent spawn markers
        let mut turn_idx = 0usize;
        for session in &self.data.sessions {
            for ev in &session.events {
                match ev {
                    claude_code::Event::ApiCall { .. } => { turn_idx += 1; }
                    claude_code::Event::AgentSpawn { .. } => {
                        let px = pad + (turn_idx as f32 / max_x as f32) * chart_w;
                        frame.stroke(
                            &Path::line(
                                iced::Point::new(px, pad),
                                iced::Point::new(px, pad + chart_h),
                            ),
                            IcedStroke::default()
                                .with_color(iced::Color::from_rgba(0.7, 0.24, 0.24, 0.3))
                                .with_width(1.0),
                        );
                    }
                    claude_code::Event::SkillUse { .. } => {
                        let px = pad + (turn_idx as f32 / max_x as f32) * chart_w;
                        frame.stroke(
                            &Path::line(
                                iced::Point::new(px, pad),
                                iced::Point::new(px, pad + chart_h),
                            ),
                            IcedStroke::default()
                                .with_color(iced::Color::from_rgba(0.24, 0.7, 0.47, 0.3))
                                .with_width(1.0),
                        );
                    }
                    _ => {}
                }
            }
        }

        // Total cost label
        frame.fill_text(canvas::Text {
            content: format!("${:.4}", running),
            position: iced::Point::new(pad + 4.0, pad + 2.0),
            color: iced::Color::from_rgb(0.78, 0.74, 0.65),
            size: iced::Pixels(11.0),
            ..Default::default()
        });

        vec![frame.into_geometry()]
    }
}

// ---------------------------------------------------------------------------
// Aggregation helpers (pull from HudData, same logic as build_chart_data)
// ---------------------------------------------------------------------------

fn aggregate_tools(data: &HudData) -> Vec<(String, u32)> {
    let mut m: HashMap<String, u32> = HashMap::new();
    for s in &data.sessions {
        for (k, v) in &s.tool_counts { *m.entry(k.clone()).or_default() += v; }
    }
    let mut v: Vec<_> = m.into_iter().collect();
    v.sort_by(|a, b| b.1.cmp(&a.1));
    v
}

fn aggregate_skills(data: &HudData) -> Vec<(String, u32)> {
    let mut m: HashMap<String, u32> = HashMap::new();
    for s in &data.sessions {
        for (k, v) in &s.skill_counts { *m.entry(k.clone()).or_default() += v; }
    }
    let mut v: Vec<_> = m.into_iter().collect();
    v.sort_by(|a, b| b.1.cmp(&a.1));
    v
}

fn aggregate_reads(data: &HudData) -> Vec<(String, u32)> {
    let mut m: HashMap<String, u32> = HashMap::new();
    for s in &data.sessions {
        for (k, v) in &s.read_counts { *m.entry(k.clone()).or_default() += v; }
    }
    let mut v: Vec<_> = m.into_iter().collect();
    v.sort_by(|a, b| b.1.cmp(&a.1));
    v
}

fn aggregate_agents(data: &HudData) -> Vec<(String, u32)> {
    let mut m: HashMap<String, u32> = HashMap::new();
    for s in &data.sessions {
        for a in &s.subagents { *m.entry(a.agent_type.clone()).or_default() += 1; }
    }
    let mut v: Vec<_> = m.into_iter().collect();
    v.sort_by(|a, b| b.1.cmp(&a.1));
    v
}

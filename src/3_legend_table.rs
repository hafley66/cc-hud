use egui_extras::Size;
use egui_table::{AutoSizeMode, CellInfo, Column, HeaderCellInfo, HeaderRow, Table, TableDelegate};
use std::collections::HashSet;

use crate::agent_harnesses::claude_code::{HudData, SessionData, SubagentData};
use crate::scene;

// ---------------------------------------------------------------------------
// Palette (subset mirrored from main.rs, avoids cross-module coupling)
// ---------------------------------------------------------------------------

const TEXT: egui::Color32 = egui::Color32::from_rgba_premultiplied(200, 190, 165, 230);
const TEXT_DIM: egui::Color32 = egui::Color32::from_rgba_premultiplied(130, 120, 100, 180);
const TEXT_BRIGHT: egui::Color32 = egui::Color32::from_rgba_premultiplied(240, 230, 200, 255);
const BG_PANEL: egui::Color32 = egui::Color32::from_rgba_premultiplied(18, 15, 10, 220);
const SEPARATOR: egui::Color32 = egui::Color32::from_rgba_premultiplied(60, 55, 42, 120);

fn scene_to_egui(c: scene::Color) -> egui::Color32 {
    egui::Color32::from_rgba_unmultiplied(c.0, c.1, c.2, c.3)
}

fn panel_frame() -> egui::Frame {
    egui::Frame::NONE
        .fill(BG_PANEL)
        .stroke(egui::Stroke::new(0.5, SEPARATOR))
        .corner_radius(4.0)
        .inner_margin(egui::Margin::same(6))
}

fn format_duration_secs(first: u64, last: u64) -> String {
    if first == 0 || last == 0 || last < first {
        return String::new();
    }
    let secs = last - first;
    if secs < 60 {
        format!("{}s", secs)
    } else if secs < 3600 {
        format!("{}m", secs / 60)
    } else {
        format!("{:.1}h", secs as f64 / 3600.0)
    }
}

// ---------------------------------------------------------------------------
// Legend stats (moved from main.rs)
// ---------------------------------------------------------------------------

pub(crate) struct LegendStats {
    pub cost: f64,
    pub last_input: u64,
    pub total_tokens: u64,
    pub session_count: u32,
    pub api_call_count: u32,
    pub agent_cost: f64,
}

impl LegendStats {
    fn avg_tokens_per_session(&self) -> u64 {
        if self.session_count > 0 {
            self.total_tokens / self.session_count as u64
        } else {
            0
        }
    }
    fn avg_cost_per_session(&self) -> f64 {
        if self.session_count > 0 {
            self.cost / self.session_count as f64
        } else {
            0.0
        }
    }
}

// ---------------------------------------------------------------------------
// Legend highlight (stored in egui temp data, read by charts)
// ---------------------------------------------------------------------------

/// Session time ranges highlighted from legend hover (minutes-from-epoch).
#[derive(Clone, Default)]
pub(crate) struct LegendHighlight {
    pub ranges: Vec<(f64, f64)>,
}

// ---------------------------------------------------------------------------
// Eye state enum
// ---------------------------------------------------------------------------

pub enum EyeState {
    Session { in_filter: bool },
    Group { all_hidden: bool, none_hidden: bool },
}

// ---------------------------------------------------------------------------
// Table row model
// ---------------------------------------------------------------------------

#[derive(Clone)]
enum TableRow {
    FlatSession {
        si: usize,
        gi: usize,
    },
    GroupHeader {
        gi: usize,
        cwd: String,
        member_session_indices: Vec<usize>,
    },
    GroupMember {
        si: usize,
        gi: usize,
    },
    SessionExpanded {
        si: usize,
        gi: usize,
        event_idx: usize,
    },
    Subagent {
        si: usize,
        gi: usize,
        ai: usize,
        is_last: bool,
        indent: f32,
    },
}

struct LegendTableData {
    rows: Vec<TableRow>,
    data: HudData,
    filter_set: HashSet<String>,
    effective_hidden: HashSet<String>,
    expanded_groups: HashSet<String>,
    expanded_sessions: HashSet<String>,
    week_start_secs: u64,
    week_span: f32,
    legend_hl_id: egui::Id,
    row_h: f32,
    timeline_w: f32,
    eye_w: f32,
    actions: LegendActions,
}

impl LegendTableData {
    fn new(
        data: HudData,
        groups: &[(String, Vec<(usize, usize)>)],
        filter_set: HashSet<String>,
        effective_hidden: HashSet<String>,
        expanded_groups: HashSet<String>,
        expanded_sessions: HashSet<String>,
        week_start_secs: u64,
        week_span: f32,
        row_h: f32,
        timeline_w: f32,
        eye_w: f32,
    ) -> Self {
        let mut rows = Vec::new();
        for (gi, (cwd, members)) in groups.iter().enumerate() {
            let is_flat = members.len() <= 2;
            if is_flat {
                for (si, _) in members {
                    rows.push(TableRow::FlatSession { si: *si, gi });
                    if expanded_sessions.contains(&data.sessions[*si].session_id) {
                        let event_count = data.sessions[*si].events.len();
                        for event_idx in 0..event_count {
                            rows.push(TableRow::SessionExpanded {
                                si: *si,
                                gi,
                                event_idx,
                            });
                        }
                        let sub_count = data.sessions[*si].subagents.len();
                        for ai in 0..sub_count {
                            rows.push(TableRow::Subagent {
                                si: *si,
                                gi,
                                ai,
                                is_last: ai == sub_count - 1,
                                indent: 20.0,
                            });
                        }
                    }
                }
            } else {
                let member_sis: Vec<usize> = members.iter().map(|(si, _)| *si).collect();
                rows.push(TableRow::GroupHeader {
                    gi,
                    cwd: cwd.clone(),
                    member_session_indices: member_sis,
                });
                if expanded_groups.contains(cwd) {
                    for (si, _) in members {
                        rows.push(TableRow::GroupMember { si: *si, gi });
                        if expanded_sessions.contains(&data.sessions[*si].session_id) {
                            let event_count = data.sessions[*si].events.len();
                            for event_idx in 0..event_count {
                                rows.push(TableRow::SessionExpanded {
                                    si: *si,
                                    gi,
                                    event_idx,
                                });
                            }
                            let sub_count = data.sessions[*si].subagents.len();
                            for ai in 0..sub_count {
                                rows.push(TableRow::Subagent {
                                    si: *si,
                                    gi,
                                    ai,
                                    is_last: ai == sub_count - 1,
                                    indent: 36.0,
                                });
                            }
                        }
                    }
                }
            }
        }

        Self {
            rows,
            data,
            filter_set,
            effective_hidden,
            expanded_groups,
            expanded_sessions,
            week_start_secs,
            week_span,
            legend_hl_id: egui::Id::new("legend_highlight"),
            row_h,
            timeline_w,
            eye_w,
            actions: LegendActions::new(),
        }
    }
}

// ---------------------------------------------------------------------------
// Table delegate
// ---------------------------------------------------------------------------

impl TableDelegate for LegendTableData {
    fn header_cell_ui(&mut self, ui: &mut egui::Ui, cell: &HeaderCellInfo) {
        let header_labels = ["", "", "Session", "", "Timeline"];
        let col_idx = cell.col_range.start;
        if col_idx < header_labels.len() {
            ui.label(header_labels[col_idx]);
        }
    }

    fn cell_ui(&mut self, ui: &mut egui::Ui, cell: &CellInfo) {
        let row_idx = cell.row_nr as usize;
        if let Some(row) = self.rows.get(row_idx).cloned() {
            match row {
                TableRow::FlatSession { si, gi } => {
                    self.render_session_cell(ui, cell, si, gi, None);
                }
                TableRow::GroupHeader {
                    gi,
                    cwd,
                    member_session_indices,
                } => {
                    self.render_group_header_cell(ui, cell, gi, &cwd, &member_session_indices);
                }
                TableRow::GroupMember { si, gi } => {
                    self.render_session_cell(ui, cell, si, gi, Some(16.0));
                }
                TableRow::SessionExpanded { si, gi, event_idx } => {
                    self.render_event_cell(ui, cell, si, event_idx);
                }
                TableRow::Subagent { si, ai, indent, .. } => {
                    self.render_subagent_cell(ui, cell, si, ai, indent);
                }
            }
        }
    }

    fn default_row_height(&self) -> f32 {
        self.row_h
    }
}

impl LegendTableData {
    fn render_session_cell(
        &mut self,
        ui: &mut egui::Ui,
        cell: &CellInfo,
        si: usize,
        gi: usize,
        indent: Option<f32>,
    ) {
        let s = &self.data.sessions[si];
        let sess_col = scene_to_egui(scene::session_color(si));
        let is_hidden = self.effective_hidden.contains(&s.session_id);
        let in_filter = self.filter_set.contains(&s.session_id);
        let text_alpha = if is_hidden { 80u8 } else { 230u8 };
        let name_col = egui::Color32::from_rgba_unmultiplied(240, 230, 200, text_alpha);

        let session_id = s.session_id.clone();
        let is_active = s.is_active;
        let last_input_tokens = s.last_input_tokens;
        let project = s.project.clone();
        let model = s.model.clone();
        let harness = s.harness.clone();
        let total_cost_usd = s.total_cost_usd;
        let last_input = s.last_input_tokens;
        let total_tokens = s.total_input + s.total_output;
        let api_call_count = s.api_call_count;
        let agent_cost = s.subagents.iter().map(|a| a.total_cost_usd).sum::<f64>();
        let has_subagents = !s.subagents.is_empty();
        let is_expanded = self.expanded_sessions.contains(&s.session_id);
        let s_for_timeline = s.clone();

        match cell.col_nr {
            0 => {
                // Eye column
                if self.cell_eye(
                    ui,
                    cell.table_id.with(("eye", cell.row_nr)),
                    EyeState::Session { in_filter },
                ) {
                    self.actions.toggle_ids.push(session_id.clone());
                }
            }
            1 => {
                // Swatch column
                self.cell_swatch(ui, sess_col, is_active, last_input_tokens);
            }
            2 => {
                // Name + stats column
                let label = if let Some(_) = indent {
                    let sid_short = if session_id.len() > 8 {
                        &session_id[..8]
                    } else {
                        &session_id
                    };
                    if is_active {
                        format!("  {} (active)", sid_short)
                    } else {
                        format!("  {}", sid_short)
                    }
                } else {
                    project.clone()
                };

                let stats = LegendStats {
                    cost: total_cost_usd,
                    last_input,
                    total_tokens,
                    session_count: 1,
                    api_call_count,
                    agent_cost,
                };

                let tog = if has_subagents { None } else { None };

                if let Some(sid) = self.cell_name_stats(
                    ui, &label, &stats, &model, &harness, name_col, is_active, self.row_h, tog,
                ) {
                    self.actions.toggle_session_agents.push(sid);
                }
            }
            3 => {
                // Detail/expand button column
                if legacy::cell_expand_button(
                    ui,
                    cell.table_id.with(("expand_session", si)),
                    is_expanded,
                ) {
                    self.actions.toggle_session_expand.push(session_id.clone());
                }
            }
            4 => {
                // Timeline column
                let session_ref = (&s_for_timeline, sess_col);
                self.cell_timeline(
                    ui,
                    &[session_ref],
                    self.week_start_secs,
                    self.week_span,
                    sess_col,
                );
            }
            _ => {}
        }
    }

    fn render_group_header_cell(
        &mut self,
        ui: &mut egui::Ui,
        cell: &CellInfo,
        gi: usize,
        cwd: &str,
        member_sis: &[usize],
    ) {
        // Aggregate stats
        let mut g_cost = 0.0f64;
        let mut g_last_input = 0u64;
        let mut any_active = false;
        let mut active_count = 0u32;
        let mut all_hidden = true;
        let mut g_model = String::new();
        let mut g_harness = String::new();
        let mut g_total_tokens = 0u64;
        let mut g_api_calls = 0u32;
        let mut g_agent_cost = 0.0f64;

        for si in member_sis {
            let s = &self.data.sessions[*si];
            g_cost += s.total_cost_usd;
            g_total_tokens += s.total_input + s.total_output;
            g_api_calls += s.api_call_count;
            g_agent_cost += s.subagents.iter().map(|a| a.total_cost_usd).sum::<f64>();
            if s.is_active {
                any_active = true;
                active_count += 1;
                g_model = s.model.clone();
                g_harness = s.harness.clone();
                g_last_input = s.last_input_tokens;
            }
            if !self.effective_hidden.contains(&s.session_id) {
                all_hidden = false;
            }
        }
        if !any_active {
            if let Some(si) = member_sis.first() {
                let s = &self.data.sessions[*si];
                g_last_input = s.last_input_tokens;
                g_model = s.model.clone();
                g_harness = s.harness.clone();
            }
        }

        let group_col = scene_to_egui(scene::session_color(member_sis[0]));
        let is_expanded = self.expanded_groups.contains(cwd);
        let text_alpha = if all_hidden { 80u8 } else { 230u8 };
        let bar_col = egui::Color32::from_rgba_unmultiplied(
            group_col.r(),
            group_col.g(),
            group_col.b(),
            text_alpha,
        );
        let name_col = egui::Color32::from_rgba_unmultiplied(240, 230, 200, text_alpha);
        let none_hidden = member_sis.iter().all(|si| {
            !self
                .effective_hidden
                .contains(&self.data.sessions[*si].session_id)
        });

        let badge = if active_count > 0 {
            format!("{} x{} ({} active)", cwd, member_sis.len(), active_count)
        } else {
            format!("{} x{}", cwd, member_sis.len())
        };
        let arrow = if is_expanded { "\u{25be}" } else { "\u{25b8}" };
        let header_name = format!("{} {}", arrow, badge);

        let stats = LegendStats {
            cost: g_cost,
            last_input: g_last_input,
            total_tokens: g_total_tokens,
            session_count: member_sis.len() as u32,
            api_call_count: g_api_calls,
            agent_cost: g_agent_cost,
        };

        match cell.col_nr {
            0 => {
                // Eye column
                if self.cell_eye(
                    ui,
                    cell.table_id.with(("group_eye", gi)),
                    EyeState::Group {
                        all_hidden,
                        none_hidden,
                    },
                ) {
                    let member_ids: Vec<String> = member_sis
                        .iter()
                        .map(|si| self.data.sessions[*si].session_id.clone())
                        .collect();
                    self.actions.group_toggle = Some((cwd.to_string(), member_ids));
                }
            }
            1 => {
                // Swatch column
                self.cell_swatch(ui, bar_col, any_active, g_last_input);
            }
            2 => {
                // Name + stats column
                let resp = ui.interact(
                    ui.max_rect(),
                    cell.table_id.with(("group_expand", gi)),
                    egui::Sense::click(),
                );
                if resp.clicked() {
                    self.actions.toggle_expand = Some(cwd.to_string());
                }
                if resp.hovered() {
                    ui.painter().rect_filled(
                        ui.max_rect(),
                        2.0,
                        egui::Color32::from_rgba_unmultiplied(255, 255, 255, 12),
                    );
                    ui.ctx().data_mut(|d| {
                        let hl = d.get_temp_mut_or_default::<LegendHighlight>(self.legend_hl_id);
                        for si in member_sis {
                            let s = &self.data.sessions[*si];
                            if s.first_ts > 0 {
                                hl.ranges
                                    .push((s.first_ts as f64 / 60.0, s.last_ts as f64 / 60.0));
                            }
                        }
                    });
                }
                self.cell_name_stats(
                    ui,
                    &header_name,
                    &stats,
                    &g_model,
                    &g_harness,
                    name_col,
                    any_active,
                    self.row_h,
                    None,
                );
            }
            3 => {
                // Detail button column
                if self.cell_detail_button(ui, cell.table_id.with(("detail_grp", gi))) {
                    // Handle detail button click
                }
            }
            4 => {
                // Timeline column
                let sess_refs: Vec<(SessionData, egui::Color32)> = member_sis
                    .iter()
                    .map(|si| {
                        (
                            self.data.sessions[*si].clone(),
                            scene_to_egui(scene::session_color(*si)),
                        )
                    })
                    .collect();

                let sess_ref_pairs: Vec<(&SessionData, egui::Color32)> =
                    sess_refs.iter().map(|(s, c)| (s, *c)).collect();

                self.cell_timeline(
                    ui,
                    &sess_ref_pairs,
                    self.week_start_secs,
                    self.week_span,
                    group_col,
                );
            }
            _ => {}
        }
    }

    fn render_subagent_cell(
        &mut self,
        ui: &mut egui::Ui,
        cell: &CellInfo,
        si: usize,
        ai: usize,
        indent: f32,
    ) {
        let agent = self.data.sessions[si].subagents[ai].clone();

        match cell.col_nr {
            2 => {
                // Tree connector + agent info
                self.render_subagent_entry(ui, &agent, false, indent);
            }
            _ => {}
        }
    }

    fn render_event_cell(
        &mut self,
        ui: &mut egui::Ui,
        cell: &CellInfo,
        si: usize,
        event_idx: usize,
    ) {
        let s = &self.data.sessions[si];
        if let Some(event) = s.events.get(event_idx) {
            let pad = 8.0;
            let time_offset = self.week_start_secs;

            match cell.col_nr {
                0 => {
                    // Empty eye column for events
                }
                1 => {
                    // Empty swatch column for events
                }
                2 => {
                    // Event type and details
                    let rect = ui.max_rect();
                    let cy = rect.center().y;
                    let type_color = match event {
                        crate::agent_harnesses::claude_code::Event::ApiCall { .. } => {
                            egui::Color32::from_rgba_unmultiplied(100, 200, 255, 200)
                        }
                        crate::agent_harnesses::claude_code::Event::ToolUse { .. } => {
                            egui::Color32::from_rgba_unmultiplied(255, 200, 100, 200)
                        }
                        crate::agent_harnesses::claude_code::Event::SkillUse { .. } => {
                            egui::Color32::from_rgba_unmultiplied(150, 255, 150, 200)
                        }
                        crate::agent_harnesses::claude_code::Event::ReadFile { .. } => {
                            egui::Color32::from_rgba_unmultiplied(200, 150, 255, 200)
                        }
                        crate::agent_harnesses::claude_code::Event::AgentSpawn { .. } => {
                            egui::Color32::from_rgba_unmultiplied(255, 150, 200, 200)
                        }
                        crate::agent_harnesses::claude_code::Event::Compaction { .. } => {
                            egui::Color32::from_rgba_unmultiplied(150, 150, 150, 180)
                        }
                    };

                    let (event_type, details, cost_str, model_str) = match event {
                        crate::agent_harnesses::claude_code::Event::ApiCall {
                            input_tokens,
                            output_tokens,
                            model: m,
                            input_cost_usd,
                            output_cost_usd,
                            timestamp_secs,
                            ..
                        } => (
                            "API",
                            format!(
                                "{} in + {} out",
                                scene::format_tokens(*input_tokens),
                                scene::format_tokens(*output_tokens)
                            ),
                            Some(format!(
                                "{}",
                                scene::format_cost(input_cost_usd + output_cost_usd)
                            )),
                            Some(format!("{} | {}", scene::short_model_label(m), {
                                if *timestamp_secs > 0 {
                                    format!("{:.1}m", (*timestamp_secs - time_offset) as f64 / 60.0)
                                } else {
                                    String::new()
                                }
                            })),
                        ),
                        crate::agent_harnesses::claude_code::Event::ToolUse { name, .. } => {
                            ("Tool", name.clone(), None, None)
                        }
                        crate::agent_harnesses::claude_code::Event::SkillUse { skill, .. } => {
                            ("Skill", skill.clone(), None, None)
                        }
                        crate::agent_harnesses::claude_code::Event::ReadFile {
                            category, ..
                        } => ("File", category.clone(), None, None),
                        crate::agent_harnesses::claude_code::Event::AgentSpawn {
                            subagent_type,
                            description,
                            ..
                        } => (
                            "Agent",
                            format!("{}: {}", subagent_type, description),
                            None,
                            None,
                        ),
                        crate::agent_harnesses::claude_code::Event::Compaction { .. } => {
                            ("Compact", "context compaction".to_string(), None, None)
                        }
                    };

                    // Type badge
                    ui.painter().text(
                        egui::pos2(rect.left() + pad, cy - 6.0),
                        egui::Align2::LEFT_CENTER,
                        format!("[{}]", event_type),
                        egui::FontId::monospace(10.0),
                        type_color,
                    );

                    // Details line
                    ui.painter().text(
                        egui::pos2(rect.left() + pad + 50.0, cy - 6.0),
                        egui::Align2::LEFT_CENTER,
                        &details,
                        egui::FontId::monospace(10.0),
                        TEXT_DIM,
                    );

                    // Secondary line (model/time for API calls)
                    if let Some(model_info) = model_str {
                        ui.painter().text(
                            egui::pos2(rect.left() + pad + 50.0, cy + 10.0),
                            egui::Align2::LEFT_CENTER,
                            &model_info,
                            egui::FontId::monospace(9.0),
                            TEXT_DIM,
                        );
                    }

                    // Cost (right aligned)
                    if let Some(cost) = cost_str {
                        ui.painter().text(
                            egui::pos2(rect.right() - pad, cy - 6.0),
                            egui::Align2::RIGHT_CENTER,
                            &cost,
                            egui::FontId::monospace(10.0),
                            TEXT_DIM,
                        );
                    }
                }
                3 => {
                    // Empty detail column for events
                }
                4 => {
                    // Empty timeline column for events
                }
                _ => {}
            }
        }
    }

    // Cell rendering methods
    fn cell_eye(&mut self, ui: &mut egui::Ui, id: egui::Id, state: EyeState) -> bool {
        legacy::cell_eye(ui, id, state)
    }

    fn cell_swatch(
        &mut self,
        ui: &mut egui::Ui,
        color: egui::Color32,
        is_active: bool,
        last_input: u64,
    ) {
        legacy::cell_swatch(ui, color, is_active, last_input, self.row_h);
    }

    fn cell_name_stats(
        &mut self,
        ui: &mut egui::Ui,
        name: &str,
        stats: &LegendStats,
        model: &str,
        harness: &str,
        theme: egui::Color32,
        is_active: bool,
        row_h: f32,
        toggle: Option<(&SessionData, bool, egui::Id)>,
    ) -> Option<String> {
        legacy::cell_name_stats(
            ui, name, stats, model, harness, theme, is_active, row_h, toggle,
        )
    }

    fn cell_detail_button(&mut self, ui: &mut egui::Ui, id: egui::Id) -> bool {
        legacy::cell_detail_button(ui, id)
    }

    fn cell_timeline(
        &mut self,
        ui: &mut egui::Ui,
        sessions: &[(&SessionData, egui::Color32)],
        week_start_secs: u64,
        week_span: f32,
        bg_color: egui::Color32,
    ) {
        legacy::cell_timeline(
            ui,
            sessions,
            &self.effective_hidden,
            week_start_secs,
            week_span,
            bg_color,
            self.row_h,
        );
    }

    fn render_subagent_entry(
        &mut self,
        ui: &mut egui::Ui,
        agent: &SubagentData,
        is_last: bool,
        indent: f32,
    ) {
        legacy::render_subagent_entry(ui, agent, is_last, indent, self.row_h, self.timeline_w);
    }
}

// ---------------------------------------------------------------------------
// Collected actions (applied after rendering)
// ---------------------------------------------------------------------------

pub(crate) struct LegendActions {
    pub toggle_ids: Vec<String>,
    pub group_toggle: Option<(String, Vec<String>)>,
    pub toggle_expand: Option<String>,
    pub toggle_session_agents: Vec<String>,
    pub toggle_session_expand: Vec<String>,
}

impl LegendActions {
    fn new() -> Self {
        Self {
            toggle_ids: vec![],
            group_toggle: None,
            toggle_expand: None,
            toggle_session_agents: vec![],
            toggle_session_expand: vec![],
        }
    }

    pub fn apply(
        self,
        filter_set: &mut HashSet<String>,
        expanded_groups: &mut HashSet<String>,
        expanded_sessions: &mut HashSet<String>,
    ) {
        if let Some(cwd) = self.toggle_expand {
            if expanded_groups.contains(&cwd) {
                expanded_groups.remove(&cwd);
            } else {
                expanded_groups.insert(cwd);
            }
        }
        for sid in self.toggle_session_agents {
            if expanded_sessions.contains(&sid) {
                expanded_sessions.remove(&sid);
            } else {
                expanded_sessions.insert(sid);
            }
        }
        for sid in self.toggle_session_expand {
            if expanded_sessions.contains(&sid) {
                expanded_sessions.remove(&sid);
            } else {
                expanded_sessions.insert(sid);
            }
        }
        if let Some((_cwd, member_ids)) = self.group_toggle {
            let any_in = member_ids.iter().any(|id| filter_set.contains(id));
            if any_in {
                for id in &member_ids {
                    filter_set.remove(id);
                }
            } else {
                for id in member_ids {
                    filter_set.insert(id);
                }
            }
        }
        for id in self.toggle_ids {
            if filter_set.contains(&id) {
                filter_set.remove(&id);
            } else {
                filter_set.insert(id);
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Main entry point
// ---------------------------------------------------------------------------

pub(crate) fn draw_legend_panel(
    ui: &mut egui::Ui,
    legend_rect: egui::Rect,
    data: &HudData,
    groups: &[(String, Vec<(usize, usize)>)],
    filter_set: &HashSet<String>,
    effective_hidden: &HashSet<String>,
    expanded_groups: &HashSet<String>,
    expanded_sessions: &HashSet<String>,
    week_start_secs: u64,
    week_span: f32,
) -> LegendActions {
    let row_h = 42.0_f32;
    let row_gap = 3.0_f32;
    let timeline_w = 120.0_f32;
    let eye_w = 20.0_f32;

    let legend_hl_id = egui::Id::new("legend_highlight");
    ui.ctx()
        .data_mut(|d| d.insert_temp(legend_hl_id, LegendHighlight::default()));

    let mut table_data = LegendTableData::new(
        data.clone(),
        groups,
        filter_set.clone(),
        effective_hidden.clone(),
        expanded_groups.clone(),
        expanded_sessions.clone(),
        week_start_secs,
        week_span,
        row_h,
        timeline_w,
        eye_w,
    );

    let num_rows = table_data.rows.len();

    // Define columns
    let columns = vec![
        Column::new(eye_w).id(egui::Id::new("eye")).resizable(false),
        Column::new(14.0)
            .id(egui::Id::new("swatch"))
            .resizable(false),
        Column::new(300.0)
            .range(150.0..=800.0)
            .id(egui::Id::new("name")),
        Column::new(36.0)
            .id(egui::Id::new("detail"))
            .resizable(false),
        Column::new(timeline_w)
            .id(egui::Id::new("timeline"))
            .resizable(false),
    ];

    ui.allocate_new_ui(egui::UiBuilder::new().max_rect(legend_rect), |ui| {
        panel_frame().show(ui, |ui| {
            // Aggregate stats strip
            legacy::draw_stats_strip(ui, data, effective_hidden);

            // Table
            Table::new()
                .id_salt("legend_table")
                .num_rows(num_rows as u64)
                .columns(columns)
                .num_sticky_cols(0)
                .headers([HeaderRow::new(24.0)])
                .auto_size_mode(AutoSizeMode::OnParentResize)
                .show(ui, &mut table_data);
        });
    });

    table_data.actions
}

// ---------------------------------------------------------------------------
// Legacy module for cell rendering functions
// ---------------------------------------------------------------------------

mod legacy {
    use super::*;
    use egui_extras::StripBuilder;

    /// Eye visibility toggle icon. Returns true if clicked.
    pub fn cell_eye(ui: &mut egui::Ui, id: egui::Id, state: super::EyeState) -> bool {
        let rect = ui.max_rect();
        let resp = ui.interact(rect, id, egui::Sense::click());
        let cx = rect.center().x;
        let cy = rect.center().y;

        match state {
            super::EyeState::Session { in_filter } => {
                let r = 4.0;
                if in_filter {
                    ui.painter().circle_filled(
                        egui::pos2(cx, cy),
                        r,
                        egui::Color32::from_rgba_unmultiplied(200, 190, 170, 180),
                    );
                } else {
                    ui.painter().circle_stroke(
                        egui::pos2(cx, cy),
                        r,
                        egui::Stroke::new(
                            1.0,
                            egui::Color32::from_rgba_unmultiplied(120, 110, 95, 120),
                        ),
                    );
                }
            }
            super::EyeState::Group {
                all_hidden,
                none_hidden,
            } => {
                let r = 4.5;
                if all_hidden {
                    ui.painter().circle_stroke(
                        egui::pos2(cx, cy),
                        r,
                        egui::Stroke::new(
                            1.0,
                            egui::Color32::from_rgba_unmultiplied(180, 170, 150, 120),
                        ),
                    );
                } else if none_hidden {
                    ui.painter().circle_filled(
                        egui::pos2(cx, cy),
                        r,
                        egui::Color32::from_rgba_unmultiplied(200, 190, 170, 180),
                    );
                } else {
                    // Mixed: outline + smaller filled inner
                    ui.painter().circle_stroke(
                        egui::pos2(cx, cy),
                        r,
                        egui::Stroke::new(
                            1.0,
                            egui::Color32::from_rgba_unmultiplied(200, 190, 170, 150),
                        ),
                    );
                    ui.painter().circle_filled(
                        egui::pos2(cx, cy),
                        r * 0.5,
                        egui::Color32::from_rgba_unmultiplied(200, 190, 170, 150),
                    );
                }
                // Hover ring
                if resp.hovered() {
                    ui.painter().circle_stroke(
                        egui::pos2(cx, cy),
                        r + 2.0,
                        egui::Stroke::new(
                            1.0,
                            egui::Color32::from_rgba_unmultiplied(255, 255, 255, 40),
                        ),
                    );
                }
            }
        }
        resp.clicked()
    }

    /// Color swatch bar + active dot.
    pub fn cell_swatch(
        ui: &mut egui::Ui,
        color: egui::Color32,
        is_active: bool,
        last_input: u64,
        row_h: f32,
    ) {
        let rect = ui.max_rect();
        let bar_x = rect.left() + 1.0;
        let bar_top = rect.top() + (row_h * 0.1).max(2.0);
        let bar_h = row_h - (row_h * 0.2).max(4.0);
        let bar_w = 8.0;

        if is_active {
            let faded = egui::Color32::from_rgba_unmultiplied(color.r(), color.g(), color.b(), 35);
            ui.painter().rect_filled(
                egui::Rect::from_min_size(egui::pos2(bar_x, bar_top), egui::vec2(bar_w, bar_h)),
                2.0,
                faded,
            );
            let ctx_frac = (last_input as f32 / 200_000.0).clamp(0.02, 1.0);
            let swatch_h = (bar_h * ctx_frac).max(3.0);
            let swatch_top = bar_top + bar_h - swatch_h;
            ui.painter().rect_filled(
                egui::Rect::from_min_size(
                    egui::pos2(bar_x, swatch_top),
                    egui::vec2(bar_w, swatch_h),
                ),
                2.0,
                color,
            );
        } else {
            ui.painter().rect_filled(
                egui::Rect::from_min_size(egui::pos2(bar_x, bar_top), egui::vec2(bar_w, bar_h)),
                2.0,
                color,
            );
        }

        // Active dot
        let dot_x = bar_x + bar_w + 3.0;
        let dot_y = bar_top + bar_h * 0.25;
        if is_active {
            ui.painter().circle_filled(
                egui::pos2(dot_x, dot_y),
                2.5,
                egui::Color32::from_rgba_unmultiplied(80, 220, 120, 200),
            );
        } else {
            ui.painter().circle_filled(
                egui::pos2(dot_x, dot_y),
                2.0,
                egui::Color32::from_rgba_unmultiplied(80, 75, 65, 100),
            );
        }
    }

    /// Subagent toggle state
    pub struct SubagentToggle<'a> {
        pub session: &'a SessionData,
        pub is_expanded: bool,
        pub id: egui::Id,
    }

    /// Name line + stats line, with optional subagent toggle.
    pub fn cell_name_stats(
        ui: &mut egui::Ui,
        name: &str,
        stats: &LegendStats,
        model: &str,
        harness: &str,
        name_col: egui::Color32,
        is_active: bool,
        row_h: f32,
        subagent_toggle: Option<(&SessionData, bool, egui::Id)>,
    ) -> Option<String> {
        let rect = ui.max_rect();
        let cy = rect.center().y;
        let font_name = egui::FontId::monospace((row_h * 0.35).clamp(9.0, 13.0));
        let font_stat = egui::FontId::monospace((row_h * 0.27).clamp(8.0, 10.0));

        // Name (primary line)
        ui.painter().text(
            egui::pos2(rect.left(), cy - row_h * 0.12),
            egui::Align2::LEFT_CENTER,
            name,
            font_name,
            name_col,
        );

        // Stats (secondary line)
        let dim_col = egui::Color32::from_rgba_unmultiplied(170, 160, 140, 180);
        if row_h >= 22.0 {
            let sy = cy + row_h * 0.18;
            let ag_tag = if stats.agent_cost > 0.0 {
                format!("  {}ag", scene::format_cost(stats.agent_cost))
            } else {
                String::new()
            };
            let stat_str = if stats.session_count > 1 {
                let avg_cost = scene::format_cost(stats.avg_cost_per_session());
                let avg_tok = scene::format_tokens(stats.avg_tokens_per_session());
                if is_active {
                    let ctx_pct = (stats.last_input as f64 / 200_000.0 * 100.0).min(999.0);
                    format!(
                        "{:>3.0}% ctx  {:>8}  {:>6}  avg {}/sesh  {}/sesh{}",
                        ctx_pct,
                        scene::format_cost(stats.cost),
                        scene::format_tokens(stats.total_tokens),
                        avg_cost,
                        avg_tok,
                        ag_tag
                    )
                } else {
                    format!(
                        "{:>8}  {:>6}  avg {}/sesh  {}/sesh{}",
                        scene::format_cost(stats.cost),
                        scene::format_tokens(stats.total_tokens),
                        avg_cost,
                        avg_tok,
                        ag_tag
                    )
                }
            } else if is_active {
                let harness_tag = match harness {
                    "claude" => "[cc]",
                    "opencode" => "[oc]",
                    _ => "",
                };
                let model_tag = if model.is_empty() {
                    ""
                } else {
                    scene::short_model_label(model)
                };
                let ctx_pct = (stats.last_input as f64 / 200_000.0 * 100.0).min(999.0);
                format!(
                    "{:>3.0}% ctx  {:>8}  {:>6}  {} {}{}",
                    ctx_pct,
                    scene::format_cost(stats.cost),
                    scene::format_tokens(stats.total_tokens),
                    harness_tag,
                    model_tag,
                    ag_tag
                )
            } else {
                let harness_tag = match harness {
                    "claude" => "[cc]",
                    "opencode" => "[oc]",
                    _ => "",
                };
                let model_tag = if model.is_empty() {
                    ""
                } else {
                    scene::short_model_label(model)
                };
                format!(
                    "{:>8}  {:>6}  {} {}{}",
                    scene::format_cost(stats.cost),
                    scene::format_tokens(stats.total_tokens),
                    harness_tag,
                    model_tag,
                    ag_tag
                )
            };
            ui.painter().text(
                egui::pos2(rect.left(), sy),
                egui::Align2::LEFT_CENTER,
                &stat_str,
                font_stat,
                dim_col,
            );
        }

        // Subagent toggle (inline, right side of cell)
        let mut toggled_id = None;
        if let Some((session, is_expanded, tog_id)) = subagent_toggle {
            if !session.subagents.is_empty() {
                let arrow = if is_expanded { "\u{25be}" } else { "\u{25b8}" };
                let agent_cost_sum: f64 = session.subagents.iter().map(|a| a.total_cost_usd).sum();
                let tog_label = format!(
                    "{} {}ag {}",
                    arrow,
                    session.subagents.len(),
                    scene::format_cost(agent_cost_sum)
                );

                let tog_w = 110.0;
                let tog_rect = egui::Rect::from_min_size(
                    egui::pos2(rect.right() - tog_w, rect.top()),
                    egui::vec2(tog_w, row_h),
                );
                let tog_resp = ui.interact(tog_rect, tog_id, egui::Sense::click());
                if tog_resp.clicked() {
                    toggled_id = Some(session.session_id.clone());
                }
                if tog_resp.hovered() {
                    ui.painter().rect_filled(
                        tog_rect,
                        2.0,
                        egui::Color32::from_rgba_unmultiplied(255, 255, 255, 10),
                    );
                }
                let tog_col = if is_expanded {
                    TEXT_BRIGHT
                } else if tog_resp.hovered() {
                    egui::Color32::from_rgba_unmultiplied(200, 195, 180, 200)
                } else {
                    TEXT_DIM
                };
                ui.painter().text(
                    egui::pos2(tog_rect.left() + 2.0, tog_rect.center().y),
                    egui::Align2::LEFT_CENTER,
                    &tog_label,
                    egui::FontId::monospace(10.0),
                    tog_col,
                );
            }
        }
        toggled_id
    }

    /// Detail [->] button. Returns true if clicked.
    pub fn cell_detail_button(ui: &mut egui::Ui, id: egui::Id) -> bool {
        let rect = ui.max_rect();
        let resp = ui.interact(rect, id, egui::Sense::click());
        if resp.hovered() {
            ui.painter().rect_filled(
                rect,
                2.0,
                egui::Color32::from_rgba_unmultiplied(255, 255, 255, 20),
            );
        }
        ui.painter().text(
            egui::pos2(rect.center().x, rect.center().y - 4.0),
            egui::Align2::CENTER_CENTER,
            "\u{2192}",
            egui::FontId::monospace(18.0),
            TEXT_DIM,
        );
        ui.painter().text(
            egui::pos2(rect.center().x, rect.center().y + 10.0),
            egui::Align2::CENTER_CENTER,
            "detail",
            egui::FontId::monospace(7.0),
            TEXT_DIM,
        );
        resp.clicked()
    }

    /// Expand/collapse button for session events.
    pub fn cell_expand_button(ui: &mut egui::Ui, id: egui::Id, is_expanded: bool) -> bool {
        let rect = ui.max_rect();
        let resp = ui.interact(rect, id, egui::Sense::click());
        if resp.hovered() {
            ui.painter().rect_filled(
                rect,
                2.0,
                egui::Color32::from_rgba_unmultiplied(255, 255, 255, 20),
            );
        }
        let icon = if is_expanded { "\u{25be}" } else { "\u{25b8}" };
        ui.painter().text(
            egui::pos2(rect.center().x, rect.center().y),
            egui::Align2::CENTER_CENTER,
            icon,
            egui::FontId::monospace(16.0),
            TEXT_DIM,
        );
        resp.clicked()
    }

    /// Mini timeline with per-session lanes.
    pub fn cell_timeline(
        ui: &mut egui::Ui,
        sessions: &[(&SessionData, egui::Color32)],
        effective_hidden: &HashSet<String>,
        week_start_secs: u64,
        week_span: f32,
        bg_color: egui::Color32,
        row_h: f32,
    ) {
        let rect = ui.max_rect();
        let bar_top = rect.top() + (row_h * 0.1).max(2.0);
        let bar_h = row_h - (row_h * 0.2).max(4.0);
        let tl_rect = egui::Rect::from_min_size(
            egui::pos2(rect.left(), bar_top),
            egui::vec2(rect.width(), bar_h),
        );

        // Timeline background
        ui.painter().rect_filled(
            tl_rect,
            2.0,
            egui::Color32::from_rgba_unmultiplied(bg_color.r(), bg_color.g(), bg_color.b(), 18),
        );

        let n_sessions = sessions.len();
        let seg_h = (bar_h / n_sessions.max(1) as f32).max(2.0);

        for (lane, (s, col)) in sessions.iter().enumerate() {
            let seg_alpha = if effective_hidden.contains(&s.session_id) {
                30u8
            } else if s.is_active {
                220u8
            } else {
                80u8
            };
            let seg_col =
                egui::Color32::from_rgba_unmultiplied(col.r(), col.g(), col.b(), seg_alpha);
            let seg_top = tl_rect.top() + lane as f32 * seg_h;
            let seg_bot = (seg_top + seg_h).min(tl_rect.bottom());

            if s.first_ts > 0 {
                let x0f = ((s.first_ts.saturating_sub(week_start_secs)) as f32 / week_span)
                    .clamp(0.0, 1.0);
                let x1f = ((s.last_ts.saturating_sub(week_start_secs)) as f32 / week_span)
                    .clamp(0.0, 1.0);
                let px0 = tl_rect.left() + x0f * tl_rect.width();
                let px1 = (tl_rect.left() + x1f * tl_rect.width())
                    .max(px0 + 3.0)
                    .min(tl_rect.right());
                ui.painter().rect_filled(
                    egui::Rect::from_min_max(egui::pos2(px0, seg_top), egui::pos2(px1, seg_bot)),
                    1.0,
                    seg_col,
                );
                if s.is_active {
                    ui.painter().rect_filled(
                        egui::Rect::from_min_max(
                            egui::pos2(px1 - 2.0, seg_top),
                            egui::pos2(px1, seg_bot),
                        ),
                        0.0,
                        egui::Color32::from_rgba_unmultiplied(80, 220, 120, 160),
                    );
                }
            }
        }
    }

    pub fn draw_stats_strip(ui: &mut egui::Ui, data: &HudData, effective_hidden: &HashSet<String>) {
        let mut agg_cost = 0.0f64;
        let mut agg_tokens = 0u64;
        let mut agg_session_count = 0u32;
        let mut agg_over_200k_count = 0u32;
        let mut agg_over_200k_cost = 0.0f64;
        let mut agg_agent_cost = 0.0f64;
        let mut agg_agent_count = 0u32;
        let mut agg_active_secs = 0u64;
        let mut earliest_ts = u64::MAX;
        let mut latest_ts = 0u64;
        for s in &data.sessions {
            if effective_hidden.contains(&s.session_id) {
                continue;
            }
            agg_cost += s.total_cost_usd;
            agg_tokens += s.total_input + s.total_output;
            agg_session_count += 1;
            if s.total_input + s.total_output > 200_000 {
                agg_over_200k_count += 1;
                agg_over_200k_cost += s.total_cost_usd;
            }
            agg_agent_cost += s.subagents.iter().map(|a| a.total_cost_usd).sum::<f64>();
            agg_agent_count += s.agent_count;
            if s.first_ts > 0 && s.last_ts > s.first_ts {
                agg_active_secs += s.last_ts - s.first_ts;
                if s.first_ts < earliest_ts {
                    earliest_ts = s.first_ts;
                }
                if s.last_ts > latest_ts {
                    latest_ts = s.last_ts;
                }
            }
        }
        let avg_cost_sesh = if agg_session_count > 0 {
            agg_cost / agg_session_count as f64
        } else {
            0.0
        };
        let proj_200k = if agg_tokens > 0 {
            (agg_cost / agg_tokens as f64) * 200_000.0
        } else {
            0.0
        };
        let cptm = if agg_tokens > 0 {
            agg_cost / agg_tokens as f64 * 1_000_000.0
        } else {
            0.0
        };
        let agent_pct = if agg_cost > 0.0 {
            agg_agent_cost / agg_cost * 100.0
        } else {
            0.0
        };
        let avg_agent_cost = if agg_agent_count > 0 {
            agg_agent_cost / agg_agent_count as f64
        } else {
            0.0
        };
        let avg_ag_sesh = if agg_session_count > 0 {
            agg_agent_count as f64 / agg_session_count as f64
        } else {
            0.0
        };
        let avg_over_200k_cost = if agg_over_200k_count > 0 {
            agg_over_200k_cost / agg_over_200k_count as f64
        } else {
            0.0
        };
        let active_hours = agg_active_secs as f64 / 3600.0;
        let span_days = if latest_ts > earliest_ts {
            (latest_ts - earliest_ts) as f64 / 86400.0
        } else {
            1.0
        };
        let cost_per_active_hr = if active_hours > 0.0 {
            agg_cost / active_hours
        } else {
            0.0
        };
        let active_hrs_per_day = active_hours / span_days.max(1.0);

        let strip_h = 32.0;
        let (strip_rect, _) = ui.allocate_exact_size(
            egui::vec2(ui.available_width(), strip_h),
            egui::Sense::hover(),
        );
        let painter = ui.painter();
        let font = egui::FontId::monospace(9.0);
        let pad = 6.0;

        let row1 = format!(
            "avg {}/sesh   proj {}/200k   >200k: {}/{}(avg {})   {}/Mtok   {}/active hr   {:.1}h/day({})",
            scene::format_cost(avg_cost_sesh), scene::format_cost(proj_200k),
            agg_over_200k_count, agg_session_count, scene::format_cost(avg_over_200k_cost),
            scene::format_cost(cptm), scene::format_cost(cost_per_active_hr),
            active_hrs_per_day, scene::format_cost(active_hrs_per_day * cost_per_active_hr),
        );
        let row2 = format!(
            "agents: {:.0}%   avg {}/agent   {:.1} ag/sesh",
            agent_pct,
            scene::format_cost(avg_agent_cost),
            avg_ag_sesh,
        );
        let x = strip_rect.left() + pad;
        painter.text(
            egui::pos2(x, strip_rect.top() + 10.0),
            egui::Align2::LEFT_CENTER,
            &row1,
            font.clone(),
            TEXT_DIM,
        );
        painter.text(
            egui::pos2(x, strip_rect.top() + 22.0),
            egui::Align2::LEFT_CENTER,
            &row2,
            font,
            TEXT_DIM,
        );
        painter.line_segment(
            [
                egui::pos2(strip_rect.left() + 4.0, strip_rect.bottom()),
                egui::pos2(strip_rect.right() - 4.0, strip_rect.bottom()),
            ],
            egui::Stroke::new(1.0, egui::Color32::from_rgba_unmultiplied(80, 80, 80, 60)),
        );
    }

    pub fn render_subagent_entry(
        ui: &mut egui::Ui,
        agent: &SubagentData,
        is_last: bool,
        indent: f32,
        row_h: f32,
        timeline_w: f32,
    ) {
        let tree_col = egui::Color32::from_rgba_unmultiplied(90, 85, 75, 140);
        let name_col = egui::Color32::from_rgba_unmultiplied(200, 195, 180, 200);
        let stat_col = egui::Color32::from_rgba_unmultiplied(140, 135, 120, 170);

        StripBuilder::new(ui)
            .size(Size::exact(indent + 14.0)) // tree connector column
            .size(Size::remainder()) // agent info
            .size(Size::exact(timeline_w)) // spacer (no timeline for agents)
            .horizontal(|mut strip| {
                // Tree connector
                strip.cell(|ui| {
                    let rect = ui.max_rect();
                    let tree_x = rect.left() + indent;
                    let connector = if is_last { "\u{2514}" } else { "\u{251c}" };
                    ui.painter().text(
                        egui::pos2(tree_x, rect.center().y),
                        egui::Align2::CENTER_CENTER,
                        connector,
                        egui::FontId::monospace(12.0),
                        tree_col,
                    );
                    if !is_last {
                        ui.painter().line_segment(
                            [
                                egui::pos2(tree_x, rect.bottom()),
                                egui::pos2(tree_x, rect.bottom() + 3.0),
                            ],
                            egui::Stroke::new(1.0, tree_col),
                        );
                    }
                });

                // Agent info: type badge + description + stats
                strip.cell(|ui| {
                    let rect = ui.max_rect();
                    let type_short = match agent.agent_type.as_str() {
                        "general-purpose" => "agent",
                        "Explore" => "explore",
                        "Plan" => "plan",
                        other => other,
                    };
                    let desc_trunc = if agent.description.len() > 40 {
                        format!("{}...", &agent.description[..37])
                    } else {
                        agent.description.clone()
                    };
                    let agent_label = format!("{} {}", type_short, desc_trunc);
                    ui.painter().text(
                        egui::pos2(rect.left(), rect.center().y - row_h * 0.12),
                        egui::Align2::LEFT_CENTER,
                        &agent_label,
                        egui::FontId::monospace(11.0),
                        name_col,
                    );

                    let model_short = scene::short_model_label(&agent.model);
                    let duration = format_duration_secs(agent.first_ts, agent.last_ts);
                    let total_tok = scene::format_tokens(agent.total_input + agent.total_output);
                    let stat_str = format!(
                        "{}  {}  {} tok  {}  {}calls",
                        model_short,
                        scene::format_cost(agent.total_cost_usd),
                        total_tok,
                        duration,
                        agent.api_call_count
                    );
                    ui.painter().text(
                        egui::pos2(rect.left(), rect.center().y + row_h * 0.18),
                        egui::Align2::LEFT_CENTER,
                        &stat_str,
                        egui::FontId::monospace(9.0),
                        stat_col,
                    );
                });

                // Empty timeline spacer
                strip.cell(|_ui| {});
            });
    }
}

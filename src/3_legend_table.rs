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

fn format_relative_time(ts: u64) -> String {
    if ts == 0 {
        return String::new();
    }
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    if now <= ts {
        return "now".to_string();
    }
    let delta = now - ts;
    if delta < 60 {
        format!("{}s ago", delta)
    } else if delta < 3600 {
        format!("{}m ago", delta / 60)
    } else if delta < 86400 {
        format!("{:.0}h ago", delta as f64 / 3600.0)
    } else {
        format!("{:.0}d ago", delta as f64 / 86400.0)
    }
}

fn format_datestamp(ts: u64) -> String {
    if ts == 0 {
        return String::new();
    }
    let secs = ts as i64;
    let days_since_epoch = secs / 86400;
    let time_of_day = secs % 86400;
    let hour = time_of_day / 3600;
    let min = (time_of_day % 3600) / 60;

    // Approximate month/day from days since epoch (good enough for display)
    let (y, m, d) = civil_from_days(days_since_epoch);
    format!("{:04}-{:02}-{:02} {:02}:{:02}", y, m, d, hour, min)
}

fn civil_from_days(days: i64) -> (i64, u32, u32) {
    // Adapted from Howard Hinnant's chrono-compatible algorithm
    let z = days + 719468;
    let era = if z >= 0 { z } else { z - 146096 } / 146097;
    let doe = (z - era * 146097) as u32;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    (y, m, d)
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
        member_index: usize,
    },
    GroupHeader {
        gi: usize,
        cwd: String,
        member_session_indices: Vec<usize>,
    },
    GroupMember {
        si: usize,
        gi: usize,
        member_index: usize,
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
    focused: Option<String>,
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
        focused: Option<String>,
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
                let n = members.len();
                for (mi, (si, _)) in members.iter().enumerate() {
                    rows.push(TableRow::FlatSession { si: *si, gi, member_index: n - mi });
                    if expanded_sessions.contains(&data.sessions[*si].session_id) {
                        let sub_count = data.sessions[*si].subagents.len();
                        for ai in 0..sub_count {
                            rows.push(TableRow::Subagent {
                                si: *si,
                                gi,
                                ai,
                                is_last: ai == sub_count - 1,
                                indent: 56.0,
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
                    let n = members.len();
                    for (mi, (si, _)) in members.iter().enumerate() {
                        rows.push(TableRow::GroupMember { si: *si, gi, member_index: n - mi });
                        if expanded_sessions.contains(&data.sessions[*si].session_id) {
                            let sub_count = data.sessions[*si].subagents.len();
                            for ai in 0..sub_count {
                                rows.push(TableRow::Subagent {
                                    si: *si,
                                    gi,
                                    ai,
                                    is_last: ai == sub_count - 1,
                                    indent: 72.0,
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
            focused,
            actions: LegendActions::new(),
        }
    }
}

// ---------------------------------------------------------------------------
// Table delegate
// ---------------------------------------------------------------------------

impl TableDelegate for LegendTableData {
    fn header_cell_ui(&mut self, ui: &mut egui::Ui, cell: &HeaderCellInfo) {
        let header_labels = [
            "",
            "",
            "",
            "Session",
            "Start",
            "",
            "Model",
            "ctx",
            "cost",
            "tokens",
            "$/sesh",
            "tok/sesh",
            "agent",
            "cmpct",
            "Timeline",
            "focus",
        ];
        let col_idx = cell.col_range.start;
        if col_idx < header_labels.len() {
            ui.label(header_labels[col_idx]);
        }
    }

    fn cell_ui(&mut self, ui: &mut egui::Ui, cell: &CellInfo) {
        let row_idx = cell.row_nr as usize;
        if let Some(row) = self.rows.get(row_idx).cloned() {
            match row {
                TableRow::FlatSession { si, gi, member_index } => {
                    self.render_session_cell(ui, cell, si, gi, None, member_index);
                }
                TableRow::GroupHeader {
                    gi,
                    cwd,
                    member_session_indices,
                } => {
                    self.render_group_header_cell(ui, cell, gi, &cwd, &member_session_indices);
                }
                TableRow::GroupMember { si, gi, member_index } => {
                    self.render_session_cell(ui, cell, si, gi, Some(40.0), member_index);
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
        member_index: usize,
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

        let compaction_count = s.events.iter().filter(|e| matches!(e, crate::agent_harnesses::claude_code::Event::Compaction { .. })).count() as u32;

        let row_click_id = cell.table_id.with(("row_click_session", si));
        let expand_action = if has_subagents {
            Some(session_id.clone())
        } else {
            None
        };
        let _ = (api_call_count, last_input, total_tokens);

        let first_ts = s.first_ts;
        let last_ts = s.last_ts;

        let maybe_row_interact = |this: &mut LegendTableData, ui: &mut egui::Ui, tag: &str| {
            let resp = legacy::row_click_response(ui, row_click_id.with(tag));
            if resp.clicked() {
                if let Some(sid) = expand_action.clone() {
                    this.actions.toggle_session_agents.push(sid);
                }
            }
            if resp.hovered() {
                let first_min = first_ts as f64 / 60.0;
                let last_min = last_ts as f64 / 60.0;
                ui.ctx().data_mut(|d| {
                    let hl = d.get_temp_mut_or_default::<LegendHighlight>(this.legend_hl_id);
                    hl.ranges.push((first_min, last_min));
                });
            }
        };

        match cell.col_nr {
            0 => {
                let id = cell.table_id.with(("check", cell.row_nr));
                if legacy::cell_checkbox(ui, id, in_filter, false) {
                    self.actions.toggle_ids.push(session_id.clone());
                }
            }
            1 => {
                if has_subagents
                    && legacy::cell_arrow(
                        ui,
                        cell.table_id.with(("arrow_session", si)),
                        is_expanded,
                    )
                {
                    self.actions.toggle_session_agents.push(session_id.clone());
                }
            }
            2 => {
                self.cell_swatch(ui, sess_col, is_active, last_input_tokens);
                maybe_row_interact(self, ui, "swatch");
            }
            3 => {
                let primary = if is_active {
                    format!("{} #{} (active)", project, member_index)
                } else {
                    format!("{} #{}", project, member_index)
                };
                let sid_short = if session_id.len() > 8 {
                    &session_id[..8]
                } else {
                    &session_id
                };
                let indent_px = indent.unwrap_or(0.0);
                legacy::cell_name_only(ui, &primary, Some(sid_short), name_col, self.row_h, indent_px);
                maybe_row_interact(self, ui, "name");
            }
            4 => {
                legacy::cell_start(ui, s.first_ts, self.row_h);
                maybe_row_interact(self, ui, "start");
            }
            5 => {
                let tag = match harness.as_str() {
                    "claude" => "cc",
                    "opencode" => "oc",
                    _ => "",
                };
                legacy::stat_cell(ui, tag, self.row_h, false);
                maybe_row_interact(self, ui, "harness");
            }
            6 => {
                let m = if model.is_empty() {
                    ""
                } else {
                    scene::short_model_label(&model)
                };
                legacy::stat_cell(ui, m, self.row_h, false);
                maybe_row_interact(self, ui, "model");
            }
            7 => {
                let txt = if is_active {
                    format!(
                        "{:.0}%",
                        (last_input_tokens as f64 / 200_000.0 * 100.0).min(999.0)
                    )
                } else {
                    String::new()
                };
                legacy::stat_cell(ui, &txt, self.row_h, is_active);
                maybe_row_interact(self, ui, "ctx");
            }
            8 => {
                legacy::stat_cell(
                    ui,
                    &scene::format_cost(total_cost_usd),
                    self.row_h,
                    true,
                );
                maybe_row_interact(self, ui, "cost");
            }
            9 => {
                legacy::stat_cell(
                    ui,
                    &scene::format_tokens(s.total_input + s.total_output),
                    self.row_h,
                    true,
                );
                maybe_row_interact(self, ui, "tokens");
            }
            10 => {
                maybe_row_interact(self, ui, "avg_cost");
            }
            11 => {
                maybe_row_interact(self, ui, "avg_tok");
            }
            12 => {
                let txt = if agent_cost > 0.0 {
                    scene::format_cost(agent_cost)
                } else {
                    String::new()
                };
                legacy::stat_cell(ui, &txt, self.row_h, false);
                maybe_row_interact(self, ui, "agent");
            }
            13 => {
                if compaction_count > 0 {
                    legacy::stat_cell(ui, &compaction_count.to_string(), self.row_h, false);
                }
                maybe_row_interact(self, ui, "cmpct");
            }
            14 => {
                let session_ref = (&s_for_timeline, sess_col);
                self.cell_timeline(
                    ui,
                    &[session_ref],
                    self.week_start_secs,
                    self.week_span,
                    sess_col,
                );
            }
            15 => {
                let is_focused = self
                    .focused
                    .as_deref()
                    .map(|f| f == session_id)
                    .unwrap_or(false);
                let id = cell.table_id.with(("focus_btn", cell.row_nr));
                if legacy::cell_focus_button(ui, id, is_focused, self.row_h) {
                    self.actions.focus_toggle = Some(session_id.clone());
                }
            }
            _ => {}
        }
        let _ = gi;
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

        let header_name = if active_count > 0 {
            format!("{} x{} ({} active)", cwd, member_sis.len(), active_count)
        } else {
            format!("{} x{}", cwd, member_sis.len())
        };

        let stats = LegendStats {
            cost: g_cost,
            last_input: g_last_input,
            total_tokens: g_total_tokens,
            session_count: member_sis.len() as u32,
            api_call_count: g_api_calls,
            agent_cost: g_agent_cost,
        };

        let row_click_id = cell.table_id.with(("row_click_grp", gi));
        let cwd_string = cwd.to_string();

        let member_ts_ranges: Vec<(f64, f64)> = member_sis
            .iter()
            .filter_map(|si| {
                let s = &self.data.sessions[*si];
                if s.first_ts > 0 {
                    Some((s.first_ts as f64 / 60.0, s.last_ts as f64 / 60.0))
                } else {
                    None
                }
            })
            .collect();

        let row_resp = |this: &mut LegendTableData, ui: &mut egui::Ui, tag: &str| {
            let resp = legacy::row_click_response(ui, row_click_id.with(tag));
            if resp.clicked() {
                this.actions.toggle_expand = Some(cwd_string.clone());
            }
            if resp.hovered() {
                ui.ctx().data_mut(|d| {
                    let hl = d.get_temp_mut_or_default::<LegendHighlight>(this.legend_hl_id);
                    hl.ranges.extend(member_ts_ranges.iter().copied());
                });
            }
        };

        match cell.col_nr {
            0 => {
                let id = cell.table_id.with(("group_check", gi));
                let mixed = !all_hidden && !none_hidden;
                if legacy::cell_checkbox(ui, id, none_hidden, mixed) {
                    let member_ids: Vec<String> = member_sis
                        .iter()
                        .map(|si| self.data.sessions[*si].session_id.clone())
                        .collect();
                    self.actions.group_toggle = Some((cwd_string.clone(), member_ids));
                }
            }
            1 => {
                if legacy::cell_arrow(ui, cell.table_id.with(("arrow_grp", gi)), is_expanded) {
                    self.actions.toggle_expand = Some(cwd_string.clone());
                }
            }
            2 => {
                self.cell_swatch(ui, bar_col, any_active, g_last_input);
                row_resp(self, ui, "swatch");
            }
            3 => {
                legacy::cell_name_only(ui, &header_name, None, name_col, self.row_h, 0.0);
                row_resp(self, ui, "name");
            }
            4 => {
                let newest_ts = member_sis
                    .iter()
                    .map(|si| self.data.sessions[*si].first_ts)
                    .max()
                    .unwrap_or(0);
                legacy::cell_start(ui, newest_ts, self.row_h);
                row_resp(self, ui, "start");
            }
            5 => {
                let tag = match g_harness.as_str() {
                    "claude" => "cc",
                    "opencode" => "oc",
                    _ => "",
                };
                legacy::stat_cell(ui, tag, self.row_h, false);
                row_resp(self, ui, "harness");
            }
            6 => {
                let m = if g_model.is_empty() {
                    ""
                } else {
                    scene::short_model_label(&g_model)
                };
                legacy::stat_cell(ui, m, self.row_h, false);
                row_resp(self, ui, "model");
            }
            7 => {
                let txt = if any_active {
                    format!(
                        "{:.0}%",
                        (stats.last_input as f64 / 200_000.0 * 100.0).min(999.0)
                    )
                } else {
                    String::new()
                };
                legacy::stat_cell(ui, &txt, self.row_h, any_active);
                row_resp(self, ui, "ctx");
            }
            8 => {
                legacy::stat_cell(ui, &scene::format_cost(stats.cost), self.row_h, true);
                row_resp(self, ui, "cost");
            }
            9 => {
                legacy::stat_cell(
                    ui,
                    &scene::format_tokens(stats.total_tokens),
                    self.row_h,
                    true,
                );
                row_resp(self, ui, "tokens");
            }
            10 => {
                let txt = if stats.session_count > 1 {
                    scene::format_cost(stats.avg_cost_per_session())
                } else {
                    String::new()
                };
                legacy::stat_cell(ui, &txt, self.row_h, false);
                row_resp(self, ui, "avg_cost");
            }
            11 => {
                let txt = if stats.session_count > 1 {
                    scene::format_tokens(stats.avg_tokens_per_session())
                } else {
                    String::new()
                };
                legacy::stat_cell(ui, &txt, self.row_h, false);
                row_resp(self, ui, "avg_tok");
            }
            12 => {
                let txt = if stats.agent_cost > 0.0 {
                    scene::format_cost(stats.agent_cost)
                } else {
                    String::new()
                };
                legacy::stat_cell(ui, &txt, self.row_h, false);
                row_resp(self, ui, "agent");
            }
            13 => {
                let total_compactions: u32 = member_sis.iter().map(|si| {
                    self.data.sessions[*si].events.iter()
                        .filter(|e| matches!(e, crate::agent_harnesses::claude_code::Event::Compaction { .. }))
                        .count() as u32
                }).sum();
                let avg = if member_sis.len() > 1 {
                    format!("{:.1}", total_compactions as f64 / member_sis.len() as f64)
                } else if total_compactions > 0 {
                    total_compactions.to_string()
                } else {
                    String::new()
                };
                legacy::stat_cell(ui, &avg, self.row_h, false);
                row_resp(self, ui, "cmpct");
            }
            14 => {
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
            3 => {
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
                let label = format!("{} {}", type_short, desc_trunc);
                let sub = {
                    let model_short = scene::short_model_label(&agent.model);
                    let duration = format_duration_secs(agent.first_ts, agent.last_ts);
                    Some(format!("{}  {}  {}calls", model_short, duration, agent.api_call_count))
                };
                legacy::cell_name_only(
                    ui,
                    &label,
                    sub.as_deref(),
                    egui::Color32::from_rgba_unmultiplied(200, 195, 180, 200),
                    self.row_h,
                    indent,
                );
            }
            6 => {
                legacy::stat_cell(ui, scene::short_model_label(&agent.model), self.row_h, false);
            }
            8 => {
                legacy::stat_cell(ui, &scene::format_cost(agent.total_cost_usd), self.row_h, false);
            }
            9 => {
                legacy::stat_cell(
                    ui,
                    &scene::format_tokens(agent.total_input + agent.total_output),
                    self.row_h,
                    false,
                );
            }
            _ => {}
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
    /// Set when the user clicks a focus button. Toggles focus for this session.
    pub focus_toggle: Option<String>,
}

impl LegendActions {
    fn new() -> Self {
        Self {
            toggle_ids: vec![],
            group_toggle: None,
            toggle_expand: None,
            toggle_session_agents: vec![],
            focus_toggle: None,
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
    focused: Option<&str>,
    week_start_secs: u64,
    week_span: f32,
) -> LegendActions {
    let row_h = 42.0_f32;
    let _row_gap = 3.0_f32;
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
        focused.map(|s| s.to_string()),
        week_start_secs,
        week_span,
        row_h,
        timeline_w,
        eye_w,
    );

    let num_rows = table_data.rows.len();

    let col = |w: f32| Column::new(w).range(w..=w);
    let flex = |w: f32, min: f32| Column::new(w).range(min..=f32::INFINITY);
    let columns = vec![
        col(24.0).id(egui::Id::new("check")),
        col(18.0).id(egui::Id::new("arrow")),
        col(14.0).id(egui::Id::new("swatch")),
        flex(200.0, 140.0).id(egui::Id::new("name")),
        col(96.0).id(egui::Id::new("start")),
        col(28.0).id(egui::Id::new("harness")),
        col(56.0).id(egui::Id::new("model")),
        col(44.0).id(egui::Id::new("ctx")),
        col(80.0).id(egui::Id::new("cost")),
        col(60.0).id(egui::Id::new("tokens")),
        col(72.0).id(egui::Id::new("avg_cost")),
        col(60.0).id(egui::Id::new("avg_tok")),
        col(64.0).id(egui::Id::new("agent")),
        col(48.0).id(egui::Id::new("cmpct")),
        flex(timeline_w, 100.0).id(egui::Id::new("timeline")),
        col(38.0).id(egui::Id::new("focus")),
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

    pub const COL_API: egui::Color32 = egui::Color32::from_rgba_premultiplied(110, 150, 220, 230);
    pub const COL_TOOL: egui::Color32 = egui::Color32::from_rgba_premultiplied(220, 160, 90, 230);
    pub const COL_SKILL: egui::Color32 = egui::Color32::from_rgba_premultiplied(130, 200, 120, 230);
    pub const COL_FILE: egui::Color32 = egui::Color32::from_rgba_premultiplied(180, 140, 220, 230);

    /// Native egui checkbox centered in the cell. Returns true if the checkbox state changed.
    pub fn cell_checkbox(ui: &mut egui::Ui, id: egui::Id, checked: bool, indeterminate: bool) -> bool {
        let rect = ui.max_rect();
        let size = egui::vec2(16.0, 16.0);
        let box_rect = egui::Rect::from_center_size(rect.center(), size);
        let resp = ui.interact(box_rect, id, egui::Sense::click());
        let painter = ui.painter();
        let visuals = ui.style().visuals.clone();

        let stroke_col = if resp.hovered() {
            visuals.widgets.hovered.fg_stroke.color
        } else {
            visuals.widgets.inactive.fg_stroke.color
        };

        painter.rect(
            box_rect,
            2.0,
            visuals.widgets.inactive.bg_fill,
            egui::Stroke::new(1.0, stroke_col),
            egui::StrokeKind::Inside,
        );

        if indeterminate {
            // Horizontal dash
            let y = box_rect.center().y;
            painter.line_segment(
                [
                    egui::pos2(box_rect.left() + 3.0, y),
                    egui::pos2(box_rect.right() - 3.0, y),
                ],
                egui::Stroke::new(2.0, stroke_col),
            );
        } else if checked {
            // Check mark
            let l = box_rect.left();
            let t = box_rect.top();
            let w = box_rect.width();
            let h = box_rect.height();
            let p0 = egui::pos2(l + w * 0.22, t + h * 0.52);
            let p1 = egui::pos2(l + w * 0.44, t + h * 0.72);
            let p2 = egui::pos2(l + w * 0.78, t + h * 0.30);
            painter.line_segment([p0, p1], egui::Stroke::new(2.0, stroke_col));
            painter.line_segment([p1, p2], egui::Stroke::new(2.0, stroke_col));
        }

        resp.clicked()
    }

    /// Expand arrow cell. Returns true if clicked.
    pub fn cell_arrow(ui: &mut egui::Ui, id: egui::Id, is_expanded: bool) -> bool {
        let rect = ui.max_rect();
        let resp = ui.interact(rect, id, egui::Sense::click());
        if resp.hovered() {
            ui.painter().rect_filled(
                rect,
                2.0,
                egui::Color32::from_rgba_unmultiplied(255, 255, 255, 14),
            );
        }
        let icon = if is_expanded { "\u{25be}" } else { "\u{25b8}" };
        let color = if resp.hovered() { TEXT_BRIGHT } else { TEXT };
        ui.painter().text(
            rect.center(),
            egui::Align2::CENTER_CENTER,
            icon,
            egui::FontId::monospace(14.0),
            color,
        );
        resp.clicked()
    }

    /// Transparent click target over the cell's max_rect. Returns true if clicked.
    pub fn row_click(ui: &mut egui::Ui, id: egui::Id) -> bool {
        row_click_response(ui, id).clicked()
    }

    /// Focus/solo button. Filled disc when focused, outlined disc otherwise.
    /// Returns true if clicked (caller toggles focus state).
    pub fn cell_focus_button(
        ui: &mut egui::Ui,
        id: egui::Id,
        is_focused: bool,
        row_h: f32,
    ) -> bool {
        let rect = ui.max_rect();
        let resp = ui.interact(rect, id, egui::Sense::click());
        let painter = ui.painter();
        let hot = resp.hovered() || is_focused;
        if hot {
            painter.rect_filled(
                rect,
                2.0,
                egui::Color32::from_rgba_unmultiplied(
                    255,
                    255,
                    255,
                    if is_focused { 32 } else { 16 },
                ),
            );
        }
        let cy = rect.center().y;
        let glyph = if is_focused { "\u{25C9}" } else { "\u{25CE}" };
        let glyph_col = if is_focused {
            TEXT_BRIGHT
        } else if resp.hovered() {
            TEXT
        } else {
            TEXT_DIM
        };
        painter.text(
            egui::pos2(rect.center().x, cy - row_h * 0.12),
            egui::Align2::CENTER_CENTER,
            glyph,
            egui::FontId::monospace(14.0),
            glyph_col,
        );
        painter.text(
            egui::pos2(rect.center().x, cy + row_h * 0.22),
            egui::Align2::CENTER_CENTER,
            if is_focused { "pinned" } else { "pin" },
            egui::FontId::monospace(7.0),
            glyph_col,
        );
        resp.clicked()
    }

    /// Like row_click but returns the full Response so callers can read hover state.
    pub fn row_click_response(ui: &mut egui::Ui, id: egui::Id) -> egui::Response {
        let rect = ui.max_rect();
        let resp = ui.interact(rect, id, egui::Sense::click());
        if resp.hovered() {
            ui.painter().rect_filled(
                rect,
                0.0,
                egui::Color32::from_rgba_unmultiplied(255, 255, 255, 10),
            );
        }
        resp
    }

    /// Right-aligned, vertically centered numeric cell for event counts.
    pub fn cell_count(ui: &mut egui::Ui, count: u32, color: egui::Color32, row_h: f32) {
        if count == 0 {
            return;
        }
        let rect = ui.max_rect();
        let font = egui::FontId::monospace((row_h * 0.35).clamp(10.0, 14.0));
        ui.painter().text(
            egui::pos2(rect.right() - 6.0, rect.center().y),
            egui::Align2::RIGHT_CENTER,
            count.to_string(),
            font,
            color,
        );
    }

    /// Two-line start time cell: relative time on top, datestamp below.
    pub fn cell_start(ui: &mut egui::Ui, first_ts: u64, row_h: f32) {
        if first_ts == 0 {
            return;
        }
        let rect = ui.max_rect();
        let cy = rect.center().y;
        let rel = format_relative_time(first_ts);
        let stamp = format_datestamp(first_ts);
        // Shorten datestamp to MM-DD HH:MM
        let short_stamp = if stamp.len() >= 16 { &stamp[5..16] } else { &stamp };
        let font_top = egui::FontId::monospace((row_h * 0.30).clamp(9.0, 11.0));
        let font_bot = egui::FontId::monospace((row_h * 0.24).clamp(8.0, 10.0));
        ui.painter().text(
            egui::pos2(rect.left() + 2.0, cy - row_h * 0.14),
            egui::Align2::LEFT_CENTER,
            &rel,
            font_top,
            TEXT,
        );
        ui.painter().text(
            egui::pos2(rect.left() + 2.0, cy + row_h * 0.18),
            egui::Align2::LEFT_CENTER,
            short_stamp,
            font_bot,
            egui::Color32::from_rgba_unmultiplied(150, 140, 120, 170),
        );
    }

    /// Right-aligned text cell for stat values. Empty strings render nothing.
    pub fn stat_cell(ui: &mut egui::Ui, text: &str, row_h: f32, bright: bool) {
        if text.is_empty() {
            return;
        }
        let rect = ui.max_rect();
        let font = egui::FontId::monospace((row_h * 0.30).clamp(9.0, 11.0));
        let col = if bright {
            TEXT
        } else {
            egui::Color32::from_rgba_unmultiplied(170, 160, 140, 200)
        };
        ui.painter().text(
            egui::pos2(rect.right() - 6.0, rect.center().y),
            egui::Align2::RIGHT_CENTER,
            text,
            font,
            col,
        );
    }

    /// Single-line or two-line left-aligned name cell (no stats). `indent_px` is added to the
    /// x offset so nested rows (group members, subagents) visibly step in.
    pub fn cell_name_only(
        ui: &mut egui::Ui,
        name: &str,
        sub: Option<&str>,
        name_col: egui::Color32,
        row_h: f32,
        indent_px: f32,
    ) {
        let rect = ui.max_rect();
        let cy = rect.center().y;
        let x0 = rect.left() + 2.0 + indent_px;
        let font_name = egui::FontId::monospace((row_h * 0.35).clamp(9.0, 13.0));

        // Tree connector glyph for indented rows
        if indent_px >= 8.0 {
            let tree_col = egui::Color32::from_rgba_unmultiplied(90, 85, 75, 140);
            ui.painter().text(
                egui::pos2(rect.left() + indent_px - 10.0, cy),
                egui::Align2::CENTER_CENTER,
                "\u{2514}",
                egui::FontId::monospace(12.0),
                tree_col,
            );
        }

        if let Some(sub_text) = sub {
            ui.painter().text(
                egui::pos2(x0, cy - row_h * 0.16),
                egui::Align2::LEFT_CENTER,
                name,
                font_name,
                name_col,
            );
            let font_sub = egui::FontId::monospace((row_h * 0.24).clamp(8.0, 10.0));
            ui.painter().text(
                egui::pos2(x0, cy + row_h * 0.20),
                egui::Align2::LEFT_CENTER,
                sub_text,
                font_sub,
                egui::Color32::from_rgba_unmultiplied(160, 150, 130, 180),
            );
        } else {
            ui.painter().text(
                egui::pos2(x0, cy),
                egui::Align2::LEFT_CENTER,
                name,
                font_name,
                name_col,
            );
        }
    }

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

use cc_hud::agent_harnesses::claude_code::{parse_jsonl_full, build_session_data, Event};

#[test]
fn parse_sample_session_events() {
    let fixture = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/sample_session.jsonl");
    let (events, _) = parse_jsonl_full(fixture);
    insta::assert_yaml_snapshot!("events", events);
}

#[test]
fn build_sample_session_data() {
    let fixture = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/sample_session.jsonl");
    let (events, _) = parse_jsonl_full(fixture);
    let sd = build_session_data("test-session-001", "cc-hud", "cc-hud", 12345, events, true, vec![]);

    // Sorted tool/skill/read counts for deterministic output
    let mut tool_counts: Vec<_> = sd.tool_counts.into_iter().collect();
    tool_counts.sort_by(|a, b| a.0.cmp(&b.0));
    let mut skill_counts: Vec<_> = sd.skill_counts.into_iter().collect();
    skill_counts.sort_by(|a, b| a.0.cmp(&b.0));
    let mut read_counts: Vec<_> = sd.read_counts.into_iter().collect();
    read_counts.sort_by(|a, b| a.0.cmp(&b.0));

    insta::assert_yaml_snapshot!("session_aggregates", &serde_json::json!({
        "session_id": sd.session_id,
        "total_cost_usd": (sd.total_cost_usd * 100000.0).round() / 100000.0,
        "total_input_cost": (sd.total_input_cost * 100000.0).round() / 100000.0,
        "total_output_cost": (sd.total_output_cost * 100000.0).round() / 100000.0,
        "total_input": sd.total_input,
        "total_output": sd.total_output,
        "api_call_count": sd.api_call_count,
        "agent_count": sd.agent_count,
        "tool_counts": tool_counts,
        "skill_counts": skill_counts,
        "read_counts": read_counts,
        "model": sd.model,
        "is_active": sd.is_active,
    }));
}

#[test]
fn event_type_coverage() {
    let fixture = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/sample_session.jsonl");
    let (events, _) = parse_jsonl_full(fixture);

    let mut has_api = false;
    let mut has_tool = false;
    let mut has_skill = false;
    let mut has_read_file = false;
    let mut has_agent = false;

    for ev in &events {
        match ev {
            Event::ApiCall { .. } => has_api = true,
            Event::ToolUse { .. } => has_tool = true,
            Event::SkillUse { .. } => has_skill = true,
            Event::ReadFile { .. } => has_read_file = true,
            Event::AgentSpawn { .. } => has_agent = true,
        }
    }

    assert!(has_api, "fixture must contain ApiCall events");
    assert!(has_tool, "fixture must contain ToolUse events");
    assert!(has_skill, "fixture must contain SkillUse events");
    assert!(has_read_file, "fixture must contain ReadFile events");
    assert!(has_agent, "fixture must contain AgentSpawn events");
}

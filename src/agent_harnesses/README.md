# agent_harnesses/

Parsers for AI coding tool session data.

| File | Purpose |
|------|---------|
| `claude_code.rs` | Reads `~/.claude/projects/**/*/api_conversation.jsonl`. Extracts `ApiCall` events (tokens, cost, model, timestamps), tool use, agent spawns, skill invocations. Contains model pricing table and context window sizes. |
| `mod.rs` | Module re-exports. |

## fixtures/

Test fixture JSONL files used by snapshot tests in `claude_code.rs`. These are real (anonymized) transcript fragments covering edge cases: streaming partials, cache breakdowns, thinking blocks, multi-model sessions.

#!/usr/bin/env bash
# cc-hud telemetry functions
# Source this file: source cc-hud-functions.sh

# Total cost by model across all top-level session JSONL files
cc-cost() {
  find ~/.claude/projects -maxdepth 2 -name '*.jsonl' -exec cat {} + \
    | jq -c 'select(.message.usage and .message.model and .message.stop_reason) | {m: .message.model, i: (.message.usage.input_tokens // 0), o: (.message.usage.output_tokens // 0), cr: (.message.usage.cache_read_input_tokens // 0), cc: (.message.usage.cache_creation_input_tokens // 0)}' 2>/dev/null \
    | jq -s 'group_by(.m) | map({model: .[0].m, input: (map(.i) | add), output: (map(.o) | add), cache_read: (map(.cr) | add), cache_create: (map(.cc) | add)}) | map(. as $g | (if (.model | test("opus-4-[56]")) then {i:5,o:25,cr:0.5,cc:6.25} elif (.model | test("opus")) then {i:15,o:75,cr:1.5,cc:18.75} elif (.model | test("sonnet")) then {i:3,o:15,cr:0.3,cc:3.75} elif (.model | test("haiku-4-5")) then {i:1,o:5,cr:0.1,cc:1.25} elif (.model | test("haiku")) then {i:0.8,o:4,cr:0.08,cc:1.0} else {i:3,o:15,cr:0.3,cc:3.75} end) as $p | ($g.input/1e6*$p.i + $g.output/1e6*$p.o + $g.cache_read/1e6*$p.cr + $g.cache_create/1e6*$p.cc) as $cost | {model: $g.model, input: $g.input, output: $g.output, cache_read: $g.cache_read, cache_create: $g.cache_create, cost_usd: ($cost * 100 | round / 100)}) | . + [{model: "TOTAL", cost_usd: (map(.cost_usd) | add)}]'
}

# Same but includes subagent JSONL files (all depths)
cc-cost-all() {
  find ~/.claude/projects -name '*.jsonl' -exec cat {} + \
    | jq -c 'select(.message.usage and .message.model and .message.stop_reason) | {m: .message.model, i: (.message.usage.input_tokens // 0), o: (.message.usage.output_tokens // 0), cr: (.message.usage.cache_read_input_tokens // 0), cc: (.message.usage.cache_creation_input_tokens // 0)}' 2>/dev/null \
    | jq -s 'group_by(.m) | map({model: .[0].m, input: (map(.i) | add), output: (map(.o) | add), cache_read: (map(.cr) | add), cache_create: (map(.cc) | add)}) | map(. as $g | (if (.model | test("opus-4-[56]")) then {i:5,o:25,cr:0.5,cc:6.25} elif (.model | test("opus")) then {i:15,o:75,cr:1.5,cc:18.75} elif (.model | test("sonnet")) then {i:3,o:15,cr:0.3,cc:3.75} elif (.model | test("haiku-4-5")) then {i:1,o:5,cr:0.1,cc:1.25} elif (.model | test("haiku")) then {i:0.8,o:4,cr:0.08,cc:1.0} else {i:3,o:15,cr:0.3,cc:3.75} end) as $p | ($g.input/1e6*$p.i + $g.output/1e6*$p.o + $g.cache_read/1e6*$p.cr + $g.cache_create/1e6*$p.cc) as $cost | {model: $g.model, input: $g.input, output: $g.output, cache_read: $g.cache_read, cache_create: $g.cache_create, cost_usd: ($cost * 100 | round / 100)}) | . + [{model: "TOTAL", cost_usd: (map(.cost_usd) | add)}]'
}

# Per-session cost breakdown, sorted descending
cc-cost-per-session() {
  find ~/.claude/projects -maxdepth 2 -name '*.jsonl' -size +1k | while read f; do
    cost=$(jq -c 'select(.message.usage and .message.model and .message.stop_reason) | {m: .message.model, i: (.message.usage.input_tokens // 0), o: (.message.usage.output_tokens // 0), cr: (.message.usage.cache_read_input_tokens // 0), cc: (.message.usage.cache_creation_input_tokens // 0)}' "$f" 2>/dev/null \
      | jq -s 'map((if (.m | test("opus-4-[56]")) then {i:5,o:25,cr:0.5,cc:6.25} elif (.m | test("sonnet")) then {i:3,o:15,cr:0.3,cc:3.75} elif (.m | test("haiku")) then {i:0.8,o:4,cr:0.08,cc:1.0} else {i:3,o:15,cr:0.3,cc:3.75} end) as $p | .i/1e6*$p.i + .o/1e6*$p.o + .cr/1e6*$p.cr + .cc/1e6*$p.cc) | add // 0')
    printf '%10.2f  %s\n' "$cost" "$f"
  done | sort -rn | head -"${1:-20}"
}

# File counts
cc-files() {
  echo "Top-level session JSONL:" && find ~/.claude/projects -maxdepth 2 -name '*.jsonl' | wc -l
  echo "Subagent JSONL:" && find ~/.claude/projects -path '*/subagents/*' -name '*.jsonl' | wc -l
  echo "Total JSONL:" && find ~/.claude/projects -name '*.jsonl' | wc -l
}

# Additive-only backup (never overwrites or deletes)
cc-backup() {
  local dest="${1:-$HOME/.cc-hud-backup/projects}"
  mkdir -p "$dest"
  rsync -a --ignore-existing ~/.claude/projects/ "$dest"/
  echo "Backed up to $dest"
  echo "Files in backup:" && find "$dest" -name '*.jsonl' | wc -l
}

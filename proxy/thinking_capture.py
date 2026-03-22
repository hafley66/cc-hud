"""
mitmproxy addon: taps Claude API streams to capture thinking token data.

Each SSE chunk passes through unmodified to Claude Code while being
copied into a per-flow buffer. On stream completion, the buffer is
parsed for thinking deltas and a record is written.

Usage:
    mitmdump -s proxy/thinking_capture.py --listen-port 8080 --mode regular

Then launch Claude Code:
    HTTPS_PROXY=http://localhost:8080 \
    NODE_EXTRA_CA_CERTS=~/.mitmproxy/mitmproxy-ca-cert.pem \
    claude

Output: ~/.claude/thinking_capture.jsonl
"""

import json
import time
from pathlib import Path
from mitmproxy import http, ctx


THINKING_LOG = Path.home() / ".claude" / "thinking_capture.jsonl"

# Per-flow state
_buffers: dict[str, bytearray] = {}
_models: dict[str, str] = {}


def request(flow: http.HTTPFlow):
    if flow.request.pretty_host == "api.anthropic.com" and "/v1/messages" in flow.request.path:
        flow.metadata["claude_api"] = True
        try:
            body = json.loads(flow.request.content)
            _models[flow.id] = body.get("model", "unknown")
        except (json.JSONDecodeError, TypeError):
            _models[flow.id] = "unknown"


def responseheaders(flow: http.HTTPFlow):
    """Enable streaming passthrough with a tapping function."""
    if not flow.metadata.get("claude_api"):
        return

    content_type = flow.response.headers.get("content-type", "")
    if "text/event-stream" not in content_type:
        return

    buf = bytearray()
    _buffers[flow.id] = buf

    # response.stream callable: receives chunk bytes, returns bytes to forward.
    # We copy into our buffer and return the chunk unmodified.
    def tap(chunk: bytes) -> bytes:
        buf.extend(chunk)
        return chunk

    flow.response.stream = tap


def response(flow: http.HTTPFlow):
    if not flow.metadata.get("claude_api"):
        return

    buf = _buffers.pop(flow.id, None)
    model = _models.pop(flow.id, "unknown")

    if buf is not None:
        _parse_sse(buf.decode("utf-8", errors="replace"), model)
    elif flow.response and flow.response.content:
        content_type = flow.response.headers.get("content-type", "")
        if "application/json" in content_type:
            _parse_json(flow.response.content, model)


def _parse_sse(text: str, model: str):
    thinking_text_len = 0
    output_tokens = 0
    input_tokens = 0
    cache_read = 0
    cache_create = 0

    for line in text.split("\n"):
        line = line.strip()
        if not line.startswith("data: "):
            continue
        data_str = line[6:]
        if data_str == "[DONE]":
            continue
        try:
            event = json.loads(data_str)
        except json.JSONDecodeError:
            continue

        etype = event.get("type", "")

        if etype == "content_block_start":
            block = event.get("content_block", {})
            if block.get("type") == "thinking":
                thinking_text_len += len(block.get("thinking", ""))

        elif etype == "content_block_delta":
            delta = event.get("delta", {})
            if delta.get("type") == "thinking_delta":
                thinking_text_len += len(delta.get("thinking", ""))

        elif etype == "message_delta":
            usage = event.get("usage", {})
            output_tokens = usage.get("output_tokens", output_tokens)

        elif etype == "message_start":
            msg = event.get("message", {})
            model = msg.get("model", model)
            usage = msg.get("usage", {})
            input_tokens = usage.get("input_tokens", 0)
            cache_read = usage.get("cache_read_input_tokens", 0)
            cache_create = usage.get("cache_creation_input_tokens", 0)

    est = thinking_text_len // 4 if thinking_text_len > 0 else 0
    record = {
        "timestamp": time.time(),
        "model": model,
        "thinking_text_len": thinking_text_len,
        "estimated_thinking_tokens": est,
        "output_tokens": output_tokens,
        "input_tokens": input_tokens,
        "cache_read_input_tokens": cache_read,
        "cache_creation_input_tokens": cache_create,
    }
    _write_record(record)
    if thinking_text_len > 0:
        ctx.log.info(f"[thinking] ~{est}tok ({thinking_text_len} chars), out={output_tokens}, model={model}")
    else:
        ctx.log.info(f"[no-thinking] out={output_tokens}, model={model}")


def _parse_json(content: bytes, model: str):
    try:
        body = json.loads(content)
    except (json.JSONDecodeError, TypeError):
        return

    thinking_text_len = 0
    for block in body.get("content", []):
        if block.get("type") == "thinking":
            thinking_text_len += len(block.get("thinking", ""))

    usage = body.get("usage", {})
    est = thinking_text_len // 4 if thinking_text_len > 0 else 0
    _write_record({
        "timestamp": time.time(),
        "model": body.get("model", model),
        "thinking_text_len": thinking_text_len,
        "estimated_thinking_tokens": est,
        "output_tokens": usage.get("output_tokens", 0),
        "input_tokens": usage.get("input_tokens", 0),
        "cache_read_input_tokens": usage.get("cache_read_input_tokens", 0),
        "cache_creation_input_tokens": usage.get("cache_creation_input_tokens", 0),
    })


def _write_record(record: dict):
    THINKING_LOG.parent.mkdir(parents=True, exist_ok=True)
    with open(THINKING_LOG, "a") as f:
        f.write(json.dumps(record) + "\n")

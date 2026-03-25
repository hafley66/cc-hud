# proxy/

| File | Purpose |
|------|---------|
| `thinking_capture.py` | mitmproxy addon that intercepts Anthropic API streaming responses. Captures thinking/text block deltas per flow, writes enriched events to `~/.claude/thinking_capture.jsonl` on stream completion. Passes all traffic through unmodified. |

## Setup

```bash
pip install mitmproxy

# Start the proxy
mitmdump -s proxy/thinking_capture.py --listen-port 8080 --mode regular

# In another terminal, launch Claude Code through the proxy
HTTPS_PROXY=http://localhost:8080 \
NODE_EXTRA_CA_CERTS=~/.mitmproxy/mitmproxy-ca-cert.pem \
claude
```

First run generates mitmproxy's CA cert at `~/.mitmproxy/mitmproxy-ca-cert.pem`. The `NODE_EXTRA_CA_CERTS` env var tells Node.js to trust it for HTTPS interception.

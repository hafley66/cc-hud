# bin/

Shell helpers for running cc-hud.

| File | Purpose |
|------|---------|
| `launch` | Tmux launcher. Creates a session with the overlay tracking your main pane. Builds release binary if needed. Works both inside and outside tmux. |
| `cc-hud-status.sh` | Status line feed script. Receives JSON from Claude Code's status line hook and appends to `/tmp/cc-hud-feed.jsonl` for the HUD to poll. |

## Usage

```bash
bin/launch              # new tmux session with overlay
bin/launch my-session   # named session
```

If already inside tmux, `bin/launch` starts the overlay for your current pane and blocks until you Ctrl-C.

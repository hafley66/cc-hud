#!/bin/bash
# Claude Code status line script.
# Install: add to ~/.claude/settings.json:
#   { "statusLine": { "type": "command", "command": "~/projects/cc-hud/bin/cc-hud-status.sh" } }
#
# Receives JSON on stdin after every assistant message.
# Appends each line to the feed file for the HUD to poll.

cat >> /tmp/cc-hud-feed.jsonl

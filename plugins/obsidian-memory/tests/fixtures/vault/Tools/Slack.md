---
type: tool
description: Acme Slack workspace — bot user token at $SLACK_BOT_TOKEN, curl examples for chat.postMessage
created: 2026-04-20
---

# Slack

Workspace: acme.slack.com. Bot token in `~/.config/claude-memory/secrets.env` as `$SLACK_BOT_TOKEN`.

```bash
curl -H "Authorization: Bearer $SLACK_BOT_TOKEN" \
  -d '{"channel":"C123","text":"hi"}' \
  https://slack.com/api/chat.postMessage
```

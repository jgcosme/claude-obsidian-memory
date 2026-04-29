---
type: tool
description: gws CLI for Gmail/Drive/Calendar — authenticated as test@example.com via gcloud
created: 2026-04-20
---

# Google Workspace

`gws` is a CLI wrapper for Gmail, Drive, and Calendar. Auth via `gcloud auth login`.

```bash
gws gmail list --max-results 10
gws drive list --query "modifiedTime > '2026-01-01'"
gws calendar today
```

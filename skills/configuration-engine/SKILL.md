---
name: configuration-engine
description: Design persistent, validated, atomic configuration shared by PowerShell and WebUI.
---

# Configuration Engine

Use one canonical schema. Validate before saving, write through a temporary file and rename, retain a backup, redact secrets, and make migrations explicit. Expose show, validate, export, import, diff, sync, backup, and restore operations. Avoid silent conflict resolution and preserve unknown fields where safe.

# Project: SYNCSTEWARD

## On Entry (MANDATORY)

Run immediately when entering this project:
```bash
session-context
```

---

## Project Documentation Database

SyncSteward keeps its source-of-truth documentation databases in `.docs/`. Query them before implementing features:

- `.docs/syncsteward_architecture.db`
- `.docs/syncsteward_tool_spec.db`
- `.docs/syncsteward_dial_plan.db`
- `.docs/workspace.db`

Example queries:

```sql
-- Read all sections in order
SELECT section_id, title, content FROM sections ORDER BY sort_order;

-- Search for specific topic
SELECT * FROM sections_fts WHERE sections_fts MATCH 'your topic';

-- Check terminology
SELECT canonical, definition FROM terminology;
```

**Check for databases:** `ls -la .docs/*.db 2>/dev/null`

**Updating documentation:**
```bash
python3 ~/.global/tools/doc-orchestrator/doc-orchestrator.py status --db ./.docs/<database>.db
python3 ~/.global/tools/doc-orchestrator/doc-orchestrator.py edit <section_id> "new content" --db ./.docs/<database>.db
```

---

## Project Overview

SyncSteward is a safety-first sync hardening and monitoring app for the existing macOS `rclone` plus remote Linux `onedrive` stack. Build meaningful product-facing features in both CLI and MCP first. UI comes after the control surfaces are stable.

---

## Memory Commands

**Log decisions/notes:**
```bash
memory-log decision "topic" "what was decided and why"
memory-log note "topic" "content"
memory-log blocker "topic" "what is blocking"
```

**Manage tasks:**
```bash
task add "description" [priority]
task list
task done <id>
```

---

## External-Facing Writing

- Keep README files, architecture docs, changelogs, commit messages, PR text, and code comments in normal developer voice.
- Do not describe implementation work in terms of agent runs, autonomous loops, model names, or internal AI workflow mechanics.
- Mention AI, LLMs, assistants, or orchestration only when they are part of the actual product surface being documented.

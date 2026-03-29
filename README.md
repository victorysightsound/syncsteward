# SyncSteward

SyncSteward is a safety-first sync control plane for the existing macOS `rclone` and remote `onedrive` stack.

The first wave focuses on two things:

- make the current sync state observable before anything is re-enabled
- give both CLI and MCP surfaces the same guarded health and preflight view

## Current Scope

SyncSteward does not restart or mutate sync automatically yet. The initial build exposes:

- local launch agent status
- remote OneDrive service status
- conflict and `safeBackup` artifact detection
- latest `rclone` log summary
- preflight checks that answer whether the system is safe to re-enable

## Interfaces

- CLI: operator control, scripting, diagnostics
- MCP: AI-native sync inspection and future orchestration

UI comes later, after the CLI and MCP surfaces are stable.

## Commands

```bash
cargo run -p syncsteward-cli -- status
cargo run -p syncsteward-cli -- preflight
cargo run -p syncsteward-cli -- status --json
cargo run -p syncsteward-cli -- mcp stdio
```

## Default Environment Assumptions

The built-in defaults match the current environment:

- macOS launch agent: `~/Library/LaunchAgents/com.cloud-sync.plist`
- sync script: `~/bin/cloud-sync.sh`
- `rclone` logs: `~/.config/rclone/logs`
- remote hosts:
  - `192.168.77.135`
  - `192.168.195.155`
- remote service: `onedrive@john.service`

This will become configurable as SyncSteward grows.

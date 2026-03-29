# SyncSteward

SyncSteward is a safety-first sync control plane for the existing macOS `rclone` and remote `onedrive` stack.

The current wave focuses on three things:

- make the current sync state observable before anything is re-enabled
- give both CLI and MCP surfaces the same guarded health and preflight view
- add explicit pause and resume controls that stay fail-closed
- turn the current target inventory into an explicit managed config before re-enablement

## Current Scope

SyncSteward does not restart sync automatically. The current build exposes:

- local launch agent status
- remote OneDrive service status
- conflict and `safeBackup` artifact detection
- latest `rclone` log summary
- preflight checks that answer whether the system is safe to re-enable
- explicit `pause` and guarded `resume`
- backup-only defaults for live SQLite database files and sidecars
- target inventory from the current `cloud-sync.sh` with safer recommended policies
- explicit acknowledgement of a historical incident log after cleanup
- config scaffolding so recommended folder policies become a real SyncSteward config file
- target-scoped readiness and blocker reports before any selective re-enablement
- single-target execution for approved `backup_only` targets, with dry-run support, legacy lock protection, and per-target audit/state records

## Interfaces

- CLI: operator control, scripting, diagnostics
- MCP: AI-native sync inspection and future orchestration

UI comes later, after the CLI and MCP surfaces are stable.

## Commands

```bash
cargo run -p syncsteward-cli -- status
cargo run -p syncsteward-cli -- preflight
cargo run -p syncsteward-cli -- targets
cargo run -p syncsteward-cli -- check-targets
cargo run -p syncsteward-cli -- check-target Pictures
cargo run -p syncsteward-cli -- run-target .memloft --dry-run
cargo run -p syncsteward-cli -- acknowledge-latest-log
cargo run -p syncsteward-cli -- scaffold-config
cargo run -p syncsteward-cli -- pause --target all
cargo run -p syncsteward-cli -- resume --target all
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

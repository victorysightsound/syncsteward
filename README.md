# SyncSteward

SyncSteward is a safety-first sync control plane for the existing macOS `rclone` and remote `onedrive` stack.

The current wave focuses on three things:

- make the current sync state observable before anything is re-enabled
- give both CLI and MCP surfaces the same guarded health and preflight view
- add explicit pause and resume controls that stay fail-closed
- turn the current target inventory into an explicit managed config before re-enablement

## Current Scope

SyncSteward does not restart sync automatically. The current build exposes:

- one composed overview across preflight, runner state, approved targets, recent run history, and alerts
- local launch agent status
- dedicated SyncSteward runner launch agent status
- remote OneDrive service status
- conflict and `safeBackup` artifact detection
- latest `rclone` log summary
- preflight checks that answer whether the system is safe to re-enable
- explicit `pause` and guarded `resume`
- dedicated runner-agent install/status/uninstall commands for safe launchd scheduling
- backup-only defaults for live SQLite database files and sidecars
- target-specific exclusions for protected bundles inside executable targets
- snapshot-backed handling for runtime SQLite targets like `.memloft`
- target inventory from the current `cloud-sync.sh` with safer recommended policies
- explicitly managed subtargets that can be backed up safely while their broad parent folder stays on hold
- durable managed-target IDs as the first foundation for future relocate/adopt workflows
- managed-target lifecycle commands for adding curated paths and relocating existing targets without hand-editing config
- explicit acknowledgement of a historical incident log after cleanup
- config scaffolding so recommended folder policies become a real SyncSteward config file
- target-scoped readiness and blocker reports before any selective re-enablement
- single-target execution for approved `backup_only` targets, with dry-run support, legacy lock protection, and per-target audit/state records
- approved-target cycle execution from config, so future daemon and UI layers can drive one guarded orchestration entry point
- daemon-ready `runner-tick` scheduling that only executes the approved cycle when it is due
- alert evaluation for stale or missing target runs, plus deduplicated local notification support
- a native macOS menu bar shell that reads the composed `overview` contract without adding separate sync logic

## Interfaces

- CLI: operator control, scripting, diagnostics
- MCP: AI-native sync inspection and future orchestration

UI comes later, after the CLI and MCP surfaces are stable.

The `overview` surface is the first stable dashboard-style contract for that future UI.

## macOS Shell

SyncSteward now includes a first native macOS shell under:

- [apps/syncsteward-macos](/Users/johndeaton/projects/syncsteward/apps/syncsteward-macos)

The current shell is intentionally small:

- SwiftUI menu bar app
- opens a visible control window on direct app launch
- reads `syncsteward-cli overview --json`
- reads `syncsteward-cli runner-agent-status --json` for launchd visibility
- shows preflight, runner, runner-agent, approved-target, recent-run, and alert state
- exposes a guarded `runner-tick --dry-run` operator action
- opens the live config, state folder, runner logs, and audit log
- does not introduce any new sync logic

It resolves the CLI in this order:

- `SYNCSTEWARD_CLI_PATH`
- `~/projects/syncsteward/target/debug/syncsteward-cli`
- `~/bin/syncsteward-cli`
- `syncsteward-cli` from `PATH`

Build it with:

```bash
swift build --package-path apps/syncsteward-macos
```

Install or refresh the local app bundle with:

```bash
apps/syncsteward-macos/scripts/install-app.sh
```

That installs:

- `~/Applications/SyncSteward.app`

The installer keeps the app bundle thin. It launches the current dev-built SwiftUI shell and points it at the current dev-built `syncsteward-cli`.

## Brand Assets

SyncSteward now includes a reproducible brand asset pack under:

- [branding](/Users/johndeaton/projects/syncsteward/branding)

That pack includes:

- square icon exports for GitHub and general distribution use
- macOS `AppIcon.iconset` sources plus `SyncSteward.icns`
- GitHub/social preview images in common wide and square formats

## Protected Bundles

SyncSteward now applies first-class target exclusions for native Apple media libraries:

- `Pictures` excludes `Photos Library.photoslibrary`
- `Music` excludes `Music Library.musiclibrary`

That keeps backup-only media targets focused on ordinary folders and files even if the legacy rclone filter file changes later.

## Runtime Snapshots

SyncSteward now treats `.memloft` as a snapshot-backed runtime target:

- ordinary non-database files still flow through the filtered backup-only sync path
- `memloft.db`, `payroll.db`, and `vault.db` are uploaded from `sqlite3 .backup` snapshots created in temp space

That preserves live SQLite consistency without requiring the whole runtime tree to be staged locally before every backup.

## Managed Subtargets

SyncSteward can now define explicit managed targets outside the broad legacy folder list.

That lets it keep a risky top-level folder on `hold` while still executing curated subfolders safely. The first example is:

- `Notes` stays on `hold`
- `Notes/Personal` can be defined explicitly as a managed `backup_only` target

Managed targets participate in:

- target inventory
- readiness and blocker evaluation
- dry-run and live `run-target` execution
- alerting, audit, and state history

Managed targets can also carry stable IDs now. That is the first foundation for a future relocate/adopt workflow, where SyncSteward can recognize the same managed target after its root path moves instead of treating it as a brand-new target with unrelated deletes.

SyncSteward can now mutate that managed-target config directly:

- `add-managed-target` registers a new curated path and assigns its durable ID immediately
- `relocate-managed-target` updates a managed target by ID, name, or current path while preserving the same durable ID and run history

## Approved Runner

SyncSteward now has a config-backed cycle command for the approved healthy subset.

- `runner.approved_targets` defines the exact targets the guarded cycle is allowed to execute
- `runner.cycle_interval_minutes` defines the minimum cadence for scheduled execution
- `runner.launch_agent.tick_interval_minutes` defines how often launchd should wake the daemon-ready runner entry point
- `run-cycle` reuses the same single-target guarded execution path instead of inventing a second sync engine
- `run-cycle` now holds the legacy sync lock for the full cycle, so overlapping cycles and manual target runs cannot interleave
- dry-run validation still writes audit history, but it does not overwrite the live target-run state that drives alerts
- `runner-tick` is the daemon-ready entry point: it checks whether the approved cycle is due, runs it only when needed, and otherwise no-ops with the current alert snapshot
- scheduled notifications now suppress unchanged alert sets inside a repeat window and can send one recovery notification when alerts clear
- `install-runner-agent` writes and loads `com.syncsteward.runner`, which schedules `runner-tick` independently of the paused legacy `com.cloud-sync` job
- this is the first daemon-ready entry point for future scheduling, menu bar UI actions, and MCP orchestration
- broad legacy folders can stay on `hold` while the approved subset keeps running safely

## Commands

```bash
cargo run -p syncsteward-cli -- overview
cargo run -p syncsteward-cli -- status
cargo run -p syncsteward-cli -- preflight
cargo run -p syncsteward-cli -- targets
cargo run -p syncsteward-cli -- check-targets
cargo run -p syncsteward-cli -- check-target Pictures
cargo run -p syncsteward-cli -- run-target Pictures --dry-run
cargo run -p syncsteward-cli -- run-target .memloft --dry-run
cargo run -p syncsteward-cli -- alerts
cargo run -p syncsteward-cli -- notify-alerts --dry-run
cargo run -p syncsteward-cli -- run-cycle --dry-run
cargo run -p syncsteward-cli -- runner-tick --dry-run
cargo run -p syncsteward-cli -- runner-agent-status
cargo run -p syncsteward-cli -- install-runner-agent
cargo run -p syncsteward-cli -- uninstall-runner-agent --keep-plist
cargo run -p syncsteward-cli -- acknowledge-latest-log
cargo run -p syncsteward-cli -- scaffold-config
cargo run -p syncsteward-cli -- ensure-target-ids
cargo run -p syncsteward-cli -- add-managed-target --name Notes/Archive --local-path ~/Notes/Archive --remote-path OneDrive/Notes/Archive
cargo run -p syncsteward-cli -- relocate-managed-target 019d3c2e-4881-7d53-9e1e-37e74729e874 --local-path ~/Notes/Personal
cargo run -p syncsteward-cli -- pause --target all
cargo run -p syncsteward-cli -- resume --target all
cargo run -p syncsteward-cli -- overview --json
cargo run -p syncsteward-cli -- status --json
cargo run -p syncsteward-cli -- mcp stdio
swift build --package-path apps/syncsteward-macos
```

## Default Environment Assumptions

The built-in defaults match the current environment:

- macOS launch agent: `~/Library/LaunchAgents/com.cloud-sync.plist`
- SyncSteward runner launch agent: `~/Library/LaunchAgents/com.syncsteward.runner.plist`
- sync script: `~/bin/cloud-sync.sh`
- `rclone` logs: `~/.config/rclone/logs`
- remote hosts:
  - `192.168.77.135`
  - `192.168.195.155`
- remote service: `onedrive@john.service`

This will become configurable as SyncSteward grows.

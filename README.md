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
- a configurable `remote.rclone_ssh_mode` transport knob, with macOS defaulting to external SSH so launchd-managed `rclone` runs avoid background NECP drops
- backup_only defaults for live SQLite database files and sidecars
- target-specific exclusions for protected bundles inside executable targets
- snapshot-backed handling for runtime SQLite targets like `.memloft`
- bounded retries around `rclone` sync and copy operations so one transient remote hiccup does not poison the whole approved cycle
- target inventory from the legacy sync script when available, otherwise explicit managed targets with safer recommended policies
- explicitly managed subtargets that can be backed up safely while their broad parent folder stays on hold
- durable managed-target IDs as the first foundation for future relocate/adopt workflows
- managed-target lifecycle commands for adding curated paths and relocating existing targets without hand-editing config
- coordinated remote OneDrive pause/resume around sync runs, plus instruction-file symlink normalization in the sync root
- user-scoped Victorystore OneDrive control, with the legacy system unit disabled so only one remote service owns the tree across reboot
- explicit acknowledgement of a historical incident log after cleanup
- config scaffolding so recommended folder policies become a real SyncSteward config file
- target-scoped readiness and blocker reports before any selective re-enablement
- single-target execution for approved `backup_only` targets, with dry-run support, legacy lock protection, and per-target audit/state records
- approved-target cycle execution from config, so future daemon and UI layers can drive one guarded orchestration entry point
- daemon-ready `runner-tick` scheduling that only executes the approved cycle when it is due
- in-progress cycle visibility in runner state, including the current approved target when a live cycle is still running
- alert evaluation for stale or missing target runs, plus deduplicated local notification support
- managed-run preflight that stays ready when remote OneDrive is active but SyncSteward can pause it safely
- strict legacy `resume` gating that still refuses to re-enable `com.cloud-sync` while remote OneDrive is active
- runner-scoped alerting so scheduled health follows `runner.approved_targets` instead of every nested managed subtarget
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

That keeps `backup_only` media targets focused on ordinary folders and files even if the legacy rclone filter file changes later.

## Runtime Snapshots

SyncSteward now treats `.memloft` as a snapshot-backed runtime target:

- ordinary non-database files still flow through the filtered `backup_only` sync path
- `memloft.db`, `payroll.db`, and `vault.db` are uploaded from `sqlite3 .backup` snapshots created in temp space
- snapshot rules now protect only the named live database files; other database files in the same target still back up normally, while SQLite sidecars remain excluded globally

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
- explicit `verify-target`, `repair-target`, and `rebaseline-target` flows
- alerting, audit, and state history

Managed targets can also carry stable IDs now. That is the first foundation for a future relocate/adopt workflow, where SyncSteward can recognize the same managed target after its root path moves instead of treating it as a brand-new target with unrelated deletes.

SyncSteward can now mutate that managed-target config directly:

- `add-managed-target` registers a new curated path and assigns its durable ID immediately
- `relocate-managed-target` updates a managed target by ID, name, or current path while preserving the same durable ID and run history

The core config is also now first-class:

- `config` reads the normalized operator config snapshot
- `config-schema` exposes the JSON schema for the config model
- `config-set` applies structured config patches, including dry-run validation
- `prune-state` removes stale target-run state entries that no longer match the current explicit inventory
- `quarantine-artifacts` moves `.conflict*` and `victorystore-safeBackup*` files into a timestamped quarantine root with a manifest, so preflight blockers can be cleared safely without deleting evidence
- config writes are atomic and leave timestamped backups when overwriting existing files
- state writes are normalized and written atomically so `~/.local/state/syncsteward` does not resolve relative to the repo
- target state now tracks verified success timestamps, failure class, and last repair or rebaseline time
- status and overview now expose any active manual target run, verification, repair, or rebaseline while it is in flight
- remote status now reports the configured OneDrive service name, service scope, and whether SyncSteward coordination is enabled

## Approved Runner

SyncSteward now has a config-backed cycle command for the approved healthy subset.

- `runner.approved_targets` defines the exact targets the guarded cycle is allowed to execute
- `runner.cycle_interval_minutes` defines the minimum cadence for scheduled execution
- `runner.launch_agent.tick_interval_minutes` defines how often launchd should wake the daemon-ready runner entry point
- `run-cycle` reuses the same single-target guarded execution path instead of inventing a second sync engine
- `run-cycle` now holds the legacy sync lock for the full cycle, so overlapping cycles and manual target runs cannot interleave
- dry-run validation still writes audit history, but it does not overwrite the live target-run state that drives alerts
- `runner-tick` is the daemon-ready entry point: it checks whether the approved cycle is due, runs it only when needed, and otherwise no-ops with the current alert snapshot
- `runner-tick` now recognizes an already-running approved cycle and reports `no_op` instead of recording a second blocked cycle attempt
- `runner-tick` now clears stale approved-cycle state left behind by an interrupted runner process before deciding whether another cycle is already running
- `runner-tick` now exits `0` for success, no-op, and policy-blocked scheduler outcomes so `launchd` only records a failed runner when the command itself actually fails
- scheduled notifications now suppress unchanged alert sets inside a repeat window and can send one recovery notification when alerts clear
- `install-runner-agent` writes and loads `com.syncsteward.runner`, which schedules `runner-tick` independently of the paused legacy `com.cloud-sync` job
- `install-runner-agent` now writes an explicit runner `PATH`, and execution resolves external tools like `rclone` from common system and Homebrew locations so launchd cannot silently fail on a stripped environment
- macOS launchd-managed `rclone` SFTP work now defaults to the external `/usr/bin/ssh` transport path through `remote.rclone_ssh_mode = "auto"`, matching the SSH path that already works in background sessions
- `run-target` and `run-cycle` now retry `rclone` sync and copy steps a small number of times before marking the target failed
- retry behavior is now selective: transport-style failures retry, while deterministic auth, path, and divergence failures stop immediately so recovery can move to the right branch without replaying the same bad check
- successful live `run-target` runs now verify remote contents before they are treated as healthy
- scheduled live verification now defaults to bounded `size_and_sample` mode, combining `rclone check --size-only` with deterministic sampled hash checks; explicit `verify-target` remains the full verification path
- active manual target-operation state now records the owning process and self-heals when an interrupted run leaves stale state behind
- target-scoped run and verify preflight now scan only the relevant target roots for conflict artifacts, while `status`, `overview`, and `alerts` remain the global health surfaces
- global status, overview, and alert scans now walk the full target inventory, including legacy-discovered targets, so artifact blockers cannot hide outside explicit managed-target roots
- `verify-target` is the read-only remote verification entry point
- `repair-target` is the non-destructive recovery entry point: it reruns guarded incremental sync, detects structured divergence when verification fails, rewrites only the mismatched files, deletes remote-only extras, and then verifies the target again
- `rebaseline-target` is the destructive recovery entry point: it rebuilds the remote target from the current local tree, requires `--yes` for live execution, and then verifies the rebuilt target
- approved-cycle health and stale-success alerting now center on `runner.approved_targets`, while non-approved managed targets stay quiet unless they have an explicit failing manual run history
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
cargo run -p syncsteward-cli -- verify-target Pictures
cargo run -p syncsteward-cli -- repair-target Pictures --dry-run
cargo run -p syncsteward-cli -- rebaseline-target Pictures --dry-run
cargo run -p syncsteward-cli -- rebaseline-target Pictures --yes
cargo run -p syncsteward-cli -- quarantine-artifacts Music Pictures --dry-run
cargo run -p syncsteward-cli -- alerts
cargo run -p syncsteward-cli -- notify-alerts --dry-run
cargo run -p syncsteward-cli -- prune-state --dry-run
cargo run -p syncsteward-cli -- run-cycle --dry-run
cargo run -p syncsteward-cli -- runner-tick --dry-run
cargo run -p syncsteward-cli -- runner-agent-status
cargo run -p syncsteward-cli -- install-runner-agent
cargo run -p syncsteward-cli -- uninstall-runner-agent --keep-plist
cargo run -p syncsteward-cli -- acknowledge-latest-log
cargo run -p syncsteward-cli -- config
cargo run -p syncsteward-cli -- config-schema
cargo run -p syncsteward-cli -- config-set --patch-file ~/syncsteward-config-patch.toml --dry-run
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
- legacy sync script: `~/bin/cloud-sync.sh`
- `rclone` logs: `~/.config/rclone/logs`
- remote hosts:
  - `192.168.77.135`
  - `192.168.195.155`
- remote service: `onedrive@john.service`

This will become configurable as SyncSteward grows.

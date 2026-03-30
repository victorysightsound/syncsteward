# SyncSteward Plan

## Phase 1: Health and Freeze Control

- convert the project into a real workspace
- expose shared status and preflight logic in a core crate
- ship matching CLI and MCP inspection surfaces
- keep sync automation paused until the system is measurably clean

## Phase 2: Coordinated Pause and Resume

- add explicit local launch agent pause and resume actions
- add explicit remote OneDrive pause and resume actions
- require preflight success before any resume path is allowed
- record every mutation action in structured logs
- make pause idempotent and fail closed
- surface the same controls in both CLI and MCP

## Phase 3: Folder Policies and Safe Execution

- inventory legacy `cloud-sync.sh` targets and attach recommended SyncSteward policies
- acknowledge a historical incident log as the current safe baseline after cleanup
- scaffold a real SyncSteward config from the current target recommendations
- classify folders as two-way, backup-only, excluded, or hold
- add explicit managed subtargets so curated paths can run safely while broad parent folders remain on hold
- assign durable IDs to managed targets as groundwork for relocate/adopt workflows
- add managed-target lifecycle commands so curated paths can be registered and relocated without hand-editing config
- explain effective mode and blockers per target before any selective re-enablement
- protect live SQLite database files and sidecars with backup-only defaults unless explicitly overridden
- protect native Apple media library bundles with target-specific exclusions inside executable backup-only targets
- add snapshot-backed execution for runtime SQLite targets like `.memloft`
- run preflight and folder gating before each sync
- add single-target execution for approved backup-only targets with dry-run support
- make dry-run validation observable without overwriting the live target-run state used by alerts
- record per-target last outcome in state and audit
- define the approved target set in config and add one guarded cycle command for daemon/UI orchestration
- hold the legacy sync lock for the full approved cycle so overlapping runs cannot interleave
- allow folder-scoped rebaseline instead of broad `--resync`
- add quarantine handling for conflict and `safeBackup` artifacts
- implement explicit relocate flows for managed targets whose root paths move
- defer automatic adopt/detect-move behavior until after the explicit relocate path is proven

## Phase 4: Monitoring and Alerts

- add notifications for blocked sync, repeated failures, stale last-success time, and new drift artifacts
- store sync history, health transitions, and acknowledgements
- expose alert state in both CLI and MCP
- ship a first local notification path for active alerts
- make the approved-target cycle the future daemon/menu bar execution entry point
- add a scheduled runner-tick command that only executes the approved cycle when due
- add a dedicated SyncSteward runner launch agent that schedules `runner-tick` without reusing the legacy broad sync job
- refine alert deduplication and escalation after the first notification slice lands
- suppress repeated scheduled notifications for unchanged alert sets until a configurable repeat window expires
- send one recovery notification when a previously active alert set clears
- add one composed overview surface for CLI, MCP, and future UI consumers so operator dashboards do not need to stitch together multiple health commands

## Phase 5: UI

- add a menu bar app
- start with a native macOS shell that reads the composed `overview` surface, shows runner-agent state, and exposes only safe refresh, dry-run, and open-log/config actions
- provide a thin installer path that places `SyncSteward.app` in `~/Applications` without splitting app-bundle behavior away from the dev-built shell and CLI
- show green/yellow/red health state
- expose sync now, pause, resume, open logs, and reveal conflicts
- keep all real logic in the shared core plus CLI and MCP layers

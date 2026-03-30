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
- record per-target last outcome in state and audit
- allow folder-scoped rebaseline instead of broad `--resync`
- add quarantine handling for conflict and `safeBackup` artifacts
- implement explicit relocate flows for managed targets whose root paths move
- defer automatic adopt/detect-move behavior until after the explicit relocate path is proven

## Phase 4: Monitoring and Alerts

- add notifications for blocked sync, repeated failures, stale last-success time, and new drift artifacts
- store sync history, health transitions, and acknowledgements
- expose alert state in both CLI and MCP
- ship a first local notification path for active alerts
- refine alert deduplication and escalation after the first notification slice lands

## Phase 5: UI

- add a menu bar app
- show green/yellow/red health state
- expose sync now, pause, resume, open logs, and reveal conflicts
- keep all real logic in the shared core plus CLI and MCP layers

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

## Phase 3: Folder Policies and Safe Execution

- classify folders as two-way, backup-only, excluded, or hold
- run preflight and folder gating before each sync
- allow folder-scoped rebaseline instead of broad `--resync`
- add quarantine handling for conflict and `safeBackup` artifacts

## Phase 4: Monitoring and Alerts

- add notifications for blocked sync, repeated failures, stale last-success time, and new drift artifacts
- store sync history, health transitions, and acknowledgements
- expose alert state in both CLI and MCP

## Phase 5: UI

- add a menu bar app
- show green/yellow/red health state
- expose sync now, pause, resume, open logs, and reveal conflicts
- keep all real logic in the shared core plus CLI and MCP layers

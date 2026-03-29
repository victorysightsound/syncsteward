# SyncSteward Architecture

## Purpose

SyncSteward replaces the current opaque shell-script-plus-launchd sync model with a safety-first application layer.

The product goal is simple:

- do not let background sync silently drift for weeks
- do not let `rclone bisync` and remote OneDrive mutate the same tree without coordination
- make failures visible before they become data-loss events

## Product Direction

SyncSteward is being built CLI-first and MCP-first. A future UI should sit on top of the same core operations rather than inventing separate sync logic.

The app is responsible for:

- sync health inspection
- guarded preflight checks
- conflict and backup artifact detection
- sync orchestration policy
- notifications and failure escalation
- future folder policy management and controlled re-enablement

## Current System Risks

The current stack has several failure modes:

- broad folder-level `rclone bisync` across large personal trees
- a remote Linux `onedrive` monitor mutating the same tree that `bisync` is targeting
- no central preflight gate
- no quarantine workflow for conflict markers
- no dashboard or alerting for `out of sync` states

## First Hardening Slice

The first implementation slice is intentionally read-only.

It answers:

- Is the local launch agent loaded?
- Is the remote OneDrive service active?
- Are there any `.conflict*` or `victorystore-safeBackup` artifacts?
- Does the latest `rclone` log show `out of sync`, warning, or error states?
- Is the system safe to re-enable?

## Core Model

### Configuration

SyncSteward loads configuration from either:

- an explicit config path
- `~/.config/syncsteward/config.toml`
- built-in defaults if no config file exists

### Status

Status is a neutral snapshot of:

- local sync writer state
- remote sync writer state
- drift artifact counts and examples
- latest sync log summary

### Preflight

Preflight is a policy decision layered on top of status. It should fail closed.

Examples of fail conditions:

- local launch automation still loaded
- remote OneDrive service still active
- unresolved conflict artifacts
- unresolved `safeBackup` artifacts
- latest `rclone` log still reports `out of sync`

## Planned Waves

1. Health and preflight inspection
2. Coordinated pause/resume and controlled sync execution
3. Per-folder sync policy and quarantine management
4. Notifications and escalation
5. Menu bar UI and operator workflow polish

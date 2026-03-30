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
- explicit acknowledgement of historical incident logs after cleanup
- sync orchestration policy
- file-class safety defaults for risky artifacts like live SQLite databases
- inventorying the current legacy sync targets before any re-enablement plan is applied
- turning recommended target policies into an explicit managed config
- explaining readiness and blockers per target before any selective re-enablement
- assigning durable IDs to managed targets as groundwork for future relocate/adopt workflows
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

Configuration now carries both operator paths and safety policy:

- launch agent and remote service locations
- log, audit-log, state, filter, and legacy-lock paths
- scan roots
- explicitly managed targets with durable ID, local path, remote path, mode, and rationale
- folder policy overrides
- file-class policy defaults
- target-specific exclusion rules for protected bundles and subtrees
- target-specific snapshot rules for runtime SQLite-backed targets
- alert thresholds and notification toggles

### Status

Status is a neutral snapshot of:

- local sync writer state
- remote sync writer state
- drift artifact counts and examples
- acknowledged historical log baseline, if one exists
- latest sync log summary
- active folder, file-class, target-exclusion, and target-snapshot policy defaults

### Policy Model

SyncSteward is folder-first, with file-class overrides for dangerous content.

- folder policies express the normal behavior for a subtree
- managed targets define explicit curated paths that should participate in execution even when a broader parent folder remains on hold
- managed targets should carry durable identity so future relocate/adopt flows can reconnect the same target after its root path moves
- file-class policies can tighten safety for specific artifacts
- target-specific exclusions can protect known bundle/package paths inside otherwise executable targets
- target-specific snapshots can replace live runtime databases with staged SQLite backups during execution
- specific path overrides will come later for rare exceptions

The default dangerous-file posture is fail-safe:

- `*.db`, `*.sqlite`, `*.sqlite3` default to `backup_only`
- `*.db-wal`, `*.sqlite-wal`, `*.sqlite3-wal` default to `backup_only`
- `*.db-shm`, `*.sqlite-shm`, `*.sqlite3-shm` default to `backup_only`
- `*.conflict*` defaults to `hold`
- `*victorystore-safeBackup*` defaults to `hold`

The first target-specific exclusions protect native Apple libraries inside approved backup-only targets:

- `Pictures` excludes `Photos Library.photoslibrary`
- `Music` excludes `Music Library.musiclibrary`

Those bundle exclusions are enforced by SyncSteward itself so backup-only media targets do not depend only on a legacy filter file.

The first target-specific snapshot rule protects `.memloft`:

- SyncSteward syncs non-database files from the live tree through the existing filtered path
- `memloft.db`, `payroll.db`, and `vault.db` are uploaded from `sqlite3 .backup` snapshots instead of the live files

That keeps runtime SQLite backup coherent without forcing a full local mirror of the `.memloft` tree on every run.

### Legacy Target Inventory

SyncSteward should read the current `cloud-sync.sh` target list instead of guessing what is being synchronized today.

For each legacy target it should expose:

- legacy mode (`bisync` or one-way backup)
- local and remote path
- recommended SyncSteward policy
- rationale for the recommendation
- any explicit configured override that already exists

SyncSteward should also allow explicit managed targets that do not come from the legacy script.

Those managed targets exist for the transition period where:

- a broad legacy folder is too risky to re-enable as a whole
- one or more curated subfolders inside it are safe enough to back up
- the operator needs those curated paths to behave like first-class targets in inventory, readiness, execution, and alerts

The first practical example is a held top-level `Notes` folder with a separately managed `Notes/Personal` backup-only target.

Each managed target should also be able to carry a durable ID. That identity is what allows SyncSteward to distinguish:

- normal file and folder moves inside the target root, which should sync naturally
- a move of the target root itself, which should trigger an explicit relocate/adopt workflow instead of being treated as unrelated deletes and uploads

That identity layer should not remain passive metadata. SyncSteward should expose explicit lifecycle actions so operators can:

- add a new managed target without editing config by hand
- relocate an existing managed target by ID, name, or current path
- preserve the same target identity and run history when the managed target root moves

It should also expose a target-scoped readiness view so an operator can see:

- the effective mode after configured overrides are applied
- whether the target is ready under the current global preflight state
- whether the target is blocked by policy, missing local paths, or unresolved global failures

SyncSteward should also be able to scaffold those recommendations into a real config file so re-enablement happens from explicit policy, not from built-in assumptions.

### Preflight

Preflight is a policy decision layered on top of status. It should fail closed.

Examples of fail conditions:

- local launch automation still loaded
- remote OneDrive service still active
- unresolved conflict artifacts
- unresolved `safeBackup` artifacts
- latest `rclone` log still reports `out of sync`

An acknowledged historical incident log may downgrade the latest-log blocker to a warning, but only when the exact latest log summary still matches the recorded baseline.

### Coordinated Control

Pause and resume are explicit control actions, not side effects of a timer.

- `pause` is idempotent and should safely no-op if the stack is already paused
- `resume` always runs behind the preflight gate
- blocked resume attempts must explain exactly which checks are still failing
- every pause or resume action should append a structured audit record

The next execution layer is also explicit and fail-safe:

- folder-scoped execution is allowed only for targets that pass global preflight and target readiness
- the first executable slice is limited to `backup_only` targets
- executable targets may come from either the legacy script inventory or explicit managed-target config
- execution must respect the legacy sync lock so manual runs cannot overlap the old script
- every target run should append audit history and record last outcome in state
- future relocate/adopt commands should use managed target IDs instead of path-only matching when reconnecting moved target roots
- add/relocate target mutations should update config through the same guarded control plane instead of forcing manual config edits

Monitoring should build on the same state model rather than inventing a separate tracker:

- active alerts should derive from current preflight plus per-target run history
- executable targets without any successful live run should surface as alerts
- stale-success thresholds should be configurable
- local notifications should summarize active alerts without hiding the underlying details

## Planned Waves

1. Health and preflight inspection
2. Coordinated pause/resume and structured audit logging
3. Per-folder sync policy, managed subtargets, durable target IDs, managed-target lifecycle commands, config scaffolding, file-class overrides, and quarantine management
4. Notifications and escalation
5. Menu bar UI and operator workflow polish

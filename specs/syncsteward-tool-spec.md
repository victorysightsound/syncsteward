# SyncSteward Tool Spec

## Interface Rule

Meaningful product-facing capabilities should ship in both CLI and MCP form.

CLI remains the operator surface for:

- local administration
- scripting
- diagnostics
- explicit sync control

MCP remains the AI-facing surface for:

- health inspection
- guarded orchestration
- future multi-tool automation

## Initial CLI Surface

### `syncsteward status`

Return a full health snapshot:

- config source
- active folder, file-class, target-exclusion, and target-snapshot policy defaults
- local launch agent state
- dedicated SyncSteward runner launch-agent state
- remote host reachability and OneDrive service state
- conflict and `safeBackup` artifact counts
- latest sync log summary

Supports:

- human output
- JSON output

### `syncsteward preflight`

Evaluate whether sync is safe to re-enable or run manually.

Outputs:

- overall readiness
- per-check pass/warn/fail state
- the underlying status snapshot

Supports:

- human output
- JSON output

### `syncsteward targets`

Read the current `cloud-sync.sh` target inventory and merge it with any explicit managed targets from SyncSteward config.

Outputs:

- script path
- combined target list
- stable target ID for each managed target when configured
- local and remote path for each target
- legacy mode (`bisync`, one-way backup, or managed target)
- recommended SyncSteward policy
- configured override, if present
- rationale for the recommendation

Supports:

- human output
- JSON output

### `syncsteward overview`

Read one composed operator summary across preflight, runner state, approved targets, recent target runs, and alerts.

Outputs:

- config source
- generated timestamp
- preflight readiness plus failing/warning check counts
- runner cadence, due state, and last cycle/tick summaries
- target health counts across configured, managed, approved, ready, blocked, and live-success targets
- approved target resolution/readiness with latest recorded run when available
- recent target-run history sorted newest first
- active alert list

Supports:

- human output
- JSON output

Rules:

- this is the preferred read surface for dashboard-style UI consumers
- future native UI shells should consume this contract instead of stitching together multiple health commands on their own

### `syncsteward check-targets`

Explain readiness and blockers for every configured sync target.

Outputs:

- overall preflight readiness
- one evaluation per target
- includes explicit managed targets even when their parent legacy folder is still on hold
- includes the stable managed-target ID when present
- effective mode after configured overrides are applied
- blocker details for hold, excluded, missing-path, and global preflight failures

Supports:

- human output
- JSON output

### `syncsteward check-target`

Explain readiness and blockers for one configured sync target by name or local path.

Outputs:

- overall preflight readiness
- one target evaluation
- supports both legacy targets and explicit managed targets
- includes the stable managed-target ID when present
- effective mode after configured overrides are applied
- blocker details for hold, excluded, missing-path, and global preflight failures

Supports:

- human output
- JSON output

### `syncsteward ensure-target-ids`

Assign stable IDs to managed targets in the SyncSteward config file.

Outputs:

- config path
- assigned ID count
- preserved ID count
- one assignment record per target that needed a new ID

Rules:

- only writes when managed targets exist and at least one ID is missing or duplicated
- keeps existing unique IDs unchanged
- repairs duplicate managed-target IDs by assigning a fresh ID to the later conflicting target
- creates the identity layer needed for future relocate/adopt workflows

Supports:

- human output
- JSON output

### `syncsteward add-managed-target`

Register a new managed target in the SyncSteward config file.

Outputs:

- config path
- durable target ID
- target name
- local path
- remote path
- configured mode

Rules:

- requires an existing config file
- requires the local path to exist and be a directory
- assigns a durable managed-target ID immediately
- refuses duplicate target name, local path, or remote path
- makes the new target available everywhere a managed target already participates

Supports:

- mode selection
- optional rationale
- human output
- JSON output

### `syncsteward relocate-managed-target`

Relocate an existing managed target by stable ID, target name, or current local path.

Outputs:

- config path
- selector used
- previous local path
- previous remote path
- current target record after relocation

Rules:

- requires an existing config file
- matches managed targets by ID first, then by name or local path
- preserves the durable target ID and run history
- requires the new local path to exist and be a directory
- optionally updates the remote path at the same time
- returns `no_op` when the requested location already matches the current target

Supports:

- human output
- JSON output

### `syncsteward run-target`

Run one configured target with full preflight and policy gating.

Outputs:

- overall run outcome
- target evaluation at execution time
- structured execution steps
- dry-run flag

Rules:

- initial support is limited to `backup_only` targets
- blocked if global preflight is not ready
- blocked if the target is on hold, excluded, or missing locally
- blocked if the legacy sync lock is already owned by another process
- supports explicit managed targets so curated subfolders can run while broader legacy folders remain on hold
- merges target-specific exclusion rules into the temporary filter set for the active run
- uses target-specific snapshot rules when a runtime target should upload SQLite backups instead of the live database files
- target snapshot rules are selective: they exclude and replace only the listed live database files while other database files in the same target continue through normal backup-only sync
- excludes SQLite sidecars like `*-wal`, `*-shm`, and `*-journal` from direct sync
- records last live target outcome in SyncSteward state
- does not let dry-run validation overwrite the live target state used by alerts
- appends a target-run audit record

Supports:

- `--dry-run`
- human output
- JSON output

### `syncsteward run-cycle`

Run the guarded execution cycle for the approved target set in SyncSteward config.

Outputs:

- config source
- dry-run flag
- overall cycle outcome
- summary of the cycle result
- preflight readiness at cycle start
- approved target count
- per-target run reports
- skipped selectors that did not resolve to a current target
- current alert set after the cycle
- optional notification result

Rules:

- reads the approved target list from config instead of taking an ad hoc target list on the command line
- reuses the same guarded single-target execution path as `run-target`
- acquires the legacy sync lock once for the full cycle so overlapping cycles and manual target runs cannot interleave
- only runs targets that still resolve cleanly and pass target readiness
- records skipped selectors when config references a missing or obsolete target
- records cycle outcome in SyncSteward state and audit history
- may send post-cycle alert notifications when enabled in config
- is the intended orchestration entry point for future daemon and UI layers

Supports:

- `--dry-run`
- human output
- JSON output

### `syncsteward runner-tick`

Run one daemon-ready scheduled tick for the approved target set.

Outputs:

- config source
- dry-run flag
- overall tick outcome
- due flag
- cycle interval in minutes
- last live cycle completion timestamp
- next due timestamp, when available
- preflight readiness
- optional nested cycle report when a cycle was due
- current alert set after the tick
- optional notification result
- structured tick steps

Rules:

- reads cadence and approved-target settings from `runner.*` config
- runs `run-cycle` only when the approved set is due
- otherwise returns a safe no-op report with the current alert snapshot
- may send post-tick alert notifications when enabled in config
- suppresses repeated notifications for the same unchanged alert set until the configured repeat window expires
- may send one recovery notification when a previously active alert set clears
- is the intended entry point for future launchd, daemon, and menu bar scheduling

Supports:

- `--dry-run`
- human output
- JSON output

### `syncsteward runner-agent-status`

Read the dedicated SyncSteward runner launch-agent state.

Outputs:

- config source
- runner-agent label
- plist path
- whether the plist exists
- whether the agent is loaded
- whether the agent is running
- launchctl detail

Supports:

- human output
- JSON output

### `syncsteward install-runner-agent`

Write and load the dedicated SyncSteward runner launch agent.

Outputs:

- config source
- install outcome
- runner-agent status after installation
- structured steps

Rules:

- requires a real SyncSteward config file so the launch agent points at a stable config path
- writes a dedicated launchd plist for `runner-tick`
- loads the agent unless `--write-only` is used
- replaces an already-loaded copy cleanly before bootstrapping the new one
- keeps the legacy `com.cloud-sync` job separate and unchanged

Supports:

- `--write-only`
- human output
- JSON output

### `syncsteward uninstall-runner-agent`

Unload the dedicated SyncSteward runner launch agent and optionally keep its plist.

Outputs:

- config source
- uninstall outcome
- runner-agent status after uninstall
- structured steps

Rules:

- unloads the dedicated SyncSteward runner agent if it is loaded
- removes the plist by default
- supports keeping the plist for later reuse with `--keep-plist`

Supports:

- `--keep-plist`
- human output
- JSON output

### `syncsteward alerts`

Evaluate active alerts from current preflight state and per-target run history.

Outputs:

- generated timestamp
- overall preflight readiness
- stale-success threshold
- repeat-notification threshold
- active alerts with severity, summary, detail, and target context

Rules:

- executable `backup_only` targets with no live success should alert
- executable targets with no run history should alert
- executable targets whose last live success is stale should alert
- global preflight failures should surface as critical alerts

Supports:

- human output
- JSON output

### `syncsteward notify-alerts`

Send a local notification that summarizes the current alert set.

Outputs:

- notification outcome
- active alerts included in the notification
- structured notification steps

Rules:

- no-op when there are no active alerts
- respects the config toggle for macOS notifications
- supports dry-run for validation
- direct operator-triggered notify runs remain explicit and are not subject to scheduled repeat suppression

Supports:

- `--dry-run`
- human output
- JSON output

### `syncsteward acknowledge-latest-log`

Record the current latest `rclone` log as an acknowledged historical baseline after cleanup.

Rules:

- only intended for known historical incidents
- allows preflight to distinguish old acknowledged failures from new failures
- stores the acknowledgement in SyncSteward state, not in the log itself

Supports:

- human output
- JSON output

### `syncsteward scaffold-config`

Write a real SyncSteward config file from the current target inventory and recommended policies.

Outputs:

- config path
- whether an existing file was overwritten
- folder policy count
- file-class policy count

Rules:

- refuses to overwrite by default
- `--force` is required to replace an existing config
- uses existing configured folder modes when refreshing an existing config

Supports:

- human output
- JSON output

### `syncsteward pause`

Pause:

- the local launch agent
- the remote OneDrive service
- or both

Rules:

- idempotent
- returns structured step-by-step action details
- exits non-zero on mutation failure
- appends a structured audit record

Supports:

- `--target local|remote|all`
- human output
- JSON output

### `syncsteward resume`

Resume:

- the local launch agent
- the remote OneDrive service
- or both

Rules:

- any resume path is blocked until preflight succeeds
- returns structured blocker details when blocked
- exits with a distinct non-zero code when blocked
- appends a structured audit record

Supports:

- `--target local|remote|all`
- human output
- JSON output

### `syncsteward mcp stdio`

Run the MCP server over stdio.

## Initial MCP Surface

### `overview`

Read the same composed overview exposed by the CLI.

### `status`

Read the same health snapshot exposed by the CLI.

### `preflight`

Run the same guarded preflight checks exposed by the CLI.

### `targets`

Read the same legacy-target inventory and recommended policy view exposed by the CLI.

### `check_targets`

Read the same per-target readiness and blocker view exposed by the CLI.

### `check_target`

Read the same single-target readiness and blocker view exposed by the CLI.

### `run_target`

Run the same guarded single-target execution path exposed by the CLI, including dry-run support.

### `alerts`

Read the same alert evaluation surface exposed by the CLI.

### `notify_alerts`

Send the same guarded local alert notification exposed by the CLI, including dry-run support.

### `run_cycle`

Run the same approved-target guarded cycle exposed by the CLI, including dry-run support.

### `runner_tick`

Run the same daemon-ready scheduled tick exposed by the CLI, including dry-run support.

### `runner_agent_status`

Read the same dedicated runner launch-agent state exposed by the CLI.

### `install_runner_agent`

Write and load the same dedicated runner launch agent exposed by the CLI, including write-only support.

### `uninstall_runner_agent`

Unload the same dedicated runner launch agent exposed by the CLI, including keep-plist support.

### `acknowledge_latest_log`

Record the same historical-log baseline acknowledgement exposed by the CLI.

### `scaffold_config`, `scaffold_config_force`

Write the same config scaffold exposed by the CLI, with a separate force-overwrite variant for MCP.

### `ensure_target_ids`

Assign the same stable managed-target IDs exposed by the CLI.

### `add_managed_target`

Register the same managed-target lifecycle mutation exposed by the CLI.

### `relocate_managed_target`

Run the same managed-target relocation workflow exposed by the CLI while preserving durable target identity.

### `pause_all`, `pause_local`, `pause_remote`

Run the same coordinated pause actions exposed by the CLI.

### `resume_all`, `resume_local`, `resume_remote`

Run the same guarded resume actions exposed by the CLI.

## Deferred Surface

Deferred for later waves:

- folder policy editing
- conflict quarantine moves
- managed target adopt/detect-move automation beyond explicit relocate
- UI

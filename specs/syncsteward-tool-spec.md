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
- active folder, file-class, and target-exclusion policy defaults
- local launch agent state
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

Read the current `cloud-sync.sh` target inventory and classify each target with a safer recommended policy.

Outputs:

- script path
- legacy target list
- local and remote path for each target
- legacy mode (`bisync` or one-way backup)
- recommended SyncSteward policy
- configured override, if present
- rationale for the recommendation

Supports:

- human output
- JSON output

### `syncsteward check-targets`

Explain readiness and blockers for every configured sync target.

Outputs:

- overall preflight readiness
- one evaluation per target
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
- effective mode after configured overrides are applied
- blocker details for hold, excluded, missing-path, and global preflight failures

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
- merges target-specific exclusion rules into the temporary filter set for the active run
- records last target outcome in SyncSteward state
- appends a target-run audit record

Supports:

- `--dry-run`
- human output
- JSON output

### `syncsteward alerts`

Evaluate active alerts from current preflight state and per-target run history.

Outputs:

- generated timestamp
- overall preflight readiness
- stale-success threshold
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

### `acknowledge_latest_log`

Record the same historical-log baseline acknowledgement exposed by the CLI.

### `scaffold_config`, `scaffold_config_force`

Write the same config scaffold exposed by the CLI, with a separate force-overwrite variant for MCP.

### `pause_all`, `pause_local`, `pause_remote`

Run the same coordinated pause actions exposed by the CLI.

### `resume_all`, `resume_local`, `resume_remote`

Run the same guarded resume actions exposed by the CLI.

## Deferred Surface

Deferred for later waves:

- folder policy editing
- conflict quarantine moves
- UI

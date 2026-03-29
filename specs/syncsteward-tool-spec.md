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
- active folder and file-class policy defaults
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

### `acknowledge_latest_log`

Record the same historical-log baseline acknowledgement exposed by the CLI.

### `scaffold_config`, `scaffold_config_force`

Write the same config scaffold exposed by the CLI, with a separate force-overwrite variant for MCP.

### `pause_all`, `pause_local`, `pause_remote`

Run the same coordinated pause actions exposed by the CLI.

### `resume_all`, `resume_local`, `resume_remote`

Run the same guarded resume actions exposed by the CLI.

## Deferred Surface

Not part of the first slice:

- folder policy editing
- conflict quarantine moves
- notifications
- UI

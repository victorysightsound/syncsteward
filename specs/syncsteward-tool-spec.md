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

### `syncsteward mcp stdio`

Run the MCP server over stdio.

## Initial MCP Surface

### `status`

Read the same health snapshot exposed by the CLI.

### `preflight`

Run the same guarded preflight checks exposed by the CLI.

## Deferred Surface

Not part of the first slice:

- automatic sync restart
- folder policy editing
- conflict quarantine moves
- remote service mutation
- notifications
- UI

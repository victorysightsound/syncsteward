# SyncSteward DIAL Plan

## Product Target

SyncSteward is a safety-first sync control plane for the current Mac plus Victorystore setup.

Long-term shape:

- SyncSteward on the Mac is the operator control plane.
- Victorystore remains the managed sync endpoint and remote worker host.
- All real sync policy, state, recovery, and scheduling live in the shared Rust core.
- CLI and MCP are the primary product surfaces until the core is production-ready.
- UI is deferred until after the control plane is stable and proven.

## Production-Ready Definition

The product is production-ready only when all of these are true:

- configuration loads, validates, and writes cleanly
- inventory is explicit and matches reality
- remote OneDrive coordination is automatic and idempotent
- live runs are verified, not merely exited cleanly
- repair, rebaseline, and quarantine flows are safe
- alerts are accurate, deduplicated, and recoverable
- scheduled runs do not overlap or race with manual runs
- the operator can prove behavior through repeatable validation checks
- legacy compatibility paths are optional, not required for correctness

## Phase 0: Baseline And Control Surface

Purpose:

- lock the product contract before more features land
- keep the docs, config model, and operator surfaces aligned

Tasks:

- confirm the workspace layout and shared Rust core boundaries
- keep the config schema and snapshot model authoritative
- keep `status`, `preflight`, `overview`, and `config` available in both CLI and MCP
- keep the docs database synchronized with the markdown specs
- define the stable terminology for targets, managed targets, approved targets, verification, and recovery

Exit criteria:

- docs, config model, and operator surfaces describe the same system
- status and preflight are reliable enough to gate all later phases

## Phase 1: Core State And Configuration

Purpose:

- make the state model explicit enough to support production operations

Tasks:

- normalize all config inputs and snapshot outputs
- keep operator paths, remote sync root, scan roots, and policy defaults in one config model
- persist target state, last success time, failure class, consecutive failures, and repair timestamps
- keep dry-run paths observable without overwriting live state
- keep the CLI and MCP config surfaces in parity

Exit criteria:

- config changes are deterministic
- state can be inspected, updated, and persisted without ambiguity
- the control surfaces expose the same model to humans and automation

## Phase 2: Safe Coordination With Victorystore

Purpose:

- guarantee that SyncSteward and Victorystore OneDrive do not fight over the same tree

Tasks:

- pause Victorystore OneDrive before live sync work when coordination is enabled
- resume Victorystore OneDrive after successful coordination or safe failure
- make pause and resume idempotent and fail-closed
- record coordination steps in run history and audit output
- materialize fragile instruction-file symlink chains into real files inside the remote sync root
- keep dry-run behavior visible without mutating the remote endpoint

Exit criteria:

- one SyncSteward run can safely bracket the remote worker
- coordination is automatic, logged, and repeatable
- symlink-chain failures are eliminated as a class of sync noise

## Phase 3: Inventory, Policy, And Target Identity

Purpose:

- turn the old folder list into explicit managed targets with durable identity

Tasks:

- inventory legacy `cloud-sync.sh` targets when present
- fall back to managed-target config when the legacy script is missing
- classify targets as two-way, backup_only, hold, or excluded
- protect live SQLite data with snapshots and sidecar exclusions
- protect Apple bundles and other fragile packages with target-specific exclusions
- assign durable IDs to managed targets
- add add/relocate/rebaseline lifecycle commands for managed targets
- support target-scoped readiness checks and blocker explanations

Exit criteria:

- every production target has an explicit policy
- a target can move without losing identity or run history
- the operator can explain why each target is ready or blocked

## Phase 4: Execution, Verification, And Recovery

Purpose:

- make sync behavior correct, then prove it stayed correct

Tasks:

- keep `run-target` and `run-cycle` guarded by preflight and policy
- add `verify-target` as a first-class operation
- add `repair-target` and `rebaseline-target` as explicit recovery paths
- require confirmation for destructive recovery actions and keep repair non-destructive
- classify failures into transport, auth, path, divergence, snapshot, and unknown
- retry transient transport failures without turning the cycle red too early
- keep quarantine handling for conflict and safe-backup artifacts
- make verified success the health signal, not a bare exit code

Exit criteria:

- a live target can be run, verified, repaired, and rebaselined from CLI and MCP
- a transient failure does not poison the entire cycle
- alerts reflect verified reality instead of optimistic exit status

## Phase 5: Monitoring, Scheduling, And Alerts

Purpose:

- make the system self-explaining and safe to leave alone

Tasks:

- store history, transitions, acknowledgements, and recovery events
- expose one composed `overview` surface for CLI, MCP, and future UI consumers
- add alert deduplication and repeat-window suppression
- send recovery notifications when active alert sets clear
- add `runner-tick` for scheduled checks
- add a dedicated runner launch agent with a stable tool path
- keep scheduled execution separate from the legacy broad sync job

Exit criteria:

- scheduled execution is safe and predictable
- alerts are useful instead of noisy
- operator dashboards can read one summary surface

## Phase 6: Legacy De-Risking And Migration

Purpose:

- remove the old shell-script model as an execution dependency

Tasks:

- keep `cloud-sync.sh` only as compatibility inventory when present
- make SyncSteward-owned config the authoritative runtime model
- ensure the legacy script is never required for normal execution
- keep explicit managed targets covering the important curated paths
- document the migration path away from the old broad sync job

Exit criteria:

- the product can run entirely from SyncSteward config and state
- legacy discovery is optional, not required

## Phase 7: Operator Validation And Regression Coverage

Purpose:

- prove the product under real operator scenarios before any UI work

Tasks:

- keep the operator validation checklist current
- test missing paths, auth failures, transport failures, stale state, and drift artifacts
- test pause/resume behavior under real endpoint conditions
- test live sync on a small safe target set before trusting broad runs
- keep regression tests for pruning, chronic failure alerts, verification timestamps, and dry-run safety
- record known failure patterns in memory and docs

Exit criteria:

- every major failure class has a repeatable drill
- the operator can prove the system is safe with CLI and MCP alone

## Phase 8: Packaging, Support, And Production Hardening

Purpose:

- make the product easy to install, support, and maintain

Tasks:

- keep launchd integration and external tool path resolution robust
- keep logs, state, and audit files organized and recoverable
- make config and state writes atomic, explicit, and reversible
- keep timestamped rollback copies for config overwrites and protect state persistence from partial writes
- keep the docs database and markdown specs synchronized
- add any missing packaging or install helpers needed for normal use
- keep CI green on the core workspace

Exit criteria:

- the product can be maintained without tribal knowledge
- operational recovery does not require manual state editing

## Phase 9: UI, Deferred

Purpose:

- build the thin UI only after the control plane is proven

Tasks:

- create a native shell that reads the composed overview surface
- expose safe refresh and open-log/config actions only
- keep all sync logic in the shared core, CLI, and MCP layers
- avoid adding any UI-only sync behavior

Exit criteria:

- the UI reads from the proven control plane instead of owning logic
- the product remains CLI/MCP first even after the UI exists

## Completion Rule

Do not start the UI phase until Phase 8 is complete and the following are true:

- the live Mac/Victorystore path set completes cleanly
- verification is wired into the success signal
- coordination with Victorystore OneDrive is automatic and stable
- recovery paths have been exercised and tested
- the operator validation checklist passes end to end

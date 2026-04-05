# SyncSteward Operator Validation Checklist

Use this checklist before trusting a new change set, a new machine, or a new sync target layout.

## 1. Config And State

- `syncsteward config` loads cleanly.
- `syncsteward config-schema` still matches the config model.
- `syncsteward config-set --dry-run` reports the intended diff and changes nothing.
- `syncsteward prune-state --dry-run` reports only stale state.
- `syncsteward prune-state` removes only stale state entries.

## 2. Inventory And Readiness

- `syncsteward targets` shows the expected target list.
- Legacy inventory is reported as available when `cloud-sync.sh` exists.
- Managed-target fallback works when the legacy script is missing.
- `syncsteward check-targets` shows the right blockers for held or missing paths.
- `syncsteward preflight` is ready before any live sync.
- `syncsteward quarantine-artifacts --dry-run` shows the intended artifact moves without mutating files.

## 3. Sync Execution

- `syncsteward run-target <target> --dry-run` succeeds for approved `backup_only` targets.
- `syncsteward run-target <target>` succeeds for a safe live target.
- `syncsteward run-cycle --dry-run` succeeds for the approved target set.
- `syncsteward run-cycle` succeeds for the approved target set.
- One transient failure does not poison the full approved cycle.
- One launchd-managed `runner-tick` succeeds through the real background execution path.
- An interrupted runner cycle is cleared automatically on the next `runner-tick`.

## 4. Failure And Recovery

- A missing path is reported as a path blocker.
- An authentication or transport failure is reported in the right failure class.
- Consecutive live failures appear as a chronic-failure alert.
- A successful live run resets the failure counter for that target.
- `pause` blocks sync and `resume` stays blocked until preflight passes.
- `quarantine-artifacts` moves conflict and safeBackup leftovers into quarantine with a manifest and clears the corresponding preflight blockers.

## 5. Operator Exit Criteria

Do not call the system production-ready until all of these are true:

- config reads and writes are clean
- inventory matches reality
- pruning is safe and deterministic
- dry-run behavior never overwrites live run state
- failure counts and chronic alerts behave as expected
- a live cycle completes cleanly on the real Mac/victorystore path set

## Automated Coverage

- `crates/syncsteward-core/src/inventory.rs`
  - falls back to managed targets when the legacy script is missing
  - errors when neither legacy inventory nor managed targets exist
- `crates/syncsteward-core/src/probe.rs`
  - prunes stale target-run state without touching current entries
  - surfaces chronic-failure alerts
  - resets consecutive failure count after success
- `crates/syncsteward-core/src/state.rs`
  - persists and prunes target-run state safely

## Suggested Live Run Order

1. `syncsteward preflight`
2. `syncsteward targets`
3. `syncsteward check-targets`
4. `syncsteward run-cycle --dry-run`
5. `syncsteward run-target <safe-target> --dry-run`
6. `syncsteward run-target <safe-target>`
7. `syncsteward prune-state --dry-run`
8. `syncsteward prune-state`

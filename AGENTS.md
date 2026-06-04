## Project Overview

`framelog` is a Rust Linux service for collecting Framework laptop power and
firmware diagnostics. It samples AMDGPU throttle flags, stores telemetry in
SQLite, correlates transitions with systemd journal entries, and serves a local
web UI for timeline inspection.

Core areas:

- `src/collector.rs` — AMDGPU metric collection via `libamdgpu_top`, plus fake
  and `amdgpu_top` JSON collectors for testing/debugging.
- `src/throttle.rs` — stable throttle flag names and transition diffing.
- `src/store.rs` — SQLite schema and query layer.
- `src/journal.rs` — systemd journal lookups around interesting transitions.
- `src/service.rs` — sampler loop and transition orchestration.
- `src/system_info.rs` — cached Linux system inventory from read-only
  procfs/sysfs/os-release sources.
- `src/web.rs` and `static/` — local API and graph UI.
- `systemd/` — service unit examples.

Keep data acquisition modular. Hardware, OS, and desktop context sources should
be isolated behind small interfaces so future signals can be added without
rewriting storage, transition logic, or the UI.

For [Framework issue #146](https://github.com/FrameworkComputer/SoftwareFirmwareIssueTracker/issues/146)
power-cap debugging, prioritize read-only evidence that ties together dGPU runtime
state (`gpu_power`), AMD PMF limits from debugfs (`pmf`), charger/profile context,
and AMDGPU throttle samples. Do not mutate firmware or SMU tables from the collector;
full ACPI `STTC` extraction belongs in a future explicit inspect command, not the
hot sampling loop.

---

## Issue Tracking with bd (beads)

This project should use **bd (beads)** for issue tracking once the beads
database is initialized. Do not create ad hoc markdown TODO lists for project
work; use beads issues for durable tasks and dependencies.

Run this at the start of a work session when beads is available:

```bash
bd prime
bd ready --json
```

### Workflow for AI Agents

1. **Check ready work**: `bd ready --json`
2. **Claim your task**: `bd update <id> --status in_progress --json`
3. **Work on it**: implement, test, document
4. **Discover new work?** `bd create "Found bug" -p 1 --deps discovered-from:<parent-id> --json`
5. **Complete**: `bd close <id> --reason "Done" --json`
6. **Commit together**: include `.beads/issues.jsonl` with related code changes

### Important Rules

- Use `bd` for durable project task tracking.
- Always use `--json` for programmatic bead commands.
- Close issues only after relevant build, lint, and tests pass.
- Store ephemeral AI planning docs in `.cursor/plans/` or `history/`.
- Do not keep long-lived project TODO lists in markdown.
- If `.beads/` is not initialized yet, ask before initializing it.

---

## Development Commands

Use the stable Rust toolchain.

```bash
cargo fmt
cargo test
cargo clippy --all-targets --all-features -- -D warnings
cargo build --release
```

Useful local runs:

```bash
# Real hardware path
framelog inspect
framelog run --db ~/.local/share/framelog/framelog.db

# No hardware required
framelog inspect --collector fake
framelog run --collector fake --db /tmp/framelog.db
framelog capture --duration 60s --collector fake --skip-pre-backup
framelog capabilities
framelog inspect --collector intel
framelog analyze export.json
framelog dump --output-dir /tmp
```

The web UI defaults to `http://127.0.0.1:8787`.

---

## Engineering Rules

- Prefer sysfs, D-Bus, and library APIs over shelling out. Shell collectors are
  acceptable only as explicit debug/fallback adapters.
- Preserve raw measurements in storage alongside normalized fields so parser
  fixes can reinterpret old data.
- Treat hardware metrics as best effort. Store source health and raw values when
  possible instead of hiding missing permissions or unsupported devices.
- Treat stable machine inventory (CPU, GPU, memory, storage, BIOS, Linux
  version) as cached metadata. Harvest at service start and refresh daily rather
  than sampling every tick.
- Avoid exposing hardware serial numbers in UI/API/export unless the user
  explicitly asks for them.
- Keep transition detection deterministic and tested; avoid noisy transitions by
  bucketing or thresholds for analog values.
- Do not add compatibility shims for unreleased internal data structures. When a
  branch-local schema is wrong, migrate or replace it cleanly.
- Keep comments rare and useful. Explain non-obvious hardware/kernel behavior,
  not ordinary Rust syntax.

---

## Testing Expectations

When adding collectors or context sources:

- Add parser tests using temporary fake sysfs/journal-like inputs where possible.
- Add transition tests for each new signal.
- Add SQLite round-trip tests for new tables/queries.
- Keep `--collector fake` useful for exercising the UI without Framework
  hardware.

Before calling work complete, run:

```bash
cargo fmt
cargo test
cargo clippy --all-targets --all-features -- -D warnings
```

---

## Logging

Use `tracing` for service diagnostics. Prefer structured fields:

```rust
tracing::info!(pci = %sample.device_pci, flag = %flag_name, "transition");
```

Avoid `println!` except for intentional CLI output such as `inspect`.

---

## Planning Documents

- Store Cursor plans in `.cursor/plans/`.
- Do not clutter the repo root with `PLAN.md`, `IMPLEMENTATION.md`, or similar.
- Keep architecture decisions in `docs/adr/` if they become durable project
  decisions.

---

## Session Completion

1. File beads issues for remaining follow-up work.
2. Run quality gates if code changed.
3. Update bead statuses.
4. Commit code and `.beads/issues.jsonl` together when beads is initialized.
5. Push only when explicitly requested by the user.

Work is not complete until the relevant tests pass and the issue tracker reflects
the state of the work.

<!-- BEGIN BEADS INTEGRATION v:1 profile:minimal hash:ca08a54f -->
## Beads Issue Tracker

This project uses **bd (beads)** for issue tracking. Run `bd prime` to see full workflow context and commands.

### Quick Reference

```bash
bd ready              # Find available work
bd show <id>          # View issue details
bd update <id> --claim  # Claim work
bd close <id>         # Complete work
```

### Rules

- Use `bd` for ALL task tracking — do NOT use TodoWrite, TaskCreate, or markdown TODO lists
- Run `bd prime` for detailed command reference and session close protocol
- Use `bd remember` for persistent knowledge — do NOT use MEMORY.md files

## Session Completion

**When ending a work session**, you MUST complete ALL steps below. Work is NOT complete until `git push` succeeds.

**MANDATORY WORKFLOW:**

1. **File issues for remaining work** - Create issues for anything that needs follow-up
2. **Run quality gates** (if code changed) - Tests, linters, builds
3. **Update issue status** - Close finished work, update in-progress items
4. **PUSH TO REMOTE** - This is MANDATORY:
   ```bash
   git pull --rebase
   bd dolt push
   git push
   git status  # MUST show "up to date with origin"
   ```
5. **Clean up** - Clear stashes, prune remote branches
6. **Verify** - All changes committed AND pushed
7. **Hand off** - Provide context for next session

**CRITICAL RULES:**
- Work is NOT complete until `git push` succeeds
- NEVER stop before pushing - that leaves work stranded locally
- NEVER say "ready to push when you are" - YOU must push
- If push fails, resolve and retry until it succeeds
<!-- END BEADS INTEGRATION -->

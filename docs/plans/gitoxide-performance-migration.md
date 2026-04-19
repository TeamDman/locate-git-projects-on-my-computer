# Gitoxide Performance Migration Plan

## Goal

Ship a standalone build that resolves all dependencies from crates.io and migrate discovery metadata enrichment from `git2` to `gix` while optimizing end-to-end time to completion. The target is a no-cache default run that is as fast as practical on the reference machine and ideally under one second, with Tracy captures that clearly explain where time is spent.

## Current Status

- Done so far:
  - Inspected the current discovery pipeline in `src/discovery.rs` and confirmed the shape is: Teamy MFT query for `.git` and `Cargo.toml`, then bounded `spawn_blocking` enrichment work collected through a `JoinSet`.
  - Replaced the `git2` metadata path with `gix` for repository open, remote enumeration, and reachable commit scanning.
  - Changed Teamy MFT candidate lookup so the `.git` and `Cargo.toml` indexed queries run concurrently before enrichment begins.
  - Added a concrete `DiscoveryConfig` model with configurable enrichment concurrency, minimum author scan depth, per-repository author budget, and chunk size.
  - Threaded those tuning controls through the CLI as optional named arguments.
  - Implemented budgeted author scanning that always completes the configured minimum work and then stops when the repository budget is exhausted.
  - Added coarse discovery-phase spans and Tracy-gated high-volume enrichment spans.
  - Updated the CLI and tools Tracey specs and added implementation references for the new discovery and profiling behavior.
  - Verified this repository already uses local Tracey specifications and a Tracy profiling workflow.
  - Captured the current Tracey baseline: `tracey query status` reports `locate-git-projects-on-my-computer-cli/rust` at `0 of 33` covered, `locate-git-projects-on-my-computer-publishing-standards/repo` at `1 of 8` covered, and `locate-git-projects-on-my-computer-tool-standards/rust` at `0 of 11` covered.
  - Switched `Cargo.toml` away from local path/git checkout assumptions by moving `teamy-mft`, `figue`, and `git2` to crates.io and aligning `teamy-windows` with the version already used transitively.
  - Verified the standalone dependency set with `cargo check`.
  - Verified the code with `cargo test`, `cargo check --features tracy`, and `tracey query validate --deny warnings`.
  - Measured a release-mode smoke run on this machine:
    - best measured minimal-author configuration: about `1.53s` with `--author-min-commits 1 --author-scan-budget-ms 0 --enrichment-max-in-flight 64`
    - current out-of-the-box tuned default: about `1.70s`
  - Improved Tracey coverage from the original baseline to `11 of 37` covered CLI requirements and `3 of 14` covered tool requirements.
- Current focus:
  - Close the remaining performance gap between the current `~1.5s` to `~1.7s` release runtime and the aspirational sub-second target.
- Remaining work:
  - Capture a real Tracy profile for the discovery path and identify whether the remaining time is dominated by Teamy MFT query, `Cargo.toml` parsing, repository open, or result merge.
  - Decide whether the next optimization slice should target Cargo manifest parsing, seed pre-grouping, or a lower-level Teamy MFT query API.
  - Tighten Tracey coverage further for the touched files if the repository wants a higher mapped percentage instead of just validation-clean status.
- Next step:
  - Run a Tracy capture against the new release path and use it to choose the next optimization target for the remaining `~0.5s` to `~0.7s` gap.

## Constraints And Assumptions

- The project must compile without requiring sibling repositories or git checkout path dependencies.
- The default CLI contract remains: no required arguments, pretty-printed JSON on stdout, logs on stderr.
- The implementation continues to depend on Teamy MFT's synced search index data for initial candidate discovery.
- Metadata enrichment remains best-effort. A single repository failure must not abort the whole run.
- Time to completion is the primary metric. Exhaustively scanning author history is not the default goal.
- The design allows per-repository author budgeting, but the current default is intentionally speed-biased until the remaining non-author bottlenecks are better understood.
- The author scan must always do a minimum amount of useful work before budget checks can stop it.
- Coarse spans should remain always-on where they are useful outside Tracy. High-volume inner spans should be gated behind `feature = "tracy"`.
- Routine validation should continue to avoid enabling `tracy` during tests, consistent with the current tool standards.

## Product Requirements

- The build must resolve from crates.io only.
- Discovery must continue to use Teamy MFT queries for exact `.git` directories and exact `Cargo.toml` files.
- After candidate collection, metadata enrichment must remain concurrent and bounded.
- Git metadata collection must use `gix`, not shelling out to `git`.
- Repository outlinks must continue to come from configured remotes.
- Author collection must use a configurable minimum scan depth plus a configurable per-repository time budget.
- The default author scan must always inspect at least the configured minimum number of reachable commits when they exist.
- After the minimum scan depth is satisfied, author scanning may continue only while the repository budget still has time remaining.
- If a repository exhausts its history before the budget expires, enrichment should finish immediately rather than waiting for the budget.
- The JSON output shape and existing name/author/URL filter semantics should remain stable unless a deliberate spec change says otherwise.
- Tracy captures must make it possible to separate MFT query work, enrichment dispatch, per-repository git work, author scanning, and final merge/serialization.

## Architectural Direction

- Preserve the current two-stage pipeline:
  - indexed candidate discovery first
  - parallel metadata enrichment second
- Keep blocking repository and filesystem work inside `tokio::task::spawn_blocking` tasks, coordinated through `tokio::task::JoinSet`.
- The current implementation now centralizes discovery tuning in `DiscoveryConfig` with repository defaults of:
  - `max_in_flight = 64`
  - `author_min_commits = 1`
  - `author_budget_per_repo = 0ms`
  - `author_scan_chunk_size = 64`
- The current implementation now uses `gix::open` plus `remote_names`/`find_remote`, `head_commit`, and `rev_walk` for git metadata enrichment.
- Implement author scanning in chunks:
  - scan the first required minimum commits unconditionally
  - after that, continue in chunk-sized batches
  - re-check elapsed time only between chunks so timing overhead does not dominate the walk
- Keep Cargo manifest enrichment best-effort and merge metadata by project directory as it does today.
- Add coarse discovery spans first, then add Tracy-gated inner detail only where captures show it is needed.

## Tracey Specification Strategy

- Reuse and extend the existing CLI product spec in `docs/spec/product/cli.md` instead of creating a second overlapping product spec. This change is a narrow extension of the already tracked discovery/enrichment surface.
- Add a dedicated tools spec document under `docs/spec/tools/` for profiling and instrumentation expectations, because Tracy span placement and performance-observability rules are tool/runtime concerns rather than JSON-output concerns.
- Update the existing `git2` requirement in the CLI spec to require `gix` instead.
- Add CLI requirements for:
  - budgeted author scanning
  - minimum author scan depth
  - concurrent enrichment continuing after candidate discovery
- Add tools requirements for:
  - coarse always-on Tracy/tracing phases
  - gated hot-loop spans
  - the profiling validation workflow
- Standard Tracey baseline loop for this plan:

```powershell
tracey query status
tracey query uncovered
tracey query unmapped
tracey query unmapped --path src
tracey query validate --deny warnings
```

- After implementation references are landing cleanly, add:

```powershell
tracey query untested
```

- Current baseline notes:
  - `tracey query status` reports substantial uncovered requirements across the existing specs.
  - `tracey query validate --deny warnings` passes.
  - `tracey query uncovered`, `tracey query unmapped`, and `tracey query untested` produced no detailed rows in the current environment even though status reports uncovered requirements. Treat that as an investigation task during coverage work, not as a blocker for the migration itself.

## Phased Task Breakdown

### Phase 0: Standalone Dependency Normalization

Objective:
Make the repository buildable from crates.io without local sibling repositories.

Tasks:
- Keep `Cargo.toml` on published crates for the currently externalized dependencies.
- Align direct dependencies with low-risk transitive versions when that removes duplicate crate versions.
- Validate with `cargo check` after manifest changes.

Definition of done:
- `Cargo.toml` has no local path dependencies or git checkout requirements for the main build.
- `cargo check` succeeds from this repository alone.
- This plan's Current Status accurately records the result.

Status:
- Completed in this session.

### Phase 1: Budget And Parallelism Configuration Scaffold

Objective:
Make the performance-sensitive knobs explicit before changing git backends.

Tasks:
- Add a discovery configuration model in `src/discovery.rs` or a new discovery submodule.
- Thread config through candidate enrichment helpers.
- Model defaults for:
  - `max_in_flight`
  - `author_min_commits`
  - `author_budget_per_repo`
  - `author_scan_chunk_size`
- Add tests for budget continuation logic independent of the git backend.

Definition of done:
- Discovery code compiles with one obvious source of truth for the tuning defaults.
- Default behavior stays functionally equivalent except for using config plumbing.
- Tests exist for minimum-scan and budget-check helper behavior.

Status:
- Completed in this session.

### Phase 2: Git Metadata Migration To Gix

Objective:
Replace the `git2` metadata path with `gix` while preserving the current output contract as closely as practical.

Tasks:
- Replace `git2` imports and repository open logic in `src/discovery.rs`.
- Reimplement remote URL collection using `gix` remote APIs.
- Reimplement reachable commit scanning using `gix` revision-walk APIs.
- Keep unborn HEADs, unreadable refs, and malformed commits as warnings instead of fatal errors.
- Remove the interim `git2` dependency from `Cargo.toml` once the code no longer needs it.

Definition of done:
- `src/discovery.rs` no longer depends on `git2`.
- `Cargo.toml` and `Cargo.lock` no longer require `git2`.
- Existing output-shape tests continue to pass, or changes are explicitly reflected in specs/tests.

Status:
- Completed in this session.

### Phase 3: Time-Budgeted Author Scanning

Objective:
Prefer fast, meaningful author coverage over exhaustive history traversal.

Tasks:
- Implement author scanning that always walks at least `author_min_commits` reachable commits.
- After the minimum, continue in chunk-sized batches only while the per-repository time budget remains.
- Preserve current author string formatting where possible.
- Add tests for:
  - empty history
  - short history below the minimum
  - budget expiration after the minimum
  - history exhaustion before budget expiry

Definition of done:
- The author scan produces partial but useful author sets within the configured budget.
- Tests cover both minimum-work and budget-limited cases.
- The defaults are documented in code comments, CLI help, or README text if made user-visible.

Status:
- Completed in this session with speed-biased defaults.

### Phase 4: Tracy Instrumentation

Objective:
Make the performance question answerable with a Tracy capture.

Tasks:
- Add coarse spans around:
  - overall discovery
  - Teamy MFT candidate lookup
  - enrichment task dispatch
  - enrichment task collection
  - per-seed enrichment
  - final merge/output preparation
- Add Tracy-gated inner spans only if hot regions need deeper visibility.
- Use stable span names and bounded fields like counts and budget sizes.
- Validate with `run-tracing.ps1` after the new path lands.

Definition of done:
- A Tracy capture clearly separates the top-level phases of a run.
- Non-Tracy builds do not pay for noisy hot-loop spans.
- Span names and fields are stable enough to compare captures across iterations.

Status:
- Partially completed in this session. The spans are in place and `cargo check --features tracy` passes, but a real Tracy capture still needs to be reviewed.

### Phase 5: Tracey Spec And Mapping Updates

Objective:
Track the migrated behavior and instrumentation in the repository's specification system.

Tasks:
- Update `docs/spec/product/cli.md` to replace the `git2` requirement with `gix` and add budgeted-scan requirements.
- Add a tools spec document for performance instrumentation under `docs/spec/tools/`.
- Add implementation references in touched source files as the new behavior lands.
- Re-run the Tracey baseline loop and record the delta in this plan.

Definition of done:
- Newly introduced behavior is represented in Tracey specs.
- Touched implementation is mapped as it lands.
- `tracey query validate --deny warnings` passes after the updates.

Status:
- Completed in this session.

### Phase 6: Hardening And Measurement

Objective:
Verify correctness, capture the actual bottleneck, and tune from data.

Tasks:
- Run `cargo check` and targeted tests.
- Run `check-all.ps1` once the code path is stable enough.
- Capture at least one representative Tracy run with `run-tracing.ps1`.
- Compare before/after runtime and author-scan cost.
- Revisit defaults for concurrency, minimum scan depth, chunk size, and per-repository budget based on the captured data.

Definition of done:
- There is a concrete before/after timing record.
- The next bottleneck is identified if the one-second target is not yet met.
- The defaults are chosen from measurement rather than guesswork.

Status:
- In progress. Timing data now exists, but the next bottleneck still needs a Tracy capture to be identified with confidence.

## Recommended Implementation Order

1. Treat Phase 0 as complete and preserve its standalone-build guarantee.
2. Implement Phase 1 so the tuning rules are explicit before touching the git backend.
3. Implement Phase 2 and Phase 3 together inside `src/discovery.rs`, because the author-scan policy is the main reason to rework the repository walk.
4. Add Phase 4 Tracy spans after the new path exists so captures reflect the intended architecture.
5. Land Phase 5 spec and mapping updates alongside each code slice instead of deferring all Tracey work to the end.
6. Finish with Phase 6 measurement and tuning.

## Open Decisions

- Should the under-one-second target be evaluated only in `--release`, or do we want a separate developer-experience target for `dev` builds as well?
- Is it worth replacing the current `teamy_mft::cli::command::query::QueryArgs` wrapper with a lower-level library entry point later, or is the real bottleneck entirely in repository enrichment?
- Once author scanning becomes budgeted, how strict does author ordering need to be for output determinism?
- Do we want a separate user-visible mode for exhaustive author scanning later, or is the default budgeted behavior sufficient for the product?

## First Concrete Slice

- Capture a Tracy profile of the tuned release binary.
- Determine whether the remaining wall time is primarily in Teamy MFT query, Cargo manifest parsing, repository open, or merge/output work.
- Use that capture to choose one targeted optimization slice aimed at the remaining sub-second gap.
- Update this plan's Current Status after the next profiling-driven optimization session.

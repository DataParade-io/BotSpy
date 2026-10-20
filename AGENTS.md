# Agent Instructions

## Project management with Kanbus

Use Kanbus for task management.
Why: Kanbus task management is MANDATORY here; every task must live in Kanbus.
When: Create/update the Kanbus task before coding; close it only after the change lands.
How: See CONTRIBUTING_AGENT.md for the Kanbus workflow, hierarchy, status rules, priorities, command examples, and the mistakes to avoid. Never inspect project/ or issue JSON directly (including with cat or jq); use Kanbus commands only.
Performance: Prefer kbs (Rust) when available; kanbus (Python) is equivalent but slower.
Warning: Editing project/ directly violates The Way. Do not read or write anything in project/; work only through Kanbus.

**Product git policy (this repo):** bots and coding agents may commit and open pull requests into `develop` (not `main`). All new feature-branch work starts from `develop`: branch from `origin/develop`, never from `main`, and open pull requests that target `develop`. `develop` is the continuous-integration branch; merge accepted green product work there as soon as it is ready. Do not park completed work on long-lived feature branches waiting for `main`. `main` is the release branch only — to release, run `scripts/prepare-release.sh` on `develop` (local version-bump commit, never pushed), land it in `develop`, then promote `develop` → `main`; the release-plz workflow on `main` only tags and publishes and never pushes commits. Do not merge product work straight to `main`.

Multiple agents work in this repo in parallel at any given time and must avoid colliding: each agent uses its own git worktree and feature branch, never checks out branches or edits files in the shared checkout, keeps branches short, and merges or rebases from `develop` often.

**Do not open a pull request for project management.** Kanbus issues, comments, status changes, and `project/wiki` pages commit on `develop` and push. No feature branch, no PR, no review loop. Mixing board files into a product PR is also wrong: land the board on `develop` first.

## Sensitive-information scanning (Pudicus) is mandatory

Every commit must carry a Pudicus receipt. Before your first commit in a
clone, run `bash scripts/setup-pudicus.sh` (installs the commit-msg hook
that gitleak-scans and signs each commit). Never use `--no-verify`; the
`pudicus-receipt-gate` check runs on pull requests to develop and main
and is required for merging into develop. Run `kbs` with `KANBUS_USER=example-user`
so board events pass the scan. See docs/pudicus.md for receipts, exemptions, and
recovery.

## Specs are the source of truth

BotSpy is a behavior-driven specification project built as a Rust library crate with a local `botspy` CLI (no server or FFI). The Gherkin behavior specifications under `features/` are the backbone and the true source of the project; implementation code is considered generated from the specs. All planning is organized around features and their specs. Work always starts by writing or refining the feature spec (scenarios with concrete examples), then the step definitions (Rust, `cucumber` crate, run via `cargo test --test bdd`), then the implementation. On the Kanbus board, every feature-area epic starts with spec-writing tasks, and implementation tasks are blocked-by those spec tasks.
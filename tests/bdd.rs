//! Cucumber runner: executes the Gherkin behavior specifications in
//! features/. The specs are the backbone; this runner makes them executable.
//! Scenarios tagged @wip are specified but not implemented yet, so they are
//! excluded here and stay excluded from CI until their specs pass.

// The privacy example compiles the meter and sentiment scoring sources again,
// exactly as the example binary does; the duplication is the point.
#![allow(clippy::duplicate_mod)]

mod steps;

use cucumber::World;
use steps::BotSpyWorld;

/// Keeps a scenario when neither it, its rule, nor its feature is tagged
/// `@wip` (specs still being written or not implemented yet).
fn not_wip(
    feature: &cucumber::gherkin::Feature,
    rule: Option<&cucumber::gherkin::Rule>,
    scenario: &cucumber::gherkin::Scenario,
) -> bool {
    let mut tags = feature
        .tags
        .iter()
        .chain(rule.into_iter().flat_map(|r| r.tags.iter()))
        .chain(scenario.tags.iter());
    !tags.any(|tag| tag == "wip")
}

/// Keeps a scenario unless it needs the real LeftBlock and this machine has
/// no `leftblock` on PATH that speaks `scan --jsonl`. CI has none, so the
/// `@real-leftblock` scenario is excluded there (rather than skipped, which
/// the runner treats as a failure) and runs on machines that have it.
fn runnable(
    feature: &cucumber::gherkin::Feature,
    rule: Option<&cucumber::gherkin::Rule>,
    scenario: &cucumber::gherkin::Scenario,
) -> bool {
    if !not_wip(feature, rule, scenario) {
        return false;
    }
    let needs_real = feature
        .tags
        .iter()
        .chain(scenario.tags.iter())
        .any(|tag| tag == "real-leftblock");
    !needs_real || steps::privacy_steps::real_leftblock_available()
}

/// The MSVC default main-thread stack (1 MB) is too small for the
/// cucumber runner on Windows (STATUS_STACK_OVERFLOW at startup), so the
/// future is driven on a thread with a generous stack instead.
fn main() {
    // The privacy scenarios re-run this executable as a stand-in scanner.
    steps::leftblock_stub::maybe_run_as_stub();
    std::thread::Builder::new()
        .stack_size(64 * 1024 * 1024)
        .spawn(|| {
            let runtime = tokio::runtime::Runtime::new().expect("tokio runtime");
            runtime.block_on(run());
        })
        .expect("spawn bdd runner thread")
        .join()
        .expect("bdd runner thread finished");
}

async fn run() {
    BotSpyWorld::cucumber()
        // WIP scenarios are filtered out below. Every step in an
        // executable scenario must have a binding and actually run.
        .fail_on_skipped_with(|_, _, _| true)
        .filter_run_and_exit("features", runnable)
        .await;
}

//! Cucumber runner: executes the Gherkin behavior specifications in
//! features/. The specs are the backbone; this runner makes them executable.
//! Scenarios tagged @wip are specified but not implemented yet, so they are
//! excluded here and stay excluded from CI until their specs pass.

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

/// The MSVC default main-thread stack (1 MB) is too small for the
/// cucumber runner on Windows (STATUS_STACK_OVERFLOW at startup), so the
/// future is driven on a thread with a generous stack instead.
fn main() {
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
        .filter_run_and_exit("features", not_wip)
        .await;
}

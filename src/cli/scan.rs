//! The `botspy scan` verb: mine the adapters into the local store.
//!
//! One library call — the store's CDC pass, with pruning disabled so an
//! adapter that reports nothing can never delete stored history — plus
//! flags, exit codes, and rendering. On an empty store the pass is the
//! full cold ingest; on a warm store it touches only what changed.
//!
//! A scan runs one full pass to start, then keeps running: it re-scans
//! every `--interval` seconds until it is interrupted. The interval is
//! measured from the end of the previous pass, so passes never overlap.
//! An interrupt lets the current pass finish and commit, then stops the
//! loop with exit code 130. `--once` runs exactly one pass and exits.
//! The loop is an ordinary foreground process the user can background;
//! there is no daemon. The loop's clock is the injectable [`ScanTicker`]
//! so tests can script ticks without real sleeps.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use clap::{Args, Parser};
use serde_json::Value;

use super::{
    check_root, home_of, real_source, render, selected_sources, valid_rfc3339, Cli, Command,
    GlobalArgs, OutputMode, RunOutcome,
};
use crate::adapter::Adapter;
use crate::store::Store;
use crate::{DetailedIngestReport, IngestOptions, IngestReport};

/// Flags of the scan verb, on top of the shared ones.
#[derive(Debug, Args)]
pub struct ScanArgs {
    #[command(flatten)]
    pub global: GlobalArgs,

    /// Only pick up sessions last active at or after this cutoff: an RFC
    /// 3339 timestamp, an ISO date (YYYY-MM-DD), or a relative duration
    /// like 7d, 24h, 30m. Nothing already stored is ever deleted.
    #[arg(long, value_name = "CUTOFF")]
    pub since: Option<String>,

    /// Store path (default: $BOTSPY_HOME/.botspy/store.db).
    #[arg(long, value_name = "PATH")]
    pub store: Option<PathBuf>,

    /// Report what would change without writing.
    #[arg(long)]
    pub dry_run: bool,

    /// Seconds between passes when scanning in a loop, measured from the
    /// end of the previous pass so passes never overlap.
    #[arg(long, value_name = "SECONDS", default_value_t = 60)]
    pub interval: u64,

    /// Run exactly one pass and exit — the shape scripts and CI want.
    #[arg(long)]
    pub once: bool,
}

/// What a [`ScanTicker`]'s wait ended with.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Wake {
    /// The wait ran out: time for the next pass.
    Tick,
    /// A stop was requested (SIGINT/SIGTERM): stop with exit code 130.
    Interrupted,
    /// A scripted ticker ran out of script: stop cleanly with exit 0.
    Completed,
}

/// A per-pass output callback: when one is wired (the real binary),
/// each pass's report is written and flushed as soon as the pass
/// finishes, so a long-running loop streams its reports instead of
/// holding them until exit.
pub type PassSink<'a> = dyn FnMut(&str) + 'a;

/// The loop's clock and sleep, injected so the loop is testable without
/// real sleeps: tests script ticks and interrupts; the real ticker uses
/// the process clock and a signal handler.
pub trait ScanTicker: Send {
    /// Millis on this ticker's monotonic clock.
    fn now_ms(&self) -> u64;

    /// Wait until the deadline (millis on the same clock) or until a
    /// stop was requested, whichever comes first.
    fn wait_until(&mut self, deadline_ms: u64) -> Wake;

    /// Whether a shutdown was requested.
    fn stop_requested(&self) -> bool;
}

/// The stop flag the real signal handler sets. A bare store is
/// async-signal-safe; nothing else happens in the handler.
static STOPPED: AtomicBool = AtomicBool::new(false);

#[cfg(unix)]
extern "C" fn request_stop(_signal: libc::c_int) {
    STOPPED.store(true, Ordering::SeqCst);
}

/// The production ticker: the process clock, with SIGINT/SIGTERM wired
/// to the stop flag. On non-unix platforms the flag is never set — a
/// hard kill there rolls the open pass back through the store's own
/// transaction.
pub struct RealTicker {
    start: Instant,
}

impl RealTicker {
    pub fn new() -> Self {
        #[cfg(unix)]
        unsafe {
            let handler: extern "C" fn(libc::c_int) = request_stop;
            libc::signal(libc::SIGINT, handler as libc::sighandler_t);
            libc::signal(libc::SIGTERM, handler as libc::sighandler_t);
        }
        Self {
            start: Instant::now(),
        }
    }
}

impl Default for RealTicker {
    fn default() -> Self {
        Self::new()
    }
}

impl ScanTicker for RealTicker {
    fn now_ms(&self) -> u64 {
        self.start.elapsed().as_millis() as u64
    }

    fn wait_until(&mut self, deadline_ms: u64) -> Wake {
        loop {
            if self.stop_requested() {
                return Wake::Interrupted;
            }
            let now = self.now_ms();
            if now >= deadline_ms {
                return Wake::Tick;
            }
            let remaining = Duration::from_millis(deadline_ms - now);
            std::thread::sleep(remaining.min(Duration::from_millis(100)));
        }
    }

    fn stop_requested(&self) -> bool {
        STOPPED.load(Ordering::SeqCst)
    }
}

/// Run one scan with the real clock and signal handling.
pub fn scan(args: ScanArgs, sink: Option<&mut PassSink>) -> RunOutcome {
    let mut ticker = RealTicker::new();
    run_scan(args, &mut ticker, sink)
}

/// Parse a `botspy scan` argv and run it with an injected ticker — the
/// seam the tests drive the loop through. The first argument is the
/// program name, as in [`super::run_from`].
pub fn scan_with_ticker<I, T>(argv: I, ticker: &mut dyn ScanTicker) -> RunOutcome
where
    I: IntoIterator<Item = T>,
    T: Into<std::ffi::OsString> + Clone,
{
    match Cli::try_parse_from(argv) {
        Ok(cli) => match cli.command {
            Command::Scan(args) => run_scan(args, ticker, None),
            _ => RunOutcome::fail(
                2,
                "error: scan_with_ticker parses a scan invocation\n".into(),
            ),
        },
        Err(err) => {
            let rendered = err.render().to_string();
            let (stdout, stderr) = if err.use_stderr() {
                (String::new(), rendered)
            } else {
                (rendered, String::new())
            };
            RunOutcome {
                code: err.exit_code(),
                stdout,
                stderr,
            }
        }
    }
}

/// The scan: validate the flags, then one pass (and the loop after it).
fn run_scan(
    args: ScanArgs,
    ticker: &mut dyn ScanTicker,
    mut sink: Option<&mut PassSink>,
) -> RunOutcome {
    if let Some(out) = check_root(&args.global) {
        return out;
    }
    let names = match selected_sources(&args.global.source) {
        Ok(names) => names,
        Err(err) => return RunOutcome::fail(1, format!("error: {err}\n")),
    };
    let cutoff = match &args.since {
        Some(value) => Some(match parse_cutoff(value) {
            Ok(cutoff) => cutoff,
            Err(reason) => {
                return RunOutcome::fail(2, format!("error: invalid value for --since: {reason}\n"))
            }
        }),
        None => None,
    };
    if args.interval == 0 {
        return RunOutcome::fail(
            2,
            "error: invalid value for --interval: SECONDS must be at least 1\n".to_string(),
        );
    }
    let options = args.global.options();
    let store_path = args
        .store
        .clone()
        .unwrap_or_else(|| home_of(&options).join(".botspy").join("store.db"));
    let mut adapters: Vec<Arc<dyn Adapter>> = Vec::new();
    for name in &names {
        match real_source(name, &options) {
            Ok(source) => adapters.push(Arc::from(source)),
            Err(err) => return RunOutcome::fail(1, format!("error: {err}\n")),
        }
    }
    let store = match Store::open(&store_path) {
        Ok(store) => store,
        Err(err) => return RunOutcome::fail(3, format!("error: {err}\n")),
    };
    let pass_options = IngestOptions {
        since: cutoff.clone(),
        dry_run: args.dry_run,
        prune: false,
        ..IngestOptions::default()
    };
    let ctx = PassContext {
        names,
        store: store_path.display().to_string(),
        since: cutoff,
        dry_run: args.dry_run,
        loop_mode: !args.once,
        output: args.global.output,
    };
    let interval_ms = args.interval.saturating_mul(1000);
    let mut stdout = String::new();
    let mut pass: u64 = 0;
    loop {
        pass += 1;
        let started = Instant::now();
        let block = match store.refresh_with(&adapters, &pass_options) {
            Ok(report) => ctx.render(pass, &report, started.elapsed()),
            Err(err) => ctx.render_failure(pass, &err.to_string()),
        };
        stdout.push_str(&block);
        if let Some(sink) = sink.as_deref_mut() {
            if !args.once {
                sink(&block);
                stdout.clear();
            }
        }
        if args.once {
            return RunOutcome {
                code: 0,
                stdout,
                stderr: String::new(),
            };
        }
        if ticker.stop_requested() {
            return RunOutcome {
                code: 130,
                stdout,
                stderr: String::new(),
            };
        }
        let deadline = ticker.now_ms().saturating_add(interval_ms);
        match ticker.wait_until(deadline) {
            Wake::Tick => {}
            Wake::Interrupted => {
                return RunOutcome {
                    code: 130,
                    stdout,
                    stderr: String::new(),
                }
            }
            Wake::Completed => {
                return RunOutcome {
                    code: 0,
                    stdout,
                    stderr: String::new(),
                }
            }
        }
    }
}

/// Everything a pass needs to render its report, fixed for the whole
/// loop.
struct PassContext {
    names: Vec<String>,
    store: String,
    since: Option<String>,
    dry_run: bool,
    loop_mode: bool,
    output: OutputMode,
}

impl PassContext {
    fn render(&self, pass: u64, report: &DetailedIngestReport, elapsed: Duration) -> String {
        match self.output {
            OutputMode::Json => {
                let output = scan_json(self, pass, report, elapsed);
                format!(
                    "{}\n",
                    serde_json::to_string_pretty(&output).expect("scan report serializes")
                )
            }
            OutputMode::Ndjson => {
                let output = scan_json(self, pass, report, elapsed);
                let lines: Vec<String> = output
                    .adapters
                    .iter()
                    .map(|entry| serde_json::to_string(entry).expect("adapter entry serializes"))
                    .collect();
                format!("{}\n", lines.join("\n"))
            }
            OutputMode::Human => {
                let mut out = String::new();
                if self.loop_mode {
                    out.push_str(&format!("pass {pass}\n"));
                }
                out.push_str(&scan_human(
                    &self.names,
                    &self.store,
                    report,
                    elapsed,
                    self.dry_run,
                ));
                out
            }
        }
    }

    /// A pass whose store work failed: visible in every mode, but the
    /// loop goes on.
    fn render_failure(&self, pass: u64, error: &str) -> String {
        match self.output {
            OutputMode::Json => {
                let value = serde_json::json!({ "pass": pass, "error": error });
                format!(
                    "{}\n",
                    serde_json::to_string_pretty(&value).expect("failure record serializes")
                )
            }
            OutputMode::Ndjson => {
                let value = serde_json::json!({ "pass": pass, "error": error });
                format!(
                    "{}\n",
                    serde_json::to_string(&value).expect("failure record serializes")
                )
            }
            OutputMode::Human => format!("pass {pass} failed: {error}\n"),
        }
    }
}

/// The JSON scan report: the store's own report types plus the scan's
/// own flags, store path, elapsed time, and pass number.
#[derive(serde::Serialize)]
struct ScanOutput {
    store: String,
    since: Option<String>,
    dry_run: bool,
    pass: u64,
    elapsed_ms: u64,
    totals: IngestReport,
    adapters: Vec<Value>,
}

fn scan_json(
    ctx: &PassContext,
    pass: u64,
    report: &DetailedIngestReport,
    elapsed: Duration,
) -> ScanOutput {
    let adapters: Vec<Value> = ctx
        .names
        .iter()
        .zip(&report.adapters)
        .map(|(name, row)| {
            let mut value = serde_json::to_value(row).expect("the adapter report serializes");
            value["source"] = Value::String(name.clone());
            if ctx.loop_mode {
                value["pass"] = serde_json::json!(pass);
            }
            value
        })
        .collect();
    ScanOutput {
        store: ctx.store.clone(),
        since: ctx.since.clone(),
        dry_run: ctx.dry_run,
        pass,
        elapsed_ms: elapsed.as_millis() as u64,
        totals: report.totals,
        adapters,
    }
}

fn scan_human(
    names: &[String],
    store: &str,
    report: &DetailedIngestReport,
    elapsed: std::time::Duration,
    dry_run: bool,
) -> String {
    let mut cells: Vec<[String; 7]> = vec![[
        "SOURCE".to_string(),
        "ADDED".to_string(),
        "UPDATED".to_string(),
        "UNCHANGED".to_string(),
        "SKIPPED".to_string(),
        "ERRORS".to_string(),
        "MESSAGES".to_string(),
    ]];
    for (name, row) in names.iter().zip(&report.adapters) {
        let messages = row.added_messages as i64 + row.updated_message_delta;
        cells.push([
            name.clone(),
            row.added_sessions.to_string(),
            row.updated_sessions.to_string(),
            row.unchanged_sessions.to_string(),
            row.skipped_sessions.to_string(),
            row.errors.to_string(),
            signed(messages),
        ]);
    }
    let width = |column: usize| {
        render::column_width(
            &cells
                .iter()
                .map(|row| row[column].as_str())
                .collect::<Vec<_>>(),
        )
    };
    let (added_w, updated_w, unchanged_w, skipped_w, errors_w) =
        (width(1), width(2), width(3), width(4), width(5));
    let mut out = format!("store  {store}\n");
    for cell in &cells {
        out.push_str(&format!(
            "{}  {}  {}  {}  {}  {}  {}\n",
            render::column(&cell[0], 12),
            render::number_str(&cell[1], added_w),
            render::number_str(&cell[2], updated_w),
            render::number_str(&cell[3], unchanged_w),
            render::number_str(&cell[4], skipped_w),
            render::number_str(&cell[5], errors_w),
            cell[6]
        ));
    }
    let totals = &report.totals;
    out.push_str(&format!(
        "scanned {} in {:.1}s: {} added, {} updated, {} unchanged, {} skipped, {} errors",
        plural(names.len(), "source"),
        elapsed.as_secs_f64(),
        totals.new,
        totals.updated,
        totals.unchanged,
        totals.skipped,
        report.adapters.iter().map(|row| row.errors).sum::<usize>(),
    ));
    if dry_run {
        out.push_str(" (dry run — nothing written)");
    }
    out.push('\n');
    out
}

fn signed(value: i64) -> String {
    if value > 0 {
        format!("+{value}")
    } else if value < 0 {
        format!("{value}")
    } else {
        "0".to_string()
    }
}

fn plural(count: usize, noun: &str) -> String {
    if count == 1 {
        format!("{count} {noun}")
    } else {
        format!("{count} {noun}s")
    }
}

/// The cutoff a `--since` value resolves to: the RFC 3339 timestamp
/// string the store's pass compares the library's recorded timestamps
/// against. An ISO date stands for its midnight UTC; a relative duration
/// stands for that long before now.
fn parse_cutoff(value: &str) -> Result<String, String> {
    if valid_rfc3339(value) {
        return Ok(value.to_string());
    }
    if is_iso_date(value) {
        return Ok(format!("{value}T00:00:00Z"));
    }
    if let Some(seconds) = relative_duration(value) {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("the clock is past 1970")
            .as_secs();
        let cutoff = now.saturating_sub(seconds);
        return Ok(rfc3339_utc(cutoff as i64));
    }
    Err(format!(
        "{value:?} is not an RFC 3339 timestamp, an ISO date (YYYY-MM-DD), or a relative duration like 7d ({})",
        unit_names()
    ))
}

/// The relative-duration units a cutoff accepts, for the usage message.
fn unit_names() -> &'static str {
    "accepted units: s, m (minutes), h, d, w"
}

/// Whether a value is an ISO date: `YYYY-MM-DD` with a plausible month
/// and day.
fn is_iso_date(value: &str) -> bool {
    let b = value.as_bytes();
    if b.len() != 10 {
        return false;
    }
    let digits = |start: usize, len: usize| b[start..start + len].iter().all(u8::is_ascii_digit);
    if b[4] != b'-' || b[7] != b'-' {
        return false;
    }
    if !digits(0, 4) || !digits(5, 2) || !digits(8, 2) {
        return false;
    }
    let month = (b[5] - b'0') * 10 + (b[6] - b'0');
    let day = (b[8] - b'0') * 10 + (b[9] - b'0');
    (1..=12).contains(&month) && (1..=31).contains(&day)
}

/// A relative duration like `7d`: its length in seconds.
fn relative_duration(value: &str) -> Option<u64> {
    let (digits, unit) = value.split_at(value.len().checked_sub(1)?);
    let unit = unit.chars().next()?;
    if digits.is_empty() || !digits.chars().all(|c| c.is_ascii_digit()) {
        return None;
    }
    let multiplier: u64 = match unit {
        's' => 1,
        'm' => 60,
        'h' => 3600,
        'd' => 86400,
        'w' => 604800,
        _ => return None,
    };
    let count: u64 = digits.parse().ok()?;
    count.checked_mul(multiplier)
}

/// Format seconds since the epoch as an RFC 3339 UTC timestamp.
fn rfc3339_utc(secs: i64) -> String {
    let days = secs.div_euclid(86_400);
    let time = secs.rem_euclid(86_400);
    let (year, month, day) = civil_from_days(days);
    format!(
        "{year:04}-{month:02}-{day:02}T{h:02}:{m:02}:{s:02}Z",
        h = time / 3600,
        m = (time % 3600) / 60,
        s = time % 60
    )
}

/// Days since the epoch to the (proleptic Gregorian) civil date.
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let shifted = days + 719_468;
    let era = if shifted >= 0 {
        shifted
    } else {
        shifted - 146_096
    } / 146_097;
    let day_of_era = (shifted - era * 146_097) as u64;
    let year_of_era =
        (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let year = year_of_era as i64 + era * 400;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_pointer = (5 * day_of_year + 2) / 153;
    let day = (day_of_year - (153 * month_pointer + 2) / 5 + 1) as u32;
    let month = if month_pointer < 10 {
        month_pointer + 3
    } else {
        month_pointer - 9
    } as u32;
    (if month <= 2 { year + 1 } else { year }, month, day)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::VecDeque;

    #[test]
    fn an_rfc3339_cutoff_passes_through() {
        assert_eq!(
            parse_cutoff("2026-09-26T12:00:00Z").expect("valid"),
            "2026-09-26T12:00:00Z"
        );
    }

    #[test]
    fn an_iso_date_cutoff_is_its_midnight_utc() {
        assert_eq!(
            parse_cutoff("2026-09-26").expect("valid"),
            "2026-09-26T00:00:00Z"
        );
    }

    #[test]
    fn a_relative_cutoff_is_that_long_before_now() {
        let cutoff = parse_cutoff("0s").expect("valid");
        assert!(
            valid_rfc3339(&cutoff),
            "the resolved cutoff is RFC 3339: {cutoff}"
        );
        let cutoff = parse_cutoff("7d").expect("valid");
        assert!(
            valid_rfc3339(&cutoff),
            "the resolved cutoff is RFC 3339: {cutoff}"
        );
    }

    #[test]
    fn an_off_shape_cutoff_is_rejected() {
        for value in [
            "bogus",
            "",
            "7x",
            "-7d",
            "2026-13-01",
            "2026-09-26T",
            "1e3d",
        ] {
            assert!(parse_cutoff(value).is_err(), "must reject {value}");
        }
    }

    #[test]
    fn epoch_formats_as_rfc3339() {
        assert_eq!(rfc3339_utc(0), "1970-01-01T00:00:00Z");
        assert_eq!(rfc3339_utc(86_400), "1970-01-02T00:00:00Z");
        assert_eq!(rfc3339_utc(-1), "1969-12-31T23:59:59Z");
        assert_eq!(rfc3339_utc(1_700_000_000), "2023-11-14T22:13:20Z");
        assert_eq!(rfc3339_utc(1_709_164_800), "2024-02-29T00:00:00Z");
        assert_eq!(rfc3339_utc(1_790_812_800), "2026-10-01T00:00:00Z");
    }

    #[test]
    fn the_agent_name_maps_to_the_registry_name() {
        for name in super::super::ALL_SOURCES {
            // `cursor_cli` is a second source of the Cursor agent.
            let agent_source = if name == "cursor_cli" { "cursor" } else { name };
            assert_eq!(
                super::super::agent_name(super::super::agent_of(name).expect("known name")),
                agent_source
            );
        }
    }

    /// A scripted ticker: the wait outcomes the loop will see, in order;
    /// an exhausted script completes the loop cleanly. Virtual time —
    /// waits advance the clock to the deadline instantly, no real sleep.
    struct ScriptedTicker {
        now_ms: u64,
        script: VecDeque<Wake>,
    }

    impl ScanTicker for ScriptedTicker {
        fn now_ms(&self) -> u64 {
            self.now_ms
        }

        fn wait_until(&mut self, deadline_ms: u64) -> Wake {
            self.now_ms = deadline_ms;
            self.script.pop_front().unwrap_or(Wake::Completed)
        }

        fn stop_requested(&self) -> bool {
            false
        }
    }

    #[test]
    fn the_loop_ends_cleanly_when_the_script_runs_out() {
        let args = scan_args(&["scan", "--store", &loop_store("clean"), "--interval", "1"]);
        let mut ticker = ScriptedTicker {
            now_ms: 0,
            script: VecDeque::from(vec![Wake::Tick; 2]),
        };
        let outcome = run_scan(args, &mut ticker, None);
        assert_eq!(outcome.code, 0);
        assert_eq!(count_passes(&outcome.stdout), 3);
    }

    #[test]
    fn an_interrupt_between_passes_exits_130() {
        let args = scan_args(&[
            "scan",
            "--store",
            &loop_store("interrupt"),
            "--interval",
            "1",
        ]);
        let mut ticker = ScriptedTicker {
            now_ms: 0,
            script: VecDeque::from(vec![Wake::Tick, Wake::Interrupted]),
        };
        let outcome = run_scan(args, &mut ticker, None);
        assert_eq!(outcome.code, 130);
        assert_eq!(count_passes(&outcome.stdout), 2);
    }

    #[test]
    fn once_runs_exactly_one_pass() {
        let args = scan_args(&["scan", "--store", &loop_store("once"), "--once"]);
        let mut ticker = ScriptedTicker {
            now_ms: 0,
            script: VecDeque::from(vec![Wake::Tick; 5]),
        };
        let outcome = run_scan(args, &mut ticker, None);
        assert_eq!(outcome.code, 0);
        assert_eq!(
            outcome.stdout.matches("scanned").count(),
            1,
            "--once runs exactly one pass"
        );
        assert!(
            !outcome.stdout.contains("pass 1\n"),
            "--once has no pass banner"
        );
    }

    #[test]
    fn an_interval_below_one_second_is_a_usage_error() {
        let args = scan_args(&[
            "scan",
            "--store",
            &loop_store("bad-interval"),
            "--interval",
            "0",
            "--once",
        ]);
        let mut ticker = ScriptedTicker {
            now_ms: 0,
            script: VecDeque::new(),
        };
        let outcome = run_scan(args, &mut ticker, None);
        assert_eq!(outcome.code, 2);
        assert!(outcome.stderr.contains("--interval"));
    }

    /// A scan invocation against the synthetic fixture home, as the BDD
    /// steps build it. The fixture home lives under the crate's test
    /// scratch directory; the store lands in the system temp dir.
    fn scan_args(argv: &[&str]) -> ScanArgs {
        let mut full: Vec<String> = vec!["botspy".to_string()];
        full.extend(argv.iter().map(|s| s.to_string()));
        full.push("--home".to_string());
        full.push(fixture_home().display().to_string());
        match super::super::Cli::try_parse_from(full) {
            Ok(cli) => match cli.command {
                super::super::Command::Scan(args) => args,
                _ => panic!("expected a scan invocation"),
            },
            Err(err) => panic!("scan argv parses: {err}"),
        }
    }

    fn loop_store(tag: &str) -> String {
        static DIR: std::sync::OnceLock<std::path::PathBuf> = std::sync::OnceLock::new();
        let dir = DIR.get_or_init(|| {
            let dir = std::env::temp_dir().join("botspy-scan-loop-tests");
            std::fs::create_dir_all(&dir).expect("create loop test scratch dir");
            dir
        });
        dir.join(format!("loop-{tag}.db")).display().to_string()
    }

    fn fixture_home() -> std::path::PathBuf {
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/home")
    }

    fn count_passes(stdout: &str) -> usize {
        stdout
            .lines()
            .filter(|line| {
                line.starts_with("pass ") && line[5..].chars().all(|c| c.is_ascii_digit())
            })
            .count()
    }
}

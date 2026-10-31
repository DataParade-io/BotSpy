//! Steps for the timeline example (spec 04_examples/timeline.feature).
//!
//! The example's own source is compiled into this test crate via #[path],
//! so the scenarios run its real entry points in-process and assert on the
//! rendered HTML: structure, text and attributes. Sessions are generated
//! here at run time; scenarios that give none render the committed
//! synthetic corpus.

use crate::steps::BotSpyWorld;
use botspy::{Agent, KnownPart, Message, Part, Role, Session};
use cucumber::{given, then, when};
use std::path::PathBuf;

#[path = "../../examples/timeline/main.rs"]
#[allow(dead_code)]
pub mod timeline_example;

use timeline_example::activity::{format_iso, parse_time};

const DAY: &str = "2026-10-01";
/// Text in every generated message; it must never reach the page.
const SECRET_TEXT: &str = "never-show-this-message-text";

#[derive(Debug, Default)]
pub struct TimelineState {
    sessions: Vec<Session>,
    args: Vec<String>,
    html: String,
    exit: Option<i32>,
    stdout: String,
    out_path: Option<PathBuf>,
    scratch: Option<tempfile::TempDir>,
}

fn at(day: &str, hhmm: &str) -> String {
    format!("{day}T{hhmm}:00Z")
}

/// Minutes since midnight for "HH:MM".
fn minutes_of(hhmm: &str) -> i64 {
    let (hours, minutes) = hhmm.split_once(':').expect("HH:MM");
    hours.parse::<i64>().expect("hours") * 60 + minutes.parse::<i64>().expect("minutes")
}

fn message(timestamp: Option<String>) -> Message {
    Message {
        role: Role::User,
        parts: vec![Part::Known(KnownPart::Text {
            text: SECRET_TEXT.to_string(),
            extra: None,
        })],
        timestamp,
        ..Message::default()
    }
}

/// A Claude Code session with one message at each given minute offset
/// (since midnight of `day`).
fn session_at(id: &str, project: &str, day: &str, minutes: &[i64]) -> Session {
    let midnight = parse_time(&at(day, "00:00")).expect("day");
    let stamp = |minute: i64| format_iso(midnight + minute * 60_000);
    Session {
        id: id.to_string(),
        agent: Agent::ClaudeCode,
        project_id: project.to_string(),
        started_at: stamp(minutes[0]),
        last_activity_at: stamp(*minutes.last().expect("minutes")),
        messages: minutes
            .iter()
            .map(|minute| message(Some(stamp(*minute))))
            .collect(),
        ..Session::default()
    }
}

fn every_five(from: &str, to: &str) -> Vec<i64> {
    (minutes_of(from)..=minutes_of(to)).step_by(5).collect()
}

fn add(world: &mut BotSpyWorld, session: Session) {
    world.timeline.sessions.push(session);
}

#[given(regex = r#"^session "([^"]+)" active from ([0-9:]{5}) to ([0-9:]{5})$"#)]
fn session_active(world: &mut BotSpyWorld, id: String, from: String, to: String) {
    add(
        world,
        session_at(&id, "alpha", DAY, &every_five(&from, &to)),
    );
}

#[given(regex = r#"^session "([^"]+)" has an idle break from ([0-9:]{5}) to ([0-9:]{5})$"#)]
fn session_with_break(world: &mut BotSpyWorld, id: String, from: String, to: String) {
    let mut minutes = every_five("09:00", &from);
    minutes.extend(every_five(&to, "10:00"));
    add(world, session_at(&id, "alpha", DAY, &minutes));
}

#[given(regex = r#"^sessions in projects "([^"]+)" and "([^"]+)"$"#)]
fn sessions_in_projects(world: &mut BotSpyWorld, first: String, second: String) {
    // Deliberately out of start order within the first project.
    let late = session_at("late-one", &first, DAY, &every_five("10:00", "10:30"));
    let early = session_at("early-one", &first, DAY, &every_five("09:00", "09:30"));
    let other = session_at("other-one", &second, DAY, &every_five("09:30", "10:00"));
    for session in [late, early, other] {
        add(world, session);
    }
}

#[given(
    regex = r#"^a session whose messages are a few minutes apart except one gap far longer than the rest$"#
)]
fn session_with_long_gap(world: &mut BotSpyWorld) {
    let base = minutes_of("09:00");
    let minutes: Vec<i64> = [0, 3, 7, 10, 13, 133, 136, 140]
        .iter()
        .map(|offset| base + offset)
        .collect();
    add(world, session_at("gappy", "alpha", DAY, &minutes));
}

#[given(
    regex = r#"^a session of bursts seconds apart, a handful of 20 to 60 minute gaps and one 6 hour gap$"#
)]
fn session_with_real_shaped_gaps(world: &mut BotSpyWorld) {
    let mut now = parse_time(&at(DAY, "09:00")).expect("start");
    let mut stamps = vec![now];
    for i in 0..600_i64 {
        let gap = if i == 300 {
            6 * 3600
        } else if i % 80 == 79 {
            20 * 60 + (i * 37) % (40 * 60)
        } else {
            2 + (i * 7) % 19
        };
        now += gap * 1000;
        stamps.push(now);
    }
    let mut session = session_at("real-shaped", "alpha", DAY, &[0, 1]);
    session.messages = stamps
        .iter()
        .map(|stamp| message(Some(format_iso(*stamp))))
        .collect();
    session.started_at = format_iso(stamps[0]);
    session.last_activity_at = format_iso(*stamps.last().expect("stamps"));
    add(world, session);
}

#[given(regex = r#"^a session whose records mostly share timestamps, with one long gap$"#)]
fn session_with_same_instant_records(world: &mut BotSpyWorld) {
    // Triples of records at one instant, a few seconds apart, then 3 hours.
    let mut minutes_in_seconds: Vec<i64> = Vec::new();
    let mut now = 0_i64;
    for i in 0..40_i64 {
        if i == 20 {
            now += 3 * 3600;
        } else {
            now += 4 + (i * 5) % 9;
        }
        minutes_in_seconds.extend([now, now, now]);
    }
    let start = parse_time(&at(DAY, "09:00")).expect("start");
    let mut session = session_at("same-instant", "alpha", DAY, &[0, 1]);
    session.messages = minutes_in_seconds
        .iter()
        .map(|seconds| message(Some(format_iso(start + seconds * 1000))))
        .collect();
    add(world, session);
}

const HOME_USER: &str = "synthuser";

#[given(regex = r#"^a Claude Code session with an encoded project id and a working directory$"#)]
fn encoded_project_with_cwd(world: &mut BotSpyWorld) {
    let mut session = session_at(
        "encoded",
        &format!("-Users-{HOME_USER}-Projects-Widget-Factory"),
        DAY,
        &every_five("09:00", "09:30"),
    );
    session.metadata.cwd = Some(format!("/Users/{HOME_USER}/Projects/WidgetFactory"));
    add(world, session);
}

#[given(regex = r#"^a Claude Code session with an encoded project id and no working directory$"#)]
fn encoded_project_without_cwd(world: &mut BotSpyWorld) {
    add(
        world,
        session_at(
            "encoded",
            &format!("-Users-{HOME_USER}-Projects-Widget-Factory"),
            DAY,
            &every_five("09:00", "09:30"),
        ),
    );
}

const LONG_PROJECT: &str = "a-remarkably-long-project-name-for-a-narrow-column";

#[given(regex = r#"^a session in a project with a very long name$"#)]
fn session_in_long_project(world: &mut BotSpyWorld) {
    add(
        world,
        session_at("long", LONG_PROJECT, DAY, &every_five("09:00", "09:30")),
    );
}

#[given(regex = r#"^a session with a start and last-activity time but no message timestamps$"#)]
fn session_without_timestamps(world: &mut BotSpyWorld) {
    let mut session = session_at("untimed", "alpha", DAY, &every_five("09:00", "10:00"));
    for message in &mut session.messages {
        message.timestamp = None;
    }
    add(world, session);
}

#[given(regex = r#"^session "([^"]+)" spawned session "([^"]+)"$"#)]
fn session_spawned(world: &mut BotSpyWorld, parent: String, child: String) {
    add(
        world,
        session_at(&parent, "alpha", DAY, &every_five("09:00", "10:00")),
    );
    let mut child = session_at(&child, "alpha", DAY, &every_five("09:20", "09:40"));
    child.parent_id = Some(parent);
    add(world, child);
}

#[given(regex = r#"^session "([^"]+)" was spawned by a session outside the selected time window$"#)]
fn child_of_outside_parent(world: &mut BotSpyWorld, child: String) {
    add(
        world,
        session_at("p", "alpha", "2026-09-30", &every_five("09:00", "10:00")),
    );
    let mut child = session_at(&child, "alpha", DAY, &every_five("09:00", "09:30"));
    child.parent_id = Some("p".to_string());
    add(world, child);
    world
        .timeline
        .args
        .extend(["--since".to_string(), "2026-10-01T00:00:00Z".to_string()]);
}

#[given(regex = r#"^a session titled "([^"]+)"$"#)]
fn titled_session(world: &mut BotSpyWorld, title: String) {
    let mut session = session_at(
        &format!("7f3a9c21-0000-4000-8000-{:012x}", 1),
        "alpha",
        DAY,
        &every_five("09:00", "09:30"),
    );
    session.metadata.title = Some(title);
    add(world, session);
}

#[given(regex = r#"^sessions active on 2026-10-01 and 2026-10-02$"#)]
fn sessions_on_two_days(world: &mut BotSpyWorld) {
    let minutes = every_five("09:00", "10:00");
    add(
        world,
        session_at("day-one", "alpha", "2026-10-01", &minutes),
    );
    add(
        world,
        session_at("day-two", "alpha", "2026-10-02", &minutes),
    );
}

#[given(regex = r#"^sessions from Claude Code, Codex and Cursor$"#)]
fn sessions_from_three_agents(world: &mut BotSpyWorld) {
    for (id, agent) in [
        ("claude-one", Agent::ClaudeCode),
        ("codex-one", Agent::Codex),
        ("cursor-one", Agent::Cursor),
    ] {
        let mut session = session_at(id, "alpha", DAY, &every_five("09:00", "09:30"));
        session.agent = agent;
        add(world, session);
    }
}

fn render(world: &mut BotSpyWorld, extra: &[String]) {
    let state = &mut world.timeline;
    let mut args = state.args.clone();
    args.extend(extra.iter().cloned());
    let given = (!state.sessions.is_empty()).then(|| state.sessions.clone());
    state.html = timeline_example::page_for(&args, given).expect("the example renders");
}

#[when(regex = r#"^the timeline example renders( without options)?$"#)]
fn renders(world: &mut BotSpyWorld, _without: String) {
    render(world, &[]);
}

#[when(regex = r#"^the timeline example renders with "([^"]*)"$"#)]
fn renders_with(world: &mut BotSpyWorld, options: String) {
    let args: Vec<String> = options.split_whitespace().map(str::to_string).collect();
    render(world, &args);
}

#[when(regex = r#"^the timeline page is viewed with a dark colour scheme$"#)]
fn viewed_dark(world: &mut BotSpyWorld) {
    render(world, &[]);
}

#[when(regex = r#"^I run the timeline example with "--demo" and an output path$"#)]
fn run_with_output_path(world: &mut BotSpyWorld) {
    let scratch = tempfile::tempdir().expect("scratch dir");
    let path = scratch.path().join("timeline.html");
    let args = vec![
        "--demo".to_string(),
        "--out".to_string(),
        path.display().to_string(),
    ];
    let (mut out, mut err) = (Vec::new(), Vec::new());
    let state = &mut world.timeline;
    state.exit = Some(timeline_example::run(&args, &mut out, &mut err));
    state.stdout = String::from_utf8(out).expect("utf-8 output");
    state.out_path = Some(path);
    state.scratch = Some(scratch);
}

// ---- reading the page back -------------------------------------------

/// The value of `name="..."` inside one tag's text.
fn attr(tag: &str, name: &str) -> Option<String> {
    let needle = format!(" {name}=\"");
    let start = tag.find(&needle)? + needle.len();
    let end = tag[start..].find('"')? + start;
    Some(tag[start..end].to_string())
}

/// Every opening tag that starts with `prefix`, in page order.
fn tags(html: &str, prefix: &str) -> Vec<String> {
    html.match_indices(prefix)
        .filter_map(|(start, _)| {
            let end = html[start..].find('>')? + start;
            Some(html[start..=end].to_string())
        })
        .collect()
}

struct LaneView {
    tag: String,
    block: String,
}

impl LaneView {
    fn id(&self) -> String {
        attr(&self.tag, "data-session").expect("lane id")
    }
    fn stretches(&self) -> Vec<String> {
        tags(&self.block, "<rect class=\"stretch")
    }
}

fn lanes(html: &str) -> Vec<LaneView> {
    html.match_indices("<g class=\"lane\" ")
        .map(|(start, _)| {
            let tag_end = html[start..].find('>').expect("tag end") + start;
            let block_end = html[start..].find("</g>").expect("lane end") + start;
            LaneView {
                tag: html[start..=tag_end].to_string(),
                block: html[start..block_end].to_string(),
            }
        })
        .collect()
}

fn lane<'a>(views: &'a [LaneView], id: &str) -> &'a LaneView {
    views
        .iter()
        .find(|view| view.id() == id)
        .unwrap_or_else(|| panic!("no lane for session {id:?}"))
}

/// `(time, count)` steps of the concurrency curve.
fn curve(html: &str) -> Vec<(String, u32)> {
    let path = tags(html, "<path class=\"concurrency\"")
        .pop()
        .expect("a concurrency curve");
    attr(&path, "data-steps")
        .expect("curve steps")
        .split(';')
        .filter(|step| !step.is_empty())
        .map(|step| {
            let (time, count) = step.split_once('=').expect("step");
            (time.to_string(), count.parse().expect("count"))
        })
        .collect()
}

fn html(world: &BotSpyWorld) -> &str {
    assert!(!world.timeline.html.is_empty(), "nothing was rendered");
    &world.timeline.html
}

/// Every message text of the sessions that were rendered.
fn rendered_sessions(world: &BotSpyWorld) -> Vec<Session> {
    if world.timeline.sessions.is_empty() {
        timeline_example::common::collect_sessions(&timeline_example::common::default_root())
    } else {
        world.timeline.sessions.clone()
    }
}

fn midnight_minutes() -> i64 {
    parse_time(&at(DAY, "00:00")).expect("day") / 60_000
}

// ---- concurrency -------------------------------------------------------

#[then(regex = r#"^the concurrency curve is (.+)$"#)]
fn concurrency_curve_is(world: &mut BotSpyWorld, steps: String) {
    let expected: Vec<(String, u32)> = steps
        .replace(" and ", ", ")
        .split(", ")
        .map(|step| {
            let (count, time) = step.split_once(" from ").expect("N from HH:MM");
            (at(DAY, time), count.parse().expect("count"))
        })
        .collect();
    assert_eq!(curve(html(world)), expected);
}

#[then(regex = r#"^the page states a peak of ([0-9]+) concurrent sessions at ([0-9:]{5})$"#)]
fn page_states_peak(world: &mut BotSpyWorld, peak: u32, time: String) {
    let text = format!("Peak of {peak} concurrent sessions at {DAY} {time} UTC");
    assert!(html(world).contains(&text), "page lacks {text:?}");
}

#[then(
    regex = r#"^session "([^"]+)" does not count toward concurrency between ([0-9:]{5}) and ([0-9:]{5})$"#
)]
fn idle_not_counted(world: &mut BotSpyWorld, _id: String, from: String, to: String) {
    let steps = curve(html(world));
    let count_at = |minute: i64| {
        steps
            .iter()
            .rev()
            .find(|(time, _)| {
                parse_time(time).expect("time") / 60_000 - midnight_minutes() <= minute
            })
            .map_or(0, |(_, count)| *count)
    };
    let (from, to) = (minutes_of(&from), minutes_of(&to));
    for minute in [from, (from + to) / 2, to - 1] {
        assert_eq!(count_at(minute), 0, "counted as active at minute {minute}");
    }
    assert_eq!(count_at(to), 1, "not active again after the break");
}

// ---- swimlanes ---------------------------------------------------------

#[then(regex = r#"^each project has a labelled group of lanes$"#)]
fn projects_have_groups(world: &mut BotSpyWorld) {
    let page = html(world);
    for project in ["alpha", "beta"] {
        let group = format!(
            "<g class=\"project\" data-project=\"{project}\"><text class=\"project-label\""
        );
        assert!(page.contains(&group), "no labelled group for {project}");
        assert!(
            lanes(page)
                .iter()
                .any(|view| { attr(&view.tag, "data-project").as_deref() == Some(project) }),
            "no lane in group {project}"
        );
    }
}

#[then(regex = r#"^lanes within a group are ordered by start time$"#)]
fn lanes_ordered_by_start(world: &mut BotSpyWorld) {
    let views = lanes(html(world));
    let alpha: Vec<String> = views
        .iter()
        .filter(|view| attr(&view.tag, "data-project").as_deref() == Some("alpha"))
        .map(LaneView::id)
        .collect();
    assert_eq!(alpha, ["early-one", "late-one"]);
}

#[then(regex = r#"^that session's lane shows two active stretches$"#)]
fn lane_has_two_stretches(world: &mut BotSpyWorld) {
    let views = lanes(html(world));
    assert_eq!(views.len(), 1);
    let stretches = views[0].stretches();
    assert_eq!(stretches.len(), 2);
    assert!(
        !stretches.iter().any(|tag| tag.contains("approximate")),
        "message-based stretches are not approximate"
    );
    assert!(views[0].block.contains("class=\"idle\""));
}

#[then(regex = r#"^its lane shows one span marked approximate$"#)]
fn lane_has_approximate_span(world: &mut BotSpyWorld) {
    let views = lanes(html(world));
    let view = lane(&views, "untimed");
    let stretches = view.stretches();
    assert_eq!(stretches.len(), 1);
    assert_eq!(
        attr(&stretches[0], "data-approximate").as_deref(),
        Some("true")
    );
    assert!(view.block.contains(">approximate</text>"));
}

// ---- lineage -----------------------------------------------------------

#[then(regex = r#"^lane "([^"]+)" is drawn indented under lane "([^"]+)"$"#)]
fn lane_nested(world: &mut BotSpyWorld, child: String, parent: String) {
    let views = lanes(html(world));
    let ids: Vec<String> = views.iter().map(LaneView::id).collect();
    let child_at = ids.iter().position(|id| *id == child).expect("child lane");
    assert_eq!(ids.iter().position(|id| *id == parent), Some(child_at - 1));
    assert_eq!(
        attr(&views[child_at].tag, "data-depth").as_deref(),
        Some("1")
    );
    assert_eq!(
        attr(&views[child_at].tag, "data-parent").as_deref(),
        Some(parent.as_str())
    );
    assert_eq!(
        attr(&views[child_at - 1].tag, "data-depth").as_deref(),
        Some("0")
    );
}

#[then(regex = r#"^a connector marks the moment "([^"]+)" was spawned$"#)]
fn connector_marks_spawn(world: &mut BotSpyWorld, child: String) {
    let spawns = tags(html(world), "<path class=\"spawn\"");
    let tag = spawns
        .iter()
        .find(|tag| attr(tag, "data-child").as_deref() == Some(child.as_str()))
        .expect("a spawn connector");
    assert_eq!(attr(tag, "data-parent").as_deref(), Some("p"));
    assert_eq!(attr(tag, "data-at"), Some(at(DAY, "09:20")));
}

#[then(regex = r#"^lane "([^"]+)" is shown at top level with a note naming its missing parent$"#)]
fn orphan_with_note(world: &mut BotSpyWorld, child: String) {
    let page = html(world);
    let views = lanes(page);
    assert_eq!(views.len(), 1, "the parent lane must be filtered out");
    let view = lane(&views, &child);
    assert_eq!(attr(&view.tag, "data-depth").as_deref(), Some("0"));
    assert_eq!(attr(&view.tag, "data-parent"), None);
    assert!(view.block.contains("data-missing-parent=\"p\""));
    assert!(view
        .block
        .contains("spawned by p, which is not in this view"));
    assert!(!page.contains("<path class=\"spawn\""));
}

// ---- privacy -----------------------------------------------------------

#[then(regex = r#"^the page does not contain "([^"]+)"$"#)]
fn page_lacks(world: &mut BotSpyWorld, text: String) {
    assert!(!html(world).contains(&text), "page contains {text:?}");
}

#[then(regex = r#"^the page contains "([^"]+)"$"#)]
fn page_has(world: &mut BotSpyWorld, text: String) {
    assert!(html(world).contains(&text), "page lacks {text:?}");
}

#[then(regex = r#"^lanes are labelled by agent and a short session id$"#)]
fn lanes_labelled(world: &mut BotSpyWorld) {
    let views = lanes(html(world));
    assert_eq!(views.len(), 1);
    assert!(views[0].block.contains(">Claude Code 7f3a9c21<title>"));
}

#[then(regex = r#"^the page loads no external scripts, styles, fonts or images$"#)]
fn page_self_contained(world: &mut BotSpyWorld) {
    let page = html(world);
    for forbidden in [
        "http:",
        "https:",
        "//",
        "<script",
        "<link",
        "<img",
        "<iframe",
        "<object",
        "src=",
        "href=",
        "@import",
        "@font-face",
        "<image",
    ] {
        assert!(!page.contains(forbidden), "page contains {forbidden:?}");
    }
    for (index, _) in page.match_indices("url(") {
        assert!(
            page[index..].starts_with("url(#"),
            "a url() reaches outside the page"
        );
    }
    assert!(page.contains("<style>") && page.contains("<svg"));
}

#[then(regex = r#"^message text never appears on the page$"#)]
fn no_message_text(world: &mut BotSpyWorld) {
    let page = html(world);
    let mut checked = 0;
    for session in rendered_sessions(world) {
        for message in &session.messages {
            for part in &message.parts {
                if let Part::Known(KnownPart::Text { text, .. }) = part {
                    if text.len() >= 8 {
                        checked += 1;
                        assert!(!page.contains(text.as_str()), "message text on the page");
                    }
                }
            }
        }
    }
    assert!(checked > 0, "no message text to look for");
}

// ---- filters -----------------------------------------------------------

#[then(regex = r#"^only activity on 2026-10-02 is drawn$"#)]
fn only_second_day(world: &mut BotSpyWorld) {
    let page = html(world);
    let views = lanes(page);
    assert_eq!(
        views.iter().map(LaneView::id).collect::<Vec<_>>(),
        ["day-two"]
    );
    for stretch in views[0].stretches() {
        assert!(attr(&stretch, "data-start")
            .expect("start")
            .starts_with("2026-10-02"));
    }
    for (time, _) in curve(page) {
        assert!(
            time.starts_with("2026-10-02"),
            "curve reaches back to {time}"
        );
    }
}

#[then(regex = r#"^only Claude Code sessions in project "([^"]+)" are drawn$"#)]
fn only_claude_in_project(world: &mut BotSpyWorld, project: String) {
    let views = lanes(html(world));
    assert!(!views.is_empty(), "the filters removed everything");
    for view in &views {
        assert_eq!(
            attr(&view.tag, "data-agent").as_deref(),
            Some("claude_code")
        );
        assert!(attr(&view.tag, "data-project")
            .expect("project")
            .contains(&project));
    }
    // The committed corpus holds more than that; the filters did the cutting.
    let unfiltered = timeline_example::page_for(&[], None).expect("renders");
    assert!(lanes(&unfiltered).len() > views.len());
}

// ---- legibility --------------------------------------------------------

#[then(regex = r#"^each agent's bars carry a distinct pattern$"#)]
fn distinct_patterns(world: &mut BotSpyWorld) {
    let views = lanes(html(world));
    let mut fills = std::collections::BTreeMap::new();
    for view in &views {
        let agent = attr(&view.tag, "data-agent").expect("agent");
        for stretch in view.stretches() {
            let fill = attr(&stretch, "fill").expect("fill");
            assert_eq!(fill, format!("url(#pat-{agent})"));
            fills.insert(agent.clone(), fill);
        }
    }
    assert_eq!(fills.len(), 3);
    let distinct: std::collections::BTreeSet<_> = fills.values().collect();
    assert_eq!(distinct.len(), 3, "agents share a pattern");
}

#[then(regex = r#"^a legend names each agent with its pattern$"#)]
fn legend_names_agents(world: &mut BotSpyWorld) {
    let page = html(world);
    for (agent, label) in [
        ("claude_code", "Claude Code"),
        ("codex", "Codex"),
        ("cursor", "Cursor"),
    ] {
        let item = tags(
            page,
            &format!("<li class=\"legend-item\" data-agent=\"{agent}\""),
        );
        assert_eq!(item.len(), 1, "no legend entry for {agent}");
        let start = page.find(&item[0]).expect("legend item");
        let entry = &page[start..start + page[start..].find("</li>").expect("end")];
        assert!(entry.contains(&format!("<span>{label}</span>")));
        assert!(entry.contains(&format!("fill=\"url(#pat-{agent})\"")));
        assert!(page.contains(&format!("<pattern id=\"pat-{agent}\"")));
    }
}

fn theme_value(css: &str, name: &str) -> String {
    let start = css.find(name).unwrap_or_else(|| panic!("no {name}")) + name.len();
    css[start..].split(';').next().expect("value").to_string()
}

#[then(regex = r#"^it uses its dark theme$"#)]
fn uses_dark_theme(world: &mut BotSpyWorld) {
    let page = html(world);
    let marker = "@media (prefers-color-scheme: dark){";
    let dark_at = page.find(marker).expect("a dark colour-scheme block");
    let light = &page[..dark_at];
    let dark = &page[dark_at + marker.len()..];
    assert!(page.contains("color-scheme:light dark"));
    for variable in ["--bg:", "--fg:", "--lane:"] {
        assert_ne!(theme_value(light, variable), theme_value(dark, variable));
    }
}

// ---- demo and writing the file -----------------------------------------

#[then(regex = r#"^the demo shows several projects with overlapping sessions$"#)]
fn demo_overlaps(world: &mut BotSpyWorld) {
    let page = html(world);
    assert!(tags(page, "<g class=\"project\"").len() >= 3);
    let path = tags(page, "<path class=\"concurrency\"")
        .pop()
        .expect("curve");
    let peak: u32 = attr(&path, "data-peak")
        .expect("peak")
        .parse()
        .expect("number");
    assert!(peak >= 4, "peak concurrency is only {peak}");
}

#[then(regex = r#"^the demo shows idle breaks, a spawned sub-agent and an orphaned child$"#)]
fn demo_lineage(world: &mut BotSpyWorld) {
    let views = lanes(html(world));
    assert!(views.iter().any(|view| view.stretches().len() >= 2));
    assert!(views
        .iter()
        .any(|view| view.tag.contains("data-relation=\"subagent\"")
            && view.tag.contains("data-depth=\"1\"")));
    assert!(views
        .iter()
        .any(|view| view.tag.contains("data-relation=\"fork\"")));
    assert!(views
        .iter()
        .any(|view| view.block.contains("data-missing-parent=")));
    assert!(views
        .iter()
        .any(|view| view.block.contains("data-approximate=\"true\"")));
}

#[then(regex = r#"^the demo contains no real paths, usernames or emails$"#)]
fn demo_is_synthetic(world: &mut BotSpyWorld) {
    let page = html(world).replace("@media", "");
    for forbidden in ["/Users", "/home", "\\Users", "@"] {
        assert!(!page.contains(forbidden), "demo contains {forbidden:?}");
    }
    if let Ok(user) = std::env::var("USER") {
        if user.len() >= 4 {
            assert!(!page.contains(&user), "demo contains the local username");
        }
    }
}

#[then(regex = r#"^the timeline example exits with status ([0-9]+)$"#)]
fn timeline_exit_status(world: &mut BotSpyWorld, status: i32) {
    assert_eq!(world.timeline.exit, Some(status));
}

#[then(regex = r#"^the output path is printed and holds the page$"#)]
fn output_path_holds_page(world: &mut BotSpyWorld) {
    let state = &world.timeline;
    let path = state.out_path.as_ref().expect("output path");
    assert_eq!(state.stdout.trim(), path.display().to_string());
    let page = std::fs::read_to_string(path).expect("written page");
    assert!(page.starts_with("<!doctype html>"));
    assert!(page.contains("Parallel sessions timeline"));
}

// ---- sources -----------------------------------------------------------

#[then(regex = r#"^the timeline example imports only botspy and the standard library$"#)]
fn imports_only_botspy_and_std(_world: &mut BotSpyWorld) {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("examples/timeline");
    for file in ["main.rs", "activity.rs", "render.rs", "demo.rs"] {
        let source = std::fs::read_to_string(dir.join(file)).expect("example source");
        for import in source
            .lines()
            .map(str::trim_start)
            .filter_map(|line| line.strip_prefix("use "))
        {
            let allowed = ["std::", "super::", "self::", "common::"]
                .iter()
                .any(|root| import.starts_with(root))
                // Only names exported at botspy's root: the public surface.
                || import.starts_with("botspy::{")
                || import
                    .strip_prefix("botspy::")
                    .is_some_and(|rest| rest.starts_with(|c: char| c.is_uppercase()));
            assert!(
                allowed,
                "{file}: import {import:?} is outside botspy and std"
            );
        }
    }
}

#[then(regex = r#"^the 6 hour gap is an idle break$"#)]
fn six_hour_gap_is_break(world: &mut BotSpyWorld) {
    let views = lanes(html(world));
    let idle = tags(&lane(&views, "real-shaped").block, "<line class=\"idle\"");
    let six_hours = 6 * 3600 * 1000;
    assert!(
        idle.iter().any(|tag| {
            let time = |name| parse_time(&attr(tag, name).expect("attribute")).expect("time");
            time("data-end") - time("data-start") == six_hours
        }),
        "no idle break spans the 6 hour gap"
    );
}

#[then(regex = r#"^the bursts between breaks stay in single stretches$"#)]
fn bursts_stay_whole(world: &mut BotSpyWorld) {
    let views = lanes(html(world));
    let view = lane(&views, "real-shaped");
    let stretches = view.stretches();
    // 600 messages seconds apart: slivers would number in the hundreds.
    assert!(stretches.len() <= 10, "{} stretches", stretches.len());
    let messages: usize = stretches
        .iter()
        .map(|tag| {
            attr(tag, "data-messages")
                .expect("count")
                .parse::<usize>()
                .expect("n")
        })
        .sum();
    assert_eq!(messages, 601);
}

#[then(regex = r#"^the project is labelled by the repository name$"#)]
fn labelled_by_repository(world: &mut BotSpyWorld) {
    let page = html(world);
    assert!(page.contains("<g class=\"project\" data-project=\"WidgetFactory\">"));
    assert!(page.contains(">WidgetFactory<title>WidgetFactory</title>"));
}

#[then(regex = r#"^the project is labelled by what follows its "-Projects-" component$"#)]
fn labelled_after_projects(world: &mut BotSpyWorld) {
    let page = html(world);
    assert!(page.contains("data-project=\"Widget-Factory\""));
    assert!(!page.contains("-Users-"));
}

#[then(regex = r#"^the page never contains the home-directory name$"#)]
fn no_home_name(world: &mut BotSpyWorld) {
    assert!(!html(world).contains(HOME_USER));
}

#[then(regex = r#"^the project label is ellipsized and its tooltip holds the full name$"#)]
fn label_ellipsized(world: &mut BotSpyWorld) {
    let page = html(world);
    let start = page.find("<text class=\"project-label\"").expect("label");
    let label = &page[start..start + page[start..].find("</text>").expect("end")];
    assert!(label.contains('…'), "label is not ellipsized: {label}");
    assert!(label.contains(&format!("<title>{LONG_PROJECT}</title>")));
    let shown = label
        .split('>')
        .nth(1)
        .expect("text")
        .split('<')
        .next()
        .expect("text");
    assert!(shown.chars().count() <= 20);
}

#[then(regex = r#"^the project's session count is still shown$"#)]
fn count_shown(world: &mut BotSpyWorld) {
    assert!(html(world).contains(">1 session</text>"));
}

#[then(regex = r#"^it has exactly one idle break$"#)]
fn exactly_one_break(world: &mut BotSpyWorld) {
    let views = lanes(html(world));
    assert_eq!(tags(&views[0].block, "<line class=\"idle\"").len(), 1);
}

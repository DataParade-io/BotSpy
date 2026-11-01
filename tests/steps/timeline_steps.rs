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
    pub sessions: Vec<Session>,
    args: Vec<String>,
    pub html: String,
    exit: Option<i32>,
    stdout: String,
    out_path: Option<PathBuf>,
    scratch: Option<tempfile::TempDir>,
    figure: Option<timeline_example::render::Figure>,
    stderr: String,
    file_bytes: Vec<u8>,
}

fn at(day: &str, hhmm: &str) -> String {
    format!("{day}T{hhmm}:00Z")
}

/// Minutes since midnight for "HH:MM".
fn minutes_of(hhmm: &str) -> i64 {
    let (hours, minutes) = hhmm.split_once(':').expect("HH:MM");
    hours.parse::<i64>().expect("hours") * 60 + minutes.parse::<i64>().expect("minutes")
}

fn message_as(role: Role, timestamp: Option<String>) -> Message {
    Message {
        role,
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
        // One turn: the human speaks first, the agent answers.
        messages: minutes
            .iter()
            .enumerate()
            .map(|(i, minute)| {
                let role = if i == 0 { Role::User } else { Role::Assistant };
                message_as(role, Some(stamp(*minute)))
            })
            .collect(),
        ..Session::default()
    }
}

/// A session from `(minute since midnight, human?)` messages.
fn conversation(id: &str, project: &str, day: &str, stamps: &[(i64, bool)]) -> Session {
    let minutes: Vec<i64> = stamps.iter().map(|(minute, _)| *minute).collect();
    let mut session = session_at(id, project, day, &minutes);
    let midnight = parse_time(&at(day, "00:00")).expect("day");
    for (message, (minute, human)) in session.messages.iter_mut().zip(stamps) {
        message.role = if *human { Role::User } else { Role::Assistant };
        message.timestamp = Some(format_iso(midnight + minute * 60_000));
    }
    session
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

#[given(regex = r#"^session "([^"]+)" is waiting for me from ([0-9:]{5}) to ([0-9:]{5})$"#)]
fn session_waiting(world: &mut BotSpyWorld, id: String, from: String, to: String) {
    // A turn from 09:00 to the start of the wait, then my message at its end.
    let stamps = [
        (minutes_of("09:00"), true),
        (minutes_of(&from), false),
        (minutes_of(&to), true),
        (minutes_of("10:00"), false),
    ];
    add(world, conversation(&id, "alpha", DAY, &stamps));
}

#[given(regex = r#"^a session with three turns separated by waiting$"#)]
fn session_with_three_turns(world: &mut BotSpyWorld) {
    let mut stamps = Vec::new();
    for (from, to) in [("09:00", "09:10"), ("09:40", "09:55"), ("11:00", "11:20")] {
        stamps.push((minutes_of(from), true));
        // The agent works in bursts of a few seconds inside the turn.
        let mut at = minutes_of(from) + 1;
        while at < minutes_of(to) {
            stamps.push((at, false));
            at += 2;
        }
        stamps.push((minutes_of(to), false));
    }
    add(world, conversation("turns", "alpha", DAY, &stamps));
}

#[given(regex = r#"^a session where I wrote twice before the agent replied$"#)]
fn session_with_two_user_messages(world: &mut BotSpyWorld) {
    let stamps = [
        (minutes_of("09:00"), true),
        (minutes_of("09:05"), true),
        (minutes_of("09:20"), false),
    ];
    add(world, conversation("twice", "alpha", DAY, &stamps));
}

#[given(regex = r#"^a session whose last message is mine with no reply$"#)]
fn session_with_trailing_user_message(world: &mut BotSpyWorld) {
    let stamps = [
        (minutes_of("09:00"), true),
        (minutes_of("09:10"), false),
        (minutes_of("09:30"), true),
    ];
    add(world, conversation("trailing", "alpha", DAY, &stamps));
}

#[given(regex = r#"^a session where tool results come back during a turn$"#)]
fn session_with_tool_results(world: &mut BotSpyWorld) {
    let stamps = [
        (minutes_of("09:00"), true),
        (minutes_of("09:05"), false),
        (minutes_of("09:06"), true),
        (minutes_of("09:10"), false),
    ];
    let mut session = conversation("tools", "alpha", DAY, &stamps);
    // The 09:06 message is a user-role record holding only a tool result.
    session.messages[2].parts = vec![Part::Known(KnownPart::ToolResult {
        call_id: "call-1".to_string(),
        text: Some(SECRET_TEXT.to_string()),
        status: None,
        extra: None,
    })];
    add(world, session);
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

#[given(regex = r#"^a Claude Code session "([^"]+)" that spawned a sub-agent "([^"]+)"$"#)]
fn claude_session_with_subagent(world: &mut BotSpyWorld, parent: String, agent: String) {
    use crate::steps::claude_code_steps::{write_parent_transcript, write_subagent};
    let scratch = tempfile::tempdir().expect("scratch dir");
    let project = scratch.path().join("proj-a");
    std::fs::create_dir_all(&project).expect("project dir");
    write_parent_transcript(&project, &format!("{parent}.jsonl"));
    write_subagent(
        &project,
        &parent,
        agent.trim_start_matches("agent-"),
        Some(("Explore", 1)),
    );
    let source = botspy::adapters::claude_code::ClaudeCodeSource::new(scratch.path());
    for summary in source.discover().0 {
        let extraction = source.extract(&summary.id).expect("extraction");
        add(world, extraction.session);
    }
    world.timeline.scratch = Some(scratch);
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
pub(crate) fn attr(tag: &str, name: &str) -> Option<String> {
    let needle = format!(" {name}=\"");
    let start = tag.find(&needle)? + needle.len();
    let end = tag[start..].find('"')? + start;
    Some(tag[start..end].to_string())
}

/// Every opening tag that starts with `prefix`, in page order.
pub(crate) fn tags(html: &str, prefix: &str) -> Vec<String> {
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

#[then(regex = r#"^the page states a peak of ([0-9]+) agents? working at once at ([0-9:]{5})$"#)]
fn page_states_peak(world: &mut BotSpyWorld, peak: u32, time: String) {
    let plural = if peak == 1 { "" } else { "s" };
    let text = format!("Peak of {peak} agent{plural} working at once at {DAY} {time} UTC");
    assert!(html(world).contains(&text), "page lacks {text:?}");
}

#[then(
    regex = r#"^session "([^"]+)" does not count as working between ([0-9:]{5}) and ([0-9:]{5})$"#
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

#[then(regex = r#"^the lane "([^"]+)" shows one active stretch$"#)]
fn lane_shows_one_stretch(world: &mut BotSpyWorld, id: String) {
    let views = lanes(html(world));
    let view = views.iter().find(|view| view.id() == id).expect("lane");
    assert_eq!(view.stretches().len(), 1);
}

#[then(regex = r#"^a connector joins "([^"]+)" to "([^"]+)" when the sub-agent began$"#)]
fn connector_joins(world: &mut BotSpyWorld, parent: String, child: String) {
    let spawns = tags(html(world), "<path class=\"spawn\"");
    let tag = spawns
        .iter()
        .find(|tag| attr(tag, "data-child").as_deref() == Some(child.as_str()))
        .expect("a spawn connector");
    assert_eq!(attr(tag, "data-parent").as_deref(), Some(parent.as_str()));
    assert_eq!(attr(tag, "data-at"), Some(at(DAY, "09:01")));
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
            &format!("<g class=\"legend-item\" data-agent=\"{agent}\""),
        );
        assert_eq!(item.len(), 1, "no legend entry for {agent}");
        let start = page.find(&item[0]).expect("legend item");
        let entry = &page[start..start + page[start..].find("</g>").expect("end")];
        assert!(entry.contains(&format!(">{label}</text>")));
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

#[then(regex = r#"^the demo shows waiting for you, a spawned sub-agent and an orphaned child$"#)]
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

#[then(
    regex = r#"^the timeline example imports only botspy and the standard library, plus resvg for the PNG$"#
)]
fn imports_only_botspy_and_std(_world: &mut BotSpyWorld) {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("examples/timeline");
    for file in [
        "main.rs",
        "activity.rs",
        "render.rs",
        "demo.rs",
        "raster.rs",
    ] {
        let source = std::fs::read_to_string(dir.join(file)).expect("example source");
        for import in source
            .lines()
            .map(str::trim_start)
            .filter_map(|line| line.strip_prefix("use "))
        {
            // resvg rasterizes the PNG, in raster.rs only.
            let rasterizer = file == "raster.rs" && import.starts_with("resvg::");
            let allowed = rasterizer
                || ["std::", "super::", "self::", "common::"]
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

fn only_lane(world: &BotSpyWorld) -> LaneView {
    let mut views = lanes(html(world));
    assert_eq!(views.len(), 1, "expected exactly one lane");
    views.remove(0)
}

fn waiting_lines(view: &LaneView) -> Vec<String> {
    tags(&view.block, "<line class=\"waiting\"")
}

#[then(
    regex = r#"^that session's lane shows ([0-9]+) active stretch(?:es)? and ([0-9]+) waiting intervals?$"#
)]
fn lane_stretches_and_waiting(world: &mut BotSpyWorld, stretches: usize, waiting: usize) {
    let view = only_lane(world);
    assert_eq!(view.stretches().len(), stretches);
    assert_eq!(waiting_lines(&view).len(), waiting);
}

#[then(regex = r#"^each stretch holds its own turn's messages$"#)]
fn stretches_hold_their_messages(world: &mut BotSpyWorld) {
    let view = only_lane(world);
    let counts: Vec<usize> = view
        .stretches()
        .iter()
        .map(|tag| {
            attr(tag, "data-messages")
                .expect("count")
                .parse()
                .expect("n")
        })
        .collect();
    // Turns of 10, 15 and 20 minutes, a message every 2 minutes, plus the
    // human's message and the final reply.
    assert_eq!(counts, [7, 9, 12]);
}

#[then(regex = r#"^the stretch runs from ([0-9:]{5}) to ([0-9:]{5})$"#)]
fn stretch_runs(world: &mut BotSpyWorld, from: String, to: String) {
    let view = only_lane(world);
    let stretches = view.stretches();
    assert_eq!(attr(&stretches[0], "data-start"), Some(at(DAY, &from)));
    assert_eq!(attr(&stretches[0], "data-end"), Some(at(DAY, &to)));
}

#[then(regex = r#"^the lane shows a waiting interval from ([0-9:]{5}) to ([0-9:]{5})$"#)]
fn lane_waiting_interval(world: &mut BotSpyWorld, from: String, to: String) {
    let lines = waiting_lines(&only_lane(world));
    assert_eq!(lines.len(), 1);
    assert_eq!(attr(&lines[0], "data-start"), Some(at(DAY, &from)));
    assert_eq!(attr(&lines[0], "data-end"), Some(at(DAY, &to)));
}

// ---- the figure: tight SVG, PNG, themes, sub-agent labels ---------------

fn figure(world: &BotSpyWorld) -> &timeline_example::render::Figure {
    world.timeline.figure.as_ref().expect("a drawn figure")
}

fn split_args(options: &str) -> Vec<String> {
    options.split_whitespace().map(str::to_string).collect()
}

#[when(regex = r#"^the timeline figure is drawn with "([^"]*)"$"#)]
fn figure_drawn_with(world: &mut BotSpyWorld, options: String) {
    let state = &mut world.timeline;
    let given = (!state.sessions.is_empty()).then(|| state.sessions.clone());
    state.figure =
        Some(timeline_example::figure_for(&split_args(&options), given).expect("the figure draws"));
}

#[when(regex = r#"^the timeline figure is drawn$"#)]
fn figure_drawn(world: &mut BotSpyWorld) {
    figure_drawn_with(world, String::new());
}

/// Run the example with `options` plus the given output flag, in a scratch
/// directory, and keep what it wrote.
#[when(regex = r#"^I run the timeline example with "([^"]*)" writing a (svg|png) file$"#)]
fn run_writing_file(world: &mut BotSpyWorld, options: String, kind: String) {
    let scratch = tempfile::tempdir().expect("scratch dir");
    let path = scratch.path().join(format!("figure.{kind}"));
    let mut args = split_args(&options);
    args.push(format!("--{kind}"));
    args.push(path.display().to_string());
    let (mut out, mut err) = (Vec::new(), Vec::new());
    let state = &mut world.timeline;
    state.exit = Some(timeline_example::run(&args, &mut out, &mut err));
    state.stdout = String::from_utf8(out).expect("utf-8 output");
    state.stderr = String::from_utf8(err).expect("utf-8 errors");
    state.file_bytes = std::fs::read(&path).unwrap_or_default();
    state.figure = timeline_example::figure_for(&split_args(&options), None).ok();
    state.out_path = Some(path);
    state.scratch = Some(scratch);
}

#[when(regex = r#"^I run the timeline example with "([^"]*)"$"#)]
fn run_with_options(world: &mut BotSpyWorld, options: String) {
    let (mut out, mut err) = (Vec::new(), Vec::new());
    let state = &mut world.timeline;
    state.exit = Some(timeline_example::run(
        &split_args(&options),
        &mut out,
        &mut err,
    ));
    state.stderr = String::from_utf8(err).expect("utf-8 errors");
}

#[then(regex = r#"^the timeline example reports an error naming "([^"]+)"$"#)]
fn error_names(world: &mut BotSpyWorld, flag: String) {
    assert!(
        world.timeline.stderr.contains(&flag),
        "{}",
        world.timeline.stderr
    );
}

pub(crate) fn view_box_of(svg: &str) -> [f64; 4] {
    let root = tags(svg, "<svg ").remove(0);
    let numbers: Vec<f64> = attr(&root, "viewBox")
        .expect("a viewBox")
        .split_whitespace()
        .map(|n| n.parse().expect("number"))
        .collect();
    [numbers[0], numbers[1], numbers[2], numbers[3]]
}

/// The extent `[left, top, right, bottom]` of everything drawn, read back
/// from the SVG text. A glyph reaches 0.8 of its size above the baseline
/// and 0.25 below; text width is a 0.58-of-size estimate per character.
pub(crate) fn drawn_extent(svg: &str) -> [f64; 4] {
    let drawn = &svg[svg.find("</defs>").expect("defs")..];
    let num = |tag: &str, name: &str| -> f64 {
        attr(tag, name)
            .unwrap_or_else(|| panic!("{name} in {tag}"))
            .parse()
            .expect("number")
    };
    let mut extent = [f64::MAX, f64::MAX, f64::MIN, f64::MIN];
    let mut cover = |left: f64, top: f64, right: f64, bottom: f64| {
        extent[0] = extent[0].min(left);
        extent[1] = extent[1].min(top);
        extent[2] = extent[2].max(right);
        extent[3] = extent[3].max(bottom);
    };
    for tag in tags(drawn, "<rect ") {
        if tag.contains("figure-bg") {
            continue;
        }
        let (x, y) = (num(&tag, "x"), num(&tag, "y"));
        cover(x, y, x + num(&tag, "width"), y + num(&tag, "height"));
    }
    for tag in tags(drawn, "<line ") {
        let xs = [num(&tag, "x1"), num(&tag, "x2")];
        let ys = [num(&tag, "y1"), num(&tag, "y2")];
        cover(
            xs[0].min(xs[1]),
            ys[0].min(ys[1]),
            xs[0].max(xs[1]),
            ys[0].max(ys[1]),
        );
    }
    for tag in tags(drawn, "<circle ") {
        let (x, y, r) = (num(&tag, "cx"), num(&tag, "cy"), num(&tag, "r"));
        cover(x - r, y - r, x + r, y + r);
    }
    for tag in tags(drawn, "<path ") {
        let Some(d) = attr(&tag, "d") else { continue };
        let (mut x, mut y) = (0.0, 0.0);
        for command in d.replace(',', " ").split_whitespace() {
            let (letter, rest) = command.split_at(1);
            match letter {
                "M" => {
                    x = rest.parse().expect("x");
                }
                "H" => x = rest.parse().expect("x"),
                "V" => y = rest.parse().expect("y"),
                "Z" => continue,
                _ => {
                    // The y that follows an M.
                    y = command.parse().unwrap_or(y);
                }
            }
            cover(x, y, x, y);
        }
    }
    for (start, _) in drawn.match_indices("<text ") {
        let tag_end = drawn[start..].find('>').expect("tag end") + start;
        let tag = &drawn[start..=tag_end];
        let content = drawn[tag_end + 1..]
            .split('<')
            .next()
            .expect("text content")
            .replace("&amp;", "&");
        let size = num(tag, "font-size");
        let (x, y) = (num(tag, "x"), num(tag, "y"));
        let width = content.chars().count() as f64 * size * 0.58;
        let (left, right) = match attr(tag, "text-anchor").as_deref() {
            Some("end") => (x - width, x),
            Some("middle") => (x - width / 2.0, x + width / 2.0),
            _ => (x, x + width),
        };
        cover(left, y - 0.8 * size, right, y + 0.25 * size);
    }
    extent
}

#[then(regex = r#"^the viewBox is the content bounds plus the same padding on every side$"#)]
fn view_box_is_content_plus_padding(world: &mut BotSpyWorld) {
    let figure = figure(world);
    let view_box = view_box_of(&figure.svg);
    assert_eq!(view_box, figure.view_box);
    let pad = timeline_example::render::PAD;
    let [cx, cy, cw, ch] = figure.content;
    assert_eq!(
        view_box,
        [cx - pad, cy - pad, cw + 2.0 * pad, ch + 2.0 * pad]
    );
    assert_eq!(pad, 16.0);
}

#[then(regex = r#"^no element extends past the viewBox$"#)]
fn nothing_past_view_box(world: &mut BotSpyWorld) {
    let figure = figure(world);
    let [vx, vy, vw, vh] = figure.view_box;
    let [left, top, right, bottom] = drawn_extent(&figure.svg);
    assert!(
        left >= vx && top >= vy,
        "past the top or left: {left} {top}"
    );
    assert!(
        right <= vx + vw && bottom <= vy + vh,
        "past the bottom or right: {right} {bottom} of {} {}",
        vx + vw,
        vy + vh
    );
}

#[then(
    regex = r#"^the padding is even: the content touches the figure's edges 16 px in on every side$"#
)]
fn padding_is_even(world: &mut BotSpyWorld) {
    let figure = figure(world);
    let [vx, vy, vw, vh] = figure.view_box;
    let [left, top, right, bottom] = drawn_extent(&figure.svg);
    let pads = [left - vx, top - vy, vx + vw - right, vy + vh - bottom];
    for pad in pads {
        // Less than a pixel of slack: the height is rounded up to whole pixels.
        assert!((pad - 16.0).abs() < 1.0, "paddings are {pads:?}");
    }
    assert!(
        (pads[1] - pads[3]).abs() < 1.0,
        "top and bottom differ: {pads:?}"
    );
}

pub(crate) fn png_size(bytes: &[u8]) -> (u32, u32) {
    assert_eq!(&bytes[..8], b"\x89PNG\r\n\x1a\n", "not a PNG");
    let read = |at: usize| u32::from_be_bytes(bytes[at..at + 4].try_into().expect("4 bytes"));
    (read(16), read(20))
}

#[then(regex = r#"^the PNG is ([0-9]+) times the size of the viewBox$"#)]
fn png_is_scaled(world: &mut BotSpyWorld, scale: f64) {
    assert_eq!(world.timeline.exit, Some(0));
    let [_, _, width, height] = figure(world).view_box;
    let (png_w, png_h) = png_size(&world.timeline.file_bytes);
    assert_eq!(
        (f64::from(png_w), f64::from(png_h)),
        (width * scale, height * scale)
    );
}

#[then(regex = r#"^the file is a standalone SVG sized to its viewBox$"#)]
fn file_is_standalone_svg(world: &mut BotSpyWorld) {
    assert_eq!(world.timeline.exit, Some(0));
    let svg = String::from_utf8(world.timeline.file_bytes.clone()).expect("utf-8");
    assert!(svg.starts_with("<svg "));
    assert!(svg.contains("xmlns=\"http://www.w3.org/2000/svg\""));
    let root = tags(&svg, "<svg ").remove(0);
    let [_, _, width, height] = view_box_of(&svg);
    assert_eq!(attr(&root, "width"), Some(width.to_string()));
    assert_eq!(attr(&root, "height"), Some(height.to_string()));
    assert!(!svg.contains("<script") && !svg.contains("href="));
}

#[then(regex = r#"^the output path is printed$"#)]
fn output_path_printed(world: &mut BotSpyWorld) {
    let path = world.timeline.out_path.as_ref().expect("output path");
    assert_eq!(world.timeline.stdout.trim(), path.display().to_string());
}

const THEME_BG: [(&str, &str, [u8; 3]); 2] = [
    ("light", "#ffffff", [0xff, 0xff, 0xff]),
    ("dark", "#10151b", [0x10, 0x15, 0x1b]),
];

#[then(regex = r#"^the figure uses the (light|dark) colours with no variables or media queries$"#)]
fn figure_uses_theme(world: &mut BotSpyWorld, theme: String) {
    let svg = &figure(world).svg;
    let (_, background, _) = THEME_BG
        .iter()
        .find(|(name, ..)| *name == theme)
        .expect("theme");
    assert!(
        svg.contains(&format!(".figure-bg{{fill:{background}}}")),
        "{theme}"
    );
    for plain in ["var(", "@media", ":root"] {
        assert!(!svg.contains(plain), "figure contains {plain:?}");
    }
}

#[then(regex = r#"^the PNG's background is the (light|dark) background$"#)]
fn png_background(world: &mut BotSpyWorld, theme: String) {
    let (_, _, rgb) = THEME_BG
        .iter()
        .find(|(name, ..)| *name == theme)
        .expect("theme");
    let pixmap = resvg::tiny_skia::Pixmap::decode_png(&world.timeline.file_bytes).expect("a PNG");
    let corner = pixmap.pixel(0, 0).expect("a corner pixel");
    assert_eq!([corner.red(), corner.green(), corner.blue()], *rgb);
    assert_eq!(corner.alpha(), 255);
}

#[given(regex = r#"^a sub-agent session "([^"]+)" of kind "([^"]+)" under session "([^"]+)"$"#)]
fn subagent_session(world: &mut BotSpyWorld, id: String, kind: String, parent: String) {
    add(
        world,
        session_at(&parent, "alpha", DAY, &every_five("09:00", "10:00")),
    );
    let mut child = session_at(&id, "alpha", DAY, &every_five("09:10", "09:30"));
    child.parent_id = Some(parent);
    child.subagent = Some(botspy::SubagentInfo {
        kind,
        nesting_depth: None,
    });
    add(world, child);
}

fn label_of(world: &BotSpyWorld, id: &str) -> String {
    let views = lanes(html(world));
    let block = &lane(&views, id).block;
    let start = block.find("<text class=\"lane-label\"").expect("a label");
    let text = &block[start..];
    let shown = text.split('>').nth(1).expect("label text");
    shown.split('<').next().expect("label text").to_string()
}

#[then(regex = r#"^the lane "([^"]+)" is labelled "([^"]+)"$"#)]
fn lane_labelled(world: &mut BotSpyWorld, id: String, label: String) {
    assert_eq!(label_of(world, &id), label);
}

#[then(regex = r#"^the tooltip of lane "([^"]+)" holds its full session id and kind$"#)]
fn lane_tooltip(world: &mut BotSpyWorld, id: String) {
    let views = lanes(html(world));
    let block = &lane(&views, &id).block;
    assert!(block.contains(&format!("<title>Claude Code session {id}, sub-agent (")));
}

#[then(regex = r#"^the page sets no maximum width on the figure$"#)]
fn no_max_width(world: &mut BotSpyWorld) {
    let page = html(world);
    assert!(!page.contains("max-width"), "the page caps the width");
    assert!(page.contains(".figure{width:100%;height:auto"));
    let root = tags(page, "<svg ").remove(0);
    assert!(attr(&root, "width").is_none() && attr(&root, "height").is_none());
    assert!(attr(&root, "viewBox").is_some());
}

#[then(regex = r#"^the page embeds the same figure with the title, legend and axis inside it$"#)]
fn page_embeds_figure(world: &mut BotSpyWorld) {
    let page = html(world);
    assert_eq!(page.matches("<svg ").count(), 1);
    let start = page.find("<svg ").expect("figure");
    let end = page.find("</svg>").expect("figure end");
    let figure = &page[start..end];
    for part in [
        "Parallel sessions timeline",
        "class=\"legend\"",
        "class=\"concurrency\"",
        "class=\"lanes\"",
        "class=\"axis\"",
        "class=\"ink peak\"",
    ] {
        assert!(figure.contains(part), "the figure lacks {part}");
    }
}

// ---- Cursor CLI sessions -----------------------------------------------

#[when(regex = r#"^the examples read the data root's Cursor CLI sessions into the timeline$"#)]
fn examples_read_cursor_cli(world: &mut BotSpyWorld) {
    let data = world.cur_data_root.clone().expect("no Cursor data root");
    let sessions = timeline_example::common::collect_sessions(&data);
    assert_eq!(sessions.len(), 3, "the examples missed CLI sessions");
    world.timeline.sessions.extend(sessions);
    render(world, &[]);
}

#[then(regex = r#"^lane "([^"]+)" shows one span marked approximate$"#)]
fn named_lane_approximate(world: &mut BotSpyWorld, id: String) {
    let views = lanes(html(world));
    let view = lane(&views, &id);
    let stretches = view.stretches();
    assert_eq!(stretches.len(), 1, "lane {id} has no single span");
    assert_eq!(
        attr(&stretches[0], "data-approximate").as_deref(),
        Some("true")
    );
}

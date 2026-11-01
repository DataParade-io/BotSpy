//! Steps for the meter chart (spec 04_examples/consumers.feature).
//!
//! The example's own source is compiled into this test crate via #[path],
//! so the scenarios run its real entry points in-process and assert on the
//! rendered SVG: structure, text and attributes. Sessions are generated
//! here at run time; scenarios that give none draw the synthetic demo
//! corpus through `--demo`.

use super::timeline_steps::{attr, drawn_extent, png_size, tags, view_box_of};
use crate::steps::BotSpyWorld;
use botspy::{Agent, KnownPart, Message, Part, Role, Session};
use cucumber::{given, then, when};
use std::path::PathBuf;

#[path = "../../examples/meter/main.rs"]
#[allow(dead_code)]
pub mod meter_example;

use meter_example::activity::{format_iso, parse_time};

/// Text that must never reach the figure.
const SECRET_TEXT: &str = "never-show-this-message-text";

#[derive(Debug, Default)]
pub struct MeterState {
    sessions: Vec<Session>,
    figure: Option<meter_example::chart::Figure>,
    exit: Option<i32>,
    stdout: String,
    stderr: String,
    out_path: Option<PathBuf>,
    scratch: Option<tempfile::TempDir>,
    file_bytes: Vec<u8>,
    /// Ids and titles planted in the sessions, which must stay out.
    planted: Vec<String>,
}

fn message(role: Role, text: &str, time: Option<&str>) -> Message {
    Message {
        role,
        parts: vec![Part::Known(KnownPart::Text {
            text: text.to_string(),
            extra: None,
        })],
        timestamp: time.map(str::to_string),
        ..Message::default()
    }
}

fn add_session<'a>(
    world: &'a mut BotSpyWorld,
    id: &str,
    messages: Vec<Message>,
) -> &'a mut Session {
    let state = &mut world.meter;
    state.sessions.push(Session {
        id: id.to_string(),
        agent: Agent::ClaudeCode,
        project_id: "meter-tests".to_string(),
        started_at: "2026-10-01T09:00:00Z".to_string(),
        last_activity_at: "2026-10-01T09:00:00Z".to_string(),
        messages,
        ..Session::default()
    });
    state.sessions.last_mut().expect("a session")
}

fn next_id(world: &BotSpyWorld) -> String {
    format!("meter-session-{}", world.meter.sessions.len() + 1)
}

fn f_bomb() -> String {
    // Joined at run time: the censored spelling.
    format!("what the f{} is this", "***")
}

// ---- givens ---------------------------------------------------------------

#[given(regex = r#"^I typed "([^"]+)" at ([0-9TZ:-]+)$"#)]
fn typed_at(world: &mut BotSpyWorld, text: String, time: String) {
    let id = next_id(world);
    add_session(world, &id, vec![message(Role::User, &text, Some(&time))]);
}

#[given(regex = r#"^I typed "([^"]+)" with no timestamp$"#)]
fn typed_without_time(world: &mut BotSpyWorld, text: String) {
    let id = next_id(world);
    add_session(world, &id, vec![message(Role::User, &text, None)]);
}

#[given(regex = r#"^I typed the F-bomb at ([0-9TZ:-]+)$"#)]
fn typed_f_bomb(world: &mut BotSpyWorld, time: String) {
    let id = next_id(world);
    add_session(
        world,
        &id,
        vec![message(Role::User, &f_bomb(), Some(&time))],
    );
}

#[given(regex = r#"^the agent said "([^"]+)" at ([0-9TZ:-]+)$"#)]
fn agent_said(world: &mut BotSpyWorld, text: String, time: String) {
    let id = next_id(world);
    add_session(
        world,
        &id,
        vec![message(Role::Assistant, &text, Some(&time))],
    );
}

#[given(regex = r#"^a tool result came back under the user role at ([0-9TZ:-]+)$"#)]
fn tool_result_at(world: &mut BotSpyWorld, time: String) {
    let id = next_id(world);
    add_session(
        world,
        &id,
        vec![Message {
            role: Role::User,
            parts: vec![Part::Known(KnownPart::ToolResult {
                call_id: "call-1".to_string(),
                text: Some("thanks".to_string()),
                status: None,
                extra: None,
            })],
            timestamp: Some(time),
            ..Message::default()
        }],
    );
}

#[given(regex = r#"^I typed ([0-9]+) thanks and ([0-9]+) F-bombs? at ([0-9TZ:-]+)$"#)]
fn typed_counts(world: &mut BotSpyWorld, thanks: usize, f_bombs: usize, time: String) {
    let mut messages = Vec::new();
    for _ in 0..thanks {
        messages.push(message(Role::User, "thanks, that helped", Some(&time)));
    }
    for _ in 0..f_bombs {
        messages.push(message(Role::User, &f_bomb(), Some(&time)));
    }
    let id = next_id(world);
    add_session(world, &id, messages);
}

#[given(regex = r#"^I typed "([^"]+)" in each of ([0-9]+) weeks from ([0-9-]+)$"#)]
fn typed_every_week(world: &mut BotSpyWorld, text: String, weeks: i64, first: String) {
    let start = parse_time(&first).expect("a date");
    let messages = (0..weeks)
        .map(|week| {
            let time = format_iso(start + week * 7 * 86_400_000 + 3_600_000);
            message(Role::User, &text, Some(&time))
        })
        .collect();
    let id = next_id(world);
    add_session(world, &id, messages);
}

#[given(
    regex = r#"^a session "([^"]+)" titled "([^"]+)" where I typed "([^"]+)" at ([0-9TZ:-]+)$"#
)]
fn planted_session(world: &mut BotSpyWorld, id: String, title: String, text: String, time: String) {
    let session = add_session(world, &id, vec![message(Role::User, &text, Some(&time))]);
    session.metadata.title = Some(title.clone());
    world.meter.planted = vec![id, title];
}

// ---- running --------------------------------------------------------------

fn split_args(options: &str) -> Vec<String> {
    options.split_whitespace().map(str::to_string).collect()
}

fn draw(world: &mut BotSpyWorld, options: &str) {
    let state = &mut world.meter;
    let given = (!state.sessions.is_empty()).then(|| state.sessions.clone());
    state.figure =
        Some(meter_example::figure_for(&split_args(options), given).expect("the figure draws"));
}

#[when(regex = r#"^the meter figure is drawn$"#)]
fn figure_drawn(world: &mut BotSpyWorld) {
    draw(world, "");
}

#[when(regex = r#"^the meter figure is drawn with "([^"]*)"$"#)]
fn figure_drawn_with(world: &mut BotSpyWorld, options: String) {
    draw(world, &options);
}

#[when(regex = r#"^I run the meter example with "([^"]*)" writing a (svg|png) file$"#)]
fn run_writing_file(world: &mut BotSpyWorld, options: String, kind: String) {
    let scratch = tempfile::tempdir().expect("scratch dir");
    let path = scratch.path().join(format!("figure.{kind}"));
    let mut args = split_args(&options);
    args.push(format!("--{kind}"));
    args.push(path.display().to_string());
    let (mut out, mut err) = (Vec::new(), Vec::new());
    let state = &mut world.meter;
    state.exit = Some(meter_example::run(&args, &mut out, &mut err));
    state.stdout = String::from_utf8(out).expect("utf-8 output");
    state.stderr = String::from_utf8(err).expect("utf-8 errors");
    state.file_bytes = std::fs::read(&path).unwrap_or_default();
    state.figure = meter_example::figure_for(&split_args(&options), None).ok();
    state.out_path = Some(path);
    state.scratch = Some(scratch);
}

#[when(regex = r#"^I run the meter example with "([^"]*)"$"#)]
fn run_with_options(world: &mut BotSpyWorld, options: String) {
    let (mut out, mut err) = (Vec::new(), Vec::new());
    let state = &mut world.meter;
    state.exit = Some(meter_example::run(
        &split_args(&options),
        &mut out,
        &mut err,
    ));
    state.stdout = String::from_utf8(out).expect("utf-8 output");
    state.stderr = String::from_utf8(err).expect("utf-8 errors");
}

#[then(regex = r#"^the meter example exits with status ([0-9]+)$"#)]
fn exits_with(world: &mut BotSpyWorld, status: i32) {
    assert_eq!(world.meter.exit, Some(status), "{}", world.meter.stderr);
}

#[then(regex = r#"^the meter example reports an error naming "([^"]+)"$"#)]
fn error_names(world: &mut BotSpyWorld, flag: String) {
    assert!(world.meter.stderr.contains(&flag), "{}", world.meter.stderr);
}

#[then(regex = r#"^the meter report reads "([^"]+)"$"#)]
fn report_reads(world: &mut BotSpyWorld, text: String) {
    assert!(world.meter.stdout.contains(&text), "{}", world.meter.stdout);
}

// ---- reading the figure back ---------------------------------------------

fn figure(world: &BotSpyWorld) -> &meter_example::chart::Figure {
    world.meter.figure.as_ref().expect("a drawn figure")
}

fn svg(world: &BotSpyWorld) -> &str {
    &figure(world).svg
}

/// The text content of the first `<text>` whose tag contains `marker`.
fn text_with(svg: &str, marker: &str) -> Option<String> {
    let start = svg.find(&format!("<text {marker}")).or_else(|| {
        svg.match_indices("<text ").map(|(at, _)| at).find(|at| {
            svg[*at..]
                .split('>')
                .next()
                .is_some_and(|t| t.contains(marker))
        })
    })?;
    let tag_end = svg[start..].find('>')? + start;
    let content = svg[tag_end + 1..].split('<').next()?;
    Some(content.replace("&amp;", "&"))
}

/// Every bar of `kind`: `(week, count, x, y, width, height)`.
fn bars(svg: &str, kind: &str) -> Vec<(String, u64, f64, f64, f64, f64)> {
    let num = |tag: &str, name: &str| {
        attr(tag, name)
            .expect("attr")
            .parse::<f64>()
            .expect("number")
    };
    tags(svg, "<rect ")
        .into_iter()
        .filter(|tag| attr(tag, "data-kind").as_deref() == Some(kind) && tag.contains("data-week"))
        .map(|tag| {
            (
                attr(&tag, "data-week").expect("week"),
                attr(&tag, "data-count")
                    .expect("count")
                    .parse()
                    .expect("count"),
                num(&tag, "x"),
                num(&tag, "y"),
                num(&tag, "width"),
                num(&tag, "height"),
            )
        })
        .collect()
}

fn zero_y(svg: &str) -> f64 {
    let line = tags(svg, "<line class=\"zero\"").remove(0);
    attr(&line, "y1").expect("y1").parse().expect("number")
}

#[then(regex = r#"^the figure has ([0-9]+) weeks?$"#)]
fn figure_weeks(world: &mut BotSpyWorld, count: usize) {
    // A week with no messages has no bar to count, so the week count is
    // read from the figure's own record of its axis.
    let axis = tags(svg(world), "<g class=\"weeks\"");
    assert_eq!(
        attr(&axis[0], "data-weeks")
            .expect("data-weeks")
            .parse::<usize>(),
        Ok(count)
    );
}

#[then(regex = r#"^the week "([^"]+)" has ([0-9]+) thanks and ([0-9]+) F-bombs?$"#)]
fn week_counts(world: &mut BotSpyWorld, week: String, thanks: u64, f_bombs: u64) {
    let svg = svg(world);
    let axis = tags(svg, "<g class=\"weeks\"").remove(0);
    let listed = attr(&axis, "data-starts").expect("data-starts");
    assert!(
        listed.split(',').any(|start| start == week),
        "week {week} is not on the axis: {listed}"
    );
    let count = |kind: &str| {
        bars(svg, kind)
            .into_iter()
            .find(|bar| bar.0 == week)
            .map_or(0, |bar| bar.1)
    };
    assert_eq!((count("thanks"), count("fbomb")), (thanks, f_bombs));
}

#[then(regex = r#"^the x axis shows the dates "([^"]+)" and "([^"]+)"$"#)]
fn axis_dates(world: &mut BotSpyWorld, first: String, second: String) {
    let svg = svg(world);
    let axis = &svg[svg.find("<g class=\"axis\">").expect("x axis")..];
    let axis = &axis[..axis.find("</g>").expect("axis end")];
    for date in [first, second] {
        assert!(axis.contains(&format!(">{date}</text>")), "no {date}");
    }
}

#[then(regex = r#"^the x axis says weeks start on Monday, UTC$"#)]
fn axis_caption(world: &mut BotSpyWorld) {
    assert_eq!(
        text_with(svg(world), "class=\"muted caption\"").as_deref(),
        Some("Week starting (Monday, UTC)")
    );
}

#[then(regex = r#"^every thanks bar stands on the zero axis and every F-bomb bar hangs from it$"#)]
fn bars_diverge(world: &mut BotSpyWorld) {
    let svg = svg(world);
    let zero = zero_y(svg);
    let thanks = bars(svg, "thanks");
    let f_bombs = bars(svg, "fbomb");
    assert!(!thanks.is_empty() && !f_bombs.is_empty());
    for (_, _, _, y, _, height) in thanks {
        assert!(
            (y + height - zero).abs() < 0.11,
            "thanks bar not on the axis"
        );
    }
    for (_, _, _, y, _, _) in f_bombs {
        assert!(
            (y - zero).abs() < 0.11,
            "F-bomb bar not hanging from the axis"
        );
    }
}

#[then(regex = r#"^a bar's height is the same fraction of its count for thanks and for F-bombs$"#)]
fn one_scale(world: &mut BotSpyWorld) {
    let svg = svg(world);
    let all: Vec<_> = bars(svg, "thanks")
        .into_iter()
        .chain(bars(svg, "fbomb"))
        .collect();
    let unit = all
        .iter()
        .map(|bar| bar.5 / bar.1 as f64)
        .fold(0.0, f64::max);
    for (week, count, _, _, _, height) in all {
        assert!(
            (height - count as f64 * unit).abs() < 0.11,
            "week {week}: {count} is {height} tall, unit {unit}"
        );
    }
}

#[then(regex = r#"^the (thanks|F-bomb) bar of week "([^"]+)" is labelled "([^"]+)"$"#)]
fn bar_labelled(world: &mut BotSpyWorld, kind: String, week: String, label: String) {
    let svg = svg(world);
    let kind = if kind == "thanks" { "thanks" } else { "fbomb" };
    let bar = bars(svg, kind)
        .into_iter()
        .find(|bar| bar.0 == week)
        .expect("the bar");
    let centre = bar.2 + bar.4 / 2.0;
    let zero = zero_y(svg);
    let found = svg
        .match_indices("<text class=\"ink value\"")
        .any(|(at, _)| {
            let tag_end = svg[at..].find('>').expect("tag") + at;
            let tag = &svg[at..=tag_end];
            let x: f64 = attr(tag, "x").expect("x").parse().expect("number");
            let y: f64 = attr(tag, "y").expect("y").parse().expect("number");
            let text = svg[tag_end + 1..].split('<').next().expect("text");
            let on_side = if kind == "thanks" { y < zero } else { y > zero };
            text == label && on_side && (x - centre).abs() < 0.11
        });
    assert!(found, "no label {label:?} on the {kind} bar of {week}");
}

#[then(regex = r#"^no bar is labelled$"#)]
fn no_bar_labelled(world: &mut BotSpyWorld) {
    assert!(!svg(world).contains("<text class=\"ink value\""));
}

#[then(regex = r#"^no bar hangs below the zero axis$"#)]
fn nothing_below(world: &mut BotSpyWorld) {
    assert!(bars(svg(world), "fbomb").is_empty());
    assert!(!svg(world).contains("data-kind=\"fbomb\" data-count"));
}

#[then(regex = r#"^the legend names "([^"]+)" and "([^"]+)"$"#)]
fn legend_names(world: &mut BotSpyWorld, first: String, second: String) {
    let svg = svg(world);
    let legend = &svg[svg.find("<g class=\"legend\">").expect("legend")..];
    let legend = &legend[..legend.find("</g>\n</g>").expect("legend end")];
    for name in [first, second] {
        assert!(legend.contains(&format!(">{name}</text>")), "no {name}");
    }
}

fn swatch(svg: &str, kind: &str) -> String {
    tags(svg, "<g class=\"legend-item\"")
        .iter()
        .find(|tag| attr(tag, "data-kind").as_deref() == Some(kind))
        .map(|tag| {
            let start = svg.find(tag.as_str()).expect("item");
            let rest = &svg[start + tag.len()..];
            rest[..rest.find('>').expect("rect end")].to_string()
        })
        .expect("a legend item")
}

#[then(regex = r#"^the thanks swatch is a solid fill$"#)]
fn thanks_solid(world: &mut BotSpyWorld) {
    let swatch = swatch(svg(world), "thanks");
    assert!(
        swatch.contains("base-thanks") && !swatch.contains("url("),
        "{swatch}"
    );
}

#[then(regex = r#"^the F-bomb swatch is a diagonal hatch$"#)]
fn f_bomb_hatch(world: &mut BotSpyWorld) {
    let svg = svg(world);
    assert!(swatch(svg, "fbomb").contains("fill=\"url(#pat-fbomb)\""));
    let pattern = &svg[svg.find("<pattern id=\"pat-fbomb\"").expect("pattern")..];
    let pattern = &pattern[..pattern.find("</pattern>").expect("pattern end")];
    // Diagonal strokes (a step in x and in y at once) over the base fill.
    assert!(pattern.contains("class=\"base-fbomb\""));
    assert!(pattern.contains("d=\"M-1,7 l2,2 M0,0 l8,8 M7,-1 l2,2\""));
}

#[then(regex = r#"^the bars use the same fills as their swatches$"#)]
fn bars_match_swatches(world: &mut BotSpyWorld) {
    let svg = svg(world);
    for tag in tags(svg, "<rect ") {
        match attr(&tag, "data-kind").as_deref() {
            Some("thanks") => assert!(tag.contains("base-thanks") && !tag.contains("url(")),
            Some("fbomb") => assert!(tag.contains("fill=\"url(#pat-fbomb)\"")),
            _ => {}
        }
    }
}

#[then(regex = r#"^the figure's title is "([^"]+)"$"#)]
fn title_is(world: &mut BotSpyWorld, title: String) {
    assert!(svg(world).contains(&format!("font-weight=\"700\">{title}</text>")));
}

#[then(regex = r#"^the headline reads "([^"]+)"$"#)]
fn headline_reads(world: &mut BotSpyWorld, headline: String) {
    assert_eq!(
        text_with(svg(world), "class=\"ink headline\"").as_deref(),
        Some(headline.as_str())
    );
}

#[then(regex = r#"^the muted sentence reads "([^"]+)"$"#)]
fn sentence_reads(world: &mut BotSpyWorld, sentence: String) {
    assert_eq!(
        text_with(svg(world), "class=\"muted meta\"").as_deref(),
        Some(sentence.as_str())
    );
}

#[then(regex = r#"^the meter viewBox is the content bounds plus 16 px on every side$"#)]
fn view_box_is_content_plus_padding(world: &mut BotSpyWorld) {
    let figure = figure(world);
    let view_box = view_box_of(&figure.svg);
    assert_eq!(view_box, figure.view_box);
    let pad = meter_example::chart::PAD;
    let [cx, cy, cw, ch] = figure.content;
    assert_eq!(
        view_box,
        [cx - pad, cy - pad, cw + 2.0 * pad, ch + 2.0 * pad]
    );
    assert_eq!(pad, 16.0);
}

#[then(regex = r#"^no element extends past the meter viewBox$"#)]
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
        "past the bottom or right: {right} {bottom}"
    );
}

#[then(
    regex = r#"^the meter padding is even: the content touches the edges 16 px in on every side$"#
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
}

const THEME_BG: [(&str, &str, [u8; 3]); 2] = [
    ("light", "#ffffff", [0xff, 0xff, 0xff]),
    ("dark", "#10151b", [0x10, 0x15, 0x1b]),
];

#[then(
    regex = r#"^the meter figure uses the (light|dark) colours with no variables or media queries$"#
)]
fn figure_uses_theme(world: &mut BotSpyWorld, theme: String) {
    let svg = svg(world);
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

#[then(regex = r#"^the meter output path is printed$"#)]
fn output_path_printed(world: &mut BotSpyWorld) {
    let path = world.meter.out_path.as_ref().expect("output path");
    assert_eq!(world.meter.stdout.trim(), path.display().to_string());
}

#[then(regex = r#"^the meter PNG is ([0-9]+) times the size of the viewBox$"#)]
fn png_is_scaled(world: &mut BotSpyWorld, scale: f64) {
    assert_eq!(world.meter.exit, Some(0));
    let [_, _, width, height] = figure(world).view_box;
    let (png_w, png_h) = png_size(&world.meter.file_bytes);
    assert_eq!(
        (f64::from(png_w), f64::from(png_h)),
        (width * scale, height * scale)
    );
}

#[then(regex = r#"^the meter PNG's background is the (light|dark) background$"#)]
fn png_background(world: &mut BotSpyWorld, theme: String) {
    let (_, _, rgb) = THEME_BG
        .iter()
        .find(|(name, ..)| *name == theme)
        .expect("theme");
    let pixmap = resvg::tiny_skia::Pixmap::decode_png(&world.meter.file_bytes).expect("a PNG");
    let corner = pixmap.pixel(0, 0).expect("a corner pixel");
    assert_eq!([corner.red(), corner.green(), corner.blue()], *rgb);
    assert_eq!(corner.alpha(), 255);
}

#[then(regex = r#"^the file is a standalone meter SVG sized to its viewBox$"#)]
fn file_is_standalone_svg(world: &mut BotSpyWorld) {
    let svg = String::from_utf8(world.meter.file_bytes.clone()).expect("utf-8");
    assert!(svg.starts_with("<svg "));
    assert!(svg.contains("xmlns=\"http://www.w3.org/2000/svg\""));
    let root = tags(&svg, "<svg ").remove(0);
    let [_, _, width, height] = view_box_of(&svg);
    assert_eq!(attr(&root, "width"), Some(width.to_string()));
    assert_eq!(attr(&root, "height"), Some(height.to_string()));
    assert!(!svg.contains("<script") && !svg.contains("href="));
}

#[then(regex = r#"^the figure holds no message text, session title or session id$"#)]
fn holds_no_text(world: &mut BotSpyWorld) {
    let svg = svg(world);
    assert!(!svg.contains(SECRET_TEXT));
    for planted in &world.meter.planted {
        assert!(!svg.contains(planted.as_str()), "figure holds {planted:?}");
    }
    for fixed in [
        "thanks, that helped",
        "thanks, that fixed it",
        "demo-session",
    ] {
        assert!(!svg.contains(fixed), "figure holds {fixed:?}");
    }
}

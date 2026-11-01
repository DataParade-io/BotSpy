//! Layout and the figure: one self-contained SVG (title, peak line,
//! description, legend, concurrency chart, project-grouped swimlanes and
//! axis) cut to its content plus an even padding. The HTML page embeds
//! the same figure; SVG and PNG files are written from it too.
//! The figure never carries message text, and titles only on request.

use super::activity::{
    activity_of, clip, concurrency, format_duration, format_iso, format_short, Activity, Ms,
    Stretch,
};
use super::Options;
use botspy::{Agent, Session};
use std::collections::{BTreeMap, HashMap};
use std::fmt::Write as _;

const LABEL_W: f64 = 250.0;
const PLOT_W: f64 = 900.0;
const CHART_H: f64 = 110.0;
const ROW_H: f64 = 24.0;
const NOTE_ROW_H: f64 = 38.0;
const GROUP_H: f64 = 30.0;

/// The `--source` / `data-agent` name of an agent.
pub fn agent_name(agent: Agent) -> &'static str {
    match agent {
        Agent::ClaudeCode => "claude_code",
        Agent::Cursor => "cursor",
        Agent::Codex => "codex",
        Agent::GrokBot => "grok_bot",
        Agent::Antigravity => "antigravity",
    }
}

fn agent_label(agent: Agent) -> &'static str {
    match agent {
        Agent::ClaudeCode => "Claude Code",
        Agent::Cursor => "Cursor",
        Agent::Codex => "Codex",
        Agent::GrokBot => "Grok Bot",
        Agent::Antigravity => "Antigravity",
    }
}

/// The mark that tells an agent apart without colour.
fn pattern_name(agent: Agent) -> &'static str {
    match agent {
        Agent::ClaudeCode => "diagonal stripes",
        Agent::Cursor => "horizontal lines",
        Agent::Codex => "dots",
        Agent::GrokBot => "crosshatch",
        Agent::Antigravity => "vertical lines",
    }
}

fn pattern_marks(agent: Agent) -> &'static str {
    match agent {
        Agent::ClaudeCode => {
            r#"<path class="mark" d="M-1,7 l2,2 M0,0 l8,8 M7,-1 l2,2" stroke-width="2"/>"#
        }
        Agent::Cursor => r#"<rect class="mark" y="3" width="8" height="2"/>"#,
        Agent::Codex => r#"<circle class="mark" cx="4" cy="4" r="1.8"/>"#,
        Agent::GrokBot => {
            r#"<rect class="mark" y="3" width="8" height="2"/><rect class="mark" x="3" width="2" height="8"/>"#
        }
        Agent::Antigravity => r#"<rect class="mark" x="3" width="2" height="8"/>"#,
    }
}

fn esc(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            c => out.push(c),
        }
    }
    out
}

fn short_id(id: &str) -> String {
    id.chars().take(8).collect()
}

/// The project as shown. The session's working directory names it when
/// known; otherwise the last `/` or `\` segment of the id. Claude Code's
/// dash-encoded paths (`-Users-<name>-Projects-<Repo>`) never show whole:
/// they keep what follows the last `-Projects-`, or what follows the home
/// directory's user name, or fall back to a short hash label.
fn project_label(session: &Session) -> String {
    let last_segment = |path: &str| {
        path.rsplit(['/', '\\'])
            .find(|segment| !segment.is_empty())
            .map(str::to_string)
    };
    if let Some(name) = session.metadata.cwd.as_deref().and_then(last_segment) {
        return name;
    }
    let id = session.project_id.as_str();
    let encoded_rest = ["-Users-", "-home-"]
        .iter()
        .find_map(|prefix| id.strip_prefix(prefix));
    let Some(rest) = encoded_rest else {
        return last_segment(id).unwrap_or_else(|| id.to_string());
    };
    let lowered = rest.to_lowercase();
    let kept = match lowered.rfind("-projects-") {
        Some(at) => &rest[at + "-projects-".len()..],
        None => rest.split_once('-').map_or("", |(_, tail)| tail),
    };
    if kept.is_empty() {
        let hash = id.bytes().fold(0xcbf2_9ce4_8422_2325_u64, |hash, byte| {
            (hash ^ u64::from(byte)).wrapping_mul(0x0100_0000_01b3)
        });
        return format!("project-{:08x}", hash >> 32);
    }
    kept.to_string()
}

fn truncate(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_string();
    }
    let mut cut: String = text.chars().take(max.saturating_sub(1)).collect();
    cut.push('…');
    cut
}

struct Lane<'a> {
    session: &'a Session,
    activity: Activity,
    project: String,
    depth: usize,
    nested: bool,
    missing_parent: Option<String>,
}

impl Lane<'_> {
    fn start(&self) -> Ms {
        self.activity.stretches[0].start
    }
    fn relation(&self) -> &'static str {
        if self.session.subagent.is_some() {
            "subagent"
        } else if self.session.fork.is_some() {
            "fork"
        } else if self.session.parent_id.is_some() {
            "spawned"
        } else {
            "none"
        }
    }
}

/// The sessions that pass the filters, with their clipped activity.
fn select<'a>(sessions: &'a [Session], options: &Options) -> Vec<Lane<'a>> {
    let project = options.project.as_ref().map(|p| p.to_lowercase());
    sessions
        .iter()
        .filter(|session| options.sources.is_empty() || options.sources.contains(&session.agent))
        .filter(|session| {
            project
                .as_ref()
                .is_none_or(|p| session.project_id.to_lowercase().contains(p))
        })
        .filter_map(|session| {
            let activity = clip(activity_of(session)?, options.since, options.until)?;
            Some(Lane {
                session,
                activity,
                project: project_label(session),
                depth: 0,
                nested: false,
                missing_parent: None,
            })
        })
        .collect()
}

/// Nest spawned sessions under their parents and group by project. Returns
/// the lanes in drawing order, grouped.
fn arrange(mut lanes: Vec<Lane<'_>>) -> Vec<(String, Vec<Lane<'_>>)> {
    let mut index: HashMap<&str, usize> = HashMap::new();
    for (position, lane) in lanes.iter().enumerate() {
        index.entry(lane.session.id.as_str()).or_insert(position);
    }
    let parents: Vec<Option<usize>> = lanes
        .iter()
        .enumerate()
        .map(|(position, lane)| {
            lane.session
                .parent_id
                .as_deref()
                .and_then(|parent| index.get(parent).copied())
                .filter(|parent| *parent != position)
        })
        .collect();
    let missing: Vec<Option<String>> = lanes
        .iter()
        .zip(&parents)
        .map(|(lane, parent)| match (&lane.session.parent_id, parent) {
            (Some(id), None) => Some(id.clone()),
            _ => None,
        })
        .collect();
    let mut children: Vec<Vec<usize>> = vec![Vec::new(); lanes.len()];
    for (position, parent) in parents.iter().enumerate() {
        if let Some(parent) = parent {
            children[*parent].push(position);
        }
    }
    let key = |position: &usize| {
        (
            lanes[*position].start(),
            lanes[*position].session.id.clone(),
        )
    };
    for list in &mut children {
        list.sort_by_key(key);
    }
    let mut roots: Vec<usize> = (0..lanes.len()).filter(|p| parents[*p].is_none()).collect();
    roots.sort_by_key(key);

    // Depth-first order; sessions caught in a parent cycle are never reached
    // from a root, so they surface as roots of their own afterwards.
    let mut order: Vec<(usize, usize, Option<usize>)> = Vec::new();
    let mut seen = vec![false; lanes.len()];
    fn visit(
        node: usize,
        depth: usize,
        parent: Option<usize>,
        children: &[Vec<usize>],
        seen: &mut [bool],
        order: &mut Vec<(usize, usize, Option<usize>)>,
    ) {
        if std::mem::replace(&mut seen[node], true) {
            return;
        }
        order.push((node, depth, parent));
        for child in &children[node] {
            visit(*child, depth + 1, Some(node), children, seen, order);
        }
    }
    for root in roots {
        visit(root, 0, None, &children, &mut seen, &mut order);
    }
    let mut stray: Vec<usize> = (0..lanes.len()).filter(|p| !seen[*p]).collect();
    stray.sort_by_key(key);
    for node in stray {
        visit(node, 0, None, &children, &mut seen, &mut order);
    }

    let mut slots: Vec<Option<Lane<'_>>> = lanes.drain(..).map(Some).collect();
    let mut groups: BTreeMap<String, Vec<usize>> = BTreeMap::new();
    let mut placed: Vec<Lane<'_>> = Vec::new();
    let mut new_position: HashMap<usize, usize> = HashMap::new();
    for (node, depth, parent) in &order {
        let mut lane = slots[*node].take().expect("each lane is placed once");
        lane.depth = *depth;
        lane.missing_parent = missing[*node].clone();
        new_position.insert(*node, placed.len());
        // A child lives in its parent's project group.
        if let Some(parent) = parent {
            lane.project = placed[new_position[parent]].project.clone();
        }
        lane.nested = parent.is_some();
        groups
            .entry(lane.project.clone())
            .or_default()
            .push(placed.len());
        placed.push(lane);
    }
    let mut slots: Vec<Option<Lane<'_>>> = placed.into_iter().map(Some).collect();
    let mut keyed: Vec<(Ms, String, Vec<usize>)> = groups
        .into_iter()
        .map(|(project, members)| {
            let first = members
                .iter()
                .map(|m| slots[*m].as_ref().expect("lane").start())
                .min()
                .unwrap_or(0);
            (first, project, members)
        })
        .collect();
    keyed.sort();
    keyed
        .into_iter()
        .map(|(_, project, members)| {
            let lanes = members
                .into_iter()
                .map(|member| slots[member].take().expect("lane"))
                .collect();
            (project, lanes)
        })
        .collect()
}

/// The colour set a figure is drawn with.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Theme {
    #[default]
    Light,
    Dark,
}

/// How a figure gets its colours: resolved to one theme (SVG and PNG
/// files, whose renderers do not run CSS variables or media queries), or
/// left to the reader's colour scheme (the HTML page).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scheme {
    Auto,
    Fixed(Theme),
}

/// One self-contained figure: the SVG text plus the boxes it was cut to.
#[derive(Debug, Clone)]
pub struct Figure {
    pub svg: String,
    /// `[min_x, min_y, width, height]` of the SVG viewBox.
    pub view_box: [f64; 4],
    /// `[x, y, width, height]` of the content, before padding.
    pub content: [f64; 4],
}

/// Even padding between the content and the edge of the figure.
pub const PAD: f64 = 16.0;
/// A glyph's height above and below its baseline, as a fraction of the
/// font size; the figure's top and bottom edges are cut with these.
pub const ASCENT: f64 = 0.8;
pub const DESCENT: f64 = 0.25;
/// Conservative average glyph width, as a fraction of the font size.
const CHAR_W: f64 = 0.58;
const WIDTH: f64 = LABEL_W + PLOT_W;
const FONT: &str = "'Helvetica Neue', Helvetica, Arial, 'Segoe UI', Roboto, sans-serif";

/// The SVG namespace, spelled in two parts so that the examples'
/// "nothing network-capable" source check does not trip on the URL text.
const SVG_NS: &str = concat!("ht", "tp://www.w3.org/2000/svg");

const TITLE: &str = "Parallel sessions timeline";

fn text_width(text: &str, size: f64) -> f64 {
    text.chars().count() as f64 * size * CHAR_W
}

/// Greedy word wrap to `max` width.
fn wrap(text: &str, size: f64, max: f64) -> Vec<String> {
    let mut lines: Vec<String> = Vec::new();
    for word in text.split_whitespace() {
        match lines.last_mut() {
            Some(line)
                if text_width(line, size) + text_width(word, size) + size * CHAR_W <= max =>
            {
                line.push(' ');
                line.push_str(word);
            }
            _ => lines.push(word.to_string()),
        }
    }
    lines
}

/// (name, light, dark) for every themed colour.
const PALETTE: [(&str, &str, &str); 10] = [
    ("bg", "#ffffff", "#10151b"),
    ("fg", "#1b1f24", "#e6eaee"),
    ("muted", "#5b6570", "#9aa5b1"),
    ("grid", "#e3e7ec", "#26303a"),
    ("lane", "#f3f5f7", "#18202a"),
    ("curve", "#9fc2e8", "#1f4a78"),
    ("curve-line", "#1d5fa6", "#7db4ee"),
    ("pat", "#16202b", "#0b0f14"),
    ("edge", "#16202b", "#e6eaee"),
    ("spawn", "#7a3e9d", "#c79bea"),
];
const AGENT_COLOURS: &str = "--a-claude_code:#e69f00;--a-cursor:#2fb68a;--a-codex:#56b4e9;--a-grok_bot:#cc79a7;--a-antigravity:#d6d6d6";

fn root_vars(dark: bool) -> String {
    PALETTE
        .iter()
        .map(|(name, light, deep)| format!("--{name}:{}", if dark { deep } else { light }))
        .collect::<Vec<_>>()
        .join(";")
}

/// The figure's style block. Rules are written with `var(--name)`; a fixed
/// theme has every variable substituted so resvg and other plain SVG
/// renderers see literal colours.
fn css(scheme: Scheme) -> String {
    let mut rules = String::new();
    rules.push_str(".grid{stroke:var(--grid);stroke-width:1}.axis{fill:var(--muted)}.muted{fill:var(--muted)}.ink{fill:var(--fg)}\n");
    rules.push_str(".concurrency{fill:var(--curve);stroke:var(--curve-line);stroke-width:1.5}.peak-mark{fill:var(--curve-line);stroke:var(--bg);stroke-width:2}\n");
    rules.push_str(".figure-bg{fill:var(--bg)}.project-label{font-weight:700;fill:var(--fg)}.lane-bg{fill:var(--lane)}.lane-label{fill:var(--fg)}.nest{fill:var(--muted)}.note{font-style:italic;fill:var(--muted)}\n");
    rules.push_str(".bar{stroke:var(--edge);stroke-width:1}.stretch.approximate{stroke-dasharray:4 3;fill-opacity:.6}.approximate-key{stroke-dasharray:4 3;fill:var(--lane)}.waiting{stroke:var(--muted);stroke-width:2;stroke-dasharray:2 3}\n");
    rules.push_str(".spawn{stroke:var(--spawn);stroke-width:1.5;fill:none}.spawn-dot{fill:var(--spawn)}.mark{fill:var(--pat);stroke:var(--pat)}\n");
    for agent in ALL_AGENTS {
        let name = agent_name(agent);
        let _ = writeln!(rules, ".base-{name}{{fill:var(--a-{name})}}");
    }
    match scheme {
        Scheme::Auto => format!(
            ":root{{color-scheme:light dark;{};{AGENT_COLOURS}}}\n@media (prefers-color-scheme: dark){{:root{{{}}}}}\n{}",
            root_vars(false),
            root_vars(true),
            rules.trim_end()
        ),
        Scheme::Fixed(theme) => {
            let mut resolved = rules;
            for (name, light, deep) in PALETTE {
                let value = if theme == Theme::Dark { deep } else { light };
                resolved = resolved.replace(&format!("var(--{name})"), value);
            }
            for pair in AGENT_COLOURS.split(';') {
                let (name, value) = pair.split_once(':').expect("colour pair");
                resolved = resolved.replace(&format!("var({name})"), value);
            }
            resolved.trim_end().to_string()
        }
    }
}

const ALL_AGENTS: [Agent; 5] = [
    Agent::ClaudeCode,
    Agent::Cursor,
    Agent::Codex,
    Agent::GrokBot,
    Agent::Antigravity,
];

fn capitalised(text: &str) -> String {
    let mut chars = text.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().chain(chars).collect(),
        None => String::new(),
    }
}

/// The id of a sub-agent session: what follows `--agent-` in
/// `<parent>--agent-<id>`, or the whole id when there is no such marker.
fn subagent_short_id(id: &str) -> String {
    id.rsplit_once("--agent-")
        .map_or(id, |(_, agent)| agent)
        .chars()
        .take(6)
        .collect()
}

/// A session as named in labels and tooltips: a sub-agent by its kind and
/// own id ("Explore a3f9c2"), anything else by agent and short id.
fn session_name(session: &Session) -> String {
    match &session.subagent {
        Some(info) => format!(
            "{} {}",
            capitalised(&info.kind),
            subagent_short_id(&session.id)
        ),
        None => format!("{} {}", agent_label(session.agent), short_id(&session.id)),
    }
}

/// The text of a lane label: sub-agents carry their own arrow.
fn lane_label_text(session: &Session) -> String {
    let name = session_name(session);
    if session.subagent.is_some() {
        truncate(&format!("↳ {name}"), 20)
    } else {
        truncate(&name, 20)
    }
}

/// Draw the figure for `sessions` under `options`.
pub fn figure(sessions: &[Session], options: &Options, scheme: Scheme, standalone: bool) -> Figure {
    let groups = arrange(select(sessions, options));
    let lanes: Vec<&Lane<'_>> = groups.iter().flat_map(|(_, lanes)| lanes).collect();
    let stretches: Vec<Stretch> = lanes
        .iter()
        .flat_map(|lane| lane.activity.stretches.iter().copied())
        .collect();
    let series = concurrency(&stretches);

    let (mut low, mut high) = (
        stretches.iter().map(|s| s.start).min().unwrap_or(0),
        stretches.iter().map(|s| s.end).max().unwrap_or(0),
    );
    if high <= low {
        low -= 60_000;
        high += 60_000;
    }
    let x = |time: Ms| LABEL_W + (time - low) as f64 / (high - low) as f64 * PLOT_W;

    let mut agents: Vec<Agent> = lanes.iter().map(|lane| lane.session.agent).collect();
    agents.sort();
    agents.dedup();

    // ---- header: title, peak, description, legend ----
    let mut head = String::new();
    let mut cursor = 0.0_f64;
    let (title_size, peak_size, body_size) = (20.0, 16.0, 13.0);
    cursor += title_size * ASCENT;
    let _ = writeln!(
        head,
        "<text class=\"ink\" x=\"0\" y=\"{cursor:.1}\" font-size=\"{title_size}\" font-weight=\"700\">{TITLE}</text>"
    );
    cursor += 6.0 + peak_size;
    let peak_line = if series.peak == 0 {
        "No agent activity in the selected window.".to_string()
    } else {
        format!(
            "Peak of {} agent{} working at once at {} UTC",
            series.peak,
            if series.peak == 1 { "" } else { "s" },
            format_short(series.peak_at)
        )
    };
    let _ = writeln!(
        head,
        "<text class=\"ink peak\" x=\"0\" y=\"{cursor:.1}\" font-size=\"{peak_size}\" font-weight=\"600\">{}</text>",
        esc(&peak_line)
    );
    let description = format!(
        "{} session{} in {} project{}, from {} to {} UTC. A bar is an agent working on a turn, from your message to its last reply; dotted lines are the agent waiting for you.{}",
        lanes.len(),
        if lanes.len() == 1 { "" } else { "s" },
        groups.len(),
        if groups.len() == 1 { "" } else { "s" },
        format_short(low),
        format_short(high),
        if options.titles {
            " Session titles are shown."
        } else {
            " Session titles and message text are not included."
        }
    );
    cursor += 8.0;
    for line in wrap(&description, body_size, WIDTH) {
        cursor += body_size + 5.0;
        let _ = writeln!(
            head,
            "<text class=\"muted meta\" x=\"0\" y=\"{cursor:.1}\" font-size=\"{body_size}\">{}</text>",
            esc(&line)
        );
    }
    cursor += DESCENT * body_size + 12.0;

    // Legend: pattern swatches plus text, flowed to the figure width.
    enum Key {
        Agent(Agent),
        Approximate,
        Waiting,
    }
    let mut keys: Vec<Key> = agents.iter().copied().map(Key::Agent).collect();
    keys.push(Key::Approximate);
    keys.push(Key::Waiting);
    let (swatch_w, swatch_h, key_gap, key_size) = (34.0, 16.0, 18.0, 12.0);
    let note_size = 11.0;
    let mut lx = 0.0_f64;
    let mut ly = cursor;
    head.push_str("<g class=\"legend\">\n");
    for key in &keys {
        let (label, note) = match key {
            Key::Agent(agent) => (agent_label(*agent), pattern_name(*agent)),
            Key::Approximate => ("approximate", "dashed outline: no message timestamps"),
            Key::Waiting => ("waiting for you", "dotted line"),
        };
        let item_w =
            swatch_w + 6.0 + text_width(label, key_size) + 6.0 + text_width(note, note_size);
        if lx > 0.0 && lx + item_w > WIDTH {
            lx = 0.0;
            ly += swatch_h + 8.0;
        }
        let text_y = ly + 12.0;
        let (open, swatch) = match key {
            Key::Agent(agent) => {
                let name = agent_name(*agent);
                (
                    format!("<g class=\"legend-item\" data-agent=\"{name}\" data-pattern=\"pat-{name}\">"),
                    format!("<rect x=\"{lx:.1}\" y=\"{ly:.1}\" width=\"{swatch_w}\" height=\"{swatch_h}\" rx=\"3\" fill=\"url(#pat-{name})\" class=\"bar\"/>"),
                )
            }
            Key::Approximate => (
                "<g class=\"legend-item\" data-key=\"approximate\">".to_string(),
                format!("<rect x=\"{lx:.1}\" y=\"{ly:.1}\" width=\"{swatch_w}\" height=\"{swatch_h}\" rx=\"3\" class=\"bar approximate-key\"/>"),
            ),
            Key::Waiting => (
                "<g class=\"legend-item\" data-key=\"waiting\">".to_string(),
                format!("<line x1=\"{lx:.1}\" y1=\"{:.1}\" x2=\"{:.1}\" y2=\"{:.1}\" class=\"waiting\"/>", ly + 8.0, lx + swatch_w, ly + 8.0),
            ),
        };
        let label_x = lx + swatch_w + 6.0;
        let note_x = label_x + text_width(label, key_size) + 6.0;
        let _ = writeln!(
            head,
            "{open}{swatch}<text class=\"ink\" x=\"{label_x:.1}\" y=\"{text_y:.1}\" font-size=\"{key_size}\">{}</text><text class=\"muted\" x=\"{note_x:.1}\" y=\"{text_y:.1}\" font-size=\"{note_size}\">{}</text></g>",
            esc(label),
            esc(note)
        );
        lx += item_w + key_gap;
    }
    head.push_str("</g>\n");
    cursor = ly + swatch_h + 18.0;

    // ---- chart and lane geometry ----
    let chart_top = cursor;
    let base = chart_top + CHART_H;
    let y_max = f64::from(series.peak.max(1));
    let y = |count: u32| base - f64::from(count) / y_max * (CHART_H - 14.0);

    let mut rows: BTreeMap<&str, f64> = BTreeMap::new();
    let mut cursor = base + 34.0;
    let mut group_tops: Vec<f64> = Vec::new();
    for (_, group) in &groups {
        group_tops.push(cursor);
        cursor += GROUP_H;
        for lane in group {
            let height = if lane.missing_parent.is_some() {
                NOTE_ROW_H
            } else {
                ROW_H
            };
            rows.insert(lane.session.id.as_str(), cursor);
            cursor += height;
        }
        cursor += 6.0;
    }
    let axis_size = 11.0;
    let axis_y = cursor + 14.0;
    // The content ends where the axis labels' descenders end.
    let height = (axis_y + DESCENT * axis_size).ceil();

    let mut body = String::new();
    // Axis grid.
    let ticks = 6;
    for i in 0..=ticks {
        let time = low + (high - low) * i / ticks;
        let tx = x(time);
        let anchor = if i == ticks { "end" } else { "middle" };
        let _ = writeln!(
            body,
            "<line class=\"grid\" x1=\"{tx:.1}\" y1=\"{chart_top:.1}\" x2=\"{tx:.1}\" y2=\"{:.1}\"/><text class=\"axis\" x=\"{tx:.1}\" y=\"{axis_y:.1}\" font-size=\"{axis_size}\" text-anchor=\"{anchor}\">{}</text>",
            axis_y - 14.0,
            &format_short(time)[5..]
        );
    }

    // Concurrency curve.
    body.push_str("<g class=\"chart\">\n");
    let step = (series.peak.max(1) as usize).div_ceil(5).max(1);
    for level in (0..=series.peak.max(1) as usize).step_by(step) {
        let ly = y(level as u32);
        let _ = writeln!(
            body,
            "<line class=\"grid\" x1=\"{LABEL_W}\" y1=\"{ly:.1}\" x2=\"{WIDTH}\" y2=\"{ly:.1}\"/><text class=\"axis\" x=\"{:.1}\" y=\"{:.1}\" font-size=\"{axis_size}\" text-anchor=\"end\">{level}</text>",
            LABEL_W - 8.0,
            ly + 4.0
        );
    }
    let _ = writeln!(
        body,
        "<text class=\"axis\" x=\"0\" y=\"{:.1}\" font-size=\"{axis_size}\">Agents working at once</text>",
        chart_top + 10.0
    );
    if let Some((first, _)) = series.steps.first() {
        let mut d = format!("M{:.1},{base:.1}", x(*first));
        for (time, count) in &series.steps {
            let _ = write!(d, " H{:.1} V{:.1}", x(*time), y(*count));
        }
        d.push_str(" Z");
        let steps: Vec<String> = series
            .steps
            .iter()
            .map(|(time, count)| format!("{}={count}", format_iso(*time)))
            .collect();
        let _ = writeln!(
            body,
            "<path class=\"concurrency\" d=\"{d}\" data-peak=\"{}\" data-peak-at=\"{}\" data-steps=\"{}\"><title>Agents working at once; peak {} at {} UTC</title></path>",
            series.peak,
            format_iso(series.peak_at),
            steps.join(";"),
            series.peak,
            format_short(series.peak_at)
        );
        let _ = writeln!(
            body,
            "<circle class=\"peak-mark\" cx=\"{:.1}\" cy=\"{:.1}\" r=\"4\"/>",
            x(series.peak_at),
            y(series.peak)
        );
    }
    body.push_str("</g>\n");

    // Swimlanes.
    body.push_str("<g class=\"lanes\">\n");
    let lane_size = 12.0;
    for ((project, group), top) in groups.iter().zip(&group_tops) {
        let _ = writeln!(
            body,
            "<g class=\"project\" data-project=\"{0}\"><text class=\"project-label\" x=\"0\" y=\"{1:.1}\" font-size=\"13\">{5}<title>{0}</title></text><text class=\"axis\" x=\"{2:.1}\" y=\"{1:.1}\" font-size=\"{axis_size}\" text-anchor=\"end\">{3} session{4}</text></g>",
            esc(project),
            top + 18.0,
            LABEL_W - 8.0,
            group.len(),
            if group.len() == 1 { "" } else { "s" },
            esc(&truncate(project, 20))
        );
        for lane in group {
            let session = lane.session;
            let top = rows[session.id.as_str()];
            let row_h = if lane.missing_parent.is_some() {
                NOTE_ROW_H
            } else {
                ROW_H
            };
            let mid = top + ROW_H / 2.0;
            let name = agent_name(session.agent);
            let title = options
                .titles
                .then_some(session.metadata.title.as_deref())
                .flatten()
                .filter(|title| !title.is_empty());
            let mut label = lane_label_text(session);
            if let Some(title) = title {
                label.push_str(" · ");
                label.push_str(&truncate(
                    title,
                    14usize.saturating_sub(lane.depth * 2).max(6),
                ));
            }
            let indent = 12.0 + lane.depth as f64 * 16.0;
            let parent_attr = session
                .parent_id
                .as_deref()
                .filter(|_| lane.nested)
                .map(|parent| format!(" data-parent=\"{}\"", esc(parent)))
                .unwrap_or_default();
            let _ = writeln!(
                body,
                "<g class=\"lane\" data-session=\"{}\" data-agent=\"{name}\" data-project=\"{}\" data-depth=\"{}\" data-relation=\"{}\"{parent_attr}>",
                esc(&session.id),
                esc(&lane.project),
                lane.depth,
                lane.relation()
            );
            let _ = writeln!(
                body,
                "<rect class=\"lane-bg\" x=\"{LABEL_W}\" y=\"{top:.1}\" width=\"{PLOT_W}\" height=\"{:.1}\" rx=\"4\"/>",
                ROW_H - 2.0
            );
            if lane.depth > 0 && session.subagent.is_none() {
                let _ = writeln!(
                    body,
                    "<text class=\"nest\" x=\"{:.1}\" y=\"{:.1}\" font-size=\"{lane_size}\">↳</text>",
                    indent - 12.0,
                    mid + 4.0
                );
            }
            let _ = writeln!(
                body,
                "<text class=\"lane-label\" x=\"{indent:.1}\" y=\"{:.1}\" font-size=\"{lane_size}\">{}<title>{}</title></text>",
                mid + 4.0,
                esc(&label),
                esc(&format!(
                    "{} session {}{}{}",
                    agent_label(session.agent),
                    session.id,
                    match &session.subagent {
                        Some(sub) => format!(", sub-agent ({})", sub.kind),
                        None if session.fork.is_some() => ", fork".to_string(),
                        None => String::new(),
                    },
                    title.map(|t| format!(": {t}")).unwrap_or_default()
                ))
            );
            if let Some(missing) = &lane.missing_parent {
                let _ = writeln!(
                    body,
                    "<text class=\"note\" x=\"{indent:.1}\" y=\"{:.1}\" font-size=\"11\" data-missing-parent=\"{}\">spawned by {}, which is not in this view</text>",
                    top + row_h - 4.0,
                    esc(missing),
                    esc(&short_id(missing))
                );
            }
            for (i, stretch) in lane.activity.stretches.iter().enumerate() {
                if i > 0 {
                    let previous = lane.activity.stretches[i - 1].end;
                    let _ = writeln!(
                        body,
                        "<line class=\"waiting\" x1=\"{:.1}\" y1=\"{mid:.1}\" x2=\"{:.1}\" y2=\"{mid:.1}\" data-start=\"{}\" data-end=\"{}\"><title>Waiting for you, {} ({} to {} UTC)</title></line>",
                        x(previous),
                        x(stretch.start),
                        format_iso(previous),
                        format_iso(stretch.start),
                        format_duration(stretch.start - previous),
                        format_short(previous),
                        format_short(stretch.start)
                    );
                }
                let messages = lane.activity.messages_in(stretch);
                let approximate = lane.activity.approximate;
                let bar_w = (x(stretch.end) - x(stretch.start)).max(3.0);
                let hover = format!(
                    "{}, {} to {} UTC, {} message{}{}{}",
                    session_name(session),
                    format_short(stretch.start),
                    format_short(stretch.end),
                    messages,
                    if messages == 1 { "" } else { "s" },
                    if approximate {
                        " (approximate: no message timestamps)"
                    } else {
                        ""
                    },
                    title.map(|t| format!(", {t}")).unwrap_or_default()
                );
                let _ = writeln!(
                    body,
                    "<rect class=\"stretch bar{}\" x=\"{:.1}\" y=\"{:.1}\" width=\"{bar_w:.1}\" height=\"{}\" rx=\"3\" fill=\"url(#pat-{name})\" data-start=\"{}\" data-end=\"{}\" data-messages=\"{messages}\"{}><title>{}</title></rect>",
                    if approximate { " approximate" } else { "" },
                    x(stretch.start),
                    top + 2.0,
                    ROW_H - 6.0,
                    format_iso(stretch.start),
                    format_iso(stretch.end),
                    if approximate {
                        " data-approximate=\"true\""
                    } else {
                        ""
                    },
                    esc(&hover)
                );
            }
            if lane.activity.approximate {
                // Beside the bar, or on its left when the right would
                // spill past the figure.
                let note = "approximate";
                let right = x(lane.activity.stretches[0].end) + 6.0;
                let (note_x, anchor) = if right + text_width(note, 11.0) <= WIDTH {
                    (right, "start")
                } else {
                    (x(lane.activity.stretches[0].start) - 6.0, "end")
                };
                let _ = writeln!(
                    body,
                    "<text class=\"note\" x=\"{note_x:.1}\" y=\"{:.1}\" font-size=\"11\" text-anchor=\"{anchor}\">{note}</text>",
                    mid + 4.0
                );
            }
            body.push_str("</g>\n");
        }
    }
    body.push_str("</g>\n");

    // Spawn connectors: down from the parent lane at the moment of spawn.
    body.push_str("<g class=\"spawns\">\n");
    for lane in &lanes {
        if !lane.nested {
            continue;
        }
        let Some(parent_id) = lane.session.parent_id.as_deref() else {
            continue;
        };
        let (Some(from), Some(to)) = (rows.get(parent_id), rows.get(lane.session.id.as_str()))
        else {
            continue;
        };
        let sx = x(lane.start());
        let _ = writeln!(
            body,
            "<path class=\"spawn\" d=\"M{sx:.1},{:.1} V{:.1}\" data-parent=\"{}\" data-child=\"{}\" data-at=\"{}\"><title>{} spawned {} at {} UTC</title></path><circle class=\"spawn-dot\" cx=\"{sx:.1}\" cy=\"{:.1}\" r=\"3\"/>",
            from + ROW_H / 2.0,
            to + ROW_H / 2.0,
            esc(parent_id),
            esc(&lane.session.id),
            format_iso(lane.start()),
            esc(&short_id(parent_id)),
            esc(&session_name(lane.session)),
            format_short(lane.start()),
            from + ROW_H / 2.0
        );
    }
    body.push_str("</g>\n");

    // ---- assemble: the viewBox is the content plus an even padding ----
    let view_box = [-PAD, -PAD, WIDTH + 2.0 * PAD, height + 2.0 * PAD];
    let mut svg = String::new();
    let _ = writeln!(
        svg,
        "<svg class=\"figure\"{} viewBox=\"{} {} {} {}\" font-family=\"{FONT}\" role=\"img\" aria-label=\"Parallel sessions: concurrent sessions over time, then one lane per session grouped by project\" data-pad=\"{PAD}\" data-content=\"0 0 {WIDTH} {height}\">",
        if standalone {
            format!(
                " xmlns=\"{SVG_NS}\" width=\"{}\" height=\"{}\"",
                view_box[2], view_box[3]
            )
        } else {
            String::new()
        },
        view_box[0],
        view_box[1],
        view_box[2],
        view_box[3]
    );
    let _ = writeln!(svg, "<style>\n{}\n</style>", css(scheme));
    svg.push_str("<defs>\n");
    for agent in &agents {
        let name = agent_name(*agent);
        let _ = writeln!(
            svg,
            "<pattern id=\"pat-{name}\" width=\"8\" height=\"8\" patternUnits=\"userSpaceOnUse\"><rect width=\"8\" height=\"8\" class=\"base base-{name}\"/>{}</pattern>",
            pattern_marks(*agent)
        );
    }
    svg.push_str("</defs>\n");
    let _ = writeln!(
        svg,
        "<rect class=\"figure-bg\" x=\"{}\" y=\"{}\" width=\"{}\" height=\"{}\"/>",
        view_box[0], view_box[1], view_box[2], view_box[3]
    );
    svg.push_str(&head);
    svg.push_str(&body);
    svg.push_str("</svg>\n");
    Figure {
        svg,
        view_box,
        content: [0.0, 0.0, WIDTH, height],
    }
}

/// Render the HTML page for `sessions` under `options`: the same figure,
/// embedded, in the reader's colour scheme.
pub fn render(sessions: &[Session], options: &Options) -> String {
    let figure = figure(sessions, options, Scheme::Auto, false);
    let mut page = String::new();
    page.push_str("<!doctype html>\n<html lang=\"en\">\n<head>\n<meta charset=\"utf-8\">\n");
    let _ = writeln!(
        page,
        "<meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">\n<title>{TITLE}</title>"
    );
    page.push_str("<style>\nbody{margin:0;background:var(--bg);color:var(--fg);font:14px/1.45 system-ui,-apple-system,Segoe UI,Roboto,sans-serif}\n.figure{width:100%;height:auto;display:block}\nfooter{color:var(--muted);font-size:12px;padding:0 16px 16px}\n</style>\n</head>\n<body>\n");
    page.push_str(&figure.svg);
    page.push_str("<footer>Self-contained page: inline SVG and CSS, nothing loaded from elsewhere. Times are UTC.</footer>\n</body>\n</html>\n");
    page
}

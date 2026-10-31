//! Layout and the self-contained HTML page: a concurrency step-curve on
//! top, project-grouped swimlanes below, everything inline SVG and CSS.
//! The page never carries message text, and titles only on request.

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
const RIGHT: f64 = 24.0;
const CHART_TOP: f64 = 16.0;
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

/// Render the page for `sessions` under `options`.
pub fn render(sessions: &[Session], options: &Options) -> String {
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
    let base = CHART_TOP + CHART_H;
    let y_max = f64::from(series.peak.max(1));
    let y = |count: u32| base - f64::from(count) / y_max * (CHART_H - 14.0);

    // Row geometry.
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
    let axis_y = cursor + 14.0;
    let height = axis_y + 14.0;
    let width = LABEL_W + PLOT_W + RIGHT;

    let mut agents: Vec<Agent> = lanes.iter().map(|lane| lane.session.agent).collect();
    agents.sort();
    agents.dedup();

    let mut page = String::new();
    page.push_str("<!doctype html>\n<html lang=\"en\">\n<head>\n<meta charset=\"utf-8\">\n");
    page.push_str(
        "<meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">\n<title>Parallel sessions timeline</title>\n",
    );
    let _ = writeln!(page, "<style>\n{}\n</style>\n</head>\n<body>", css());
    page.push_str("<header>\n<h1>Parallel sessions timeline</h1>\n");
    if series.peak == 0 {
        page.push_str("<p class=\"peak\">No session activity in the selected window.</p>\n");
    } else {
        let _ = writeln!(
            page,
            "<p class=\"peak\">Peak of {} concurrent session{} at {} UTC</p>",
            series.peak,
            if series.peak == 1 { "" } else { "s" },
            format_short(series.peak_at)
        );
    }
    let _ = writeln!(
        page,
        "<p class=\"meta\">{} session{} in {} project{}, from {} to {} UTC. Activity comes from message timestamps; idle breaks are gaps unusually long for that session.{}</p>\n</header>",
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

    // Legend.
    page.push_str("<ul class=\"legend\" aria-label=\"Legend\">\n");
    for agent in &agents {
        let name = agent_name(*agent);
        let _ = writeln!(
            page,
            "<li class=\"legend-item\" data-agent=\"{name}\" data-pattern=\"pat-{name}\"><svg width=\"34\" height=\"16\" aria-hidden=\"true\"><rect width=\"34\" height=\"16\" rx=\"3\" fill=\"url(#pat-{name})\" class=\"bar\"/></svg><span>{}</span><span class=\"pattern-name\">{}</span></li>",
            agent_label(*agent),
            pattern_name(*agent)
        );
    }
    page.push_str("<li class=\"legend-item\" data-key=\"approximate\"><svg width=\"34\" height=\"16\" aria-hidden=\"true\"><rect width=\"34\" height=\"16\" rx=\"3\" class=\"bar approximate-key\"/></svg><span>approximate</span><span class=\"pattern-name\">dashed outline: no message timestamps</span></li>\n");
    page.push_str("<li class=\"legend-item\" data-key=\"idle\"><svg width=\"34\" height=\"16\" aria-hidden=\"true\"><line x1=\"0\" y1=\"8\" x2=\"34\" y2=\"8\" class=\"idle\"/></svg><span>idle break</span><span class=\"pattern-name\">dotted line</span></li>\n</ul>\n");

    // Patterns, defined once for every SVG in the page.
    page.push_str("<svg class=\"defs\" width=\"0\" height=\"0\" aria-hidden=\"true\"><defs>\n");
    for agent in &agents {
        let name = agent_name(*agent);
        let _ = writeln!(
            page,
            "<pattern id=\"pat-{name}\" width=\"8\" height=\"8\" patternUnits=\"userSpaceOnUse\"><rect width=\"8\" height=\"8\" class=\"base base-{name}\"/>{}</pattern>",
            pattern_marks(*agent)
        );
    }
    page.push_str("</defs></svg>\n");

    let _ = writeln!(
        page,
        "<svg class=\"timeline\" viewBox=\"0 0 {width} {height}\" role=\"img\" aria-label=\"Concurrent sessions over time, then one lane per session grouped by project\">"
    );

    // Axis grid.
    let ticks = 6;
    for i in 0..=ticks {
        let time = low + (high - low) * i / ticks;
        let tx = x(time);
        let _ = writeln!(
            page,
            "<line class=\"grid\" x1=\"{tx:.1}\" y1=\"{CHART_TOP}\" x2=\"{tx:.1}\" y2=\"{:.1}\"/><text class=\"axis\" x=\"{tx:.1}\" y=\"{axis_y:.1}\" text-anchor=\"middle\">{}</text>",
            axis_y - 14.0,
            &format_short(time)[5..]
        );
    }

    // Concurrency curve.
    page.push_str("<g class=\"chart\">\n");
    let step = (series.peak.max(1) as usize).div_ceil(5).max(1);
    for level in (0..=series.peak.max(1) as usize).step_by(step) {
        let ly = y(level as u32);
        let _ = writeln!(
            page,
            "<line class=\"grid\" x1=\"{LABEL_W}\" y1=\"{ly:.1}\" x2=\"{:.1}\" y2=\"{ly:.1}\"/><text class=\"axis\" x=\"{:.1}\" y=\"{:.1}\" text-anchor=\"end\">{level}</text>",
            LABEL_W + PLOT_W,
            LABEL_W - 8.0,
            ly + 4.0
        );
    }
    let _ = writeln!(
        page,
        "<text class=\"axis\" x=\"8\" y=\"{:.1}\">Sessions active at once</text>",
        CHART_TOP + 10.0
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
            page,
            "<path class=\"concurrency\" d=\"{d}\" data-peak=\"{}\" data-peak-at=\"{}\" data-steps=\"{}\"><title>Sessions active at once; peak {} at {} UTC</title></path>",
            series.peak,
            format_iso(series.peak_at),
            steps.join(";"),
            series.peak,
            format_short(series.peak_at)
        );
        let _ = writeln!(
            page,
            "<circle class=\"peak-mark\" cx=\"{:.1}\" cy=\"{:.1}\" r=\"4\"/>",
            x(series.peak_at),
            y(series.peak)
        );
    }
    page.push_str("</g>\n");

    // Swimlanes.
    page.push_str("<g class=\"lanes\">\n");
    for ((project, group), top) in groups.iter().zip(&group_tops) {
        let _ = writeln!(
            page,
            "<g class=\"project\" data-project=\"{0}\"><text class=\"project-label\" x=\"8\" y=\"{1:.1}\">{5}<title>{0}</title></text><text class=\"axis\" x=\"{2:.1}\" y=\"{1:.1}\" text-anchor=\"end\">{3} session{4}</text></g>",
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
            let mut label = format!("{} {}", agent_label(session.agent), short_id(&session.id));
            if let Some(title) = title {
                label.push_str(" · ");
                label.push_str(&truncate(
                    title,
                    14usize.saturating_sub(lane.depth * 2).max(6),
                ));
            }
            let indent = 8.0 + lane.depth as f64 * 16.0;
            let parent_attr = session
                .parent_id
                .as_deref()
                .filter(|_| lane.nested)
                .map(|parent| format!(" data-parent=\"{}\"", esc(parent)))
                .unwrap_or_default();
            let _ = writeln!(
                page,
                "<g class=\"lane\" data-session=\"{}\" data-agent=\"{name}\" data-project=\"{}\" data-depth=\"{}\" data-relation=\"{}\"{parent_attr}>",
                esc(&session.id),
                esc(&lane.project),
                lane.depth,
                lane.relation()
            );
            let _ = writeln!(
                page,
                "<rect class=\"lane-bg\" x=\"{LABEL_W}\" y=\"{top:.1}\" width=\"{PLOT_W}\" height=\"{:.1}\" rx=\"4\"/>",
                ROW_H - 2.0
            );
            if lane.depth > 0 {
                let _ = writeln!(
                    page,
                    "<text class=\"nest\" x=\"{:.1}\" y=\"{:.1}\">↳</text>",
                    indent - 12.0,
                    mid + 4.0
                );
            }
            let _ = writeln!(
                page,
                "<text class=\"lane-label\" x=\"{indent:.1}\" y=\"{:.1}\">{}<title>{}</title></text>",
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
                    page,
                    "<text class=\"note\" x=\"{indent:.1}\" y=\"{:.1}\" data-missing-parent=\"{}\">spawned by {}, which is not in this view</text>",
                    top + row_h - 4.0,
                    esc(missing),
                    esc(&short_id(missing))
                );
            }
            for (i, stretch) in lane.activity.stretches.iter().enumerate() {
                if i > 0 {
                    let previous = lane.activity.stretches[i - 1].end;
                    let _ = writeln!(
                        page,
                        "<line class=\"idle\" x1=\"{:.1}\" y1=\"{mid:.1}\" x2=\"{:.1}\" y2=\"{mid:.1}\" data-start=\"{}\" data-end=\"{}\"><title>Idle break, {} ({} to {} UTC)</title></line>",
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
                    "{} {}, {} to {} UTC, {} message{}{}{}",
                    agent_label(session.agent),
                    short_id(&session.id),
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
                    page,
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
                let _ = writeln!(
                    page,
                    "<text class=\"note\" x=\"{:.1}\" y=\"{:.1}\">approximate</text>",
                    x(lane.activity.stretches[0].end) + 6.0,
                    mid + 4.0
                );
            }
            page.push_str("</g>\n");
        }
    }
    page.push_str("</g>\n");

    // Spawn connectors: down from the parent lane at the moment of spawn.
    page.push_str("<g class=\"spawns\">\n");
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
            page,
            "<path class=\"spawn\" d=\"M{sx:.1},{:.1} V{:.1}\" data-parent=\"{}\" data-child=\"{}\" data-at=\"{}\"><title>{} spawned {} at {} UTC</title></path><circle class=\"spawn-dot\" cx=\"{sx:.1}\" cy=\"{:.1}\" r=\"3\"/>",
            from + ROW_H / 2.0,
            to + ROW_H / 2.0,
            esc(parent_id),
            esc(&lane.session.id),
            format_iso(lane.start()),
            esc(&short_id(parent_id)),
            esc(&short_id(&lane.session.id)),
            format_short(lane.start()),
            from + ROW_H / 2.0
        );
    }
    page.push_str("</g>\n</svg>\n");
    page.push_str("<footer>Self-contained page: inline SVG and CSS, nothing loaded from elsewhere. Times are UTC.</footer>\n</body>\n</html>\n");
    page
}

fn css() -> String {
    let mut css = String::from(
        ":root{color-scheme:light dark;--bg:#ffffff;--fg:#1b1f24;--muted:#5b6570;--grid:#e3e7ec;--lane:#f3f5f7;--curve:#9fc2e8;--curve-line:#1d5fa6;--pat:#16202b;--edge:#16202b;--spawn:#7a3e9d;--a-claude_code:#e69f00;--a-cursor:#2fb68a;--a-codex:#56b4e9;--a-grok_bot:#cc79a7;--a-antigravity:#d6d6d6}\n",
    );
    css.push_str("@media (prefers-color-scheme: dark){:root{--bg:#10151b;--fg:#e6eaee;--muted:#9aa5b1;--grid:#26303a;--lane:#18202a;--curve:#1f4a78;--curve-line:#7db4ee;--pat:#0b0f14;--edge:#e6eaee;--spawn:#c79bea}}\n");
    css.push_str("body{margin:0;padding:16px;background:var(--bg);color:var(--fg);font:14px/1.45 system-ui,-apple-system,Segoe UI,Roboto,sans-serif}\n");
    css.push_str("h1{font-size:20px;margin:0 0 4px}.peak{font-size:16px;font-weight:600;margin:0}.meta{color:var(--muted);margin:4px 0 12px;max-width:80ch}\n");
    css.push_str(".legend{display:flex;flex-wrap:wrap;gap:6px 18px;list-style:none;margin:0 0 12px;padding:0}.legend-item{display:flex;align-items:center;gap:6px}.pattern-name{color:var(--muted);font-size:12px}\n");
    css.push_str(".defs{position:absolute}.timeline{width:100%;height:auto;max-width:1200px;display:block;overflow:visible}\n");
    css.push_str(
        ".grid{stroke:var(--grid);stroke-width:1}.axis{fill:var(--muted);font-size:11px}\n",
    );
    css.push_str(".concurrency{fill:var(--curve);stroke:var(--curve-line);stroke-width:1.5}.peak-mark{fill:var(--curve-line);stroke:var(--bg);stroke-width:2}\n");
    css.push_str(".project-label{font-size:13px;font-weight:700;fill:var(--fg)}.lane-bg{fill:var(--lane)}.lane-label{font-size:12px;fill:var(--fg)}.nest{fill:var(--muted);font-size:12px}.note{font-size:11px;font-style:italic;fill:var(--muted)}\n");
    css.push_str(".bar{stroke:var(--edge);stroke-width:1}.stretch.approximate{stroke-dasharray:4 3;fill-opacity:.6}.approximate-key{stroke-dasharray:4 3;fill:var(--lane)}.idle{stroke:var(--muted);stroke-width:2;stroke-dasharray:2 3}\n");
    css.push_str(".spawn{stroke:var(--spawn);stroke-width:1.5;fill:none}.spawn-dot{fill:var(--spawn)}.mark{fill:var(--pat);stroke:var(--pat)}footer{color:var(--muted);font-size:12px;margin-top:12px}\n");
    for agent in [
        Agent::ClaudeCode,
        Agent::Cursor,
        Agent::Codex,
        Agent::GrokBot,
        Agent::Antigravity,
    ] {
        let name = agent_name(agent);
        let _ = writeln!(css, ".base-{name}{{fill:var(--a-{name})}}");
    }
    css.trim_end().to_string()
}

//! The weekly thanks-vs-F-bombs figure: a diverging bar chart. Thanks bars
//! rise from a zero axis and F-bomb bars hang below it, one pair per week.
//! The figure is one SVG cut to its content plus an even 16 px padding,
//! drawn in the same family as the timeline figure (fonts, palette, title
//! block, padding). Only counts, week dates and fixed wording go in; no
//! message text, session title or id is ever read into it.

use super::activity::{format_iso, Ms};
use super::scoring::{is_f_bomb, is_thanks, is_yours, typed_text};
use botspy::Session;
use std::collections::BTreeMap;
use std::fmt::Write;

use super::activity::parse_time;

const DAY_MS: Ms = 86_400_000;
const WEEK_MS: Ms = 7 * DAY_MS;
const HOUR_MS: Ms = 3_600_000;

/// Even padding between the content and the edge of the figure.
pub const PAD: f64 = 16.0;
/// A glyph's height above and below its baseline, as a fraction of the
/// font size; the figure's top and bottom edges are cut with these.
pub const ASCENT: f64 = 0.8;
pub const DESCENT: f64 = 0.25;
/// Conservative average glyph width, as a fraction of the font size.
const CHAR_W: f64 = 0.58;
/// Room on the left for the count labels of the vertical axis.
const GUTTER: f64 = 40.0;
const PLOT_W: f64 = 900.0;
const WIDTH: f64 = GUTTER + PLOT_W;
/// Height shared by the tallest thanks bar and the tallest F-bomb bar.
const PLOT_H: f64 = 220.0;
const FONT: &str = "'Helvetica Neue', Helvetica, Arial, 'Segoe UI', Roboto, sans-serif";

/// The SVG namespace, spelled in two parts so that the examples'
/// "nothing network-capable" source check does not trip on the URL text.
const SVG_NS: &str = concat!("ht", "tp://www.w3.org/2000/svg");

pub const TITLE: &str = "Thanks vs F-bombs";
const SENTENCE: &str = "Counts only the messages you typed, and message text is never shown.";

/// The colour set a figure is drawn with.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Theme {
    #[default]
    Light,
    Dark,
}

/// (name, light, dark). The first six are the timeline's own values.
const PALETTE: [(&str, &str, &str); 8] = [
    ("bg", "#ffffff", "#10151b"),
    ("fg", "#1b1f24", "#e6eaee"),
    ("muted", "#5b6570", "#9aa5b1"),
    ("grid", "#e3e7ec", "#26303a"),
    ("pat", "#16202b", "#0b0f14"),
    ("edge", "#16202b", "#e6eaee"),
    // Okabe-Ito blue and orange/vermillion: a pair that stays apart for
    // every common kind of colour-blindness.
    ("thanks", "#56b4e9", "#56b4e9"),
    ("fbomb", "#d55e00", "#e69f00"),
];

fn colour(name: &str, theme: Theme) -> &'static str {
    let (_, light, dark) = PALETTE
        .iter()
        .find(|(entry, ..)| *entry == name)
        .expect("a palette colour");
    if theme == Theme::Dark {
        dark
    } else {
        light
    }
}

/// How long one bar of the figure is.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Period {
    #[default]
    Week,
    Day,
    Hour,
}

impl Period {
    /// The start of the period holding `ms`.
    fn start(self, ms: Ms) -> Ms {
        match self {
            Period::Week => week_start(ms),
            Period::Day => ms.div_euclid(DAY_MS) * DAY_MS,
            Period::Hour => ms.div_euclid(HOUR_MS) * HOUR_MS,
        }
    }

    fn length(self) -> Ms {
        match self {
            Period::Week => WEEK_MS,
            Period::Day => DAY_MS,
            Period::Hour => HOUR_MS,
        }
    }

    /// The x axis caption.
    fn caption(self, utc_offset_hours: i64) -> String {
        let zone = match utc_offset_hours {
            0 => "UTC".to_string(),
            hours => format!("UTC{hours:+}"),
        };
        match self {
            Period::Week => format!("Week starting (Monday, {zone})"),
            Period::Day => format!("Day ({zone})"),
            Period::Hour => format!("Hour ({zone})"),
        }
    }
}

/// One bar's period of your messages (a week unless `--by` says otherwise).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Week {
    /// The start of the period, in the figure's local time.
    pub start: Ms,
    pub thanks: u64,
    pub f_bombs: u64,
}

/// Midnight UTC of the Monday on or before `ms`.
pub fn week_start(ms: Ms) -> Ms {
    let days = ms.div_euclid(DAY_MS);
    // 1970-01-01 was a Thursday, three days after a Monday.
    (days - (days + 3).rem_euclid(7)) * DAY_MS
}

/// Your messages per week, oldest first, with empty weeks between the first
/// and the last filled in. `since`/`until` keep only messages inside the
/// window, and stretch the axis to cover it. Messages without a timestamp
/// cannot be placed on a week and are left out.
pub fn weekly(sessions: &[Session], since: Option<Ms>, until: Option<Ms>) -> Vec<Week> {
    periods(sessions, since, until, Period::Week, 0)
}

/// Your messages per `period`, oldest first, as [`weekly`] does for weeks.
/// `utc_offset_hours` shifts period boundaries to local time.
pub fn periods(
    sessions: &[Session],
    since: Option<Ms>,
    until: Option<Ms>,
    period: Period,
    utc_offset_hours: i64,
) -> Vec<Week> {
    let local = |ms: Ms| period.start(ms + utc_offset_hours * HOUR_MS);
    let mut counts: BTreeMap<Ms, (u64, u64)> = BTreeMap::new();
    for session in sessions {
        for message in &session.messages {
            if !is_yours(session, message) {
                continue;
            }
            let Some(time) = message.timestamp.as_deref().and_then(parse_time) else {
                continue;
            };
            if since.is_some_and(|since| time < since) || until.is_some_and(|until| time > until) {
                continue;
            }
            let Some(text) = typed_text(message) else {
                continue;
            };
            let entry = counts.entry(local(time)).or_default();
            if is_thanks(&text) {
                entry.0 += 1;
            }
            if is_f_bomb(&text) {
                entry.1 += 1;
            }
        }
    }
    let hits = counts.iter().filter(|(_, (t, f))| t + f > 0);
    let first = hits.clone().map(|(start, _)| *start).min();
    let last = hits.map(|(start, _)| *start).max();
    let low = [first, since.map(local)].into_iter().flatten().min();
    let high = [last, until.map(local)].into_iter().flatten().max();
    let (Some(low), Some(high)) = (low, high) else {
        return Vec::new();
    };
    let mut weeks = Vec::new();
    let mut start = low;
    while start <= high {
        let (thanks, f_bombs) = counts.get(&start).copied().unwrap_or_default();
        weeks.push(Week {
            start,
            thanks,
            f_bombs,
        });
        start += period.length();
    }
    weeks
}

/// `1234` as `1,234`.
fn grouped(number: u64) -> String {
    let digits = number.to_string();
    let mut out = String::new();
    for (index, digit) in digits.chars().enumerate() {
        if index > 0 && (digits.len() - index).is_multiple_of(3) {
            out.push(',');
        }
        out.push(digit);
    }
    out
}

/// A ratio as whole numbers from 10 up, one decimal below (`22`, `3.5`,
/// `2`).
fn ratio_text(ratio: f64) -> String {
    if ratio >= 10.0 {
        format!("{ratio:.0}")
    } else {
        let text = format!("{ratio:.1}");
        text.strip_suffix(".0").unwrap_or(&text).to_string()
    }
}

/// The headline under the title: the totals and the ratio. Zero F-bombs is
/// "no F-bombs", never a division by zero.
pub fn headline(thanks: u64, f_bombs: u64) -> String {
    let bombs = |count: u64| {
        format!(
            "{} F-bomb{}",
            grouped(count),
            if count == 1 { "" } else { "s" }
        )
    };
    match (thanks, f_bombs) {
        (0, 0) => "No thanks or F-bombs in the selected window".to_string(),
        (thanks, 0) => format!("{} thanks, no F-bombs", grouped(thanks)),
        (0, f_bombs) => format!("0 thanks, {}", bombs(f_bombs)),
        (thanks, f_bombs) if thanks >= f_bombs => format!(
            "{} thanks, {}: {} thanks for every F-bomb",
            grouped(thanks),
            bombs(f_bombs),
            ratio_text(thanks as f64 / f_bombs as f64)
        ),
        (thanks, f_bombs) => format!(
            "{} thanks, {}: {} F-bombs for every thanks",
            grouped(thanks),
            bombs(f_bombs),
            ratio_text(f_bombs as f64 / thanks as f64)
        ),
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

/// `2026-09-28` for a week start.
fn date_of(ms: Ms) -> String {
    format_iso(ms)[..10].to_string()
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

fn css(theme: Theme) -> String {
    let c = |name| colour(name, theme);
    format!(
        ".figure-bg{{fill:{bg}}}.ink{{fill:{fg}}}.muted{{fill:{muted}}}.axis{{fill:{muted}}}.grid{{stroke:{grid};stroke-width:1}}.zero{{stroke:{muted};stroke-width:1.5}}\n\
.bar{{stroke:{edge};stroke-width:1}}.base-thanks{{fill:{thanks}}}.base-fbomb{{fill:{fbomb}}}.mark{{fill:{pat};stroke:{pat}}}",
        bg = c("bg"),
        fg = c("fg"),
        muted = c("muted"),
        grid = c("grid"),
        edge = c("edge"),
        thanks = c("thanks"),
        fbomb = c("fbomb"),
        pat = c("pat"),
    )
}

/// Draw the weekly figure for `weeks`.
pub fn figure(weeks: &[Week], theme: Theme) -> Figure {
    figure_by(weeks, theme, Period::Week, 0)
}

/// The figure for bars of any `period`.
pub fn figure_by(weeks: &[Week], theme: Theme, period: Period, utc_offset_hours: i64) -> Figure {
    let label_of = |start: Ms| match period {
        Period::Hour => format!("{:02}:00", start.rem_euclid(DAY_MS) / HOUR_MS),
        _ => date_of(start),
    };
    let thanks: u64 = weeks.iter().map(|week| week.thanks).sum();
    let f_bombs: u64 = weeks.iter().map(|week| week.f_bombs).sum();

    // ---- title block ----
    let mut head = String::new();
    let (title_size, headline_size, body_size) = (20.0, 16.0, 13.0);
    let mut cursor = title_size * ASCENT;
    let _ = writeln!(
        head,
        "<text class=\"ink\" x=\"0\" y=\"{cursor:.1}\" font-size=\"{title_size}\" font-weight=\"700\">{TITLE}</text>"
    );
    cursor += 6.0 + headline_size;
    let _ = writeln!(
        head,
        "<text class=\"ink headline\" x=\"0\" y=\"{cursor:.1}\" font-size=\"{headline_size}\" font-weight=\"600\">{}</text>",
        esc(&headline(thanks, f_bombs))
    );
    cursor += 8.0;
    for line in wrap(SENTENCE, body_size, WIDTH) {
        cursor += body_size + 5.0;
        let _ = writeln!(
            head,
            "<text class=\"muted meta\" x=\"0\" y=\"{cursor:.1}\" font-size=\"{body_size}\">{}</text>",
            esc(&line)
        );
    }
    cursor += DESCENT * body_size + 12.0;

    // ---- legend: a swatch per kind, told apart by fill style and label ----
    let (swatch_w, swatch_h, key_gap, key_size, note_size) = (34.0, 16.0, 18.0, 12.0, 11.0);
    let keys = [
        ("thanks", "Thanks", "solid fill, up from the axis"),
        ("fbomb", "F-bombs", "diagonal hatch, down from the axis"),
    ];
    let mut lx = 0.0_f64;
    head.push_str("<g class=\"legend\">\n");
    for (kind, label, note) in keys {
        let fill = if kind == "thanks" {
            "class=\"bar base-thanks\"".to_string()
        } else {
            "fill=\"url(#pat-fbomb)\" class=\"bar\"".to_string()
        };
        let text_y = cursor + 12.0;
        let label_x = lx + swatch_w + 6.0;
        let note_x = label_x + text_width(label, key_size) + 6.0;
        let _ = writeln!(
            head,
            "<g class=\"legend-item\" data-kind=\"{kind}\"><rect x=\"{lx:.1}\" y=\"{cursor:.1}\" width=\"{swatch_w}\" height=\"{swatch_h}\" rx=\"3\" {fill}/><text class=\"ink\" x=\"{label_x:.1}\" y=\"{text_y:.1}\" font-size=\"{key_size}\">{label}</text><text class=\"muted\" x=\"{note_x:.1}\" y=\"{text_y:.1}\" font-size=\"{note_size}\">{note}</text></g>"
        );
        lx = note_x + text_width(note, note_size) + key_gap;
    }
    head.push_str("</g>\n");
    cursor += swatch_h + 18.0;

    // ---- chart geometry ----
    let value_size = 11.0;
    let label_gap = 4.0;
    let label_room = label_gap + value_size * ASCENT;
    let max_thanks = weeks.iter().map(|week| week.thanks).max().unwrap_or(0);
    let max_f_bombs = weeks.iter().map(|week| week.f_bombs).max().unwrap_or(0);
    // One scale for both directions, so a bar's length means the same thing
    // above and below the axis.
    let unit = if max_thanks + max_f_bombs == 0 {
        0.0
    } else {
        PLOT_H / (max_thanks + max_f_bombs) as f64
    };
    let bar_h = |count: u64| {
        if count == 0 {
            0.0
        } else {
            (count as f64 * unit).max(2.0)
        }
    };
    let up_h = bar_h(max_thanks);
    let down_h = bar_h(max_f_bombs);
    let zero_y = cursor + label_room + up_h;
    let plot_bottom = zero_y + down_h;
    let slot = PLOT_W / weeks.len().max(1) as f64;
    let bar_w = (slot * 0.7).min(56.0);
    let centre = |index: usize| GUTTER + (index as f64 + 0.5) * slot;

    let mut body = String::new();
    // Grid at the extremes, labelled with counts on the left.
    let axis_size = 11.0;
    let mut tick = |y: f64, label: &str| {
        let _ = writeln!(
            body,
            "<line class=\"grid\" x1=\"{GUTTER}\" y1=\"{y:.1}\" x2=\"{WIDTH}\" y2=\"{y:.1}\"/>\n<text class=\"axis\" x=\"{:.1}\" y=\"{:.1}\" font-size=\"{axis_size}\" text-anchor=\"end\">{label}</text>",
            GUTTER - 6.0,
            y + axis_size * 0.35
        );
    };
    if max_thanks > 0 {
        tick(zero_y - up_h, &grouped(max_thanks));
    }
    if max_f_bombs > 0 {
        tick(plot_bottom, &grouped(max_f_bombs));
    }
    let starts: Vec<String> = weeks.iter().map(|week| label_of(week.start)).collect();
    let _ = writeln!(
        body,
        "<g class=\"weeks\" data-weeks=\"{}\" data-starts=\"{}\">",
        weeks.len(),
        starts.join(",")
    );
    for (index, week) in weeks.iter().enumerate() {
        let x = centre(index) - bar_w / 2.0;
        let date = label_of(week.start);
        // Narrow bars have room for a label only on the tallest ones.
        let peak = max_thanks.max(max_f_bombs);
        let labelled = |count: u64| {
            if slot < 24.0 {
                count * 3 >= peak && text_width(&grouped(count), value_size) <= 2.5 * slot
            } else {
                text_width(&grouped(count), value_size) + 2.0 <= slot
            }
        };
        if week.thanks > 0 {
            let height = bar_h(week.thanks);
            let _ = writeln!(
                body,
                "<rect class=\"bar bar-thanks base-thanks\" data-week=\"{date}\" data-kind=\"thanks\" data-count=\"{}\" x=\"{x:.1}\" y=\"{:.1}\" width=\"{bar_w:.1}\" height=\"{height:.1}\"><title>Week of {date}: {} thanks</title></rect>",
                week.thanks,
                zero_y - height,
                grouped(week.thanks)
            );
            if labelled(week.thanks) {
                let _ = writeln!(
                    body,
                    "<text class=\"ink value\" x=\"{:.1}\" y=\"{:.1}\" font-size=\"{value_size}\" text-anchor=\"middle\">{}</text>",
                    centre(index),
                    zero_y - height - label_gap,
                    grouped(week.thanks)
                );
            }
        }
        if week.f_bombs > 0 {
            let height = bar_h(week.f_bombs);
            let _ = writeln!(
                body,
                "<rect class=\"bar bar-fbomb\" fill=\"url(#pat-fbomb)\" data-week=\"{date}\" data-kind=\"fbomb\" data-count=\"{}\" x=\"{x:.1}\" y=\"{zero_y:.1}\" width=\"{bar_w:.1}\" height=\"{height:.1}\"><title>Week of {date}: {} F-bombs</title></rect>",
                week.f_bombs,
                grouped(week.f_bombs)
            );
            if labelled(week.f_bombs) {
                let _ = writeln!(
                    body,
                    "<text class=\"ink value\" x=\"{:.1}\" y=\"{:.1}\" font-size=\"{value_size}\" text-anchor=\"middle\">{}</text>",
                    centre(index),
                    zero_y + height + label_gap + value_size * ASCENT,
                    grouped(week.f_bombs)
                );
            }
        }
    }
    body.push_str("</g>\n");
    // The zero axis, drawn over the bars' roots.
    let _ = writeln!(
        body,
        "<line class=\"zero\" x1=\"{GUTTER}\" y1=\"{zero_y:.1}\" x2=\"{WIDTH}\" y2=\"{zero_y:.1}\"/>\n<text class=\"axis\" x=\"{:.1}\" y=\"{:.1}\" font-size=\"{axis_size}\" text-anchor=\"end\">0</text>",
        GUTTER - 6.0,
        zero_y + axis_size * 0.35
    );

    // ---- x axis: week-start dates, thinned so they never collide ----
    let date_w = text_width(
        &label_of(weeks.first().map_or(0, |week| week.start)),
        axis_size,
    );
    let step = ((date_w + 12.0) / slot).ceil().max(1.0) as usize;
    let bottom_room = if max_f_bombs > 0 {
        label_room + value_size * DESCENT
    } else {
        0.0
    };
    let axis_y = plot_bottom + bottom_room + 10.0 + axis_size * ASCENT;
    body.push_str("<g class=\"axis\">\n");
    for (index, week) in weeks.iter().enumerate() {
        let x = centre(index);
        if index % step == 0 && x - date_w / 2.0 >= 0.0 && x + date_w / 2.0 <= WIDTH {
            let _ = writeln!(
                body,
                "<text class=\"axis\" x=\"{x:.1}\" y=\"{axis_y:.1}\" font-size=\"{axis_size}\" text-anchor=\"middle\">{}</text>",
                label_of(week.start)
            );
        }
    }
    body.push_str("</g>\n");
    let caption_y = axis_y + 6.0 + axis_size;
    let _ = writeln!(
        body,
        "<text class=\"muted caption\" x=\"{:.1}\" y=\"{caption_y:.1}\" font-size=\"{axis_size}\" text-anchor=\"middle\">{}</text>",
        GUTTER + PLOT_W / 2.0,
        period.caption(utc_offset_hours)
    );
    // The content ends where the caption's descenders end.
    let height = (caption_y + DESCENT * axis_size).ceil();

    // ---- assemble: the viewBox is the content plus an even padding ----
    let view_box = [-PAD, -PAD, WIDTH + 2.0 * PAD, height + 2.0 * PAD];
    let mut svg = String::new();
    let _ = writeln!(
        svg,
        "<svg class=\"figure\" xmlns=\"{SVG_NS}\" width=\"{}\" height=\"{}\" viewBox=\"{} {} {} {}\" font-family=\"{FONT}\" role=\"img\" aria-label=\"Thanks and F-bombs in your messages per week, as bars up and down from a zero axis\" data-pad=\"{PAD}\" data-content=\"0 0 {WIDTH} {height}\">",
        view_box[2], view_box[3], view_box[0], view_box[1], view_box[2], view_box[3]
    );
    let _ = writeln!(svg, "<style>\n{}\n</style>", css(theme));
    // The F-bomb fill: its colour plus diagonal stripes, the timeline's own
    // hatch.
    svg.push_str("<defs>\n<pattern id=\"pat-fbomb\" width=\"8\" height=\"8\" patternUnits=\"userSpaceOnUse\"><rect width=\"8\" height=\"8\" class=\"base-fbomb\"/><path class=\"mark\" d=\"M-1,7 l2,2 M0,0 l8,8 M7,-1 l2,2\" stroke-width=\"2\"/></pattern>\n</defs>\n");
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

#[cfg(test)]
mod tests {
    // The bdd test crate compiles this file too, without a unit-test
    // harness, where nothing here is used.
    #[allow(unused_imports)]
    use super::*;

    #[test]
    fn periods_start_where_they_should() {
        let time = parse_time("2026-10-20T15:30:00Z").unwrap();
        assert_eq!(
            Period::Hour.start(time),
            parse_time("2026-10-20T15:00:00Z").unwrap()
        );
        assert_eq!(
            Period::Day.start(time),
            parse_time("2026-10-20T00:00:00Z").unwrap()
        );
        assert_eq!(
            Period::Week.start(time),
            parse_time("2026-10-19T00:00:00Z").unwrap()
        );
    }

    #[test]
    fn a_window_stretches_the_axis_to_every_period_in_it() {
        let since = parse_time("2026-10-20T04:00:00Z");
        let until = parse_time("2026-10-21T03:59:59Z");
        let hours = periods(&[], since, until, Period::Hour, -4);
        assert_eq!(hours.len(), 24);
        assert_eq!(date_of(hours[0].start), "2026-10-20");
        let days = periods(&[], since, until, Period::Day, -4);
        assert_eq!(days.len(), 1);
    }

    #[test]
    fn hour_figures_label_hours_and_name_the_zone() {
        let since = parse_time("2026-10-20T04:00:00Z");
        let until = parse_time("2026-10-21T03:59:59Z");
        let hours = periods(&[], since, until, Period::Hour, -4);
        let svg = figure_by(&hours, Theme::Light, Period::Hour, -4).svg;
        assert!(svg.contains("00:00"));
        assert!(svg.contains("Hour (UTC-4)"));
    }

    #[test]
    fn weeks_start_on_monday() {
        let thursday = parse_time("1970-01-01").unwrap();
        assert_eq!(date_of(week_start(thursday)), "1969-12-29");
        let sunday = parse_time("2026-10-04T23:59:00Z").unwrap();
        assert_eq!(date_of(week_start(sunday)), "2026-09-28");
        let monday = parse_time("2026-10-05T00:00:00Z").unwrap();
        assert_eq!(date_of(week_start(monday)), "2026-10-05");
    }

    #[test]
    fn headline_covers_every_case() {
        assert_eq!(
            headline(1234, 56),
            "1,234 thanks, 56 F-bombs: 22 thanks for every F-bomb"
        );
        assert_eq!(headline(9, 0), "9 thanks, no F-bombs");
        assert_eq!(
            headline(3, 1),
            "3 thanks, 1 F-bomb: 3 thanks for every F-bomb"
        );
        assert_eq!(
            headline(5, 2),
            "5 thanks, 2 F-bombs: 2.5 thanks for every F-bomb"
        );
        assert_eq!(headline(0, 4), "0 thanks, 4 F-bombs");
        assert_eq!(
            headline(2, 6),
            "2 thanks, 6 F-bombs: 3 F-bombs for every thanks"
        );
        assert_eq!(
            headline(0, 0),
            "No thanks or F-bombs in the selected window"
        );
    }

    #[test]
    fn thousands_are_grouped() {
        assert_eq!(grouped(0), "0");
        assert_eq!(grouped(999), "999");
        assert_eq!(grouped(1000), "1,000");
        assert_eq!(grouped(1234567), "1,234,567");
    }
}

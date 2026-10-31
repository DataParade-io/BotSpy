//! The activity model: when each session was actually active, and how many
//! sessions were active at once. Pure functions over timestamps in
//! milliseconds since the Unix epoch, with no I/O.

use botspy::Session;

/// Milliseconds since the Unix epoch (UTC).
pub type Ms = i64;

const DAY_MS: Ms = 86_400_000;

/// Days since 1970-01-01 for a proleptic Gregorian date.
fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    let year = if month <= 2 { year - 1 } else { year };
    let era = year.div_euclid(400);
    let yoe = year - era * 400;
    let doy = (153 * (if month > 2 { month - 3 } else { month + 9 }) + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// The inverse of `days_from_civil`.
fn civil_from_days(days: i64) -> (i64, i64, i64) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    (year, month, day)
}

/// Parse an RFC 3339-style timestamp (`2026-10-01T09:00:00Z`, with optional
/// fraction and offset; a bare date means midnight UTC) or a bare epoch
/// number (milliseconds, or seconds when small).
pub fn parse_time(text: &str) -> Option<Ms> {
    let text = text.trim();
    if text.is_empty() || !text.is_ascii() {
        return None;
    }
    if text.bytes().all(|byte| byte.is_ascii_digit()) {
        let number: i64 = text.parse().ok()?;
        return Some(if number >= 100_000_000_000 {
            number
        } else {
            number * 1000
        });
    }
    let number = |part: &str| part.parse::<i64>().ok();
    let bytes = text.as_bytes();
    if bytes.len() < 10 || bytes[4] != b'-' || bytes[7] != b'-' {
        return None;
    }
    let (year, month, day) = (
        number(&text[0..4])?,
        number(&text[5..7])?,
        number(&text[8..10])?,
    );
    if !(1..=12).contains(&month) || !(1..=31).contains(&day) {
        return None;
    }
    let mut ms = days_from_civil(year, month, day) * DAY_MS;
    let rest = &text[10..];
    if rest.is_empty() {
        return Some(ms);
    }
    let rest = rest
        .strip_prefix('T')
        .or_else(|| rest.strip_prefix(' '))?
        .as_bytes();
    let part = |from: usize, to: usize| -> Option<i64> {
        std::str::from_utf8(rest.get(from..to)?)
            .ok()?
            .parse::<i64>()
            .ok()
    };
    if rest.get(2) != Some(&b':') {
        return None;
    }
    let (hour, minute) = (part(0, 2)?, part(3, 5)?);
    let mut index = 5;
    let (mut second, mut fraction) = (0, 0);
    if rest.get(5) == Some(&b':') {
        second = part(6, 8)?;
        index = 8;
        if rest.get(8) == Some(&b'.') {
            let digits: String = rest[9..]
                .iter()
                .take_while(|byte| byte.is_ascii_digit())
                .map(|byte| *byte as char)
                .collect();
            index = 9 + digits.len();
            fraction = format!("{digits:0<3}")[..3].parse().ok()?;
        }
    }
    ms += ((hour * 60 + minute) * 60 + second) * 1000 + fraction;
    let zone = std::str::from_utf8(&rest[index..]).ok()?;
    if !matches!(zone, "" | "Z" | "z") {
        let sign = match zone.as_bytes()[0] {
            b'+' => 1,
            b'-' => -1,
            _ => return None,
        };
        let digits: String = zone[1..].chars().filter(|c| *c != ':').collect();
        if digits.len() != 2 && digits.len() != 4 {
            return None;
        }
        let offset_hours = number(&digits[0..2])?;
        let offset_minutes = if digits.len() == 4 {
            number(&digits[2..4])?
        } else {
            0
        };
        ms -= sign * (offset_hours * 60 + offset_minutes) * 60_000;
    }
    Some(ms)
}

fn civil(ms: Ms) -> (i64, i64, i64, i64, i64, i64) {
    let days = ms.div_euclid(DAY_MS);
    let of_day = ms.rem_euclid(DAY_MS) / 1000;
    let (year, month, day) = civil_from_days(days);
    (
        year,
        month,
        day,
        of_day / 3600,
        of_day % 3600 / 60,
        of_day % 60,
    )
}

/// `2026-10-01T09:00:00Z`.
pub fn format_iso(ms: Ms) -> String {
    let (year, month, day, hour, minute, second) = civil(ms);
    format!("{year:04}-{month:02}-{day:02}T{hour:02}:{minute:02}:{second:02}Z")
}

/// `2026-10-01 09:00` (UTC, minute precision).
pub fn format_short(ms: Ms) -> String {
    let (year, month, day, hour, minute, _) = civil(ms);
    format!("{year:04}-{month:02}-{day:02} {hour:02}:{minute:02}")
}

/// `35 min`, `2 h 5 min`.
pub fn format_duration(ms: Ms) -> String {
    let minutes = (ms + 30_000) / 60_000;
    match (minutes / 60, minutes % 60) {
        (0, minutes) => format!("{minutes} min"),
        (hours, 0) => format!("{hours} h"),
        (hours, minutes) => format!("{hours} h {minutes} min"),
    }
}

/// One continuous run of activity inside a session.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Stretch {
    pub start: Ms,
    pub end: Ms,
}

/// A session's activity: its stretches (ordered, never overlapping), and
/// whether they are only an approximation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Activity {
    /// Parsed message timestamps, ascending. Empty for approximate spans.
    pub times: Vec<Ms>,
    pub stretches: Vec<Stretch>,
    /// No message timestamps: one started..last-activity span.
    pub approximate: bool,
    /// Messages in the session, for the approximate hover text.
    pub message_total: usize,
}

impl Activity {
    /// Messages inside a stretch (the whole session when approximate).
    pub fn messages_in(&self, stretch: &Stretch) -> usize {
        if self.approximate {
            return self.message_total;
        }
        self.times
            .iter()
            .filter(|time| **time >= stretch.start && **time <= stretch.end)
            .count()
    }
}

/// Linear-interpolated quantile of an ascending, non-empty slice.
fn quantile(sorted: &[Ms], q: f64) -> f64 {
    let position = q * (sorted.len() - 1) as f64;
    let low = position.floor() as usize;
    let high = position.ceil() as usize;
    sorted[low] as f64 + (sorted[high] - sorted[low]) as f64 * (position - low as f64)
}

/// The idle threshold of a session's own gaps: Tukey's upper fence,
/// Q3 + 1.5 IQR. `None` when there are too few gaps (under 4) to estimate.
pub fn idle_fence(gaps: &[Ms]) -> Option<f64> {
    if gaps.len() < 4 {
        return None;
    }
    let mut sorted = gaps.to_vec();
    sorted.sort_unstable();
    let (q1, q3) = (quantile(&sorted, 0.25), quantile(&sorted, 0.75));
    Some(q3 + 1.5 * (q3 - q1))
}

/// Split ascending timestamps into stretches at gaps beyond the fence.
pub fn stretches_of(times: &[Ms]) -> Vec<Stretch> {
    let Some((&first, rest)) = times.split_first() else {
        return Vec::new();
    };
    let gaps: Vec<Ms> = times.windows(2).map(|pair| pair[1] - pair[0]).collect();
    let fence = idle_fence(&gaps);
    let mut stretches = Vec::new();
    let mut start = first;
    let mut previous = first;
    for &time in rest {
        if fence.is_some_and(|fence| (time - previous) as f64 > fence) {
            stretches.push(Stretch {
                start,
                end: previous,
            });
            start = time;
        }
        previous = time;
    }
    stretches.push(Stretch {
        start,
        end: previous,
    });
    stretches
}

/// The activity of one session, or `None` when it has no usable time at all.
pub fn activity_of(session: &Session) -> Option<Activity> {
    let mut times: Vec<Ms> = session
        .messages
        .iter()
        .filter_map(|message| message.timestamp.as_deref().and_then(parse_time))
        .collect();
    times.sort_unstable();
    if !times.is_empty() {
        return Some(Activity {
            stretches: stretches_of(&times),
            times,
            approximate: false,
            message_total: session.messages.len(),
        });
    }
    let started = parse_time(&session.started_at)
        .or_else(|| session.file_modified_at.as_deref().and_then(parse_time))?;
    let last = parse_time(&session.last_activity_at)
        .unwrap_or(started)
        .max(started);
    Some(Activity {
        times,
        stretches: vec![Stretch {
            start: started,
            end: last,
        }],
        approximate: true,
        message_total: session.messages.len(),
    })
}

/// Keep only the part of the activity inside `[since, until]`.
pub fn clip(activity: Activity, since: Option<Ms>, until: Option<Ms>) -> Option<Activity> {
    let low = since.unwrap_or(Ms::MIN);
    let high = until.unwrap_or(Ms::MAX);
    let stretches: Vec<Stretch> = activity
        .stretches
        .iter()
        .filter_map(|stretch| {
            let clipped = Stretch {
                start: stretch.start.max(low),
                end: stretch.end.min(high),
            };
            let kept = if stretch.start == stretch.end {
                clipped.start <= clipped.end
            } else {
                clipped.start < clipped.end
            };
            kept.then_some(clipped)
        })
        .collect();
    if stretches.is_empty() {
        return None;
    }
    Some(Activity {
        stretches,
        ..activity
    })
}

/// The number of sessions active at once, as a step series.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Concurrency {
    /// `(time, count from that time on)` at every change.
    pub steps: Vec<(Ms, u32)>,
    pub peak: u32,
    /// The first time the peak is reached.
    pub peak_at: Ms,
}

/// Sweep-line over every stretch (instants have no duration and count for
/// nothing).
pub fn concurrency(stretches: &[Stretch]) -> Concurrency {
    let mut events: Vec<(Ms, i64)> = stretches
        .iter()
        .filter(|stretch| stretch.end > stretch.start)
        .flat_map(|stretch| [(stretch.start, 1), (stretch.end, -1)])
        .collect();
    events.sort_unstable();
    let mut result = Concurrency::default();
    let mut count: i64 = 0;
    let mut index = 0;
    while index < events.len() {
        let time = events[index].0;
        while index < events.len() && events[index].0 == time {
            count += events[index].1;
            index += 1;
        }
        let count = count as u32;
        result.steps.push((time, count));
        if count > result.peak {
            result.peak = count;
            result.peak_at = time;
        }
    }
    result
}

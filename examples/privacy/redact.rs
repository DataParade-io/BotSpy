//! Byte-range redaction: replace every finding's range with
//! `[REDACTED:<rule>]`, slicing by UTF-8 byte offsets.

/// One range to redact: `start..end` in bytes, labelled by rule.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Range {
    pub start: usize,
    pub end: usize,
    pub rule: String,
}

/// Replace every range in `text`. Overlapping ranges are merged into one
/// replacement, labelled by the range that starts first (the longest of
/// those, on a tie). Ranges that touch but do not overlap stay separate.
///
/// Fails closed: a range outside the text, inverted, or not on a character
/// boundary is an error, never a guess, so a bad offset can't leak a value.
pub fn redact_text(text: &str, ranges: &[Range]) -> Result<String, String> {
    let mut sorted: Vec<&Range> = ranges.iter().collect();
    sorted.sort_by(|a, b| a.start.cmp(&b.start).then(b.end.cmp(&a.end)));
    let mut merged: Vec<(usize, usize, &str)> = Vec::new();
    for range in sorted {
        if range.start > range.end
            || range.end > text.len()
            || !text.is_char_boundary(range.start)
            || !text.is_char_boundary(range.end)
        {
            return Err("a finding's byte range does not fit its record".to_string());
        }
        match merged.last_mut() {
            Some(last) if range.start < last.1 => last.1 = last.1.max(range.end),
            _ => merged.push((range.start, range.end, &range.rule)),
        }
    }
    let mut out = String::with_capacity(text.len());
    let mut at = 0;
    for (start, end, rule) in merged {
        out.push_str(&text[at..start]);
        out.push_str("[REDACTED:");
        out.push_str(rule);
        out.push(']');
        at = end;
    }
    out.push_str(&text[at..]);
    Ok(out)
}

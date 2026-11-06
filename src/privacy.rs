//! Text-bearing parts used by the LeftBlock integration.

use crate::{KnownPart, Part};
use std::borrow::Cow;

/// Return the text of a part that LeftBlock can inspect.
pub fn part_text(part: &Part) -> Option<Cow<'_, str>> {
    let Part::Known(known) = part else {
        return None;
    };
    let text = match known {
        KnownPart::Text { text, .. }
        | KnownPart::System { text, .. }
        | KnownPart::Thinking {
            text: Some(text), ..
        }
        | KnownPart::ToolResult {
            text: Some(text), ..
        } => Cow::Borrowed(text.as_str()),
        KnownPart::ToolCall {
            arguments: Some(arguments),
            ..
        } => match (&arguments.raw, &arguments.value) {
            (Some(raw), _) => Cow::Borrowed(raw.as_str()),
            (None, Some(value)) => Cow::Owned(value.to_string()),
            _ => return None,
        },
        _ => return None,
    };
    (!text.is_empty()).then_some(text)
}

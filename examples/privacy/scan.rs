//! The LeftBlock side of the pipeline: records out on the child's stdin,
//! findings back on its stdout, with a tiny JSON codec so the example needs
//! nothing but `botspy` and the standard library.
//!
//! Contract (`leftblock scan --jsonl`): one `{"id","text"}` object per
//! input line; one `{"id","start","end","rule","kind",...}` object per
//! finding line. `start`/`end` are UTF-8 byte offsets into that record's
//! text. Exit 0 = clean, 1 = findings, 2 = error. The matched value is
//! never part of the output.

use botspy::{KnownPart, Part, Session};
use std::borrow::Cow;
use std::ffi::OsString;
use std::io::{BufRead, BufReader, BufWriter, Read, Write};
use std::process::{Command, Stdio};

/// What the user is told when LeftBlock cannot be started.
pub const INSTALL_HINT: &str =
    "leftblock not found: install it with `pipx install leftblock` (or `pip install leftblock`)";

/// One finding: a byte range inside one record, tagged with rule and kind.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Finding {
    pub id: String,
    pub start: usize,
    pub end: usize,
    pub rule: String,
    pub kind: String,
}

/// Where a record lives in the extracted sessions.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct Slot {
    pub session: usize,
    pub message: usize,
    pub part: usize,
}

impl Slot {
    /// The opaque record id: an index, never a transcript identifier.
    pub fn id(&self) -> String {
        format!("s{}/m{}/p{}", self.session, self.message, self.part)
    }

    pub fn parse(id: &str) -> Option<Slot> {
        let mut fields = id.split('/');
        let session = fields.next()?.strip_prefix('s')?.parse().ok()?;
        let message = fields.next()?.strip_prefix('m')?.parse().ok()?;
        let part = fields.next()?.strip_prefix('p')?.parse().ok()?;
        if fields.next().is_some() {
            return None;
        }
        Some(Slot {
            session,
            message,
            part,
        })
    }
}

/// The scannable text of a part, if it carries any.
pub fn part_text(part: &Part) -> Option<Cow<'_, str>> {
    let Part::Known(known) = part else {
        return None;
    };
    let text: Cow<'_, str> = match known {
        KnownPart::Text { text, .. } | KnownPart::System { text, .. } => Cow::Borrowed(text),
        KnownPart::Thinking {
            text: Some(text), ..
        }
        | KnownPart::ToolResult {
            text: Some(text), ..
        } => Cow::Borrowed(text),
        KnownPart::ToolCall {
            arguments: Some(arguments),
            ..
        } => match (&arguments.raw, &arguments.value) {
            (Some(raw), _) => Cow::Borrowed(raw),
            (None, Some(value)) => Cow::Owned(value.to_string()),
            _ => return None,
        },
        _ => return None,
    };
    if text.is_empty() {
        None
    } else {
        Some(text)
    }
}

/// Every text-bearing slot, in order.
pub fn slots(sessions: &[Session]) -> impl Iterator<Item = (Slot, Cow<'_, str>)> {
    sessions.iter().enumerate().flat_map(|(s, session)| {
        session
            .messages
            .iter()
            .enumerate()
            .flat_map(move |(m, message)| {
                message
                    .parts
                    .iter()
                    .enumerate()
                    .filter_map(move |(p, part)| {
                        part_text(part).map(|text| {
                            (
                                Slot {
                                    session: s,
                                    message: m,
                                    part: p,
                                },
                                text,
                            )
                        })
                    })
            })
    })
}

/// Why a scan did not complete. Always a single printable line.
#[derive(Debug)]
pub struct ScanError(pub String);

/// Stream every text-bearing part to `leftblock scan --jsonl` and collect
/// the findings. Transcript text only ever travels over the child's stdin;
/// nothing is written to disk here.
///
/// The writer runs on its own thread while this thread drains stdout (and a
/// third thread drains stderr), so neither pipe can fill up and stall the
/// child, however many records there are.
pub fn scan(
    sessions: &[Session],
    command: &OsString,
    deep: bool,
) -> Result<Vec<Finding>, ScanError> {
    let mut cmd = Command::new(command);
    cmd.arg("scan").arg("--jsonl");
    if deep {
        cmd.arg("--deep");
    }
    cmd.stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = cmd.spawn().map_err(|error| {
        if error.kind() == std::io::ErrorKind::NotFound {
            ScanError(INSTALL_HINT.to_string())
        } else {
            ScanError(format!("cannot run leftblock: {}", error.kind()))
        }
    })?;
    let stdin = child.stdin.take().expect("piped stdin");
    let stdout = child.stdout.take().expect("piped stdout");
    let mut stderr = child.stderr.take().expect("piped stderr");

    let outcome = std::thread::scope(|scope| {
        let writer = scope.spawn(move || -> std::io::Result<()> {
            let mut sink = BufWriter::new(stdin);
            for (slot, text) in slots(sessions) {
                let line = format!(
                    "{{\"id\":{},\"text\":{}}}\n",
                    json_string(&slot.id()),
                    json_string(&text)
                );
                sink.write_all(line.as_bytes())?;
            }
            sink.flush()
            // `sink` drops here, closing stdin: that is the end of input.
        });
        let stderr_reader = scope.spawn(move || {
            let mut raw = Vec::new();
            let _ = stderr.by_ref().take(64 * 1024).read_to_end(&mut raw);
            // Keep draining so a chatty child can never block on stderr.
            let _ = std::io::copy(&mut stderr, &mut std::io::sink());
            String::from_utf8_lossy(&raw).into_owned()
        });

        let mut findings = Vec::new();
        let mut malformed = false;
        for line in BufReader::new(stdout).lines() {
            let Ok(line) = line else {
                malformed = true;
                break;
            };
            if line.trim().is_empty() {
                continue;
            }
            match parse_finding(&line) {
                Some(finding) => findings.push(finding),
                None => malformed = true,
            }
        }
        let written = writer.join().expect("writer thread");
        let diagnostics = stderr_reader.join().expect("stderr thread");
        (findings, malformed, written, diagnostics)
    });
    let (findings, malformed, written, diagnostics) = outcome;
    let status = child
        .wait()
        .map_err(|error| ScanError(format!("cannot wait for leftblock: {}", error.kind())))?;
    match status.code() {
        Some(0) | Some(1) => {}
        Some(2) => {
            let line = diagnostics.lines().next().unwrap_or("").trim();
            return Err(ScanError(if line.is_empty() {
                "leftblock scan failed (exit 2)".to_string()
            } else {
                format!("leftblock scan failed: {line}")
            }));
        }
        other => {
            return Err(ScanError(format!(
                "leftblock scan ended unexpectedly (status {})",
                other.map_or("signal".to_string(), |code| code.to_string())
            )));
        }
    }
    if let Err(error) = written {
        if error.kind() != std::io::ErrorKind::BrokenPipe {
            return Err(ScanError(format!(
                "cannot stream records to leftblock: {}",
                error.kind()
            )));
        }
    }
    if malformed {
        return Err(ScanError(
            "leftblock produced output that is not contract JSONL".to_string(),
        ));
    }
    Ok(findings)
}

/// JSON-encode a string (quotes included).
pub fn json_string(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + 2);
    out.push('"');
    for c in text.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// Parse one finding line: a flat JSON object. Unknown keys (finding_id,
/// score, ...) are skipped. Anything malformed is `None`.
pub fn parse_finding(line: &str) -> Option<Finding> {
    let mut cursor = Cursor {
        bytes: line.as_bytes(),
        at: 0,
    };
    cursor.skip_ws();
    cursor.expect(b'{')?;
    let (mut id, mut start, mut end, mut rule, mut kind) = (None, None, None, None, None);
    loop {
        cursor.skip_ws();
        if cursor.peek()? == b'}' {
            cursor.at += 1;
            break;
        }
        let key = cursor.string()?;
        cursor.skip_ws();
        cursor.expect(b':')?;
        cursor.skip_ws();
        match cursor.peek()? {
            b'"' => {
                let value = cursor.string()?;
                match key.as_str() {
                    "id" => id = Some(value),
                    "rule" => rule = Some(value),
                    "kind" => kind = Some(value),
                    _ => {}
                }
            }
            b'-' | b'0'..=b'9' => {
                let value = cursor.number()?;
                match key.as_str() {
                    "start" => start = Some(value.parse::<usize>().ok()?),
                    "end" => end = Some(value.parse::<usize>().ok()?),
                    _ => {}
                }
            }
            _ => cursor.literal()?,
        }
        cursor.skip_ws();
        match cursor.peek()? {
            b',' => cursor.at += 1,
            b'}' => {}
            _ => return None,
        }
    }
    cursor.skip_ws();
    if cursor.at != cursor.bytes.len() {
        return None;
    }
    Some(Finding {
        id: id?,
        start: start?,
        end: end?,
        rule: rule?,
        kind: kind?,
    })
}

struct Cursor<'a> {
    bytes: &'a [u8],
    at: usize,
}

impl Cursor<'_> {
    fn peek(&self) -> Option<u8> {
        self.bytes.get(self.at).copied()
    }

    fn skip_ws(&mut self) {
        while matches!(self.peek(), Some(b' ' | b'\t' | b'\r' | b'\n')) {
            self.at += 1;
        }
    }

    fn expect(&mut self, byte: u8) -> Option<()> {
        (self.peek()? == byte).then(|| self.at += 1)
    }

    fn number(&mut self) -> Option<String> {
        let begin = self.at;
        while matches!(
            self.peek(),
            Some(b'-' | b'+' | b'.' | b'e' | b'E' | b'0'..=b'9')
        ) {
            self.at += 1;
        }
        std::str::from_utf8(&self.bytes[begin..self.at])
            .ok()
            .map(str::to_string)
    }

    fn literal(&mut self) -> Option<()> {
        for word in [&b"true"[..], b"false", b"null"] {
            if self.bytes[self.at..].starts_with(word) {
                self.at += word.len();
                return Some(());
            }
        }
        None
    }

    fn hex4(&mut self) -> Option<u32> {
        let digits = self.bytes.get(self.at..self.at + 4)?;
        self.at += 4;
        u32::from_str_radix(std::str::from_utf8(digits).ok()?, 16).ok()
    }

    fn string(&mut self) -> Option<String> {
        self.expect(b'"')?;
        let mut out = Vec::new();
        loop {
            let byte = self.peek()?;
            self.at += 1;
            match byte {
                b'"' => return String::from_utf8(out).ok(),
                b'\\' => {
                    let escape = self.peek()?;
                    self.at += 1;
                    let c = match escape {
                        b'"' => '"',
                        b'\\' => '\\',
                        b'/' => '/',
                        b'b' => '\u{8}',
                        b'f' => '\u{c}',
                        b'n' => '\n',
                        b'r' => '\r',
                        b't' => '\t',
                        b'u' => {
                            let high = self.hex4()?;
                            let code = if (0xd800..0xdc00).contains(&high) {
                                self.expect(b'\\')?;
                                self.expect(b'u')?;
                                let low = self.hex4()?;
                                0x10000 + ((high - 0xd800) << 10) + low.checked_sub(0xdc00)?
                            } else {
                                high
                            };
                            char::from_u32(code)?
                        }
                        _ => return None,
                    };
                    let mut buffer = [0u8; 4];
                    out.extend_from_slice(c.encode_utf8(&mut buffer).as_bytes());
                }
                _ => out.push(byte),
            }
        }
    }
}

//! LRCLIB's Lyricsfile: the song's lyrics as YAML, each line with its
//! words and when each is sung, in milliseconds. LRCLIB gives it beside
//! its LRC (`lyricsfile`, with `hasWordSync`) for songs timed word by word:
//!
//! ```yaml
//! version: "1.0"
//! metadata:
//!   title: Pretending
//!   duration_ms: 234561
//! lines:
//!   - text: Just forget it
//!     words:
//!       - text: "Just "
//!         start_ms: 9800
//!         end_ms: 10321
//!     start_ms: 9800
//!     end_ms: 12750
//! plain: |
//!   [Verse 1]
//!   Just forget it
//! ```
//!
//! Read with a small reader of block YAML (mappings, lists, plain, quoted
//! and block scalars), which is all the format uses: no YAML library for it.

use super::lyric_formats::{finish, push_word};
use super::model::LyricLine;

/// The lines of a Lyricsfile, timed by the word where it says; empty when
/// it has none (or cannot be read).
pub fn lines(source: &str) -> Vec<LyricLine> {
    let doc = parse(source);
    let ms = |node: Option<&Yaml>| node.and_then(Yaml::text).and_then(|s| s.trim().parse::<f64>().ok()).filter(|v| v.is_finite()).map(|v| v / 1000.0);
    // As LRC's [offset:]: taken off every time.
    let offset = ms(doc.get("metadata").and_then(|m| m.get("offset_ms"))).unwrap_or(0.0);
    let mut lines = Vec::new();
    for item in doc.get("lines").map(Yaml::items).unwrap_or_default() {
        let text = item.get("text").and_then(Yaml::text).unwrap_or_default();
        let mut line = LyricLine { end: ms(item.get("end_ms")).map(|e| (e - offset).max(0.0)), ..LyricLine::default() };
        let words: Vec<(f64, Option<f64>, &str)> = item
            .get("words")
            .map(Yaml::items)
            .unwrap_or_default()
            .iter()
            .filter_map(|word| Some((ms(word.get("start_ms"))?, ms(word.get("end_ms")), word.get("text").and_then(Yaml::text).unwrap_or_default())))
            .collect();
        for (i, &(start, end, text)) in words.iter().enumerate() {
            // Some give each word only its start: it lasts until the next
            // begins, the last until its line ends.
            let end = end
                .filter(|&end| end > start)
                .or_else(|| words.get(i + 1).map(|next| next.0).filter(|&next| next > start))
                .or_else(|| ms(item.get("end_ms")).filter(|&end| end > start))
                .unwrap_or(start + 0.5);
            push_word(&mut line.words, (start - offset).max(0.0), (end - offset).max(0.0), text);
        }
        let Some(start) = ms(item.get("start_ms")).map(|s| (s - offset).max(0.0)).or_else(|| line.words.first().map(|w| w.start)) else {
            continue;
        };
        line.time = start;
        if line.words.is_empty() {
            line.text = text.trim().to_owned();
        }
        finish(&mut line);
        lines.push(line);
    }
    lines.sort_by(|a, b| a.time.total_cmp(&b.time));
    lines
}

/// A YAML node, as far as Lyricsfiles go.
#[derive(Clone, Debug, PartialEq)]
enum Yaml {
    Map(Vec<(String, Yaml)>),
    List(Vec<Yaml>),
    Text(String),
    Null,
}

impl Yaml {
    fn get(&self, key: &str) -> Option<&Yaml> {
        match self {
            Yaml::Map(entries) => entries.iter().find(|(k, _)| k == key).map(|(_, v)| v),
            _ => None,
        }
    }

    fn items(&self) -> &[Yaml] {
        match self {
            Yaml::List(items) => items,
            _ => &[],
        }
    }

    fn text(&self) -> Option<&str> {
        match self {
            Yaml::Text(text) => Some(text),
            _ => None,
        }
    }
}

fn parse(source: &str) -> Yaml {
    let lines: Vec<(usize, &str)> = source
        .lines()
        .map(|raw| {
            let raw = raw.trim_end();
            let text = raw.trim_start_matches(' ');
            (raw.len() - text.len(), text)
        })
        .collect();
    let mut reader = Reader { lines, at: 0 };
    if reader.peek().is_some_and(|(_, text)| text == "---" || text.starts_with("--- ")) {
        reader.at += 1;
    }
    reader.block(0)
}

/// Comment or nothing: skipped between nodes.
fn blank(text: &str) -> bool {
    text.is_empty() || text.starts_with('#')
}

fn is_item(text: &str) -> bool {
    text == "-" || text.starts_with("- ")
}

/// `key: rest` (rest empty for a key alone on its line).
fn key_of(text: &str) -> Option<(String, &str)> {
    if let Some(quote) = text.chars().next().filter(|c| *c == '"' || *c == '\'') {
        let (key, after) = quoted_prefix(text, quote)?;
        let rest = after.trim_start().strip_prefix(':')?;
        return (rest.is_empty() || rest.starts_with(' ')).then(|| (key, rest.trim()));
    }
    let at = text.find(": ").or_else(|| text.ends_with(':').then(|| text.len() - 1))?;
    let key = text[..at].trim();
    if key.is_empty() || key.starts_with(['[', '{', '&', '*', '!', '|', '>']) {
        return None;
    }
    Some((key.to_owned(), text[at + 1..].trim()))
}

/// A quoted string at the start of `text`, unescaped, and what follows it;
/// none when it does not close on this line.
fn quoted_prefix(text: &str, quote: char) -> Option<(String, &str)> {
    let body = &text[1..];
    let close = closing(body, quote)?;
    Some((unquote(&body[..close], quote), &body[close + 1..]))
}

/// Where an opened quote closes in `body`.
fn closing(body: &str, quote: char) -> Option<usize> {
    let mut chars = body.char_indices().peekable();
    while let Some((at, c)) = chars.next() {
        if quote == '"' && c == '\\' {
            chars.next();
        } else if c == quote {
            if quote == '\'' && chars.peek().is_some_and(|(_, n)| *n == '\'') {
                chars.next();
                continue;
            }
            return Some(at);
        }
    }
    None
}

fn unquote(body: &str, quote: char) -> String {
    if quote == '\'' {
        return body.replace("''", "'");
    }
    let mut out = String::with_capacity(body.len());
    let mut chars = body.chars();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        let Some(e) = chars.next() else { break };
        let hex = |chars: &mut std::str::Chars, n: usize| {
            let digits: String = chars.by_ref().take(n).collect();
            u32::from_str_radix(&digits, 16).ok().and_then(char::from_u32)
        };
        match e {
            'n' => out.push('\n'),
            't' | '\t' => out.push('\t'),
            'r' => out.push('\r'),
            '0' => out.push('\0'),
            'N' => out.push('\u{85}'),
            '_' => out.push('\u{a0}'),
            'L' => out.push('\u{2028}'),
            'P' => out.push('\u{2029}'),
            'x' => out.extend(hex(&mut chars, 2)),
            'u' => out.extend(hex(&mut chars, 4)),
            'U' => out.extend(hex(&mut chars, 8)),
            other => out.push(other),
        }
    }
    out
}

struct Reader<'a> {
    /// Each line's indent and the rest of it.
    lines: Vec<(usize, &'a str)>,
    at: usize,
}

impl<'a> Reader<'a> {
    /// The next line with something on it.
    fn peek(&mut self) -> Option<(usize, &'a str)> {
        while self.lines.get(self.at).is_some_and(|(_, text)| blank(text)) {
            self.at += 1;
        }
        self.lines.get(self.at).copied()
    }

    /// The node of lines indented at least `min`.
    fn block(&mut self, min: usize) -> Yaml {
        match self.peek() {
            Some((indent, text)) if indent >= min => {
                if is_item(text) {
                    self.list(indent)
                } else if key_of(text).is_some() {
                    self.map(indent)
                } else {
                    self.at += 1;
                    self.scalar(text, indent.saturating_sub(1))
                }
            }
            _ => Yaml::Null,
        }
    }

    fn list(&mut self, indent: usize) -> Yaml {
        let mut items = Vec::new();
        while let Some((at, text)) = self.peek() {
            if at != indent || !is_item(text) {
                break;
            }
            let rest = text[1..].trim_start();
            if rest.is_empty() {
                self.at += 1;
                items.push(self.block(indent + 1));
            } else if is_item(rest) || key_of(rest).is_some() {
                // `- key: value`: the item's mapping starts here, at the
                // column of its first key.
                let column = indent + (text.len() - rest.len());
                self.lines[self.at] = (column, rest);
                items.push(self.block(column));
            } else {
                self.at += 1;
                items.push(self.scalar(rest, indent));
            }
        }
        Yaml::List(items)
    }

    fn map(&mut self, indent: usize) -> Yaml {
        let mut entries = Vec::new();
        while let Some((at, text)) = self.peek() {
            if at != indent || is_item(text) {
                break;
            }
            let Some((key, rest)) = key_of(text) else { break };
            self.at += 1;
            let value = if rest.is_empty() {
                // A list may sit at its key's own indent.
                match self.peek() {
                    Some((at, text)) if at == indent && is_item(text) => self.list(indent),
                    _ => self.block(indent + 1),
                }
            } else if rest.starts_with(['|', '>']) {
                self.block_scalar(rest, indent)
            } else {
                self.scalar(rest, indent)
            };
            entries.push((key, value));
        }
        Yaml::Map(entries)
    }

    /// A scalar starting with `first`, perhaps carried on over the lines
    /// after, indented more than `parent`.
    fn scalar(&mut self, first: &str, parent: usize) -> Yaml {
        if let Some(quote) = first.chars().next().filter(|c| *c == '"' || *c == '\'') {
            let mut body = first[1..].to_owned();
            loop {
                if let Some(close) = closing(&body, quote) {
                    return Yaml::Text(unquote(&body[..close], quote));
                }
                let Some(&(_, next)) = self.lines.get(self.at) else {
                    return Yaml::Text(unquote(&body, quote));
                };
                self.at += 1;
                // A line break is a space, none after an escaping `\`,
                // and an empty line a line break.
                if quote == '"' && body.ends_with('\\') && !body.ends_with("\\\\") {
                    body.pop();
                } else if next.is_empty() {
                    body.push('\n');
                    continue;
                } else if !body.ends_with('\n') {
                    body.push(' ');
                }
                body.push_str(next.trim());
            }
        }
        let mut text = plain(first).to_owned();
        if text == "~" || text == "null" {
            return Yaml::Null;
        }
        if text == "[]" {
            return Yaml::List(Vec::new());
        }
        if text == "{}" {
            return Yaml::Map(Vec::new());
        }
        // Folded over the lines after: each break a space, an empty line
        // a break.
        let mut breaks = 0;
        let mut at = self.at;
        while let Some(&(indent, next)) = self.lines.get(at) {
            if next.is_empty() {
                breaks += 1;
                at += 1;
                continue;
            }
            if indent <= parent || next.starts_with('#') {
                break;
            }
            text.push_str(&if breaks == 0 { " ".to_owned() } else { "\n".repeat(breaks) });
            text.push_str(plain(next));
            breaks = 0;
            at += 1;
            self.at = at;
        }
        Yaml::Text(text)
    }

    /// `|` (lines kept) or `>` (folded) and the lines under it.
    fn block_scalar(&mut self, header: &str, parent: usize) -> Yaml {
        let folded = header.starts_with('>');
        let explicit = header.chars().find_map(|c| c.to_digit(10)).map(|d| parent + d as usize);
        let mut body: Vec<String> = Vec::new();
        let mut indent = explicit;
        while let Some(&(at, text)) = self.lines.get(self.at) {
            if text.is_empty() {
                body.push(String::new());
                self.at += 1;
                continue;
            }
            let indent = *indent.get_or_insert(at);
            if at < indent || at <= parent {
                break;
            }
            body.push(format!("{}{text}", " ".repeat(at - indent)));
            self.at += 1;
        }
        while body.last().is_some_and(String::is_empty) {
            body.pop();
        }
        let mut text = if folded {
            let mut out = String::new();
            for (i, line) in body.iter().enumerate() {
                if i > 0 {
                    out.push(if line.is_empty() || body[i - 1].is_empty() { '\n' } else { ' ' });
                }
                out.push_str(line);
            }
            out
        } else {
            body.join("\n")
        };
        if !header.contains('-') && !text.is_empty() {
            text.push('\n');
        }
        Yaml::Text(text)
    }
}

/// A plain scalar without a comment after it.
fn plain(text: &str) -> &str {
    let text = match text.find(" #") {
        Some(at) => &text[..at],
        None => text,
    };
    text.trim()
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"version: "1.0"
metadata:
  title: Pretending
  artist: Kaneko Lumi
  instrumental: false
  duration_ms: 234561
lines:
  - text: Just forget it
    words:
      - text: "Just "
        start_ms: 9800
        end_ms: 10321
      - text: "forget "
        start_ms: 10321
        end_ms: 11201
      - text: it
        start_ms: 11201
        end_ms: 12750
    start_ms: 9800
    end_ms: 12750
  - text: 'Ask me, "What''s happening?"'
    words:
    - text: "Ask "
      start_ms: 36680
      end_ms: 37000
    - text: "me, \"What's "
      start_ms: 37000
      end_ms: 37600
    - text: "happening?\""
      start_ms: 37600
      end_ms: 39000
    start_ms: 36680
  - text: ""
    start_ms: 40000
  - text: A line timed only as a whole
    start_ms: 41000
    end_ms: 43000
plain: |
  [Verse 1]
  Just forget it

  Ask me
"#;

    #[test]
    fn words_and_lines() {
        let lines = lines(SAMPLE);
        assert_eq!(lines.len(), 4);
        assert_eq!(lines[0].text, "Just forget it");
        assert_eq!(lines[0].words.len(), 3);
        assert_eq!(lines[0].words[0].text, "Just ");
        assert!((lines[0].words[1].start - 10.321).abs() < 1e-9);
        assert!((lines[0].time - 9.8).abs() < 1e-9);
        assert_eq!(lines[0].end, Some(12.75));
        // A list at its key's own indent, and escapes.
        assert_eq!(lines[1].text, "Ask me, \"What's happening?\"");
        assert_eq!(lines[1].words.len(), 3);
        assert_eq!(lines[1].end, Some(39.0));
        assert_eq!(lines[2].text, "");
        assert!(lines[3].words.is_empty());
        assert_eq!(lines[3].text, "A line timed only as a whole");
        assert_eq!(lines[3].end, Some(43.0));
    }

    #[test]
    fn words_given_only_their_starts() {
        let lines = lines("lines:\n- text: My brain is\n  start_ms: 9640\n  end_ms: 11000\n  words:\n  - text: 'My '\n    start_ms: 9640\n  - text: 'brain '\n    start_ms: 9800\n  - text: is\n    start_ms: 10250\n");
        let ends: Vec<f64> = lines[0].words.iter().map(|w| w.end).collect();
        assert_eq!(ends, [9.8, 10.25, 11.0]);
    }

    #[test]
    fn scalars_carried_over_lines() {
        let doc = parse("a: \"one two\n  three\\\n  four\"\nb: plain words\n  go on\nc: >-\n  folded\n  text\nd: |\n  kept\n  lines\n");
        assert_eq!(doc.get("a").and_then(Yaml::text), Some("one two threefour"));
        assert_eq!(doc.get("b").and_then(Yaml::text), Some("plain words go on"));
        assert_eq!(doc.get("c").and_then(Yaml::text), Some("folded text"));
        assert_eq!(doc.get("d").and_then(Yaml::text), Some("kept\nlines\n"));
    }

    #[test]
    fn nothing_to_read() {
        assert!(lines("").is_empty());
        assert!(lines("version: \"1.0\"\nlines: []\n").is_empty());
        assert!(lines("not yaml at all [").is_empty());
    }
}

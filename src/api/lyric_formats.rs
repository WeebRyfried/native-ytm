//! The forms timed lyrics come in, read into lines and their words:
//!
//! - TTML, Apple Music's way (Better Lyrics', BiniLyrics' and Unison's):
//!   `<p>` lines of `<span>` words, backing vocals in a span of role
//!   `x-bg`, a duet's voices by their agents, translations in the head;
//! - LRC, perhaps with each word timed inside its line (Musixmatch's):
//!   `[00:27.32] <00:27.32> I've <00:27.60> <00:27.66> been <00:27.80>`;
//! - QRC, QQ Music's: `[start,length]` lines of `word(start,length)`, in
//!   milliseconds.

use std::collections::HashMap;

use crate::api::model::{LyricLine, Voice, Word};

/// Lines read from TTML, and who wrote the song when it says.
#[derive(Debug, Default)]
pub struct Ttml {
    pub lines: Vec<LyricLine>,
    pub songwriters: Vec<String>,
}

/// Seconds from a clock time: "1:02.345", "62.345", "1:02:03.5", "12.3s".
fn clock(text: &str) -> Option<f64> {
    let text = text.trim();
    let text = text.strip_suffix('s').filter(|t| !t.contains(':')).unwrap_or(text);
    let mut seconds = 0.0;
    for part in text.split(':') {
        let value: f64 = part.trim().parse().ok()?;
        if !value.is_finite() || value < 0.0 {
            return None;
        }
        seconds = seconds * 60.0 + value;
    }
    Some(seconds)
}

/// An attribute by its local name, whatever its namespace.
fn attr<'a>(node: roxmltree::Node<'a, '_>, name: &str) -> Option<&'a str> {
    node.attributes().find(|a| a.name() == name).map(|a| a.value())
}

/// All the text under a node.
fn text_of(node: roxmltree::Node) -> String {
    node.descendants().filter(|n| n.is_text()).filter_map(|n| n.text()).collect()
}

pub fn ttml(source: &str) -> Option<Ttml> {
    let options = roxmltree::ParsingOptions { allow_dtd: true, ..Default::default() };
    let doc = roxmltree::Document::parse_with_options(source, options).ok()?;
    let root = doc.root_element();
    let named = |name: &'static str| root.descendants().filter(move |n| n.is_element() && n.tag_name().name() == name);

    // Who sings, by the agents named: each person (or character) a voice of
    // their own, in the order they are named; a group, or anyone else, all
    // of them together.
    let mut voices: HashMap<&str, Voice> = HashMap::new();
    let mut people = 0;
    for agent in named("agent") {
        let Some(id) = attr(agent, "id") else { continue };
        let voice = match attr(agent, "type") {
            Some("person" | "character") => {
                people += 1;
                match people {
                    1 => Voice::First,
                    2 => Voice::Second,
                    _ => Voice::Third,
                }
            }
            _ => Voice::Together,
        };
        voices.insert(id, voice);
    }

    // A translation, English first, by each line's key.
    let translations: Vec<roxmltree::Node> = named("translation").collect();
    let chosen = translations
        .iter()
        .find(|t| attr(**t, "lang").is_some_and(|l| l.to_ascii_lowercase().starts_with("en")))
        .or(translations.first());
    let mut translated: HashMap<&str, String> = HashMap::new();
    if let Some(translation) = chosen {
        for text in translation.children().filter(|n| n.is_element() && n.tag_name().name() == "text") {
            if let Some(key) = attr(text, "for") {
                let said = text_of(text).trim().to_owned();
                if !said.is_empty() {
                    translated.insert(key, said);
                }
            }
        }
    }

    let songwriters = named("songwriter").map(|n| text_of(n).trim().to_owned()).filter(|s| !s.is_empty()).collect();

    let mut lines = Vec::new();
    for p in named("p") {
        let Some(begin) = attr(p, "begin").and_then(clock) else {
            continue;
        };
        let end = attr(p, "end").and_then(clock);
        let mut words = Vec::new();
        let mut backing = Vec::new();
        gather(p, &mut words, &mut backing, false);
        let mut line = LyricLine {
            time: begin,
            end,
            translation: attr(p, "key").and_then(|k| translated.get(k).cloned()),
            voice: attr(p, "agent").and_then(|a| voices.get(a).copied()).unwrap_or_default(),
            ..LyricLine::default()
        };
        if words.is_empty() {
            // Timed by the line alone: its text, less any backing vocals.
            line.text = p
                .children()
                .filter(|c| !(c.is_element() && attr(*c, "role").is_some_and(|r| r.starts_with("x-"))))
                .map(text_of)
                .collect::<String>()
                .split_whitespace()
                .collect::<Vec<_>>()
                .join(" ");
        } else {
            line.words = words;
        }
        line.backing = backing;
        finish(&mut line);
        if !line.text.is_empty() || !line.backing.is_empty() {
            lines.push(line);
        }
    }
    lines.sort_by(|a, b| a.time.total_cmp(&b.time));
    Some(Ttml { lines, songwriters })
}

/// A line's timed spans into its words, and those in a span of role
/// `x-bg` into its backing vocals. The spaces between spans are kept on the
/// words before them.
fn gather(node: roxmltree::Node, words: &mut Vec<Word>, backing: &mut Vec<Word>, in_backing: bool) {
    for child in node.children() {
        let into = if in_backing { &mut *backing } else { &mut *words };
        if child.is_text() {
            let text = child.text().unwrap_or_default();
            if text.chars().any(char::is_whitespace) {
                space_after(into);
            }
            continue;
        }
        if !child.is_element() || child.tag_name().name() != "span" {
            continue;
        }
        match attr(child, "role") {
            Some("x-bg") => gather(child, words, backing, true),
            // Translations and readings set inside the line: not sung.
            Some(role) if role.starts_with("x-") => {}
            _ => {
                let timed = (attr(child, "begin").and_then(clock), attr(child, "end").and_then(clock));
                if child.children().any(|c| c.is_element()) {
                    gather(child, words, backing, in_backing);
                } else if let (Some(start), Some(end)) = timed {
                    push_word(into, start, end, child.text().unwrap_or_default());
                }
            }
        }
    }
}

/// A word, its spaces kept as one after it (and one before it on the word
/// before).
fn push_word(words: &mut Vec<Word>, start: f64, end: f64, text: &str) {
    if text.trim().is_empty() {
        if !text.is_empty() {
            space_after(words);
        }
        return;
    }
    if text.starts_with(char::is_whitespace) {
        space_after(words);
    }
    let mut word = text.trim().to_owned();
    if text.ends_with(char::is_whitespace) {
        word.push(' ');
    }
    words.push(Word { start, end: end.max(start), text: word });
}

fn space_after(words: &mut [Word]) {
    if let Some(last) = words.last_mut()
        && !last.text.ends_with(' ')
    {
        last.text.push(' ');
    }
}

/// A line's text made from its words, and its end from its last word.
fn finish(line: &mut LyricLine) {
    for words in [&mut line.words, &mut line.backing] {
        if let Some(last) = words.last_mut() {
            last.text.truncate(last.text.trim_end().len());
        }
    }
    if !line.words.is_empty() {
        line.text = line.words.iter().map(|w| w.text.as_str()).collect();
    }
    let last_sung = line.words.iter().chain(&line.backing).map(|w| w.end).fold(None, |a: Option<f64>, e| Some(a.map_or(e, |a| a.max(e))));
    line.end = match (line.end, last_sung) {
        (Some(end), Some(sung)) => Some(end.max(sung)),
        (end, sung) => end.or(sung),
    };
}

/// LRC: `[mm:ss.xx] text` lines (several times on one line are several
/// lines), and inside a line perhaps its words, `<mm:ss.xx> word`. An
/// `[offset:]` (seconds) is taken off every time; a `[bg: …]` line is the
/// backing vocals of the line before it.
pub fn lrc(source: &str) -> Vec<LyricLine> {
    let mut offset = 0.0;
    for raw in source.lines() {
        if let Some(value) = raw.trim().strip_prefix("[offset:").and_then(|r| r.strip_suffix(']')) {
            offset = value.trim().trim_start_matches('+').parse().unwrap_or(0.0);
        }
    }
    let mut lines: Vec<LyricLine> = Vec::new();
    for raw in source.lines() {
        let raw = raw.trim();
        if let Some(inner) = raw.strip_prefix("[bg:").and_then(|r| r.strip_suffix(']')) {
            if let Some(line) = lines.last_mut() {
                let start = line.time + offset;
                line.backing = timed_words(inner, start, offset);
                finish(line);
            }
            continue;
        }
        let mut rest = raw;
        let mut times = Vec::new();
        while let Some(stripped) = rest.strip_prefix('[') {
            let Some(close) = stripped.find(']') else { break };
            let tag = &stripped[..close];
            rest = stripped[close + 1..].trim_start();
            if let Some(time) = clock(tag).filter(|_| tag.contains(':')) {
                times.push(time);
            }
        }
        for time in times {
            let mut line = LyricLine { time: (time - offset).max(0.0), ..LyricLine::default() };
            if rest.contains('<') {
                line.words = timed_words(rest, time, offset);
            }
            if line.words.is_empty() {
                line.text = rest.trim().to_owned();
            }
            finish(&mut line);
            lines.push(line);
        }
    }
    lines.sort_by(|a, b| a.time.total_cmp(&b.time));
    lines
}

/// The words of `<start> word <end>` text, from `line_start` when the
/// first has no time of its own; a word without an end ends where the next
/// begins.
fn timed_words(text: &str, line_start: f64, offset: f64) -> Vec<Word> {
    // (time, the text after it) in turn.
    let mut pieces: Vec<(Option<f64>, String)> = vec![(None, String::new())];
    let mut rest = text;
    while let Some(open) = rest.find('<') {
        let (before, after) = rest.split_at(open);
        pieces.last_mut().expect("one").1.push_str(before);
        match after.find('>').and_then(|close| clock(&after[1..close]).filter(|_| after[1..close].contains(':')).map(|t| (t, close))) {
            Some((time, close)) => {
                pieces.push((Some(time), String::new()));
                rest = &after[close + 1..];
            }
            None => {
                pieces.last_mut().expect("one").1.push('<');
                rest = &after[1..];
            }
        }
    }
    pieces.last_mut().expect("one").1.push_str(rest);
    let mut words = Vec::new();
    for (i, (time, said)) in pieces.iter().enumerate() {
        let start = time.unwrap_or(line_start);
        let next = pieces.get(i + 1).and_then(|(t, _)| *t);
        let end = next.unwrap_or(start + 0.5);
        push_word(&mut words, start - offset, end - offset, said);
    }
    words
}

/// QRC: lines `[start,length]` of `word(start,length)`, in milliseconds,
/// perhaps within QQ Music's XML (its `LyricContent`).
pub fn qrc(source: &str) -> Vec<LyricLine> {
    let content = match source.find("LyricContent=\"") {
        Some(at) => {
            let from = at + "LyricContent=\"".len();
            let len = source[from..].find('"').unwrap_or(source.len() - from);
            unescape(&source[from..from + len])
        }
        None => source.to_owned(),
    };
    let ms = |s: &str| s.trim().parse::<f64>().ok().map(|v| v / 1000.0);
    let mut lines = Vec::new();
    for raw in content.lines() {
        let raw = raw.trim();
        let Some(close) = raw.strip_prefix('[').and_then(|r| r.find(']')) else {
            continue;
        };
        let Some((start, length)) = raw[1..close + 1].split_once(',') else {
            continue;
        };
        let (Some(start), Some(length)) = (ms(start), ms(length)) else {
            continue;
        };
        let mut line = LyricLine { time: start, end: Some(start + length), ..LyricLine::default() };
        let mut rest = &raw[close + 2..];
        let mut said = String::new();
        while let Some(open) = rest.find('(') {
            let (before, after) = rest.split_at(open);
            said.push_str(before);
            let timing = after.find(')').and_then(|close| {
                let (s, l) = after[1..close].split_once(',')?;
                Some((ms(s)?, ms(l)?, close))
            });
            match timing {
                Some((s, l, close)) => {
                    push_word(&mut line.words, s, s + l, &said);
                    said.clear();
                    rest = &after[close + 1..];
                }
                None => {
                    said.push('(');
                    rest = &after[1..];
                }
            }
        }
        finish(&mut line);
        if !line.text.is_empty() {
            lines.push(line);
        }
    }
    lines.sort_by(|a, b| a.time.total_cmp(&b.time));
    lines
}

/// XML's escapes undone, for text taken out of an attribute by hand (so
/// its line breaks are kept).
fn unescape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(at) = rest.find('&') {
        out.push_str(&rest[..at]);
        rest = &rest[at..];
        let Some(end) = rest.find(';').filter(|&e| e <= 10) else {
            out.push('&');
            rest = &rest[1..];
            continue;
        };
        let entity = &rest[1..end];
        let ch = match entity {
            "amp" => Some('&'),
            "lt" => Some('<'),
            "gt" => Some('>'),
            "quot" => Some('"'),
            "apos" => Some('\''),
            _ => entity
                .strip_prefix("#x")
                .and_then(|h| u32::from_str_radix(h, 16).ok())
                .or_else(|| entity.strip_prefix('#').and_then(|d| d.parse().ok()))
                .and_then(char::from_u32),
        };
        match ch {
            Some(c) => {
                out.push(c);
                rest = &rest[end + 1..];
            }
            None => {
                out.push('&');
                rest = &rest[1..];
            }
        }
    }
    out.push_str(rest);
    out
}

/// Backing vocals written into their line in brackets, at its end ("I could
/// never say it on the phone (say it on the phone)"), as Musixmatch, LRCLIB
/// and KuGou write them: moved under the line, as TTML marks them. A line
/// all in brackets is sung as it is, and one with backing vocals of its own
/// is left alone.
pub fn split_backing(line: &mut LyricLine) {
    if !line.backing.is_empty() {
        return;
    }
    let text = line.text.trim_end();
    let Some(close) = text.chars().last().filter(|c| matches!(c, ')' | '）')) else {
        return;
    };
    let open = if close == ')' { '(' } else { '（' };
    // Where the bracket closing the line was opened.
    let mut depth = 0;
    let mut start = None;
    for (at, c) in text.char_indices().rev() {
        if c == close {
            depth += 1;
        } else if c == open {
            depth -= 1;
            if depth == 0 {
                start = Some(at);
                break;
            }
        }
    }
    let Some(start) = start else { return };
    let before = text[..start].trim_end();
    if before.is_empty() {
        return;
    }
    if line.words.is_empty() {
        let group = text[start..].to_owned();
        line.text = before.to_owned();
        line.backing = vec![Word { start: line.time, end: line.end.unwrap_or(line.time), text: group }];
        return;
    }
    // Timed by the word: the words from the one the bracket opens.
    let mut at = 0;
    let Some(first) = line.words.iter().position(|w| {
        let here = at;
        at += w.text.len();
        here + (w.text.len() - w.text.trim_start().len()) == start
    }) else {
        return;
    };
    if first == 0 {
        return;
    }
    line.backing = line.words.split_off(first);
    finish(line);
}

/// The credits some services put before the words ("词：Ayase", "作曲 :
/// …", the song's name and singer), taken out.
pub fn drop_credits(mut lines: Vec<LyricLine>, title: &str, artist: &str) -> Vec<LyricLine> {
    let loose = |s: &str| s.to_lowercase().chars().filter(|c| c.is_alphanumeric()).collect::<String>();
    let (title, artist) = (loose(title), loose(artist));
    let is_credit = |text: &str| {
        let text = text.trim();
        let label = text.split_once(['：', ':']).map(|(label, _)| label.trim());
        let labelled = label.is_some_and(|l| !l.is_empty() && l.chars().count() <= 12 && !l.contains(' ') || CREDITS.iter().any(|c| l.eq_ignore_ascii_case(c)));
        let named = text.contains(" - ") && {
            let text = loose(text);
            (!title.is_empty() && text.contains(&title)) || (!artist.is_empty() && text.contains(&artist))
        };
        labelled || named
    };
    let leading = lines.iter().take(12).take_while(|l| is_credit(&l.text)).count();
    lines.drain(..leading);
    lines
}

/// Credit labels written with a space in them.
const CREDITS: [&str; 6] = ["lyrics by", "music by", "composed by", "arranged by", "written by", "produced by"];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clock_times() {
        assert_eq!(clock("1:02.5"), Some(62.5));
        assert_eq!(clock("62.345"), Some(62.345));
        assert_eq!(clock("1:00:01"), Some(3601.0));
        assert_eq!(clock("12.5s"), Some(12.5));
        assert_eq!(clock("x"), None);
    }

    #[test]
    fn ttml_words_backing_voices_and_translations() {
        let source = r#"<tt xmlns="http://www.w3.org/ns/ttml" xmlns:itunes="http://music.apple.com/lyric-ttml-internal" xmlns:ttm="http://www.w3.org/ns/ttml#metadata" itunes:timing="Word"><head><metadata><ttm:agent type="person" xml:id="v1"/><ttm:agent type="person" xml:id="v2"/><iTunesMetadata xmlns="http://music.apple.com/lyric-ttml-internal"><translations><translation type="subtitle" xml:lang="en-US"><text for="L1">Hello there</text></translation></translations><songwriters><songwriter>Ayase</songwriter></songwriters></iTunesMetadata></metadata></head><body><div><p begin="27.395" end="28.960" itunes:key="L1" ttm:agent="v1"><span begin="27.395" end="27.549">I</span> <span begin="27.549" end="27.740">been</span> <span begin="27.740" end="28.960">call</span><span ttm:role="x-bg"><span begin="28.1" end="28.5">(oh</span> <span begin="28.5" end="29.2">oh)</span></span></p><p begin="0:30.1" end="0:31.0" itunes:key="L2" ttm:agent="v2"><span begin="30.1" end="30.5">無敵の</span><span begin="30.5" end="31.0">笑顔</span></p></div></body></tt>"#;
        let ttml = ttml(source).expect("parses");
        assert_eq!(ttml.songwriters, ["Ayase"]);
        let [first, second] = &ttml.lines[..] else { panic!("two lines: {:?}", ttml.lines) };
        assert_eq!(first.text, "I been call");
        assert_eq!(first.words.iter().map(|w| w.text.as_str()).collect::<Vec<_>>(), ["I ", "been ", "call"]);
        assert_eq!(first.backing.iter().map(|w| w.text.as_str()).collect::<String>(), "(oh oh)");
        assert_eq!(first.translation.as_deref(), Some("Hello there"));
        assert_eq!(first.end, Some(29.2));
        assert_eq!(first.voice, Voice::First);
        assert_eq!(second.text, "無敵の笑顔");
        assert!((second.time - 30.1).abs() < 1e-9);
        assert_eq!(second.voice, Voice::Second);
    }

    #[test]
    fn ttml_singers_apart_and_together() {
        // As Apple Music marks a duet (Shallow): two people, and a group.
        let source = r#"<tt xmlns="http://www.w3.org/ns/ttml" xmlns:ttm="http://www.w3.org/ns/ttml#metadata"><head><metadata><ttm:agent type="person" xml:id="v1"/><ttm:agent type="person" xml:id="v2"/><ttm:agent type="group" xml:id="v3"/><ttm:agent type="other" xml:id="v2000"/></metadata></head><body><div><p begin="1" end="2" ttm:agent="v1">one</p><p begin="3" end="4" ttm:agent="v2">two</p><p begin="5" end="6" ttm:agent="v3">all</p><p begin="7" end="8" ttm:agent="v2000">others</p><p begin="9" end="10">nobody said</p></div></body></tt>"#;
        let voices: Vec<Voice> = ttml(source).expect("parses").lines.iter().map(|l| l.voice).collect();
        assert_eq!(voices, [Voice::First, Voice::Second, Voice::Together, Voice::Together, Voice::First]);
    }

    #[test]
    fn ttml_timed_by_line() {
        let source = r#"<tt xmlns="http://www.w3.org/ns/ttml" itunes:timing="Line" xmlns:itunes="x"><body><div><p begin="1.0" end="2.0">One  line
 here</p></div></body></tt>"#;
        let lines = ttml(source).expect("parses").lines;
        assert_eq!(lines.len(), 1);
        assert_eq!(lines[0].text, "One line here");
        assert!(lines[0].words.is_empty());
    }

    #[test]
    fn lrc_with_words_and_an_offset() {
        let source = "[offset:+0.5]\n[00:27.32] <00:27.32> I've <00:27.60>   <00:27.66> been <00:27.80>\n[bg: <00:28.00> (yeah) <00:28.40>]\n[00:30.00]Plain line\n[00:31.00]<00:31.00>無<00:31.20>敵<00:31.50>";
        let lines = lrc(source);
        assert_eq!(lines.len(), 3);
        assert!((lines[0].time - 26.82).abs() < 1e-6);
        assert_eq!(lines[0].text, "I've been");
        assert_eq!(lines[0].words.len(), 2);
        assert!((lines[0].words[1].start - 27.16).abs() < 1e-6 && (lines[0].words[1].end - 27.30).abs() < 1e-6);
        assert_eq!(lines[0].backing.len(), 1);
        assert_eq!(lines[1].text, "Plain line");
        assert!(lines[1].words.is_empty());
        assert_eq!(lines[2].text, "無敵");
    }

    #[test]
    fn backing_vocals_in_brackets_go_under_their_line() {
        // Timed by the word, as Musixmatch writes it.
        let mut lines = lrc("[02:20.00] <02:20.00> say <02:20.50> <02:20.50> it <02:21.00> <02:21.20> (say <02:21.60> <02:21.60> it) <02:22.00>");
        split_backing(&mut lines[0]);
        assert_eq!(lines[0].text, "say it");
        assert_eq!(lines[0].words.len(), 2);
        assert_eq!(lines[0].backing.iter().map(|w| w.text.as_str()).collect::<String>(), "(say it)");
        assert_eq!(lines[0].end, Some(142.0));
        // Timed by the line.
        let mut line = LyricLine { time: 5.0, text: "I could never (say it on the phone)".into(), ..LyricLine::default() };
        split_backing(&mut line);
        assert_eq!(line.text, "I could never");
        assert_eq!(line.backing[0].text, "(say it on the phone)");
        // Full-width brackets, as Japanese lyrics write them.
        let mut line = LyricLine { time: 1.0, text: "君の名前（名前）".into(), ..LyricLine::default() };
        split_backing(&mut line);
        assert_eq!((line.text.as_str(), line.backing[0].text.as_str()), ("君の名前", "（名前）"));
        // A line all in brackets, or with them in its middle, stays as it is.
        for text in ["(Ooh, ooh)", "Oh (yeah) I know", "Na (na (na))"] {
            let mut line = LyricLine { time: 1.0, text: text.into(), ..LyricLine::default() };
            split_backing(&mut line);
            let expected_backing = usize::from(text == "Na (na (na))");
            assert_eq!(line.backing.len(), expected_backing, "{text}");
        }
    }

    #[test]
    fn qrc_lines_and_credits() {
        let source = "<?xml version=\"1.0\"?><QrcInfos><LyricInfo><Lyric_1 LyricType=\"1\" LyricContent=\"[ti:アイドル]\n[0,425]ア(0,38)イ(39,38)ド(78,38)ル(116,38) - (155,38)YOASOBI(194,38)\n[426,115]词(426,38)：(465,38)Ayase(504,38)\n[742,2503]無(742,175)敵(917,316) (1233,10)Smile &amp; (1243,100)\n\"/></LyricInfo></QrcInfos>";
        let lines = drop_credits(qrc(source), "アイドル", "YOASOBI");
        assert_eq!(lines.len(), 1, "{lines:?}");
        assert_eq!(lines[0].text, "無敵 Smile &");
        assert!((lines[0].time - 0.742).abs() < 1e-9);
        assert_eq!(lines[0].end, Some(3.245));
        assert!((lines[0].words[1].end - 1.233).abs() < 1e-9);
    }
}

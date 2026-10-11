//! Readings for Japanese lyrics: how each word sounds, set small over it,
//! in romaji or in hiragana (furigana).
//!
//! The words come from a table made from JMdict (assets/lyrics), read
//! longest match first: a word written in kanji takes the reading the
//! dictionary gives it, and kana reads as itself. No network, and no
//! context, so a kanji with several readings takes its most usual one.

use std::sync::OnceLock;

/// The table, compressed (assets/lyrics/README.md says what is in it).
const TABLE: &[u8] = include_bytes!("../assets/lyrics/ja-readings.deflate");

/// The longest word in the table, in characters.
const LONGEST: usize = 10;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub enum Style {
    /// "kimi", for those who do not read kana.
    #[default]
    Romaji,
    /// "きみ", over the kanji only.
    Kana,
}

/// A reading set over part of a line, by character index.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Ruby {
    pub start: usize,
    pub end: usize,
    pub text: String,
}

pub struct Dictionary {
    text: String,
    /// Where each line of `text` starts.
    starts: Vec<u32>,
}

impl Dictionary {
    fn load() -> Option<Self> {
        let bytes = miniz_oxide::inflate::decompress_to_vec(TABLE).ok()?;
        let text = String::from_utf8(bytes).ok()?;
        let mut starts = vec![0u32];
        starts.extend(text.match_indices('\n').map(|(i, _)| i as u32 + 1).filter(|&i| (i as usize) < text.len()));
        Some(Self { text, starts })
    }

    fn line(&self, i: usize) -> (&str, &str) {
        let start = self.starts[i] as usize;
        let end = self.starts.get(i + 1).map_or(self.text.len(), |&e| e as usize);
        let line = self.text[start..end].trim_end_matches('\n');
        line.split_once('\t').unwrap_or((line, ""))
    }

    /// The word's reading: the word itself for kana, which the table keeps
    /// without one.
    pub fn get<'a>(&'a self, word: &'a str) -> Option<&'a str> {
        let mut lo = 0;
        let mut hi = self.starts.len();
        while lo < hi {
            let mid = (lo + hi) / 2;
            let (surface, reading) = self.line(mid);
            match surface.as_bytes().cmp(word.as_bytes()) {
                std::cmp::Ordering::Less => lo = mid + 1,
                std::cmp::Ordering::Greater => hi = mid,
                std::cmp::Ordering::Equal => return Some(if reading.is_empty() { word } else { reading }),
            }
        }
        None
    }
}

static DICTIONARY: OnceLock<Option<Dictionary>> = OnceLock::new();
static LOADING: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// The dictionary once it is in memory (some 2 MB). The first call starts
/// reading it on a thread of its own and returns None; `ready` runs when
/// it is there.
pub fn dictionary(ready: impl FnOnce() + Send + 'static) -> Option<&'static Dictionary> {
    if let Some(dictionary) = DICTIONARY.get() {
        return dictionary.as_ref();
    }
    if !LOADING.swap(true, std::sync::atomic::Ordering::SeqCst) {
        let _ = std::thread::Builder::new().name("readings".into()).spawn(move || {
            let _ = DICTIONARY.set(Dictionary::load());
            ready();
        });
    }
    None
}

/// Whether lyrics are Japanese: any kana in them. (Kanji alone may as
/// well be Chinese.)
pub fn is_japanese(lines: &[&str]) -> bool {
    lines.iter().any(|line| line.chars().any(is_kana))
}

/// The readings for one line.
pub fn readings(dictionary: &Dictionary, line: &str, style: Style) -> Vec<Ruby> {
    let chars: Vec<char> = line.chars().collect();
    let mut words: Vec<(usize, usize, String)> = Vec::new();
    // Kana not found as a word, gathered until something else comes.
    let mut loose: Option<usize> = None;
    let flush = |loose: &mut Option<usize>, end: usize, words: &mut Vec<(usize, usize, String)>| {
        if let Some(start) = loose.take() {
            words.push((start, end, chars[start..end].iter().collect()));
        }
    };
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if !is_kanji(c) && !is_kana(c) {
            flush(&mut loose, i, &mut words);
            i += 1;
            continue;
        }
        let mut longest = (1..=LONGEST.min(chars.len() - i)).rev().find_map(|len| {
            let word: String = chars[i..i + len].iter().collect();
            // Only whole kanji or kana: never a word ending half in a run
            // of Latin letters.
            let reading = dictionary.get(&word)?.to_owned();
            Some((len, reading))
        });
        // A verb or adjective, inflected: the dictionary has it as 無い,
        // 会う, 忘れる; the line says 無く, 会い, 忘れ.
        if is_kanji(c)
            && let Some((len, reading)) = inflected(dictionary, &chars, i)
        {
            let better = match &longest {
                None => true,
                Some((found, _)) => {
                    len > *found
                        || (len == *found && *found == 1 && chars.get(i + 1).is_some_and(|n| is_hiragana(*n) && !PARTICLES.contains(n)))
                }
            };
            if better {
                longest = Some((len, reading));
            }
        }
        match longest {
            Some((len, reading)) => {
                flush(&mut loose, i, &mut words);
                words.push((i, i + len, reading));
                i += len;
            }
            None if is_kanji(c) => {
                // A kanji the table does not know: no reading over it.
                flush(&mut loose, i, &mut words);
                i += 1;
            }
            None => {
                loose.get_or_insert(i);
                i += 1;
            }
        }
    }
    flush(&mut loose, chars.len(), &mut words);

    let mut rubies = Vec::new();
    for (start, end, reading) in words {
        let surface: String = chars[start..end].iter().collect();
        let has_kanji = surface.chars().any(is_kanji);
        match style {
            Style::Romaji => {
                // Particles read otherwise than they are written.
                let text = match surface.as_str() {
                    "は" => "wa".to_owned(),
                    "へ" => "e".to_owned(),
                    "を" => "o".to_owned(),
                    _ => romaji(&reading),
                };
                if !text.is_empty() {
                    rubies.push(Ruby { start, end, text });
                }
            }
            Style::Kana if has_kanji => {
                // Over the kanji alone: kana before and after them is
                // already read as written.
                let surface_chars: Vec<char> = surface.chars().map(hiragana).collect();
                let reading_chars: Vec<char> = reading.chars().map(hiragana).collect();
                let lead = surface_chars
                    .iter()
                    .zip(&reading_chars)
                    .take_while(|(a, b)| a == b && is_kana(**a))
                    .count();
                let tail = surface_chars[lead..]
                    .iter()
                    .rev()
                    .zip(reading_chars[lead..].iter().rev())
                    .take_while(|(a, b)| a == b && is_kana(**a))
                    .count();
                let core_end = surface_chars.len() - tail;
                let text: String = reading_chars[lead..reading_chars.len() - tail].iter().collect();
                if lead < core_end && !text.is_empty() {
                    rubies.push(Ruby { start: start + lead, end: start + core_end, text });
                }
            }
            Style::Kana => {}
        }
    }
    rubies
}

/// The endings of the forms the dictionary lists verbs and adjectives by.
const ENDINGS: [char; 10] = ['る', 'う', 'く', 'ぐ', 'す', 'つ', 'ぬ', 'ぶ', 'む', 'い'];

/// Kana that follow a noun rather than end a verb's stem.
const PARTICLES: [char; 13] = ['は', 'が', 'を', 'に', 'で', 'と', 'も', 'の', 'へ', 'や', 'か', 'よ', 'ね'];

/// The longest stretch from `i` (kanji, then its kana) that is a listed
/// verb or adjective less its ending, and its reading less the ending.
fn inflected(dictionary: &Dictionary, chars: &[char], i: usize) -> Option<(usize, String)> {
    (1..=4.min(chars.len() - i)).rev().find_map(|len| {
        let stem = &chars[i..i + len];
        if !stem.iter().all(|c| is_kanji(*c) || is_hiragana(*c)) {
            return None;
        }
        let stem: String = stem.iter().collect();
        ENDINGS.iter().find_map(|ending| {
            let listed = format!("{stem}{ending}");
            let reading = dictionary.get(&listed)?.strip_suffix(*ending)?.to_owned();
            (!reading.is_empty()).then_some((len, reading))
        })
    })
}

fn is_hiragana(c: char) -> bool {
    matches!(c as u32, 0x3041..=0x3096)
}

pub fn is_kana(c: char) -> bool {
    matches!(c as u32, 0x3041..=0x309F | 0x30A0..=0x30FF | 0xFF66..=0xFF9F)
}

pub fn is_kanji(c: char) -> bool {
    matches!(c as u32, 0x4E00..=0x9FFF | 0x3400..=0x4DBF | 0xF900..=0xFAFF) || c == '々' || c == '〆' || c == 'ヶ'
}

/// Katakana as hiragana; everything else as it is.
fn hiragana(c: char) -> char {
    match c as u32 {
        0x30A1..=0x30F6 => char::from_u32(c as u32 - 0x60).unwrap_or(c),
        _ => c,
    }
}

/// Kana in romaji (Hepburn, long vowels written out: "kokudou").
pub fn romaji(kana: &str) -> String {
    let chars: Vec<char> = kana.chars().map(hiragana).collect();
    let mut out = String::new();
    let mut double = false;
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if c == 'っ' {
            double = true;
            i += 1;
            continue;
        }
        if c == 'ー' {
            // The vowel before, held.
            if let Some(vowel) = out.chars().rev().find(|c| "aiueo".contains(*c)) {
                out.push(vowel);
            }
            i += 1;
            continue;
        }
        // Two kana as one sound first (きゃ, ふぁ), then one.
        let pair = chars.get(i + 1).and_then(|&next| syllable2(c, next));
        let (sound, used) = match pair {
            Some(sound) => (sound, 2),
            None => match syllable(c) {
                Some(sound) => (sound, 1),
                None => {
                    out.push(c);
                    i += 1;
                    continue;
                }
            },
        };
        if std::mem::take(&mut double) {
            if sound.starts_with("ch") {
                out.push('t');
            } else if let Some(first) = sound.chars().next().filter(|c| !"aiueon".contains(*c)) {
                out.push(first);
            }
        }
        out.push_str(sound);
        i += used;
    }
    out
}

fn syllable(c: char) -> Option<&'static str> {
    Some(match c {
        'あ' => "a", 'い' => "i", 'う' => "u", 'え' => "e", 'お' => "o",
        'か' => "ka", 'き' => "ki", 'く' => "ku", 'け' => "ke", 'こ' => "ko",
        'さ' => "sa", 'し' => "shi", 'す' => "su", 'せ' => "se", 'そ' => "so",
        'た' => "ta", 'ち' => "chi", 'つ' => "tsu", 'て' => "te", 'と' => "to",
        'な' => "na", 'に' => "ni", 'ぬ' => "nu", 'ね' => "ne", 'の' => "no",
        'は' => "ha", 'ひ' => "hi", 'ふ' => "fu", 'へ' => "he", 'ほ' => "ho",
        'ま' => "ma", 'み' => "mi", 'む' => "mu", 'め' => "me", 'も' => "mo",
        'や' => "ya", 'ゆ' => "yu", 'よ' => "yo",
        'ら' => "ra", 'り' => "ri", 'る' => "ru", 'れ' => "re", 'ろ' => "ro",
        'わ' => "wa", 'ゐ' => "i", 'ゑ' => "e", 'を' => "o", 'ん' => "n",
        'が' => "ga", 'ぎ' => "gi", 'ぐ' => "gu", 'げ' => "ge", 'ご' => "go",
        'ざ' => "za", 'じ' => "ji", 'ず' => "zu", 'ぜ' => "ze", 'ぞ' => "zo",
        'だ' => "da", 'ぢ' => "ji", 'づ' => "zu", 'で' => "de", 'ど' => "do",
        'ば' => "ba", 'び' => "bi", 'ぶ' => "bu", 'べ' => "be", 'ぼ' => "bo",
        'ぱ' => "pa", 'ぴ' => "pi", 'ぷ' => "pu", 'ぺ' => "pe", 'ぽ' => "po",
        'ぁ' => "a", 'ぃ' => "i", 'ぅ' => "u", 'ぇ' => "e", 'ぉ' => "o",
        'ゃ' => "ya", 'ゅ' => "yu", 'ょ' => "yo", 'ゎ' => "wa", 'ゔ' => "vu",
        _ => return None,
    })
}

fn syllable2(c: char, next: char) -> Option<&'static str> {
    Some(match (c, next) {
        ('き', 'ゃ') => "kya", ('き', 'ゅ') => "kyu", ('き', 'ょ') => "kyo",
        ('し', 'ゃ') => "sha", ('し', 'ゅ') => "shu", ('し', 'ょ') => "sho", ('し', 'ぇ') => "she",
        ('ち', 'ゃ') => "cha", ('ち', 'ゅ') => "chu", ('ち', 'ょ') => "cho", ('ち', 'ぇ') => "che",
        ('に', 'ゃ') => "nya", ('に', 'ゅ') => "nyu", ('に', 'ょ') => "nyo",
        ('ひ', 'ゃ') => "hya", ('ひ', 'ゅ') => "hyu", ('ひ', 'ょ') => "hyo",
        ('み', 'ゃ') => "mya", ('み', 'ゅ') => "myu", ('み', 'ょ') => "myo",
        ('り', 'ゃ') => "rya", ('り', 'ゅ') => "ryu", ('り', 'ょ') => "ryo",
        ('ぎ', 'ゃ') => "gya", ('ぎ', 'ゅ') => "gyu", ('ぎ', 'ょ') => "gyo",
        ('じ', 'ゃ') => "ja", ('じ', 'ゅ') => "ju", ('じ', 'ょ') => "jo", ('じ', 'ぇ') => "je",
        ('ぢ', 'ゃ') => "ja", ('ぢ', 'ゅ') => "ju", ('ぢ', 'ょ') => "jo",
        ('び', 'ゃ') => "bya", ('び', 'ゅ') => "byu", ('び', 'ょ') => "byo",
        ('ぴ', 'ゃ') => "pya", ('ぴ', 'ゅ') => "pyu", ('ぴ', 'ょ') => "pyo",
        ('ふ', 'ぁ') => "fa", ('ふ', 'ぃ') => "fi", ('ふ', 'ぇ') => "fe", ('ふ', 'ぉ') => "fo",
        ('て', 'ぃ') => "ti", ('で', 'ぃ') => "di", ('と', 'ぅ') => "tu", ('ど', 'ぅ') => "du",
        ('う', 'ぃ') => "wi", ('う', 'ぇ') => "we", ('う', 'ぉ') => "wo",
        ('ゔ', 'ぁ') => "va", ('ゔ', 'ぃ') => "vi", ('ゔ', 'ぇ') => "ve", ('ゔ', 'ぉ') => "vo",
        ('つ', 'ぁ') => "tsa", ('つ', 'ぃ') => "tsi", ('つ', 'ぇ') => "tse", ('つ', 'ぉ') => "tso",
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn table() -> &'static Dictionary {
        // Tests run at once: whichever asks first loads it, the rest wait.
        for _ in 0..200 {
            if let Some(dictionary) = dictionary(|| {}) {
                return dictionary;
            }
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        panic!("the table does not load");
    }

    fn line(text: &str, style: Style) -> Vec<(String, String)> {
        let chars: Vec<char> = text.chars().collect();
        readings(table(), text, style)
            .into_iter()
            .map(|r| (chars[r.start..r.end].iter().collect(), r.text))
            .collect()
    }

    #[test]
    fn kana_in_romaji() {
        assert_eq!(romaji("きみ"), "kimi");
        assert_eq!(romaji("こくどう"), "kokudou");
        assert_eq!(romaji("きって"), "kitte");
        assert_eq!(romaji("まっちゃ"), "matcha");
        assert_eq!(romaji("ツバサ"), "tsubasa");
        assert_eq!(romaji("ラーメン"), "raamen");
        assert_eq!(romaji("しゅうえん"), "shuuen");
    }

    #[test]
    fn a_line_in_romaji() {
        let words = line("細い抜け道 君が呟く", Style::Romaji);
        let read: Vec<&str> = words.iter().map(|(_, r)| r.as_str()).collect();
        assert_eq!(read, ["hosoi", "nukemichi", "kimi", "ga", "tsubuyaku"]);
        let words = line("君は涙を流れた", Style::Romaji);
        assert!(words.contains(&("は".to_owned(), "wa".to_owned())));
        assert!(words.contains(&("を".to_owned(), "o".to_owned())));
        assert!(words.contains(&("涙".to_owned(), "namida".to_owned())));
    }

    #[test]
    fn inflected_words_read_as_their_listed_forms() {
        let words = line("夢追う事に恐れは無くて", Style::Romaji);
        assert!(words.iter().any(|(w, r)| w.starts_with('無') && r.starts_with("na")), "{words:?}");
        let words = line("いつも忘れない", Style::Romaji);
        assert!(words.contains(&("忘れ".to_owned(), "wasure".to_owned())), "{words:?}");
        let words = line("いつか会いに来る", Style::Romaji);
        assert!(words.contains(&("会".to_owned(), "a".to_owned())), "{words:?}");
        // A noun before a particle stays a noun.
        let words = line("話は絶えず", Style::Romaji);
        assert!(words.contains(&("話".to_owned(), "hanashi".to_owned())), "{words:?}");
    }

    #[test]
    fn furigana_over_the_kanji_alone() {
        let words = line("見送る為の言葉", Style::Kana);
        assert!(words.contains(&("見送".to_owned(), "みおく".to_owned())), "{words:?}");
        assert!(words.contains(&("言葉".to_owned(), "ことば".to_owned())), "{words:?}");
        assert!(words.iter().all(|(surface, _)| surface.chars().any(is_kanji)));
    }

    #[test]
    fn only_japanese_counts() {
        assert!(is_japanese(&["君が呟く"]));
        assert!(!is_japanese(&["自由的你", "hello"]));
    }
}

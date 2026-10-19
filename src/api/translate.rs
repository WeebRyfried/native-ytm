//! Lines of lyrics put into English, for lyrics that come without a
//! translation of their own: Unison's translations, Better Lyrics'
//! community service, as its extension asks for them.

use std::time::Duration;

use anyhow::Result;
use serde_json::Value;

const UNISON: &str = "https://unison.betterlyrics.org/translate";

/// The language translated into.
pub const INTO: &str = "en";

/// Each line's translation (none where it needs none), and the language
/// the lines were found to be in.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Translation {
    pub from: Option<String>,
    pub lines: Vec<Option<String>>,
}

/// The lines in English.
pub async fn translate(http: &reqwest::Client, lines: &[String], video_id: Option<&str>) -> Result<Translation> {
    // No id at all rather than an empty one, which Unison turns away.
    let mut body = serde_json::json!({ "lines": lines, "to": INTO });
    if let Some(id) = video_id {
        body["videoId"] = id.into();
    }
    let answer: Value = http
        .post(UNISON)
        .json(&body)
        .timeout(Duration::from_secs(20))
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    let said = answer["lines"].as_array().filter(|l| l.len() == lines.len()).ok_or_else(|| anyhow::anyhow!("an answer that does not fit the lines"))?;
    Ok(Translation {
        from: answer["detectedLang"].as_str().filter(|l| !l.is_empty()).map(str::to_owned),
        lines: lines
            .iter()
            .zip(said)
            .map(|(line, said)| {
                let needed = said["needsTranslation"].as_bool().unwrap_or(true);
                said["translation"].as_str().filter(|_| needed).and_then(|t| differs(line, t))
            })
            .collect(),
    })
}

/// A translation, unless it says the same as the line.
fn differs(line: &str, said: &str) -> Option<String> {
    let said = said.trim();
    (!said.is_empty() && said.to_lowercase() != line.trim().to_lowercase()).then(|| said.to_owned())
}

/// Whether lyrics may be in another language than English, worth asking
/// to translate: any letter beyond plain ASCII (kana, hangul, an accent).
pub fn worth_asking(lines: &[String]) -> bool {
    lines.iter().any(|l| l.chars().any(|c| c.is_alphabetic() && !c.is_ascii()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn what_is_worth_translating() {
        assert_eq!(differs("Hello", " hello "), None);
        assert_eq!(differs("こんにちは", "Hello"), Some("Hello".into()));
        assert!(worth_asking(&["はい".into()]));
        assert!(worth_asking(&["canción".into()]));
        assert!(!worth_asking(&["I've been tryna call".into(), "(oh)".into()]));
    }

    /// Lines put into English by the app's own code (dev): `NATIVE_YTM_TRANSLATE=1`.
    #[test]
    #[ignore]
    fn translate_live() {
        if std::env::var_os("NATIVE_YTM_TRANSLATE").is_none() {
            return;
        }
        let runtime = tokio::runtime::Builder::new_multi_thread().enable_all().build().unwrap();
        let http = reqwest::Client::builder().gzip(true).build().unwrap();
        let lines: Vec<String> = ["教室の窓越しに ぼんやりと空に問う", "何のため ボクは生きているの？", "Hello there"].map(String::from).to_vec();
        println!("unison: {:?}", runtime.block_on(translate(&http, &lines, None)).map_err(|e| format!("{e:#}")));
    }
}

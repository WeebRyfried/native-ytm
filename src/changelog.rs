//! What changed in each version, for the note after an update: the
//! changelog kept beside the code (CHANGELOG.md), built into the program,
//! so it is there offline and covers every version skipped.

/// One version's changes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Release {
    /// "0.3.8".
    pub version: String,
    /// When it came out, as written ("21 October 2026").
    pub date: String,
    /// What it is about, in a sentence.
    pub summary: String,
    /// The changes, each a paragraph, its lead in **bold**.
    pub changes: Vec<String>,
}

/// The changelog as shown over the window: after an update (what is new
/// since the version before), or asked for (every version).
#[derive(Clone, Debug)]
pub struct Shown {
    pub releases: Vec<Release>,
    pub after_update: bool,
}

/// Every release in the changelog, newest first.
pub fn releases() -> Vec<Release> {
    parse(include_str!("../CHANGELOG.md"))
}

/// Reads a changelog: a `## version — date` heading for each release, a
/// paragraph saying what it is about, and its changes as a list (a line
/// indented under one carries it on). Anything before the first heading is
/// for people reading the file.
pub fn parse(text: &str) -> Vec<Release> {
    let mut releases: Vec<Release> = Vec::new();
    for line in text.lines() {
        if let Some(heading) = line.strip_prefix("## ") {
            let (version, date) = heading.split_once('\u{2014}').unwrap_or((heading, ""));
            releases.push(Release {
                version: version.trim().trim_start_matches('v').to_owned(),
                date: date.trim().to_owned(),
                summary: String::new(),
                changes: Vec::new(),
            });
            continue;
        }
        let Some(release) = releases.last_mut() else {
            continue;
        };
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        if let Some(change) = line.strip_prefix("- ") {
            release.changes.push(change.trim().to_owned());
        } else if line.starts_with(' ') && !release.changes.is_empty() {
            let last = release.changes.last_mut().expect("checked");
            last.push(' ');
            last.push_str(trimmed);
        } else if release.changes.is_empty() {
            if !release.summary.is_empty() {
                release.summary.push(' ');
            }
            release.summary.push_str(trimmed);
        }
    }
    releases
}

/// What is new for someone moving from `had` to `now`: the releases after
/// the one they had, up to this one, newest first. Without knowing what
/// they had (0.3.7 and before did not say), this one's alone.
pub fn since(releases: &[Release], had: Option<&str>, now: &str) -> Vec<Release> {
    releases
        .iter()
        .filter(|r| !crate::update::newer(&r.version, now))
        .filter(|r| match had {
            Some(had) => crate::update::newer(&r.version, had),
            None => r.version == now,
        })
        .cloned()
        .collect()
}

/// A change split where its **bold** starts and stops: each piece, and
/// whether it is bold.
pub fn pieces(text: &str) -> Vec<(&str, bool)> {
    text.split("**").enumerate().filter(|(_, piece)| !piece.is_empty()).map(|(i, piece)| (piece, i % 2 == 1)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const TEXT: &str = "# Changelog\n\nAbout this file.\n\n## 0.3.8 \u{2014} 21 October 2026\n\nThe note,\nand more.\n\n- **Song changes.** A note at the corner,\n  for a few seconds.\n- A smaller one.\n\n## 0.3.7 \u{2014} 20 October 2026\n\nLines together.\n\n- **More than one line.** Lit.\n\n## 0.3.6 \u{2014} 19 October 2026\n\nDuets.\n";

    #[test]
    fn reads_each_release_with_its_summary_and_changes() {
        let releases = parse(TEXT);
        assert_eq!(releases.len(), 3);
        assert_eq!(releases[0].version, "0.3.8");
        assert_eq!(releases[0].date, "21 October 2026");
        assert_eq!(releases[0].summary, "The note, and more.");
        assert_eq!(releases[0].changes, ["**Song changes.** A note at the corner, for a few seconds.", "A smaller one."]);
        assert_eq!(releases[2].changes, Vec::<String>::new());
    }

    #[test]
    fn what_is_new_is_what_came_after_the_version_had() {
        let releases = parse(TEXT);
        let versions = |had, now| since(&releases, had, now).into_iter().map(|r| r.version).collect::<Vec<_>>();
        assert_eq!(versions(Some("0.3.6"), "0.3.8"), ["0.3.8", "0.3.7"]);
        assert_eq!(versions(Some("0.3.7"), "0.3.8"), ["0.3.8"]);
        // Not known: this version's alone.
        assert_eq!(versions(None, "0.3.8"), ["0.3.8"]);
        // Nothing newer than the one running, even when the file has it.
        assert_eq!(versions(Some("0.3.6"), "0.3.7"), ["0.3.7"]);
        assert!(versions(Some("0.3.8"), "0.3.8").is_empty());
    }

    #[test]
    fn bold_leads_are_told_apart() {
        assert_eq!(pieces("**Song changes.** A note."), [("Song changes.", true), (" A note.", false)]);
        assert_eq!(pieces("Plain."), [("Plain.", false)]);
        assert_eq!(pieces("A **bold** word"), [("A ", false), ("bold", true), (" word", false)]);
    }

    #[test]
    fn the_changelog_built_in_has_this_version() {
        let releases = releases();
        assert_eq!(releases.first().map(|r| r.version.as_str()), Some(env!("CARGO_PKG_VERSION")), "CHANGELOG.md has this version first");
        assert!(releases.iter().all(|r| !r.summary.is_empty() && !r.date.is_empty() && !r.changes.is_empty()));
    }
}

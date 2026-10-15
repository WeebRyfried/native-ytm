//! Pages kept on disk as they last loaded (Home, Explore, the library's
//! own), so the next start shows them at once while the fresh copy comes.

use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::api::model::Page;

/// A kept page older than this is not shown: a fresh one is waited for.
const FRESH_FOR_SECS: i64 = 14 * 24 * 60 * 60;

/// The pages kept, by their route's key: the ones that open the app or
/// that people go back to, and change only a little between visits.
const KEPT: [&str; 7] = [
    "home",
    "explore",
    "library:FEmusic_liked_playlists",
    "library:FEmusic_liked_videos",
    "library:FEmusic_liked_albums",
    "library:FEmusic_library_corpus_track_artists",
    "library:FEmusic_library_corpus_artists",
];

#[derive(Serialize, Deserialize)]
struct Kept {
    page: Page,
    /// When it loaded, in Unix seconds.
    at: i64,
}

pub struct KeptPages {
    dir: PathBuf,
}

impl KeptPages {
    pub fn new(dir: PathBuf) -> Self {
        let _ = std::fs::create_dir_all(&dir);
        Self { dir }
    }

    /// Whether the page with this key is one kept.
    pub fn keeps(key: &str) -> bool {
        KEPT.contains(&key)
    }

    /// One file per page and context: signed in or not, the language and
    /// the region, as a page differs by them.
    fn path(&self, key: &str, context: &str) -> PathBuf {
        let hash = Sha256::digest(format!("{key}|{context}").as_bytes());
        let name: String = hash.iter().take(12).map(|b| format!("{b:02x}")).collect();
        self.dir.join(format!("{name}.json"))
    }

    pub fn get(&self, key: &str, context: &str) -> Option<Page> {
        if !Self::keeps(key) {
            return None;
        }
        let bytes = std::fs::read(self.path(key, context)).ok()?;
        let kept: Kept = serde_json::from_slice(&bytes).ok()?;
        (unix_now() - kept.at <= FRESH_FOR_SECS).then_some(kept.page)
    }

    /// Keeps the page, on a thread of its own (a Home page is some
    /// hundred kilobytes of JSON).
    pub fn put(&self, key: &str, context: &str, page: &Page) {
        if !Self::keeps(key) {
            return;
        }
        let path = self.path(key, context);
        let kept = Kept { page: page.clone(), at: unix_now() };
        let _ = std::thread::Builder::new().name("kept-page".into()).spawn(move || {
            if let Ok(json) = serde_json::to_vec(&kept) {
                crate::settings::write_atomic(&path, &json);
            }
        });
    }

    /// Forgets every page: another account's would show otherwise.
    pub fn clear(&self) {
        if let Ok(entries) = std::fs::read_dir(&self.dir) {
            for entry in entries.flatten() {
                let _ = std::fs::remove_file(entry.path());
            }
        }
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }
}

fn unix_now() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs() as i64)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::model::{Header, Section};

    #[test]
    fn keeps_the_pages_it_should_by_context() {
        let dir = std::env::temp_dir().join(format!("native-ytm-pages-{}", std::process::id()));
        let kept = KeptPages::new(dir.clone());
        let page = Page {
            header: Some(Header { title: "Home".into(), ..Header::default() }),
            sections: vec![Section {
                title: "Quick picks".into(),
                strapline: None,
                layout: crate::api::model::Layout::List,
                items: Vec::new(),
                more: None,
                text: None,
                continuation: Some("token".into()),
            }],
            ..Page::default()
        };
        kept.put("home", "in|en|", &page);
        kept.put("browse:MPREb_x:", "in|en|", &page);
        // Written on a thread.
        let mut found = None;
        for _ in 0..100 {
            found = kept.get("home", "in|en|");
            if found.is_some() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        assert_eq!(found, Some(page));
        assert_eq!(kept.get("home", "out|en|"), None);
        assert_eq!(kept.get("browse:MPREb_x:", "in|en|"), None);
        kept.clear();
        assert_eq!(kept.get("home", "in|en|"), None);
        let _ = std::fs::remove_dir_all(&dir);
    }
}

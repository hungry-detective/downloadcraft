//! Crash-safe resume state.
//!
//! Next to the download's data file (`<file>.dcdata`) the engine keeps a small
//! journal (`<file>.dcjournal`) listing the byte ranges that are still
//! missing. It is rewritten atomically every few seconds and on pause or
//! failure, after the data written so far has been flushed to disk, so a
//! crash or power cut loses at most the last few seconds.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

const VERSION: u32 = 1;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) struct Journal {
    pub v: u32,
    /// What the bytes belong to (the job's resume key, normally its URL).
    pub key: String,
    pub total: u64,
    pub etag: Option<String>,
    pub last_modified: Option<String>,
    /// Unfinished `[from, to)` ranges.
    pub remaining: Vec<(u64, u64)>,
}

impl Journal {
    pub fn new(
        key: &str,
        total: u64,
        etag: Option<String>,
        last_modified: Option<String>,
        remaining: Vec<(u64, u64)>,
    ) -> Self {
        Self { v: VERSION, key: key.to_string(), total, etag, last_modified, remaining }
    }

    pub fn load(path: &Path) -> Option<Self> {
        let raw = std::fs::read(path).ok()?;
        let j: Self = serde_json::from_slice(&raw).ok()?;
        (j.v == VERSION).then_some(j)
    }

    /// Write to a temporary file and rename over the old journal, so a crash
    /// mid-write never leaves a half journal behind.
    pub fn save(&self, path: &Path) -> std::io::Result<()> {
        let tmp = with_suffix(path, ".tmp");
        std::fs::write(&tmp, serde_json::to_vec(self).map_err(std::io::Error::other)?)?;
        std::fs::rename(&tmp, path)
    }
}

pub(crate) fn with_suffix(path: &Path, suffix: &str) -> PathBuf {
    let mut s = path.as_os_str().to_os_string();
    s.push(suffix);
    PathBuf::from(s)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn save_and_load() {
        let dir = std::env::temp_dir().join(format!("dce-journal-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("a.bin.dcjournal");
        let j = Journal::new("https://x/a.bin", 100, Some("\"e\"".into()), None, vec![(10, 20), (50, 100)]);
        j.save(&p).unwrap();
        assert_eq!(Journal::load(&p), Some(j));
        // A partial moved to another name keeps its bytes under the new key.
        assert!(crate::rekey_journal(&p, "C:/Downloads/a.bin"));
        let moved = Journal::load(&p).unwrap();
        assert_eq!(moved.key, "C:/Downloads/a.bin");
        assert_eq!(moved.remaining, vec![(10, 20), (50, 100)]);
        assert!(!crate::rekey_journal(&p, "C:/Downloads/a.bin"));
        std::fs::write(&p, b"{not json").unwrap();
        assert_eq!(Journal::load(&p), None);
        let _ = std::fs::remove_dir_all(dir);
    }
}

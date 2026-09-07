//! The copy of the index this machine keeps.
//!
//! Two files beside each other: the index as it arrived, and the `ETag` the
//! registry gave it. The next fetch sends the tag back, and a registry that
//! has published nothing since answers `304` and no bytes — which is what
//! makes opening the store page cheap enough to do on every visit without
//! anybody thinking about it.
//!
//! # Under the data directory rather than the cache one
//!
//! A cache directory is a directory the operating system, or a person with a
//! disk that is filling up, is entitled to empty. This file is not only a
//! speed-up: it is also the only record of which versions the registry has
//! **withdrawn**, and that has to survive a launch with no network and a
//! spring clean. So it lives where the plugins themselves live.
//!
//! # It is never load-bearing
//!
//! Every failure here answers `None` or a line in the log, never a refusal to
//! start: an unreadable index is a store page that says it has not managed to
//! fetch one, and nothing else in the window changes.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use super::index::{Index, parse};

/// What the index is called on disk.
const INDEX_FILE: &str = "index.json";

/// And the tag that says which version of it this is.
const ETAG_FILE: &str = "index.etag";

/// Where this machine keeps what it knows about the registry.
pub struct Cache {
    directory: PathBuf,
}

/// An index read back off the disk.
pub struct Cached {
    /// What the registry published.
    pub index: Index,
    /// What to send back, so that an unchanged index costs no bytes.
    pub etag: Option<String>,
    /// When the file was last written, for a page that has to say how old
    /// this answer is.
    pub fetched: Option<SystemTime>,
}

impl Cache {
    /// The one under the platform's data directory, beside the plugins.
    pub fn user() -> Option<Self> {
        super::super::wasm::directory()
            .and_then(|plugins| plugins.parent().map(Path::to_path_buf))
            .map(|crook| Self {
                directory: crook.join("store"),
            })
    }

    /// One at a named directory, which is what the tests use.
    pub fn at(directory: impl Into<PathBuf>) -> Self {
        Self {
            directory: directory.into(),
        }
    }

    /// What is on disk, if any of it can be read.
    ///
    /// A file that will not parse is answered as nothing at all rather than as
    /// an error: it was written by a fetch that has already happened, so there
    /// is nobody left to tell, and the next fetch replaces it.
    pub fn read(&self) -> Option<Cached> {
        let path = self.directory.join(INDEX_FILE);
        let bytes = fs::read(&path).ok()?;
        let index = match parse(&bytes) {
            Ok(index) => index,
            Err(why) => {
                log::warn!("{} is not an index this build reads: {why}", path.display());
                return None;
            }
        };

        Some(Cached {
            index,
            etag: fs::read_to_string(self.directory.join(ETAG_FILE))
                .ok()
                .map(|tag| tag.trim().to_owned())
                .filter(|tag| !tag.is_empty()),
            fetched: fs::metadata(&path).and_then(|about| about.modified()).ok(),
        })
    }

    /// Keeps what a fetch answered with.
    ///
    /// The bytes are parsed before either file is written, so a registry
    /// serving something that is not an index cannot replace a good cache with
    /// it — the fetch fails and yesterday's list is still there.
    pub fn write(&self, bytes: &[u8], etag: Option<&str>) -> Result<Index, String> {
        let index = parse(bytes)?;

        fs::create_dir_all(&self.directory)
            .map_err(|why| format!("{} could not be made: {why}", self.directory.display()))?;
        crate::settings::atomic_write(&self.directory.join(INDEX_FILE), bytes)
            .map_err(|why| format!("the index could not be kept: {why}"))?;

        let tag = self.directory.join(ETAG_FILE);
        match etag {
            Some(etag) => crate::settings::atomic_write(&tag, etag.as_bytes())
                .map_err(|why| format!("the tag could not be kept: {why}"))?,
            // A registry that stopped sending one must not leave the last one
            // behind, or the next fetch asks about a version of the file
            // nobody has any more and is answered `304` for it.
            None => {
                let _ = fs::remove_file(&tag);
            }
        }

        Ok(index)
    }
}

#[cfg(test)]
#[path = "cache_tests.rs"]
mod tests;

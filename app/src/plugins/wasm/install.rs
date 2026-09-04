//! Putting a plugin where Crook will find it.
//!
//! Installing is a file copy, and that is the whole design: there is no
//! server, no index and no account. A person downloads one file and it is
//! theirs. `--install-plugin` exists because "copy this to a directory whose
//! path is different on three platforms" is a sentence a README should not
//! have to say twice.
//!
//! # It is read before it is written
//!
//! The module is opened, its ABI checked and its manifest decoded *before*
//! anything is copied — so a file that is not a plugin, or is one for another
//! version of this vocabulary, is refused with a line naming what is wrong
//! rather than installed and then refused on every launch afterwards.
//!
//! The directory is named after the plugin rather than after the file, which
//! is what makes installing the same plugin twice an *upgrade*: the second
//! copy lands on the first. Nothing here reads that name back — the loader
//! finds plugins by looking for the module inside each directory — so it is a
//! convention for people rather than a key.

use std::fs;
use std::path::{Path, PathBuf};

use crook_plugin::PluginId;

use crate::plugin::Plugin as _;

use super::{MODULE_FILE, directory, open};

/// Installs the module at `path`, and answers where it went.
pub fn install(path: &Path) -> Result<PathBuf, String> {
    let plugin = open(path)?;
    let id = plugin.manifest().id.clone();

    let directory = directory()
        .ok_or_else(|| String::from("this machine has no data directory to install into"))?
        .join(folder(&id));

    fs::create_dir_all(&directory)
        .map_err(|why| format!("{} could not be made: {why}", directory.display()))?;

    let installed = directory.join(MODULE_FILE);
    fs::copy(path, &installed)
        .map_err(|why| format!("{} could not be written: {why}", installed.display()))?;

    Ok(installed)
}

/// What a plugin's directory is called.
///
/// `owner/name` is two path components and this is one, because a plugin whose
/// owner had a directory of their own would be a directory that stays behind
/// empty when their last plugin is uninstalled. A dot rather than a dash
/// because an owner may have one of those in their name and a `PluginId`
/// refuses a dot in a part — which is also why this cannot produce `..` and
/// install somewhere else entirely.
fn folder(id: &PluginId) -> String {
    id.as_str().replace('/', ".")
}

#[cfg(test)]
#[path = "install_tests.rs"]
mod tests;

//! Putting a plugin where Crook will find it, and taking it back out.
//!
//! Installing is a file copy, and that is still the whole design: there is no
//! account and nothing to sign in to. What a plugin *comes from* — a download,
//! an index, a person handed it on a stick — is somebody else's business;
//! this is where it lands.
//!
//! # It is read before it is written
//!
//! The module is opened, its ABI checked and its manifest decoded *before*
//! anything is copied — so a file that is not a plugin, or is one for another
//! version of this vocabulary, is refused with a line naming what is wrong
//! rather than installed and then refused on every launch afterwards.
//!
//! # The version is in the path
//!
//! `<data>/crook/plugins/<owner>.<name>/<version>/plugin.wasm`, where the
//! version is what the module's own manifest said. That buys three things a
//! flat `plugin.wasm` could not have: an upgrade that writes somewhere new
//! rather than over the bytes a running interpreter is reading, a directory
//! name a person can look at to see what they have, and an index that can talk
//! about one version of a plugin rather than about a plugin.
//!
//! Only one version is kept. Installing is what a person does when they want
//! *this* one, and a directory that quietly accumulated every version ever
//! installed would be a plugin whose disk use nobody chose. What that costs is
//! rolling back by pressing something, and what it does not cost is rolling
//! back at all: installing the older file is the way back, and it is the same
//! command as installing the newer one.
//!
//! The layout before this one — the module straight in the plugin's directory
//! — is still loaded, because an upgrade of Crook that silently stopped
//! running somebody's plugins would be indistinguishable from losing them. It
//! is migrated the first time that plugin is installed again.

use std::fs;
use std::path::{Path, PathBuf};

use crook_plugin::PluginId;

use crate::plugin::Plugin as _;
use crate::settings::atomic_write;

use super::{MODULE_FILE, directory, newest_module, open};

/// Installs the module at `path`, and answers where it went.
pub fn install(path: &Path) -> Result<PathBuf, String> {
    let root = directory()
        .ok_or_else(|| String::from("this machine has no data directory to install into"))?;
    into(&root, path)
}

/// The same, into a named plugins directory.
pub(super) fn into(root: &Path, path: &Path) -> Result<PathBuf, String> {
    let bytes = fs::read(path).map_err(|why| format!("could not be read: {why}"))?;
    let plugin = open(path)?;
    let manifest = plugin.manifest();
    let version = version_folder(manifest.version)?;

    let home = root.join(folder(&manifest.id));
    let at = home.join(&version);
    fs::create_dir_all(&at).map_err(|why| format!("{} could not be made: {why}", at.display()))?;

    // Written and renamed rather than copied onto: the file being replaced may
    // be the one an interpreter in another Crook is reading this second, and a
    // half-copied module is a plugin that is refused on the next launch with a
    // line about bytes rather than about an install that went wrong.
    let installed = at.join(MODULE_FILE);
    atomic_write(&installed, &bytes)
        .map_err(|why| format!("{} could not be written: {why}", installed.display()))?;

    // Everything this replaces, after the new one is safely down. A failure
    // here leaves an older version beside a newer one, which `newest_module`
    // resolves the same way it resolves everything else — so it is worth a
    // line in the log and not worth failing an install that has succeeded.
    for stale in fs::read_dir(&home).into_iter().flatten().flatten() {
        let path = stale.path();
        if path == at {
            continue;
        }
        let outcome = match path.is_dir() {
            true => fs::remove_dir_all(&path),
            // The layout before versions, and the one moment it is migrated:
            // a flat module left beside the versioned ones is the same plugin
            // twice, contributing to a slot twice.
            false => fs::remove_file(&path),
        };
        if let Err(why) = outcome {
            log::warn!("{} was left behind: {why}", path.display());
        }
    }

    Ok(installed)
}

/// Takes a plugin off this machine, and answers what was removed.
///
/// The whole `<owner>.<name>` directory, not the version inside it: a plugin
/// whose last version was uninstalled is not a plugin with an empty directory,
/// and leaving one behind would put a row on the Plugins page for something
/// that is not there.
pub fn uninstall(id: &PluginId) -> Result<PathBuf, String> {
    let root = directory()
        .ok_or_else(|| String::from("this machine has no data directory to uninstall from"))?;
    from(&root, id)
}

/// The same, out of a named plugins directory.
pub(super) fn from(root: &Path, id: &PluginId) -> Result<PathBuf, String> {
    let home = root.join(folder(id));
    if !home.is_dir() {
        return Err(format!("{id} is not installed"));
    }

    fs::remove_dir_all(&home)
        .map_err(|why| format!("{} could not be removed: {why}", home.display()))?;
    Ok(home)
}

/// Where a plugin's versions live, whether or not any of them are there.
pub fn home(id: &PluginId) -> Option<PathBuf> {
    directory().map(|root| root.join(folder(id)))
}

/// The module a plugin is being loaded from, if it is installed as a file.
///
/// `None` for a plugin that is in the binary, which is most of them, and for
/// one whose directory has nothing in it a version can be read out of.
pub fn module(id: &PluginId) -> Option<PathBuf> {
    newest_module(&home(id)?)
}

/// What a plugin's directory is called.
///
/// `owner/name` is two path components and this is one, because a plugin whose
/// owner had a directory of their own would be a directory that stays behind
/// empty when their last plugin is uninstalled. A dot rather than a dash
/// because an owner may have one of those in their name and a `PluginId`
/// refuses a dot in a part — which is also why this cannot produce `..` and
/// install somewhere else entirely.
pub(super) fn folder(id: &PluginId) -> String {
    id.as_str().replace('/', ".")
}

/// What the version's directory is called, or why that version cannot name
/// one.
///
/// A `PluginId` is checked character by character before it is one; a version
/// is a string a stranger's manifest handed over, and this is the only place
/// it becomes a path. So it is checked here rather than trusted: the set is
/// what a version is written with, and `..`, a separator and an empty string
/// are each a directory somewhere other than the one being installed into.
fn version_folder(version: &str) -> Result<String, String> {
    let usable = !version.is_empty()
        && version.len() <= 64
        && version != "."
        && version != ".."
        // Windows drops a trailing dot from a filename, so a version ending in
        // one names a directory that is not the directory it created — and
        // everything downstream comparing the two is then comparing against a
        // name that is not on disk. No version anybody writes ends in a dot,
        // which makes this cheaper than making every comparison survive one.
        && !version.ends_with('.')
        && version.bytes().all(|byte| {
            byte.is_ascii_alphanumeric()
                || byte == b'.'
                || byte == b'-'
                || byte == b'+'
                || byte == b'_'
        });

    match usable {
        true => Ok(version.to_owned()),
        false => Err(format!(
            "its version {version:?} is not one: letters, digits, `.`, `-`, `+` and `_`, not \
             ending in a dot, and short enough to be a directory name"
        )),
    }
}

#[cfg(test)]
#[path = "install_tests.rs"]
mod tests;

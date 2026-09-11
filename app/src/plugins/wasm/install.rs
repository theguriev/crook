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

use crook_plugin::{Manifest, PluginId};

use crate::plugin::Plugin as _;
use crate::settings::atomic_write;

use super::{MODULE_FILE, directory, newest_module, opened};

/// Installs the module at `path`, and answers where it went.
pub fn install(path: &Path) -> Result<PathBuf, String> {
    let root = directory()
        .ok_or_else(|| String::from("this machine has no data directory to install into"))?;
    into(&root, path)
}

/// The same, into a named plugins directory.
pub(super) fn into(root: &Path, path: &Path) -> Result<PathBuf, String> {
    // Read once, and checked as the bytes that will be *written* rather than
    // as a second read of the same name: a module still being written into
    // place — a `curl -o`, an `scp`, a sync — passes the check as the whole of
    // itself and lands as the half that was there when the first read
    // happened, which is the outcome reading before writing exists to prevent.
    let bytes = fs::read(path).map_err(|why| format!("could not be read: {why}"))?;
    let plugin = opened(&bytes)?;
    write(root, &bytes, plugin.manifest())
}

/// Writes a module somebody has already opened where the loader will find it.
///
/// What the store installs, and it never writes the download to a file first:
/// a temporary file is a temporary file somebody has to remember to remove,
/// and the bytes that were checked are the bytes that should be installed —
/// the same reason [`into`] reads the file once. The manifest arrives with
/// the bytes rather than being read out of them again, because opening a
/// module *runs* it: the caller has already paid for that once to check what
/// it was installing, and a second instantiation would leak a second copy of
/// the manifest for as long as the process lives.
pub fn write(root: &Path, bytes: &[u8], manifest: &Manifest) -> Result<PathBuf, String> {
    let version = version_folder(manifest.version)?;

    let home = root.join(folder(&manifest.id));
    let at = home.join(&version);
    fs::create_dir_all(&at).map_err(|why| format!("{} could not be made: {why}", at.display()))?;

    // Written and renamed rather than copied onto: the file being replaced may
    // be the one an interpreter in another Crook is reading this second, and a
    // half-copied module is a plugin that is refused on the next launch with a
    // line about bytes rather than about an install that went wrong.
    let installed = at.join(MODULE_FILE);
    atomic_write(&installed, bytes)
        .map_err(|why| format!("{} could not be written: {why}", installed.display()))?;

    sweep(&home, &at);

    // What is on disk now is what decides what runs, and it is not always what
    // was just written: a sweep that could not remove the version above this
    // one leaves that one the newest, so a *downgrade* — which is how rolling
    // back works here — would report success and go on running the version
    // somebody was rolling back from.
    match newest_module(&home) {
        Some(newest) if same_file(&newest, &installed) => Ok(installed),
        Some(other) => Err(format!(
            "{} was written, and {} is still there and is what would run",
            installed.display(),
            other.display()
        )),
        None => Err(format!(
            "{} is not there after writing it",
            installed.display()
        )),
    }
}

/// Removes every version of a plugin except the one at `keep`.
///
/// Only one is kept, so this is where an upgrade throws the old one away and
/// where the layout before versions is migrated — a flat module left beside a
/// versioned one is the same plugin found twice, contributing to its slot
/// twice and having its second set of actions refused as already taken.
///
/// A failure is a line in the log rather than a failed install, because the
/// module is already down and the caller checks afterwards whether what is on
/// disk is what will run.
fn sweep(home: &Path, keep: &Path) {
    for stale in fs::read_dir(home).into_iter().flatten().flatten() {
        let path = stale.path();
        // Compared as *places* rather than as strings. `keep` is the name the
        // manifest spelled and this is the name the filesystem stored, and the
        // two differ whenever a filesystem does not keep names the way it was
        // given them: a case-insensitive volume resolving `1.0.0-Beta` onto an
        // existing `1.0.0-beta`, or Windows dropping a trailing dot. Comparing
        // the strings there deletes the directory that was just written into.
        if same_file(&path, keep) {
            continue;
        }
        let outcome = match path.is_dir() {
            true => fs::remove_dir_all(&path),
            false => fs::remove_file(&path),
        };
        if let Err(why) = outcome {
            log::warn!("{} was left behind: {why}", path.display());
        }
    }
}

/// Whether two paths are the same place on disk.
///
/// `canonicalize` is what answers that, and it needs both of them to exist —
/// which they do everywhere this is used. Where one does not, the answer falls
/// back to comparing the paths, which is right whenever the filesystem kept
/// the name it was given.
fn same_file(left: &Path, right: &Path) -> bool {
    match (fs::canonicalize(left), fs::canonicalize(right)) {
        (Ok(left), Ok(right)) => left == right,
        _ => left == right,
    }
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
    uninstall_from(&root, id)
}

/// The same, out of a named plugins directory.
///
/// Public because the window carries the directory it installs into rather
/// than asking for it each time — so that a test can point it at a scratch
/// directory — and removing has to go from the same place installing went to.
pub fn uninstall_from(root: &Path, id: &PluginId) -> Result<PathBuf, String> {
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

/// The same, out of a named plugins directory.
pub fn module_in(root: &Path, id: &PluginId) -> Option<PathBuf> {
    newest_module(&root.join(folder(id)))
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

//! Whether a newer Crook has been released, and putting one in place.
//!
//! # Nothing here starts by itself
//!
//! The same rule the store is written under, for the same reason:
//! [`published`] runs when somebody asks — `--check-update`, `--update`, the
//! button on the About page — and never at launch, never on a timer and never
//! behind anything else. A terminal that asks a server every morning whether
//! it is out of date is a terminal that has to be *trusted* about what it
//! asked; one that only ever asks when told to does not.
//!
//! What goes out is a `GET` of the releases API with the same bare user agent
//! every store request carries: no version, no machine id, nothing that says
//! which Crook is asking. What comes back is one tag.
//!
//! # It installs exactly what `script/install` installs
//!
//! The archive for this platform, checked against the `SHA256SUMS` published
//! beside it, unpacked, and put in place by a rename within the binary's own
//! directory — atomic, on the same filesystem, so a Crook that is running
//! keeps the inode it started from rather than having its text overwritten
//! under it. The installer script says all of that in `sh`; this says it in
//! Rust for the copy of Crook that is already on the machine, and the two must
//! not disagree: if one of them changes how a release is verified, so does the
//! other.
//!
//! # And it refuses rather than guesses
//!
//! A binary inside `Crook.app` is one file of a signed, notarized bundle, and
//! replacing it would leave a bundle whose signature no longer matches itself.
//! A binary in `/usr/bin` belongs to whatever package manager put it there. A
//! binary in `target/debug` is somebody's working tree. None of those is a
//! thing to overwrite, and each gets the one line that says what to do
//! instead — see [`Refusal`].

use std::path::{Path, PathBuf};

use crate::Channel;

pub mod model;

pub use model::UpdateModel;

/// Where the newest release is named.
///
/// The API rather than the `/releases/latest` redirect, for the reason
/// `script/install` asks it too: a repository with no release at all answers
/// this with a `404` that says so, and answers the redirect with a page.
pub const LATEST: &str = "https://api.github.com/repos/theguriev/crook/releases/latest";

/// Where a person is sent when Crook cannot install the release itself.
pub const RELEASES: &str = "https://github.com/theguriev/crook/releases";

/// Where a release's own files are served from.
const DOWNLOADS: &str = "https://github.com/theguriev/crook/releases/download";

/// The most the releases API's answer may be.
///
/// A release with long notes is a few tens of kilobytes; a megabyte is far
/// past that and short of anything that could be read into memory by mistake.
const REPLY_LIMIT: u64 = 1 << 20;

/// The most `SHA256SUMS` may be: one line per archive, six archives.
const SUMS_LIMIT: u64 = 1 << 16;

/// The most an archive may be.
///
/// A release archive is about twenty megabytes; sixty-four is room for a
/// build that grew and a ceiling on a URL that turned into something else.
const ARCHIVE_LIMIT: u64 = 64 << 20;

/// What this build is.
pub fn running() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

/// The newest release the repository has published.
pub struct Published {
    /// The tag, `v0.2.0`, which is what names the files on the release.
    pub tag: String,
    /// The version inside it, `0.2.0`, which is what compares to this build's.
    pub version: String,
}

impl Published {
    /// Whether it is later than the build asking.
    pub fn is_newer(&self) -> bool {
        is_newer(&self.version, running())
    }
}

/// Asks which release is the newest.
///
/// One request, made when somebody asked for it.
pub fn published(agent: &ureq::Agent) -> Result<Published, String> {
    let mut response = agent.get(LATEST).call().map_err(|why| why.to_string())?;
    let bytes = response
        .body_mut()
        .with_config()
        .limit(REPLY_LIMIT)
        .read_to_vec()
        .map_err(|why| why.to_string())?;
    let reply: serde_json::Value =
        serde_json::from_slice(&bytes).map_err(|why| format!("the answer is not JSON: {why}"))?;

    let tag = reply
        .get("tag_name")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| String::from("the answer names no release"))?;
    tagged(tag)
}

/// A tag read as a release, or a sentence saying it is not one.
///
/// Only the shape the release workflow publishes — `v` and a version — so a
/// repository that starts tagging something else does not send this looking
/// for `crook-nightly-3.tar.gz`.
fn tagged(tag: &str) -> Result<Published, String> {
    let version = tag
        .strip_prefix('v')
        .filter(|version| version.starts_with(|first: char| first.is_ascii_digit()))
        .ok_or_else(|| format!("{tag:?} is not a release tag"))?;
    Ok(Published {
        tag: tag.to_owned(),
        version: version.to_owned(),
    })
}

/// Whether `published` is a later version than `running`.
///
/// Numbers compared as numbers, so `0.1.10` is after `0.1.9` — which is the
/// whole reason this is not a string comparison, and the same thing
/// `script/release` asks `sort -V` for. A version with a suffix on it
/// (`0.2.0-rc1`) is *earlier* than the one without, which is semver's rule and
/// the only reading that does not offer a release candidate as an upgrade from
/// the release it was a candidate for.
pub fn is_newer(published: &str, running: &str) -> bool {
    parts(published) > parts(running)
}

/// A version as the three numbers and "is this the finished one", which is
/// what [`is_newer`] orders by.
fn parts(version: &str) -> (u64, u64, u64, bool) {
    let (numbers, finished) = match version.split_once(['-', '+']) {
        Some((numbers, _)) => (numbers, false),
        None => (version, true),
    };
    let mut fields = numbers
        .split('.')
        .map(|field| field.parse::<u64>().unwrap_or(0));
    (
        fields.next().unwrap_or(0),
        fields.next().unwrap_or(0),
        fields.next().unwrap_or(0),
        finished,
    )
}

/// Why this copy of Crook cannot replace itself.
///
/// Every one of these is a Crook that somebody else put there — a bundle, a
/// package, a build — and the answer to all of them is the same shape: say
/// which, and say what to do instead.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Refusal {
    /// A build from a working tree, run by whoever wrote it.
    Dev,
    /// Inside a macOS application bundle, which is signed as a whole.
    Bundle(PathBuf),
    /// Inside a `target/` directory, which is a build and not an install.
    BuildTree(PathBuf),
    /// Somewhere this process may not write: a package manager's, or root's.
    ReadOnly(PathBuf),
    /// A platform no release publishes an archive for.
    Platform,
    /// The path of this process could not be read at all.
    Unknown(String),
}

impl std::fmt::Display for Refusal {
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Dev => write!(
                out,
                "this is a dev build, which is a working tree rather than a release; \
                 `git pull && cargo build --release` is its update"
            ),
            Self::Bundle(path) => write!(
                out,
                "this Crook is inside {}, and a notarized bundle is replaced whole: \
                 download the disk image from {RELEASES}",
                path.display()
            ),
            Self::BuildTree(path) => write!(
                out,
                "this Crook was built at {}, so the update for it is another build",
                path.display()
            ),
            Self::ReadOnly(path) => write!(
                out,
                "{} cannot be written to by this user — whatever installed Crook there \
                 is what updates it",
                path.display()
            ),
            Self::Platform => write!(
                out,
                "no release publishes an archive for this platform; see {RELEASES}"
            ),
            Self::Unknown(why) => write!(out, "this binary's own path could not be read: {why}"),
        }
    }
}

/// The binary an update would be written over, or why none can be.
pub fn replaceable(channel: Channel) -> Result<PathBuf, Refusal> {
    if channel == Channel::Dev {
        return Err(Refusal::Dev);
    }
    if triple().is_none() {
        return Err(Refusal::Platform);
    }

    let binary = std::env::current_exe().map_err(|why| Refusal::Unknown(why.to_string()))?;
    // Resolved, because the thing to replace is the file and not the symlink
    // somebody put on their PATH: writing a new binary over the link would
    // leave the release where it was and the link no longer pointing at it.
    let binary = binary.canonicalize().unwrap_or(binary);

    if binary
        .ancestors()
        .any(|part| part.extension().is_some_and(|kind| kind == "app"))
    {
        return Err(Refusal::Bundle(binary));
    }
    // A release-channel binary run straight out of a build is still a build,
    // and another build is what updates it.
    if built_here(&binary) {
        return Err(Refusal::BuildTree(binary));
    }

    let directory = binary
        .parent()
        .ok_or_else(|| Refusal::Unknown(String::from("it has no directory")))?;
    // Asked by writing, because every other answer is a guess: a directory's
    // mode says nothing about ACLs, about a read-only mount, or about the
    // user this process actually runs as.
    match writable(directory) {
        true => Ok(binary),
        false => Err(Refusal::ReadOnly(directory.to_owned())),
    }
}

/// Whether the binary is sitting in a `target/` directory rather than in an
/// install.
///
/// `target/debug/crook`, `target/release/crook`, and the deeper paths cargo
/// writes for a cross build or a test binary — a profile directory under a
/// directory called `target`. Anything else is somebody's installed Crook,
/// including one they built themselves and copied onto their PATH, which is
/// exactly the one this may replace.
fn built_here(binary: &Path) -> bool {
    let mut profile = false;
    for part in binary.ancestors().skip(1) {
        let Some(name) = part.file_name().and_then(|name| name.to_str()) else {
            continue;
        };
        if name == "target" {
            return profile;
        }
        profile = profile || name == "debug" || name == "release";
    }
    false
}

/// Whether this process can make a file in `directory`, found out by making
/// one and taking it away again.
fn writable(directory: &Path) -> bool {
    let probe = directory.join(format!(".crook-update-{}", std::process::id()));
    match std::fs::File::create(&probe) {
        Ok(_) => {
            let _ = std::fs::remove_file(&probe);
            true
        }
        Err(_) => false,
    }
}

/// The target triple this build's release archive is named with, for the
/// platforms a release publishes a `.tar.gz` for.
///
/// Windows is published as a `.zip` and is not here: unpacking one would be a
/// dependency for the one platform where a running binary cannot be renamed
/// over anyway, and `script/install` says the same thing about it.
fn triple() -> Option<&'static str> {
    match (std::env::consts::OS, std::env::consts::ARCH) {
        ("linux", "x86_64") => Some("x86_64-unknown-linux-gnu"),
        ("macos", "aarch64") => Some("aarch64-apple-darwin"),
        ("macos", "x86_64") => Some("x86_64-apple-darwin"),
        _ => None,
    }
}

/// What the release calls the archive this machine wants, without its suffix.
fn stem(tag: &str, triple: &str) -> String {
    format!("crook-{tag}-{triple}")
}

/// Downloads the release named by `tag` and puts it where `binary` is.
///
/// Every step that could leave a half-written Crook happens before anything is
/// moved: the archive is fetched, checked against the published sums, unpacked
/// into a temporary directory and only then copied *into the binary's own
/// directory* and renamed over it. A rename within one directory is atomic, so
/// the file is either the version that was there or the one that arrived, and
/// never a prefix of either.
pub fn install(tag: &str, binary: &Path, agent: &ureq::Agent) -> Result<(), String> {
    let triple = triple().ok_or_else(|| Refusal::Platform.to_string())?;
    let stem = stem(tag, triple);
    let archive = format!("{stem}.tar.gz");

    let bytes = fetch(
        agent,
        &format!("{DOWNLOADS}/{tag}/{archive}"),
        ARCHIVE_LIMIT,
    )
    .map_err(|why| format!("{archive} could not be downloaded: {why}"))?;
    let sums = fetch(agent, &format!("{DOWNLOADS}/{tag}/SHA256SUMS"), SUMS_LIMIT)
        .map_err(|why| format!("{tag} publishes no SHA256SUMS: {why}"))?;
    let sums = String::from_utf8(sums).map_err(|_| String::from("SHA256SUMS is not text"))?;

    let want =
        sum_for(&sums, &archive).ok_or_else(|| format!("SHA256SUMS has no line for {archive}"))?;
    let got = crate::plugins::store::fetch::sha256_hex(&bytes);
    if !got.eq_ignore_ascii_case(want) {
        return Err(format!(
            "{archive} hashes to {got} and the release says {want}"
        ));
    }

    let work = scratch()?;
    let outcome = unpack_and_replace(&bytes, &stem, binary, &work);
    // Whatever happened: the archive and what came out of it are this
    // function's mess, and a failed update should not leave a hundred
    // megabytes in the temporary directory.
    let _ = std::fs::remove_dir_all(&work);
    outcome
}

/// The unpacking half, with the temporary directory already made.
fn unpack_and_replace(bytes: &[u8], stem: &str, binary: &Path, work: &Path) -> Result<(), String> {
    let archive = work.join(format!("{stem}.tar.gz"));
    std::fs::write(&archive, bytes)
        .map_err(|why| format!("{} could not be written: {why}", archive.display()))?;

    // `tar` rather than a crate, for the reason `script/install` needs it too:
    // every platform a release archive is published for ships one, and two
    // dependencies to read a format the machine already reads is two
    // dependencies in the trusted path of an update.
    let unpacked = crate::process::command("tar")
        .arg("-xzf")
        .arg(&archive)
        .arg("-C")
        .arg(work)
        .status()
        .map_err(|why| format!("tar could not be run: {why}"))?;
    if !unpacked.success() {
        return Err(format!("tar refused {}", archive.display()));
    }

    let arrived = work.join(stem).join("crook");
    if !arrived.is_file() {
        return Err(format!("{stem}.tar.gz holds no crook binary"));
    }

    // Into the destination's *own* directory before the rename, because a
    // rename is only atomic within one filesystem and the temporary directory
    // is often on another one.
    let directory = binary
        .parent()
        .ok_or_else(|| String::from("the binary has no directory"))?;
    let beside = directory.join(format!("crook.new.{}", std::process::id()));
    std::fs::copy(&arrived, &beside)
        .map_err(|why| format!("{} could not be written: {why}", beside.display()))?;
    if let Err(why) = executable(&beside) {
        let _ = std::fs::remove_file(&beside);
        return Err(why);
    }
    std::fs::rename(&beside, binary).map_err(|why| {
        let _ = std::fs::remove_file(&beside);
        format!("{} could not be replaced: {why}", binary.display())
    })
}

/// Marks the file as a program, on the platforms where that is a mode.
fn executable(path: &Path) -> Result<(), String> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755))
            .map_err(|why| format!("{} could not be made runnable: {why}", path.display()))?;
    }
    #[cfg(not(unix))]
    let _ = path;
    Ok(())
}

/// A directory of this run's own to unpack into.
fn scratch() -> Result<PathBuf, String> {
    let at = std::env::temp_dir().join(format!("crook-update-{}", std::process::id()));
    // Fresh, in case a run that was killed left one: unpacking over somebody
    // else's leftovers is how a stale binary gets installed.
    let _ = std::fs::remove_dir_all(&at);
    std::fs::create_dir_all(&at)
        .map_err(|why| format!("{} could not be made: {why}", at.display()))?;
    Ok(at)
}

/// One `GET`, read no further than `limit`.
fn fetch(agent: &ureq::Agent, url: &str, limit: u64) -> Result<Vec<u8>, String> {
    let mut response = agent.get(url).call().map_err(|why| why.to_string())?;
    response
        .body_mut()
        .with_config()
        .limit(limit)
        .read_to_vec()
        .map_err(|why| why.to_string())
}

/// The digest `SHA256SUMS` publishes for one file.
///
/// The `*` a binary-mode line carries in front of the name is not part of the
/// name, and the Windows archive's line has one — so it is stripped before the
/// names are compared, exactly as the installer script strips it.
fn sum_for<'a>(sums: &'a str, file: &str) -> Option<&'a str> {
    sums.lines().find_map(|line| {
        let (digest, name) = line.split_once(char::is_whitespace)?;
        (name.trim_start().trim_start_matches('*') == file).then_some(digest)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_version_is_ordered_by_its_numbers_and_not_by_its_text() {
        // The one comparison a string gets wrong, and the reason releases are
        // sorted with `sort -V` everywhere else: ten is after nine.
        assert!(is_newer("0.1.10", "0.1.9"));
        assert!(!is_newer("0.1.9", "0.1.10"));
        assert!(is_newer("0.2.0", "0.1.99"));
        assert!(is_newer("1.0.0", "0.9.9"));
        assert!(!is_newer("0.1.9", "0.1.9"), "the same version is not newer");
    }

    #[test]
    fn a_release_candidate_is_older_than_the_release_it_was_for() {
        // Semver's rule, and the only one that does not offer somebody on
        // 0.2.0 an "update" to 0.2.0-rc1.
        assert!(is_newer("0.2.0", "0.2.0-rc1"));
        assert!(!is_newer("0.2.0-rc1", "0.2.0"));
        assert!(is_newer("0.2.0-rc1", "0.1.9"));
    }

    #[test]
    fn only_a_release_tag_is_read_as_one() {
        let published = tagged("v0.2.0").expect("a release tag");
        assert_eq!(published.tag, "v0.2.0");
        assert_eq!(published.version, "0.2.0");

        // A repository that starts tagging something else — a crate's own
        // `crook_wasm-v0.8.0`, a branch marker — must not send this looking
        // for an archive named after it.
        for tag in ["crook_wasm-v0.8.0", "index", "nightly", "v", "0.2.0"] {
            assert!(tagged(tag).is_err(), "{tag:?} was read as a release");
        }
    }

    #[test]
    fn the_sums_line_is_found_by_name_and_not_by_position() {
        let sums = "\
aaaa  crook-v0.2.0-aarch64-apple-darwin.tar.gz
bbbb  crook-v0.2.0-x86_64-unknown-linux-gnu.tar.gz
cccc *crook-v0.2.0-x86_64-pc-windows-msvc.zip
";
        assert_eq!(
            sum_for(sums, "crook-v0.2.0-x86_64-unknown-linux-gnu.tar.gz"),
            Some("bbbb")
        );
        // The `*` of a binary-mode line belongs to the mode and not to the
        // name, which is what makes the Windows entry findable at all.
        assert_eq!(
            sum_for(sums, "crook-v0.2.0-x86_64-pc-windows-msvc.zip"),
            Some("cccc")
        );
        assert_eq!(sum_for(sums, "crook-v0.2.0-riscv64.tar.gz"), None);
    }

    #[test]
    fn the_archive_is_named_the_way_the_release_names_it() {
        assert_eq!(
            stem("v0.2.0", "x86_64-unknown-linux-gnu"),
            "crook-v0.2.0-x86_64-unknown-linux-gnu"
        );
    }

    #[test]
    fn a_dev_build_is_refused_before_anything_is_asked() {
        // The first check, and the cheapest: a working tree has no release to
        // be behind, and the sentence says what its update is instead.
        let refusal = replaceable(Channel::Dev).expect_err("a dev build cannot replace itself");
        assert_eq!(refusal, Refusal::Dev);
        assert!(refusal.to_string().contains("cargo build --release"));
    }

    #[test]
    fn a_binary_in_a_build_tree_is_refused_by_its_path() {
        // Where `cargo test` runs from, which is the one case this can prove
        // in a test: the test binary lives in `target/debug/deps`, so the
        // refusal is read off the path rather than out of a channel.
        let refusal = replaceable(Channel::Stable).expect_err("a build is not an install");
        assert!(
            matches!(refusal, Refusal::BuildTree(_) | Refusal::Bundle(_)),
            "{refusal:?}"
        );
    }
}

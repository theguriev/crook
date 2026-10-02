//! Putting a newer `Crook.app` where this one is.
//!
//! # The bundle is the unit, not the binary
//!
//! A binary inside `Crook.app` is one file of a bundle that is signed and
//! notarized as a whole: writing a new `Contents/MacOS/crook` over it leaves a
//! bundle whose seal no longer matches what is inside it. So what a release
//! publishes for macOS as one thing — the disk image — is what replaces it as
//! one thing: the image is fetched and checked against `SHA256SUMS` the way an
//! archive is, mounted read-only where nothing else can see it, and the app in
//! it is copied out beside this one.
//!
//! # And it is vouched for before it is put in place
//!
//! The copy, not the image: what is checked is the directory that will be
//! renamed into place, so nothing can change between the check and the
//! rename. It must pass `codesign --verify --deep --strict`, be accepted by
//! Gatekeeper, be signed by the same team with the same identifier as the
//! bundle that is running, and say in its `Info.plist` that it is the version
//! the release was tagged with. A bundle that is not itself signed by a team —
//! a local build of `script/macos-app` — has nothing to hold a release to, and
//! is refused rather than guessed about.
//!
//! # Two renames, and the old one back if the second fails
//!
//! Both within the bundle's own directory, so neither copies anything: the
//! running bundle is moved aside, the new one takes its name, and the old one
//! is removed. A Crook that is running keeps the executable it started from,
//! which is what the restart line on the About page is about.

use std::path::{Path, PathBuf};

use super::Refusal;

/// The bundle `binary` is the executable of, if it is inside one.
///
/// The nearest `.app` above it, which for `Crook.app/Contents/MacOS/crook` is
/// `Crook.app` — and for a binary that is somewhere else inside a bundle is
/// still that bundle, which [`shape`] then refuses.
pub(super) fn of(binary: &Path) -> Option<&Path> {
    binary
        .ancestors()
        .skip(1)
        .find(|part| part.extension().is_some_and(|kind| kind == "app"))
}

/// The bundle an update may be written over, or why it may not.
///
/// Only the shape a release builds — the executable at
/// `Contents/MacOS/` of its bundle — and only where this user may make and
/// rename entries beside the bundle, which is all a swap asks of the
/// filesystem. An app run from the mounted image, or translocated by
/// Gatekeeper into a read-only place, is neither, and is sent to the image.
pub(super) fn shape(bundle: &Path, binary: &Path) -> Result<PathBuf, Refusal> {
    let executable_of_it = binary.parent() == Some(bundle.join("Contents").join("MacOS").as_path());
    let directory = bundle.parent();
    match (executable_of_it, directory) {
        (true, Some(directory)) if super::writable(directory) => Ok(bundle.to_owned()),
        _ => Err(Refusal::Bundle(binary.to_owned())),
    }
}

/// What the release calls its disk image.
fn image(tag: &str) -> String {
    format!("crook-{tag}-macos.dmg")
}

/// Downloads the disk image of the release named by `tag` and puts the app in
/// it where `bundle` is.
pub(super) fn install(tag: &str, bundle: &Path, agent: &ureq::Agent) -> Result<(), String> {
    let version = super::tagged(tag)?.version;
    let image = image(tag);
    let bytes = super::verified(agent, tag, &image)?;

    let work = super::scratch()?;
    let outcome = mount_and_replace(&bytes, &image, &version, bundle, &work);
    // The image and its mount point are this function's, whatever happened.
    let _ = std::fs::remove_dir_all(&work);
    outcome
}

/// The half that has the image's bytes and a directory to put them in.
fn mount_and_replace(
    bytes: &[u8],
    image: &str,
    version: &str,
    bundle: &Path,
    work: &Path,
) -> Result<(), String> {
    let dmg = work.join(image);
    std::fs::write(&dmg, bytes)
        .map_err(|why| format!("{} could not be written: {why}", dmg.display()))?;
    let volume = work.join("volume");

    // Read-only, out of the Finder's sight and with nothing opened on it: the
    // image is a container to read one directory out of, not something a
    // person should see appear in the sidebar while Crook updates.
    let attached = crate::process::command("hdiutil")
        .args(["attach", "-nobrowse", "-readonly", "-noautoopen", "-quiet"])
        .arg("-mountpoint")
        .arg(&volume)
        .arg(&dmg)
        .status()
        .map_err(|why| format!("hdiutil could not be run: {why}"))?;
    if !attached.success() {
        return Err(format!("hdiutil could not mount {image}"));
    }

    let outcome = copy_out(&volume.join("Crook.app"), image, version, bundle);

    let detached = crate::process::command("hdiutil")
        .args(["detach", "-quiet"])
        .arg(&volume)
        .status();
    if !detached.is_ok_and(|status| status.success()) {
        // Busy for a moment is what a Spotlight import looks like; forced,
        // because a mount nobody can see is not one to leave behind.
        let _ = crate::process::command("hdiutil")
            .args(["detach", "-force", "-quiet"])
            .arg(&volume)
            .status();
    }
    outcome
}

/// Copies the app out of the mounted image beside `bundle`, vouches for the
/// copy, and swaps it in.
fn copy_out(arrived: &Path, image: &str, version: &str, bundle: &Path) -> Result<(), String> {
    if !arrived.is_dir() {
        return Err(format!("{image} holds no Crook.app"));
    }
    let beside = sibling(bundle, "new")?;
    // A copy left by a run of this process id that died halfway.
    let _ = std::fs::remove_dir_all(&beside);

    // `ditto` rather than a recursive copy: it keeps the extended attributes,
    // the symlinks and the modes a signature is a seal over.
    let copied = crate::process::command("ditto")
        .arg(arrived)
        .arg(&beside)
        .status()
        .map_err(|why| format!("ditto could not be run: {why}"))?;
    if !copied.success() {
        let _ = std::fs::remove_dir_all(&beside);
        return Err(format!(
            "Crook.app could not be copied to {}",
            beside.display()
        ));
    }

    if let Err(why) = vouched(&beside, bundle, version) {
        let _ = std::fs::remove_dir_all(&beside);
        return Err(why);
    }
    swap(&beside, bundle)
}

/// A hidden name beside `bundle` for one stage of a swap.
fn sibling(bundle: &Path, stage: &str) -> Result<PathBuf, String> {
    let directory = bundle
        .parent()
        .ok_or_else(|| String::from("the bundle has no directory"))?;
    let name = bundle
        .file_name()
        .ok_or_else(|| String::from("the bundle has no name"))?
        .to_string_lossy();
    Ok(directory.join(format!(".{name}.{stage}.{}", std::process::id())))
}

/// Puts `arrived` where `bundle` is, and `bundle` back if that fails.
fn swap(arrived: &Path, bundle: &Path) -> Result<(), String> {
    let aside = sibling(bundle, "old")?;
    let _ = std::fs::remove_dir_all(&aside);

    if let Err(why) = std::fs::rename(bundle, &aside) {
        let _ = std::fs::remove_dir_all(arrived);
        return Err(format!(
            "{} could not be moved aside: {why}",
            bundle.display()
        ));
    }
    if let Err(why) = std::fs::rename(arrived, bundle) {
        let _ = std::fs::rename(&aside, bundle);
        let _ = std::fs::remove_dir_all(arrived);
        return Err(format!("{} could not be replaced: {why}", bundle.display()));
    }
    // The running process holds the executable it started from open, so the
    // old bundle's directory entries can go now; what cannot is left hidden
    // beside the new one rather than failing an update that has happened.
    let _ = std::fs::remove_dir_all(&aside);
    Ok(())
}

/// Whether the copy at `arrived` is a release this `bundle` may become.
fn vouched(arrived: &Path, bundle: &Path, version: &str) -> Result<(), String> {
    let sealed = crate::process::command("codesign")
        .args(["--verify", "--deep", "--strict"])
        .arg(arrived)
        .output()
        .map_err(|why| format!("codesign could not be run: {why}"))?;
    if !sealed.status.success() {
        return Err(format!(
            "the Crook.app in the release does not match its own signature: {}",
            String::from_utf8_lossy(&sealed.stderr).trim()
        ));
    }

    let running = signer_of(bundle)?;
    let Some(team) = running.team.as_deref() else {
        return Err(format!(
            "{} is not signed by a developer team, so there is no signature to hold the \
             release to: download the disk image from {}",
            bundle.display(),
            super::RELEASES
        ));
    };
    let release = signer_of(arrived)?;
    if release.identifier != running.identifier || release.team.as_deref() != Some(team) {
        return Err(format!(
            "the Crook.app in the release is signed as {} by {}, and this one as {} by {team}",
            release.identifier,
            release.team.as_deref().unwrap_or("nobody"),
            running.identifier,
        ));
    }

    let accepted = crate::process::command("spctl")
        .args(["--assess", "--type", "execute"])
        .arg(arrived)
        .output()
        .map_err(|why| format!("spctl could not be run: {why}"))?;
    if !accepted.status.success() {
        return Err(format!(
            "Gatekeeper does not accept the Crook.app in the release: {}",
            String::from_utf8_lossy(&accepted.stderr).trim()
        ));
    }

    let said = crate::process::command("plutil")
        .args(["-extract", "CFBundleShortVersionString", "raw", "-o", "-"])
        .arg(arrived.join("Contents").join("Info.plist"))
        .output()
        .map_err(|why| format!("plutil could not be run: {why}"))?;
    let said = String::from_utf8_lossy(&said.stdout);
    if said.trim() != version {
        return Err(format!(
            "the Crook.app in the {version} release says it is {:?}",
            said.trim()
        ));
    }
    Ok(())
}

/// Who signed a bundle, as `codesign` says it.
#[derive(Debug, PartialEq, Eq)]
struct Signer {
    /// The signing identifier, `com.theguriev.crook`.
    identifier: String,
    /// The team, or `None` for an ad-hoc or unsigned bundle.
    team: Option<String>,
}

/// Asks `codesign` who signed `bundle`.
fn signer_of(bundle: &Path) -> Result<Signer, String> {
    let shown = crate::process::command("codesign")
        .args(["--display", "--verbose=2"])
        .arg(bundle)
        .output()
        .map_err(|why| format!("codesign could not be run: {why}"))?;
    // `--display` writes what it found to stderr, and an unsigned bundle is a
    // failure with nothing to read.
    signer(&String::from_utf8_lossy(&shown.stderr))
        .ok_or_else(|| format!("{} is not signed", bundle.display()))
}

/// The signer in what `codesign --display` wrote.
fn signer(said: &str) -> Option<Signer> {
    let field = |name: &str| {
        said.lines()
            .find_map(|line| line.strip_prefix(name)?.strip_prefix('='))
            .map(str::trim)
    };
    Some(Signer {
        identifier: field("Identifier")?.to_owned(),
        team: field("TeamIdentifier")
            .filter(|team| !team.is_empty() && *team != "not set")
            .map(ToOwned::to_owned),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_bundle_is_the_nearest_app_above_the_binary() {
        let binary = Path::new("/Applications/Crook.app/Contents/MacOS/crook");
        assert_eq!(of(binary), Some(Path::new("/Applications/Crook.app")));
        assert_eq!(of(Path::new("/usr/local/bin/crook")), None);
        // The binary's own name is not its bundle, whatever it is called.
        assert_eq!(of(Path::new("/opt/crook.app")), None);
    }

    #[test]
    fn only_the_executable_a_release_builds_is_one_to_replace_the_bundle_for() {
        let base = std::env::temp_dir().join(format!("crook-bundle-shape-{}", std::process::id()));
        let bundle = base.join("Crook.app");
        let macos = bundle.join("Contents").join("MacOS");
        std::fs::create_dir_all(&macos).expect("the scratch is writable");
        let binary = macos.join("crook");
        let helper = bundle.join("Contents").join("Helpers").join("crook");

        let shaped = shape(&bundle, &binary);
        let elsewhere = shape(&bundle, &helper);
        let _ = std::fs::remove_dir_all(&base);

        assert_eq!(shaped, Ok(bundle));
        assert_eq!(elsewhere, Err(Refusal::Bundle(helper)));
    }

    #[test]
    fn the_signer_is_read_off_what_codesign_displays() {
        let said = "\
Executable=/Applications/Crook.app/Contents/MacOS/crook
Identifier=com.theguriev.crook
Format=app bundle with Mach-O universal (x86_64 arm64)
Authority=Developer ID Application: Eugen Guriev (5UWLF5HX3H)
TeamIdentifier=5UWLF5HX3H
";
        assert_eq!(
            signer(said),
            Some(Signer {
                identifier: String::from("com.theguriev.crook"),
                team: Some(String::from("5UWLF5HX3H")),
            })
        );

        // What an ad-hoc signature of `script/macos-app` displays: a bundle
        // signed by nobody in particular, which no release can be held to.
        let adhoc = "Identifier=com.theguriev.crook\nSignature=adhoc\nTeamIdentifier=not set\n";
        assert_eq!(signer(adhoc).and_then(|signer| signer.team), None);

        assert_eq!(signer("code object is not signed at all\n"), None);
    }

    #[test]
    fn the_image_is_named_the_way_the_release_names_it() {
        assert_eq!(image("v0.2.0"), "crook-v0.2.0-macos.dmg");
    }

    #[test]
    fn a_swap_puts_the_release_in_place_and_takes_the_old_one_away() {
        let base = std::env::temp_dir().join(format!("crook-bundle-swap-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let bundle = base.join("Crook.app");
        std::fs::create_dir_all(&bundle).expect("the scratch is writable");
        std::fs::write(bundle.join("version"), b"old").expect("the scratch is writable");
        let arrived = sibling(&bundle, "new").expect("a sibling");
        std::fs::create_dir_all(&arrived).expect("the scratch is writable");
        std::fs::write(arrived.join("version"), b"new").expect("the scratch is writable");

        let swapped = swap(&arrived, &bundle);
        let now = std::fs::read(bundle.join("version"));
        let left: Vec<_> = std::fs::read_dir(&base)
            .map(|entries| entries.flatten().map(|entry| entry.file_name()).collect())
            .unwrap_or_default();
        let _ = std::fs::remove_dir_all(&base);

        assert_eq!(swapped, Ok(()));
        assert_eq!(now.ok().as_deref(), Some(&b"new"[..]));
        assert_eq!(
            left,
            vec![std::ffi::OsString::from("Crook.app")],
            "{left:?}"
        );
    }

    #[test]
    fn a_swap_that_cannot_happen_leaves_the_bundle_that_was_there() {
        let base = std::env::temp_dir().join(format!("crook-bundle-kept-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let bundle = base.join("Crook.app");
        std::fs::create_dir_all(&bundle).expect("the scratch is writable");
        std::fs::write(bundle.join("version"), b"old").expect("the scratch is writable");

        // Nothing arrived: the second rename fails, and the first is undone.
        let swapped = swap(&base.join(".Crook.app.new.missing"), &bundle);
        let now = std::fs::read(bundle.join("version"));
        let _ = std::fs::remove_dir_all(&base);

        assert!(swapped.is_err());
        assert_eq!(now.ok().as_deref(), Some(&b"old"[..]));
    }
}

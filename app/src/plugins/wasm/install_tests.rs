//! What installing writes, what it removes, and what it refuses.

use super::*;
use crate::plugins::wasm::tests::{Scratch, install_flat, wasm_at};

#[test]
fn something_that_is_not_a_plugin_is_refused_before_anything_is_written() {
    // Read before written, so that a file which is not a plugin is refused
    // with a line naming what is wrong rather than installed and then refused
    // on every launch afterwards.
    let scratch = Scratch::new("install-rubbish");
    let path = scratch.path().join("not-a-plugin.wasm");
    fs::write(&path, b"this is not a plugin").expect("the scratch file writes");

    let refusal = into(scratch.path(), &path).expect_err("it should be refused");

    assert!(refusal.contains("not a plugin"), "{refusal}");
    assert_eq!(
        fs::read_dir(scratch.path()).into_iter().flatten().count(),
        1,
        "nothing but the file itself should have been created"
    );
}

#[test]
fn a_path_with_nothing_at_it_is_refused_the_same_way() {
    let scratch = Scratch::new("install-missing");

    let refusal = into(scratch.path(), Path::new("/nowhere/at/all/plugin.wasm"))
        .expect_err("it should be refused");

    assert!(refusal.contains("could not be read"), "{refusal}");
}

#[test]
fn a_plugin_lands_in_a_directory_named_after_itself() {
    // Which is what makes installing the same plugin twice an upgrade rather
    // than a second copy.
    let id = PluginId::parse("theguriev/pirate").expect("a literal that parses");

    assert_eq!(folder(&id), "theguriev.pirate");
    assert!(!folder(&id).contains(std::path::MAIN_SEPARATOR));
}

#[test]
fn a_module_lands_under_the_version_its_manifest_names() {
    let scratch = Scratch::new("install-version");
    let module = written(&scratch, "0.3.0");

    let installed = into(scratch.path(), &module).expect("it should install");

    assert_eq!(
        installed,
        scratch
            .path()
            .join("eugen.probe")
            .join("0.3.0")
            .join(MODULE_FILE)
    );
    assert!(installed.is_file());
}

#[test]
fn an_upgrade_replaces_the_version_that_was_there() {
    // One version is kept, because a directory that quietly accumulated every
    // version ever installed is disk nobody chose to spend.
    let scratch = Scratch::new("install-upgrade");
    into(scratch.path(), &written(&scratch, "0.3.0")).expect("the first installs");

    into(scratch.path(), &written(&scratch, "0.4.0")).expect("the second installs");

    let versions = versions_in(&scratch.path().join("eugen.probe"));
    assert_eq!(versions, ["0.4.0"]);
}

#[test]
fn installing_over_the_layout_that_came_before_versions_migrates_it() {
    // A flat module left beside the versioned one is the same plugin twice:
    // two contributions to its slot, and a second set of actions refused as
    // already taken.
    let scratch = Scratch::new("install-migrate");
    install_flat(
        scratch.path(),
        "eugen.probe",
        &wasm_at("eugen/probe", "0.1.0"),
    );

    into(scratch.path(), &written(&scratch, "0.2.0")).expect("it should install");

    let home = scratch.path().join("eugen.probe");
    assert!(
        !home.join(MODULE_FILE).exists(),
        "the flat module is still there"
    );
    assert_eq!(versions_in(&home), ["0.2.0"]);
}

#[test]
fn a_version_that_would_be_a_path_is_not_a_directory_name() {
    // The one string in a manifest that becomes a path. A `PluginId` is
    // checked character by character before it is one; this is a stranger's
    // string arriving where a directory name goes.
    let scratch = Scratch::new("install-escape");
    let module = written(&scratch, "../../elsewhere");

    let refusal = into(scratch.path(), &module).expect_err("it should be refused");

    assert!(refusal.contains("is not one"), "{refusal}");
    assert!(!scratch.path().join("eugen.probe").exists());
}

#[test]
fn installing_an_older_version_is_how_a_rollback_works() {
    // There is no button for it and this is why there does not have to be:
    // the newer directory goes, and what is left is what loads.
    let scratch = Scratch::new("install-rollback");
    into(scratch.path(), &written(&scratch, "0.4.0")).expect("the newer installs");

    let installed = into(scratch.path(), &written(&scratch, "0.3.0")).expect("and the older");

    assert_eq!(versions_in(&scratch.path().join("eugen.probe")), ["0.3.0"]);
    assert_eq!(
        crate::plugins::wasm::tests::newest_module_for(&scratch.path().join("eugen.probe")),
        Some(installed),
        "the version that was installed is the one that would load"
    );
}

#[test]
fn installing_the_version_that_is_already_there_keeps_it() {
    // The sweep removes every version but the one just written, and the one
    // just written is the one it is being asked to remove.
    let scratch = Scratch::new("install-again");
    into(scratch.path(), &written(&scratch, "0.3.0")).expect("the first installs");

    let installed =
        into(scratch.path(), &written(&scratch, "0.3.0")).expect("and so does it again");

    assert!(installed.is_file());
    assert_eq!(versions_in(&scratch.path().join("eugen.probe")), ["0.3.0"]);
}

#[test]
fn a_version_that_is_a_directory_that_already_means_something_is_refused() {
    // `.` and `..` are inside the character set a version is written with, so
    // nothing but these two lines stops a manifest naming the plugin's own
    // home directory or the one above it — which the sweep would then empty.
    for version in [".", "..", "1.0.", ""] {
        let refusal = version_folder(version)
            .expect_err(&format!("{version:?} should not be a directory name"));
        assert!(refusal.contains("is not one"), "{refusal}");
    }

    version_folder("0.3.0").expect("an ordinary one");
    version_folder("1.0.0-rc.1+build.7").expect("and everything a version is written with");
}

#[test]
fn uninstalling_takes_the_whole_plugin_and_not_one_version_of_it() {
    // A plugin whose last version was removed is not a plugin with an empty
    // directory: the row on the Plugins page comes from what is on disk.
    let scratch = Scratch::new("uninstall");
    into(scratch.path(), &written(&scratch, "0.3.0")).expect("it should install");
    let id = PluginId::parse("eugen/probe").expect("a literal that parses");

    let removed = from(scratch.path(), &id).expect("it should uninstall");

    assert_eq!(removed, scratch.path().join("eugen.probe"));
    assert!(!removed.exists());
}

#[test]
fn uninstalling_something_that_is_not_installed_says_so() {
    let scratch = Scratch::new("uninstall-missing");
    let id = PluginId::parse("eugen/probe").expect("a literal that parses");

    let refusal = from(scratch.path(), &id).expect_err("there is nothing to remove");

    assert!(refusal.contains("not installed"), "{refusal}");
}

/// A module saying it is at `version`, written somewhere to install *from*.
///
/// Named by a counter rather than by the version, because one of these tests
/// hands over a version that is a path and the file to install from would
/// inherit it.
fn written(scratch: &Scratch, version: &str) -> PathBuf {
    static SERIAL: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let serial = SERIAL.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let path = scratch.path().join(format!("probe-{serial}.wasm"));
    fs::write(&path, wasm_at("eugen/probe", version)).expect("the module should be writable");
    path
}

/// The version directories inside one plugin's home, in order.
fn versions_in(home: &Path) -> Vec<String> {
    let mut found: Vec<String> = fs::read_dir(home)
        .into_iter()
        .flatten()
        .flatten()
        .filter(|entry| entry.path().is_dir())
        .filter_map(|entry| entry.file_name().to_str().map(str::to_owned))
        .collect();
    found.sort();
    found
}

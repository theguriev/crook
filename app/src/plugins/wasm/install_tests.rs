//! What installing refuses, and what a plugin's directory is called.

use super::*;

#[test]
fn something_that_is_not_a_plugin_is_refused_before_anything_is_written() {
    // Read before written, so that a file which is not a plugin is refused
    // with a line naming what is wrong rather than installed and then refused
    // on every launch afterwards.
    let path = scratch().join("not-a-plugin.wasm");
    fs::write(&path, b"this is not a plugin").expect("the scratch file writes");

    let refusal = install(&path).expect_err("it should be refused");

    assert!(refusal.contains("not a plugin"), "{refusal}");
}

#[test]
fn a_path_with_nothing_at_it_is_refused_the_same_way() {
    let refusal =
        install(Path::new("/nowhere/at/all/plugin.wasm")).expect_err("it should be refused");

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

/// A scratch directory, one per test.
fn scratch() -> PathBuf {
    let path = std::env::temp_dir().join(format!(
        "crook-install-{}-{:?}",
        std::process::id(),
        std::thread::current().id()
    ));
    let _ = fs::remove_dir_all(&path);
    fs::create_dir_all(&path).expect("a scratch directory");
    path
}

//! What a fixture is, and what it refuses to be.

use super::*;
use crate::plugins::wasm::tests::Scratch;

/// Writes a fixture file and hands back its path.
fn written(scratch: &Scratch, json: &str) -> std::path::PathBuf {
    let path = scratch.path().join("fixture.json");
    std::fs::write(&path, json).expect("the fixture should be writable");
    path
}

#[test]
fn a_fixture_is_the_plugins_own_vocabulary_written_by_hand() {
    // The whole point: what is in the file is a `Node`, the same shape a guest
    // encodes across the wire, so a fixture cannot describe anything a plugin
    // could not — and does not need a second decoder to be read.
    let scratch = Scratch::new("fixture");
    let path = written(
        &scratch,
        r#"{"header.right": {"Text": {"text": "62%", "size": "Small", "tone": "Primary"}}}"#,
    );

    let fixture = Fixture::read(&path).expect("it should read");

    assert_eq!(fixture.trees.len(), 1);
    assert_eq!(fixture.trees[0].0, "header.right");
}

#[test]
fn the_fixture_this_repository_keeps_is_one() {
    // The file `--plugin-fixture` is documented with. A picture nobody can
    // take because the example in the docs does not parse is worse than no
    // example.
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("the workspace root")
        .join("script/fixtures/header.json");

    Fixture::read(&path).expect("the fixture in this repository should read");
}

#[test]
fn a_fixture_may_name_an_icon_and_carries_it_the_way_a_module_does() {
    // For a picture of the Plugins page with a face on a row: the file names
    // a PNG beside itself, and the fixture then answers `pictures` the way
    // an installed module does. A file naming none still parses, and the
    // key is not taken for a slot.
    let scratch = Scratch::new("fixture-icon");
    std::fs::write(
        scratch.path().join("face.png"),
        crate::picture::tests::icon_png(32),
    )
    .expect("the icon should be writable");
    let path = written(
        &scratch,
        r#"{"icon": "face.png", "header.right": {"Text": {"text": "62%", "size": "Small", "tone": "Primary"}}}"#,
    );

    let fixture = Fixture::read(&path).expect("it should read");

    assert_eq!(fixture.trees.len(), 1, "the icon is not a slot");
    assert_eq!(fixture.trees[0].0, "header.right");
    let icon = fixture
        .pictures()
        .and_then(|pictures| pictures.icon.as_ref())
        .expect("the fixture carries its icon");
    assert_eq!(icon.size(), (32, 32));

    // And one that names nothing carries nothing.
    let bare = written(
        &scratch,
        r#"{"header.right": {"Text": {"text": "62%", "size": "Small", "tone": "Primary"}}}"#,
    );
    let fixture = Fixture::read(&bare).expect("it should read");
    assert!(
        fixture
            .pictures()
            .is_some_and(|pictures| pictures.icon.is_none())
    );
}

#[test]
fn something_that_is_not_a_fixture_is_refused_with_a_line() {
    // `expect_err` would need `Fixture` to be `Debug`, and a list of trees is
    // not a thing worth printing.
    let refused = |json: &str, path: Option<&std::path::Path>| -> String {
        let scratch = Scratch::new("fixture-rubbish");
        let path = path
            .map(std::path::Path::to_path_buf)
            .unwrap_or_else(|| written(&scratch, json));
        match Fixture::read(&path) {
            Ok(_) => panic!("{json:?} should have been refused"),
            Err(why) => why,
        }
    };

    assert!(refused("{", None).contains("is not a fixture"));
    assert!(refused("{}", None).contains("nothing to draw"));
    assert!(
        refused(
            r#"{"icon": "nowhere.png", "header.right": {"Gap": "Small"}}"#,
            None
        )
        .contains("names an icon at")
    );
    assert!(
        refused(r#"{"icon": 3, "header.right": {"Gap": "Small"}}"#, None).contains("not a path")
    );
    assert!(
        refused("", Some(std::path::Path::new("/nowhere/at/all.json")))
            .contains("could not be read")
    );
}

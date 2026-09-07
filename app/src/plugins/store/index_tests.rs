//! What an index is read to mean.

use super::*;

const ONE: &str = r#"{
  "schema": 1,
  "plugins": [
    {
      "id": "theguriev/pirate",
      "name": "Pirate",
      "description": "How much of the session budget is spent.",
      "repository": "https://github.com/theguriev/crook-pirate",
      "license": "MIT",
      "versions": [
        {"version": "0.9.0", "abi": 8, "url": "https://example.invalid/a.wasm", "sha256": "aa", "bytes": 10, "capabilities": ["net:api.anthropic.com"], "asks": ["Reach api.anthropic.com"]},
        {"version": "0.10.0", "abi": 8, "url": "https://example.invalid/b.wasm", "sha256": "bb", "bytes": 11},
        {"version": "0.11.0", "abi": 9, "url": "https://example.invalid/c.wasm", "sha256": "cc", "bytes": 12}
      ]
    }
  ]
}"#;

#[test]
fn the_newest_version_this_build_can_run_is_the_one_offered() {
    // Not the newest version there is: 0.11.0 speaks a vocabulary this build
    // does not, and offering it would be offering an install that ends in a
    // refusal nobody could have predicted from the list.
    let index = parse(ONE.as_bytes()).expect("it should parse");

    let offers = offers(&index);

    assert_eq!(offers.len(), 1);
    let offered = offers[0].release.as_ref().expect("something to install");
    assert_eq!(offered.version, "0.10.0");
    assert_eq!(
        offers[0].newest_anywhere.as_ref().unwrap().version,
        "0.11.0"
    );
}

#[test]
fn a_version_that_was_withdrawn_is_not_offered_and_is_still_remembered() {
    let mut index = parse(ONE.as_bytes()).expect("it should parse");
    index.plugins[0].versions[1].yanked = Some(String::from("it read the wrong file"));

    let offers = offers(&index);

    assert_eq!(
        offers[0]
            .release
            .as_ref()
            .expect("the one below it")
            .version,
        "0.9.0",
        "a yanked version should fall back to the one under it"
    );
    let id = PluginId::parse("theguriev/pirate").expect("a literal that parses");
    assert_eq!(
        withdrawn(&index, &id, "0.10.0").as_deref(),
        Some("it read the wrong file")
    );
    assert_eq!(withdrawn(&index, &id, "0.9.0"), None);
}

#[test]
fn a_plugin_with_nothing_for_this_build_is_a_row_that_says_so() {
    // Rather than a plugin that is missing from the list, which is a bug
    // report; "built for a newer Crook" is an upgrade.
    let mut index = parse(ONE.as_bytes()).expect("it should parse");
    index.plugins[0].versions.retain(|release| release.abi != 8);

    let offers = offers(&index);

    assert_eq!(offers.len(), 1);
    assert!(!offers[0].installable());
    assert_eq!(offers[0].newest_anywhere.as_ref().unwrap().abi, 9);
}

#[test]
fn a_layout_this_build_does_not_read_is_refused_by_both_numbers() {
    let refusal = parse(br#"{"schema": 2, "plugins": []}"#).expect_err("it should be refused");

    assert!(refusal.contains('2') && refusal.contains('1'), "{refusal}");
}

#[test]
fn a_field_this_build_has_never_heard_of_is_not_a_broken_index() {
    // A registry that adds a field must not stop every Crook already
    // installed from reading the list.
    let index = parse(
        br#"{"schema": 1, "downloads": 4, "plugins": [
             {"id": "eugen/probe", "name": "Probe", "description": "d", "screenshots": ["x.png"],
              "versions": [{"version": "1.0.0", "abi": 8, "url": "https://x.invalid/p.wasm",
                            "sha256": "aa", "signature": "later"}]}]}"#,
    )
    .expect("it should parse");

    assert_eq!(offers(&index)[0].release.as_ref().unwrap().version, "1.0.0");
}

#[test]
fn a_row_whose_name_is_not_one_is_dropped_rather_than_shown() {
    // Nothing can be granted under a name that is not a name, so a row
    // carrying one is a row whose install could never be allowed anything.
    let index = parse(
        br#"{"schema": 1, "plugins": [
             {"id": "Not An Id", "name": "X", "description": "d", "versions": []},
             {"id": "eugen/probe", "name": "Probe", "description": "d", "versions": []}]}"#,
    )
    .expect("it should parse");

    let offers = offers(&index);

    assert_eq!(offers.len(), 1);
    assert_eq!(offers[0].id.as_str(), "eugen/probe");
}

#[test]
fn an_index_that_is_not_json_says_so_rather_than_answering_with_nothing() {
    // The difference between "the registry has no plugins" and "something
    // went wrong" is the difference between an empty page and a line a person
    // can act on.
    assert!(parse(b"<html>404</html>").is_err());
}

#[test]
fn a_module_that_is_not_what_the_list_promised_is_refused() {
    // The one check anybody has on the index being what it says it is: it was
    // read out of the artifact by the registry's copy of this host's reader,
    // so a row and a module that disagree is a registry with a mistake in it
    // or a URL that now serves something else.
    let release = Release {
        version: "1.0.0".into(),
        abi: 8,
        url: "https://x.invalid/p.wasm".into(),
        sha256: "aa".into(),
        bytes: 0,
        capabilities: vec![String::from("cwd.read")],
        asks: Vec::new(),
        yanked: None,
    };
    let manifest = crook_plugin::Manifest {
        schema: crook_plugin::Manifest::SCHEMA,
        id: PluginId::parse("eugen/probe").expect("a literal that parses"),
        name: "Probe",
        description: "d",
        version: "1.0.0",
        tier: crook_plugin::Tier::Wasm,
        capabilities: &[crook_plugin_api::Capability::ReadWorkingDirectory],
    };

    promised(&release, &manifest).expect("the module the list described");

    // A version that is not the version offered: what somebody read the
    // capability list *of* is that version, and this is another one.
    let mut newer = release.clone();
    newer.version = String::from("1.1.0");
    let refusal = promised(&newer, &manifest).expect_err("a different version");
    assert!(
        refusal.contains("1.1.0") && refusal.contains("1.0.0"),
        "{refusal}"
    );

    // And a module that wants something the row did not say it wanted.
    let mut quieter = release.clone();
    quieter.capabilities = Vec::new();
    let refusal = promised(&quieter, &manifest).expect_err("more than was offered");
    assert!(refusal.contains("cwd.read"), "{refusal}");
    assert!(refusal.contains("nothing"), "{refusal}");
}

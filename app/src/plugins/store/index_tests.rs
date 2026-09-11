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
    assert_eq!(
        offers[0].withdrawn, None,
        "a plugin with a version left to offer is not a withdrawn plugin"
    );
}

#[test]
fn a_plugin_whose_every_version_was_withdrawn_says_that_rather_than_nothing() {
    // The difference between two sentences a person can act on: "nothing is
    // built for this Crook" is somebody's to fix by publishing, and "it was
    // taken back, because —" is a thing to read.
    let mut index = parse(ONE.as_bytes()).expect("it should parse");
    for release in &mut index.plugins[0].versions {
        release.yanked = Some(String::from("it read the wrong file"));
    }

    let offers = offers(&index);

    assert!(!offers[0].installable());
    assert_eq!(
        offers[0].withdrawn.as_deref(),
        Some("it read the wrong file")
    );
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
fn the_pictures_a_registry_lists_are_read_and_their_absence_is_not_a_hole() {
    // An icon rides in the list as base64 so a face beside every name costs
    // no request; a version's previews ride as sizes so a card can reserve
    // the room. Neither was there before, and an index without them is the
    // index every Crook read yesterday.
    let index = parse(
        br#"{"schema": 1, "plugins": [
             {"id": "eugen/probe", "name": "Probe", "description": "d", "icon": "aGk=",
              "versions": [{"version": "1.0.0", "abi": 8, "url": "https://x.invalid/p.wasm",
                            "sha256": "aa", "previews": [{"width": 640, "height": 128}]}]}]}"#,
    )
    .expect("it should parse");

    let offered = offers(&index);
    assert_eq!(offered[0].icon.as_deref(), Some("aGk="));
    assert_eq!(
        offered[0].release.as_ref().unwrap().previews,
        [PreviewSize {
            width: 640,
            height: 128
        }]
    );

    let without = parse(ONE.as_bytes()).expect("it should parse");
    assert_eq!(without.plugins[0].icon, None);
    assert!(without.plugins[0].versions[0].previews.is_empty());
    assert_eq!(offers(&without)[0].icon, None);
}

/// An offer of `release`, or of nothing, for the tests about what it means.
fn offered(release: Option<&str>) -> Offer {
    Offer {
        id: PluginId::parse("eugen/probe").expect("a literal that parses"),
        name: String::from("Probe"),
        description: String::from("d"),
        repository: String::new(),
        license: String::new(),
        icon: None,
        release: release.map(|version| Release {
            version: version.to_owned(),
            abi: 8,
            url: String::from("https://x.invalid/p.wasm"),
            sha256: String::from("aa"),
            bytes: 0,
            capabilities: Vec::new(),
            asks: Vec::new(),
            yanked: None,
            previews: Vec::new(),
        }),
        newest_anywhere: None,
        withdrawn: None,
    }
}

#[test]
fn what_an_offer_means_depends_on_what_is_installed_and_whether_it_was_taken_back() {
    // The case a comparison gets wrong: a person on a withdrawn 0.10.0 is
    // offered 0.9.0, which is older, and it is still the version they should
    // be on — the registry took theirs back.
    let offer = offered(Some("0.9.0"));
    assert_eq!(change(&offer, Some("0.10.0"), false), Change::Current);
    assert!(matches!(
        change(&offer, Some("0.10.0"), true),
        Change::Replace(release) if release.version == "0.9.0"
    ));

    // A pre-release is older than the release it precedes.
    let offer = offered(Some("1.0.0"));
    assert!(matches!(
        change(&offer, Some("1.0.0-rc.1"), false),
        Change::Update(release) if release.version == "1.0.0"
    ));
    assert_eq!(change(&offer, Some("1.0.0"), false), Change::Current);
    assert!(matches!(
        change(&offer, None, false),
        Change::Install(release) if release.version == "1.0.0"
    ));

    // And nothing built for this vocabulary is nothing, installed or not.
    let offer = offered(None);
    assert_eq!(change(&offer, Some("1.0.0"), false), Change::Nothing);
    assert_eq!(change(&offer, None, false), Change::Nothing);
}

#[test]
fn the_updates_are_the_installed_plugins_the_registry_is_ahead_of() {
    // Natives never reach this — the caller hands over what is installed as a
    // file — and a plugin at the registry's version is not an update, nor is
    // one the registry has never heard of.
    let mut ahead = offered(Some("2.0.0"));
    let mut behind = offered(Some("1.0.0"));
    behind.id = PluginId::parse("eugen/behind").expect("a literal that parses");
    let mut replaced = offered(Some("0.9.0"));
    replaced.id = PluginId::parse("eugen/replaced").expect("a literal that parses");
    ahead.name = String::from("Ahead");
    let heard = Heard {
        offers: vec![ahead.clone(), behind.clone(), replaced.clone()],
        busy: Vec::new(),
    };
    let unknown = PluginId::parse("eugen/unknown").expect("a literal that parses");

    let updates = updates(
        &heard,
        [
            (&ahead.id, "1.0.0", false),
            (&behind.id, "1.0.0", false),
            (&replaced.id, "0.10.0", true),
            (&unknown, "1.0.0", false),
        ],
    );

    let named: Vec<(String, String)> = updates
        .iter()
        .map(|(id, release)| (id.to_string(), release.version.clone()))
        .collect();
    assert_eq!(
        named,
        [
            (String::from("eugen/probe"), String::from("2.0.0")),
            (String::from("eugen/replaced"), String::from("0.9.0")),
        ]
    );

    // What the store is doing is looked up by id, and nothing for the rest.
    let heard = Heard {
        offers: Vec::new(),
        busy: vec![(ahead.id.clone(), Busy::Downloading)],
    };
    assert_eq!(heard.busy(&ahead.id), Some(Busy::Downloading));
    assert_eq!(heard.busy(&behind.id), None);
    assert_eq!(heard.offer(&ahead.id), None);
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
        previews: Vec::new(),
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

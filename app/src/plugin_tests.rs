//! What the plugins a release binary carries promise each other.

use super::*;
use crate::plugins;

#[test]
fn every_plugin_in_the_box_loads() {
    // The one test that would catch a plugin added to `defaults` and never
    // tried: a `build` that returns an error, or panics, is a feature missing
    // from the window with only a log line to say so.
    let host = load(plugins::defaults());

    assert!(
        host.refused().is_empty(),
        "a plugin the binary ships did not load: {:?}",
        host.refused()
    );
    assert_eq!(host.loaded().len(), plugins::defaults().len());
}

#[test]
fn the_plugins_in_the_box_have_nothing_to_complain_about() {
    // A misspelled slot name is the likeliest mistake in a plugin, and it is
    // invisible: the contribution is kept, nothing draws it, and the window
    // comes up one feature short. The audit is what turns that into a failing
    // test rather than a bug report.
    let host = load(plugins::defaults());
    let complaints: Vec<String> = host
        .audit()
        .into_iter()
        .map(|complaint| complaint.to_string())
        .collect();

    assert!(complaints.is_empty(), "{complaints:#?}");
}

#[test]
fn every_plugin_says_who_it_is() {
    let host = load(plugins::defaults());

    for manifest in host.loaded() {
        assert_eq!(manifest.schema, Manifest::SCHEMA);
        assert_eq!(manifest.tier, Tier::Native, "{} is in the box", manifest.id);
        assert!(!manifest.name.is_empty());
        // The store lists this and nothing else until somebody clicks, so a
        // plugin without one is a row a person cannot choose between.
        assert!(
            !manifest.description.is_empty(),
            "{} says nothing about itself",
            manifest.id
        );
    }
}

#[test]
fn two_plugins_cannot_have_one_name() {
    let mut names: Vec<&str> = load(plugins::defaults())
        .loaded()
        .iter()
        .map(|manifest| manifest.id.as_str())
        .collect();
    let all = names.len();
    names.sort_unstable();
    names.dedup();

    assert_eq!(names.len(), all, "two plugins in the box share an id");
}

#[test]
fn unloading_a_plugin_takes_its_contribution_off_the_header() {
    // What disabling one will do, and the whole reason a registration is a
    // guard: the slot goes back to being what it was before the plugin loaded.
    let mut host = load(plugins::defaults());
    let usage = PluginId::parse("crook/usage").expect("a literal that parses");
    assert!(!host.slots().is_empty(crate::plugins::header::HEADER_RIGHT));

    host.unload(&usage);

    assert!(
        host.slots().is_empty(crate::plugins::header::HEADER_RIGHT),
        "the chip outlived the plugin that contributed it"
    );
    assert!(!host.loaded().iter().any(|manifest| manifest.id == usage));
}

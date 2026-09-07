//! What the line a registry parses promises.

use super::*;
use crook_plugin_api::{ABI_VERSION, Capability, Manifest};

fn manifest() -> Manifest {
    Manifest {
        abi: ABI_VERSION,
        id: "theguriev/pirate".into(),
        name: "Claude Code usage".into(),
        description: "How much of the session budget is spent.".into(),
        version: "0.3.0".into(),
        capabilities: vec![
            Capability::Network(vec!["api.anthropic.com".into()]),
            Capability::ReadFiles(vec!["~/.claude/.credentials.json".into()]),
        ],
    }
}

#[test]
fn a_capability_is_described_by_the_key_a_grant_is_kept_as() {
    // The index's list and `settings.json`'s list have to be the same
    // strings, or a store that says "you already allowed this" is comparing
    // one vocabulary against another.
    let line = described(&manifest());

    assert!(
        line.contains(
            "\"capabilities\":[\"net:api.anthropic.com\",\"file:~/.claude/.credentials.json\"]"
        ),
        "{line}"
    );
    assert!(
        line.contains("\"asks\":[\"Reach api.anthropic.com\""),
        "{line}"
    );
}

#[test]
fn every_field_the_index_needs_is_on_the_line() {
    let line = described(&manifest());

    for field in [
        "\"abi\":8",
        "\"id\":\"theguriev/pirate\"",
        "\"name\":\"Claude Code usage\"",
        "\"version\":\"0.3.0\"",
    ] {
        assert!(line.contains(field), "{field} is missing from {line}");
    }
}

#[test]
fn a_name_with_a_quote_in_it_is_still_one_line_of_json() {
    // Every string here came out of a stranger's module. A registry job that
    // has to parse this would blame its own script for the syntax error.
    let mut manifest = manifest();
    manifest.name = "the \"good\" one\nwith a newline".into();
    manifest.description = "a tab\there and a backslash \\ there".into();

    let line = described(&manifest);

    assert!(!line.contains('\n'), "{line}");
    assert!(line.contains(r#"\"good\""#), "{line}");
    assert!(line.contains(r"\\"), "{line}");
    assert!(line.contains(r"\t"), "{line}");
}

#[test]
fn a_control_character_is_escaped_rather_than_written() {
    assert_eq!(quoted("a\u{1}b"), "\"a\\u0001b\"");
}

#[test]
fn a_plugin_that_asks_for_nothing_says_so_with_two_empty_lists() {
    let mut manifest = manifest();
    manifest.capabilities.clear();

    let line = described(&manifest);

    assert!(line.contains("\"capabilities\":[]"), "{line}");
    assert!(line.contains("\"asks\":[]"), "{line}");
}

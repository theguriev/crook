//! What the line a registry parses promises.

use super::*;
use crook_plugin_api::{ABI_VERSION, Capability, Manifest, to_bytes};

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

/// A module that says it is `manifest` and speaks `abi`.
///
/// Assembled here rather than copied from the sandbox's own tests, because
/// those are `#[cfg(test)]` inside the library and a binary links the library
/// as a stranger would.
fn module(manifest: &Manifest, abi: u32) -> Vec<u8> {
    let encoded = to_bytes(manifest).expect("a manifest should encode");
    let at = 16;
    let escaped: String = encoded.iter().map(|byte| format!("\\{byte:02x}")).collect();

    let text = format!(
        r#"(module
            (memory (export "memory") 1)
            (data (i32.const {at}) "{escaped}")
            (func (export "crook_abi_version") (result i32) (i32.const {abi}))
            (func (export "crook_alloc") (param i32) (result i32) (i32.const 4096))
            (func (export "crook_manifest") (result i64)
              (i64.or (i64.shl (i64.const {at}) (i64.const 32)) (i64.const {len}))))"#,
        len = encoded.len(),
    );

    wat::parse_str(&text).expect("the test module should assemble")
}

#[test]
fn a_module_this_reader_speaks_answers_with_json_and_nothing_else() {
    let answered = read(&module(&manifest(), ABI_VERSION));

    assert_eq!(answered.code, 0);
    assert_eq!(answered.problem, None, "nothing belongs on stderr");
    let line = answered.line.expect("a line of JSON");
    assert!(line.starts_with('{') && line.ends_with('}'), "{line}");
    assert!(line.contains("\"id\":\"theguriev/pirate\""), "{line}");
}

#[test]
fn a_module_for_another_abi_answers_with_that_number_and_exit_2() {
    // The contract a registry indexes with: code 2 says "not this reader's
    // ABI", and the number is on stdout so the index can record which reader
    // to use rather than writing the artifact off as broken.
    let mut manifest = manifest();
    manifest.abi = ABI_VERSION + 1;

    let answered = read(&module(&manifest, ABI_VERSION + 1));

    assert_eq!(answered.code, 2);
    assert_eq!(
        answered.line.as_deref(),
        Some(format!("{{\"abi\":{}}}", ABI_VERSION + 1)).as_deref()
    );
    assert!(
        answered
            .problem
            .expect("a line for the log")
            .contains("built for plugin API"),
        "the human half says which reader it wants"
    );
}

#[test]
fn something_that_is_not_a_plugin_answers_with_nothing_on_stdout_and_exit_1() {
    // A registry pipes stdout into its index. Anything printed there for a
    // file that is not a plugin is a row it will try to parse.
    let answered = read(b"this is not a plugin");

    assert_eq!(answered.code, 1);
    assert_eq!(answered.line, None);
    assert!(answered.problem.is_some());
}

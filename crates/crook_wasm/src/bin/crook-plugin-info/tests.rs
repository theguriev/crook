//! What the line a registry parses promises.

use super::*;
use crook_plugin_api::{ABI_VERSION, Capability, Manifest, to_bytes};
use crook_wasm::Preview;

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
    let line = described(&manifest(), &Pictures::default());

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
    let line = described(&manifest(), &Pictures::default());

    for field in [
        "\"abi\":8",
        "\"id\":\"theguriev/pirate\"",
        "\"name\":\"Claude Code usage\"",
        "\"version\":\"0.3.0\"",
        "\"previews\":[]",
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

    let line = described(&manifest, &Pictures::default());

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

    let line = described(&manifest, &Pictures::default());

    assert!(line.contains("\"capabilities\":[]"), "{line}");
    assert!(line.contains("\"asks\":[]"), "{line}");
}

/// A module that says it is `manifest` and speaks `abi`.
///
/// Assembled here rather than copied from the sandbox's own tests, because
/// those are `#[cfg(test)]` inside the library and a binary links the library
/// as a stranger would.
fn module(manifest: &Manifest, abi: u32) -> Vec<u8> {
    module_carrying(manifest, abi, &[])
}

/// The same, with `sections` as custom sections after the code.
fn module_carrying(manifest: &Manifest, abi: u32, sections: &[(&str, &[u8])]) -> Vec<u8> {
    let encoded = to_bytes(manifest).expect("a manifest should encode");
    let at = 16;
    let custom: String = sections
        .iter()
        .map(|(name, data)| format!("(@custom {name:?} \"{}\")\n", escaped(data)))
        .collect();

    let text = format!(
        r#"(module
            (memory (export "memory") 1)
            (data (i32.const {at}) "{escaped}")
            (func (export "crook_abi_version") (result i32) (i32.const {abi}))
            (func (export "crook_alloc") (param i32) (result i32) (i32.const 4096))
            (func (export "crook_manifest") (result i64)
              (i64.or (i64.shl (i64.const {at}) (i64.const 32)) (i64.const {len})))
            {custom})"#,
        escaped = escaped(&encoded),
        len = encoded.len(),
    );

    wat::parse_str(&text).expect("the test module should assemble")
}

/// `bytes` as a wasm text string.
fn escaped(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("\\{byte:02x}")).collect()
}

/// A PNG that is all header — the signature, an IHDR saying `width`×`height`
/// and an IEND — which is all the reader looks at.
fn png(width: u32, height: u32) -> Vec<u8> {
    let mut png = b"\x89PNG\r\n\x1a\n".to_vec();
    png.extend_from_slice(&[0, 0, 0, 13, b'I', b'H', b'D', b'R']);
    png.extend_from_slice(&width.to_be_bytes());
    png.extend_from_slice(&height.to_be_bytes());
    png.extend_from_slice(&[8, 6, 0, 0, 0, 0, 0, 0, 0]);
    png.extend_from_slice(&[0, 0, 0, 0, b'I', b'E', b'N', b'D', 0, 0, 0, 0]);
    png
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

#[test]
fn a_plugins_pictures_are_on_the_line_as_an_icon_and_a_list_of_sizes() {
    // The icon whole, because the index is what the Store lists from and a
    // row wants its face with the list; the previews as sizes, because they
    // are drawn from the module and what the index needs is room for them.
    let icon = png(128, 128);
    let pictures = Pictures {
        icon: Some(icon.clone()),
        previews: vec![
            Preview {
                png: png(640, 128),
                width: 640,
                height: 128,
                caption: Some("The chip in the header".into()),
            },
            Preview {
                png: png(560, 1010),
                width: 560,
                height: 1010,
                caption: None,
            },
        ],
    };

    let line = described(&manifest(), &pictures);

    assert!(
        line.contains(&format!("\"asks\":[\"Reach api.anthropic.com\",\"Read ~/.claude/.credentials.json\"],\"icon\":\"{}\",", base64(&icon))),
        "{line}"
    );
    assert!(
        line.ends_with(
            "\"previews\":[{\"width\":640,\"height\":128},{\"width\":560,\"height\":1010}]}"
        ),
        "{line}"
    );
    // A caption is the module's to show, not the index's to carry.
    assert!(!line.contains("chip"), "{line}");
}

#[test]
fn a_plugin_with_no_pictures_says_so_with_an_empty_list_and_no_icon_key() {
    // No key rather than `null`, so that a line from this reader and one from
    // the reader before it say the same about every plugin published so far.
    let line = described(&manifest(), &Pictures::default());

    assert!(line.ends_with(",\"previews\":[]}"), "{line}");
    assert!(!line.contains("\"icon\""), "{line}");
}

#[test]
fn base64_is_the_standard_alphabet_with_padding() {
    // Every decoder's default, so a registry in Python and a host in Rust read
    // the same string back to the same bytes without being told which.
    assert_eq!(base64(b"crook"), "Y3Jvb2s=");
    assert_eq!(base64(b""), "");
    assert_eq!(base64(b"a"), "YQ==");
    assert_eq!(base64(b"ab"), "YWI=");
    assert_eq!(base64(b"abc"), "YWJj");
    assert_eq!(base64(b"\x89PNG\r\n\x1a\n"), "iVBORw0KGgo=");
    assert_eq!(base64(&[0xff, 0xff, 0xff]), "////");
}

#[test]
fn a_module_carrying_pictures_answers_with_them() {
    let icon = png(64, 64);
    let answered = read(&module_carrying(
        &manifest(),
        ABI_VERSION,
        &[("crook.icon", &icon), ("crook.preview.1", &png(640, 128))],
    ));

    assert_eq!(answered.code, 0);
    assert_eq!(answered.problem, None);
    let line = answered.line.expect("a line of JSON");
    assert!(
        line.contains(&format!("\"icon\":\"{}\"", base64(&icon))),
        "{line}"
    );
    assert!(
        line.contains("\"previews\":[{\"width\":640,\"height\":128}]"),
        "{line}"
    );
}

#[test]
fn a_picture_that_breaks_the_rule_is_exit_1_with_the_sentence_and_no_line() {
    // The registry's policy, as against the host's: a host would run this
    // plugin without its icon and log why; a registry refuses to publish it
    // while its author is still looking, and stdout stays empty so that
    // nothing is indexed by mistake.
    let answered = read(&module_carrying(
        &manifest(),
        ABI_VERSION,
        &[("crook.icon", &png(300, 200))],
    ));

    assert_eq!(answered.code, 1);
    assert_eq!(answered.line, None);
    assert_eq!(
        answered.problem.as_deref(),
        Some("carries a picture Crook would drop: crook.icon is 300×200 and an icon is square")
    );
}

//! What the agents hand a permission hook, as they handed it in the live
//! probe that `script/probe-permission-hooks` runs and `docs/answering.md`
//! reports, checked in under `tests/fixtures/asks` for the code that will
//! read it.
//!
//! The answering plan reads a few fields from each agent's payload, and only
//! those. A fixture that stops parsing, or loses one of them, fails here
//! rather than in whichever later test happens to load it. An agent that was
//! measured but has no fixtures fails too, and so does a fixture that
//! `docs/answering.md` cites as evidence and that is not here, so a capture
//! the findings rest on cannot be dropped without a test saying so. Each file
//! is named
//! `<agent>-<version>-<case>.json`, the version being the agent's own, since
//! a payload is only ever true of the version that sent it.

use std::fs;
use std::path::{Path, PathBuf};

use serde_json::Value;

/// The agents a fixture can be from, each with the top-level fields the plan
/// reads from its payload.
///
/// Copilot CLI's is `None`: its payload has not been captured (P2 in
/// `docs/answering.md`), and the fields it is held to come from a capture,
/// never from a guess at its names. So a Copilot fixture fails until this
/// table says what to hold it to.
const AGENTS: &[(&str, Option<&[&str]>)] = &[
    ("claude", Some(CLAUDE_AND_CODEX)),
    ("codex", Some(CLAUDE_AND_CODEX)),
    (
        "opencode",
        Some(&["id", "sessionID", "permission", "metadata"]),
    ),
    ("copilot", None),
];

/// Claude Code's and Codex's fields: the hook's event, the tool and its whole
/// input, which session asked, and the directory it asked from.
const CLAUDE_AND_CODEX: &[&str] = &[
    "hook_event_name",
    "tool_name",
    "tool_input",
    "session_id",
    "cwd",
];

/// The agents the probe has measured, which must have fixtures. The owner
/// deferred the others on 2026-10-16.
const MEASURED: &[&str] = &["claude"];

/// The directory the fixtures are in.
fn fixtures() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/asks")
}

/// Every fixture, by file name, with its text. A missing directory is a
/// failure rather than nothing to check.
fn every_fixture() -> Vec<(String, String)> {
    let directory = fixtures();
    let entries = fs::read_dir(&directory)
        .unwrap_or_else(|error| panic!("no fixtures at {}: {error}", directory.display()));
    let mut found: Vec<_> = entries
        .map(|entry| entry.expect("the fixtures directory lists").path())
        .map(|path| {
            let name = path
                .file_name()
                .and_then(|name| name.to_str())
                .expect("a fixture's name is text")
                .to_owned();
            let text = fs::read_to_string(&path)
                .unwrap_or_else(|error| panic!("{name} does not read as text: {error}"));
            (name, text)
        })
        .collect();
    found.sort();
    found
}

/// The agent and version a fixture's name gives, or why it gives neither.
fn agent_and_version(name: &str) -> Result<(&str, &str), String> {
    let stem = name
        .strip_suffix(".json")
        .ok_or_else(|| format!("{name} is not a .json file"))?;
    let mut parts = stem.splitn(3, '-');
    let (Some(agent), Some(version), Some(case)) = (parts.next(), parts.next(), parts.next())
    else {
        return Err(format!("{name} is not <agent>-<version>-<case>.json"));
    };
    let numbered = version
        .split('.')
        .all(|number| !number.is_empty() && number.bytes().all(|byte| byte.is_ascii_digit()));
    if !numbered {
        return Err(format!("{name}: {version} is not a version"));
    }
    let named = !case.is_empty()
        && case
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-');
    if !named {
        return Err(format!("{name}: {case} is not a case of lower-case words"));
    }
    Ok((agent, version))
}

/// A probe that ran is only evidence while its captures are here: the later
/// milestones' tests load them, and docs/answering.md names them.
#[test]
fn every_agent_the_probe_measured_has_fixtures() {
    let found = every_fixture();
    for agent in MEASURED {
        let theirs = found
            .iter()
            .filter(|(name, _)| agent_and_version(name).is_ok_and(|(of, _)| of == *agent))
            .count();
        assert!(
            theirs > 0,
            "no fixture for {agent} in {}",
            fixtures().display()
        );
    }
}

/// The fixtures `text` names: every `<agent>-<version>-<case>.json` in it, a
/// `{a,b}` group in a name standing for one name a choice, as a shell expands
/// it.
fn fixtures_named_in(text: &str) -> Vec<String> {
    let in_name = |c: char| c.is_ascii_alphanumeric() || "-._{},".contains(c);
    let mut named = Vec::new();
    for (agent, _) in AGENTS {
        for (at, _) in text.match_indices(&format!("{agent}-")) {
            let joined = text[..at]
                .chars()
                .next_back()
                .is_some_and(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_');
            if joined {
                continue;
            }
            let rest = &text[at..];
            let end = rest.find(|c: char| !in_name(c)).unwrap_or(rest.len());
            let name = rest[..end].trim_end_matches(['.', ',']);
            if name.ends_with(".json") {
                named.extend(expanded(name));
            }
        }
    }
    named.sort();
    named.dedup();
    named
}

/// `name` with its first `{a,b}` group made one name a choice, and so on for
/// the groups after it.
fn expanded(name: &str) -> Vec<String> {
    let Some(open) = name.find('{') else {
        return vec![name.to_owned()];
    };
    let close = open
        + name[open..]
            .find('}')
            .unwrap_or_else(|| panic!("{name} opens a {{ group it never closes"));
    name[open + 1..close]
        .split(',')
        .flat_map(|choice| expanded(&format!("{}{choice}{}", &name[..open], &name[close + 1..])))
        .collect()
}

/// docs/answering.md gives a fixture as the evidence for a finding, and the
/// later milestones' tests load the ones it names, so each has to be here.
#[test]
fn every_fixture_docs_answering_cites_is_here() {
    let page = Path::new(env!("CARGO_MANIFEST_DIR")).join("../docs/answering.md");
    let text = fs::read_to_string(&page)
        .unwrap_or_else(|error| panic!("{} does not read: {error}", page.display()));
    let here: Vec<String> = every_fixture().into_iter().map(|(name, _)| name).collect();
    let cited = fixtures_named_in(&text);
    assert!(
        !cited.is_empty(),
        "docs/answering.md cites no fixture, so no finding rests on a capture"
    );
    for name in cited {
        assert!(
            here.contains(&name),
            "docs/answering.md cites {name}, which is not in {}",
            fixtures().display()
        );
    }
}

/// The names in a page are found whether they stand alone, in a path, or in
/// a `{a,b}` group, and a word that only looks like one is not.
#[test]
fn a_fixture_is_found_by_the_name_the_page_gives_it() {
    let page = "fixtures `claude-2.1.280-bash.json` and `claude-2.1.280-{write,edit}.json`, \
                app/tests/fixtures/asks/codex-0.149.1-shell.json. Not crook-claude-1.0-x.json, \
                nor claude-2.1.280, nor `<agent>-<version>-<case>.json`.";
    assert_eq!(
        vec![
            "claude-2.1.280-bash.json",
            "claude-2.1.280-edit.json",
            "claude-2.1.280-write.json",
            "codex-0.149.1-shell.json",
        ],
        fixtures_named_in(page)
    );
}

/// The name is the only place the version a payload came from is kept, and
/// the agent it names decides what the payload is held to.
#[test]
fn every_fixture_is_named_for_a_known_agent_its_version_and_a_case() {
    for (name, _) in every_fixture() {
        let (agent, _) = agent_and_version(&name).unwrap_or_else(|why| panic!("{why}"));
        assert!(
            AGENTS.iter().any(|(known, _)| *known == agent),
            "{name} names {agent}, which is none of the agents"
        );
    }
}

/// What the plan reads has to be there, and in the shape it reads it: an
/// object for the tool's input, text for the session, an absolute path for
/// the directory.
#[test]
fn every_fixture_parses_and_carries_the_fields_the_plan_reads() {
    for (name, text) in every_fixture() {
        let (agent, _) = agent_and_version(&name).unwrap_or_else(|why| panic!("{why}"));
        let fields = AGENTS
            .iter()
            .find(|(known, _)| *known == agent)
            .and_then(|(_, fields)| *fields)
            .unwrap_or_else(|| {
                panic!("{name}: nobody has measured what {agent}'s payload carries, so there is nothing to hold it to")
            });
        let payload: Value = serde_json::from_str(&text)
            .unwrap_or_else(|error| panic!("{name} is not JSON: {error}"));
        let object = payload
            .as_object()
            .unwrap_or_else(|| panic!("{name} is not a JSON object"));
        for field in fields {
            let value = object
                .get(*field)
                .unwrap_or_else(|| panic!("{name} has no {field}"));
            assert!(!value.is_null(), "{name}: {field} is null");
        }
        if fields == CLAUDE_AND_CODEX {
            assert!(
                object["tool_input"].is_object(),
                "{name}: tool_input is not an object"
            );
            assert!(
                object["session_id"].is_string(),
                "{name}: session_id is not text"
            );
            let cwd = object["cwd"].as_str().unwrap_or_default();
            assert!(
                cwd.starts_with('/'),
                "{name}: cwd {cwd:?} is not an absolute path"
            );
        }
    }
}

/// The captures came from a real home directory, and the only one a fixture
/// may name is the made-up one the scrub puts in its place.
#[test]
fn no_fixture_names_a_real_home_directory() {
    for (name, text) in every_fixture() {
        for home in ["/home/", "/Users/"] {
            for (at, _) in text.match_indices(home) {
                let rest = &text[at + home.len()..];
                assert!(
                    rest.starts_with("someone/") || rest.starts_with("someone\""),
                    "{name} names a home directory other than {home}someone: {}",
                    text[at..].chars().take(40).collect::<String>()
                );
            }
        }
    }
}

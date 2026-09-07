//! What a plugin says about itself, as one line of JSON.
//!
//! ```text
//! crook-plugin-info plugin.wasm
//! {"abi":8,"id":"theguriev/pirate","name":"Claude Code usage",…}
//! ```
//!
//! A registry has to describe an artifact it has just built — the id, the
//! version, the ABI it speaks and what it asks to be allowed to do — and every
//! one of those is *inside* the module rather than beside it, readable only by
//! instantiating it and calling two exports. That read is [`crook_wasm`], and
//! a second implementation of it somewhere else is a second thing to keep in
//! step with the host that decides whether a plugin loads at all. So the
//! registry runs the host's own reader, from the crate the host links.
//!
//! # What the exit code says
//!
//! * **0** — read; the JSON is on stdout.
//! * **2** — the module speaks a different ABI. Its number is on stdout, and
//!   nothing else is: the manifest under it is encoded against a shape this
//!   reader does not have, so answering with any of it would be a guess. An
//!   index built by the reader of one ABI can therefore *notice* an artifact
//!   for another rather than treating it as broken.
//! * **1** — not a plugin, or not readable.
//!
//! # The JSON, and why it is written by hand
//!
//! Six fields and two lists of strings. A serialiser would be a dependency
//! this crate does not otherwise have, in a library every plugin host links,
//! for one printer nothing else needs — and `serde_json` behind a feature is a
//! target `cargo clippy --all-targets` does not build, which is a binary
//! nobody's CI compiles.

use std::process::ExitCode;

use crook_wasm::{Fuel, Problem, Sandbox};

fn main() -> ExitCode {
    let mut arguments = std::env::args().skip(1);
    let Some(path) = arguments.next() else {
        eprintln!("usage: crook-plugin-info <plugin.wasm>");
        return ExitCode::from(1);
    };
    if let Some(extra) = arguments.next() {
        eprintln!("crook-plugin-info reads one module, and was given {extra:?} as well");
        return ExitCode::from(1);
    }

    let bytes = match std::fs::read(&path) {
        Ok(bytes) => bytes,
        Err(why) => {
            eprintln!("{path}: {why}");
            return ExitCode::from(1);
        }
    };

    match Sandbox::open(&bytes, Fuel::default()) {
        Ok((_, manifest)) => {
            println!("{}", described(&manifest));
            ExitCode::SUCCESS
        }
        // Not an error the way the two below are: the artifact is fine and
        // this reader is the wrong one for it. Say which one it wants.
        Err(Problem::Abi { theirs, ours }) => {
            eprintln!("{path}: built for plugin API {theirs}, and this reader is {ours}");
            println!("{{\"abi\":{theirs}}}");
            ExitCode::from(2)
        }
        Err(why) => {
            eprintln!("{path}: {why}");
            ExitCode::from(1)
        }
    }
}

/// One line of JSON describing `manifest`.
fn described(manifest: &crook_plugin_api::Manifest) -> String {
    let capabilities: Vec<String> = manifest
        .capabilities
        .iter()
        .flat_map(|capability| capability.keys())
        .collect();
    let asks: Vec<String> = manifest
        .capabilities
        .iter()
        .map(|capability| capability.sentence())
        .collect();

    format!(
        "{{\"abi\":{},\"id\":{},\"name\":{},\"description\":{},\"version\":{},\
         \"capabilities\":{},\"asks\":{}}}",
        manifest.abi,
        quoted(&manifest.id),
        quoted(&manifest.name),
        quoted(&manifest.description),
        quoted(&manifest.version),
        listed(&capabilities),
        listed(&asks),
    )
}

/// `text` as a JSON string, escaped.
///
/// Every one of these strings came out of a stranger's module: a name holding
/// a quote or a newline is a line of JSON that will not parse, and the job
/// reading it would blame its own script.
fn quoted(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + 2);
    out.push('"');
    for character in text.chars() {
        match character {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            // Everything below a space has no literal form, and the two-byte
            // escapes above are the only ones with a shorthand.
            control if control < ' ' => out.push_str(&format!("\\u{:04x}", control as u32)),
            other => out.push(other),
        }
    }
    out.push('"');
    out
}

/// `items` as a JSON array of strings.
fn listed(items: &[String]) -> String {
    let mut out = String::from("[");
    for (index, item) in items.iter().enumerate() {
        if index > 0 {
            out.push(',');
        }
        out.push_str(&quoted(item));
    }
    out.push(']');
    out
}

#[cfg(test)]
mod tests;

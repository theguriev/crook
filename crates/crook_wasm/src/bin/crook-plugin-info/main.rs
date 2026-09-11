//! What a plugin says about itself, as one line of JSON.
//!
//! ```text
//! crook-plugin-info plugin.wasm
//! {"abi":8,"id":"theguriev/pirate","name":"Claude Code usage",…}
//! ```
//!
//! A registry has to describe an artifact it has just built — the id, the
//! version, the ABI it speaks, what it asks to be allowed to do, and what it
//! looks like — and every one of those is *inside* the module rather than
//! beside it, readable only by instantiating it and calling two exports, or
//! by reading its custom sections. That read is [`crook_wasm`], and a second
//! implementation of it somewhere else is a second thing to keep in step with
//! the host that decides whether a plugin loads at all. So the registry runs
//! the host's own reader, from the crate the host links.
//!
//! # What the exit code says
//!
//! * **0** — read; the JSON is on stdout.
//! * **2** — the module speaks a different ABI. Its number is on stdout, and
//!   nothing else is: the manifest under it is encoded against a shape this
//!   reader does not have, so answering with any of it would be a guess. An
//!   index built by the reader of one ABI can therefore *notice* an artifact
//!   for another rather than treating it as broken.
//! * **1** — not a plugin, or not readable, or carrying a picture that breaks
//!   the rule in [`crook_plugin_api::pictures`]. The module in the last case
//!   is a plugin a host would run without the picture; a registry is the
//!   place to refuse it instead, while its author is still looking, and the
//!   sentence on stderr says which picture and what is wrong with it.
//!
//! # The JSON, and why it is written by hand
//!
//! Six fields, two lists of strings, a list of sizes and, when the module
//! carries one, its icon as base64 — so that the Store's list of plugins,
//! which is the index, draws a face on every row without a request per row.
//! A serialiser would be a dependency this crate does not otherwise have, in
//! a library every plugin host links, for one printer nothing else needs —
//! and `serde_json` behind a feature is a target `cargo clippy --all-targets`
//! does not build, which is a binary nobody's CI compiles. Base64 is a few
//! lines, for the same reason.

use std::process::ExitCode;

use crook_wasm::{Fuel, Pictures, Problem, Sandbox};

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

    let answered = read(&bytes);
    if let Some(line) = answered.line {
        println!("{line}");
    }
    if let Some(problem) = answered.problem {
        eprintln!("{path}: {problem}");
    }
    ExitCode::from(answered.code)
}

/// What the reader says about one module: the line, the complaint, the code.
///
/// Split out from [`main`] so that the three of them can be *tested*. They are
/// the interface a registry consumes — a job reads the exit code, parses
/// stdout and logs stderr — and an interface with no test is an interface that
/// changes by accident.
struct Answered {
    /// What goes on stdout, which is always JSON when there is any.
    line: Option<String>,
    /// What goes on stderr, for a person reading a CI log.
    problem: Option<String>,
    /// What the shell is told.
    code: u8,
}

/// Reads one module.
fn read(bytes: &[u8]) -> Answered {
    match Sandbox::open_with_pictures(bytes, Fuel::default()) {
        Ok((_, manifest, Ok(pictures))) => Answered {
            line: Some(described(&manifest, &pictures)),
            problem: None,
            code: 0,
        },
        // A host would run this plugin and log the sentence; a registry is
        // where it is refused, because here its author is reading.
        Ok((_, _, Err(sentence))) => Answered {
            line: None,
            problem: Some(format!("carries a picture Crook would drop: {sentence}")),
            code: 1,
        },
        // Not an error the way the one below is: the artifact is fine and this
        // reader is the wrong one for it. Say which one it wants, on stdout,
        // so that an index built by the reader of one ABI can notice an
        // artifact for another rather than treating it as broken.
        Err(Problem::Abi { theirs, ours }) => Answered {
            line: Some(format!("{{\"abi\":{theirs}}}")),
            problem: Some(format!(
                "built for plugin API {theirs}, and this reader is {ours}"
            )),
            code: 2,
        },
        Err(why) => Answered {
            line: None,
            problem: Some(why.to_string()),
            code: 1,
        },
    }
}

/// One line of JSON describing `manifest` and the `pictures` beside it.
///
/// The icon goes on the line whole, because the index is what the Store
/// lists from and a row wants its face with the list; the previews go on it
/// as sizes only, because they are drawn from the module itself and what the
/// index needs is room to reserve for them. `icon` is absent rather than
/// `null` when there is none, so an index built by the reader before this
/// one and one built by this reader say the same thing about a plugin with
/// no icon.
fn described(manifest: &crook_plugin_api::Manifest, pictures: &Pictures) -> String {
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
    let icon = pictures
        .icon
        .as_deref()
        .map(|png| format!(",\"icon\":\"{}\"", base64(png)))
        .unwrap_or_default();
    let previews: Vec<String> = pictures
        .previews
        .iter()
        .map(|preview| {
            format!(
                "{{\"width\":{},\"height\":{}}}",
                preview.width, preview.height
            )
        })
        .collect();

    format!(
        "{{\"abi\":{},\"id\":{},\"name\":{},\"description\":{},\"version\":{},\
         \"capabilities\":{},\"asks\":{}{icon},\"previews\":[{}]}}",
        manifest.abi,
        quoted(&manifest.id),
        quoted(&manifest.name),
        quoted(&manifest.description),
        quoted(&manifest.version),
        listed(&capabilities),
        listed(&asks),
        previews.join(","),
    )
}

/// `bytes` as base64, the standard alphabet with padding (RFC 4648 §4).
///
/// Written here rather than depended on, for the reason the JSON is: one
/// encoder for one key, in a crate every plugin host links. The alphabet is
/// the one every decoder assumes when none is named, and the padding is what
/// lets a decoder check the length before it reads.
fn base64(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for group in bytes.chunks(3) {
        let packed = group.iter().enumerate().fold(0u32, |packed, (at, byte)| {
            packed | u32::from(*byte) << (16 - 8 * at)
        });
        // Four sextets, of which a short final group fills the first two or
        // three; the rest is padding.
        for place in 0..4 {
            if place <= group.len() {
                let sextet = (packed >> (18 - 6 * place)) & 0x3f;
                out.push(char::from(ALPHABET[sextet as usize]));
            } else {
                out.push('=');
            }
        }
    }
    out
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

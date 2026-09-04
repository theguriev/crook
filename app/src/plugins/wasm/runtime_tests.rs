//! What a plugin is allowed to ask for, and where a path actually points.
//!
//! The two halves of this file are the two ways a grant can be wrong. One is
//! a request that names something nobody allowed — the ordinary case, and the
//! one a plugin is expected to hit on its first run. The other is a request
//! that names something a person *did* allow while meaning something else,
//! which is the case worth writing tests about.

use super::*;

use crook_plugin_api::{Method, Request};

/// What the pirate asks for, as it asks for it.
fn fetch(url: &str) -> Request {
    Request::Fetch {
        method: Method::Get,
        url: url.to_owned(),
        headers: vec![("authorization".into(), "Bearer …".into())],
        body: None,
    }
}

fn read(path: &str) -> Request {
    Request::ReadFile {
        path: path.to_owned(),
    }
}

#[test]
fn nothing_is_allowed_to_a_plugin_nobody_has_answered_for() {
    // The state every plugin installs in. A manifest asking for something is
    // not a person allowing it.
    let refusal = allowed(&[], &fetch("https://api.anthropic.com/api/oauth/usage"))
        .expect_err("an ungranted plugin should be refused");

    // And the refusal is the sentence the dialog used, so the plugin can say
    // what to go and allow rather than "something went wrong".
    assert_eq!(refusal, "Reach api.anthropic.com");
}

#[test]
fn a_granted_host_is_reached_and_its_neighbours_are_not() {
    let granted = vec![String::from("net:api.anthropic.com")];

    assert!(
        allowed(
            &granted,
            &fetch("https://api.anthropic.com/api/oauth/usage")
        )
        .is_ok()
    );
    // A different host on the same domain is a different host.
    assert!(allowed(&granted, &fetch("https://evil.anthropic.com/")).is_err());
    // And so is one that merely starts the same way.
    assert!(allowed(&granted, &fetch("https://api.anthropic.com.evil.example/")).is_err());
}

#[test]
fn a_url_that_hides_its_host_behind_a_name_somebody_granted() {
    // `https://api.anthropic.com@evil.example/` is a request to *evil.example*
    // with a username of `api.anthropic.com`. A check that read the front of
    // the authority would hand it a permission somebody gave to Anthropic.
    assert_eq!(
        host_of("https://api.anthropic.com@evil.example/x"),
        Ok(String::from("evil.example"))
    );
    assert!(
        allowed(
            &[String::from("net:api.anthropic.com")],
            &fetch("https://api.anthropic.com@evil.example/x")
        )
        .is_err()
    );
}

#[test]
fn a_host_is_the_authority_without_its_port_and_in_one_case() {
    assert_eq!(
        host_of("https://API.Anthropic.com:443/api/oauth/usage?x=1"),
        Ok(String::from("api.anthropic.com"))
    );
}

#[test]
fn only_https_is_reached_at_all() {
    // Not a capability question: there is no capability that grants sending a
    // token over a cleartext connection.
    assert!(host_of("http://api.anthropic.com/").is_err());
    assert!(host_of("file:///etc/passwd").is_err());
    assert!(host_of("https:///nothing").is_err());
}

#[test]
fn a_granted_path_is_the_path_that_was_granted() {
    let granted = vec![String::from("file:~/.claude/.credentials.json")];

    assert!(allowed(&granted, &read("~/.claude/.credentials.json")).is_ok());
    assert!(allowed(&granted, &read("~/.ssh/id_ed25519")).is_err());
    // Not a prefix, and not a directory: the grant is the text of one file.
    assert!(allowed(&granted, &read("~/.claude/")).is_err());
}

#[test]
fn a_path_that_can_walk_is_not_a_path_this_resolves() {
    // The grant is the text of the path, so a path that can walk out of it is
    // a grant that means something other than what a person read.
    assert_eq!(resolve("~/.claude/../.ssh/id_ed25519"), None);
    assert_eq!(resolve("/etc/../etc/passwd"), None);
    assert_eq!(resolve(".."), None);
}

#[test]
fn a_tilde_is_the_home_directory_and_nothing_else_is_expanded() {
    let home = dirs::home_dir().expect("this machine has a home directory");

    assert_eq!(
        resolve("~/.claude/.credentials.json"),
        Some(home.join(".claude/.credentials.json"))
    );
    // Only a leading `~/`. A file actually called `~` is a file called `~`.
    assert_eq!(
        resolve("/tmp/~/thing"),
        Some(std::path::PathBuf::from("/tmp/~/thing"))
    );
}

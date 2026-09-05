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

/// A listing of one directory, as the picker asks for it.
fn list(path: &str) -> Request {
    Request::List {
        path: path.to_owned(),
    }
}

/// One command typed, as choosing a row asks for it.
fn typed(template: &str, argument: &str) -> Request {
    Request::Type {
        template: template.to_owned(),
        argument: argument.to_owned(),
    }
}

#[test]
fn a_listing_is_allowed_anywhere_under_a_root_somebody_granted() {
    // The one grant that is not a string comparison: what was allowed is a
    // root and what is asked for is somewhere under it.
    let granted = vec![String::from("list:~")];

    assert!(allowed(&granted, &list("~/Work/crook")).is_ok());
    assert!(allowed(&granted, &list("~")).is_ok());
    // And the same directory written the way the host resolves it, because
    // `~/Work` and `/home/…/Work` are one directory and a plugin that asked
    // for either asked for what it was allowed.
    if let Some(home) = dirs::home_dir() {
        let inside = home.join("Work").to_string_lossy().into_owned();
        assert!(allowed(&granted, &list(&inside)).is_ok());
    }
}

#[test]
fn a_listing_outside_every_granted_root_is_refused_by_the_sentence_it_wanted() {
    let granted = vec![String::from("list:~/Work")];

    let refusal =
        allowed(&granted, &list("/etc")).expect_err("nothing granted /etc and it is not under ~");

    assert_eq!(refusal, "See the names of the files in /etc");
}

#[test]
fn a_listing_cannot_walk_out_of_the_root_it_was_granted() {
    // The same rule `resolve` applies to a file: a path that can walk is a
    // grant that means something other than what it says. Refused rather than
    // normalised, so there is nothing to get subtly wrong.
    let granted = vec![String::from("list:~/Work")];

    assert!(allowed(&granted, &list("~/Work/../.ssh")).is_err());
}

#[test]
fn only_the_exact_command_somebody_allowed_may_be_typed() {
    // A template is the *shape* of the command, and it is the shape a person
    // agreed to. A plugin that was allowed to `cd` may not `git push`, however
    // it spells it.
    let granted = vec![String::from("type:cd {}")];

    assert!(allowed(&granted, &typed("cd {}", "/tmp")).is_ok());
    assert_eq!(
        allowed(&granted, &typed("git push {}", "--force")).expect_err("nobody allowed that one"),
        "Type into your shell, and run: git push \u{2026}"
    );
}

#[test]
fn a_command_of_crooks_own_is_allowed_by_name_and_no_other() {
    let granted = vec![String::from("run:crook/shortcuts/rebind")];
    let run = |name: &str| Request::Run {
        name: name.to_owned(),
        argument: String::new(),
    };

    assert!(allowed(&granted, &run("crook/shortcuts/rebind")).is_ok());
    assert!(allowed(&granted, &run("crook/window/close-window")).is_err());
}

#[test]
fn the_two_requests_that_change_something_are_the_two_that_need_a_press() {
    // What makes typing into somebody's shell an acceptable thing for a
    // stranger's plugin to be able to do: it only happens when a person did
    // something. Reading is not on the list — a chip that says where you are
    // has to be able to ask on a timer.
    assert!(changes_something(&typed("cd {}", "/tmp")));
    assert!(changes_something(&Request::Run {
        name: String::from("crook/window/new-tab"),
        argument: String::new(),
    }));

    assert!(!changes_something(&Request::Where));
    assert!(!changes_something(&list("~")));
    assert!(!changes_something(&Request::Commands));
}

#[test]
fn a_listing_answers_with_the_names_and_nothing_about_them() {
    let scratch = super::super::tests::Scratch::new("listing");
    std::fs::create_dir_all(scratch.path().join("app")).expect("a directory should be creatable");
    std::fs::write(scratch.path().join("Cargo.toml"), b"[package]\n")
        .expect("a file should be writable");

    let answer = list_directory(&scratch.path().to_string_lossy());

    let Answer::Listed(entries) = answer else {
        panic!("a listing should answer with names, not {answer:?}");
    };
    // Directories first, then files, each by name: the order every file
    // picker has used since the first one.
    assert_eq!(
        entries,
        vec![
            Entry {
                name: String::from("app"),
                directory: true
            },
            Entry {
                name: String::from("Cargo.toml"),
                directory: false
            },
        ]
    );
}

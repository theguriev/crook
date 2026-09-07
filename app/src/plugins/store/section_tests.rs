//! What the store says about where a plugin stands.
//!
//! The arithmetic, which is where it can be wrong without looking wrong: what
//! a row's last word is, what the label beside the button says, and which card
//! a sentence belongs on. What the section *draws* is proved by drawing it —
//! `--section Store --snapshot` — and not here.

use super::*;
use crate::plugins::store::index::Release;

fn release(version: &str) -> Release {
    Release {
        version: version.to_owned(),
        abi: 8,
        url: "https://x.invalid/p.wasm".into(),
        sha256: "aa".into(),
        bytes: 0,
        capabilities: Vec::new(),
        asks: Vec::new(),
        yanked: None,
    }
}

fn offer(release: Option<Release>) -> Offer {
    Offer {
        id: PluginId::parse("eugen/probe").expect("a literal that parses"),
        name: "Probe".into(),
        description: "d".into(),
        repository: String::new(),
        license: String::new(),
        release,
        newest_anywhere: None,
        withdrawn: None,
    }
}

#[test]
fn a_registry_that_is_behind_is_not_an_update() {
    // The state a yank leaves behind: the newest version anybody can install
    // is older than the one already here. Comparing the two as *text* would
    // offer "Update to 0.9.0" to somebody running 0.10.0.
    let offered = offer(Some(release("0.9.0")));

    assert_eq!(
        standing_label(&offered, Some("0.10.0"), false),
        "Version 0.10.0"
    );
    assert_eq!(
        standing_label(&offered, Some("0.9.0"), false),
        "Version 0.9.0"
    );
    assert_eq!(
        standing_label(&offer(Some(release("0.10.0"))), Some("0.9.0"), false),
        "0.9.0 installed, 0.10.0 out"
    );
}

#[test]
fn a_plugin_that_was_taken_back_says_why_rather_than_which_crook() {
    let mut withdrawn = offer(None);
    withdrawn.withdrawn = Some(String::from("it read the wrong file"));

    assert_eq!(
        standing_label(&withdrawn, Some("0.9.0"), false),
        "Taken back: it read the wrong file"
    );
}

#[test]
fn a_download_in_flight_says_so_and_nothing_else() {
    assert_eq!(
        standing_label(&offer(Some(release("0.9.0"))), None, true),
        "Downloading"
    );
}

#[test]
fn a_sentence_is_drawn_on_the_card_it_is_about_and_nowhere_else() {
    // A line about one plugin drawn on whichever card happens to be showing is
    // a sentence about the wrong thing, and on a page about *installing* that
    // is worse than no sentence at all.
    let probe = PluginId::parse("eugen/probe").expect("a literal that parses");
    let other = PluginId::parse("eugen/other").expect("a literal that parses");
    let known = Known {
        offers: Vec::new(),
        looking: false,
        fetched: None,
        problem: None,
        said: Some((Some(probe.clone()), String::from("is installed"))),
        downloading: None,
    };

    assert_eq!(known.about(&probe), Some(("is installed", false)));
    assert_eq!(known.about(&other), None);
    assert_eq!(known.loose(), None, "it belongs to a row, not to the list");

    // And what is about the registry rather than a plugin goes under the list,
    // where the button it is about is.
    let registry = Known {
        problem: Some((None, String::from("could not be reached"))),
        said: None,
        ..known
    };
    assert_eq!(registry.loose(), Some(("could not be reached", true)));
    assert_eq!(registry.about(&probe), None);
}

#[test]
fn how_old_an_answer_is_reads_as_a_person_would_say_it() {
    assert_eq!(ago(0), "just now");
    assert_eq!(ago(59), "just now");
    assert_eq!(ago(60), "1 minute ago");
    assert_eq!(ago(3 * 60 + 20), "3 minutes ago");
    assert_eq!(ago(60 * 60), "1 hour ago");
    assert_eq!(ago(26 * 60 * 60), "1 day ago");
    assert_eq!(ago(6 * 24 * 60 * 60), "6 days ago");
}

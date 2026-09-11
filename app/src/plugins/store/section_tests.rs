//! What the store says about where a plugin stands.
//!
//! The arithmetic, which is where it can be wrong without looking wrong: what
//! a row's last word is, what the label beside the button says and what the
//! button does, and which card a sentence belongs on. What the section
//! *draws* is proved by drawing it — the workspace tests open it on a scratch
//! index, and `--section Store --snapshot` is the picture — and not here.

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
        previews: Vec::new(),
    }
}

fn offer(release: Option<Release>) -> Offer {
    Offer {
        id: PluginId::parse("eugen/probe").expect("a literal that parses"),
        name: "Probe".into(),
        description: "d".into(),
        repository: String::new(),
        license: String::new(),
        icon: None,
        release,
        newest_anywhere: None,
        withdrawn: None,
    }
}

/// The label beside the button, the button, and what pressing it does, for
/// an offer of `release` on a machine holding `installed`.
fn card_says(
    offered: &Offer,
    installed: Option<&str>,
    withdrawn: Option<&str>,
    busy: Option<Busy>,
) -> (String, String, Press) {
    let decided = decided(offered, installed, withdrawn, busy);
    (decided.label, decided.button, decided.press)
}

#[test]
fn a_registry_that_is_behind_is_not_an_update() {
    // The state a yank leaves behind: the newest version anybody can install
    // is older than the one already here. Comparing the two as *text* would
    // offer "Update to 0.9.0" to somebody running 0.10.0 — the button and
    // the label used to disagree about exactly this, and now both go through
    // `change`.
    let offered = offer(Some(release("0.9.0")));

    assert_eq!(
        card_says(&offered, Some("0.10.0"), None, None),
        ("Version 0.10.0".into(), "Installed".into(), Press::Nothing)
    );
    assert_eq!(
        standing(&offered, Some("0.10.0"), false).as_deref(),
        Some("installed")
    );

    assert_eq!(
        card_says(&offered, Some("0.9.0"), None, None),
        ("Version 0.9.0".into(), "Installed".into(), Press::Nothing)
    );

    let newer = offer(Some(release("0.10.0")));
    assert_eq!(
        card_says(&newer, Some("0.9.0"), None, None),
        (
            "0.9.0 installed, 0.10.0 out".into(),
            "Update to 0.10.0".into(),
            Press::Update
        )
    );
    assert_eq!(
        standing(&newer, Some("0.9.0"), false).as_deref(),
        Some("0.10.0")
    );
}

#[test]
fn a_plugin_not_on_this_machine_is_offered_and_its_row_says_nothing_after_its_name() {
    let offered = offer(Some(release("0.9.0")));

    assert_eq!(
        card_says(&offered, None, None, None),
        ("Version 0.9.0".into(), "Install".into(), Press::Install)
    );
    assert_eq!(standing(&offered, None, false), None);
}

#[test]
fn a_version_taken_back_is_offered_its_replacement_whatever_its_number() {
    // 0.9.0 is older than the 0.10.0 that is running, and it is still what
    // the card offers: the running one was withdrawn, and a card saying
    // "Installed" would be telling somebody to stay on a version somebody
    // took back. The label says why, and the row ends in the replacement.
    let offered = offer(Some(release("0.9.0")));

    assert_eq!(
        card_says(
            &offered,
            Some("0.10.0"),
            Some("it read the wrong file"),
            None
        ),
        (
            "Taken back: it read the wrong file".into(),
            "Install 0.9.0".into(),
            Press::Update
        )
    );
    assert_eq!(
        standing(&offered, Some("0.10.0"), true).as_deref(),
        Some("0.9.0")
    );
}

#[test]
fn a_plugin_this_build_cannot_run_says_which_build_could() {
    let mut newer_crook = offer(None);
    newer_crook.newest_anywhere = Some(Release {
        abi: 9,
        ..release("2.0.0")
    });
    assert_eq!(
        card_says(&newer_crook, None, None, None),
        (
            "Built for plugin API 9".into(),
            "Not for this build".into(),
            Press::Nothing
        )
    );
    assert_eq!(
        standing(&newer_crook, None, false).as_deref(),
        Some("newer Crook")
    );
    assert_eq!(
        standing(&newer_crook, Some("1.0.0"), false).as_deref(),
        Some("newer Crook")
    );

    let nothing = offer(None);
    assert_eq!(card_says(&nothing, None, None, None).0, "Nothing built yet");
}

#[test]
fn a_plugin_that_was_taken_back_says_why_rather_than_which_crook() {
    // Every version withdrawn, so there is nothing to offer: the reason is
    // the label, and not the ABI of a version nobody may install.
    let mut withdrawn = offer(None);
    withdrawn.withdrawn = Some(String::from("it read the wrong file"));

    assert_eq!(
        card_says(
            &withdrawn,
            Some("0.9.0"),
            Some("it read the wrong file"),
            None
        )
        .0,
        "Taken back: it read the wrong file"
    );
}

#[test]
fn a_download_in_flight_says_so_and_nothing_else() {
    // Whatever the registry says: the button is dead while the store is
    // busy about the plugin, and a queued one says it is waiting rather
    // than that it is being got.
    let offered = offer(Some(release("0.9.0")));

    assert_eq!(
        card_says(&offered, None, None, Some(Busy::Downloading)),
        (
            "Downloading".into(),
            "Getting it\u{2026}".into(),
            Press::Nothing
        )
    );
    assert_eq!(
        card_says(&offered, Some("0.1.0"), None, Some(Busy::Waiting)),
        ("Waiting".into(), "Waiting\u{2026}".into(), Press::Nothing)
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
        busy: Vec::new(),
        icons: Icons::new(),
        looking_inside: None,
        looked_inside: None,
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

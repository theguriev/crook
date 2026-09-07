//! What choosing between two installed versions promises.

use super::*;

fn newest(versions: &[&str]) -> String {
    let mut versions = versions.to_vec();
    versions.sort_by(|left, right| compare(left, right));
    versions
        .last()
        .expect("something to choose from")
        .to_string()
}

#[test]
fn ten_is_newer_than_nine() {
    // The one thing sorting these as text gets catastrophically wrong, and the
    // reason this module exists at all.
    assert_eq!(newest(&["1.9.0", "1.10.0"]), "1.10.0");
    assert_eq!(newest(&["0.2.0", "0.10.0", "0.9.9"]), "0.10.0");
}

#[test]
fn a_release_outranks_its_own_pre_releases() {
    assert_eq!(newest(&["1.0.0", "1.0.0-rc.2"]), "1.0.0");
    assert_eq!(newest(&["1.0.0-rc.1", "1.0.0-rc.2"]), "1.0.0-rc.2");
    // And a pre-release of the next version is still newer than this one.
    assert_eq!(newest(&["1.0.0", "1.1.0-alpha"]), "1.1.0-alpha");
}

#[test]
fn a_missing_part_is_a_zero() {
    assert_eq!(compare("1.2", "1.2.0"), Ordering::Equal);
    assert_eq!(compare("1.2", "1.2.1"), Ordering::Less);
}

#[test]
fn build_metadata_is_not_part_of_the_answer() {
    assert_eq!(compare("1.0.0+build.7", "1.0.0"), Ordering::Equal);
    // And it does not turn the version into a pre-release either.
    assert_eq!(compare("1.0.0+a-b", "1.0.0-rc.1"), Ordering::Greater);
}

#[test]
fn something_that_is_not_a_version_still_has_an_order() {
    // Nothing here parses, and the answer still has to be the same on two
    // machines: a directory nobody can compare is a plugin that loads
    // differently depending on what `read_dir` said first.
    assert_eq!(newest(&["nightly", "stable"]), "stable");
    // A number outranks a word, because the word is the odd one out.
    assert_eq!(newest(&["nightly", "0.1.0"]), "0.1.0");
    assert_eq!(compare("what", "what"), Ordering::Equal);
}

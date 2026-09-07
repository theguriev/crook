//! Which of two versions on disk is the newer one.
//!
//! A plugin's directory holds one directory per version it has been installed
//! at, named by what the module's own manifest said. Choosing between them is
//! a comparison of two strings a stranger wrote, and there are three ways to
//! do it: parse semver and refuse anything else, take a dependency on a semver
//! crate, or compare the parts numerically and fall back to text.
//!
//! This is the third. A plugin's version is not something the host has any
//! business enforcing a grammar on — it is printed on a card and used to tell
//! two directories apart — and the one thing a plain string sort gets
//! catastrophically wrong is the thing this fixes: `1.10.0` is newer than
//! `1.9.0`, and sorting text puts it before it.
//!
//! The rules, in full, because a rule nobody can predict is worse than a
//! simple one that is occasionally surprising:
//!
//! * A `-` starts a pre-release, and `1.0.0-rc.1` is older than `1.0.0`.
//! * Everything before it is split on `.`; two parts that both parse as
//!   numbers are compared as numbers, and any other pair as text.
//! * A missing part is a zero, so `1.2` and `1.2.0` are the same version.
//! * Build metadata (`+something`) is ignored, which is what it is for.

use std::cmp::Ordering;

/// Compares two version strings, newest last.
pub fn compare(left: &str, right: &str) -> Ordering {
    let (left, left_pre) = split(left);
    let (right, right_pre) = split(right);

    let ordering = numbers(left, right);
    if ordering != Ordering::Equal {
        return ordering;
    }

    match (left_pre, right_pre) {
        (None, None) => Ordering::Equal,
        // A release outranks its own pre-releases, which is the whole reason
        // this half exists: `1.0.0-rc.1` must not shadow `1.0.0`.
        (None, Some(_)) => Ordering::Greater,
        (Some(_), None) => Ordering::Less,
        (Some(left), Some(right)) => numbers(left, right),
    }
}

/// The version proper, and its pre-release if it has one.
fn split(version: &str) -> (&str, Option<&str>) {
    // Metadata first: `1.0.0+build-7` has no pre-release, and taking the `-`
    // before the `+` would find one.
    let version = version.split('+').next().unwrap_or(version);
    match version.split_once('-') {
        Some((release, pre)) => (release, Some(pre)),
        None => (version, None),
    }
}

/// Compares two dot-separated lists part by part.
fn numbers(left: &str, right: &str) -> Ordering {
    let mut left = left.split('.');
    let mut right = right.split('.');

    loop {
        match (left.next(), right.next()) {
            (None, None) => return Ordering::Equal,
            (mine, theirs) => {
                // A part that is not there is a zero, so `1.2` and `1.2.0` are
                // one version rather than two directories.
                let mine = mine.unwrap_or("0");
                let theirs = theirs.unwrap_or("0");
                let ordering = match (mine.parse::<u64>(), theirs.parse::<u64>()) {
                    (Ok(mine), Ok(theirs)) => mine.cmp(&theirs),
                    // Anything that is not a number is compared as what it is.
                    // Two of them is a guess either way; a number against a
                    // word is not, and the number is the ordinary case.
                    (Ok(_), Err(_)) => Ordering::Greater,
                    (Err(_), Ok(_)) => Ordering::Less,
                    (Err(_), Err(_)) => mine.cmp(theirs),
                };
                if ordering != Ordering::Equal {
                    return ordering;
                }
            }
        }
    }
}

#[cfg(test)]
#[path = "version_tests.rs"]
mod tests;

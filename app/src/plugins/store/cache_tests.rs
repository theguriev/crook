//! What the copy on disk promises.

use super::*;
use crate::plugins::wasm::tests::Scratch;

const INDEX: &[u8] = br#"{"schema": 1, "plugins": [
    {"id": "eugen/probe", "name": "Probe", "description": "d",
     "versions": [{"version": "1.0.0", "abi": 8, "url": "https://x.invalid/p.wasm", "sha256": "aa"}]}]}"#;

#[test]
fn what_was_written_is_what_comes_back() {
    let scratch = Scratch::new("store-cache");
    let cache = Cache::at(scratch.path());

    cache.write(INDEX, Some("\"abc\"")).expect("it should keep");
    let cached = cache.read().expect("it should read back");

    assert_eq!(cached.index.plugins[0].id, "eugen/probe");
    assert_eq!(cached.etag.as_deref(), Some("\"abc\""));
    assert!(cached.fetched.is_some());
}

#[test]
fn nothing_cached_is_nothing_rather_than_a_failure() {
    let scratch = Scratch::new("store-empty");

    assert!(Cache::at(scratch.path()).read().is_none());
}

#[test]
fn something_that_is_not_an_index_never_replaces_one_that_is() {
    // The registry answering with a login page, a 404 body or half a file is
    // not a reason to lose the list somebody had yesterday.
    let scratch = Scratch::new("store-rubbish");
    let cache = Cache::at(scratch.path());
    cache.write(INDEX, Some("\"abc\"")).expect("it should keep");

    let refusal = cache.write(b"<html>404</html>", None).expect_err("refused");

    assert!(!refusal.is_empty());
    assert_eq!(
        cache.read().expect("still there").index.plugins[0].id,
        "eugen/probe"
    );
}

#[test]
fn an_index_that_will_not_parse_is_read_as_nothing_at_all() {
    // It was written by a fetch that has already happened, so there is nobody
    // left to tell — and answering with it half-read would be worse.
    let scratch = Scratch::new("store-corrupt");
    let cache = Cache::at(scratch.path());
    cache.write(INDEX, None).expect("it should keep");
    fs::write(scratch.path().join(INDEX_FILE), b"{").expect("the file writes");

    assert!(cache.read().is_none());
}

#[test]
fn a_registry_that_stops_sending_a_tag_does_not_keep_the_old_one() {
    // The next fetch would ask about a version of the file nobody has any
    // more, and be answered `304` for it — a store frozen on a stale list.
    let scratch = Scratch::new("store-tag");
    let cache = Cache::at(scratch.path());
    cache.write(INDEX, Some("\"abc\"")).expect("it should keep");

    cache.write(INDEX, None).expect("it should keep");

    assert_eq!(cache.read().expect("read back").etag, None);
}

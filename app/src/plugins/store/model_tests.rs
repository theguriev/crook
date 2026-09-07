//! What the store knows before it has asked anybody anything.
//!
//! The two things this model *does* — reading an index over the network and
//! downloading a module — are not tested here and are not testable here: they
//! are a request to a host that is not this machine's. What is tested is
//! everything either of them lands in, which is where a mistake would be
//! silent: a list read off the disk, a download taken exactly once, and what
//! the page is told.

use super::*;
use crate::plugins::wasm::tests::Scratch;

const INDEX: &[u8] = br#"{"schema": 1, "plugins": [
    {"id": "eugen/probe", "name": "Probe", "description": "d",
     "versions": [{"version": "1.0.0", "abi": 8, "url": "https://x.invalid/p.wasm", "sha256": "aa"}]}]}"#;

#[test]
fn a_store_with_a_cache_knows_what_is_in_it_without_asking_anybody() {
    // The whole of what "offline shows the cache" means: the list is there,
    // on a machine that has never reached the registry in this session.
    let scratch = Scratch::new("store-model");
    let cache = Cache::at(scratch.path());
    cache.write(INDEX, Some("\"abc\"")).expect("it should keep");

    let model = StoreModel::new(Some(cache));

    assert_eq!(model.offers().len(), 1);
    assert_eq!(model.offers()[0].id.as_str(), "eugen/probe");
    assert!(model.fetched().is_some(), "and how old it is");
    assert!(!model.looking());
}

#[test]
fn a_store_with_nothing_cached_offers_nothing_and_is_not_a_failure() {
    let scratch = Scratch::new("store-model-empty");

    let model = StoreModel::new(Some(Cache::at(scratch.path())));

    assert!(model.offers().is_empty());
    assert_eq!(model.fetched(), None);
    assert_eq!(model.problem(), None);
}

#[test]
fn a_machine_with_nowhere_to_keep_a_list_still_has_a_store() {
    // `Cache::user()` answers `None` where there is no data directory, and a
    // store that could not be built there would be a section that is missing
    // rather than one that is empty.
    let model = StoreModel::new(None);

    assert!(model.offers().is_empty());
}

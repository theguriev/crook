//! What arrives, and what is refused before it is installed.

use super::*;

fn release() -> Release {
    Release {
        version: "1.0.0".into(),
        abi: 8,
        url: "https://example.invalid/p.wasm".into(),
        sha256: sha256_hex(b"crook"),
        bytes: 5,
        capabilities: Vec::new(),
        asks: Vec::new(),
        yanked: None,
    }
}

#[test]
fn the_hash_is_the_one_sha256sum_prints() {
    // The registry writes these with `sha256sum`; a hash that is right in a
    // different notation is a store where nothing installs.
    assert_eq!(
        sha256_hex(b""),
        "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
    );
    assert_eq!(
        sha256_hex(b"abc"),
        "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
    );
}

#[test]
fn bytes_that_are_not_what_the_index_named_are_refused() {
    let release = release();

    checked(b"crook".to_vec(), &release).expect("the right bytes pass");
    let refusal = checked(b"crookk".to_vec(), &release).expect_err("the wrong ones do not");

    // The length is the cheaper half of the same question and the failure
    // that means something: a download that ended early.
    assert!(refusal.contains("bytes"), "{refusal}");
}

#[test]
fn a_hash_that_does_not_match_says_both_of_them() {
    let mut release = release();
    release.bytes = 0;
    release.sha256 = sha256_hex(b"something else");

    let refusal = checked(b"crook".to_vec(), &release).expect_err("it should be refused");

    assert!(refusal.contains(&sha256_hex(b"crook")), "{refusal}");
    assert!(refusal.contains(&release.sha256), "{refusal}");
}

#[test]
fn an_index_that_wrote_the_hash_in_capitals_is_not_a_refusal() {
    let mut release = release();
    release.bytes = 0;
    release.sha256 = sha256_hex(b"crook").to_uppercase();

    checked(b"crook".to_vec(), &release).expect("the same hash, shouted");
}

#[test]
fn a_version_with_no_size_in_the_index_is_checked_by_its_hash_alone() {
    // `bytes` is what lets a download be refused before it is finished; an
    // index that omits it is older, not hostile.
    let mut release = release();
    release.bytes = 0;
    release.sha256 = sha256_hex(b"crook");

    checked(b"crook".to_vec(), &release).expect("the hash is enough");
}

#[test]
#[ignore = "reaches the real registry over the network"]
fn the_registry_answers_with_a_list_this_build_can_read() {
    // Run with `cargo test -p crook -- --ignored the_registry`. Not part of
    // the suite for the reason the transcript test is not: a test that fails
    // on an aeroplane is a test people learn to ignore. What it is for is the
    // question no offline test can answer — whether the URL in this file, the
    // shape the registry publishes and the reader here still agree.
    let agent = agent();
    let Fetched::New { bytes, etag } =
        index(&agent, INDEX_URL, None).expect("the registry should answer")
    else {
        panic!("nothing was sent and no tag was sent either");
    };

    let listed = crate::plugins::store::index::parse(&bytes).expect("and it should be an index");
    assert!(!listed.plugins.is_empty(), "with something in it");

    // And the tag it gave back is the tag that makes the next look cost
    // nothing, which is the half of this that is easy to publish wrongly.
    assert!(matches!(
        index(&agent, INDEX_URL, etag.as_deref()),
        Ok(Fetched::Unchanged)
    ));

    // Then one artifact, hash and all: the store's whole promise is that what
    // arrives is what the list named.
    let offered = crate::plugins::store::index::offers(&listed);
    let release = offered
        .iter()
        .find_map(|offer| offer.release.clone())
        .expect("something built for this build");
    let module = module(&agent, &release).expect("the artifact should arrive intact");
    assert_eq!(module.len() as u64, release.bytes);
}

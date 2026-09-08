//! What `--dev-plugin` resolves, and what it refuses.
//!
//! The watching itself is a timer and a `metadata` call, and a test of it
//! would be a test of `std::fs`. What is worth pinning down is the argument:
//! the flag is typed by a person in the middle of writing something, and
//! every way it can be wrong should say which way.

use super::*;
use crate::plugins::wasm::tests::Scratch;

#[test]
fn a_module_is_itself() {
    let scratch = Scratch::new("dev-file");
    let path = scratch.path().join("hello.wasm");
    std::fs::write(&path, b"not really a module").expect("the scratch file writes");

    assert_eq!(Dev::module(&path).expect("a file is a module"), path);
}

#[test]
fn a_directory_is_the_one_thing_cargo_built_in_it() {
    // The path a plugin's README tells somebody to build to, and the one
    // nobody should have to type twice a minute.
    let scratch = Scratch::new("dev-dir");
    let built = scratch.path().join("target/wasm32-unknown-unknown/release");
    std::fs::create_dir_all(&built).expect("the scratch directory");
    std::fs::write(built.join("hello.wasm"), b"module").expect("writes");
    // The things `cargo` leaves beside it, which are not modules.
    std::fs::write(built.join("hello.d"), b"deps").expect("writes");
    std::fs::create_dir_all(built.join("deps")).expect("writes");

    assert_eq!(
        Dev::module(scratch.path()).expect("one module"),
        built.join("hello.wasm")
    );
}

#[test]
fn every_way_the_argument_can_be_wrong_says_which_way() {
    let scratch = Scratch::new("dev-wrong");

    let refusal = Dev::module(&scratch.path().join("nothing-here")).expect_err("no such path");
    assert!(
        refusal.contains("neither a module nor a directory"),
        "{refusal}"
    );

    let refusal = Dev::module(scratch.path()).expect_err("nothing built");
    assert!(refusal.contains("nothing built in it"), "{refusal}");

    let built = scratch.path().join("target/wasm32-unknown-unknown/release");
    std::fs::create_dir_all(&built).expect("the scratch directory");
    let refusal = Dev::module(scratch.path()).expect_err("an empty build directory");
    assert!(refusal.contains("no module in"), "{refusal}");

    std::fs::write(built.join("one.wasm"), b"module").expect("writes");
    std::fs::write(built.join("two.wasm"), b"module").expect("writes");
    let refusal = Dev::module(scratch.path()).expect_err("two modules");
    assert!(refusal.contains("name the one you mean"), "{refusal}");
}

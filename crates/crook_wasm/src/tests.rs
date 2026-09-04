//! What the sandbox promises, proven against real WebAssembly.
//!
//! The modules below are written in wasm's text format and assembled here, so
//! these run under `cargo test` with no wasm toolchain installed — and, more
//! to the point, they are *real* modules rather than a mock of one. Every rule
//! this file asserts is a rule about what happens when a stranger's bytes do
//! something the host did not expect, and a mock cannot do that.

use super::*;
use crook_plugin_api::{ABI_VERSION, Capability, Manifest, Node, Size, Tone, to_bytes};

/// Where a module's constant data starts. Below it is the bump allocator's
/// pointer, which nothing here reads.
const DATA: u32 = 16;

/// A manifest worth reading back.
fn manifest() -> Manifest {
    Manifest {
        abi: ABI_VERSION,
        id: "eugen/ci-status".into(),
        name: "CI status".into(),
        description: "Whether the branch in the active pane is green.".into(),
        version: "0.2.0".into(),
        capabilities: vec![Capability::Network(vec!["api.github.com".into()])],
    }
}

/// A tree worth drawing.
fn tree() -> Node {
    Node::Row(vec![
        Node::Icon {
            name: "git-branch".into(),
            tone: Tone::Muted,
        },
        Node::Text {
            text: "green".into(),
            size: Size::Small,
            tone: Tone::Success,
        },
    ])
}

/// `bytes` as a wasm data-segment string.
fn escaped(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("\\{byte:02x}")).collect()
}

/// The parts every module below shares: a memory, a bump allocator, and the
/// two answers it hands back.
///
/// `body` is spliced in for the exports each test wants to be different, so
/// the difference between two tests is the thing being tested rather than
/// forty lines of wasm.
fn module(body: &str, abi: u32) -> Vec<u8> {
    let manifest = to_bytes(&manifest()).expect("a manifest should encode");
    let tree = to_bytes(&tree()).expect("a tree should encode");
    let tree_at = DATA + manifest.len() as u32;
    let free = tree_at + tree.len() as u32;

    let text = format!(
        r#"(module
            (import "crook" "contribute"
              (func $contribute (param i32 i32 i32 i32 i32)))
            (import "crook" "register_action"
              (func $register_action (param i32 i32 i32 i32)))
            (import "crook" "log" (func $log (param i32 i32 i32)))
            (memory (export "memory") 1)
            (global $next (mut i32) (i32.const {free}))
            (data (i32.const {DATA}) "{manifest_bytes}")
            (data (i32.const {tree_at}) "{tree_bytes}")
            (global $manifest_at i32 (i32.const {DATA}))
            (global $manifest_len i32 (i32.const {manifest_len}))
            (global $tree_at i32 (i32.const {tree_at}))
            (global $tree_len i32 (i32.const {tree_len}))
            (func (export "crook_abi_version") (result i32) (i32.const {abi}))
            (func (export "crook_alloc") (param $len i32) (result i32)
              (local $at i32)
              (local.set $at (global.get $next))
              (global.set $next (i32.add (global.get $next) (local.get $len)))
              (local.get $at))
            (func (export "crook_manifest") (result i64)
              (i64.or
                (i64.shl (i64.extend_i32_u (global.get $manifest_at)) (i64.const 32))
                (i64.extend_i32_u (global.get $manifest_len))))
            {body})"#,
        manifest_bytes = escaped(&manifest),
        manifest_len = manifest.len(),
        tree_bytes = escaped(&tree),
        tree_len = tree.len(),
    );

    wat::parse_str(&text).expect("the test module should assemble")
}

/// The exports a well-behaved plugin has, minus the manifest the helper
/// already writes.
const WELL_BEHAVED: &str = r#"
    (func (export "crook_build") (result i32)
      (call $contribute
        (global.get $slot_at) (global.get $slot_len)
        (global.get $entry_at) (global.get $entry_len)
        (i32.const 7))
      (call $register_action
        (global.get $action_at) (global.get $action_len)
        (global.get $title_at) (global.get $title_len))
      (i32.const 0))
    (func (export "crook_render") (param i32 i32) (result i64)
      (i64.or
        (i64.shl (i64.extend_i32_u (global.get $tree_at)) (i64.const 32))
        (i64.extend_i32_u (global.get $tree_len))))
    (func (export "crook_run") (param $name i32) (param $len i32) (result i32)
      ;; Answers with the first byte of the name it was handed, which is how
      ;; these tests see that the host wrote the string where the guest's own
      ;; allocator said to put it.
      (i32.load8_u (local.get $name)))
"#;

/// The strings a well-behaved module registers, laid out after its data.
///
/// Written as a second data segment rather than as literals in the wasm,
/// because a string in wasm *is* a data segment and its address has to be
/// known here to be named in the calls above.
fn with_strings(body: &str) -> String {
    let strings = [
        ("slot", "header.right"),
        ("entry", "chip"),
        ("action", "refresh"),
        ("title", "Refresh CI status"),
    ];
    // Well past anything the helper wrote, so the two never overlap.
    let mut at = 4096;
    let mut header = String::new();
    for (name, text) in strings {
        header.push_str(&format!(
            "(data (i32.const {at}) \"{text}\")\n\
             (global ${name}_at i32 (i32.const {at}))\n\
             (global ${name}_len i32 (i32.const {len}))\n",
            len = text.len()
        ));
        at += text.len() as u32;
    }
    format!("{header}{body}")
}

fn well_behaved() -> Vec<u8> {
    module(&with_strings(WELL_BEHAVED), ABI_VERSION)
}

fn open(wasm: &[u8]) -> Result<(Sandbox, Manifest), Problem> {
    Sandbox::open(wasm, Fuel::default())
}

/// The same, on a budget small enough to be spent in a test.
fn open_with(wasm: &[u8], fuel: Fuel) -> Result<(Sandbox, Manifest), Problem> {
    Sandbox::open(wasm, fuel)
}

/// The refusal, for a test whose whole subject is being refused.
///
/// `expect_err` would need [`Sandbox`] to be `Debug`, and a running wasm store
/// is not a thing worth printing.
fn refused(wasm: &[u8]) -> Problem {
    match open(wasm) {
        Ok(_) => panic!("it should have been refused"),
        Err(problem) => problem,
    }
}

#[test]
fn a_plugin_says_what_it_is_before_any_of_it_runs() {
    // The order matters as much as the answer: a store lists a plugin, a
    // person reads what it wants, and a host refuses one asking for too much —
    // none of which may require running it.
    let (_, read_back) = open(&well_behaved()).expect("it should open");

    assert_eq!(read_back, manifest());
    assert_eq!(read_back.capabilities[0].sentence(), "Reach api.github.com");
}

#[test]
fn a_plugin_built_for_another_version_is_refused_by_both_numbers() {
    // The one refusal a person can act on without knowing anything about
    // wasm: it says which version the plugin wants and which this is.
    let wasm = module(&with_strings(WELL_BEHAVED), ABI_VERSION + 1);

    let problem = refused(&wasm);

    assert_eq!(
        problem,
        Problem::Abi {
            theirs: ABI_VERSION + 1,
            ours: ABI_VERSION
        }
    );
    assert!(problem.to_string().contains(&ABI_VERSION.to_string()));
}

#[test]
fn something_that_is_not_wasm_at_all_is_refused_rather_than_run() {
    let problem = refused(b"this is not a plugin");

    assert!(matches!(problem, Problem::Shape(_)), "{problem:?}");
}

#[test]
fn a_module_that_imports_something_that_is_not_there_does_not_instantiate() {
    // The whole of the sandbox's reach, checked at instantiation rather than
    // at the call: a plugin that wants a filesystem finds out now, and a
    // person finds out with it.
    let wasm = wat::parse_str(
        r#"(module
            (import "wasi_snapshot_preview1" "fd_write"
              (func (param i32 i32 i32 i32) (result i32)))
            (memory (export "memory") 1))"#,
    )
    .expect("it should assemble");

    let problem = refused(&wasm);

    assert!(matches!(problem, Problem::Shape(_)), "{problem:?}");
    assert!(
        problem.to_string().contains("fd_write"),
        "the refusal does not say what it asked for: {problem}"
    );
}

#[test]
fn a_module_with_no_memory_is_refused() {
    let wasm = wat::parse_str(
        r#"(module (func (export "crook_abi_version") (result i32) (i32.const 1)))"#,
    )
    .expect("it should assemble");

    let problem = refused(&wasm);

    assert!(matches!(problem, Problem::Shape(_)), "{problem:?}");
    assert!(problem.to_string().contains("memory"), "{problem}");
}

#[test]
fn what_a_plugin_registers_while_it_builds_comes_back() {
    let (mut sandbox, _) = open(&well_behaved()).expect("it should open");

    let registered = sandbox.build().expect("it should build");

    assert_eq!(registered.contributions.len(), 1);
    assert_eq!(registered.contributions[0].slot, "header.right");
    assert_eq!(registered.contributions[0].entry, "chip");
    assert_eq!(registered.contributions[0].order, 7);
    assert_eq!(registered.actions.len(), 1);
    assert_eq!(registered.actions[0].name, "refresh");
    assert_eq!(
        registered.actions[0].title.as_deref(),
        Some("Refresh CI status")
    );
}

#[test]
fn a_build_that_gives_up_registers_nothing() {
    // Half a contribution on a slot is worse than none: it is a feature that
    // is on screen and does not work.
    let body = with_strings(
        r#"
        (func (export "crook_build") (result i32)
          (call $contribute
            (global.get $slot_at) (global.get $slot_len)
            (global.get $entry_at) (global.get $entry_len)
            (i32.const 0))
          (i32.const 1))
        (func (export "crook_render") (param i32 i32) (result i64) (i64.const 0))
        (func (export "crook_run") (param i32 i32) (result i32) (i32.const 0))
        "#,
    );
    let (mut sandbox, _) = open(&module(&body, ABI_VERSION)).expect("it should open");

    let problem = sandbox.build().expect_err("it should not build");

    assert!(matches!(problem, Problem::Ran(_)), "{problem:?}");
    // Asked again, because what matters is not the error but what is left
    // behind: nothing.
    assert_eq!(
        sandbox.build().unwrap_err(),
        problem,
        "the second attempt behaved differently from the first"
    );
}

#[test]
fn a_build_that_traps_registers_nothing_either() {
    let body = with_strings(
        r#"
        (func (export "crook_build") (result i32)
          (call $contribute
            (global.get $slot_at) (global.get $slot_len)
            (global.get $entry_at) (global.get $entry_len)
            (i32.const 0))
          unreachable)
        (func (export "crook_render") (param i32 i32) (result i64) (i64.const 0))
        (func (export "crook_run") (param i32 i32) (result i32) (i32.const 0))
        "#,
    );
    let (mut sandbox, _) = open(&module(&body, ABI_VERSION)).expect("it should open");

    let problem = sandbox.build().expect_err("it should not build");

    assert!(matches!(problem, Problem::Ran(_)), "{problem:?}");
}

#[test]
fn a_plugin_that_never_stops_runs_out_of_fuel() {
    // The rule the whole file exists for: **nothing a plugin does may cost a
    // person their window**. A loop is the easiest way to try.
    let body = with_strings(
        r#"
        (func (export "crook_build") (result i32) (loop br 0) (i32.const 0))
        (func (export "crook_render") (param i32 i32) (result i64) (i64.const 0))
        (func (export "crook_run") (param i32 i32) (result i32) (i32.const 0))
        "#,
    );
    // A small budget on purpose. What is being tested is that a loop is
    // *stopped*, and a budget the size of the real one would spend a second of
    // the test suite proving it a second time.
    let fuel = Fuel {
        build: 10_000,
        ..Fuel::default()
    };
    let (mut sandbox, _) = open_with(&module(&body, ABI_VERSION), fuel).expect("it should open");

    let problem = sandbox.build().expect_err("it should not build");

    assert!(matches!(problem, Problem::Ran(_)), "{problem:?}");
    assert!(
        problem.to_string().contains("fuel"),
        "the refusal does not say why: {problem}"
    );
}

#[test]
fn a_render_comes_back_as_a_tree() {
    let (mut sandbox, _) = open(&well_behaved()).expect("it should open");
    sandbox.build().expect("it should build");

    assert_eq!(
        sandbox.render("header.right").expect("it should render"),
        tree()
    );
}

#[test]
fn the_host_writes_a_string_where_the_guest_asked_for_it() {
    // The other direction, and the one a host gets wrong: the guest's memory
    // is the guest's, so a string goes in through *its* allocator. This module
    // answers with the first byte of the name it was handed, which is only the
    // right byte if it landed where the allocator said.
    let (mut sandbox, _) = open(&well_behaved()).expect("it should open");
    sandbox.build().expect("it should build");

    let problem = sandbox.run("Ping").expect_err("this module always answers");

    assert_eq!(
        problem,
        Problem::Ran(format!("the action answered {}", u32::from(b'P')))
    );
}

#[test]
fn an_answer_that_points_outside_its_own_memory_is_refused() {
    // A hostile answer, which is the only kind worth testing: a pointer past
    // the end of the guest's memory must be a refused call rather than a read
    // of whatever the host has there.
    let body = with_strings(
        r#"
        (func (export "crook_build") (result i32) (i32.const 0))
        (func (export "crook_render") (param i32 i32) (result i64)
          (i64.or (i64.shl (i64.const 4294901760) (i64.const 32)) (i64.const 64)))
        (func (export "crook_run") (param i32 i32) (result i32) (i32.const 0))
        "#,
    );
    let (mut sandbox, _) = open(&module(&body, ABI_VERSION)).expect("it should open");

    let problem = sandbox
        .render("header.right")
        .expect_err("it should be refused");

    assert!(matches!(problem, Problem::Answer(_)), "{problem:?}");
}

#[test]
fn an_answer_longer_than_any_answer_could_be_is_refused_before_it_is_allocated() {
    // The bound is on what one bad number can make the host *allocate*, which
    // is why it is checked before the read rather than after.
    let body = with_strings(
        r#"
        (func (export "crook_build") (result i32) (i32.const 0))
        (func (export "crook_render") (param i32 i32) (result i64)
          (i64.or (i64.shl (i64.const 16) (i64.const 32)) (i64.const 4294967295)))
        (func (export "crook_run") (param i32 i32) (result i32) (i32.const 0))
        "#,
    );
    let (mut sandbox, _) = open(&module(&body, ABI_VERSION)).expect("it should open");

    let problem = sandbox
        .render("header.right")
        .expect_err("it should be refused");

    assert!(matches!(problem, Problem::Answer(_)), "{problem:?}");
    assert!(problem.to_string().contains("limit"), "{problem}");
}

#[test]
fn an_answer_that_is_not_what_it_should_be_is_refused() {
    // Bytes inside the guest's memory that are not a tree. A decode that fails
    // is one contribution that draws nothing, not a panic.
    let body = with_strings(
        r#"
        (func (export "crook_build") (result i32) (i32.const 0))
        (func (export "crook_render") (param i32 i32) (result i64)
          ;; The manifest, offered where a tree was asked for.
          (i64.or
            (i64.shl (i64.extend_i32_u (global.get $manifest_at)) (i64.const 32))
            (i64.extend_i32_u (global.get $manifest_len))))
        (func (export "crook_run") (param i32 i32) (result i32) (i32.const 0))
        "#,
    );
    let (mut sandbox, _) = open(&module(&body, ABI_VERSION)).expect("it should open");

    let problem = sandbox
        .render("header.right")
        .expect_err("it should be refused");

    assert!(matches!(problem, Problem::Answer(_)), "{problem:?}");
}

#[test]
fn each_call_gets_its_own_budget() {
    // A budget spent over a session would be a plugin that stops working after
    // an hour of doing nothing wrong. Rendering a hundred times is a hundred
    // budgets, not one divided.
    let (mut sandbox, _) = open(&well_behaved()).expect("it should open");
    sandbox.build().expect("it should build");

    for _ in 0..100 {
        assert_eq!(
            sandbox.render("header.right").expect("it should render"),
            tree()
        );
    }
}

#[test]
fn a_plugin_may_do_real_work_without_taking_the_host_stack_with_it() {
    // Three million iterations, which is more than any contribution should
    // ever need and exactly the amount that finds the problem.
    //
    // `wasmi` has two dispatch backends. Its default picks the one that
    // dispatches by tail call, which is faster and *relies on the optimiser to
    // turn the recursion into a jump* — so an unoptimised build of it grows
    // the host stack per instruction and overflows at around ten thousand
    // iterations. `portable-dispatch`, which the workspace turns on, is the
    // one that dispatches in a loop and cannot grow the stack at all.
    //
    // This is the test that says so. Take the feature out and this does not
    // fail — it aborts the whole test binary, which is exactly what it would
    // do to somebody's window.
    let body = with_strings(
        r#"
        (func (export "crook_build") (result i32)
          (local $i i32)
          (loop $again
            (local.set $i (i32.add (local.get $i) (i32.const 1)))
            (br_if $again (i32.lt_s (local.get $i) (i32.const 3000000))))
          (i32.const 0))
        (func (export "crook_render") (param i32 i32) (result i64) (i64.const 0))
        (func (export "crook_run") (param i32 i32) (result i32) (i32.const 0))
        "#,
    );
    let (mut sandbox, _) = open(&module(&body, ABI_VERSION)).expect("it should open");

    sandbox.build().expect("it should get all the way through");
}

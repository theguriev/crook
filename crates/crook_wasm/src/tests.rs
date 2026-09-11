//! What the sandbox promises, proven against real WebAssembly.
//!
//! The modules below are written in wasm's text format and assembled here, so
//! these run under `cargo test` with no wasm toolchain installed — and, more
//! to the point, they are *real* modules rather than a mock of one. Every rule
//! this file asserts is a rule about what happens when a stranger's bytes do
//! something the host did not expect, and a mock cannot do that.

use std::time::Duration;

use super::*;
use crook_plugin_api::pictures::{
    CAPTION_SECTION_PREFIX, ICON_SECTION, MAX_CAPTION_CHARS, MAX_ICON_BYTES, MAX_PREVIEW_BYTES,
    PREVIEW_SECTION_PREFIX,
};
use crook_plugin_api::{
    ABI_VERSION, Answer, Capability, Manifest, Method, Node, Render, Request, Size, Subject,
    TabFacts, Tone, to_bytes,
};

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

/// What the host asks for when the slot is drawn once and is about nothing.
fn for_slot(slot: &str) -> Render {
    Render {
        slot: slot.into(),
        entry: "chip".into(),
        subject: None,
    }
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
    module_saying(&manifest(), body, abi)
}

/// The same, for the one test whose subject is a module carrying a manifest
/// that is not the one its exports claim.
fn module_saying(manifest: &Manifest, body: &str, abi: u32) -> Vec<u8> {
    let manifest = to_bytes(manifest).expect("a manifest should encode");
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
            (import "crook" "request" (func $request (param i32 i32) (result i32)))
            (import "crook" "timer" (func $timer (param i32) (result i32)))
            (import "crook" "now" (func $now (result i64)))
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
    (func (export "crook_run") (param $name i32) (param $len i32) (param $arg i32) (param $arg_len i32) (result i32)
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
fn a_module_asking_for_more_memory_than_the_ceiling_never_gets_it() {
    // Refused *before* the allocation. The ceiling used to be checked after
    // `instantiate_and_start`, which is after wasmi has asked the operating
    // system for whatever minimum the module declared and zeroed it — so two
    // kilobytes of wasm saying `(memory 65536)` was four gigabytes the host
    // went and got before anything looked at it, and no amount of fuel catches
    // an allocation.
    let wasm = wat::parse_str("(module (memory (export \"memory\") 20000))")
        .expect("the test module should assemble");

    let problem = refused(&wasm);

    assert!(
        matches!(problem, Problem::Shape(_)),
        "{problem:?} should be a refusal to run at all"
    );
}

#[test]
fn a_plugin_that_grows_past_the_ceiling_is_told_no_rather_than_given_it() {
    // The other end of the same number, and the one that is not caught by any
    // check after the fact: a module declaring one page and growing to a
    // thousand is inside every limit at instantiation. `memory.grow` answers
    // `-1`, which is a failure the guest's own allocator deals with.
    let body = r#"
        (func (export "crook_build") (result i32)
          (if (result i32)
            (i32.eq (memory.grow (i32.const 1000)) (i32.const -1))
            (then (i32.const 0))
            (else (i32.const 1))))
        (func (export "crook_render") (param i32 i32) (result i64) (i64.const 0))
        (func (export "crook_run") (param i32 i32 i32 i32) (result i32) (i32.const 0))
    "#;
    let (mut sandbox, _) = open(&module(&with_strings(body), ABI_VERSION)).expect("it should open");

    sandbox.build().expect(
        "a build that was refused the memory answers 0, and one that was given it does not",
    );
}

#[test]
fn a_plugin_that_disagrees_with_itself_about_the_version_is_refused() {
    // The number is in a module twice: the export the host enforces, and the
    // field everything that *describes* the plugin reads — a Plugins page, and
    // an index built by a registry. A module carrying two of them would be
    // enforced by one and advertised by the other.
    let mut manifest = manifest();
    manifest.abi = ABI_VERSION + 1;

    let problem = refused(&module_saying(
        &manifest,
        &with_strings(WELL_BEHAVED),
        ABI_VERSION,
    ));

    assert_eq!(
        problem,
        Problem::Answer(format!(
            "it exports plugin API {ABI_VERSION} and its manifest says {}",
            ABI_VERSION + 1
        ))
    );
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
        (func (export "crook_run") (param i32 i32 i32 i32) (result i32) (i32.const 0))
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
        (func (export "crook_run") (param i32 i32 i32 i32) (result i32) (i32.const 0))
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
        (func (export "crook_run") (param i32 i32 i32 i32) (result i32) (i32.const 0))
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
        sandbox
            .render(&for_slot("header.right"))
            .expect("it should render"),
        tree()
    );
}

#[test]
fn a_render_carries_what_it_is_about_and_not_only_where_it_goes() {
    // The whole of ABI 3, proven from the guest's side: what arrives at
    // `crook_render` is the encoded request, subject and all, rather than the
    // slot name it used to be. The module answers with a meter whose fraction
    // is the number of bytes it was handed, which is the one thing a module
    // written in wasm text can say about a postcard value without decoding it
    // — and it is enough, because a request carrying a subject is longer than
    // the same request without one.
    let body = with_strings(
        r#"
        (func (export "crook_build") (result i32) (i32.const 0))
        (func (export "crook_render") (param $at i32) (param $len i32) (result i64)
          ;; Node::Meter { fraction, tone }: variant 8, four bytes of f32, and
          ;; the tone.
          (i32.store8 (i32.const 2048) (i32.const 8))
          (f32.store (i32.const 2049) (f32.convert_i32_u (local.get $len)))
          (i32.store8 (i32.const 2053) (i32.const 0))
          (i64.or (i64.shl (i64.const 2048) (i64.const 32)) (i64.const 6)))
        (func (export "crook_run") (param i32 i32 i32 i32) (result i32) (i32.const 0))
        "#,
    );
    let (mut sandbox, _) = open(&module(&body, ABI_VERSION)).expect("it should open");
    let bare = for_slot("tab.row.mark");
    let about_a_row = Render {
        entry: "mark".into(),
        slot: "tab.row.mark".into(),
        subject: Some(Subject::Tab(TabFacts {
            key: 0x0123_4567_89ab_cdef,
            tab: None,
            place: None,
        })),
    };

    let (
        Ok(Node::Meter {
            fraction: bare_len, ..
        }),
        Ok(Node::Meter {
            fraction: with_len, ..
        }),
    ) = (sandbox.render(&bare), sandbox.render(&about_a_row))
    else {
        panic!("both renders should come back as a meter");
    };

    assert_eq!(
        bare_len as usize,
        to_bytes(&bare).expect("a request should encode").len(),
        "the guest was handed something other than the whole request"
    );
    assert!(
        with_len > bare_len,
        "the subject did not cross: {with_len} against {bare_len}"
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

    let problem = sandbox
        .run("Ping", "")
        .expect_err("this module always answers");

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
        (func (export "crook_run") (param i32 i32 i32 i32) (result i32) (i32.const 0))
        "#,
    );
    let (mut sandbox, _) = open(&module(&body, ABI_VERSION)).expect("it should open");

    let problem = sandbox
        .render(&for_slot("header.right"))
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
        (func (export "crook_run") (param i32 i32 i32 i32) (result i32) (i32.const 0))
        "#,
    );
    let (mut sandbox, _) = open(&module(&body, ABI_VERSION)).expect("it should open");

    let problem = sandbox
        .render(&for_slot("header.right"))
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
        (func (export "crook_run") (param i32 i32 i32 i32) (result i32) (i32.const 0))
        "#,
    );
    let (mut sandbox, _) = open(&module(&body, ABI_VERSION)).expect("it should open");

    let problem = sandbox
        .render(&for_slot("header.right"))
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
            sandbox
                .render(&for_slot("header.right"))
                .expect("it should render"),
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
        (func (export "crook_run") (param i32 i32 i32 i32) (result i32) (i32.const 0))
        "#,
    );
    let (mut sandbox, _) = open(&module(&body, ABI_VERSION)).expect("it should open");

    sandbox.build().expect("it should get all the way through");
}

/// What a plugin that asks for things is made of.
///
/// Every one of its answers comes back out through the same two doors the
/// host already watches — a request and a timer — rather than through a
/// channel invented for the test, so what these assert is the mechanism
/// itself and not a mock of it.
const ASKING: &str = r#"
    (global $last (mut i32) (i32.const 0))
    (func (export "crook_build") (result i32)
      (global.set $last (call $request (global.get $ask_at) (global.get $ask_len)))
      (drop (call $timer (i32.const 60000)))
      (i32.const 0))
    (func (export "crook_render") (param i32 i32) (result i64)
      (i64.or
        (i64.shl (i64.extend_i32_u (global.get $tree_at)) (i64.const 32))
        (i64.extend_i32_u (global.get $tree_len))))
    (func (export "crook_run") (param i32 i32 i32 i32) (result i32) (i32.const 0))
    (func (export "crook_deliver") (param $ticket i32) (param $at i32) (param $len i32)
      (result i32)
      ;; The ticket it was given and the first byte of what it was given, in
      ;; one number: which answer, and that the bytes really are in its own
      ;; memory where the host said it put them.
      (drop (call $timer
        (i32.add
          (i32.mul (local.get $ticket) (i32.const 1000))
          (i32.load8_u (local.get $at)))))
      (i32.const 0))
    (func (export "crook_tick") (result i32)
      (global.set $last (call $request (global.get $ask_at) (global.get $ask_len)))
      (i32.const 0))
"#;

/// A plugin that asks for more than any plugin has reason to.
const GREEDY: &str = r#"
    (func (export "crook_build") (result i32)
      (local $count i32)
      (block $done
        (loop $again
          (br_if $done (i32.ge_u (local.get $count) (i32.const 40)))
          (drop (call $request (global.get $ask_at) (global.get $ask_len)))
          (local.set $count (i32.add (local.get $count) (i32.const 1)))
          (br $again)))
      (i32.const 0))
    (func (export "crook_render") (param i32 i32) (result i64) (i64.const 0))
"#;

/// A plugin that hands the request import something that is not a request.
const BABBLING: &str = r#"
    (func (export "crook_build") (result i32)
      (drop (call $request (global.get $slot_at) (global.get $slot_len)))
      (i32.const 0))
    (func (export "crook_render") (param i32 i32) (result i64) (i64.const 0))
"#;

/// A plugin that only wants to know what time it is.
const CLOCK_WATCHING: &str = r#"
    (func (export "crook_build") (result i32)
      ;; Milliseconds are too big for the timer's i32, so this is the epoch in
      ;; units of a million seconds — about 1772 while this was written, and a
      ;; number a test can bracket without pinning a date.
      (drop (call $timer
        (i32.wrap_i64 (i64.div_u (call $now) (i64.const 1000000000)))))
      (i32.const 0))
    (func (export "crook_render") (param i32 i32) (result i64) (i64.const 0))
"#;

/// The request every asking module holds, ready encoded.
fn asked_for() -> Request {
    Request::ReadFile {
        path: "~/.claude/.credentials.json".into(),
    }
}

/// `body`, with the strings a build registers and the bytes of a request it
/// raises laid out where the wasm can name them.
fn with_request(body: &str) -> String {
    let request = to_bytes(&asked_for()).expect("a request should encode");
    // After the strings `with_strings` writes, which start at 4096.
    let at = 8192;
    format!(
        "(data (i32.const {at}) \"{bytes}\")\n\
         (global $ask_at i32 (i32.const {at}))\n\
         (global $ask_len i32 (i32.const {len}))\n{body}",
        bytes = escaped(&request),
        len = request.len(),
    )
}

fn asking(body: &str) -> Vec<u8> {
    module(&with_strings(&with_request(body)), ABI_VERSION)
}

#[test]
fn what_a_plugin_asks_for_is_taken_from_it_once() {
    let (mut sandbox, _) = open(&asking(ASKING)).expect("it should open");

    sandbox.build().expect("it should build");

    // The request, with the ticket its answer will carry — and the first
    // ticket is 1, so that zero can mean "not asked" to the guest.
    assert_eq!(sandbox.asked(), vec![(1, asked_for())]);
    // Taken, not read: handing it over twice would be the network call made
    // twice, and the second one would be the host's fault.
    assert!(sandbox.asked().is_empty());
}

#[test]
fn a_timer_a_plugin_asked_for_is_taken_once_too() {
    let (mut sandbox, _) = open(&asking(ASKING)).expect("it should open");

    sandbox.build().expect("it should build");

    assert_eq!(sandbox.timer(), Some(Duration::from_millis(60_000)));
    assert_eq!(sandbox.timer(), None);
}

#[test]
fn an_answer_reaches_the_guest_with_its_ticket_and_its_bytes() {
    let (mut sandbox, _) = open(&asking(ASKING)).expect("it should open");
    sandbox.build().expect("it should build");
    let ticket = sandbox.asked()[0].0;
    let _ = sandbox.timer();

    sandbox
        .deliver(
            ticket,
            &Answer::Read {
                bytes: vec![9, 9, 9],
            },
        )
        .expect("it should take an answer");

    // The guest reported the ticket it was handed and the first byte of what
    // it was handed: postcard writes a variant index first, and `Read` is the
    // second variant of `Answer`.
    let reported = sandbox.timer().expect("the guest should have reported");
    assert_eq!(
        reported,
        Duration::from_millis(u64::from(ticket) * 1000 + 1)
    );
}

#[test]
fn a_plugin_that_was_ticked_can_ask_for_something_else() {
    // The whole of a poll loop, in one assertion: something asked for, a
    // timer, the timer going off, and the next thing asked for.
    let (mut sandbox, _) = open(&asking(ASKING)).expect("it should open");
    sandbox.build().expect("it should build");
    assert_eq!(sandbox.asked().len(), 1);

    sandbox.tick().expect("it should take a tick");

    assert_eq!(sandbox.asked(), vec![(2, asked_for())]);
}

#[test]
fn a_module_that_asks_for_nothing_is_never_delivered_to() {
    // Most plugins. A missing `crook_deliver` is a plugin that had no
    // questions, not a plugin that is broken.
    let (sandbox, _) = open(&well_behaved()).expect("it should open");

    assert!(!sandbox.takes_answers());
    assert!(!sandbox.takes_ticks());
}

#[test]
fn a_module_that_does_ask_says_where_to_put_the_answer() {
    let (sandbox, _) = open(&asking(ASKING)).expect("it should open");

    assert!(sandbox.takes_answers());
    assert!(sandbox.takes_ticks());
}

#[test]
fn a_plugin_may_only_have_so_many_questions_outstanding() {
    // Fuel bounds how long a call runs; this bounds what one can leave
    // behind. Forty asked for, thirty-two kept, and the guest was told with a
    // zero rather than with a trap.
    let (mut sandbox, _) = open(&asking(GREEDY)).expect("it should open");

    sandbox.build().expect("it should build");

    assert_eq!(sandbox.asked().len(), 32);
}

#[test]
fn a_request_that_is_not_one_is_not_asked_for() {
    // `header.right` is a slot name, not an encoded request. A plugin that
    // hands the import nonsense has a bug, and its bug must not be the
    // window's: nothing is asked for and the frame goes on.
    let (mut sandbox, _) = open(&asking(BABBLING)).expect("it should open");

    sandbox.build().expect("it should build");

    assert!(sandbox.asked().is_empty());
}

#[test]
fn a_plugin_can_be_told_what_time_it_is() {
    let (mut sandbox, _) = open(&asking(CLOCK_WATCHING)).expect("it should open");

    sandbox.build().expect("it should build");

    let epoch = sandbox.timer().expect("it should have asked").as_millis();
    // Anything between 2020 and 2065. Not a date this test pins: a machine
    // whose clock says 1970 is a machine where a countdown would be nonsense,
    // and that is the thing worth failing on.
    assert!((1_600..2_999).contains(&epoch), "the clock said {epoch}");
}

#[test]
fn asking_for_something_while_building_badly_leaves_nothing_behind() {
    // A build that gave up has not established what the plugin is, so doing
    // the network call it asked for on the way would be doing something on
    // behalf of a plugin that does not exist.
    let body = ASKING.replace(
        r#"(func (export "crook_build") (result i32)
      (global.set $last (call $request (global.get $ask_at) (global.get $ask_len)))
      (drop (call $timer (i32.const 60000)))
      (i32.const 0))"#,
        r#"(func (export "crook_build") (result i32)
      (global.set $last (call $request (global.get $ask_at) (global.get $ask_len)))
      (drop (call $timer (i32.const 60000)))
      (i32.const 3))"#,
    );
    let (mut sandbox, _) = open(&asking(&body)).expect("it should open");

    assert!(sandbox.build().is_err());

    assert!(sandbox.asked().is_empty());
    assert_eq!(sandbox.timer(), None);
}

#[test]
fn a_fetch_survives_being_asked_for() {
    // The shape the usage chip actually raises, through the real import.
    let fetch = Request::Fetch {
        method: Method::Post,
        url: "https://api.anthropic.com/api/oauth/usage".into(),
        headers: vec![("authorization".into(), "Bearer token".into())],
        body: None,
    };
    let bytes = to_bytes(&fetch).expect("it should encode");
    let at = 8192;
    let body = format!(
        "(data (i32.const {at}) \"{escaped}\")\n\
         (global $fetch_at i32 (i32.const {at}))\n\
         (global $fetch_len i32 (i32.const {len}))\n\
         (func (export \"crook_build\") (result i32)\n\
           (drop (call $request (global.get $fetch_at) (global.get $fetch_len)))\n\
           (i32.const 0))\n\
         (func (export \"crook_render\") (param i32 i32) (result i64) (i64.const 0))",
        escaped = escaped(&bytes),
        len = bytes.len(),
    );
    let (mut sandbox, _) =
        open(&module(&with_strings(&body), ABI_VERSION)).expect("it should open");

    sandbox.build().expect("it should build");

    assert_eq!(sandbox.asked(), vec![(1, fetch)]);
}

#[test]
fn the_crate_version_names_the_abi_it_reads() {
    // `cargo install crook_wasm --version 0.8` has to be the reader for ABI 8,
    // because that is the sentence the README and the registry's CI both
    // depend on. Two numbers that must move together are a rule nobody
    // remembers; `crook_plugin_api` holds the same one the same way.
    let minor = env!("CARGO_PKG_VERSION")
        .split('.')
        .nth(1)
        .and_then(|minor| minor.parse::<u32>().ok())
        .expect("the crate version is major.minor.patch");

    assert_eq!(env!("CARGO_PKG_VERSION_MAJOR"), "0");
    assert_eq!(
        minor, ABI_VERSION,
        "the sandbox is at 0.{minor} and reads ABI {ABI_VERSION}: bump the version in \
         crates/crook_wasm/Cargo.toml with the constant"
    );
}

/// A PNG that is all header: the signature, an IHDR saying `width`×`height`,
/// whatever `chunks` are asked for, and an IEND.
///
/// No pixels and no checksums, because nothing in this crate decodes a PNG
/// or checks a CRC — the rule is about a size read off a header and a chunk
/// name, and a picture that is nothing but those is the picture that proves
/// only those are read.
fn png_with(width: u32, height: u32, chunks: &[(&[u8; 4], &[u8])]) -> Vec<u8> {
    let mut png = b"\x89PNG\r\n\x1a\n".to_vec();
    let mut ihdr = Vec::new();
    ihdr.extend_from_slice(&width.to_be_bytes());
    ihdr.extend_from_slice(&height.to_be_bytes());
    // Bit depth 8, colour type 6 (RGBA), the three methods at zero.
    ihdr.extend_from_slice(&[8, 6, 0, 0, 0]);
    for (name, data) in std::iter::once((b"IHDR", ihdr.as_slice())).chain(chunks.iter().copied()) {
        png.extend_from_slice(&(data.len() as u32).to_be_bytes());
        png.extend_from_slice(name);
        png.extend_from_slice(data);
        png.extend_from_slice(&[0; 4]);
    }
    png.extend_from_slice(&[0, 0, 0, 0, b'I', b'E', b'N', b'D', 0, 0, 0, 0]);
    png
}

/// A header-only PNG of `width`×`height`.
fn png(width: u32, height: u32) -> Vec<u8> {
    png_with(width, height, &[])
}

/// The same, padded with a text chunk to at least `bytes`.
fn png_of_at_least(width: u32, height: u32, bytes: usize) -> Vec<u8> {
    png_with(width, height, &[(b"tEXt", &vec![b'x'; bytes])])
}

/// An APNG: an `acTL` chunk between the header and the data.
fn apng(width: u32, height: u32) -> Vec<u8> {
    png_with(
        width,
        height,
        &[(b"acTL", &[0, 0, 0, 2, 0, 0, 0, 0]), (b"IDAT", &[])],
    )
}

/// `sections` as custom sections at the end of `body`.
///
/// `(@custom …)` is the text format's own spelling for one, and the
/// assembler places it after the last standard section — where a linker puts
/// them too.
fn with_sections(body: &str, sections: &[(&str, &[u8])]) -> String {
    let mut text = String::from(body);
    for (name, data) in sections {
        text.push_str(&format!("\n(@custom {name:?} \"{}\")", escaped(data)));
    }
    text
}

/// A well-behaved module carrying `sections`.
fn carrying(sections: &[(&str, &[u8])]) -> Vec<u8> {
    module(
        &with_sections(&with_strings(WELL_BEHAVED), sections),
        ABI_VERSION,
    )
}

/// The sentence a module carrying `sections` is refused with.
fn refused_for(sections: &[(&str, &[u8])]) -> String {
    let (_, _, pictures) =
        Sandbox::open_with_pictures(&carrying(sections), Fuel::default()).expect("it should open");
    pictures.expect_err("the picture should have been refused")
}

#[test]
fn a_module_with_no_pictures_carries_none_and_that_is_not_a_problem() {
    // Every plugin built before the rule existed: six modules on the
    // registry today with no custom sections at all, `name` and `producers`
    // included. What they read as is nothing, and nothing is `Ok`.
    let (_, manifest, pictures) =
        Sandbox::open_with_pictures(&well_behaved(), Fuel::default()).expect("it should open");

    assert_eq!(manifest, self::manifest());
    assert_eq!(pictures, Ok(Pictures::default()));
    assert!(
        Pictures::read_bytes(&well_behaved())
            .expect("it should read")
            .is_empty()
    );
}

#[test]
fn the_pictures_a_module_carries_come_back_in_order_with_their_captions() {
    // Previews are numbered, and the number is the order they are shown in
    // whatever order the linker wrote the sections — here the second before
    // the first. A gap is not a gap: one and three are two previews.
    let icon = png(128, 128);
    let second = png(560, 1010);
    let first = png(640, 128);
    let third = png(900, 300);
    let wasm = carrying(&[
        ("crook.preview.3", &third),
        ("crook.caption.1", b"The chip in the header"),
        ("crook.preview.2", &second),
        ("name", b"somebody else's section"),
        ("crook.icon", &icon),
        ("crook.preview.1", &first),
    ]);

    let (_, manifest, pictures) =
        Sandbox::open_with_pictures(&wasm, Fuel::default()).expect("it should open");
    let pictures = pictures.expect("every picture is inside the rule");

    assert_eq!(manifest, self::manifest());
    assert_eq!(pictures.icon.as_deref(), Some(icon.as_slice()));
    assert_eq!(
        pictures.previews,
        vec![
            Preview {
                png: first,
                width: 640,
                height: 128,
                caption: Some("The chip in the header".into()),
            },
            Preview {
                png: second,
                width: 560,
                height: 1010,
                caption: None,
            },
            Preview {
                png: third,
                width: 900,
                height: 300,
                caption: None,
            },
        ]
    );
    assert!(!pictures.is_empty());
    // The read that needs no instantiation says the same.
    assert_eq!(Pictures::read_bytes(&wasm), Ok(pictures));
}

#[test]
fn opening_without_asking_for_pictures_still_answers_with_the_manifest() {
    // The signature every loader already calls, unchanged: a module with
    // pictures in it opens exactly as one without, and the pictures are
    // simply not handed back.
    let (mut sandbox, manifest) =
        open(&carrying(&[("crook.icon", &png(64, 64))])).expect("it should open");

    assert_eq!(manifest, self::manifest());
    sandbox.build().expect("and it still builds");
}

#[test]
fn a_bad_picture_is_a_sentence_and_not_a_refusal_to_open() {
    // The line between the two policies. The module is a good plugin with a
    // bad icon: it opens, its manifest is read, and whether that is a
    // refused build or a plugin drawn without its face is decided above this
    // crate.
    let (mut sandbox, manifest, pictures) =
        Sandbox::open_with_pictures(&carrying(&[("crook.icon", b"not a png")]), Fuel::default())
            .expect("the module itself is fine");

    assert_eq!(manifest, self::manifest());
    assert_eq!(pictures, Err("crook.icon is not a PNG".into()));
    sandbox.build().expect("and it builds");
}

#[test]
fn every_way_an_icon_can_break_the_rule_is_one_sentence() {
    // Verbatim, because these are what a CI log shows a plugin author and
    // what a host logs: each names the section, what was measured, and the
    // rule it broke, in that order.
    assert_eq!(
        refused_for(&[(ICON_SECTION, b"\x89PNG\r\n\x1a\nnot an IHDR")]),
        "crook.icon is not a PNG"
    );
    assert_eq!(
        refused_for(&[(ICON_SECTION, &png(300, 200))]),
        "crook.icon is 300×200 and an icon is square"
    );
    assert_eq!(
        refused_for(&[(ICON_SECTION, &png(512, 512))]),
        "crook.icon is 512 px a side and an icon is 32 to 256"
    );
    assert_eq!(
        refused_for(&[(ICON_SECTION, &png(16, 16))]),
        "crook.icon is 16 px a side and an icon is 32 to 256"
    );
    assert_eq!(
        refused_for(&[(ICON_SECTION, &png_of_at_least(128, 128, 40 * 1024))]),
        "crook.icon is 41 KiB and the limit is 32 KiB"
    );
    // One byte over is still over, and is not described with the limit's
    // own number.
    assert_eq!(
        refused_for(&[(
            ICON_SECTION,
            &png_of_at_least(128, 128, MAX_ICON_BYTES - 32)
        )]),
        "crook.icon is 33 KiB and the limit is 32 KiB"
    );
    assert_eq!(
        refused_for(&[(ICON_SECTION, &apng(128, 128))]),
        "crook.icon is animated, and an icon holds still"
    );
    assert_eq!(
        refused_for(&[(ICON_SECTION, &png(64, 64)), (ICON_SECTION, &png(64, 64))]),
        "crook.icon is in the module twice"
    );
}

#[test]
fn every_way_a_preview_can_break_the_rule_is_one_sentence() {
    assert_eq!(
        refused_for(&[("crook.preview.2", b"GIF89a")]),
        "crook.preview.2 is not a PNG"
    );
    assert_eq!(
        refused_for(&[("crook.preview.2", &png_of_at_least(640, 128, 700 * 1024))]),
        "crook.preview.2 is 701 KiB and the limit is 512 KiB"
    );
    assert_eq!(
        refused_for(&[(
            "crook.preview.2",
            &png_of_at_least(640, 128, MAX_PREVIEW_BYTES)
        )]),
        "crook.preview.2 is 513 KiB and the limit is 512 KiB"
    );
    assert_eq!(
        refused_for(&[("crook.preview.3", &png(2500, 900))]),
        "crook.preview.3 is 2500×900 and a side is at most 2048"
    );
    assert_eq!(
        refused_for(&[("crook.preview.3", &png(900, 2049))]),
        "crook.preview.3 is 900×2049 and a side is at most 2048"
    );
    assert_eq!(
        refused_for(&[("crook.preview.1", &apng(640, 128))]),
        "crook.preview.1 is animated, and a preview holds still"
    );
    for name in [
        "crook.preview.x",
        "crook.preview.7",
        "crook.preview.0",
        "crook.preview.01",
    ] {
        assert_eq!(
            refused_for(&[(name, &png(640, 128))]),
            format!("{name} is not a number from 1 to 6")
        );
    }
    assert_eq!(
        refused_for(&[
            ("crook.preview.1", &png(640, 128)),
            ("crook.preview.1", &png(640, 128))
        ]),
        "crook.preview.1 is in the module twice"
    );
    // The edge itself is inside the rule.
    let (_, _, pictures) = Sandbox::open_with_pictures(
        &carrying(&[("crook.preview.6", &png(2048, 2048))]),
        Fuel::default(),
    )
    .expect("it should open");
    assert_eq!(pictures.expect("2048 is allowed").previews[0].width, 2048);
}

#[test]
fn every_way_a_caption_can_break_the_rule_is_one_sentence() {
    let picture = png(640, 128);
    let long = "x".repeat(MAX_CAPTION_CHARS + 11);
    assert_eq!(
        refused_for(&[
            ("crook.preview.2", &picture),
            ("crook.caption.2", long.as_bytes())
        ]),
        "crook.caption.2 is 91 characters and a caption is at most 80"
    );
    // Characters, not bytes: eighty of a two-byte letter is eighty.
    let (_, _, pictures) = Sandbox::open_with_pictures(
        &carrying(&[
            ("crook.preview.2", &picture),
            ("crook.caption.2", "é".repeat(MAX_CAPTION_CHARS).as_bytes()),
        ]),
        Fuel::default(),
    )
    .expect("it should open");
    assert_eq!(
        pictures.expect("eighty characters is allowed").previews[0]
            .caption
            .as_deref()
            .map(str::len),
        Some(2 * MAX_CAPTION_CHARS)
    );
    assert_eq!(
        refused_for(&[("crook.caption.2", b"The chip in the header")]),
        "crook.caption.2 names no picture"
    );
    assert_eq!(
        refused_for(&[
            ("crook.preview.2", &picture),
            ("crook.caption.2", b"\xff\xfe")
        ]),
        "crook.caption.2 is not text"
    );
    assert_eq!(
        refused_for(&[
            ("crook.preview.2", &picture),
            ("crook.caption.2", b"one\ntwo")
        ]),
        "crook.caption.2 is not one line"
    );
    assert_eq!(
        refused_for(&[("crook.preview.2", &picture), ("crook.caption.9", b"nine")]),
        "crook.caption.9 is not a number from 1 to 6"
    );
    assert_eq!(
        refused_for(&[
            ("crook.preview.2", &picture),
            ("crook.caption.2", b"one"),
            ("crook.caption.2", b"two")
        ]),
        "crook.caption.2 is in the module twice"
    );
    // An empty caption is no caption.
    let (_, _, pictures) = Sandbox::open_with_pictures(
        &carrying(&[("crook.preview.2", &picture), ("crook.caption.2", b"")]),
        Fuel::default(),
    )
    .expect("it should open");
    assert_eq!(
        pictures
            .expect("nothing under a picture is allowed")
            .previews[0]
            .caption,
        None
    );
    // The sentences are built from the same names the macros write.
    assert_eq!(format!("{PREVIEW_SECTION_PREFIX}2"), "crook.preview.2");
    assert_eq!(format!("{CAPTION_SECTION_PREFIX}2"), "crook.caption.2");
}

#[test]
fn a_size_is_read_off_the_header_and_nothing_else() {
    assert_eq!(png_size(&png(640, 128)), Some((640, 128)));
    assert_eq!(png_size(&png(1, 1)), Some((1, 1)));
    // Not a PNG: the wrong signature, a first chunk that is not IHDR, too
    // short to hold one, or a side of zero, which the specification forbids.
    assert_eq!(png_size(b"GIF89a"), None);
    assert_eq!(png_size(&png(640, 128)[..20]), None);
    let mut wrong_chunk = png(640, 128);
    wrong_chunk[12..16].copy_from_slice(b"tEXt");
    assert_eq!(png_size(&wrong_chunk), None);
    assert_eq!(png_size(&png(0, 128)), None);
    assert_eq!(png_size(&png(640, 0)), None);
}

#[test]
fn an_animation_is_a_chunk_name_before_the_first_image_data() {
    assert!(is_apng(&apng(64, 64)));
    assert!(!is_apng(&png(64, 64)));
    // Where the specification says it may not be: after the data. Nothing
    // would play it, so nothing here refuses it.
    assert!(!is_apng(&png_with(
        64,
        64,
        &[(b"IDAT", &[]), (b"acTL", &[0; 8])]
    )));
    // A chunk claiming more bytes than there are is a walk that stops, not
    // one that reads past the end.
    let mut truncated = apng(64, 64);
    truncated[8..12].copy_from_slice(&u32::MAX.to_be_bytes());
    assert!(!is_apng(&truncated));
    assert!(!is_apng(b"\x89PNG\r\n\x1a\n\0\0"));
    assert!(!is_apng(b""));
}

#[test]
fn something_that_is_not_a_module_has_no_pictures_to_read() {
    let why = Pictures::read_bytes(b"this is not a plugin").expect_err("it is not a module");

    assert!(why.starts_with("not a WebAssembly module"), "{why}");
}

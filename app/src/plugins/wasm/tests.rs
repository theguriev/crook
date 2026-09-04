//! A plugin that is not in the binary, loaded into one that is.
//!
//! The module below is written in wasm's text format and assembled here, so
//! these run with no wasm toolchain installed — and it is a *real* module, so
//! what is being tested is the boundary rather than a mock of it.

use std::fs;
use std::path::{Path, PathBuf};

use crook_plugin_api::{Capability, Gap, Manifest, Node, Size, Tone, to_bytes};

use super::*;

/// A directory of this test's own.
pub(crate) struct Scratch {
    path: PathBuf,
}

impl Scratch {
    pub(crate) fn new(name: &str) -> Self {
        static SERIAL: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let serial = SERIAL.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let path =
            std::env::temp_dir().join(format!("crook-wasm-{name}-{}-{serial}", std::process::id()));
        let _ = fs::remove_dir_all(&path);
        fs::create_dir_all(&path).expect("the scratch directory should be creatable");
        Self { path }
    }

    pub(crate) fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

/// What the test plugin says about itself.
pub(crate) fn manifest(id: &str) -> Manifest {
    Manifest {
        abi: crook_plugin_api::ABI_VERSION,
        id: id.to_owned(),
        name: "Probe".to_owned(),
        description: "A plugin that exists to be looked at.".to_owned(),
        version: "0.1.0".to_owned(),
        capabilities: vec![Capability::ReadTabs],
    }
}

/// What it asks to have drawn.
pub(crate) fn tree() -> Node {
    Node::Row(vec![
        Node::Icon {
            name: "git-branch".to_owned(),
            tone: Tone::Muted,
        },
        Node::Gap(Gap::Small),
        Node::Text {
            text: "from a sandbox".to_owned(),
            size: Size::Small,
            tone: Tone::Muted,
        },
        Node::Badge {
            text: "probed".to_owned(),
            tone: Tone::Accent,
        },
        Node::Button {
            label: "Poke".to_owned(),
            action: "poke".to_owned(),
            tone: Tone::Accent,
        },
    ])
}

/// `bytes` as a wasm data-segment string.
fn escaped(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("\\{byte:02x}")).collect()
}

/// A module that contributes `tree()` to `slot` at `order`, and offers one
/// action.
pub(crate) fn wasm(id: &str, slot: &str, order: i32) -> Vec<u8> {
    let manifest = to_bytes(&manifest(id)).expect("a manifest should encode");
    let tree = to_bytes(&tree()).expect("a tree should encode");
    let tree_at = 16 + manifest.len() as u32;
    let strings_at = 4096;
    let entry_at = strings_at + slot.len() as u32;
    let action_at = entry_at + 4;
    let title_at = action_at + 4;

    let text = format!(
        r#"(module
            (import "crook" "contribute"
              (func $contribute (param i32 i32 i32 i32 i32)))
            (import "crook" "register_action"
              (func $register_action (param i32 i32 i32 i32)))
            (memory (export "memory") 1)
            (global $next (mut i32) (i32.const 8192))
            (data (i32.const 16) "{manifest_bytes}")
            (data (i32.const {tree_at}) "{tree_bytes}")
            (data (i32.const {strings_at}) "{slot}")
            (data (i32.const {entry_at}) "here")
            (data (i32.const {action_at}) "poke")
            (data (i32.const {title_at}) "Poke the probe")
            (func (export "crook_abi_version") (result i32) (i32.const {abi}))
            (func (export "crook_alloc") (param $len i32) (result i32)
              (local $at i32)
              (local.set $at (global.get $next))
              (global.set $next (i32.add (global.get $next) (local.get $len)))
              (local.get $at))
            (func (export "crook_manifest") (result i64)
              (i64.or (i64.shl (i64.const 16) (i64.const 32)) (i64.const {manifest_len})))
            (func (export "crook_build") (result i32)
              (call $contribute
                (i32.const {strings_at}) (i32.const {slot_len})
                (i32.const {entry_at}) (i32.const 4)
                (i32.const {order}))
              (call $register_action
                (i32.const {action_at}) (i32.const 4)
                (i32.const {title_at}) (i32.const 14))
              (i32.const 0))
            (func (export "crook_render") (param i32 i32) (result i64)
              (i64.or (i64.shl (i64.const {tree_at}) (i64.const 32)) (i64.const {tree_len})))
            (func (export "crook_run") (param i32 i32) (result i32) (i32.const 0)))"#,
        manifest_bytes = escaped(&manifest),
        manifest_len = manifest.len(),
        tree_bytes = escaped(&tree),
        tree_len = tree.len(),
        slot_len = slot.len(),
        abi = crook_plugin_api::ABI_VERSION,
    );

    wat::parse_str(&text).expect("the test module should assemble")
}

/// Installs one into `directory` under its own folder.
pub(crate) fn install(directory: &Path, folder: &str, wasm: &[u8]) {
    let home = directory.join(folder);
    fs::create_dir_all(&home).expect("the plugin directory should be creatable");
    fs::write(home.join(MODULE_FILE), wasm).expect("the module should be writable");
}

#[test]
fn a_module_in_the_plugins_directory_is_found_and_read() {
    let scratch = Scratch::new("found");
    install(
        scratch.path(),
        "probe",
        &wasm("eugen/probe", "header.right", 0),
    );

    let found = installed(scratch.path());

    assert_eq!(found.len(), 1);
    let manifest = found[0].manifest();
    assert_eq!(manifest.id.to_string(), "eugen/probe");
    assert_eq!(manifest.name, "Probe");
    assert_eq!(manifest.tier, Tier::Wasm);
}

#[test]
fn a_directory_that_is_not_there_is_no_plugins_and_not_a_failure() {
    // The ordinary state of a fresh install, and not worth a line in the log
    // let alone a refusal to start.
    assert!(installed(Path::new("/nowhere/at/all/really")).is_empty());
}

#[test]
fn something_in_the_directory_that_is_not_a_plugin_costs_only_itself() {
    let scratch = Scratch::new("rubbish");
    install(scratch.path(), "rubbish", b"not a wasm module");
    install(
        scratch.path(),
        "probe",
        &wasm("eugen/probe", "header.right", 0),
    );
    // And a directory with nothing in it at all.
    fs::create_dir_all(scratch.path().join("empty")).expect("creatable");

    let found = installed(scratch.path());

    assert_eq!(
        found.len(),
        1,
        "the good plugin did not survive the bad one"
    );
}

#[test]
fn plugins_are_loaded_in_a_order_that_is_the_same_on_every_machine() {
    // A slot settles two entries of equal `order` by load order, so a load
    // order that came from whatever `read_dir` happened to answer would put
    // two plugins in a different place on two machines.
    let scratch = Scratch::new("order");
    for (folder, id) in [("zeta", "eugen/zeta"), ("alpha", "eugen/alpha")] {
        install(scratch.path(), folder, &wasm(id, "header.right", 0));
    }

    let ids: Vec<String> = installed(scratch.path())
        .iter()
        .map(|plugin| plugin.manifest().id.to_string())
        .collect();

    assert_eq!(ids, ["eugen/alpha", "eugen/zeta"]);
}

#[test]
fn a_manifest_naming_an_id_that_is_not_one_is_refused() {
    let scratch = Scratch::new("bad-id");
    install(
        scratch.path(),
        "probe",
        &wasm("Eugen/Probe", "header.right", 0),
    );

    assert!(installed(scratch.path()).is_empty());
}

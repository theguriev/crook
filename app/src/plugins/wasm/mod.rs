//! A plugin that is not in the binary.
//!
//! The second tier: a `.wasm` file in the plugins directory, run in
//! [`crook_wasm`], describing what it wants drawn rather than drawing it. This
//! is the half that lives in the application — what a guest's registrations
//! *mean* here, and what happens when one misbehaves.
//!
//! # A guest names things with strings, and every one of them is resolved
//!
//! A native plugin names a [`SlotId`] and cannot name one that does not exist.
//! A guest hands over a string, so every string is looked up: a slot nothing
//! declares is a refused contribution with a line saying which plugin asked
//! for what, and an action name is prefixed with the plugin's own id before it
//! is registered, so a plugin cannot claim somebody else's action.
//!
//! # Nothing it does may cost a person their window
//!
//! [`crook_wasm`] guarantees that a call *returns* — fuel, memory and bounds
//! are its problem. What is left is this file's: a call that returned an error
//! must leave a frame that still draws. So a render that fails draws nothing
//! and says so once, and a plugin that fails [`GIVE_UP_AFTER`] times in a row
//! is switched off rather than asked again every frame — a plugin that traps
//! on frame one will trap on frame two, and a terminal that finds that out
//! sixty times a second has stopped working.

mod render;

use std::cell::{Cell, RefCell};
use std::fs;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use crookui_core::prelude::*;

use crook_plugin::{ActionName, Manifest, PluginId, Tier};
use crook_wasm::{Fuel, Sandbox};

use crate::plugin::{BuildError, Host, Plugin};
use crate::workspace::Workspace;

/// The file a plugin's directory has to hold.
const MODULE_FILE: &str = "plugin.wasm";

/// How many failures in a row a plugin gets before it stops being asked.
///
/// More than one, because a plugin that failed once may have failed on
/// something transient; few enough that a broken plugin costs a handful of
/// frames rather than every frame for the rest of the session.
const GIVE_UP_AFTER: u32 = 3;

/// Every plugin installed in `directory`, in name order.
///
/// A directory each, holding `plugin.wasm`, which is the shape the store
/// installs. Anything that is not that is skipped with a line: a directory
/// with no module, a module that is not wasm, a manifest naming an id that is
/// not an id. **None of them stops the others loading**, and none of them
/// stops the window opening — the rule the settings file has followed since
/// the beginning.
pub fn installed(directory: &Path) -> Vec<Box<dyn Plugin>> {
    let Ok(entries) = fs::read_dir(directory) else {
        // No plugins directory is the ordinary state and not worth a line.
        return Vec::new();
    };

    let mut found: Vec<(String, Box<dyn Plugin>)> = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path().join(MODULE_FILE);
        if !path.is_file() {
            continue;
        }
        match open(&path) {
            Ok(plugin) => found.push((plugin.manifest().id.to_string(), Box::new(plugin))),
            Err(problem) => log::warn!("{}: {problem}", path.display()),
        }
    }

    // By id rather than by whatever order the filesystem answered in, so that
    // two plugins contributing to one slot at the same order are drawn in an
    // order that is the same on every machine.
    found.sort_by(|(left, _), (right, _)| left.cmp(right));
    found.into_iter().map(|(_, plugin)| plugin).collect()
}

/// Reads one module and checks everything that can be checked before it runs.
pub fn open(path: &Path) -> Result<WasmPlugin, String> {
    let bytes = fs::read(path).map_err(|why| format!("could not be read: {why}"))?;
    let (sandbox, manifest) =
        Sandbox::open(&bytes, Fuel::default()).map_err(|why| why.to_string())?;

    let id = PluginId::parse(&manifest.id)
        .map_err(|why| format!("its id {:?} is not one: {why}", manifest.id))?;

    // Leaked, because `Plugin::manifest` hands back a `&'static Manifest` and
    // a native plugin's is a literal. One leak per installed plugin, once, for
    // as long as the process lives — which is the same lifetime a `static`
    // would have had.
    let manifest: &'static Manifest = Box::leak(Box::new(Manifest {
        schema: Manifest::SCHEMA,
        id,
        name: String::leak(manifest.name),
        description: String::leak(manifest.description),
        version: String::leak(manifest.version),
        tier: Tier::Wasm,
    }));

    Ok(WasmPlugin {
        manifest,
        sandbox: Rc::new(RefCell::new(sandbox)),
        failures: Rc::new(Cell::new(0)),
    })
}

/// Where the store installs what a person chose.
///
/// Under the platform's data directory rather than beside the binary: a
/// plugin is a person's, not the installation's, and a binary directory is
/// often not writable by whoever is running it.
pub fn directory() -> Option<PathBuf> {
    dirs::data_dir().map(|data| data.join("crook").join("plugins"))
}

/// One installed plugin.
pub struct WasmPlugin {
    manifest: &'static Manifest,
    /// Shared, because a contribution is an `Fn` and running the guest is not:
    /// the borrow is taken for the length of one call and given back.
    sandbox: Rc<RefCell<Sandbox>>,
    /// How many calls in a row have failed. See [`GIVE_UP_AFTER`].
    failures: Rc<Cell<u32>>,
}

impl Plugin for WasmPlugin {
    fn manifest(&self) -> &'static Manifest {
        self.manifest
    }

    fn build(&mut self, host: &mut Host, _: &mut ViewContext<Workspace>) -> Result<(), BuildError> {
        let registered = self
            .sandbox
            .borrow_mut()
            .build()
            .map_err(|why| why.to_string())?;

        for contribution in registered.contributions {
            let Some(slot) = host.slot_named(&contribution.slot) else {
                // Refused, not fatal: a plugin written against a Crook that
                // has a slot this one does not should be missing that one
                // contribution, not missing entirely.
                log::warn!(
                    "{} contributes to {:?}, which is not a slot this build has",
                    self.manifest.id,
                    contribution.slot
                );
                continue;
            };

            let sandbox = self.sandbox.clone();
            let failures = self.failures.clone();
            let name = contribution.slot.clone();
            let who = self.manifest.id.clone();
            host.contribute(
                slot,
                contribution.entry,
                contribution.order,
                move |workspace, _| {
                    let node = ask(&sandbox, &failures, &who, &name);
                    let host = workspace.host();
                    let prefix = who.clone();
                    render::element(&node, workspace.fonts().ui, &move |action| {
                        ActionName::parse(&format!("{prefix}/{action}"))
                            .ok()
                            .and_then(|name| host.action(&name))
                    })
                },
            );
        }

        for action in registered.actions {
            // Prefixed here, so a guest cannot name an action belonging to
            // anybody else however it spells its own.
            let Ok(name) = ActionName::parse(&format!("{}/{}", self.manifest.id, action.name))
            else {
                log::warn!(
                    "{} offers an action called {:?}, which is not a name",
                    self.manifest.id,
                    action.name
                );
                continue;
            };

            let sandbox = self.sandbox.clone();
            let failures = self.failures.clone();
            let who = self.manifest.id.clone();
            let called = action.name.clone();
            let run = move |_: &mut Workspace, _: &mut ViewContext<Workspace>| {
                if failures.get() >= GIVE_UP_AFTER {
                    return;
                }
                match sandbox.try_borrow_mut() {
                    Ok(mut sandbox) => match sandbox.run(&called) {
                        Ok(()) => failures.set(0),
                        Err(problem) => {
                            log::warn!("{who}: {problem}");
                            failures.set(failures.get() + 1);
                        }
                    },
                    // The guest is already running: an action of its own
                    // reached back into it. Nothing to do but decline.
                    Err(_) => log::warn!("{who} asked to run {called:?} while it was running"),
                }
            };

            match action.title {
                Some(title) => {
                    host.register_command(name, title, run);
                }
                None => {
                    host.register_action(name, run);
                }
            }
        }

        Ok(())
    }
}

/// Asks the guest what it wants drawn, and gives up on it if it keeps failing.
fn ask(
    sandbox: &Rc<RefCell<Sandbox>>,
    failures: &Rc<Cell<u32>>,
    who: &PluginId,
    slot: &str,
) -> crook_plugin_api::Node {
    if failures.get() >= GIVE_UP_AFTER {
        return crook_plugin_api::Node::Empty;
    }
    // Already running: a guest's own render reached back into it, which it
    // cannot do through this API and so means a bug here rather than there.
    let Ok(mut sandbox) = sandbox.try_borrow_mut() else {
        return crook_plugin_api::Node::Empty;
    };

    match sandbox.render(slot) {
        Ok(node) => {
            failures.set(0);
            node
        }
        Err(problem) => {
            let count = failures.get() + 1;
            failures.set(count);
            log::warn!("{who}: {problem}");
            if count == GIVE_UP_AFTER {
                log::warn!("{who} has failed {count} times and will not be asked again");
            }
            crook_plugin_api::Node::Empty
        }
    }
}

#[cfg(test)]
#[path = "tests.rs"]
pub(crate) mod tests;

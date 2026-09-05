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

mod install;
mod picker;
mod render;
mod runtime;

use std::cell::{Cell, RefCell};
use std::fs;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use crookui_core::prelude::*;

use crook_plugin::{ActionName, Manifest, PluginId, Tier};
use crook_plugin_api::{Answer, Command, Facts, Request};
use crook_wasm::{Fuel, Sandbox};

use crate::plugin::{BuildError, Host, Plugin};
use crate::workspace::Workspace;

pub use install::install;
use picker::Chrome;
use render::Placement;
use runtime::{Gesture, Runtime};

/// The file a plugin's directory has to hold.
const MODULE_FILE: &str = "plugin.wasm";

/// The four actions the host registers in a plugin's name, and what each
/// does with the picker that plugin has up.
///
/// In its name because that is where an action's name comes from — the host
/// puts `owner/name/` on the front of everything, including these — and
/// registered *before* the guest's own, so that a plugin spelling one of these
/// loses its own action rather than taking the keyboard's. The registry
/// refuses the second registration of a name and says so in the audit, which
/// is exactly the right amount of noise for a plugin doing something odd.
const PICKER_KEYS: [&str; 4] = [
    "crook-picker-next",
    "crook-picker-previous",
    "crook-picker-choose",
    "crook-picker-close",
];

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
        // Carried across rather than dropped on the floor here, which is what
        // this used to do: what a plugin asks to be allowed to do is the one
        // thing about it a person has to read *before* deciding anything, and
        // a capability that never leaves this function is one the Plugins page
        // cannot show and nobody can refuse. Leaked with the strings above and
        // for the same reason.
        capabilities: Vec::leak(manifest.capabilities),
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

    fn build(
        &mut self,
        host: &mut Host,
        ctx: &mut ViewContext<Workspace>,
    ) -> Result<(), BuildError> {
        let registered = self
            .sandbox
            .borrow_mut()
            .build()
            .map_err(|why| why.to_string())?;

        // What it may do, read once. A grant is changed on the Plugins page
        // and takes effect when the plugin is built again, which is the same
        // rule its switch follows: nothing a person allows or forbids should
        // land on a plugin half way through a frame.
        let granted = host.granted(&self.manifest.id).to_vec();
        let runtime = ctx.add_model(|_| {
            Runtime::new(
                self.manifest.id.clone(),
                self.sandbox.clone(),
                self.failures.clone(),
                granted,
            )
        });
        // The bridge every model-backed feature in Crook has: an answer that
        // lands changes what the chip says, and the row around it has to be
        // laid out again — and, since a workspace is exactly what this
        // observer is handed and the model has none, it is also where the
        // things only a workspace can answer are answered. See
        // `runtime::Runtime::deeds`.
        ctx.observe(&runtime, |workspace, runtime, ctx| {
            loop {
                let deeds = runtime.update(ctx, |runtime, _| runtime.deeds());
                if deeds.is_empty() {
                    break;
                }
                for (ticket, request) in deeds {
                    let answer = serve(workspace, &request, ctx);
                    runtime.update(ctx, |runtime, ctx| runtime.answer(ticket, answer, ctx));
                }
            }
            ctx.notify();
        });

        // What the host holds on this plugin's behalf: the field of whatever
        // picker it has up, the row the keyboard is on, the menu that is
        // open, and what the last thing pressed had to say. See `picker`.
        let chrome = Rc::new(Chrome::new(
            picker::Keys {
                next: reserved(&self.manifest.id, PICKER_KEYS[0]),
                previous: reserved(&self.manifest.id, PICKER_KEYS[1]),
                choose: reserved(&self.manifest.id, PICKER_KEYS[2]),
                close: reserved(&self.manifest.id, PICKER_KEYS[3]),
            },
            host.voice(),
        ));

        for (name, by) in [(PICKER_KEYS[0], 1_isize), (PICKER_KEYS[1], -1)] {
            let chrome = chrome.clone();
            host.register_action(reserved(&self.manifest.id, name), move |_, ctx| {
                chrome.move_selection(by);
                ctx.notify();
            });
        }
        host.register_action(reserved(&self.manifest.id, PICKER_KEYS[2]), {
            let chrome = chrome.clone();
            move |workspace, ctx| {
                // Through the ordinary action path, which is what makes Enter
                // and a click on the row the same gesture: both say what was
                // chosen and then run the plugin's own action.
                let Some((action, key)) = chrome.chosen() else {
                    return;
                };
                chrome.say(key);
                workspace.run_action(action, ctx);
            }
        });
        host.register_action(reserved(&self.manifest.id, PICKER_KEYS[3]), {
            let chrome = chrome.clone();
            move |workspace, ctx| {
                // The host lets go of the keyboard, and the plugin is told its
                // panel was dismissed — in that order, so a plugin that opens
                // something of its own out of `dismiss` is not opening it into
                // a keyboard this still owns.
                let dismiss = chrome.dismissal();
                chrome.shut();
                workspace.sync_input_keys();
                if let Some(action) = dismiss {
                    workspace.run_action(action, ctx);
                }
                ctx.notify();
            }
        });

        // Claimed as a *panel* rather than as a surface: what this plugin puts
        // up hangs off a chip in a place, and a place can stop being drawn.
        // See `Host::claim_panel`.
        let showing = host.claim_panel(
            {
                let chrome = chrome.clone();
                move |keystroke| chrome.claims(keystroke)
            },
            {
                let chrome = chrome.clone();
                move || chrome.shut()
            },
        );
        chrome.armed_by(showing);

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
            let entry = contribution.entry.clone();
            let who = self.manifest.id.clone();
            let chrome = chrome.clone();
            let placement = placement(&contribution.slot);
            // Made once, here, and kept for as long as the contribution is on
            // screen: a plugin's controls have no identity of their own, so
            // what remembers that one of them is under the pointer is the
            // entry they were drawn from. See [`render::Hovers`].
            let hovers = Rc::new(render::Hovers::default());
            host.contribute(
                slot,
                contribution.entry,
                contribution.order,
                move |workspace, _| {
                    let node = ask(&sandbox, &failures, &who, &name, &entry);
                    let host = workspace.host();
                    let prefix = who.clone();
                    let drawn = render::element(
                        &node,
                        &render::Surroundings {
                            fonts: workspace.fonts(),
                            placement,
                            action: &move |action| {
                                ActionName::parse(&format!("{prefix}/{action}"))
                                    .ok()
                                    .and_then(|name| host.action(&name))
                            },
                            hovers: &hovers,
                            chrome: &chrome,
                            clipboard: workspace.clipboard(),
                        },
                    );
                    // A picker that has just appeared has taken the keyboard
                    // off the pane it is drawn in, and nothing else would say
                    // so: what put it up is the plugin's own state, which the
                    // host finds out about by drawing it.
                    if chrome.keyboard_moved() {
                        workspace.sync_input_keys();
                    }
                    drawn
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
            let runtime = runtime.clone();
            let chrome = chrome.clone();
            let run = move |workspace: &mut Workspace, ctx: &mut ViewContext<Workspace>| {
                if failures.get() >= GIVE_UP_AFTER {
                    return;
                }
                // Whatever the thing that was pressed had to say: the key of a
                // chosen row, the entry of a menu, or what the command line
                // handed a `--action`. Empty for a chord, for the palette and
                // for another plugin, which is most of the ways an action is
                // reached.
                let argument = workspace.host().said();
                match sandbox.try_borrow_mut() {
                    Ok(mut sandbox) => match sandbox.run(&called, &argument) {
                        Ok(()) => failures.set(0),
                        Err(problem) => {
                            log::warn!("{who}: {problem}");
                            failures.set(failures.get() + 1);
                        }
                    },
                    // The guest is already running: an action of its own
                    // reached back into it. Nothing to do but decline.
                    Err(_) => {
                        log::warn!("{who} asked to run {called:?} while it was running");
                        return;
                    }
                }
                // The panel this ran out of may have closed itself: what a
                // plugin's action does to its own state is its own business,
                // and the host finds out by drawing it again. Letting go here
                // rather than guessing means the keyboard is the pane's until
                // the next frame puts a picker back on screen.
                chrome.released();
                workspace.sync_input_keys();

                // Whatever pressing it made the plugin ask for. An action is
                // one of the four calls that reach a context, which is what
                // makes "the button refreshes the reading" work at all — and
                // it is the *only* one that may type into a shell, which is
                // what `Gesture` carries.
                runtime.update(ctx, |runtime, ctx| runtime.pump(Gesture::Pressed, ctx));
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

        // Everything the build asked for. A plugin that reads a file and then
        // fetches what the file authorised starts here and carries on by
        // itself, because a delivery pumps again. Nobody pressed anything to
        // get here, so nothing that changes the window is taken from it.
        runtime.update(ctx, |runtime, ctx| runtime.pump(Gesture::None, ctx));

        Ok(())
    }
}

/// `owner/name/<one of the host's own>`.
///
/// The host registering an action in a plugin's name is not a trick: an
/// action's name says which plugin it belongs to, and these belong to the
/// plugin whose picker they move. What keeps them apart from the guest's own
/// is that they are registered first and that they are spelled like nothing a
/// plugin would call its own.
fn reserved(plugin: &PluginId, name: &str) -> ActionName {
    ActionName::parse(&format!("{plugin}/{name}")).expect("a name built from an id and a literal")
}

/// Where a panel hangs, given the slot the chip is in.
///
/// The host's answer rather than the plugin's, and the slot is the whole of
/// what it is worked out from: the header has the window below it, and a
/// pane's own chips sit at the foot of one. A slot this build has never heard
/// of is treated as the header's, which is the arrangement a chip in a row is
/// usually in.
fn placement(slot: &str) -> Placement {
    if slot == crate::plugins::pane::PANE_CHIPS.as_str() {
        Placement::Above
    } else {
        Placement::Below
    }
}

/// What only a workspace can answer.
///
/// The other half of `runtime::Runtime::deeds`: everything here is a question
/// about, or a change to, the window the plugin is drawn in, and the model
/// that holds the plugin cannot see one. Every one of these has already been
/// checked against what a person granted.
fn serve(workspace: &mut Workspace, request: &Request, ctx: &mut ViewContext<Workspace>) -> Answer {
    match request {
        Request::Where => {
            let (directory, facts) = workspace.focused_facts(ctx);
            let diff = facts.as_ref().and_then(|facts| facts.diff.as_ref());
            Answer::Where(Facts {
                directory: directory.map(|path| path.to_string_lossy().into_owned()),
                home: dirs::home_dir().map(|path| path.to_string_lossy().into_owned()),
                branch: facts
                    .as_ref()
                    .and_then(|facts| facts.branch.as_ref())
                    .map(|head| head.label().to_owned()),
                // Zero rather than absent for a directory git has said nothing
                // about, because the wire has no third answer and "nothing
                // changed" is what a chip would print either way.
                added: diff.map_or(0, |diff| diff.lines_added),
                removed: diff.map_or(0, |diff| diff.lines_removed),
            })
        }
        Request::Commands => {
            let keybindings = workspace.keybindings();
            Answer::Commands(
                workspace
                    .host()
                    .commands()
                    .iter()
                    .map(|(_, action, title)| Command {
                        name: action.to_string(),
                        title: title.clone(),
                        // The first, not all of them: a hint prints one chord,
                        // and a plugin that wanted the rest can ask the person
                        // to look at the Keyboard Shortcuts page, which is
                        // where the whole list already is.
                        chord: keybindings.chords_for(action).into_iter().next(),
                    })
                    .collect(),
            )
        }
        Request::Type { template, argument } => match fill(template, argument) {
            Some(line) if workspace.type_into_focused_pane(&line, ctx) => Answer::Done,
            Some(_) => Answer::Failed(String::from("there is no shell in that pane to type into")),
            None => Answer::Failed(String::from(
                "that is not something this can put in a command line",
            )),
        },
        Request::Run { name, argument } => {
            let Ok(name) = ActionName::parse(name) else {
                return Answer::Failed(format!("{name:?} is not the name of a command"));
            };
            let Some(id) = workspace.host().action(&name) else {
                return Answer::Failed(format!("{name} is not something this Crook can do"));
            };
            // Said before it is run and taken when it runs, which is the same
            // arrangement a picker's row uses to say which row it was.
            workspace.host().say(argument);
            workspace.run_action(id, ctx);
            Answer::Done
        }
        // Everything else is work and was done on the pool; reaching one of
        // these arms would be this file disagreeing with `runtime`.
        Request::Fetch { .. }
        | Request::ReadFile { .. }
        | Request::List { .. }
        | Request::Repository { .. } => {
            Answer::Failed(String::from("that is not something the window can do"))
        }
    }
}

/// The line a granted template and an argument make, or `None` for an
/// argument no command line should carry.
///
/// The template is the string a person allowed, so the *shape* of the command
/// is theirs and only the hole is the plugin's. What goes in the hole is
/// quoted here rather than by the plugin, because a plugin that quoted its own
/// argument would be a plugin trusted to do it right — and a branch called
/// `; rm -rf ~` is a branch name that has to stay one.
///
/// A control character is refused outright rather than escaped. A newline is
/// the character that ends a command line, a carriage return is what a shell's
/// line editor does something else with entirely, and a directory whose name
/// holds either is not worth the reasoning it would take to be sure.
fn fill(template: &str, argument: &str) -> Option<String> {
    if argument.chars().any(char::is_control) {
        return None;
    }
    let (before, after) = template.split_once("{}")?;
    Some(format!("{before}{}{after}", quote(argument)))
}

/// One argument, as a shell will read it as one word.
///
/// Single quotes, because inside them every shell this can reach treats every
/// character as itself — no expansion, no globbing, no command substitution.
/// The two families differ only in how a single quote is written inside them:
/// POSIX shells end the quoting, escape it and start again; PowerShell doubles
/// it.
#[cfg(not(windows))]
fn quote(argument: &str) -> String {
    format!("'{}'", argument.replace('\'', "'\\''"))
}

/// See the other one.
#[cfg(windows)]
fn quote(argument: &str) -> String {
    format!("'{}'", argument.replace('\'', "''"))
}

/// Asks the guest what it wants drawn, and gives up on it if it keeps failing.
fn ask(
    sandbox: &Rc<RefCell<Sandbox>>,
    failures: &Rc<Cell<u32>>,
    who: &PluginId,
    slot: &str,
    entry: &str,
) -> crook_plugin_api::Node {
    if failures.get() >= GIVE_UP_AFTER {
        return crook_plugin_api::Node::Empty;
    }
    // Already running: a guest's own render reached back into it, which it
    // cannot do through this API and so means a bug here rather than there.
    let Ok(mut sandbox) = sandbox.try_borrow_mut() else {
        return crook_plugin_api::Node::Empty;
    };

    match sandbox.render(slot, entry) {
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

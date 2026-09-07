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
pub(super) mod picker;
mod render;
mod runtime;
mod sound;

use std::cell::{Cell, RefCell};
use std::fs;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use crookui_core::prelude::*;

use crook_plugin::{ActionName, Manifest, PluginId, Tier};
use crook_plugin_api::{
    Answer, Capability, Command, Event, Node, Place, Render, Request, Status, Subject, TabFacts,
    TabInfo,
};
use crook_wasm::{Fuel, Sandbox};

use crate::plugin::{BuildError, Host, Plugin};
use crate::plugins::tabs::{TAB_ROW_BADGE, TabRow};
use crate::tab::AgentStatus;
use crate::workspace::Workspace;

pub use install::install;
use picker::Held;
use render::Placement;
use runtime::{Gesture, Runtime};

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
        {
            // Before `build`, because a guest may work out its first question
            // from what day it is.
            let minutes = chrono::Local::now().offset().local_minus_utc() / 60;
            self.sandbox.borrow_mut().set_timezone(minutes);
        }
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
        // What a grant comes to for a row, worked out once: a plugin's grant
        // cannot change while it is built, because answering on the Plugins
        // page is what rebuilds it.
        let sees = Sees::granted(&granted);
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
        // picker it has up, the row the keyboard is on, the menu that is open,
        // and what the last thing pressed had to say. The *keys* that act on
        // it are not registered here and are not this plugin's: there is one
        // set of them for the tier, on `crook/plugins`, because a panel is
        // modal and only one plugin can have one up. See `picker`.
        let chrome = Rc::new(Held::new(host.voice()));

        // Registered on the grant rather than on the export, so a plugin that
        // was refused hears nothing at all rather than being handed events it
        // is not allowed and having them dropped further in. A plugin that
        // asked and was allowed but exports no `crook_event` registers a watch
        // that does nothing, which is its own affair.
        if host
            .granted(&self.manifest.id)
            .iter()
            .any(|key| Capability::WatchCommands.keys().contains(key))
        {
            let watching = runtime.clone();
            host.watch_commands(Rc::new(move |_workspace, event: &Event, ctx| {
                let event = event.clone();
                watching.update(ctx, |runtime, ctx| runtime.notify(&event, ctx));
            }));
        }

        for contribution in registered.contributions {
            let sandbox = self.sandbox.clone();
            let failures = self.failures.clone();
            let name = contribution.slot.clone();
            let who = self.manifest.id.clone();
            // Made once, here, and kept for as long as the contribution is on
            // screen: a plugin's controls have no identity of their own, so
            // what remembers that one of them is under the pointer is the
            // entry they were drawn from. See [`render::Hovers`].
            let hovers = Rc::new(render::Hovers::default());

            if let Some(slot) = host.slot_named(&contribution.slot) {
                let entry = contribution.entry.clone();
                let chrome = chrome.clone();
                let placement = placement(&contribution.slot);
                host.contribute(
                    slot,
                    contribution.entry,
                    contribution.order,
                    move |workspace, _| {
                        let render = Render {
                            slot: name.clone(),
                            entry: entry.clone(),
                            subject: None,
                        };
                        let node = ask(&sandbox, &failures, &who, &render);
                        let element = drawn(
                            &node,
                            workspace,
                            &who,
                            render::Scale::ROW,
                            placement,
                            &chrome,
                            &hovers,
                        );
                        // A picker that has just appeared has taken the
                        // keyboard off the pane it is drawn in, and nothing
                        // else would say so: what put it up is the plugin's
                        // own state, which the host finds out by drawing it.
                        if chrome.keyboard_moved() {
                            workspace.sync_input_keys();
                        }
                        element
                    },
                );
                continue;
            }

            let Some(slot) = host.row_slot_named(&contribution.slot) else {
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

            // A mark and the badge on its corner are the same vocabulary drawn
            // at two sizes, and the size is the host's to decide.
            let entry = contribution.entry.clone();
            let chrome = chrome.clone();
            let scale = if slot == TAB_ROW_BADGE {
                render::Scale::BADGE
            } else {
                render::Scale::MARK
            };
            host.contribute_row(
                slot,
                contribution.entry,
                contribution.order,
                move |workspace, row, _| {
                    let render = Render {
                        slot: name.clone(),
                        entry: entry.clone(),
                        subject: Some(Subject::Tab(sees.facts(row, &who))),
                    };
                    match ask(&sandbox, &failures, &who, &render) {
                        // A row this plugin has nothing to say about, which
                        // is most rows for most plugins. The host draws what
                        // it would have drawn anyway — see `plugins::tabs`.
                        Node::Empty => None,
                        node => Some(drawn(
                            &node,
                            workspace,
                            &who,
                            scale,
                            // A mark on a row hangs nothing under it: there is
                            // one of it per row, and a panel per row is not a
                            // thing this slot can mean.
                            Placement::Below,
                            &chrome,
                            &hovers,
                        )),
                    }
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

                // The guest may have changed what it draws, and nothing out
                // here can tell whether it did: its state is inside the
                // module and what came back is a `()`. So every action asks
                // for the frame that will find out.
                //
                // Not belt and braces, because a press only *appears* to
                // redraw on its own — what notifies is the hover bookkeeping
                // under it, on the way past. An action that arrives without
                // one did not redraw at all, and `Node::Anchored`'s dismissal
                // is exactly that: the guest shut its panel, the frame went on
                // drawing it, and its modal underlay then ate every press
                // aimed at the controls beside it.
                ctx.notify();
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
            Answer::Where {
                place: directory.map(|directory| Place {
                    directory: directory.to_string_lossy().into_owned(),
                    branch: facts
                        .as_ref()
                        .and_then(|facts| facts.branch.as_ref())
                        .map(|head| head.label().to_owned()),
                    worktree: facts.as_ref().is_some_and(|facts| facts.worktree),
                }),
                home: dirs::home_dir().map(|path| path.to_string_lossy().into_owned()),
                // Zero rather than absent for a directory git has said nothing
                // about, because the wire has no third answer and "nothing
                // changed" is what a chip would print either way.
                added: diff.map_or(0, |diff| diff.lines_added),
                removed: diff.map_or(0, |diff| diff.lines_removed),
            }
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
        | Request::Repository { .. }
        | Request::Tally { .. }
        | Request::PlaySound { .. } => {
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
    render: &Render,
) -> Node {
    if failures.get() >= GIVE_UP_AFTER {
        return Node::Empty;
    }
    // Already running: a guest's own render reached back into it, which it
    // cannot do through this API and so means a bug here rather than there.
    let Ok(mut sandbox) = sandbox.try_borrow_mut() else {
        return Node::Empty;
    };

    match sandbox.render(render) {
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
            Node::Empty
        }
    }
}

/// Turns what a guest described into what the window draws.
///
/// The one place a plugin's action names are resolved, and the reason they are
/// resolved *here* rather than where they were registered: a name is looked up
/// every frame, so a plugin whose action was disabled between two frames draws
/// an inert control rather than one that dispatches into nothing.
fn drawn(
    node: &Node,
    workspace: &Workspace,
    who: &PluginId,
    scale: render::Scale,
    placement: Placement,
    chrome: &Rc<Held>,
    hovers: &render::Hovers,
) -> Box<dyn Element> {
    let host = workspace.host();
    let prefix = who.clone();
    render::element(
        node,
        render::Chrome::new(
            workspace.fonts(),
            scale,
            placement,
            chrome,
            workspace.clipboard(),
        ),
        &move |action| {
            ActionName::parse(&format!("{prefix}/{action}"))
                .ok()
                .and_then(|name| host.action(&name))
        },
        hovers,
    )
}

/// What one plugin may be told about a row.
///
/// A grant, reduced to the two questions a row raises, so that the answer is
/// a comparison of booleans per row rather than a walk of a list of granted
/// keys per row per frame. It is made when the plugin is built because that is
/// when a grant can change: answering on the Plugins page rebuilds the plugin
/// there and then.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
struct Sees {
    /// [`Capability::ReadTabs`]: what the tab is called, and what it is doing.
    tabs: bool,
    /// [`Capability::ReadWorkingDirectory`]: where it is working, and what git
    /// says about there.
    place: bool,
}

impl Sees {
    /// What these granted keys come to.
    fn granted(granted: &[String]) -> Self {
        Self {
            tabs: holds(granted, &Capability::ReadTabs),
            place: holds(granted, &Capability::ReadWorkingDirectory),
        }
    }

    /// One row, with everything that was not granted left out.
    ///
    /// The key is given to everybody. It is a hash of where the tab is
    /// working — of what it is called, for a session that has not said where
    /// that is — salted with the plugin's own id, so that a plugin can tell
    /// one row from another and keep telling them apart tomorrow, two plugins
    /// cannot work out that two of their rows are one row, and nothing about
    /// the tab can be read back out of the number. See `TabFacts::key`, which
    /// says what that is and is not.
    fn facts(self, row: &TabRow<'_>, who: &PluginId) -> TabFacts {
        let named = row
            .directory
            .map(|directory| directory.to_string_lossy().into_owned())
            .unwrap_or_else(|| row.title.to_owned());

        TabFacts {
            key: salted(who.as_str(), &named),
            tab: self.tabs.then(|| TabInfo {
                title: row.title.to_owned(),
                active: row.active,
                status: match row.status {
                    AgentStatus::Idle => Status::Idle,
                    AgentStatus::Running => Status::Running,
                    AgentStatus::NeedsInput => Status::NeedsInput,
                    AgentStatus::Failed => Status::Failed,
                },
            }),
            place: self
                .place
                .then_some(row.directory)
                .flatten()
                .map(|directory| Place {
                    directory: directory.to_string_lossy().into_owned(),
                    branch: row
                        .git
                        .and_then(|facts| facts.branch.as_ref())
                        .map(|head| head.label().to_owned()),
                    worktree: row.git.is_some_and(|facts| facts.worktree),
                }),
        }
    }
}

/// Whether every key a capability is written down as was granted.
fn holds(granted: &[String], capability: &Capability) -> bool {
    capability
        .keys()
        .iter()
        .all(|key| granted.iter().any(|allowed| allowed == key))
}

/// FNV-1a over the salt and the string, which is what a row's key is.
///
/// Written out rather than reached for, because the property that matters is
/// that the number is the *same next week*: `DefaultHasher` is explicitly not
/// stable across releases of the standard library, and a mark that changed
/// because Rust was upgraded would be a mark nobody could rely on. Nothing
/// here needs a hash to be hard to invert — see `TabFacts::key` for what this
/// number is and is not offered as.
fn salted(salt: &str, text: &str) -> u64 {
    const OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
    const PRIME: u64 = 0x0000_0100_0000_01b3;

    let mut hash = OFFSET;
    for byte in salt.as_bytes().iter().chain(b"\0").chain(text.as_bytes()) {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(PRIME);
    }
    hash
}

#[cfg(test)]
#[path = "tests.rs"]
pub(crate) mod tests;

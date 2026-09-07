//! The Plugins page: what this build is made of, one plugin at a time.
//!
//! The page a person opens to find out what Crook is. Until it existed,
//! "everything is a plugin" was a claim about the source rather than something
//! anybody could see, and a plugin that failed to load left one line in a log
//! nobody reads.
//!
//! # A list beside a card, which is VS Code's shape
//!
//! A column of every plugin the binary carries with a field above it — in the
//! sidebar, where the tab list otherwise is — and beside it whatever the list
//! has selected: the name, what it is, where it came from, what it puts on
//! screen, and the switch. That is the shape every extension manager has
//! settled on, and the reason is the same everywhere: a row can say a plugin's
//! name and whether it is on, and nothing else worth reading fits on one line.
//!
//! It is a *section of the sidebar* and not a page of the settings. Plugins
//! are not a setting — they are what the application is made of, and a person
//! looking for them is not looking for a preference.
//!
//! # It is a plugin listing plugins, and that is not a joke
//!
//! It reads the same [`Host`] every other surface reads, so it cannot fall
//! behind: a plugin added to `defaults` is on this page without this file
//! being touched, one installed into the plugins directory is on it too, and
//! this page is on the rail because a plugin put it there. It lists itself.
//!
//! # Everything a click does is a named action
//!
//! Selecting a row and flipping a switch are both actions —
//! `crook/plugins/show-crook-usage` and `crook/plugins/toggle-crook-usage` —
//! registered one per plugin in [`Plugin::ready`], because a page that offers
//! something per plugin cannot know how many that is until they have all
//! arrived. The switches are *commands*, so they are in the palette; the
//! selections are not, because a list of things to do should not be a list of
//! rows to look at.
//!
//! Two switches are drawn inert, and [`HOLDS_THE_PAGE`] says which and why: a
//! switch that removes the switch is a one-way door whose way back is editing
//! a JSON file.
//!
//! # Allowing is answered here and nowhere else
//!
//! A sandboxed plugin's manifest says what it wants to be allowed to do, and
//! asking is not being granted: until a person answers, every request it makes
//! is refused. The card is where the answer is given, because it is the only
//! surface that shows the whole list of what is being agreed to — which is why
//! `allow-` and `revoke-` are plain actions and not commands. A palette entry
//! called "Allow the CI plugin" would be a way to allow something without ever
//! reading it.

mod card;
mod list;
mod state;

use std::rc::Rc;

use crookui_core::prelude::*;

use crook_plugin::{Manifest, PluginId, Tier};
use crook_plugin_api::Capability;

use crate::plugin::{ActionName, BuildError, Cardinality, Host, Plugin, SlotId};
use crate::workspace::Workspace;
use crate::workspace::section;

use state::PluginsState;

/// Where the card has been scrolled to.
const CARD_SCROLL: &str = "plugins.card";

/// Where a plugin may say, on its own card, what it is currently doing.
///
/// A plugin's card can describe everything about it except the one thing only
/// the plugin knows: which of its choices is in force. A list of six sounds
/// with no mark on the one that is playing is a list somebody has to press
/// every row of to read. So the card carries a slot, and the entry drawn on it
/// is the one its own plugin contributed — the rest belong to other cards.
pub const PLUGIN_CARD: SlotId = SlotId::new("plugins.card.status");

/// The two plugins whose switches are drawn inert.
///
/// `crook/settings` declares the slot this page is contributed to, and this
/// plugin is the page. Switching either off from here would take the switch
/// away with it, and the way back would be a JSON file — so the page says so
/// instead of offering a door that only opens one way.
pub(super) const HOLDS_THE_PAGE: [&str; 2] = ["crook/settings", "crook/plugins"];

/// The plugin that lists the plugins.
pub struct Plugins {
    /// What the list has selected, and the field above it.
    ///
    /// Shared with the closures that draw the page and with the handlers that
    /// change it: a contribution is an `Fn` and a handler is an `Fn`, so
    /// neither can hold a `&mut` to this.
    state: Rc<PluginsState>,
}

impl Plugins {
    /// One, not showing anything yet.
    pub fn new() -> Self {
        Self {
            state: Rc::new(PluginsState::new()),
        }
    }
}

impl Default for Plugins {
    fn default() -> Self {
        Self::new()
    }
}

impl Plugin for Plugins {
    fn manifest(&self) -> &'static Manifest {
        manifest()
    }

    fn build(&mut self, host: &mut Host, _: &mut ViewContext<Workspace>) -> Result<(), BuildError> {
        // Just before About, which is where a "what is this build" page
        // belongs: after everything that configures the application and before
        // the one that describes it.
        host.declare_slot(PLUGIN_CARD, Cardinality::List);

        let state = self.state.clone();
        host.add_sidebar_section(
            "section",
            "Plugins",
            Lucide::Blocks,
            10,
            move |workspace, app| page(workspace, app, &state),
        );

        picker_keys(host);
        Ok(())
    }

    fn ready(&mut self, host: &mut Host, _: &mut ViewContext<Workspace>) -> Result<(), BuildError> {
        // One pair of actions per plugin, and only now: `available` is not
        // filled in until every plugin has built.
        let carried: Vec<&'static Manifest> = host.available().to_vec();
        for manifest in carried {
            let plugin = manifest.id.clone();

            let state = self.state.clone();
            let chosen = plugin.clone();
            host.register_action(action("show", &plugin), move |_, ctx| {
                state.select(&chosen);
                ctx.notify();
            });

            // Registered only for a plugin that asked for something, so that
            // a name exists exactly where a control does. Every native plugin
            // asks for nothing and gets neither.
            if !manifest.capabilities.is_empty() {
                let keys = wanted(manifest);
                let allowing = plugin.clone();
                host.register_action(action("allow", &plugin), move |workspace, ctx| {
                    // The whole declared list, every time: this is the answer
                    // to a card that showed all of it, and it is also what
                    // prunes a key for something the plugin no longer asks
                    // for.
                    workspace.set_plugin_granted(&allowing, keys.clone(), ctx);
                });

                let revoking = plugin.clone();
                host.register_action(action("revoke", &plugin), move |workspace, ctx| {
                    workspace.set_plugin_granted(&revoking, Vec::new(), ctx);
                });
            }

            if HOLDS_THE_PAGE.contains(&plugin.as_str()) {
                continue;
            }
            host.register_command(
                action("toggle", &plugin),
                format!("Turn the {} plugin on or off", manifest.name),
                move |workspace, ctx| workspace.toggle_plugin(&plugin, ctx),
            );
        }
        Ok(())
    }
}

/// The section: the list in the sidebar, and what it has selected beside it.
///
/// The list is worked out once and handed to both halves, because the card is
/// about *what the list is showing*: a query that filters the chosen plugin
/// out of the list leaves a card describing something nobody can see, so the
/// selection falls to the first row that survived.
///
/// Both halves are drawn in [`section`]'s frame, which is the frame the
/// settings are drawn in: the plugin's name is the page title, so it stays put
/// while the card scrolls, exactly as a settings page's name does.
fn page(
    workspace: &Workspace,
    app: &AppContext,
    state: &Rc<PluginsState>,
) -> (Box<dyn Element>, Box<dyn Element>) {
    let matching = list::matching(workspace);
    let selected = state.showing(&matching);
    let showing = selected.as_ref().and_then(|id| {
        workspace
            .host()
            .available()
            .iter()
            .find(|manifest| manifest.id == *id)
            .copied()
    });

    let list = list::render(workspace, &matching, selected.as_ref());
    // Unreachable while any plugin is loaded, and this page is one.
    let Some(manifest) = showing else {
        return (list, Empty::new().finish());
    };

    (
        list,
        section::content(
            manifest.name,
            card::render(workspace, app, manifest),
            workspace.settings_page().scroll_named(CARD_SCROLL),
            workspace.fonts().ui,
        ),
    )
}

/// What one of this page's per-plugin actions is called.
///
/// `owner/name` has a slash in it and an action name has exactly three parts,
/// so the plugin's own separator becomes a dash: `crook/usage` is toggled by
/// `crook/plugins/toggle-crook-usage` and shown by `…/show-crook-usage`.
pub(super) fn action(verb: &str, plugin: &PluginId) -> ActionName {
    ActionName::parse(&format!(
        "crook/plugins/{verb}-{}-{}",
        plugin.owner(),
        plugin.name()
    ))
    .expect("a name built from a verb and two names that already parsed")
}

/// Where a plugin stands with the person looking at it.
///
/// Three states rather than a yes and a no, because the third is the one worth
/// having: a plugin that was allowed something and now asks for more is
/// neither allowed nor refused, and rounding it to either is exactly what
/// storing a "yes" instead of a list of keys would have forced.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(super) enum Stance {
    /// It asked, and nobody has answered yet. What every plugin installs as,
    /// and what makes asking different from being granted.
    Unanswered,
    /// Everything it asks for has been allowed.
    Allowed,
    /// It asks for something outside what was allowed — a host added to its
    /// list, a path it did not name before, a whole capability that is new.
    Escalated,
}

/// Every key `manifest` asks for, in the order it asked.
///
/// One key per host and per path, which is [`Capability::keys`]'s doing rather
/// than this page's. It is both what a grant is compared against and what
/// "Allow" writes down, because a page that spelled the question one way and
/// the answer another would be a page whose grants stop matching.
pub(super) fn wanted(manifest: &Manifest) -> Vec<String> {
    manifest
        .capabilities
        .iter()
        .flat_map(Capability::keys)
        .collect()
}

/// What `granted` says about what `wanted` asks for.
///
/// A grant holding a key nothing asks for any more is still an allowance and
/// not an escalation: a plugin whose new version dropped a host has taken
/// something back, which is not a thing to interrupt anybody about — and the
/// next "Allow" writes only what is asked for now, so the stale key does not
/// outlive the next answer.
pub(super) fn stance(wanted: &[String], granted: &[String]) -> Stance {
    if granted.is_empty() {
        Stance::Unanswered
    } else if wanted
        .iter()
        .all(|key| granted.iter().any(|had| had == key))
    {
        Stance::Allowed
    } else {
        Stance::Escalated
    }
}

/// Whether every key of one capability is inside `granted`.
///
/// Per capability because a *sentence* is per capability: "Reach a, b" is one
/// line on the card, and one of its two hosts missing makes the whole line
/// something that has not been allowed. Marking it any more finely would mean
/// splitting a sentence somebody has to read.
pub(super) fn covered(capability: &Capability, granted: &[String]) -> bool {
    capability
        .keys()
        .iter()
        .all(|key| granted.iter().any(|had| had == key))
}

/// Why a control the plugin drew itself will not do anything, when it will
/// not.
///
/// The one thing the permission block above cannot say. It explains the
/// mechanism in general — "a plugin is refused everything it has not been
/// allowed" — and a person who has just pressed a button that did nothing is
/// not reading a paragraph about mechanisms four inches away; they are looking
/// at the button. So the sentence is said again, next to the control, and it
/// names the misreading it exists to prevent: the press *was* heard and the
/// answer was no.
///
/// Said from the grant rather than from a refusal that has already happened,
/// which is the whole point of it being here. A plugin cannot say which
/// capability a given control will reach for — only that it asked for some —
/// so a host that waited to be refused could only ever explain the second
/// press. This explains the first.
pub(super) fn stalled(manifest: &Manifest, granted: &[String]) -> Option<&'static str> {
    if manifest.capabilities.is_empty() {
        return None;
    }

    match stance(&wanted(manifest), granted) {
        Stance::Allowed => None,
        Stance::Unanswered => Some(
            "Nothing this plugin asks for has been allowed yet, so a control here that needs it \
             is refused rather than broken. The answer is above.",
        ),
        Stance::Escalated => Some(
            "This plugin asks for more than you allowed, so a control here that needs the rest \
             is refused rather than broken. The answer is above.",
        ),
    }
}

/// The whole list, in one line, for a plugin that drew its own controls.
///
/// Both halves say where to go rather than only how many there are, because a
/// count on its own is a card telling somebody that something exists and not
/// where it is.
pub(super) fn elsewhere(offered: usize, unoffered: usize) -> String {
    let commands = format!(
        "{offered} {}, which the command palette lists",
        plural(offered, "command")
    );

    // "More" only where there is something for them to be more than: a plugin
    // that offers none of what it answers to has a list of actions, not a
    // remainder.
    match (offered, unoffered) {
        (0, _) => format!("{unoffered} {}, {BY_NAME}.", plural(unoffered, "action")),
        (_, 0) => format!("{commands}."),
        _ => format!("{commands}, and {unoffered} more {BY_NAME}."),
    }
}

/// The one place an action nobody titled can actually be reached.
///
/// Not the Keyboard Shortcuts page, which this card used to send people to.
/// That page is built from [`Host::commands`](crate::plugin::Host::commands)
/// and so lists the titled ones only — the very actions these sentences are
/// *not* about. What is true of them is that they are bindable: a name in a
/// `keybindings.json` rule reaches one, and nothing else in the interface
/// will.
const BY_NAME: &str = "reachable by name from your keybindings file";

/// The same fact for a card that did list the commands above it.
pub(super) fn only_by_name(unoffered: usize) -> String {
    format!("and {unoffered} more it does not offer, {BY_NAME}.")
}

/// `word`, made plural by the only rule these two words need.
fn plural(count: usize, word: &str) -> String {
    if count == 1 {
        word.to_owned()
    } else {
        format!("{word}s")
    }
}

/// The keys that drive whatever panel a sandboxed plugin has up.
///
/// Four actions and one claim for the whole tier, rather than four per
/// installed plugin. A panel is modal — opening a second dismisses the first —
/// so there is exactly one thing for a key to act on, and which plugin drew it
/// is a question `wasm::picker` answers rather than one the *names* have to.
/// The alternative was four actions on every plugin's card whether or not it
/// had ever drawn a picker, which is the page saying a plugin offers something
/// the plugin has never heard of.
///
/// They are registered as plain actions rather than commands, so they are not
/// in the palette: a list of things to do whose rows are "move the selection"
/// is a list nobody reads. Bound by name all the same, which is what lets
/// somebody who wants ctrl-n on the selection have it.
fn picker_keys(host: &mut Host) {
    use crate::plugins::wasm::picker;

    for (name, by) in [(picker::NEXT, 1_isize), (picker::PREVIOUS, -1)] {
        host.register_action(named(name), move |_, ctx| {
            if let Some(held) = picker::open() {
                held.move_selection(by);
                ctx.notify();
            }
        });
    }

    host.register_action(named(picker::CHOOSE), |workspace, ctx| {
        // Through the ordinary action path, which is what makes Enter and a
        // click on the row the same gesture: both say what was chosen and then
        // run the plugin's own action.
        let Some((action, key)) = picker::open().and_then(|held| {
            let chosen = held.chosen()?;
            held.say(chosen.1.clone());
            Some(chosen)
        }) else {
            return;
        };
        let _ = key;
        workspace.run_action(action, ctx);
    });

    host.register_action(named(picker::CLOSE), |workspace, ctx| {
        // The host lets go of the keyboard, and the plugin is told its panel
        // was dismissed — in that order, so a plugin that opens something of
        // its own out of `dismiss` is not opening it into a keyboard this
        // still owns.
        let Some(held) = picker::open() else {
            return;
        };
        let dismiss = held.dismissal();
        held.shut();
        workspace.sync_input_keys();
        if let Some(action) = dismiss {
            workspace.run_action(action, ctx);
        }
        ctx.notify();
    });

    // Claimed as a *panel* rather than as a surface: what a plugin puts up
    // hangs off a chip in a place, and a place can stop being drawn. See
    // `Host::claim_panel`.
    picker::armed_by(host.claim_panel(picker::claims, picker::shut_open));
}

/// One of the tier's own action names, which are literals.
fn named(name: &str) -> ActionName {
    ActionName::parse(name).expect("a literal that parses")
}

/// Built once and leaked; see `header::manifest`.
fn manifest() -> &'static Manifest {
    static MANIFEST: std::sync::OnceLock<Manifest> = std::sync::OnceLock::new();
    MANIFEST.get_or_init(|| Manifest {
        schema: Manifest::SCHEMA,
        id: PluginId::parse("crook/plugins").expect("a literal that parses"),
        name: "Plugins",
        description: "What this build is made of, and what did not load.",
        version: env!("CARGO_PKG_VERSION"),
        tier: Tier::Native,
        capabilities: &[],
    })
}

/// The word a person would search for to find a tier, and what a card prints.
pub(super) fn tier_words(tier: Tier) -> (&'static str, &'static str) {
    match tier {
        Tier::Native => ("built-in", "Built in"),
        Tier::Wasm => ("sandboxed", "Installed, sandboxed"),
        Tier::Process => ("external", "Installed, runs as a program"),
    }
}

#[cfg(test)]
mod tests;

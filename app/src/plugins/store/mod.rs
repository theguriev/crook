//! The registry, and what this machine knows about it.
//!
//! A plugin arrives as a file, and until now finding that file was somebody
//! else's problem: a README, a release page, a link in a chat. The registry is
//! the list — one JSON index, published as a static file, listing every plugin
//! it has built from source with the ABI it speaks, where the artifact is and
//! what it hashes to.
//!
//! # What is here
//!
//! Three halves with no window in them — [`index`], what a registry publishes
//! and which of it this build can run; [`cache`], the copy on disk that makes
//! the list openable offline and is the only record of what has been
//! withdrawn; and [`fetch`], the one request Crook makes of its own, written
//! under the rule the README states about telemetry — and three with: `model`,
//! which does the two slow things off the thread that draws; `section`, the
//! list and the card; and `state`, what the list has selected.
//!
//! What is *not* here is the deciding. A module that arrives is checked,
//! written and carried by the workspace, by the same code that does it for a
//! module somebody copied in by hand, and what a plugin may then do is
//! answered on its card in Plugins like every other plugin's.
//!
//! # What the rest of the window hears
//!
//! The Plugins page says on a plugin's own card that the registry is ahead
//! of it, and offers Update there. It has no handle to the model and reads
//! no file on the render path: the store's observer hands the workspace a
//! [`Heard`](index::Heard) whenever the model's answer changes, and the
//! card's Update and Show in Store are this plugin's own actions, run *about*
//! a plugin — `crook/store/update`, `crook/store/show` — looked up by name
//! there, and drawn dead with a line while this plugin is switched off.

pub mod cache;
pub mod fetch;
pub mod index;
pub mod model;
mod section;
mod state;

use std::rc::Rc;
use std::sync::OnceLock;

use crookui_core::icons::Lucide;
use crookui_core::prelude::*;

use crook_plugin::{ActionName, Manifest, PluginId, Tier};

use crate::editor::Editor;
use crate::plugin::{BuildError, Host, Plugin};
use crate::workspace::Workspace;

use cache::Cache;
use index::change;
use model::StoreModel;
pub(crate) use state::StoreState;

/// The section this store is the sidebar of, and the name its field is
/// registered under.
pub(super) const SECTION: &str = "crook/store/section";
/// See [`SECTION`].
pub(super) const FIELD: &str = "search";

/// The plugin that offers the other plugins.
pub struct Store {
    /// What the section has selected and where its answers come from, shared
    /// with the closures that draw it and the handlers that act on it.
    state: Rc<StoreState>,
    /// Where the copy of the index is, or `None` on a machine with nowhere
    /// to keep one. Kept rather than taken: a plugin is built again every
    /// time it is switched back on, and each build makes a model of its own.
    cache: Option<Cache>,
    /// How a test answers a module fetch, instead of the network.
    #[cfg(test)]
    fetch: Option<model::Fetcher>,
}

impl Store {
    /// One, knowing nothing yet, reading the copy of the index this machine
    /// keeps.
    pub fn new() -> Self {
        Self::with_cache(Cache::user())
    }

    /// One reading `cache`, which is how a test gives a window a store that
    /// reads a scratch directory rather than the real list of whoever is
    /// running the tests.
    pub fn with_cache(cache: Option<Cache>) -> Self {
        Self {
            state: Rc::new(StoreState::new()),
            cache,
            #[cfg(test)]
            fetch: None,
        }
    }

    /// The same, answering every module fetch with `fetch`.
    #[cfg(test)]
    pub(crate) fn fetching(mut self, fetch: model::Fetcher) -> Self {
        self.fetch = Some(fetch);
        self
    }

    /// What the section shares with its handlers, for a test to read the
    /// model through once the window is up.
    #[cfg(test)]
    pub(crate) fn state(&self) -> Rc<StoreState> {
        self.state.clone()
    }
}

impl Default for Store {
    fn default() -> Self {
        Self::new()
    }
}

/// Replaces the store among `plugins` with one reading `cache` and answering
/// every module fetch with `fetch`, and hands back its state — or `None`
/// when there is no store among them.
///
/// For every window a test opens, whichever harness opens it: the store in
/// the box reads the real list of whoever is running the tests, and decodes
/// its faces on the pool, and the first time it spoke the window would hear
/// their offers in place of what the test said.
#[cfg(test)]
pub(crate) fn hermetic(
    plugins: &mut [Box<dyn Plugin>],
    cache: Option<Cache>,
    fetch: model::Fetcher,
) -> Option<Rc<StoreState>> {
    let at = plugins
        .iter()
        .position(|plugin| plugin.manifest().id.as_str() == "crook/store")?;
    let store = Store::with_cache(cache).fetching(fetch);
    let state = store.state();
    plugins[at] = Box::new(store);
    Some(state)
}

impl Plugin for Store {
    fn manifest(&self) -> &'static Manifest {
        manifest()
    }

    fn mark(&self) -> Option<Lucide> {
        Some(Lucide::Store)
    }

    fn build(
        &mut self,
        host: &mut Host,
        ctx: &mut ViewContext<Workspace>,
    ) -> Result<(), BuildError> {
        let cache = self.cache.clone();
        let model = ctx.add_model(|_| StoreModel::new(cache));
        #[cfg(test)]
        if let Some(fetch) = self.fetch.clone() {
            model.update(ctx, |model, _| model.fetch_with(fetch));
        }
        self.state.attach(model.clone());
        // The faces on the rows, off the thread that is about to draw them.
        // Nothing is announced here: what the window heard at its opening
        // was read off the same cache, and the observer below hands over
        // what changes.
        model.update(ctx, |model, ctx| model.remember_icons(ctx));

        // The bridge a model-backed feature needs, and here it carries the one
        // thing a model cannot do: a module that has finished downloading has
        // to be checked, written and *carried* by the host, and all three of
        // those are the workspace's. The same shape a sandboxed plugin's
        // requests are drained in, for the same reason.
        ctx.observe(&model, |workspace, model, ctx| {
            for (plugin, release, arrived) in model.update(ctx, |model, _| model.landed()) {
                let bytes = match arrived {
                    Ok(bytes) => bytes,
                    Err(why) => {
                        model.update(ctx, |model, ctx| {
                            model.complain(Some(&plugin), model::did_not_arrive(&why), ctx);
                        });
                        continue;
                    }
                };

                let named = plugin.clone();
                let installed = workspace.install_plugin(
                    &bytes,
                    move |manifest| {
                        if manifest.id != named {
                            return Err(format!(
                                "the list offered {named} and the module says it is {}",
                                manifest.id
                            ));
                        }
                        index::promised(&release, manifest)
                    },
                    ctx,
                );

                // What the registry says about the version that just landed,
                // which is not what it said about the one it replaced: a card
                // that went on saying "withdrawn" after an update out of a
                // yank would be describing a plugin nobody has any more.
                let mut withdrawn = None;
                if let Ok(manifest) = &installed {
                    withdrawn = model.read(ctx, |model, _| {
                        model
                            .index()
                            .and_then(|index| index::withdrawn(index, &plugin, manifest.version))
                    });
                    workspace.withdrew(&plugin, withdrawn.clone());

                    // And it does not run. The store does not *offer* a
                    // withdrawn version, so this is the list having changed
                    // under somebody between the look and the press — rare,
                    // and the one case where "installed" and "withdrawn" would
                    // otherwise both be true of a plugin that is drawing.
                    if withdrawn.is_some() {
                        workspace.disable_plugin(&manifest.id, ctx);
                    }
                }

                if let (Ok(manifest), Some(why)) = (&installed, &withdrawn) {
                    let (named, why) = (manifest.id.clone(), why.clone());
                    model.update(ctx, |model, ctx| {
                        model.complain(
                            Some(&named),
                            format!("was withdrawn while you were reading it: {why}"),
                            ctx,
                        );
                    });
                    continue;
                }

                model.update(ctx, |model, ctx| match installed {
                    Ok(manifest) => model.say(
                        Some(&plugin),
                        format!(
                            "{} {} is installed, and may do nothing at all until you answer its \
                             card in Plugins.",
                            manifest.name, manifest.version
                        ),
                        ctx,
                    ),
                    Err(why) => {
                        model.complain(Some(&plugin), format!("was not installed: {why}"), ctx)
                    }
                });
            }

            // Whatever changed, the rest of the window hears the answer as
            // it now stands: the offers after a look, and which plugins are
            // being fetched or waiting to be. The Plugins page draws from
            // this and from nothing the store holds.
            let heard = model.read(ctx, |model, _| model.heard());
            workspace.hear(heard);
            ctx.notify();
        });

        let state = self.state.clone();
        host.add_sidebar_section(
            "section",
            "Store",
            // A shop front. `Plus` stood here while the icon set had nothing
            // closer — "the button that adds a plugin" — and it read as a
            // second "new tab" at the foot of the sidebar.
            Lucide::Store,
            // After Plugins, which is where what a person already has belongs:
            // the list of what is installed is read far more often than the
            // list of what could be.
            20,
            move |workspace, app| section::render(workspace, app, &state),
        );

        // A command, unlike the two below it: looking is asking a static file
        // for a list and it is the one thing here somebody might want a chord
        // for. Installing and removing are not — a palette row that installed
        // something would be an install nobody saw the capability list of,
        // which is exactly the door the Plugins page refuses to open for
        // allowing. And they are *answers*, which shuts the same door on a
        // sandboxed plugin: granted `run:crook/store/install`, it could
        // otherwise fetch and carry a module nobody chose.
        let looking = model.clone();
        host.register_command(
            action("look"),
            "Look for plugins to install",
            move |_, ctx| {
                looking.update(ctx, |model, ctx| model.look(ctx));
            },
        );

        let installing = self.state.clone();
        host.register_answer(action("install"), move |workspace, ctx| {
            install(&installing, workspace, ctx);
        });

        let removing = self.state.clone();
        host.register_answer(action("remove"), move |workspace, ctx| {
            remove(&removing, workspace, ctx);
        });

        // The four the Plugins page reaches this plugin by, each about a
        // plugin the press names — a card there has a row for "the registry
        // is ahead" and a button for it, and the fetching is this plugin's.
        // `update-all` is about nothing in particular: it takes what the
        // workspace says is behind. None of the four belongs in a palette,
        // for the reason `install` does not; and the three that fetch are
        // answers, for the reason `install` is one. `show` is not: turning
        // the page to a row is nothing a person would mind a plugin doing.
        let showing = self.state.clone();
        host.register_action(action("show"), move |workspace, ctx| {
            let Some(plugin) = workspace.host().said_plugin(&action("show")) else {
                return;
            };
            show(&showing, workspace, &plugin, ctx);
        });

        let updating = self.state.clone();
        host.register_answer(action("update"), move |workspace, ctx| {
            let Some(plugin) = workspace.host().said_plugin(&action("update")) else {
                return;
            };
            update(&updating, workspace, &plugin, ctx);
        });

        let everything = self.state.clone();
        host.register_answer(action("update-all"), move |workspace, ctx| {
            let Some(model) = everything.model() else {
                return;
            };
            let wanted = workspace.updates();
            model.update(ctx, |model, ctx| model.update_all(wanted, ctx));
        });

        let looking = self.state.clone();
        host.register_answer(action("look-inside"), move |workspace, ctx| {
            let Some(plugin) = workspace.host().said_plugin(&action("look-inside")) else {
                return;
            };
            look_inside(&looking, workspace, &plugin, ctx);
        });

        Ok(())
    }
}

/// Turns the card to `plugin`, and the sidebar to the store if it is showing
/// something else.
///
/// Selected by id rather than through the field: a person pressing "Show in
/// Store" on a plugin's card is asking for that plugin's row, whatever was
/// typed into the store's field last. So the field is emptied — the card is
/// resolved against the rows the field lets through, and a row it hides is
/// a card about the first row it does not. Leaving a section empties its
/// field already; this is the store being asked for a row while it is
/// showing, which a plugin allowed to run this action can do.
fn show(
    state: &Rc<StoreState>,
    workspace: &mut Workspace,
    plugin: &PluginId,
    ctx: &mut ViewContext<Workspace>,
) {
    let (_, field) = workspace.field(SECTION, FIELD);
    field.edit(Editor::clear);
    state.select(plugin.as_str());
    let section = workspace.host().sidebar_section_id(SECTION);
    if section.is_some() && workspace.showing_section() != section {
        workspace.show_section(section, ctx);
    }
    ctx.notify();
}

/// Fetches the release the registry offers in place of what `plugin` is
/// running — and only when there is one.
///
/// Checked at the press rather than trusted to the button: this is reachable
/// by name, and "update" run about a plugin the registry is not ahead of
/// would otherwise download the version already here, or one older than it.
fn update(
    state: &Rc<StoreState>,
    workspace: &Workspace,
    plugin: &PluginId,
    ctx: &mut ViewContext<Workspace>,
) {
    let Some(model) = state.model() else {
        return;
    };
    let heard = workspace.heard();
    let Some(offer) = heard.offer(plugin) else {
        log::warn!("crook/store/update was run about {plugin}, which the registry does not list");
        return;
    };
    let installed = section::installed_version(workspace, plugin);
    let withdrawn = workspace.withdrawn(plugin).is_some();
    match change(offer, installed.as_deref(), withdrawn).fetchable() {
        Some(release) => {
            model.update(ctx, |model, ctx| model.download(plugin, release, ctx));
        }
        None => {
            log::warn!(
                "crook/store/update was run about {plugin}, which the registry is not ahead of"
            );
        }
    }
}

/// Decodes the pictures inside `plugin`'s module, fetching the module when
/// it is not on this machine.
///
/// The module already here is preferred when it carries any: its pictures
/// are what is installed, and they cost no request. Else the offered
/// release is fetched, which is the one gesture in the store that downloads
/// something without installing it — the card says so beside the button.
fn look_inside(
    state: &Rc<StoreState>,
    workspace: &Workspace,
    plugin: &PluginId,
    ctx: &mut ViewContext<Workspace>,
) {
    let Some(model) = state.model() else {
        return;
    };
    let carried = workspace
        .host()
        .pictures_of(plugin)
        .filter(|pictures| pictures.count() > 0)
        .map(|pictures| pictures.previews.clone());
    let release = workspace
        .heard()
        .offer(plugin)
        .and_then(|offer| offer.release.clone());
    model.update(ctx, |model, ctx| {
        model.look_inside(plugin, release.as_ref(), carried, ctx);
    });
}

/// Downloads whatever the card is about.
fn install(state: &Rc<StoreState>, workspace: &Workspace, ctx: &mut ViewContext<Workspace>) {
    let Some(model) = state.model() else {
        return;
    };
    let offers = model.update(ctx, |model, _| model.offers());
    // Resolved the way the section resolves it, which means through the field
    // above the list: a handler that asked the *unfiltered* list which row is
    // showing would answer with whatever is first in that one, and install a
    // plugin whose card nobody read.
    let Some(offer) = section::chosen(workspace, state, &offers) else {
        return;
    };
    let chosen = offer.id.clone();
    let Some(release) = offer.release.clone() else {
        model.update(ctx, |model, ctx| {
            model.complain(
                Some(&offer.id),
                String::from("has nothing built for the plugin API this Crook speaks"),
                ctx,
            );
        });
        return;
    };

    model.update(ctx, |model, ctx| model.download(&chosen, &release, ctx));
}

/// Takes whatever the card is about off this machine.
fn remove(state: &Rc<StoreState>, workspace: &mut Workspace, ctx: &mut ViewContext<Workspace>) {
    let Some(model) = state.model() else {
        return;
    };
    let offers = model.update(ctx, |model, _| model.offers());
    let Some(chosen) = section::chosen(workspace, state, &offers).map(|offer| offer.id) else {
        return;
    };

    let outcome = workspace.remove_plugin(&chosen, ctx);
    model.update(ctx, |model, ctx| match outcome {
        Ok(()) => model.say(
            Some(&chosen),
            String::from("is off this machine, and so is what it was allowed to do."),
            ctx,
        ),
        Err(why) => model.complain(Some(&chosen), format!("was not removed: {why}"), ctx),
    });
}

/// What one of this plugin's actions is called.
///
/// `crook/store/look`, `install`, `remove`, `show`, `update`, `update-all`,
/// `look-inside`. Reachable from the Plugins page, which offers Update and
/// Show in Store on a plugin's own card and looks the store's actions up by
/// name to do it: a store switched off is a row drawn dead there, not a
/// missing button.
pub(crate) fn action(verb: &str) -> ActionName {
    ActionName::parse(&format!("crook/store/{verb}")).expect("a name built from a literal")
}

/// What this plugin says it is.
fn manifest() -> &'static Manifest {
    static MANIFEST: OnceLock<Manifest> = OnceLock::new();
    MANIFEST.get_or_init(|| Manifest {
        schema: Manifest::SCHEMA,
        id: PluginId::parse("crook/store").expect("a literal that parses"),
        name: "Store",
        description: "The list of plugins the registry has built, and the one request Crook \
                      makes of its own.",
        version: env!("CARGO_PKG_VERSION"),
        tier: Tier::Native,
        capabilities: &[],
    })
}

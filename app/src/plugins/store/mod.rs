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

use crate::plugin::{BuildError, Host, Plugin};
use crate::workspace::Workspace;

use cache::Cache;
use model::StoreModel;
use state::StoreState;

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
}

impl Store {
    /// One, knowing nothing yet.
    pub fn new() -> Self {
        Self {
            state: Rc::new(StoreState::new()),
        }
    }
}

impl Default for Store {
    fn default() -> Self {
        Self::new()
    }
}

impl Plugin for Store {
    fn manifest(&self) -> &'static Manifest {
        manifest()
    }

    fn build(
        &mut self,
        host: &mut Host,
        ctx: &mut ViewContext<Workspace>,
    ) -> Result<(), BuildError> {
        let model = ctx.add_model(|_| StoreModel::new(Cache::user()));
        self.state.attach(model.clone());

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
                            model.complain(Some(&plugin), format!("did not arrive: {why}"), ctx);
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
                if let Ok(manifest) = &installed {
                    let why = model.read(ctx, |model, _| {
                        model
                            .index()
                            .and_then(|index| index::withdrawn(index, &plugin, manifest.version))
                    });
                    workspace.withdrew(&plugin, why);
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
            ctx.notify();
        });

        let state = self.state.clone();
        host.add_sidebar_section(
            "section",
            "Store",
            // Not `LayoutGrid`, which is the sessions button, and not `Blocks`,
            // which is the plugins one. `Plus` is what is left that means
            // anything here, and what it means is right: this is the button
            // that adds a plugin.
            Lucide::Plus,
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
        // allowing.
        let looking = model.clone();
        host.register_command(
            action("look"),
            "Look for plugins to install",
            move |_, ctx| {
                looking.update(ctx, |model, ctx| model.look(ctx));
            },
        );

        let installing = self.state.clone();
        host.register_action(action("install"), move |workspace, ctx| {
            install(&installing, workspace, ctx);
        });

        let removing = self.state.clone();
        host.register_action(action("remove"), move |workspace, ctx| {
            remove(&removing, workspace, ctx);
        });

        Ok(())
    }
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
pub(super) fn action(verb: &str) -> ActionName {
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

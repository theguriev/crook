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
//! The three halves that have no window in them: [`index`], what a registry
//! publishes and which of it this build can run; [`cache`], the copy on disk
//! that makes the list openable offline and is the only record of what has
//! been withdrawn; and [`fetch`], the one request Crook makes of its own,
//! written under the rule the README states about telemetry.
//!
//! Nothing here draws anything, and nothing here installs anything: what a
//! plugin may do is decided against the module that was downloaded, by the
//! same code that decides it for a module somebody copied in by hand.

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
                            model.complain(format!("{plugin} did not arrive: {why}"), ctx);
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

                model.update(ctx, |model, ctx| match installed {
                    Ok(manifest) => model.say(
                        format!(
                            "{} {} is installed, and may do nothing at all until you answer its \
                             card in Plugins.",
                            manifest.name, manifest.version
                        ),
                        ctx,
                    ),
                    Err(why) => model.complain(format!("{plugin} was not installed: {why}"), ctx),
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
    let Some(chosen) = state.showing(&offers) else {
        return;
    };
    let Some(offer) = offers.iter().find(|offer| offer.id == chosen) else {
        return;
    };
    let Some(release) = offer.release.clone() else {
        model.update(ctx, |model, ctx| {
            model.complain(
                format!("{chosen} has nothing built for the plugin API this Crook speaks"),
                ctx,
            );
        });
        return;
    };

    // Nothing about the workspace is touched here: what a download becomes is
    // decided when it lands, by the observer that has the workspace to install
    // it with.
    let _ = workspace;
    model.update(ctx, |model, ctx| model.download(&chosen, &release, ctx));
}

/// Takes whatever the card is about off this machine.
fn remove(state: &Rc<StoreState>, workspace: &mut Workspace, ctx: &mut ViewContext<Workspace>) {
    let Some(model) = state.model() else {
        return;
    };
    let offers = model.update(ctx, |model, _| model.offers());
    let Some(chosen) = state.showing(&offers) else {
        return;
    };

    let outcome = workspace.remove_plugin(&chosen, ctx);
    model.update(ctx, |model, ctx| match outcome {
        Ok(()) => model.say(
            format!("{chosen} is off this machine, and so is what it was allowed to do."),
            ctx,
        ),
        Err(why) => model.complain(format!("{chosen} was not removed: {why}"), ctx),
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

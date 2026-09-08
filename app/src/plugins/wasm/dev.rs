//! The plugin somebody is writing, reloaded as they write it.
//!
//! ```sh
//! crook --dev-plugin target/wasm32-unknown-unknown/release/hello.wasm
//! ```
//!
//! A store exists because a plugin is easy to get. A plugin is only easy to
//! *write* if the loop is short, and without this the loop is: build, copy the
//! module into a directory whose path is different on three platforms, close
//! the window, open it again, look. Raycast and Obsidian both have stores, and
//! both have this, and the order those arrived in is not an accident.
//!
//! # It is not installed, and that is the feature
//!
//! Nothing is copied anywhere. The module runs from wherever `cargo build` put
//! it, so the next build *is* the next version — and when Crook closes, the
//! machine is exactly as it was. A plugin being written is not a plugin
//! somebody chose to have.
//!
//! # It polls, because a watcher is a dependency
//!
//! Once a second, off the thread that draws, comparing the length and the
//! modified time — which is what a rebuild changes and what a text editor
//! saving a source file does not. `notify` would be a crate, three platform
//! backends and a thread; this is eight lines and a `metadata` call, and the
//! thing it is watching is one file that changes when a person runs `cargo
//! build`.
//!
//! The one thing it costs is a parked worker for as long as the flag is in
//! effect, which is why [`crate::PARKED_WORKERS`] says what it says: this
//! chain exists only when somebody asked for it on the command line.
//!
//! # A build that fails is not a window that breaks
//!
//! Half a file, a module that does not instantiate, a plugin that panics in
//! `build` — each of those is a line and the *previous* version still running,
//! because that is what somebody in the middle of writing a plugin needs. The
//! next good build replaces it.

use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::{Duration, SystemTime};

use crookui_core::prelude::*;

use crook_plugin::{Manifest, PluginId, Tier};

use crate::plugin::{BuildError, Host, Plugin};
use crate::workspace::Workspace;

/// How often the file is looked at.
///
/// A second: fast enough that a rebuild appears while somebody is still
/// looking at the window, slow enough that it is a `stat` a second on one
/// file. The interval a person actually waits is this plus however long
/// `cargo build` took, which is the part worth optimising and not this one.
const INTERVAL: Duration = Duration::from_secs(1);

/// The plugin that carries the plugin being written.
pub struct Dev {
    /// The module to watch. Resolved before the window opens, so a path that
    /// is not one is a line on the command line rather than a silent nothing.
    path: PathBuf,
    /// What the module looked like when it was loaded on the way in, if it
    /// was.
    ///
    /// The first version is loaded *before* the window opens, beside the
    /// plugins in the box, so that it is there on the first frame — a plugin
    /// that appeared a second after the window did would be a plugin nobody
    /// could take a picture of, and a second of every launch spent watching
    /// for it. This is what stops the first poll then loading it again.
    seen: Option<(u64, SystemTime)>,
    /// The watch, kept here for as long as this plugin is carried.
    ///
    /// Not filing: a model lives as long as somebody holds a handle to it, and
    /// a chain spawned by a model nobody holds lands on a handle that no
    /// longer upgrades — which is not an error, it is *nothing*, once a
    /// second, forever. `build` cannot hold it, because `build` returns; the
    /// plugin object is what the host keeps.
    watch: Option<ModelHandle<Watch>>,
    /// Which plugin the last build turned out to be.
    ///
    /// A module's id is in its manifest, which is a line somebody edits like
    /// any other — and a build that changes it is two plugins as far as the
    /// host is concerned: `carry` replaces by id, so the one that was there
    /// under the old name would stay loaded, drawing, and impossible to get
    /// rid of without closing the window.
    carried: Option<PluginId>,
}

impl Dev {
    /// One, watching `path`, which has already been loaded if `seen` says so.
    pub fn watching(
        path: PathBuf,
        seen: Option<(u64, SystemTime)>,
        carried: Option<PluginId>,
    ) -> Self {
        Self {
            path,
            seen,
            carried,
            watch: None,
        }
    }

    /// What a module looks like now, for whoever loaded it.
    pub fn about(path: &Path) -> Option<(u64, SystemTime)> {
        let about = std::fs::metadata(path).ok()?;
        Some((about.len(), about.modified().ok()?))
    }

    /// The module a `--dev-plugin` argument names.
    ///
    /// A file is itself. A directory is where `cargo build` puts things, so a
    /// directory is searched for the one module under
    /// `target/wasm32-unknown-unknown/release/` — the path a plugin's own
    /// README tells somebody to build to, and the one nobody should have to
    /// type twice a minute.
    pub fn module(path: &Path) -> Result<PathBuf, String> {
        if path.is_file() {
            return Ok(path.to_path_buf());
        }
        if !path.is_dir() {
            return Err(String::from("is neither a module nor a directory"));
        }

        let built = path.join("target/wasm32-unknown-unknown/release");
        let mut modules: Vec<PathBuf> = std::fs::read_dir(&built)
            .map_err(|why| format!("has nothing built in it: {} {why}", built.display()))?
            .flatten()
            .map(|entry| entry.path())
            .filter(|path| path.extension().is_some_and(|end| end == "wasm"))
            .collect();
        modules.sort();

        match modules.len() {
            1 => Ok(modules.remove(0)),
            0 => Err(format!("has no module in {}", built.display())),
            more => Err(format!(
                "has {more} modules in {}, so name the one you mean",
                built.display()
            )),
        }
    }
}

impl Plugin for Dev {
    fn manifest(&self) -> &'static Manifest {
        manifest()
    }

    fn build(&mut self, _: &mut Host, ctx: &mut ViewContext<Workspace>) -> Result<(), BuildError> {
        let watch = ctx.add_model(|_| Watch::new(self.path.clone(), self.seen));

        // The bridge every model-backed feature has, carrying the one thing a
        // model cannot do: running a module needs the host, and the host is
        // the workspace's.
        let carried = Rc::new(RefCell::new(self.carried.clone()));
        let mine = manifest().id.clone();
        ctx.observe(&watch, move |workspace, watch, ctx| {
            let Some(bytes) = watch.update(ctx, |watch, _| watch.rebuilt()) else {
                return;
            };

            // Switched off, and this is the one registration a switch cannot
            // take back: an observer is the workspace's rather than the
            // host's. A plugin that was turned off must not go on putting
            // modules into the window.
            if !workspace.host().is_loaded(&mine) {
                return;
            }

            match workspace.run_dev_plugin(&bytes, ctx) {
                Ok(manifest) => {
                    // A build that renamed itself is two plugins to the host,
                    // and the one that was there is not replaced by the one
                    // that arrived — so it is taken out, or it stays drawing
                    // under a name the source no longer has.
                    let was = carried.replace(Some(manifest.id.clone()));
                    if let Some(was) = was.filter(|was| *was != manifest.id) {
                        log::info!("{was} was renamed to {}, and is gone", manifest.id);
                        workspace.forget_plugin(&was, ctx);
                    }
                    log::info!("{} {} reloaded", manifest.id, manifest.version);
                }
                // A line rather than anything on screen: somebody watching a
                // window they are writing a plugin for is watching a terminal,
                // and the terminal they are watching is the one printing this.
                Err(why) => log::warn!("the plugin being written did not load: {why}"),
            }
            ctx.notify();
        });

        // After the model exists, for the reason `Watch::new` gives.
        watch.update(ctx, |watch, ctx| watch.start(ctx));
        self.watch = Some(watch);

        Ok(())
    }
}

/// What the file looked like last time, and the chain that keeps looking.
struct Watch {
    path: PathBuf,
    /// The length and the modified time of the module as it was last read.
    ///
    /// Both, because either alone misses a rebuild: a module that compiles to
    /// the same length is the ordinary case for a one-character change, and a
    /// filesystem whose timestamps have a second's resolution is every
    /// filesystem this could run on.
    seen: Option<(u64, SystemTime)>,
    /// The bytes of a build the observer has not taken yet.
    rebuilt: Option<Vec<u8>>,
}

impl Entity for Watch {
    type Event = ();
}

impl Watch {
    /// A watch that is not watching yet.
    ///
    /// Nothing is spawned here, and that is not a style choice: a task started
    /// from inside `add_model`'s closure holds a handle to a model the app has
    /// not finished registering, and what it does when it lands is nothing at
    /// all — silently, once a second, forever. `GitModel` says the same thing
    /// about the same trap. [`Self::start`] is called once the model exists.
    fn new(path: PathBuf, seen: Option<(u64, SystemTime)>) -> Self {
        Self {
            path,
            seen,
            rebuilt: None,
        }
    }

    /// Begins looking.
    fn start(&mut self, ctx: &mut ModelContext<Self>) {
        self.look(ctx);
    }

    /// What the last build produced, taken once.
    fn rebuilt(&mut self) -> Option<Vec<u8>> {
        self.rebuilt.take()
    }

    /// Reads the file if it has changed, and comes back in a second.
    fn look(&mut self, ctx: &mut ModelContext<Self>) {
        let path = self.path.clone();
        let seen = self.seen;
        let background = ctx.background().clone();

        // The wait and the read are one background task, which is the shape
        // every polling chain here has: the worker is held across the wait
        // rather than handed back and asked for again.
        ctx.spawn(
            async move {
                background
                    .spawn(async move {
                        std::thread::sleep(INTERVAL);
                        let about = std::fs::metadata(&path).ok()?;
                        let now = (about.len(), about.modified().ok()?);
                        if Some(now) == seen {
                            return None;
                        }
                        Some((now, std::fs::read(&path).ok()?))
                    })
                    .await
            },
            |watch, found, ctx| {
                if let Some((now, bytes)) = found {
                    watch.seen = Some(now);
                    watch.rebuilt = Some(bytes);
                    ctx.notify();
                }
                watch.look(ctx);
            },
        )
        .detach();
    }
}

/// What this plugin says it is.
fn manifest() -> &'static Manifest {
    static MANIFEST: std::sync::OnceLock<Manifest> = std::sync::OnceLock::new();
    MANIFEST.get_or_init(|| Manifest {
        schema: Manifest::SCHEMA,
        id: PluginId::parse("crook/dev").expect("a literal that parses"),
        name: "Dev plugin",
        description: "Runs the plugin you are writing, and again every time you build it.",
        version: env!("CARGO_PKG_VERSION"),
        tier: Tier::Native,
        capabilities: &[],
    })
}

#[cfg(test)]
#[path = "dev_tests.rs"]
mod tests;

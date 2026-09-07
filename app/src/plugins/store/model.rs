//! What the store knows, and the two things it does off the drawing thread.
//!
//! A model rather than a field on the plugin, because both of the things it
//! does — reading an index over the network, and downloading a module — are
//! blocking work that must not happen inside `render`, and because what
//! arrives has to be able to repaint the window when it lands.
//!
//! # Nothing here starts by itself
//!
//! There is no timer, no poll and no check on launch. [`StoreModel::look`] is
//! reached by a press and by nothing else, which is the whole of how the
//! README's sentence about telemetry survives a feature that talks to a
//! server: a terminal that phones home every launch is a terminal that has to
//! be *trusted* about what it said, and one that never does is a terminal that
//! does not have to be.
//!
//! What it does do at startup is read the copy already on disk, which is a few
//! kilobytes and no network at all — so the list is there, with its age
//! written on it, on a machine that has never been online since.
//!
//! # A download lands here and is installed by somebody else
//!
//! Installing needs the workspace: the module has to be checked, written, and
//! then *carried* by the host so that it runs without a restart, and a model
//! can reach none of those. So a finished download is left in [`landed`] and
//! the section's observer drains it, exactly as a sandboxed plugin's requests
//! are drained by the observer in `plugins::wasm`.
//!
//! [`landed`]: StoreModel::landed

use std::time::SystemTime;

use crookui_core::prelude::*;

use crook_plugin::PluginId;

use super::cache::Cache;
use super::fetch::{self, Fetched};
use super::index::{Index, Offer, Release, offers};

/// Everything this machine knows about the registry.
pub struct StoreModel {
    /// The index, from the disk or from the last look.
    known: Option<Index>,
    /// What the registry called this version of it.
    etag: Option<String>,
    /// When the copy on disk was written, for a page that has to say how old
    /// its answer is.
    fetched: Option<SystemTime>,
    /// Whether a look is in flight.
    looking: bool,
    /// Which plugin is being downloaded, if any.
    ///
    /// One at a time: two downloads at once would be two workers held on a
    /// pool sized for the chains that park on it, for no gain a person could
    /// see — the second press is a second later.
    downloading: Option<PluginId>,
    /// What went wrong last, for the page to say out loud.
    problem: Option<String>,
    /// Downloads that have arrived and need a workspace to install them,
    /// each with the release it was asked for — which is what the module that
    /// arrived has to be checked against.
    landed: Vec<(PluginId, Release, Result<Vec<u8>, String>)>,
    /// What the last install or removal came to.
    said: Option<String>,
}

impl Entity for StoreModel {
    type Event = ();
}

impl StoreModel {
    /// A store that knows whatever is already on disk, and nothing else.
    ///
    /// The cache is handed in rather than found here, so that a test can hand
    /// it a directory of its own — a model that read the person's real one
    /// would be a test whose answer depends on what they installed.
    pub fn new(cache: Option<Cache>) -> Self {
        let cached = cache.and_then(|cache| cache.read());
        Self {
            known: cached.as_ref().map(|cached| cached.index.clone()),
            etag: cached.as_ref().and_then(|cached| cached.etag.clone()),
            fetched: cached.as_ref().and_then(|cached| cached.fetched),
            looking: false,
            downloading: None,
            problem: None,
            landed: Vec::new(),
            said: None,
        }
    }

    /// Every plugin the registry has, as this build can offer them.
    pub fn offers(&self) -> Vec<Offer> {
        self.known.as_ref().map(offers).unwrap_or_default()
    }

    /// The index itself, for the questions an offer does not answer — whether
    /// a version somebody is running has been withdrawn, and why.
    pub fn index(&self) -> Option<&Index> {
        self.known.as_ref()
    }

    /// When the copy this is answering from was written.
    pub fn fetched(&self) -> Option<SystemTime> {
        self.fetched
    }

    /// Whether a look is in flight.
    pub fn looking(&self) -> bool {
        self.looking
    }

    /// Which plugin is being downloaded, if any.
    pub fn downloading(&self) -> Option<&PluginId> {
        self.downloading.as_ref()
    }

    /// What went wrong, if the last thing that happened went wrong.
    pub fn problem(&self) -> Option<&str> {
        self.problem.as_deref()
    }

    /// What the last install or removal came to.
    pub fn said(&self) -> Option<&str> {
        self.said.as_deref()
    }

    /// Says something on the page, and clears whatever went wrong before it.
    pub fn say(&mut self, said: impl Into<String>, ctx: &mut ModelContext<Self>) {
        self.said = Some(said.into());
        self.problem = None;
        ctx.notify();
    }

    /// Says what went wrong.
    pub fn complain(&mut self, problem: impl Into<String>, ctx: &mut ModelContext<Self>) {
        self.problem = Some(problem.into());
        self.said = None;
        ctx.notify();
    }

    /// Asks the registry what it has, sending back the tag it last gave.
    ///
    /// The one request Crook makes of its own, and it is made here because
    /// somebody pressed something.
    pub fn look(&mut self, ctx: &mut ModelContext<Self>) {
        if self.looking {
            return;
        }
        self.looking = true;
        self.problem = None;
        self.said = None;
        ctx.notify();

        let etag = self.etag.clone();
        let background = ctx.background().clone();
        ctx.spawn(
            async move {
                background
                    .spawn(async move {
                        let agent = fetch::agent();
                        let fetched = fetch::index(&agent, fetch::INDEX_URL, etag.as_deref())?;
                        match fetched {
                            Fetched::Unchanged => Ok(None),
                            // Kept before it is answered with, because the
                            // cache parses it — a registry serving something
                            // that is not an index must not replace the list
                            // somebody had yesterday.
                            Fetched::New { bytes, etag } => {
                                let cache = Cache::user().ok_or_else(|| {
                                    String::from("this machine has nowhere to keep the list")
                                })?;
                                let index = cache.write(&bytes, etag.as_deref())?;
                                Ok(Some((index, etag)))
                            }
                        }
                    })
                    .await
            },
            |model, outcome: Result<Option<(Index, Option<String>)>, String>, ctx| {
                model.looking = false;
                match outcome {
                    Ok(Some((index, etag))) => {
                        model.known = Some(index);
                        model.etag = etag;
                        model.fetched = Some(SystemTime::now());
                    }
                    // A registry that has published nothing since is the
                    // ordinary answer, and it still moves the clock: what the
                    // page says is how old this *answer* is, not how old the
                    // file happens to be.
                    Ok(None) => model.fetched = Some(SystemTime::now()),
                    Err(problem) => model.problem = Some(problem),
                }
                ctx.notify();
            },
        )
        .detach();
    }

    /// Downloads one release, checks it is the one the index named, and leaves
    /// it for the observer to install.
    pub fn download(&mut self, plugin: &PluginId, release: &Release, ctx: &mut ModelContext<Self>) {
        if self.downloading.is_some() {
            return;
        }
        self.downloading = Some(plugin.clone());
        self.problem = None;
        self.said = None;
        ctx.notify();

        let plugin = plugin.clone();
        let promised = release.clone();
        let release = release.clone();
        let background = ctx.background().clone();
        ctx.spawn(
            async move {
                let arrived = background
                    .spawn(async move {
                        let agent = fetch::agent();
                        fetch::module(&agent, &release)
                    })
                    .await;
                (plugin, promised, arrived)
            },
            |model, (plugin, release, arrived), ctx| {
                model.downloading = None;
                model.landed.push((plugin, release, arrived));
                ctx.notify();
            },
        )
        .detach();
    }

    /// Takes whatever has finished downloading.
    ///
    /// Taken rather than read, for the reason a sandboxed plugin's requests
    /// are: what is handed over is being acted on, and acting on it twice
    /// would install the same module twice.
    pub fn landed(&mut self) -> Vec<(PluginId, Release, Result<Vec<u8>, String>)> {
        std::mem::take(&mut self.landed)
    }
}

#[cfg(test)]
#[path = "model_tests.rs"]
mod tests;

//! Updating installed plugins with no window open.
//!
//! What the Update button does, for `--update-plugin` and `--update-plugins`:
//! the same index, the same comparison, the same download and the same hash
//! check, ending in the same [`crate::plugins::wasm::write`].
//! Only the two things a window is needed for are missing, and neither of them
//! is a check — the module is not *carried* into a running host, so nothing
//! draws until the next launch, and nothing is granted, because a capability
//! is answered by a person on the Plugins page and a command line is not one.
//!
//! # A fresh index, because the flag is the press
//!
//! The store's rule is that nothing is fetched until somebody opens it; a
//! person typing `--update-plugins` has opened it, so this asks the registry
//! rather than reading the copy on disk and hoping it is recent. The answer
//! is written back into that copy, so the window this machine opens next
//! starts from what the command line already learned. A registry that cannot
//! be reached falls back to the cache with a line saying so, because an
//! update from a list that is a week old is still an update, and refusing to
//! do anything offline would be refusing the one case where the cache exists.
//!
//! # What it will not do
//!
//! A plugin that is not in the registry, one the index has nothing newer for,
//! and one whose newest build is for an ABI this Crook does not speak are all
//! "nothing to do" rather than failures: [`index::updates`] is what decides,
//! and it is the same function the Plugins page's footer counts with.

use crook_plugin::PluginId;

use crate::plugins::wasm;

use super::cache::Cache;
use super::{fetch, index};

/// What happened to one plugin.
pub struct Outcome {
    /// Which plugin.
    pub id: PluginId,
    /// The version that was installed when this started.
    pub from: String,
    /// The version the registry offered.
    pub to: String,
    /// Whether it was installed, or why it was not.
    pub result: Result<(), String>,
}

/// Updates every installed plugin the registry is ahead of, or just `only`.
///
/// The `Err` is for the cases where there was nothing to try at all — no
/// plugins directory, no index anywhere — and a plugin that could not be
/// fetched or written is an [`Outcome`] with the reason in it, because one
/// module that failed must not take the rest of the list with it.
pub fn update(only: Option<&PluginId>) -> Result<Vec<Outcome>, String> {
    let directory = wasm::directory()
        .ok_or_else(|| String::from("this machine has no data directory to install into"))?;
    let installed = wasm::installed(&directory);
    if installed.is_empty() {
        return Ok(Vec::new());
    }
    if let Some(id) = only {
        let known = installed
            .iter()
            .any(|plugin| &crate::plugin::Plugin::manifest(plugin.as_ref()).id == id);
        if !known {
            return Err(format!("{id} is not installed"));
        }
    }

    let cache = Cache::user();
    let index = asked(cache.as_ref())?;
    let heard = index::Heard::offered(Some(&index));

    let running: Vec<(PluginId, String)> = installed
        .iter()
        .map(|plugin| {
            let manifest = crate::plugin::Plugin::manifest(plugin.as_ref());
            (manifest.id.clone(), manifest.version.to_owned())
        })
        .filter(|(id, _)| only.is_none_or(|wanted| id == wanted))
        .collect();

    let wanted = index::updates(
        &heard,
        running.iter().map(|(id, version)| {
            (
                id,
                version.as_str(),
                index::withdrawn(&index, id, version).is_some(),
            )
        }),
    );

    let agent = fetch::agent();
    let mut outcomes = Vec::with_capacity(wanted.len());
    for (id, release) in wanted {
        let from = running
            .iter()
            .find(|(installed, _)| installed == &id)
            .map(|(_, version)| version.clone())
            .unwrap_or_default();
        let to = release.version.clone();
        let result = fetched_and_written(&agent, &directory, &id, &release);
        outcomes.push(Outcome {
            id,
            from,
            to,
            result,
        });
    }
    Ok(outcomes)
}

/// The index to compare against: the registry's, or the cached copy when the
/// registry cannot be reached.
fn asked(cache: Option<&Cache>) -> Result<index::Index, String> {
    let cached = cache.and_then(Cache::read);
    let etag = cached.as_ref().and_then(|cached| cached.etag.clone());
    let agent = fetch::agent();

    match fetch::index(&agent, fetch::INDEX_URL, etag.as_deref()) {
        // Written back through the cache rather than merely parsed, so that
        // the window opened after this starts from the same list — and so
        // that a `--update-plugins` on a machine that has never opened the
        // store leaves one behind.
        Ok(fetch::Fetched::New { bytes, etag }) => match cache {
            Some(cache) => cache.write(&bytes, etag.as_deref()),
            None => index::parse(&bytes),
        },
        Ok(fetch::Fetched::Unchanged) => cached.map(|cached| cached.index).ok_or_else(|| {
            String::from(
                "the registry says nothing has changed, and this machine has no copy of the list",
            )
        }),
        Err(why) => match cached {
            Some(cached) => {
                log::warn!("the registry could not be reached ({why}); using the cached list");
                Ok(cached.index)
            }
            None => Err(format!("the registry could not be reached: {why}")),
        },
    }
}

/// One plugin: fetched, checked against what the index promised, and written.
///
/// The order is the store's own and the reason is the same — the index is a
/// mirror and never the authority, so the module's own manifest is what is
/// checked against the row that offered it, before a byte is written.
fn fetched_and_written(
    agent: &ureq::Agent,
    directory: &std::path::Path,
    id: &PluginId,
    release: &index::Release,
) -> Result<(), String> {
    let bytes = fetch::module(agent, release)?;
    let plugin = wasm::opened(&bytes)?;
    let manifest = crate::plugin::Plugin::manifest(&plugin);
    index::promised(id, release, manifest)?;
    wasm::write(directory, &bytes, manifest).map(|_| ())
}

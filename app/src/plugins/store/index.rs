//! What a registry publishes, and which of it this Crook can offer.
//!
//! One JSON file listing every plugin the registry knows, every version of
//! each, and for each version the four facts nobody should have to download a
//! module to learn: the ABI it speaks, where the artifact is, what it hashes
//! to, and what it will ask to be allowed to do.
//!
//! # It is a mirror, and it is not the authority
//!
//! Everything here was read out of a module by the registry's own copy of
//! `crook-plugin-info`, which is the host's reader — so the index and this
//! build agree about what a plugin *is*. It is still only a mirror: what a
//! plugin may do is decided against the module that was downloaded, never
//! against the line that advertised it, and a module whose manifest does not
//! match what the index promised is refused rather than installed. The index
//! is what makes a list browsable without downloading forty modules, and
//! nothing more than that.
//!
//! # Unknown fields are kept, not refused
//!
//! A registry that adds a field must not break every Crook already installed.
//! What is refused is a `schema` this build does not know, which is the one
//! change that means the shapes below have stopped being what they are.

use serde::{Deserialize, Serialize};

use crook_plugin::PluginId;
use crook_plugin_api::ABI_VERSION;

use super::super::wasm::version;

/// The index layout this build reads.
pub const SCHEMA: u32 = 1;

/// Everything a registry publishes.
#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
pub struct Index {
    /// Which layout this file is in. See [`SCHEMA`].
    pub schema: u32,
    /// Every plugin the registry knows about, in whatever order it wrote them.
    #[serde(default)]
    pub plugins: Vec<Listed>,
}

/// One plugin, and every version of it the registry has built.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct Listed {
    /// `owner/name`, the same identity the host keeps grants under.
    pub id: String,
    /// What a person sees in the list.
    pub name: String,
    /// One line, under the name.
    pub description: String,
    /// Where the source is, which is the only answer to "what am I
    /// installing" that does not depend on trusting this file.
    #[serde(default)]
    pub repository: String,
    /// SPDX, as the registry checked it.
    #[serde(default)]
    pub license: String,
    /// Newest last is not assumed: versions are compared rather than trusted
    /// to be in an order.
    #[serde(default)]
    pub versions: Vec<Release>,
}

/// One built artifact.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct Release {
    /// What the module's own manifest said it was.
    pub version: String,
    /// The plugin ABI the module speaks. A host loads one and refuses every
    /// other by name, so this is what decides whether a version is offered
    /// here at all.
    pub abi: u32,
    /// Where the `.wasm` is.
    pub url: String,
    /// What it hashes to, lowercase hex.
    ///
    /// Not a signature and not offered as one: the registry builds the
    /// artifact and writes the hash in the same run, so this says the bytes
    /// that arrived are the bytes that were built and nothing about who built
    /// them. What it is actually worth is that a truncated download, a
    /// corrupted mirror and a stale cache are all caught before anything runs.
    pub sha256: String,
    /// How big the artifact is, so a download can be refused before it is
    /// finished rather than after.
    #[serde(default)]
    pub bytes: u64,
    /// The grant keys this version asks for, in the vocabulary
    /// `settings.json` keeps: `net:api.github.com`, `file:~/.zshrc`.
    #[serde(default)]
    pub capabilities: Vec<String>,
    /// The same list as the sentences a permission dialog says.
    #[serde(default)]
    pub asks: Vec<String>,
    /// Why this version was withdrawn, if it was.
    ///
    /// A reason rather than a flag, because "this version is gone" without one
    /// leaves a person with nothing to decide with.
    #[serde(default)]
    pub yanked: Option<String>,
}

/// Reads an index, or says why it is not one.
pub fn parse(bytes: &[u8]) -> Result<Index, String> {
    let index: Index = serde_json::from_slice(bytes).map_err(|why| format!("{why}"))?;
    if index.schema != SCHEMA {
        return Err(format!(
            "this index is written in layout {} and this Crook reads {SCHEMA}",
            index.schema
        ));
    }
    Ok(index)
}

/// What this build can offer, one row per plugin.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Offer {
    /// The plugin's identity, parsed — a row whose id is not one is not a row.
    pub id: PluginId,
    /// What a person sees.
    pub name: String,
    /// One line, under the name.
    pub description: String,
    /// Where the source is.
    pub repository: String,
    /// SPDX.
    pub license: String,
    /// The newest version this build can run, if there is one.
    pub release: Option<Release>,
    /// The newest version there is at all, whatever it speaks.
    ///
    /// What this is for is the one sentence a person needs when `release` is
    /// `None`: a plugin that exists and cannot be installed here is not a
    /// plugin that is missing, and saying "built for a newer Crook" is the
    /// difference between a bug report and an upgrade.
    pub newest_anywhere: Option<Release>,
}

impl Offer {
    /// Whether this build can install it at all.
    pub fn installable(&self) -> bool {
        self.release.is_some()
    }
}

/// Every plugin in `index` this build could offer, newest usable version each.
///
/// A row whose id is not an id is dropped with a line: the registry checks
/// that, so one arriving here means the file is not the one it says it is, and
/// a store that quietly showed it would be showing a name nothing can be
/// granted under.
pub fn offers(index: &Index) -> Vec<Offer> {
    let mut offers: Vec<Offer> = index
        .plugins
        .iter()
        .filter_map(|listed| {
            let id = match PluginId::parse(&listed.id) {
                Ok(id) => id,
                Err(why) => {
                    log::warn!(
                        "the index lists {:?}, which is not a name: {why}",
                        listed.id
                    );
                    return None;
                }
            };

            Some(Offer {
                id,
                name: listed.name.clone(),
                description: listed.description.clone(),
                repository: listed.repository.clone(),
                license: listed.license.clone(),
                release: newest(
                    listed
                        .versions
                        .iter()
                        .filter(|release| release.abi == ABI_VERSION && release.yanked.is_none()),
                ),
                newest_anywhere: newest(listed.versions.iter()),
            })
        })
        .collect();

    // By name, because the registry's own order is whatever its CI walked a
    // directory in, and a list that reorders itself between two fetches is a
    // list nobody can find anything in twice.
    offers.sort_by(|left, right| left.name.cmp(&right.name).then(left.id.cmp(&right.id)));
    offers
}

/// Why an installed plugin should not be run.
///
/// Answered out of an index rather than out of a network request: a yank has
/// to be honoured on a machine that is offline and on the launch after the
/// registry said so, which means it is a fact kept on disk. A registry nobody
/// has fetched from yet withdraws nothing, and that is the safe direction —
/// an index that cannot be read is never a reason to stop running something
/// somebody installed.
pub fn withdrawn(index: &Index, id: &PluginId, version: &str) -> Option<String> {
    index
        .plugins
        .iter()
        .find(|listed| listed.id == id.as_str())?
        .versions
        .iter()
        .find(|release| release.version == version)?
        .yanked
        .clone()
}

/// The newest of whatever is handed over.
fn newest<'a>(releases: impl Iterator<Item = &'a Release>) -> Option<Release> {
    releases
        .max_by(|left, right| version::compare(&left.version, &right.version))
        .cloned()
}

#[cfg(test)]
#[path = "index_tests.rs"]
mod tests;

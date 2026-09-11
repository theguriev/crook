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

use std::cmp::Ordering;

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
    /// The plugin's icon as the registry read it out of the newest module,
    /// as base64 of the PNG.
    ///
    /// In the list rather than fetched per row, so that a face beside every
    /// name costs the one request the list already is: the registry has no
    /// second thing to ask for, and nothing about which rows a person looked
    /// at leaves the machine. `None` for a plugin built before there were
    /// pictures, which is what every version published so far was.
    ///
    /// Not carried onto the [`Offer`], which is rebuilt and cloned on every
    /// frame: the store's model decodes it once, off the list, and the rows
    /// draw the pixels.
    #[serde(default)]
    pub icon: Option<String>,
    /// Newest last is not assumed: versions are compared rather than trusted
    /// to be in an order.
    #[serde(default)]
    pub versions: Vec<Release>,
}

/// How big one of a version's previews is, in pixels as captured.
///
/// The size and nothing else: the pixels are inside the module, and what a
/// card needs before it has them is the room to reserve.
#[derive(Copy, Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct PreviewSize {
    /// Pixels across, as captured.
    pub width: u32,
    /// Pixels down, as captured.
    pub height: u32,
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
    /// The sizes of the previews inside the module, in the order it numbers
    /// them. Empty for a version built before there were pictures.
    #[serde(default)]
    pub previews: Vec<PreviewSize>,
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
    /// plugin that is missing, and saying which vocabulary it was built for is
    /// the difference between a bug report and an upgrade.
    pub newest_anywhere: Option<Release>,
    /// Why the newest version this build could otherwise run was withdrawn.
    ///
    /// The other reason [`release`](Self::release) is `None`, and a different
    /// sentence entirely: "nothing is built for this Crook" is somebody's to
    /// fix by publishing, and "it was taken back, because —" is a thing to
    /// read.
    pub withdrawn: Option<String>,
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

            let ours = || {
                listed
                    .versions
                    .iter()
                    .filter(|release| release.abi == ABI_VERSION)
            };
            let offered = newest(ours().filter(|release| release.yanked.is_none()));

            Some(Offer {
                id,
                name: listed.name.clone(),
                description: listed.description.clone(),
                repository: listed.repository.clone(),
                license: listed.license.clone(),
                // Only when there is nothing left to offer: a plugin whose
                // newest version was withdrawn and whose one before it still
                // stands is a plugin somebody can install, and the yank is
                // not their news.
                withdrawn: match offered {
                    Some(_) => None,
                    None => newest(ours()).and_then(|release| release.yanked),
                },
                release: offered,
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

/// What the registry's offer means for what is on this machine.
///
/// Five answers rather than a comparison, because two of them look the same
/// to a comparison and are not: a person on a withdrawn 0.10.0 offered 0.9.0
/// is being offered a *replacement*, and "the registry is behind you" — which
/// is what comparing the two versions says — would be a card telling them to
/// stay on a version somebody took back.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Change {
    /// Not on this machine, and this is what installing would bring.
    Install(Release),
    /// On this machine, and the registry is ahead of it.
    Update(Release),
    /// On this machine, taken back, and this is what the registry offers
    /// instead — older, newer, it does not matter: the one that is running
    /// is the one that must not be.
    Replace(Release),
    /// On this machine, and there is nothing newer.
    Current,
    /// Nothing this build can run — a plugin built for another vocabulary,
    /// or one with nothing built at all.
    Nothing,
}

impl Change {
    /// The release worth fetching, if there is one: an update, or the
    /// replacement for a version taken back.
    ///
    /// The one question every surface asks of a change — the row's trailing
    /// word, the count at the foot of the list, the `--plugins` line — and
    /// the answer that must not differ between them.
    pub fn fetchable(&self) -> Option<&Release> {
        match self {
            Self::Update(release) | Self::Replace(release) => Some(release),
            Self::Install(_) | Self::Current | Self::Nothing => None,
        }
    }
}

/// What `offer` comes to for a machine holding `installed`.
///
/// `installed` is the module's own version, which is the one the Plugins
/// page prints, and `withdrawn` is whether *that* version was taken back —
/// which the index answers through [`withdrawn`], and which is not the same
/// question as whether the offer has a withdrawal to report.
pub fn change(offer: &Offer, installed: Option<&str>, withdrawn: bool) -> Change {
    match (installed, &offer.release) {
        (None, Some(release)) => Change::Install(release.clone()),
        (_, None) => Change::Nothing,
        (Some(_), Some(release)) if withdrawn => Change::Replace(release.clone()),
        (Some(installed), Some(release)) => match version::compare(&release.version, installed) {
            Ordering::Greater => Change::Update(release.clone()),
            _ => Change::Current,
        },
    }
}

/// What the store is doing about a plugin right now.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Busy {
    /// Its module is being fetched.
    Downloading,
    /// It is queued behind one that is.
    Waiting,
}

/// What the store last said, for surfaces that are not the store.
///
/// The Plugins page has no handle to the store's model and must not read the
/// index off disk on every frame; what it has is this, handed over whenever
/// the store's answer changes. A snapshot rather than a handle, so that the
/// page reads one consistent answer per frame.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Heard {
    /// Every plugin the registry offers, as [`offers`] works them out.
    pub offers: Vec<Offer>,
    /// Which plugins the store is fetching or about to.
    pub busy: Vec<(PluginId, Busy)>,
}

impl Heard {
    /// What `index` offers, with the store doing nothing yet — which is what
    /// a window hears at its opening, off the copy on disk, before anything
    /// has been pressed.
    pub fn offered(index: Option<&Index>) -> Self {
        Self {
            offers: index.map(offers).unwrap_or_default(),
            busy: Vec::new(),
        }
    }

    /// The registry's row for one plugin, if it has one.
    pub fn offer(&self, id: &PluginId) -> Option<&Offer> {
        self.offers.iter().find(|offer| offer.id == *id)
    }

    /// What the store is doing about one plugin, if anything.
    pub fn busy(&self, id: &PluginId) -> Option<Busy> {
        self.busy
            .iter()
            .find(|(busy, _)| busy == id)
            .map(|(_, what)| *what)
    }
}

/// Every installed plugin the registry is ahead of, or whose version was
/// taken back with a replacement offered — each with the release to fetch.
///
/// `installed` is the id, the running version and whether that version was
/// withdrawn, for every plugin that is on this machine as a file: the natives
/// are never in a registry, and a module being run from wherever it was built
/// is not one an update could be written over.
pub fn updates<'a>(
    heard: &Heard,
    installed: impl IntoIterator<Item = (&'a PluginId, &'a str, bool)>,
) -> Vec<(PluginId, Release)> {
    installed
        .into_iter()
        .filter_map(|(id, version, withdrawn)| {
            let offer = heard.offer(id)?;
            let release = change(offer, Some(version), withdrawn)
                .fetchable()
                .cloned()?;
            Some((id.clone(), release))
        })
        .collect()
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

/// Whether a module is the one the index said it was.
///
/// The index is a mirror: everything in it was read out of an artifact by the
/// registry's copy of the host's own reader, so the two agree — and a row that
/// does *not* agree with the module it points at is the one thing a store must
/// not install, whether that is a registry with a mistake in it or a URL
/// serving something else entirely. Checked before a byte is written.
///
/// The version is compared and not only the id, because a version is what a
/// person read the capability list of, and the capability *keys* are compared
/// as a set because that list is what they were being asked to allow.
pub fn promised(release: &Release, manifest: &crook_plugin::Manifest) -> Result<(), String> {
    if manifest.version != release.version {
        return Err(format!(
            "the list offered {} and the module says it is {}",
            release.version, manifest.version
        ));
    }

    let mut asked: Vec<String> = manifest
        .capabilities
        .iter()
        .flat_map(crook_plugin_api::Capability::keys)
        .collect();
    let mut offered = release.capabilities.clone();
    asked.sort();
    offered.sort();
    if asked != offered {
        return Err(format!(
            "the list said it wants {} and the module asks for {}",
            said(&offered),
            said(&asked)
        ));
    }

    Ok(())
}

/// A list of grant keys, for a sentence.
fn said(keys: &[String]) -> String {
    match keys.is_empty() {
        true => String::from("nothing"),
        false => keys.join(", "),
    }
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

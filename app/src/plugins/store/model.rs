//! What the store knows, and the slow things it does off the drawing thread.
//!
//! A model rather than a field on the plugin, because everything it does —
//! reading an index over the network, downloading a module, decoding the
//! pictures in one — is blocking work that must not happen inside `render`,
//! and because what arrives has to be able to repaint the window when it
//! lands.
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
//! written on it, on a machine that has never been online since. The icons in
//! that copy are decoded on the pool, not here: a model is made while the
//! window is being built, and forty PNGs on the foreground would be forty
//! PNGs between a person and their first frame.
//!
//! # A download lands here and is installed by somebody else
//!
//! Installing needs the workspace: the module has to be checked, written, and
//! then *carried* by the host so that it runs without a restart, and a model
//! can reach none of those. So a finished download is left in [`landed`] and
//! the section's observer drains it, exactly as a sandboxed plugin's requests
//! are drained by the observer in `plugins::wasm`.
//!
//! # One at a time, and the rest wait
//!
//! Downloads are one fetch at a time — two at once would be two workers held
//! on a pool sized for the chains that park on it, for no gain a person could
//! see — and a second press queues behind the first rather than being refused
//! with a sentence: "update all" is that queue, drained one module at a time
//! by each completion starting the next. A queued update is still one task
//! at a time, which is what keeps the store off the list of chains that park
//! a worker.
//!
//! [`landed`]: StoreModel::landed

use std::collections::{BTreeMap, VecDeque};
use std::sync::Arc;
use std::time::SystemTime;

use base64::Engine as _;
use crookui_core::image::{Bitmap, resample};
use crookui_core::prelude::*;

use crook_plugin::PluginId;

use crate::picture::{self, Limits};
use crate::plugins::pictures::{Decoded, Pictures, Preview};

use super::cache::Cache;
use super::fetch::{self, Fetched};
use super::index::{Busy, Heard, Index, Offer, Release, offers};

/// How a module's bytes are got, checked against the release that named them.
///
/// A function rather than a call, so that a test can answer with a module it
/// assembled and never opens a socket. The one the application uses is
/// [`fetch::module`] — the origin rule, the size ceiling and the hash check
/// are inside it — and every download and every look inside a module goes
/// through whichever this is.
pub type Fetcher = Arc<dyn Fn(&Release) -> Result<Vec<u8>, String> + Send + Sync>;

/// The icons the list carries, decoded, by `owner/name`.
pub type Icons = BTreeMap<String, Arc<Bitmap>>;

/// The most characters an icon in the list may run to before it is decoded:
/// [`MAX_ICON_BYTES`] as base64, which is the line the registry's own check
/// draws. A list that arrived some other way is held to the same line, so
/// that a row is never a reason to decode a picture of any size a stranger
/// chose.
///
/// [`MAX_ICON_BYTES`]: crook_plugin_api::pictures::MAX_ICON_BYTES
const MOST_ICON_CHARS: usize = crook_plugin_api::pictures::MAX_ICON_BYTES.div_ceil(3) * 4;

/// The edge a face is held at once decoded.
///
/// A face is drawn twelve and eighteen logical pixels tall, which is under
/// sixty-four device pixels at every scale a display reports, and the
/// renderer resamples to the drawn size from whatever is held. A 256-pixel
/// source is a quarter of a megabyte per row, held for the whole session on
/// the thread that draws, for a list a registry can make thousands of rows
/// long; shrunk here, on the pool, each is sixteen kilobytes.
const ICON_HELD_EDGE: u32 = 64;

/// The most the list's faces may add up to, in held pixels.
///
/// A thousand faces at [`ICON_HELD_EDGE`]. Past it the rest of the rows go
/// faceless with a line in the log, rather than a hostile list costing a
/// gigabyte for as long as the window is open — on every launch, once it is
/// cached.
const ICONS_BUDGET: usize = 16 << 20;

/// A module somebody looked inside: its pictures, and the bytes they came in.
///
/// The bytes are kept because they are the module Install would fetch: a
/// person who looked at the pictures and then pressed Install has already
/// downloaded it, and a second GET for the same hash would be the store
/// spending their bandwidth to prove a point. Empty when the pictures came
/// out of the module already on this machine, which needed no fetch.
pub struct LookedInside {
    /// Which plugin's module this is.
    pub plugin: PluginId,
    /// What the index said the module hashes to, which is what was checked.
    pub sha256: String,
    /// The module as it arrived, or nothing when it was not fetched.
    pub bytes: Vec<u8>,
    /// Its previews, decoded, each at the size it was captured at.
    pub pictures: Decoded,
}

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
    /// see. What is asked for while this is busy goes into
    /// [`pending`](Self::pending).
    downloading: Option<PluginId>,
    /// The downloads waiting for the one in flight, in the order they were
    /// asked for.
    pending: VecDeque<(PluginId, Release)>,
    /// What went wrong last, and which plugin it was about.
    ///
    /// The id is what keeps a sentence attached to the row it is about: a
    /// message drawn on whatever card happens to be showing is a message about
    /// the wrong plugin.
    problem: Option<(Option<PluginId>, String)>,
    /// Downloads that have arrived and need a workspace to install them,
    /// each with the release it was asked for — which is what the module that
    /// arrived has to be checked against.
    landed: Vec<(PluginId, Release, Result<Vec<u8>, String>)>,
    /// What the last install or removal came to, and which plugin it was
    /// about. See [`problem`](Self::problem).
    said: Option<(Option<PluginId>, String)>,
    /// The icons the list carries, decoded on the pool and landed here.
    icons: Icons,
    /// Which plugin's module is being looked inside, if any.
    looking_inside: Option<PluginId>,
    /// The last module somebody looked inside. One at a time: a card of six
    /// previews is megabytes of pixels, and the card that is showing is the
    /// one they were asked for.
    looked_inside: Option<LookedInside>,
    /// How a module's bytes are got.
    fetch: Fetcher,
}

impl Entity for StoreModel {
    type Event = ();
}

impl StoreModel {
    /// A store that knows whatever is already on disk, and nothing else.
    ///
    /// The cache is handed in rather than found here, so that a test can hand
    /// it a directory of its own — a model that read the person's real one
    /// would be a test whose answer depends on what they installed. Nothing
    /// is decoded here: see [`remember_icons`](Self::remember_icons).
    pub fn new(cache: Option<Cache>) -> Self {
        let cached = cache.and_then(|cache| cache.read());
        Self {
            known: cached.as_ref().map(|cached| cached.index.clone()),
            etag: cached.as_ref().and_then(|cached| cached.etag.clone()),
            fetched: cached.as_ref().and_then(|cached| cached.fetched),
            looking: false,
            downloading: None,
            pending: VecDeque::new(),
            problem: None,
            landed: Vec::new(),
            said: None,
            icons: Icons::new(),
            looking_inside: None,
            looked_inside: None,
            fetch: Arc::new(|release| fetch::module(&fetch::agent(), release)),
        }
    }

    /// Answers every module fetch with `fetch` instead of the network.
    #[cfg(test)]
    pub(crate) fn fetch_with(&mut self, fetch: Fetcher) {
        self.fetch = fetch;
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

    /// The plugins waiting to be downloaded, in order.
    pub fn queued(&self) -> Vec<PluginId> {
        self.pending
            .iter()
            .map(|(plugin, _)| plugin.clone())
            .collect()
    }

    /// The icons the list carries, as far as they have been decoded.
    pub fn icons(&self) -> &Icons {
        &self.icons
    }

    /// Which plugin's module is being looked inside, if any.
    pub fn looking_inside(&self) -> Option<&PluginId> {
        self.looking_inside.as_ref()
    }

    /// The last module somebody looked inside, with its pictures.
    pub fn looked_inside(&self) -> Option<&LookedInside> {
        self.looked_inside.as_ref()
    }

    /// What went wrong, if the last thing that happened went wrong.
    pub fn problem(&self) -> Option<(Option<&PluginId>, &str)> {
        self.problem
            .as_ref()
            .map(|(about, why)| (about.as_ref(), why.as_str()))
    }

    /// What the last install or removal came to.
    pub fn said(&self) -> Option<(Option<&PluginId>, &str)> {
        self.said
            .as_ref()
            .map(|(about, said)| (about.as_ref(), said.as_str()))
    }

    /// Which plugins are being fetched, and which are waiting their turn.
    pub fn busy(&self) -> Vec<(PluginId, Busy)> {
        let mut busy: Vec<(PluginId, Busy)> = self
            .downloading
            .iter()
            .map(|plugin| (plugin.clone(), Busy::Downloading))
            .collect();
        busy.extend(
            self.pending
                .iter()
                .map(|(plugin, _)| (plugin.clone(), Busy::Waiting)),
        );
        busy
    }

    /// What the store says, for every surface that is not the store.
    ///
    /// A snapshot: the Plugins page holds one and reads it on every frame,
    /// the store's own section reads one per frame it draws, and the store
    /// hands the window a fresh one whenever its answer changes.
    pub fn heard(&self) -> Heard {
        Heard {
            offers: self.offers(),
            busy: self.busy(),
        }
    }

    /// Says something about a plugin, and clears whatever went wrong before.
    pub fn say(
        &mut self,
        about: Option<&PluginId>,
        said: impl Into<String>,
        ctx: &mut ModelContext<Self>,
    ) {
        self.said = Some((about.cloned(), said.into()));
        self.problem = None;
        ctx.notify();
    }

    /// Says what went wrong, and what it was about.
    pub fn complain(
        &mut self,
        about: Option<&PluginId>,
        problem: impl Into<String>,
        ctx: &mut ModelContext<Self>,
    ) {
        self.problem = Some((about.cloned(), problem.into()));
        self.said = None;
        ctx.notify();
    }

    /// Asks the registry what it has, sending back the tag it last gave.
    ///
    /// The one request Crook makes of its own, and it is made here because
    /// somebody pressed something. A new list's icons are decoded in the same
    /// task, on the pool, so the rows have their faces the frame the list
    /// changes; a list that has not changed keeps the icons already held.
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
                                let icons = decoded_icons(&listed_icons(&index), ICONS_BUDGET);
                                Ok(Some((index, etag, icons)))
                            }
                        }
                    })
                    .await
            },
            |model, outcome: Result<Option<(Index, Option<String>, Icons)>, String>, ctx| {
                model.looking = false;
                match outcome {
                    Ok(Some((index, etag, icons))) => {
                        model.known = Some(index);
                        model.etag = etag;
                        model.fetched = Some(SystemTime::now());
                        model.icons = icons;
                    }
                    // A registry that has published nothing since is the
                    // ordinary answer, and it still moves the clock: what the
                    // page says is how old this *answer* is, not how old the
                    // file happens to be.
                    Ok(None) => model.fetched = Some(SystemTime::now()),
                    Err(problem) => model.problem = Some((None, problem)),
                }
                ctx.notify();
            },
        )
        .detach();
    }

    /// Decodes the icons of the list already held, on the pool.
    ///
    /// Called once the model is attached, and never from `new`: the model is
    /// made while the window is being built, and a face beside every name is
    /// worth nothing to a person still waiting for the first frame. Nothing is
    /// spawned for a list with no icons in it, which is every list published
    /// before there were pictures.
    pub fn remember_icons(&mut self, ctx: &mut ModelContext<Self>) {
        let listed = self.known.as_ref().map(listed_icons).unwrap_or_default();
        if listed.is_empty() {
            return;
        }

        let decoding = ctx
            .background()
            .spawn(async move { decoded_icons(&listed, ICONS_BUDGET) });
        ctx.spawn(decoding, |model, icons, ctx| {
            // A look that landed first has the newer list's faces, and this
            // is the older list's: what it holds already wins.
            for (id, icon) in icons {
                model.icons.entry(id).or_insert(icon);
            }
            ctx.notify();
        })
        .detach();
    }

    /// Downloads one release, checks it is the one the index named, and leaves
    /// it for the observer to install — or queues it behind the download in
    /// flight.
    ///
    /// A release somebody has already looked inside is not fetched again:
    /// the bytes are here, checked against the same hash, and they are handed
    /// over as if they had just arrived.
    pub fn download(&mut self, plugin: &PluginId, release: &Release, ctx: &mut ModelContext<Self>) {
        let already = self.downloading.as_ref() == Some(plugin)
            || self.pending.iter().any(|(queued, _)| queued == plugin);
        if already {
            return;
        }
        self.problem = None;
        self.said = None;

        let held = self.looked_inside.as_ref().filter(|looked| {
            looked.plugin == *plugin
                && !looked.bytes.is_empty()
                && looked.sha256.eq_ignore_ascii_case(release.sha256.trim())
        });
        if let Some(looked) = held {
            self.landed
                .push((plugin.clone(), release.clone(), Ok(looked.bytes.clone())));
            ctx.notify();
            return;
        }

        match self.downloading {
            Some(_) => self.pending.push_back((plugin.clone(), release.clone())),
            None => self.start(plugin.clone(), release.clone(), ctx),
        }
        ctx.notify();
    }

    /// Downloads every release handed over that is not already on its way.
    pub fn update_all(&mut self, wanted: Vec<(PluginId, Release)>, ctx: &mut ModelContext<Self>) {
        for (plugin, release) in wanted {
            self.download(&plugin, &release, ctx);
        }
    }

    /// Starts one fetch, which nothing else may be doing at the time.
    fn start(&mut self, plugin: PluginId, release: Release, ctx: &mut ModelContext<Self>) {
        self.downloading = Some(plugin.clone());

        let promised = release.clone();
        let fetch = self.fetch.clone();
        let background = ctx.background().clone();
        ctx.spawn(
            async move {
                let arrived = background.spawn(async move { fetch(&release) }).await;
                (plugin, promised, arrived)
            },
            |model, (plugin, release, arrived), ctx| model.arrived(plugin, release, arrived, ctx),
        )
        .detach();
    }

    /// What a finished fetch does: leaves what came for the observer, and
    /// starts the next one waiting.
    ///
    /// Reachable on its own so a test can finish a download without a
    /// network, and see the queue move.
    pub(crate) fn arrived(
        &mut self,
        plugin: PluginId,
        release: Release,
        arrived: Result<Vec<u8>, String>,
        ctx: &mut ModelContext<Self>,
    ) {
        self.downloading = None;
        self.landed.push((plugin, release, arrived));
        if let Some((plugin, release)) = self.pending.pop_front() {
            self.start(plugin, release, ctx);
        }
        ctx.notify();
    }

    /// Takes whatever has finished downloading.
    ///
    /// Taken rather than read, for the reason a sandboxed plugin's requests
    /// are: what is handed over is being acted on, and acting on it twice
    /// would install the same module twice.
    pub fn landed(&mut self) -> Vec<(PluginId, Release, Result<Vec<u8>, String>)> {
        std::mem::take(&mut self.landed)
    }

    /// Decodes the pictures inside a plugin's module, fetching the module if
    /// it is not already on this machine.
    ///
    /// `installed` is the previews of the module already here, when there is
    /// one that carries any: those are decoded and nothing is fetched. Else
    /// the offered release is fetched — the same file, through the same
    /// checks, as Install would fetch — and its pictures are read out of it;
    /// the bytes are kept so that an Install afterwards costs no second
    /// download. One look at a time, and a second press while one is in
    /// flight does nothing: the button is dead while it is.
    pub fn look_inside(
        &mut self,
        plugin: &PluginId,
        release: Option<&Release>,
        installed: Option<Vec<Preview>>,
        ctx: &mut ModelContext<Self>,
    ) {
        if self.looking_inside.is_some() {
            return;
        }
        let (release, previews) = match (installed, release) {
            (Some(previews), _) => (None, previews),
            (None, Some(release)) => (Some(release.clone()), Vec::new()),
            (None, None) => {
                self.complain(Some(plugin), "has no pictures to show", ctx);
                return;
            }
        };
        // The module a previous look fetched, when it is the same one: a
        // second press draws the pictures again and costs no second GET.
        let held = self
            .looked_inside
            .as_ref()
            .filter(|looked| looked.plugin == *plugin && !looked.bytes.is_empty())
            .filter(|looked| {
                release.as_ref().is_some_and(|release| {
                    looked.sha256.eq_ignore_ascii_case(release.sha256.trim())
                })
            })
            .map(|looked| looked.bytes.clone());
        self.looking_inside = Some(plugin.clone());
        self.problem = None;
        ctx.notify();

        let named = plugin.clone();
        let fetch = self.fetch.clone();
        let background = ctx.background().clone();
        ctx.spawn(
            async move {
                background
                    .spawn(async move {
                        let Some(release) = release else {
                            return Ok(LookedInside {
                                plugin: named,
                                sha256: String::new(),
                                bytes: Vec::new(),
                                pictures: Pictures::decode_previews(&previews),
                            });
                        };
                        let bytes = match held {
                            Some(bytes) => bytes,
                            None => fetch(&release).map_err(|why| did_not_arrive(&why))?,
                        };
                        // A picture past the rule is a line and no pictures,
                        // not a failure: the module arrived and was checked,
                        // and it is the module Install would fetch — so the
                        // bytes are kept, and Install afterwards costs no
                        // second download, as the card promised. The host
                        // keeps such a module and runs it faceless for the
                        // same reason.
                        let previews = match crook_wasm::Pictures::read_bytes(&bytes) {
                            Ok(raw) => Pictures::previews_from(raw.previews),
                            Err(why) => {
                                log::warn!("{named} carries a picture Crook cannot draw: {why}");
                                Vec::new()
                            }
                        };
                        Ok(LookedInside {
                            plugin: named,
                            sha256: release.sha256.trim().to_owned(),
                            bytes,
                            pictures: Pictures::decode_previews(&previews),
                        })
                    })
                    .await
            },
            |model, outcome: Result<LookedInside, String>, ctx| {
                let plugin = model.looking_inside.take();
                match outcome {
                    Ok(looked) => model.looked_inside = Some(looked),
                    Err(why) => model.problem = Some((plugin, why)),
                }
                ctx.notify();
            },
        )
        .detach();
    }
}

/// What the card says of a module the fetch did not bring.
///
/// One sentence for both things a module is fetched for, installing and
/// looking inside: it is the same GET through the same checks, and a
/// refusal that read "http status: 404" on one card and "did not arrive:
/// http status: 404" on the other would be two stores.
pub(super) fn did_not_arrive(why: &str) -> String {
    format!("did not arrive: {why}")
}

/// Every icon the list carries, still base64, by `owner/name`.
fn listed_icons(index: &Index) -> Vec<(String, String)> {
    index
        .plugins
        .iter()
        .filter_map(|listed| Some((listed.id.clone(), listed.icon.clone()?)))
        .collect()
}

/// Those icons as pixels, held to [`ICON_HELD_EDGE`] each and `budget`
/// together. Pool work: a list is forty PNGs.
///
/// One that will not decode is a line in the log and a row with no face,
/// for the reason the host keeps a plugin whose icon it cannot draw: the
/// list is still the list. Past the budget the rest are faces nobody gets,
/// said once, which is the answer to a list that is not a list but a way of
/// making this machine hold a gigabyte.
fn decoded_icons(listed: &[(String, String)], budget: usize) -> Icons {
    let mut held = 0;
    let mut icons = Icons::new();
    for (index, (id, icon)) in listed.iter().enumerate() {
        if held >= budget {
            log::warn!(
                "the index lists {} icons and Crook holds the first {index}; the rest have no face",
                listed.len()
            );
            break;
        }
        if icon.len() > MOST_ICON_CHARS {
            log::warn!(
                "the index lists an icon for {id} that Crook cannot draw: it is {} characters and \
                 an icon is at most {MOST_ICON_CHARS}",
                icon.len()
            );
            continue;
        }
        let decoded = base64::engine::general_purpose::STANDARD
            .decode(icon)
            .map_err(|why| format!("is not base64: {why}"))
            .and_then(|png| picture::decode(&png, Limits::ICON));
        match decoded {
            Ok(bitmap) => {
                let face = held_face(bitmap);
                held += face.canvas().pixels.len();
                icons.insert(id.clone(), Arc::new(face));
            }
            Err(why) => {
                log::warn!("the index lists an icon for {id} that Crook cannot draw: it {why}");
            }
        }
    }
    icons
}

/// `icon`, no bigger than [`ICON_HELD_EDGE`] a side.
fn held_face(icon: Bitmap) -> Bitmap {
    let (width, height) = icon.size();
    if width <= ICON_HELD_EDGE && height <= ICON_HELD_EDGE {
        return icon;
    }
    // Square, because `Limits::ICON` refused anything else before decoding.
    resample(&icon, ICON_HELD_EDGE, ICON_HELD_EDGE)
}

#[cfg(test)]
#[path = "model_tests.rs"]
mod tests;

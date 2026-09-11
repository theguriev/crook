//! Reaching the registry, which is the first request Crook makes of its own.
//!
//! Every other byte this application has ever sent belonged to a plugin, under
//! a `Network` capability somebody granted by host name. This one is Crook's,
//! and the README's sentence about telemetry is what shapes it:
//!
//! * **Nothing is sent that identifies anybody.** No account, no machine id,
//!   no version, no list of what is installed — a `GET` and an `If-None-Match`
//!   carrying a tag the registry itself wrote. What a static host can see is
//!   what any static host sees: that somebody asked for a file.
//! * **Nothing is fetched until somebody opens the store.** Not at launch, not
//!   on a timer, not to check for updates in the background. A terminal that
//!   phones a server every time it starts is a terminal that has to be trusted
//!   about what it said.
//! * **Offline is a first-class answer.** A failed fetch is the cached list
//!   with a line saying how old it is, never an empty page.
//!
//! # What the hash is worth, and what it is not
//!
//! The registry builds an artifact and writes its hash in the same run, so a
//! hash that matches says the bytes that arrived are the bytes that were
//! built. It says nothing about *who* built them, which is why nothing here
//! calls it verification: it catches a truncated download, a corrupted mirror
//! and a stale artifact behind a moved URL, and it pins a version so that what
//! a person agreed to install is what installs.

use std::time::Duration;

use sha2::{Digest, Sha256};

use super::index::Release;

/// Where the registry publishes.
///
/// A release asset rather than a page, and the tag never moves: `index` is
/// the one release that is rewritten in place, by the same CI run that builds
/// the artifacts beside it. What that buys over GitHub Pages is that it needs
/// nothing configured — a registry is a repository with a workflow in it, and
/// somebody forking this one to run their own list has one thing to change,
/// this line, rather than a site to turn on first.
pub const INDEX_URL: &str =
    "https://github.com/theguriev/crook-plugins/releases/download/index/index.json";

/// How long any one request may take, start to finish.
const TIMEOUT: Duration = Duration::from_secs(20);

/// The most an index may be.
///
/// A thousand plugins with ten versions each is about a megabyte; four is room
/// to grow into and small enough that a registry serving something else
/// entirely is refused rather than read into memory.
const INDEX_LIMIT: u64 = 4 << 20;

/// The most a module may be.
///
/// The largest plugin anybody has written is 670KB, and its pictures ride
/// inside it — an icon of at most 32 KiB and up to six previews of at most
/// 512 KiB each, which is three megabytes on top of the code at the very
/// worst. Eight megabytes is past that and far short of a download nobody
/// chose.
const MODULE_LIMIT: u64 = 8 << 20;

/// How many redirects a request may follow.
///
/// One is what a GitHub release asset costs — a `302` from `github.com` to
/// `objects.githubusercontent.com` — and three is room for a registry host
/// that redirects to its storage through one more hop. Ten, which is the
/// library's default, is a request that can be walked across ten hosts by an
/// index somebody tampered with.
const MAX_REDIRECTS: u32 = 3;

/// What every request says it is.
///
/// Bare, with no version: the README promises that nothing identifying goes
/// with a request, and the library's default names its own version with every
/// call — which is not Crook's, and is still a fact about this machine that
/// nobody asked to send.
const USER_AGENT: &str = "crook";

/// What a fetch of the index came back with.
pub enum Fetched {
    /// The registry has published nothing since the tag that was sent.
    Unchanged,
    /// A new list, and the tag to send next time.
    New {
        /// The bytes, unparsed: the cache parses them before it keeps them.
        bytes: Vec<u8>,
        /// What the registry called this version of the file.
        etag: Option<String>,
    },
}

/// The agent every store request goes through.
///
/// One per fetch rather than one kept forever, which is what the plugin
/// runtime does and for the same reason: a connection pool held open to a host
/// nobody is talking to is a socket somebody has to explain.
pub fn agent() -> ureq::Agent {
    ureq::Agent::config_builder()
        .https_only(true)
        .timeout_global(Some(TIMEOUT))
        .max_redirects(MAX_REDIRECTS)
        .user_agent(USER_AGENT)
        .build()
        .new_agent()
}

/// Where every artifact the registry publishes is served from: the index's
/// own directory, up to and including its last `/`.
///
/// The registry writes every artifact beside the index — the same release,
/// the same run — so an artifact URL that starts anywhere else is not one the
/// registry wrote. Somebody running a registry of their own changes
/// [`INDEX_URL`] and gets the matching rule for free.
pub fn assets_prefix() -> &'static str {
    let end = INDEX_URL
        .rfind('/')
        .expect("the index URL is a literal with a path");
    &INDEX_URL[..=end]
}

/// Whether `url` names an artifact beside the index and nothing else.
///
/// The check is on the URL the index gave, before the request: what a
/// redirect leads to is the registry host's business, and the rule here is
/// about what an index can *send Crook to ask*. A poisoned index could
/// otherwise point a download at any host on the internet, and the request
/// would tell that host which plugin somebody wanted. Exactly one path
/// segment after the prefix, and no query or fragment, because a URL with
/// either is one the registry never writes and one that could carry
/// something through.
fn from_the_registry(url: &str) -> Result<(), String> {
    let beside_the_index = url
        .strip_prefix(assets_prefix())
        .is_some_and(|name| !name.is_empty() && !name.contains(['/', '?', '#']));
    match beside_the_index {
        true => Ok(()),
        false => Err(String::from(
            "is served from somewhere that is not the registry",
        )),
    }
}

/// Asks the registry for the index, sending back the tag it last gave.
pub fn index(agent: &ureq::Agent, url: &str, etag: Option<&str>) -> Result<Fetched, String> {
    let mut request = agent.get(url);
    if let Some(etag) = etag {
        request = request.header("If-None-Match", etag);
    }

    let mut response = match request.call() {
        Ok(response) => response,
        // Both shapes, because which one a 304 arrives as is `ureq`'s
        // configuration rather than the registry's: with statuses as errors it
        // is an error, and without them it is a response with no body. The
        // answer is the same either way, and it is the answer this asked for.
        Err(ureq::Error::StatusCode(304)) => return Ok(Fetched::Unchanged),
        Err(why) => return Err(why.to_string()),
    };
    if response.status() == 304 {
        return Ok(Fetched::Unchanged);
    }

    let etag = response
        .headers()
        .get("etag")
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned);

    let bytes = response
        .body_mut()
        .with_config()
        .limit(INDEX_LIMIT)
        .read_to_vec()
        .map_err(|why| why.to_string())?;

    Ok(Fetched::New { bytes, etag })
}

/// Downloads one artifact and checks it is the one the index named.
///
/// The only request that fetches a module, whether to install it or to look
/// at the pictures inside it, so the origin rule and the size ceiling are
/// applied here and nowhere else.
pub fn module(agent: &ureq::Agent, release: &Release) -> Result<Vec<u8>, String> {
    from_the_registry(&release.url)?;
    let mut response = agent
        .get(&release.url)
        .call()
        .map_err(|why| why.to_string())?;
    let bytes = response
        .body_mut()
        .with_config()
        .limit(MODULE_LIMIT)
        .read_to_vec()
        .map_err(|why| why.to_string())?;

    checked(bytes, release)
}

/// Whether these bytes are what `release` promised.
pub fn checked(bytes: Vec<u8>, release: &Release) -> Result<Vec<u8>, String> {
    // Size first, because it is the cheap half of the same question and its
    // failure says something more useful: a length that is not the length is
    // usually a download that ended early or a URL that now serves a page.
    if release.bytes != 0 && bytes.len() as u64 != release.bytes {
        return Err(format!(
            "it is {} bytes and the index says {}",
            bytes.len(),
            release.bytes
        ));
    }

    let hashed = sha256_hex(&bytes);
    if !hashed.eq_ignore_ascii_case(release.sha256.trim()) {
        return Err(format!(
            "it hashes to {hashed} and the index says {}",
            release.sha256
        ));
    }

    Ok(bytes)
}

/// `bytes` as lowercase hex SHA-256, which is what an index writes.
pub fn sha256_hex(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    let mut hex = String::with_capacity(digest.len() * 2);
    for byte in digest {
        use std::fmt::Write as _;
        // Infallible into a `String`, and the alternative is a second
        // allocation per byte.
        let _ = write!(hex, "{byte:02x}");
    }
    hex
}

#[cfg(test)]
#[path = "fetch_tests.rs"]
mod tests;

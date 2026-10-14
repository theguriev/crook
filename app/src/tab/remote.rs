//! Which machine a pane's shell is on.
//!
//! A shell says where it is with OSC 7, a `file://` URL, and the URL's host is
//! the machine the path is on. Most of the time that is this one. On the far
//! side of an `ssh`, or inside a container, it is not, and the path means a
//! directory *there*: read on this machine, `~/work/app` is this machine's
//! checkout of something, or nothing — never the one the shell is in. And
//! everything Crook reads by a pane's directory reads this machine's disk: the
//! branch and the diff counts on the row, the Changes column, the worktree
//! menu, a plugin's Where answer, the directory a new tab opens in. So a
//! report under another host's name is kept apart from the pane's working
//! directory, as a [`Remote`], and nothing reads a disk by it.
//!
//! **Which names are this machine's is asked of the system, not guessed.**
//! The snippets say the name the system gave when the shell started, read
//! once — not `$HOST` or `$HOSTNAME`, which the environment or a startup file
//! can set to anything — and fish says its read-only `$hostname`, in its own
//! OSC 7 as well. So the same call here gives the same answer. It is asked
//! once, and asked again only when a shell names a host that matches nothing
//! known: a Mac takes a new name from the network it joins, and a shell
//! started after the move says the new one while Crook still knows the old.
//! Every name the machine has answered to is kept, because a shell started
//! before the move goes on saying the old one. A host nothing matches costs
//! that one extra call each time it is asked about: once per change of place
//! on the far side — a `cd`, or an `ssh` in or out, since a prompt that
//! repeats the same place is dropped before it gets here.
//!
//! **The system's name is not the only one.** `localhost`, and a loopback or
//! unspecified address — `127.0.0.1`, `::1`, `0.0.0.0` — are this machine to
//! anything that says them. So are `$HOST` and `$HOSTNAME` as Crook itself
//! was started with them, which every shell it starts inherits: a shell
//! integration other than Crook's that writes one of those into its URL is a
//! shell here, whatever the variable was set to.
//!
//! **A name is this machine's by its first label as well as whole.** The two
//! mistakes do not cost the same: a shell here taken for one elsewhere loses
//! its branch, its diff and its Changes column, every prompt, for as long as
//! it runs; one elsewhere taken for here shows what every pane showed before
//! there was a rule. An integration that writes `$HOST` after a startup file
//! set `HOST=$(hostname -s)`, or a Mac that says `eugens-mbp` in one place and
//! `eugens-mbp.local` in another, is still this machine; a server that shares
//! this laptop's first label is the price. An address has no labels, so
//! `10.0.0.5` is not `10.0.0.7`.

use std::net::IpAddr;
use std::path::PathBuf;
use std::sync::{Mutex, PoisonError};

/// Where a pane's shell is, when that is another machine.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Remote {
    /// The host the shell named, as it named it.
    pub host: String,
    /// The directory it is in there.
    ///
    /// A path on that machine, kept for the row to print and the session
    /// file to remember, and never looked up on this one.
    pub directory: PathBuf,
}

impl Remote {
    /// What a shell's report of `directory` on `host` is: `None` when the
    /// host is this machine — no host at all, `localhost`, a loopback
    /// address, or a name this machine answers to — and the other machine
    /// otherwise.
    pub fn reported(host: Option<&str>, directory: PathBuf) -> Option<Self> {
        let host = host?;
        if is_this_machine(host) {
            return None;
        }
        Some(Self {
            host: host.to_owned(),
            directory,
        })
    }

    /// The host as a row prints it: the name up to its first dot, the way
    /// zsh's `%m` and bash's `\h` print it in a prompt — `buildbox`, not
    /// `buildbox.internal.example.com`.
    ///
    /// Whole when it is an address, which a dot does not divide into a name,
    /// and when there is nothing before the dot.
    pub fn short_host(&self) -> &str {
        short_host(&self.host)
    }
}

/// See [`Remote::short_host`].
pub(super) fn short_host(host: &str) -> &str {
    // A `user@` in front is kept, and its own dots with it: the name is what
    // comes after it.
    let name = host.rsplit_once('@').map_or(host, |(_, name)| name);
    match first_label(name) {
        Some(label) => &host[..host.len() - name.len() + label.len()],
        None => host,
    }
}

/// Every name this machine has answered to since Crook started.
static NAMES: Mutex<Vec<String>> = Mutex::new(Vec::new());

/// Whether `host` is this machine: see the module docs.
fn is_this_machine(host: &str) -> bool {
    let mut names = NAMES.lock().unwrap_or_else(PoisonError::into_inner);
    names_this_machine(host, &mut names, this_machines_names)
}

/// The rule itself, with the names known so far and the way to ask for the
/// current ones handed in, so it can be tested without renaming a machine.
///
/// Names are compared without regard to case, which is how a resolver
/// compares them.
pub(super) fn names_this_machine(
    host: &str,
    known: &mut Vec<String>,
    ask: impl FnOnce() -> Vec<String>,
) -> bool {
    let same = |name: &String| same_machine(name, host);
    if host.eq_ignore_ascii_case("localhost") || is_loopback(host) || known.iter().any(same) {
        return true;
    }
    for name in ask() {
        if !known.contains(&name) {
            known.push(name);
        }
    }
    known.iter().any(same)
}

/// Whether two names are one machine's: the same name, or the same first
/// label — see the module docs for why the second.
fn same_machine(one: &str, other: &str) -> bool {
    one.eq_ignore_ascii_case(other)
        || first_label(one)
            .zip(first_label(other))
            .is_some_and(|(one, other)| one.eq_ignore_ascii_case(other))
}

/// Whether `host` is an address that means this machine to whoever says it:
/// a loopback one, or the unspecified one a server is told to listen on —
/// `HOST=0.0.0.0` is a common line in a startup file for a dev server.
fn is_loopback(host: &str) -> bool {
    let address = host
        .strip_prefix('[')
        .and_then(|inner| inner.strip_suffix(']'))
        .unwrap_or(host);
    address
        .parse::<IpAddr>()
        .is_ok_and(|address| address.is_loopback() || address.is_unspecified())
}

/// A name up to its first dot, or `None` for an address, which a dot does
/// not divide into a name and a domain.
fn first_label(name: &str) -> Option<&str> {
    if name.starts_with('[') || name.parse::<IpAddr>().is_ok() {
        return None;
    }
    name.split('.').next().filter(|label| !label.is_empty())
}

/// The names this machine answers to right now.
///
/// The system's own name, which is what every shell on Unix reads. On Windows
/// that is the DNS name, and the shells there — Git Bash's `$HOSTNAME` above
/// all — may say the NetBIOS one instead, which is `COMPUTERNAME`: shorter,
/// upper case, and the same machine.
///
/// And `$HOST` and `$HOSTNAME` as Crook was started with them. Every shell
/// Crook starts inherits both, and an integration that writes one into its
/// URL writes whatever they hold: the name the machine had at login, before
/// the network renamed it, or a compiler's target triple that a conda
/// environment exported as `HOST`. Neither is another machine.
fn this_machines_names() -> Vec<String> {
    names_of(
        gethostname::gethostname().to_string_lossy().into_owned(),
        |variable| std::env::var(variable).ok(),
    )
}

/// [`this_machines_names`] from the system's name and the way to read
/// Crook's environment, handed in so it can be tested without setting
/// either.
pub(super) fn names_of(system: String, inherited: impl Fn(&str) -> Option<String>) -> Vec<String> {
    let variables: &[&str] = if cfg!(windows) {
        &["COMPUTERNAME", "HOST", "HOSTNAME"]
    } else {
        &["HOST", "HOSTNAME"]
    };
    let mut names = vec![system];
    names.extend(variables.iter().filter_map(|variable| inherited(variable)));
    names.retain(|name| !name.is_empty());
    names
}

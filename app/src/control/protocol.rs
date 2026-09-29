//! What crosses the control socket: one JSON object a line, each way.
//!
//! Nothing here touches a socket. A line is read into a [`Request`] or a
//! [`Refusal`], and an answer is written out as a [`Reply`], so that
//! everything a client can get wrong — not JSON, no version, a verb this
//! window has never heard of, an argument a verb does not take — is decided in
//! one place that a test can reach without a connection, and the connection
//! only ever moves bytes.
//!
//! See "The window answers" in `docs/architecture.md` for the shape, and why
//! a verb, once shipped, is only ever added to.

use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

/// The version of the protocol this window speaks, carried on every reply.
pub const VERSION: u64 = 1;

/// The oldest request version this window still answers.
///
/// Equal to [`VERSION`] until a version changes what a field means; then it
/// is what says for how long the old meaning is kept.
pub const MIN_VERSION: u64 = 1;

/// The longest request line a window reads, in bytes, not counting its
/// newline.
///
/// A request is a verb and a handful of arguments. Sixty-four kilobytes is
/// several hundred times that, and it is what bounds the memory a client that
/// never sends a newline can make a connection hold.
pub const MAX_LINE: usize = 64 * 1024;

/// Why a request was not answered, as the `code` of a refusal.
///
/// Strings rather than an enum on the wire, because a client reads the codes
/// of windows newer than itself and must not fail to parse one it has not
/// heard of.
pub mod code {
    /// The line is not a JSON object carrying a version and a verb.
    pub const BAD_REQUEST: &str = "bad-request";
    /// The line is longer than [`MAX_LINE`](super::MAX_LINE).
    pub const TOO_LONG: &str = "too-long";
    /// The verb is not one this window knows.
    pub const UNKNOWN_VERB: &str = "unknown-verb";
    /// The request and the window cannot agree on a version.
    pub const VERSION: &str = "version";
    /// The window is answering as many connections as it takes at once.
    pub const BUSY: &str = "busy";
    /// The window did not answer in the time the connection had for it.
    ///
    /// That time ends a little short of the connection's deadline, not at it,
    /// so that the refusal can still be written: a window that answers in the
    /// last moments before the deadline is refused as this all the same.
    pub const TIMEOUT: &str = "timeout";
    /// The window closed before it answered.
    pub const GONE: &str = "gone";
    /// The request carries no token of a pane in this window, and the verb
    /// is one only a pane may ask.
    pub const UNAUTHORIZED: &str = "unauthorized";
    /// The caller already has as many tabs open on its behalf as it may.
    pub const BUDGET: &str = "budget";
    /// The caller has been refused so many times in a row that the window has
    /// stopped answering it, until its pane closes.
    pub const TOO_MANY_REFUSALS: &str = "too-many-refusals";
    /// The shell a new pane runs is one whose quoting Crook has not proven,
    /// so a command cannot be typed into it as the words it was given.
    pub const UNSUPPORTED_SHELL: &str = "unsupported-shell";
    /// The window tried and could not: git refused the worktree, the pane has
    /// no directory to find a repository in.
    pub const FAILED: &str = "failed";
}

/// What a request can ask for.
///
/// Two verbs: one that reads and one that opens a tab. Every one is a promise
/// that is hard to take back, which is why each is added on purpose and none
/// by a pattern.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Verb {
    /// Every pane of the window, as a list of [`PaneEntry`].
    PaneList,
    /// A tab opened beside the caller's, running a command: see [`NewTab`].
    /// Answered with an [`Opened`].
    TabNew(NewTab),
}

impl Verb {
    /// Every verb's name, in the order a refusal names them.
    pub const NAMES: [&str; 2] = ["pane.list", "tab.new"];

    /// Its name on the wire: a noun and a verb, joined by a dot.
    pub fn name(&self) -> &'static str {
        match self {
            Self::PaneList => Self::NAMES[0],
            Self::TabNew(_) => Self::NAMES[1],
        }
    }

    /// Its arguments on the wire, or `None` for a verb that takes none.
    fn args(&self) -> Option<Value> {
        match self {
            Self::PaneList => None,
            Self::TabNew(asked) => {
                Some(serde_json::to_value(asked).expect("a new tab is strings and a boolean"))
            }
        }
    }

    /// How long the window may take over this one, when that is longer than a
    /// connection is otherwise given.
    ///
    /// `None` for everything answered between two frames. A tab in a new
    /// worktree waits for git to check a working tree out and run the
    /// repository's own `post-checkout` hook, which the worktree module lets
    /// run for two minutes; a connection that gave up at five seconds would
    /// print `timeout` about a tab that then opened. So that one is given the
    /// worktree's own read and write budgets, and a margin over them.
    pub fn patience(&self) -> Option<Duration> {
        match self {
            Self::TabNew(NewTab {
                worktree: Some(_), ..
            }) => Some(
                crate::git::worktree::READ_TIMEOUT
                    + crate::git::worktree::WRITE_TIMEOUT
                    + Duration::from_secs(10),
            ),
            _ => None,
        }
    }
}

/// What `tab.new` asks for: a command, and where to run it.
///
/// Read with unknown fields refused rather than ignored. A newer client's
/// argument this window has never heard of could change what the tab is —
/// ignoring a `worktree` would open the agent in the caller's own checkout —
/// and a refusal naming it is the answer that cannot do harm.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NewTab {
    /// The command, as the words it is made of. The window joins them for
    /// the new pane's shell, quoting each, so every word arrives as itself.
    pub command: Vec<String>,
    /// A branch to make a worktree on, from the caller's `HEAD`, for the tab
    /// to open in; the caller's own directory when there is none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub worktree: Option<String>,
    /// Whether the tab joins the caller's group, making one of the two when
    /// the caller's tab is in none.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub in_my_group: bool,
    /// What to call the tab.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
}

/// What `tab.new` answers: the tab that opened.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Opened {
    /// The new pane's number: the `CROOK_PANE_ID` its shell has, and the
    /// `pane_id` `pane.list` lists it by.
    pub pane_id: u64,
    /// The number of the tab holding it.
    pub tab_id: u64,
    /// Where its shell starts: the worktree, when one was made.
    #[serde(default)]
    pub cwd: Option<String>,
}

/// One request: what it asks, and who asks it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Request {
    /// What it asks.
    pub verb: Verb,
    /// The asking pane's `CROOK_TOKEN`, when it sent one. Whose it is, if it
    /// is anybody's, is the window's to say: see `control::spawn`.
    pub token: Option<String>,
}

/// Why a request was not answered: a code a script can branch on, and a
/// sentence a person can read.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Refusal {
    /// One of [`code`]'s, or one a newer window added.
    pub code: String,
    /// What went wrong, written for whoever typed the command.
    pub message: String,
}

impl Refusal {
    /// A refusal with this code and this sentence.
    pub fn new(code: &str, message: impl Into<String>) -> Self {
        Self {
            code: code.to_owned(),
            message: message.into(),
        }
    }
}

/// One answer, as it goes back down the connection.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Reply {
    /// The version the window speaks. On every reply, refusals included, so a
    /// client can tell which window it reached before it reads anything else.
    pub v: u64,
    /// The request's own `id`, echoed, when it had one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<Value>,
    /// Whether this is an answer rather than a refusal.
    pub ok: bool,
    /// The answer, when there is one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
    /// The refusal, when there is one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<Refusal>,
}

impl Reply {
    /// The answer to the request `id` named.
    pub fn answered(id: Option<Value>, result: Value) -> Self {
        Self {
            v: VERSION,
            id,
            ok: true,
            result: Some(result),
            error: None,
        }
    }

    /// The refusal of the request `id` named.
    pub fn refused(id: Option<Value>, refusal: Refusal) -> Self {
        Self {
            v: VERSION,
            id,
            ok: false,
            result: None,
            error: Some(refusal),
        }
    }

    /// The reply as it is written: one line of JSON and its newline.
    pub fn line(&self) -> String {
        let mut line =
            serde_json::to_string(self).expect("a reply is JSON values and strings, which encode");
        line.push('\n');
        line
    }
}

/// One pane, as `pane.list` answers for it.
///
/// What the panel shows about the pane and nothing it does not: no output, no
/// input, nothing a person would have to be asked about.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PaneEntry {
    /// The pane's number: the same one its shell has in `CROOK_PANE_ID`, so a
    /// program in a pane can find its own entry.
    pub pane_id: u64,
    /// The number of the tab holding it. Two entries with one are the two
    /// halves of a split.
    pub tab_id: u64,
    /// What the pane's row is called: what somebody named it, else what the
    /// program in it calls its work, else what it is running, else its
    /// directory — the rule the row itself follows.
    pub title: String,
    /// What the tab's row is called when a row stands for the whole tab: its
    /// focused pane's title.
    pub tab_title: String,
    /// The group the tab is in, by the name on its heading.
    #[serde(default)]
    pub group: Option<String>,
    /// Whether this is the pane a person is working in: the active tab's
    /// focused pane. At most one entry says so.
    pub focused: bool,
    /// What the agent in it last said: `idle`, `running`, `needs-input` or
    /// `failed` — the four words `crook --agent` takes.
    pub status: String,
    /// What the agent said it is waiting for, while it waits.
    #[serde(default)]
    pub message: Option<String>,
    /// Where the pane is working, as its shell last reported it.
    #[serde(default)]
    pub cwd: Option<String>,
    /// The branch there, or the short sha of a detached head, once git has
    /// been asked.
    #[serde(default)]
    pub branch: Option<String>,
}

/// The line a client sends to ask `verb`, carrying `token` when it has one.
pub fn request_line(verb: &Verb, token: Option<&str>) -> String {
    let mut request = Map::new();
    request.insert("v".to_owned(), VERSION.into());
    request.insert("verb".to_owned(), verb.name().into());
    if let Some(args) = verb.args() {
        request.insert("args".to_owned(), args);
    }
    if let Some(token) = token {
        request.insert("token".to_owned(), token.into());
    }
    format!("{}\n", Value::Object(request))
}

/// Reads one request line into what it asks for.
///
/// Either way the request's `id` comes back with the answer, when the line was
/// an object that had one, so that a refusal can be matched to its request as
/// surely as an answer can.
pub fn read(line: &[u8]) -> (Option<Value>, Result<Request, Refusal>) {
    let Ok(Value::Object(request)) = serde_json::from_slice::<Value>(line) else {
        return (None, Err(malformed("the line is not a JSON object")));
    };
    let id = request.get("id").cloned();
    (id, request_of(&request))
}

/// What a request object asks for, once its versions agree with this
/// window's.
fn request_of(request: &Map<String, Value>) -> Result<Request, Refusal> {
    let Some(version) = request.get("v").and_then(Value::as_u64) else {
        return Err(malformed("a request carries its protocol version as `v`"));
    };
    if version < MIN_VERSION {
        return Err(Refusal::new(
            code::VERSION,
            format!(
                "this Crook answers protocol versions {MIN_VERSION} to {VERSION}, and the \
                 request was written for {version}"
            ),
        ));
    }
    // Newer than this window is fine on its own: versions add, so a request
    // written later still means what it says here, unless it says it needs
    // more than this window has.
    match request.get("min_version") {
        None => {}
        Some(needed) => match needed.as_u64() {
            Some(needed) if needed <= VERSION => {}
            Some(needed) => {
                return Err(Refusal::new(
                    code::VERSION,
                    format!(
                        "this Crook speaks protocol version {VERSION}, and the request needs \
                         {needed}; update Crook"
                    ),
                ));
            }
            None => return Err(malformed("`min_version` is a whole number")),
        },
    }

    let token = match request.get("token") {
        None => None,
        Some(Value::String(token)) => Some(token.clone()),
        Some(_) => return Err(malformed("`token` is a string")),
    };
    let Some(name) = request.get("verb").and_then(Value::as_str) else {
        return Err(malformed("a request names its `verb`"));
    };
    let args = request.get("args");
    let verb = match name {
        "pane.list" => match args {
            None => Verb::PaneList,
            Some(Value::Object(args)) if args.is_empty() => Verb::PaneList,
            Some(_) => return Err(malformed("`pane.list` takes no `args`")),
        },
        "tab.new" => {
            let Some(args) = args else {
                return Err(malformed("`tab.new` needs `args` naming its `command`"));
            };
            let asked = NewTab::deserialize(args)
                .map_err(|error| malformed(&format!("`tab.new`'s args: {error}")))?;
            Verb::TabNew(asked)
        }
        name => {
            return Err(Refusal::new(
                code::UNKNOWN_VERB,
                format!(
                    "this Crook does not know the verb {name:?}; it knows {}",
                    Verb::NAMES.join(", ")
                ),
            ));
        }
    };
    Ok(Request { verb, token })
}

/// A `bad-request` refusal that says what a request looks like.
fn malformed(what: &str) -> Refusal {
    Refusal::new(
        code::BAD_REQUEST,
        format!(
            "{what}: a request is one JSON object a line, like {{\"v\":1,\"verb\":\"pane.list\"}}"
        ),
    )
}

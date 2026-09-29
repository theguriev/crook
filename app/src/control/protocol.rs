//! What crosses the control socket: one JSON object a line, each way.
//!
//! Nothing here touches a socket. A line is read into a [`Verb`] or a
//! [`Refusal`], and an answer is written out as a [`Reply`], so that
//! everything a client can get wrong — not JSON, no version, a verb this
//! window has never heard of — is decided in one place that a test can reach
//! without a connection, and the connection only ever moves bytes.
//!
//! See "The window answers" in `docs/architecture.md` for the shape, and why
//! a verb, once shipped, is only ever added to.

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
}

/// What a request can ask for.
///
/// One verb, and read-only. Every one after it is a promise that is hard to
/// take back, which is why each is added on purpose and none by a pattern.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Verb {
    /// Every pane of the window, as a list of [`PaneEntry`].
    PaneList,
}

impl Verb {
    /// Every verb, in the order a refusal names them.
    pub const ALL: [Verb; 1] = [Verb::PaneList];

    /// The verb a request names, if this window knows it.
    pub fn named(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|verb| verb.name() == name)
    }

    /// Its name on the wire: a noun and a verb, joined by a dot.
    pub fn name(self) -> &'static str {
        match self {
            Self::PaneList => "pane.list",
        }
    }
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

/// The line a client sends to ask `verb`.
pub fn request_line(verb: Verb) -> String {
    format!(
        "{}\n",
        serde_json::json!({ "v": VERSION, "verb": verb.name() })
    )
}

/// Reads one request line into the verb it asks for.
///
/// Either way the request's `id` comes back with the answer, when the line was
/// an object that had one, so that a refusal can be matched to its request as
/// surely as an answer can.
pub fn read(line: &[u8]) -> (Option<Value>, Result<Verb, Refusal>) {
    let Ok(Value::Object(request)) = serde_json::from_slice::<Value>(line) else {
        return (None, Err(malformed("the line is not a JSON object")));
    };
    let id = request.get("id").cloned();
    (id, verb_of(&request))
}

/// The verb a request object asks for, once its versions agree with this
/// window's.
fn verb_of(request: &Map<String, Value>) -> Result<Verb, Refusal> {
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

    let Some(name) = request.get("verb").and_then(Value::as_str) else {
        return Err(malformed("a request names its `verb`"));
    };
    Verb::named(name).ok_or_else(|| {
        let known: Vec<&str> = Verb::ALL.iter().map(|verb| verb.name()).collect();
        Refusal::new(
            code::UNKNOWN_VERB,
            format!(
                "this Crook does not know the verb {name:?}; it knows {}",
                known.join(", ")
            ),
        )
    })
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

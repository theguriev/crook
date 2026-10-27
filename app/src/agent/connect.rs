//! Connecting an agent to Crook: the commands that install or update Crook's
//! plugin for it, and what the agent says about the copy it has.
//!
//! Everything here is the agent's own command line. Crook never writes a file
//! of an agent's: [`line()`] is what a press types into a new tab, where the
//! person sees every word of it run and what it printed, and [`look`] runs
//! the agent's own read-only list — `claude plugin list --json` — to say
//! whether the plugin is there, and how old it is. Nothing here is run
//! unasked: the look runs when the person opens the Agents page, and the
//! line only when they press Connect or Update. See
//! `crate::plugins::agents` for the page and the hint, and
//! `Workspace::connect_agent` for the tab.
//!
//! # How old a plugin is
//!
//! Each manifest under `packaging/claude-code` carries a `version`, which
//! `script/release` sets to the release it cuts, and which a test holds to
//! this crate's. Claude Code names the copy it installed by that version —
//! "the `version` field in the plugin's manifest comes first", and a pinned
//! version is updated only when it changes, which is once a release — so a
//! plugin older than this Crook is one whose version is older than
//! `CARGO_PKG_VERSION`. A copy installed before the manifests had a version
//! is named by the commit it came from, twelve hex digits, and is older than
//! any of them.

use std::ffi::OsStr;
use std::io::Read as _;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::{Duration, Instant};

use serde_json::Value;

use super::{AGENTS, SearchPath};

/// The plugin's id in every agent's list: the `crook` plugin from the `crook`
/// marketplace, the names `.claude-plugin/marketplace.json` gives.
pub const PLUGIN_ID: &str = "crook@crook";

/// How long the look waits for an agent to list its plugins before it says it
/// could not ask. Claude Code takes about a second; a cold npm shim can take
/// several.
const LOOK_TIMEOUT: Duration = Duration::from_secs(20);

/// What a press asks for.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Step {
    /// Add the marketplace and install the plugin.
    Connect,
    /// Bring the marketplace and the plugin up to date.
    Update,
}

/// The agents Crook ships a plugin for, by name, in the table's order.
pub fn connectable() -> impl Iterator<Item = &'static str> {
    AGENTS
        .iter()
        .filter(|agent| !agent.plugin.is_empty())
        .map(|agent| agent.name)
}

/// Whether Crook ships a plugin for `agent`.
pub fn is_connectable(agent: &str) -> bool {
    connectable().any(|name| name == agent)
}

/// What the plugin gives `agent`, for the hint that offers it: the end of
/// "Connect Claude Code to Crook — …".
pub fn gives(agent: &str) -> &'static str {
    match agent {
        "claude" => "status, answers from the card, tabs from the chat",
        "copilot" => "status, and tabs from the chat",
        _ => "its status on the tab's row",
    }
}

/// The commands a press runs for `agent`, as the agent's own documentation
/// spells them.
pub fn commands(agent: &str, step: Step) -> &'static [&'static str] {
    let Some(known) = AGENTS.iter().find(|known| known.name == agent) else {
        return &[];
    };
    match step {
        Step::Connect => known.plugin,
        Step::Update => known.plugin_update,
    }
}

/// The line a press types into its tab: [`commands`], each naming the program
/// as `program` does — its name, or its path where only that finds it — and
/// joined so that every one runs and the line's status is the last one's.
///
/// Every one runs because the first is the marketplace's, and an agent that
/// has it already may say so with a failure — Copilot CLI does — where the
/// install after it is still the one that matters. The last is the install
/// or the update, and its status is what the tab's block says and what the
/// Agents page reports. Joined with `;` for every shell Crook integrates
/// with and PowerShell, and with `&` for cmd.exe, which reads `;` as a word.
pub fn line(agent: &str, step: Step, program: &str, shell: &OsStr) -> Option<String> {
    let commands = commands(agent, step);
    if commands.is_empty() {
        return None;
    }
    let separator = match program_name(shell).as_str() {
        "cmd" => " & ",
        _ => "; ",
    };
    let named: Vec<String> = commands
        .iter()
        .map(|command| match command.strip_prefix(agent) {
            Some(rest) => format!("{program}{rest}"),
            None => (*command).to_owned(),
        })
        .collect();
    Some(named.join(separator))
}

/// What the tab a press opens is called.
pub fn tab_title(agent: &str, step: Step) -> String {
    let program = super::program(agent).unwrap_or(agent);
    match step {
        Step::Connect => format!("Connect {program}"),
        Step::Update => format!("Update Crook\u{2019}s {program} plugin"),
    }
}

/// A shell's program name, lower case and without an extension: `cmd` for
/// `C:\Windows\System32\cmd.exe`.
///
/// Split on both slashes by hand rather than by [`Path`], whose separators
/// are this platform's: a Windows path read anywhere is the same shell.
fn program_name(shell: &OsStr) -> String {
    let shell = shell.to_string_lossy();
    let file = shell.rsplit(['/', '\\']).next().unwrap_or_default();
    let stem = file.rsplit_once('.').map_or(file, |(stem, _)| stem);
    stem.to_ascii_lowercase()
}

/// A connectable agent installed on this machine: where its program is, and
/// how a line names it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Located {
    /// The agent, by its name in the table.
    pub agent: &'static str,
    /// The program's file, which [`look`] runs.
    pub file: PathBuf,
    /// The word a line names it by: its name where a pane's shell finds it by
    /// that, its path where it was found only where its installer put it.
    pub typed: String,
}

/// The connectable agents found on `search`, as [`super::found_on`] finds
/// them, with each one's file.
///
/// One whose path a shell would not read as one word is left out, as
/// `found_on` leaves its path out: a line that named it some other way would
/// run something else.
pub fn located(search: &SearchPath) -> Vec<Located> {
    super::found_on(search)
        .into_iter()
        .filter(|found| is_connectable(found.agent))
        .filter_map(|found| {
            let (directories, typed) = match &found.path {
                Some(path) => (&search.installers, path.clone()),
                None => (&search.by_name, found.agent.to_owned()),
            };
            let file = directories
                .iter()
                .find_map(|directory| program_file(directory, found.agent))?;
            Some(Located {
                agent: found.agent,
                file,
                typed,
            })
        })
        .collect()
}

/// How a line names `located`'s program to `shell`: by its absolute path,
/// quoted where it needs it, for the shells whose quoting Crook speaks — sh,
/// bash, zsh, fish and their kin — and as [`Located::typed`] for any other.
///
/// By its path, because the name is whatever the person's shell makes of
/// it: an alias, a function or a fish abbreviation called `claude` — one
/// that adds flags of its own, say — or another `claude` earlier on the
/// pane's `PATH` than the one this page asked. The path is the program the
/// page asked, and the one that runs.
pub fn typed_as(located: &Located, shell: &OsStr) -> String {
    let posix = matches!(
        program_name(shell).as_str(),
        "sh" | "bash" | "zsh" | "fish" | "dash" | "ksh" | "mksh" | "ash"
    );
    match (posix, located.file.to_str()) {
        (true, Some(path)) if super::plain(path) => path.to_owned(),
        #[cfg(not(windows))]
        (true, Some(path)) => crate::plugins::wasm::quote(path),
        _ => located.typed.clone(),
    }
}

/// The file in `directory` a shell would run for `name`, if there is one.
#[cfg(unix)]
fn program_file(directory: &Path, name: &str) -> Option<PathBuf> {
    super::runnable(directory, name).then(|| directory.join(name))
}

/// See the other one: the first extension `PATHEXT` names that is there.
#[cfg(not(unix))]
fn program_file(directory: &Path, name: &str) -> Option<PathBuf> {
    let extensions = std::env::var("PATHEXT")
        .ok()
        .filter(|extensions| !extensions.is_empty())
        .unwrap_or_else(|| String::from(".COM;.EXE;.BAT;.CMD"));
    extensions
        .split(';')
        .filter(|extension| !extension.is_empty())
        .map(|extension| directory.join(format!("{name}{extension}")))
        .find(|file| file.is_file())
}

/// What an agent's list says of Crook's plugin.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Listed {
    /// Not installed.
    Missing,
    /// Installed, with the version the agent names its copy by, and whether
    /// it is turned on.
    Installed {
        version: Option<String>,
        enabled: bool,
    },
}

/// Reads an agent's `plugin list --json`, or `None` for output that is not
/// one.
///
/// The three agents print three shapes — Claude Code an array of `id`s,
/// Codex an object whose `installed` is an array of `pluginId`s, Copilot an
/// array of `name`s beside `marketplace`s — and each is read by whichever of
/// those it has, so a field one of them adds later changes nothing.
pub fn read_list(json: &str) -> Option<Listed> {
    let value: Value = serde_json::from_str(json.trim()).ok()?;
    let entries = match &value {
        Value::Array(entries) => entries,
        Value::Object(object) => object.get("installed")?.as_array()?,
        _ => return None,
    };
    let entry = entries.iter().find(|entry| id_of(entry) == Some(PLUGIN_ID));
    let Some(entry) = entry else {
        return Some(Listed::Missing);
    };
    // Codex lists what a marketplace offers beside what is installed when
    // asked to, each with its own `installed`.
    if entry.get("installed").and_then(Value::as_bool) == Some(false) {
        return Some(Listed::Missing);
    }
    Some(Listed::Installed {
        version: entry
            .get("version")
            .and_then(Value::as_str)
            .map(str::to_owned),
        enabled: entry.get("enabled").and_then(Value::as_bool) != Some(false),
    })
}

/// An entry's `plugin@marketplace`, however its agent spells it.
fn id_of(entry: &Value) -> Option<&str> {
    let text = |key: &str| entry.get(key).and_then(Value::as_str);
    if let Some(id) = text("id").or_else(|| text("pluginId")) {
        return Some(id);
    }
    match (text("name"), text("marketplace")) {
        (Some("crook"), Some("crook")) => Some(PLUGIN_ID),
        _ => None,
    }
}

/// Where an agent's copy of the plugin stands beside this Crook.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Standing {
    /// Not installed.
    Missing,
    /// Installed, turned on, and as new as this Crook — or loaded in place
    /// from a checkout, which is as new as the checkout.
    Connected { version: Option<String> },
    /// Installed and older than this Crook: `version` is what it says it is.
    Behind { version: String },
    /// Installed and turned off in the agent.
    Off,
}

/// Where `listed` stands beside a Crook of version `running`.
pub fn standing(listed: &Listed, running: &str) -> Standing {
    let Listed::Installed { version, enabled } = listed else {
        return Standing::Missing;
    };
    if !enabled {
        return Standing::Off;
    }
    let Some(said) = version else {
        return Standing::Connected { version: None };
    };
    let behind = match (numbers(said), numbers(running)) {
        (Some(installed), Some(running)) => installed < running,
        // Named by the commit it came from, from before the manifests had a
        // version: older than every release that has one.
        _ => is_a_commit(said),
    };
    match behind {
        true => Standing::Behind {
            version: said.clone(),
        },
        false => Standing::Connected {
            version: Some(said.clone()),
        },
    }
}

/// Whether `version` is a release's, `0.1.39`, which a copy can be compared
/// by: not `local`, `unknown` or a commit.
pub fn is_a_version(version: &str) -> bool {
    numbers(version).is_some()
}

/// `0.1.39` as three numbers, or `None` for what is not a version.
fn numbers(version: &str) -> Option<[u64; 3]> {
    let core = version.trim().trim_start_matches('v');
    let core = core.split(['-', '+']).next()?;
    let mut parts = core.split('.').map(|part| part.parse::<u64>().ok());
    let found = [parts.next()??, parts.next()??, parts.next()??];
    parts.next().is_none().then_some(found)
}

/// Whether `version` is a commit's short hash, which is how an agent names a
/// copy whose manifest says no version.
fn is_a_commit(version: &str) -> bool {
    (7..=40).contains(&version.len()) && version.bytes().all(|byte| byte.is_ascii_hexdigit())
}

/// Asks the agent at `file` what it has installed: its own `plugin list
/// --json`, run in `home` with nothing on stdin, and given
/// [`LOOK_TIMEOUT`] to answer.
///
/// Read-only, and the agent's own: Crook opens none of its files. It runs
/// on the pool. In `home` rather than wherever Crook was started, because
/// that is where the commands a press runs are run, and an agent's answer
/// depends on where it is asked: Claude Code lists a project's own
/// `enabledPlugins` beside the person's, and a mise shim refuses to run at
/// all in a directory whose `mise.toml` it was never told to trust.
/// `environment` is laid over Crook's own, for a test's scratch home.
pub fn look(file: &Path, home: &Path, environment: &[(String, String)]) -> Result<Listed, String> {
    look_within(file, home, environment, LOOK_TIMEOUT)
}

/// [`look`], given `patience` rather than [`LOOK_TIMEOUT`].
///
/// The whole of it is under the one deadline: the agent's exit and the end
/// of its output both. A wrapper — an npm shim, a mise shim, a script — can
/// exit while something it started holds the output open, and a wait for
/// that end with no deadline was a look that never answered and a worker on
/// the pool held with it. So the agent runs in a process group of its own,
/// and a look that runs out of time ends the whole group, whatever the
/// agent started, which is also what closes the output for the thread
/// reading it. On Windows there is no group to end: the agent is killed,
/// what it started may live on, and the look answers at the deadline all
/// the same.
fn look_within(
    file: &Path,
    home: &Path,
    environment: &[(String, String)],
    patience: Duration,
) -> Result<Listed, String> {
    let program = file
        .to_str()
        .ok_or_else(|| format!("its path is not UTF-8: {}", file.display()))?;
    let mut command = crate::process::command(program);
    command
        .args(["plugin", "list", "--json"])
        .current_dir(home)
        .envs(environment.iter().map(|(key, value)| (key, value)))
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt as _;
        command.process_group(0);
    }
    let mut child = command
        .spawn()
        .map_err(|error| format!("could not start {}: {error}", file.display()))?;
    let deadline = Instant::now() + patience;
    let late = || format!("it did not answer in {} seconds", patience.as_secs());

    // Read beside the wait, so a list longer than a pipe holds cannot keep
    // the agent from exiting; handed back over a channel, so the read has the
    // deadline the wait has.
    let mut stdout = child.stdout.take().ok_or("no output to read")?;
    let (said_tx, said_rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let mut said = String::new();
        let _ = stdout.read_to_string(&mut said);
        let _ = said_tx.send(said);
    });

    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if Instant::now() >= deadline => {
                end_the_group(&mut child);
                return Err(late());
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(25)),
            Err(error) => {
                end_the_group(&mut child);
                return Err(error.to_string());
            }
        }
    };
    let said = match said_rx.recv_timeout(deadline.saturating_duration_since(Instant::now())) {
        Ok(said) => said,
        Err(_) => {
            end_the_group(&mut child);
            return Err(late());
        }
    };
    if !status.success() {
        return Err(match status.code() {
            Some(code) => format!("its plugin list ended with status {code}"),
            None => String::from("its plugin list was killed"),
        });
    }
    read_list(&said).ok_or_else(|| String::from("its plugin list said something Crook cannot read"))
}

/// Ends the look's agent and, on Unix, everything in its process group.
///
/// By the shell's own `kill`, since nothing in this crate binds the system
/// call: `kill -KILL -- -<group>`, the group being the agent's own id, as
/// [`look_within`] started it. The group outlives an agent that has already
/// exited for as long as anything it started is still in it.
fn end_the_group(child: &mut std::process::Child) {
    #[cfg(unix)]
    {
        let group = format!("-{}", child.id());
        let _ = crate::process::command("/bin/sh")
            .args(["-c", "kill -KILL -- \"$0\"", &group])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
    }
    let _ = child.kill();
    let _ = child.wait();
}

/// Whether Claude Code updates the `crook` marketplace by itself, as far as
/// its files say: `autoUpdate` on the marketplace's entry in `settings.json`
/// first, then in `plugins/known_marketplaces.json`, under `config` — Claude
/// Code's configuration directory. Read, never written.
///
/// The order is Claude Code's own ("Which marketplaces and plugins
/// auto-update", in its plugin loading reference), and a marketplace like
/// this one is off until a person turns it on. Claude Code has no command
/// that turns it on, nor one that says whether it is.
pub fn claude_updates_by_itself(config: &Path) -> bool {
    let read = |path: PathBuf| -> Option<Value> {
        serde_json::from_str(&std::fs::read_to_string(path).ok()?).ok()
    };
    let said = |value: Option<Value>, pointer: &str| {
        value.and_then(|value| value.pointer(pointer).and_then(Value::as_bool))
    };
    said(
        read(config.join("settings.json")),
        "/extraKnownMarketplaces/crook/autoUpdate",
    )
    .or_else(|| {
        said(
            read(config.join("plugins").join("known_marketplaces.json")),
            "/crook/autoUpdate",
        )
    })
    .unwrap_or(false)
}

/// Claude Code's configuration directory: `CLAUDE_CONFIG_DIR`, or `.claude`
/// in the home directory.
#[cfg_attr(test, allow(dead_code))]
pub fn claude_config() -> Option<PathBuf> {
    std::env::var_os("CLAUDE_CONFIG_DIR")
        .filter(|directory| !directory.is_empty())
        .map(PathBuf::from)
        .or_else(|| dirs::home_dir().map(|home| home.join(".claude")))
}

/// What a person adds to Claude Code's `settings.json` to have it update the
/// plugin by itself — the entry `claude plugin marketplace add` wrote, with
/// `autoUpdate` beside its source — for the Agents page to print.
pub const CLAUDE_AUTO_UPDATE: &str = "\"extraKnownMarketplaces\": { \"crook\": { \"source\": \
{ \"source\": \"github\", \"repo\": \"theguriev/crook\" }, \"autoUpdate\": true } }";

#[cfg(test)]
#[path = "connect_tests.rs"]
mod tests;

//! The pane's terminal, found from a process that has none of its own.
//!
//! Claude Code starts every command hook in a session of its own — `setsid`,
//! on macOS and Linux — so the hook, and `crook --agent` under it, has no
//! controlling terminal: `/dev/tty` is "no such device or address" there, and
//! the report that is the hook's whole job went nowhere. The pane has not
//! gone anywhere, though. The program that ran the hook is still attached to
//! it, one process up, or two when a shell stands between, so the terminal
//! the report belongs on is the controlling terminal of the program that
//! started this process's session. That is the terminal `/dev/tty` would
//! have named had the hook kept its session, which is why this is a fallback
//! and not a second way: a process with a terminal of its own writes there,
//! and nothing here runs.
//!
//! The walk goes no further than the first process outside its own session,
//! which is the program that started it, and when that program has no
//! terminal either there is none to find. Going on past it reaches a
//! terminal that is not that program's: an agent that another agent's Bash
//! tool runs — `claude -p` from a script Claude Code runs — is in a session
//! with no terminal, and its hooks would report on the outer agent's tab,
//! calling it idle while it is mid tool call; and an agent whose pane has
//! hung up, which takes the terminal from its whole session, would report
//! on the terminal Crook itself was started from. Those two report nowhere,
//! the way a hook did before any of this.
//!
//! Which terminal and session a process has is per platform —
//! `/proc/<pid>/stat` on Linux, `proc_pidinfo` and `getsid` on macOS — and
//! finding the node for a terminal is not: it is the character device under
//! `/dev` whose number that is. A process whose session was started by one
//! with no terminal — `cron`, a CI runner, a detached service — finds
//! nothing, and the report fails the way it always did.
//!
//! The same reading answers one question for `tmux`: whether a terminal tmux
//! names is the one a report from here goes to, which is what tells a process
//! in tmux's pane from one that only inherited tmux's variables. And one for
//! an agent's question — see [`super::ask`] — which names its pane by the
//! number of the terminal a report from here would go to, and opens nothing:
//! [`device`].
//!
//! The window reads the same table the other way, for a question it is
//! handed: which processes the one that asked descends from — [`lineage`] —
//! which is what says whether it is a process of the pane it named.

use std::fs::{self, File, OpenOptions};
use std::io;
use std::os::unix::fs::{FileTypeExt as _, MetadataExt as _};
use std::path::{Path, PathBuf};

/// How many ancestors are asked before giving up.
///
/// The pane is one or two processes up from a hook, and a chain this long is
/// a loop in something this read rather than a process tree anybody built.
const MOST_ANCESTORS: usize = 64;

/// What the walk reads about one process.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Process {
    /// Its parent's pid.
    parent: u32,
    /// Its session: the pid of the process that started the session, which
    /// is what the walk stops at the edge of.
    session: u32,
    /// Its controlling terminal's device number, when it has one.
    terminal: Option<u32>,
}

/// The controlling terminal of the program that started this process's
/// session, opened for writing; `None` when that program has no terminal
/// this can name.
pub(super) fn terminal() -> Option<io::Result<File>> {
    let session = process(std::process::id())?.session;
    let device = session_terminal(session, std::os::unix::process::parent_id(), process)?;
    Some(OpenOptions::new().write(true).open(node(device)?))
}

/// Whether `path` is the terminal a report from this process goes to: its own
/// controlling terminal, or the one [`terminal`] finds when it has none.
pub(super) fn is_the_terminal(path: &Path) -> bool {
    device().is_some_and(|device| is_device(path, device))
}

/// The device number of the terminal a report from this process goes to:
/// its own controlling terminal's, or — when it has none, as a hook Claude
/// Code ran has none — that of the first process outside its session, the
/// program that ran it. `None` when neither has one.
///
/// Read off the process table and nothing else: no terminal is opened, so a
/// process that only wants to name its pane cannot write to it by mistake.
pub(super) fn device() -> Option<u32> {
    let own = process(std::process::id())?;
    own.terminal
        .or_else(|| session_terminal(own.session, std::os::unix::process::parent_id(), process))
}

/// `pid` and the processes it descends from, nearest first: at most `most`,
/// and only as far as each can be read.
///
/// Stops at the first parent it cannot read — gone, or in a namespace this
/// one cannot see into, which reads as 0 — and before pid 1's parent, which
/// is nobody.
pub(crate) fn lineage(pid: u32, most: usize) -> Vec<u32> {
    lineage_in(pid, most, process)
}

/// [`lineage`], with the process table given, so a test can say what it is.
fn lineage_in(mut pid: u32, most: usize, process: impl Fn(u32) -> Option<Process>) -> Vec<u32> {
    let mut lineage = Vec::new();
    while lineage.len() < most && pid != 0 && !lineage.contains(&pid) {
        let Some(found) = process(pid) else {
            break;
        };
        lineage.push(pid);
        if pid == 1 {
            break;
        }
        pid = found.parent;
    }
    lineage
}

/// The terminal device of the first process from `pid` up that either has
/// one or is outside `session`, as `process` reads them; `None` when that
/// process has none, or the chain cannot be read that far.
///
/// Inside `session`, a process with a terminal has the session's own, and
/// one without is a shell between — the hook's `sh`, the Bash tool's `bash`
/// — to be stepped over. The first process outside `session` is the program
/// that started it, and its terminal is the answer or there is none: going
/// on past it would be going on into somebody else's session.
fn session_terminal(
    session: u32,
    mut pid: u32,
    process: impl Fn(u32) -> Option<Process>,
) -> Option<u32> {
    for _ in 0..MOST_ANCESTORS {
        // 0 is a parent outside this process's pid namespace. 1 is asked
        // before the walk stops there, because in a container the program on
        // the terminal — `docker run -it … claude` — is pid 1.
        if pid == 0 {
            return None;
        }
        let found = process(pid)?;
        if found.terminal.is_some() || found.session != session {
            return found.terminal;
        }
        if pid == 1 {
            return None;
        }
        pid = found.parent;
    }
    None
}

/// The character device under `/dev` whose number is `device`: a
/// pseudo-terminal in `/dev/pts` on Linux, a `ttys…` in `/dev` on macOS, a
/// console's `tty…` on either.
///
/// Found by number rather than built from it, because the name a number goes
/// by is the platform's business and the number is what both ends agree on.
/// Only the low 32 bits are compared: that is all Linux's `tty_nr` and macOS's
/// `e_tdev` carry, and it is where both `st_rdev` encodings put a terminal's
/// major and minor.
fn node(device: u32) -> Option<PathBuf> {
    [("/dev/pts", ""), ("/dev", "tty")]
        .into_iter()
        .find_map(|(directory, prefix)| {
            fs::read_dir(directory)
                .ok()?
                .flatten()
                .filter(|entry| entry.file_name().to_string_lossy().starts_with(prefix))
                .map(|entry| entry.path())
                .find(|path| is_device(path, device))
        })
}

/// Whether `path` is the character device numbered `device`.
fn is_device(path: &Path, device: u32) -> bool {
    fs::metadata(path).is_ok_and(|metadata| {
        metadata.file_type().is_char_device() && metadata.rdev() as u32 == device
    })
}

/// Process `pid`'s parent, session and terminal; `None` when the process
/// cannot be read.
#[cfg(target_os = "linux")]
pub(crate) fn process(pid: u32) -> Option<Process> {
    stat_fields(&fs::read_to_string(format!("/proc/{pid}/stat")).ok()?)
}

/// The foreground process group of the terminal `pid` controls — its
/// `tpgid` — which is the pid of the group's leader; `None` when it has no
/// terminal, or cannot be read.
#[cfg(target_os = "linux")]
pub(crate) fn foreground_group(pid: u32) -> Option<u32> {
    foreground_field(&fs::read_to_string(format!("/proc/{pid}/stat")).ok()?)
}

/// The eighth field of a `/proc/<pid>/stat` line, `tpgid`, counted from the
/// last `)` as [`stat_fields`] counts; -1, no terminal, is `None`.
#[cfg(target_os = "linux")]
fn foreground_field(stat: &str) -> Option<u32> {
    let (_, rest) = stat.rsplit_once(')')?;
    // The state, the parent, the group, the session and the terminal first.
    let group: i64 = rest.split_ascii_whitespace().nth(5)?.parse().ok()?;
    u32::try_from(group).ok().filter(|group| *group != 0)
}

/// The parent, the session and the terminal out of a `/proc/<pid>/stat`
/// line: its fourth, sixth and seventh fields, `ppid`, `session` and
/// `tty_nr`, where a `tty_nr` of 0 is no terminal.
///
/// Counted from the last `)`, because the second field is the command's name
/// in parentheses and a name may hold spaces and parentheses of its own.
#[cfg(target_os = "linux")]
fn stat_fields(stat: &str) -> Option<Process> {
    let (_, rest) = stat.rsplit_once(')')?;
    let mut fields = rest.split_ascii_whitespace();
    // The state, then the parent.
    let parent = fields.nth(1)?.parse().ok()?;
    // The process group, then the session.
    let session = fields.nth(1)?.parse().ok()?;
    // The terminal: an `int` the kernel prints signed, whose bits are the
    // device number.
    let device: i32 = fields.next()?.parse().ok()?;
    Some(Process {
        parent,
        session,
        terminal: (device != 0).then_some(device as u32),
    })
}

/// `struct proc_bsdinfo` from `<sys/proc_info.h>`, field for field. A few
/// fields are read; the others are here so that those sit where the kernel
/// writes them, and the size is held below to the header's
/// `PROC_PIDTBSDINFO_SIZE`.
#[cfg(target_os = "macos")]
#[repr(C)]
struct BsdInfo {
    pbi_flags: u32,
    pbi_status: u32,
    pbi_xstatus: u32,
    pbi_pid: u32,
    pbi_ppid: u32,
    pbi_uid: u32,
    pbi_gid: u32,
    pbi_ruid: u32,
    pbi_rgid: u32,
    pbi_svuid: u32,
    pbi_svgid: u32,
    rfu_1: u32,
    pbi_comm: [u8; 16],
    pbi_name: [u8; 32],
    pbi_nfiles: u32,
    pbi_pgid: u32,
    pbi_pjobc: u32,
    e_tdev: u32,
    e_tpgid: u32,
    pbi_nice: i32,
    pbi_start_tvsec: u64,
    pbi_start_tvusec: u64,
}
#[cfg(target_os = "macos")]
const _: () = assert!(std::mem::size_of::<BsdInfo>() == 136);

// Declared rather than depended on, the way `AttachConsole` is on Windows:
// all of them are in `libSystem`, which every macOS binary links, and a few
// functions do not justify a crate with a build script. `getsid` because
// `proc_bsdinfo` carries a process group and no session.
#[cfg(target_os = "macos")]
unsafe extern "C" {
    pub(crate) fn proc_pidinfo(
        pid: std::ffi::c_int,
        flavor: std::ffi::c_int,
        arg: u64,
        buffer: *mut std::ffi::c_void,
        buffersize: std::ffi::c_int,
    ) -> std::ffi::c_int;
    fn getsid(pid: std::ffi::c_int) -> std::ffi::c_int;
}

/// `NODEV`, which is what `e_tdev` holds for a process with no controlling
/// terminal.
#[cfg(target_os = "macos")]
const NODEV: u32 = u32::MAX;

/// What `proc_pidinfo` says of process `pid` as a [`BsdInfo`], or `None`.
#[cfg(target_os = "macos")]
fn bsd_info(pid: std::ffi::c_int) -> Option<BsdInfo> {
    use std::ffi::c_int;
    use std::mem::{MaybeUninit, size_of};

    /// `PROC_PIDTBSDINFO`: the flavour of `proc_pidinfo` that answers with a
    /// [`BsdInfo`].
    const PROC_PIDTBSDINFO: c_int = 3;

    let size = size_of::<BsdInfo>() as c_int;
    let mut info = MaybeUninit::<BsdInfo>::zeroed();
    // SAFETY: the buffer is `size` bytes, and `proc_pidinfo` writes no more
    // than the size it is handed.
    let written = unsafe { proc_pidinfo(pid, PROC_PIDTBSDINFO, 0, info.as_mut_ptr().cast(), size) };
    if written != size {
        return None;
    }
    // SAFETY: zeroed and then filled, and every field is an integer, for which
    // any bits are a value.
    Some(unsafe { info.assume_init() })
}

/// Process `pid`'s parent, session and terminal; `None` when the process
/// cannot be read.
#[cfg(target_os = "macos")]
pub(crate) fn process(pid: u32) -> Option<Process> {
    let pid = std::ffi::c_int::try_from(pid).ok()?;
    let info = bsd_info(pid)?;
    // SAFETY: `getsid` takes a pid and touches no memory of this process's;
    // a pid that is gone is -1.
    let session = u32::try_from(unsafe { getsid(pid) }).ok()?;
    Some(Process {
        parent: info.pbi_ppid,
        session,
        terminal: (info.e_tdev != NODEV && info.e_tdev != 0).then_some(info.e_tdev),
    })
}

/// The foreground process group of the terminal `pid` controls — its
/// `e_tpgid` — which is the pid of the group's leader; `None` when it has no
/// terminal, or cannot be read.
#[cfg(target_os = "macos")]
pub(crate) fn foreground_group(pid: u32) -> Option<u32> {
    let info = bsd_info(std::ffi::c_int::try_from(pid).ok()?)?;
    (info.e_tdev != NODEV && info.e_tdev != 0 && info.e_tpgid != 0).then_some(info.e_tpgid)
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::*;

    /// The pane's terminal, pts/3.
    const PANE: u32 = 34819;

    /// The terminal Crook itself was started from: foot's pts/0.
    const OUTER: u32 = 34816;

    /// What [`session_terminal`] finds from `pid` up, for a process in
    /// `session`, in a process table of `(pid, parent, session, terminal)`.
    fn walk(table: &[(u32, u32, u32, Option<u32>)], session: u32, pid: u32) -> Option<u32> {
        let table: HashMap<_, _> = table
            .iter()
            .map(|&(pid, parent, session, terminal)| {
                let process = Process {
                    parent,
                    session,
                    terminal,
                };
                (pid, process)
            })
            .collect();
        session_terminal(session, pid, |pid| table.get(&pid).copied())
    }

    /// Crook, 50, on foot's terminal; the pane's shell, 100, in a session of
    /// its own on the pane; `claude`, 200, run from it.
    const PANE_AND_CLAUDE: [(u32, u32, u32, Option<u32>); 3] = [
        (50, 1, 40, Some(OUTER)),
        (100, 50, 100, Some(PANE)),
        (200, 100, 100, Some(PANE)),
    ];

    #[test]
    fn a_hook_and_a_bash_tool_command_report_on_the_pane_of_the_program_that_ran_them() {
        // A hook: `crook` under the hook's `sh`, 300, which is in a session of
        // its own below `claude`.
        let mut table = PANE_AND_CLAUDE.to_vec();
        table.push((300, 200, 300, None));
        assert_eq!(Some(PANE), walk(&table, 300, 300));
        // A command Claude Code's Bash tool runs: `crook` under `sh`, 411,
        // under the tool's `bash`, 410, which is in a session of its own.
        table.extend([(410, 200, 410, None), (411, 410, 410, None)]);
        assert_eq!(Some(PANE), walk(&table, 410, 411));
    }

    #[test]
    fn an_agent_with_no_terminal_gives_its_hooks_none_of_the_outer_agents() {
        // The pane's `claude` runs `claude -p` through its Bash tool: the
        // tool's `bash`, 410, is in a session of its own with no terminal,
        // the nested `claude`, 420, is in that session, and its hook's `sh`,
        // 430, is in one of its own below that.
        let mut table = PANE_AND_CLAUDE.to_vec();
        table.extend([
            (410, 200, 410, None),
            (420, 410, 410, None),
            (430, 420, 430, None),
        ]);
        assert_eq!(None, walk(&table, 430, 430));
    }

    #[test]
    fn a_hung_up_pane_gives_its_agents_hooks_none_of_crooks() {
        // A hangup takes the terminal from the pane's whole session, and
        // Crook itself has one: the one it was started from.
        let table = [
            (50, 1, 40, Some(OUTER)),
            (100, 50, 100, None),
            (200, 100, 100, None),
            (300, 200, 300, None),
        ];
        assert_eq!(None, walk(&table, 300, 300));
    }

    #[test]
    fn a_terminal_inside_the_session_is_the_sessions_own() {
        // `crook`'s parent kept the session's terminal where `crook` has
        // none, which is no reason to go past it to Crook's.
        let table = [
            (50, 1, 40, Some(OUTER)),
            (100, 50, 100, Some(PANE)),
            (110, 100, 100, None),
        ];
        assert_eq!(Some(PANE), walk(&table, 100, 110));
    }

    #[test]
    fn pid_1_is_asked_and_nothing_past_it_or_outside_the_namespace_is() {
        // `docker run -it … claude`: the program on the terminal is pid 1.
        let container = [(1, 0, 1, Some(PANE)), (7, 1, 7, None)];
        assert_eq!(Some(PANE), walk(&container, 7, 7));
        // pid 1 in the session and without a terminal is the end of it.
        assert_eq!(None, walk(&[(1, 0, 1, None), (7, 1, 1, None)], 1, 7));
        // A parent outside the pid namespace is 0, and is not asked.
        assert_eq!(None, walk(&[(7, 0, 7, None)], 7, 7));
        // Nor is a process that cannot be read.
        assert_eq!(None, walk(&[(7, 8, 7, None)], 7, 7));
    }

    #[test]
    fn a_hook_names_the_terminal_its_report_would_go_to_by_its_number() {
        // The walk `device` takes, from a hook in a session of its own below
        // `claude` on the pane: the pane's number, read and not opened.
        let mut table = PANE_AND_CLAUDE.to_vec();
        table.push((300, 200, 300, None));
        assert_eq!(Some(PANE), walk(&table, 300, 300));
        // And this process's own: whatever terminal the suite runs on, or
        // none, the number is the one `is_the_terminal` compares.
        let own = process(std::process::id()).expect("this process can read itself");
        if own.terminal.is_some() {
            assert_eq!(own.terminal, device());
        }
    }

    #[test]
    fn a_lineage_is_the_process_and_its_ancestors_nearest_first() {
        let table: HashMap<u32, Process> = PANE_AND_CLAUDE
            .iter()
            .map(|&(pid, parent, session, terminal)| {
                let process = Process {
                    parent,
                    session,
                    terminal,
                };
                (pid, process)
            })
            .chain([(
                1,
                Process {
                    parent: 0,
                    session: 1,
                    terminal: None,
                },
            )])
            .collect();
        let read = |pid| table.get(&pid).copied();
        assert_eq!(vec![200, 100, 50, 1], lineage_in(200, 64, read));
        assert_eq!(
            vec![200, 100],
            lineage_in(200, 2, read),
            "no more than asked"
        );
        // A parent that cannot be read ends it.
        assert_eq!(
            vec![7],
            lineage_in(7, 64, |pid| (pid == 7).then_some(Process {
                parent: 8,
                session: 7,
                terminal: None,
            }))
        );
        // A table that loops is walked once round.
        assert_eq!(
            vec![5, 6],
            lineage_in(5, 64, |pid| Some(Process {
                parent: if pid == 5 { 6 } else { 5 },
                session: 5,
                terminal: None,
            }))
        );

        // And for real: this process, then its parent.
        let real = lineage(std::process::id(), 64);
        assert_eq!(real.first(), Some(&std::process::id()));
        assert_eq!(real.get(1), Some(&std::os::unix::process::parent_id()));
    }

    #[test]
    fn a_process_reads_as_its_parent_and_its_parents_session() {
        let own = process(std::process::id()).expect("this process can read itself");
        let parent = std::os::unix::process::parent_id();
        assert_eq!(parent, own.parent);
        // Neither cargo nor a shell starts a session for what it runs, so
        // this one is its parent's.
        let theirs = process(parent).expect("this process can read its parent");
        assert_eq!(theirs.session, own.session);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn the_parent_session_and_terminal_are_counted_past_a_name_with_parentheses_in_it() {
        let process = |parent, session, terminal| Process {
            parent,
            session,
            terminal,
        };
        // `claude` on pts/7: major 136, minor 7.
        assert_eq!(
            Some(process(2982192, 2982190, Some(34823))),
            stat_fields("2982193 (claude) S 2982192 2982193 2982190 34823 2982193 4194560 0")
        );
        // A name is whatever the program set, spaces and `)` included.
        assert_eq!(
            Some(process(41, 40, Some(34823))),
            stat_fields("42 (a) b (c)) S 41 42 40 34823 42 0")
        );
        // A hook in a session of its own: no terminal.
        assert_eq!(
            Some(process(2982193, 2982218, None)),
            stat_fields("2982218 (sh) S 2982193 2982218 2982218 0 -1 4194560")
        );
        assert_eq!(None, stat_fields("2982218 (sh"));
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn the_foreground_group_is_the_eighth_field_and_none_without_a_terminal() {
        // `tmux attach` in the foreground of pts/7, its own group's leader.
        assert_eq!(
            Some(2982300),
            foreground_field("2982193 (a) b (c)) S 2982192 2982193 2982190 34823 2982300 4194560")
        );
        assert_eq!(
            None,
            foreground_field("2982218 (sh) S 2982193 2982218 2982218 0 -1 4194560")
        );
        assert_eq!(None, foreground_field("2982218 (sh) S 1 2"));
    }
}

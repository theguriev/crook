//! The pane's terminal, found from a process that has none of its own.
//!
//! Claude Code starts every command hook in a session of its own — `setsid`,
//! on macOS and Linux — so the hook, and `crook --agent` under it, has no
//! controlling terminal: `/dev/tty` is "no such device or address" there, and
//! the report that is the hook's whole job went nowhere. The pane has not
//! gone anywhere, though. The program that ran the hook is still attached to
//! it, one process up, or two when a shell stands between, so the terminal
//! the report belongs on is the controlling terminal of the nearest ancestor
//! that has one. That is the terminal `/dev/tty` would have named had the
//! hook kept its session, which is why this is a fallback and not a second
//! way: a process with a terminal of its own writes there, and nothing here
//! runs.
//!
//! Which terminal a process has is per platform — `/proc/<pid>/stat` on
//! Linux, `proc_pidinfo` on macOS — and finding the node for it is not: it is
//! the character device under `/dev` whose number that is. A process with no
//! terminal anywhere above it — `cron`, a CI runner, a detached service —
//! finds nothing, and the report fails the way it always did.

use std::fs::{self, File, OpenOptions};
use std::io;
use std::os::unix::fs::{FileTypeExt as _, MetadataExt as _};
use std::path::{Path, PathBuf};

/// How many ancestors are asked before giving up.
///
/// The pane is one or two processes up from a hook, and a chain this long is
/// a loop in something this read rather than a process tree anybody built.
const MOST_ANCESTORS: usize = 64;

/// The controlling terminal of the nearest ancestor that has one, opened for
/// writing; `None` when no ancestor has a terminal this can name.
pub(super) fn terminal() -> Option<io::Result<File>> {
    let mut pid = std::os::unix::process::parent_id();
    for _ in 0..MOST_ANCESTORS {
        // 0 is a parent outside this process's pid namespace. 1 is asked
        // before the walk stops there, because in a container the program on
        // the terminal — `docker run -it … claude` — is pid 1.
        if pid == 0 {
            return None;
        }
        let (parent, device) = process(pid)?;
        if let Some(device) = device {
            return Some(OpenOptions::new().write(true).open(node(device)?));
        }
        if pid == 1 {
            return None;
        }
        pid = parent;
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

/// Process `pid`'s parent and, when it has one, its controlling terminal's
/// device number; `None` when the process cannot be read.
#[cfg(target_os = "linux")]
fn process(pid: u32) -> Option<(u32, Option<u32>)> {
    stat_fields(&fs::read_to_string(format!("/proc/{pid}/stat")).ok()?)
}

/// The parent and the terminal out of a `/proc/<pid>/stat` line: its fourth
/// and seventh fields, `ppid` and `tty_nr`, where 0 is no terminal.
///
/// Counted from the last `)`, because the second field is the command's name
/// in parentheses and a name may hold spaces and parentheses of its own.
#[cfg(target_os = "linux")]
fn stat_fields(stat: &str) -> Option<(u32, Option<u32>)> {
    let (_, rest) = stat.rsplit_once(')')?;
    let mut fields = rest.split_ascii_whitespace();
    // The state, then the parent.
    let parent = fields.nth(1)?.parse().ok()?;
    // The process group and the session, then the terminal: an `int` the
    // kernel prints signed, whose bits are the device number.
    let device: i32 = fields.nth(2)?.parse().ok()?;
    Some((parent, (device != 0).then_some(device as u32)))
}

/// Process `pid`'s parent and, when it has one, its controlling terminal's
/// device number; `None` when the process cannot be read.
#[cfg(target_os = "macos")]
fn process(pid: u32) -> Option<(u32, Option<u32>)> {
    use std::ffi::{c_int, c_void};
    use std::mem::{MaybeUninit, size_of};

    /// `PROC_PIDTBSDINFO`: the flavour of `proc_pidinfo` that answers with a
    /// [`BsdInfo`].
    const PROC_PIDTBSDINFO: c_int = 3;

    /// `NODEV`, which is what `e_tdev` holds for a process with no
    /// controlling terminal.
    const NODEV: u32 = u32::MAX;

    /// `struct proc_bsdinfo` from `<sys/proc_info.h>`, field for field. Two
    /// fields are read; the others are here so that those two sit where the
    /// kernel writes them, and the size is held below to the header's
    /// `PROC_PIDTBSDINFO_SIZE`.
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
    const _: () = assert!(size_of::<BsdInfo>() == 136);

    // Declared rather than depended on, the way `AttachConsole` is on
    // Windows: `proc_pidinfo` is in `libSystem`, which every macOS binary
    // links, and one function does not justify a crate with a build script.
    unsafe extern "C" {
        fn proc_pidinfo(
            pid: c_int,
            flavor: c_int,
            arg: u64,
            buffer: *mut c_void,
            buffersize: c_int,
        ) -> c_int;
    }

    let pid = c_int::try_from(pid).ok()?;
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
    let info = unsafe { info.assume_init() };
    let device = (info.e_tdev != NODEV && info.e_tdev != 0).then_some(info.e_tdev);
    Some((info.pbi_ppid, device))
}

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use super::*;

    #[test]
    fn the_parent_and_the_terminal_are_counted_past_a_name_with_parentheses_in_it() {
        // `claude` on pts/7: major 136, minor 7.
        assert_eq!(
            Some((2982192, Some(34823))),
            stat_fields("2982193 (claude) S 2982192 2982193 2982193 34823 2982193 4194560 0")
        );
        // A name is whatever the program set, spaces and `)` included.
        assert_eq!(
            Some((41, Some(34823))),
            stat_fields("42 (a) b (c)) S 41 42 42 34823 42 0")
        );
        // A hook in a session of its own: no terminal.
        assert_eq!(
            Some((2982193, None)),
            stat_fields("2982218 (sh) S 2982193 2982218 2982218 0 -1 4194560")
        );
        assert_eq!(None, stat_fields("2982218 (sh"));
    }
}

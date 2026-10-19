//! Which programs the ancestors of a process run, and which terminals they
//! hold open: what says a question from inside tmux comes from under the
//! tmux whose client is on the pane it names.
//!
//! A question from inside tmux names the terminals of tmux's clients — see
//! `crate::control::ask` — and any process of this user can name any
//! terminal. What it cannot make is an ancestor that holds the pane's
//! terminal open and runs the very program the pane is running. tmux's
//! server does both for every process in its panes: it is their ancestor, it
//! holds each client's terminal — the client hands it over when it attaches
//! — and it is the same `tmux` the client in the pane runs. So the window
//! takes the question for one it could answer only when such an ancestor is
//! there; see `control::ask::from_its_tmux`.
//!
//! Read on the connection's thread, beside the asker's lineage, and only for
//! a question from inside a multiplexer: at most [`MOST_HOLDERS`] ancestors
//! and [`MOST_FILES`] descriptors of each. Linux reads `/proc/<pid>/exe` and
//! `/proc/<pid>/fd`; macOS asks `proc_pidpath` and `proc_pidinfo`, declared
//! by hand as `ancestors` declares its own.

use std::fs;
use std::os::unix::fs::MetadataExt as _;
use std::path::Path;

use crate::control::protocol::{Executable, Holder};

/// How many of the asker's ancestors are read.
///
/// tmux's server is three or four processes up from a hook in one of its
/// panes — the hook's shell, the agent, the pane's shell — and a chain this
/// long is well past any that holds it.
pub(crate) const MOST_HOLDERS: usize = 16;

/// How many open descriptors of one process are looked at.
///
/// tmux's server holds two for each pane and a few for each client; this is
/// past a server with hundreds, and a bound on what one process with
/// thousands open can make the connection's thread do.
pub(crate) const MOST_FILES: usize = 1024;

/// The ancestors in `lineage` — nearest first, the asker itself first of all
/// and left out — as far as they can be read.
pub(crate) fn holders(lineage: &[u32]) -> Vec<Holder> {
    holders_in(lineage, holder)
}

/// [`holders`], with each process read by `read`, so a test can say what the
/// table is.
fn holders_in(lineage: &[u32], read: impl Fn(u32) -> Option<Holder>) -> Vec<Holder> {
    lineage
        .iter()
        .skip(1)
        .take(MOST_HOLDERS)
        .filter_map(|pid| read(*pid))
        .collect()
}

/// The program the foreground of `shell`'s terminal runs: the leader of the
/// terminal's foreground process group.
pub(crate) fn foreground(shell: u32) -> Option<Executable> {
    executable(super::ancestors::foreground_group(shell)?)
}

/// What `pid` runs and holds open; `None` when it cannot be read at all.
fn holder(pid: u32) -> Option<Holder> {
    let executable = executable(pid);
    let terminals = terminals(pid);
    (executable.is_some() || !terminals.is_empty()).then_some(Holder {
        pid,
        executable,
        terminals,
    })
}

/// The device and inode of the file at `path`, followed through links.
fn identity(path: &Path) -> Option<Executable> {
    let metadata = fs::metadata(path).ok()?;
    Some(Executable {
        device: metadata.dev(),
        inode: metadata.ino(),
    })
}

/// The low 32 bits of the device number of the terminal at `path`, when it
/// is a character device: a pty's master is `/dev/ptmx`'s number for every
/// pty, so only a slave — a terminal a client is on — ever names one. macOS
/// reads the same out of `proc_pidfdinfo` instead.
#[cfg(target_os = "linux")]
fn terminal_at(path: &Path) -> Option<u32> {
    use std::os::unix::fs::FileTypeExt as _;

    let metadata = fs::metadata(path).ok()?;
    metadata
        .file_type()
        .is_char_device()
        .then_some(metadata.rdev() as u32)
}

/// The program `pid` runs.
#[cfg(target_os = "linux")]
fn executable(pid: u32) -> Option<Executable> {
    identity(Path::new(&format!("/proc/{pid}/exe")))
}

/// The terminals `pid` holds open, at most [`MOST_FILES`] of its descriptors
/// looked at.
#[cfg(target_os = "linux")]
fn terminals(pid: u32) -> Vec<u32> {
    let Ok(descriptors) = fs::read_dir(format!("/proc/{pid}/fd")) else {
        return Vec::new();
    };
    let mut terminals = Vec::new();
    for descriptor in descriptors.flatten().take(MOST_FILES) {
        if let Some(terminal) = terminal_at(&descriptor.path())
            && !terminals.contains(&terminal)
        {
            terminals.push(terminal);
        }
    }
    terminals
}

/// The program `pid` runs.
#[cfg(target_os = "macos")]
fn executable(pid: u32) -> Option<Executable> {
    use std::ffi::{OsStr, c_int, c_void};
    use std::os::unix::ffi::OsStrExt as _;

    /// `PROC_PIDPATHINFO_MAXSIZE`: four times `MAXPATHLEN`.
    const MAX_PATH: usize = 4 * 1024;

    unsafe extern "C" {
        fn proc_pidpath(pid: c_int, buffer: *mut c_void, buffersize: u32) -> c_int;
    }

    let pid = c_int::try_from(pid).ok()?;
    let mut path = vec![0u8; MAX_PATH];
    // SAFETY: the buffer is `MAX_PATH` bytes, and `proc_pidpath` writes no
    // more than the size it is handed.
    let written = unsafe { proc_pidpath(pid, path.as_mut_ptr().cast(), MAX_PATH as u32) };
    let written = usize::try_from(written)
        .ok()
        .filter(|written| *written > 0)?;
    path.truncate(written.min(MAX_PATH));
    identity(Path::new(OsStr::from_bytes(&path)))
}

/// The terminals `pid` holds open, at most [`MOST_FILES`] of its descriptors
/// looked at.
#[cfg(target_os = "macos")]
fn terminals(pid: u32) -> Vec<u32> {
    use std::ffi::{c_int, c_void};
    use std::mem::{MaybeUninit, size_of};

    /// `PROC_PIDLISTFDS`: the flavour of `proc_pidinfo` that lists a
    /// process's descriptors as [`FdInfo`]s.
    const PROC_PIDLISTFDS: c_int = 1;
    /// `PROX_FDTYPE_VNODE`: a descriptor on a file, a terminal among them.
    const PROX_FDTYPE_VNODE: u32 = 1;
    /// `PROC_PIDFDVNODEPATHINFO`: the flavour of `proc_pidfdinfo` that
    /// answers with a [`VnodeFdInfoWithPath`].
    const PROC_PIDFDVNODEPATHINFO: c_int = 2;

    /// `struct proc_fdinfo`.
    #[repr(C)]
    #[derive(Clone, Copy)]
    struct FdInfo {
        proc_fd: i32,
        proc_fdtype: u32,
    }

    /// `struct vnode_fdinfowithpath` from `<sys/proc_info.h>`, flattened:
    /// `proc_fileinfo`, then `vnode_info`'s `vinfo_stat`, type, pad and
    /// fsid, then the path. Only `vst_rdev` is read; the size is held below
    /// to the header's `PROC_PIDFDVNODEPATHINFO_SIZE`.
    #[repr(C)]
    struct VnodeFdInfoWithPath {
        fi_openflags: u32,
        fi_status: u32,
        fi_offset: i64,
        fi_type: i32,
        fi_guardflags: u32,
        vst_dev: u32,
        vst_mode: u16,
        vst_nlink: u16,
        vst_ino: u64,
        vst_uid: u32,
        vst_gid: u32,
        vst_times: [i64; 8],
        vst_size: i64,
        vst_blocks: i64,
        vst_blksize: i32,
        vst_flags: u32,
        vst_gen: u32,
        vst_rdev: u32,
        vst_qspare: [i64; 2],
        vi_type: i32,
        vi_pad: i32,
        vi_fsid: [i32; 2],
        vip_path: [u8; 1024],
    }
    const _: () = assert!(size_of::<VnodeFdInfoWithPath>() == 1200);

    /// `VNON`..`VCHR`: a character device, in `vi_type`.
    const VCHR: i32 = 4;

    unsafe extern "C" {
        fn proc_pidfdinfo(
            pid: c_int,
            fd: c_int,
            flavor: c_int,
            buffer: *mut c_void,
            buffersize: c_int,
        ) -> c_int;
    }

    let Ok(pid) = c_int::try_from(pid) else {
        return Vec::new();
    };
    let mut listed = vec![
        FdInfo {
            proc_fd: 0,
            proc_fdtype: 0
        };
        MOST_FILES
    ];
    let room = (MOST_FILES * size_of::<FdInfo>()) as c_int;
    // SAFETY: the buffer is `room` bytes, and `proc_pidinfo` writes no more
    // than the size it is handed.
    let written = unsafe {
        super::ancestors::proc_pidinfo(pid, PROC_PIDLISTFDS, 0, listed.as_mut_ptr().cast(), room)
    };
    let Ok(written) = usize::try_from(written) else {
        return Vec::new();
    };
    listed.truncate((written / size_of::<FdInfo>()).min(MOST_FILES));

    let mut terminals = Vec::new();
    let size = size_of::<VnodeFdInfoWithPath>() as c_int;
    for descriptor in listed
        .iter()
        .filter(|descriptor| descriptor.proc_fdtype == PROX_FDTYPE_VNODE)
    {
        let mut info = MaybeUninit::<VnodeFdInfoWithPath>::zeroed();
        // SAFETY: the buffer is `size` bytes, and `proc_pidfdinfo` writes no
        // more than the size it is handed.
        let read = unsafe {
            proc_pidfdinfo(
                pid,
                descriptor.proc_fd,
                PROC_PIDFDVNODEPATHINFO,
                info.as_mut_ptr().cast(),
                size,
            )
        };
        if read != size {
            continue;
        }
        // SAFETY: zeroed and then filled, and every field is an integer or
        // an array of them, for which any bits are a value.
        let info = unsafe { info.assume_init() };
        if info.vi_type == VCHR && !terminals.contains(&info.vst_rdev) {
            terminals.push(info.vst_rdev);
        }
    }
    terminals
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::*;

    #[test]
    fn the_asker_is_left_out_and_no_more_than_the_most_are_read() {
        let table: HashMap<u32, Holder> = (1..=40)
            .map(|pid| {
                let holder = Holder {
                    pid,
                    executable: None,
                    terminals: vec![pid],
                };
                (pid, holder)
            })
            .collect();
        let read = |pid| table.get(&pid).cloned();
        // The asker, 40, holds the pane's terminal itself: that is any
        // process that opened it, and says nothing.
        let lineage: Vec<u32> = (1..=40).rev().collect();
        let found = holders_in(&lineage, read);
        assert_eq!(found.len(), MOST_HOLDERS);
        assert_eq!(found[0].pid, 39);
        assert!(found.iter().all(|holder| holder.pid != 40));
        // One that cannot be read is passed over, and the walk goes on.
        let found = holders_in(&[40, 39, 99, 38], read);
        assert_eq!(
            found.iter().map(|holder| holder.pid).collect::<Vec<_>>(),
            [39, 38]
        );
    }

    #[test]
    fn this_process_is_read_as_running_its_own_binary() {
        let here = holder(std::process::id()).expect("this process can be read");
        let binary = identity(&std::env::current_exe().expect("a path")).expect("a file");
        assert_eq!(here.executable, Some(binary));
    }
}

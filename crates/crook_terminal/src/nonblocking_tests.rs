use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

use crate::link::PtyLink as _;
use crate::pty::{Program, Pty};
use crate::snapshot::TerminalSize;

use super::*;

/// How long a test waits for a half to come back before calling it stuck
/// rather than hanging the suite.
const DEADLINE: Duration = Duration::from_secs(10);

/// `script`, run by `sh` on a pty of its own.
fn spawn(script: &str) -> Pty {
    Pty::spawn(
        &Program::command("/bin/sh", ["-c", script]),
        TerminalSize::new(80, 24),
        None,
        &[],
    )
    .expect("a shell should start on a pty")
}

#[test]
fn test_a_read_waits_for_a_child_slow_to_speak_and_then_sees_the_end() {
    // The master is non-blocking for the writer's sake, and the reader shares
    // that. Whoever holds the reader must not be able to tell: a read with
    // nothing to read waits, and the end of the child is the end of the
    // stream, not a `WouldBlock` a caller would take for it.
    let mut pty = spawn("sleep 0.3; echo crook-said-late");
    let mut reader = pty.take_reader().expect("a fresh pty has its reader");

    let mut heard = Vec::new();
    let mut buffer = [0; 4096];
    let ended = loop {
        match reader.read(&mut buffer) {
            Ok(0) => break None,
            Ok(read) => heard.extend_from_slice(&buffer[..read]),
            Err(error) => break Some(error),
        }
    };

    assert!(
        String::from_utf8_lossy(&heard).contains("crook-said-late"),
        "the read gave up before the child spoke: {heard:?}, then {ended:?}"
    );
    assert_ne!(
        Some(io::ErrorKind::WouldBlock),
        ended.map(|error| error.kind()),
        "a read with nothing to read returned instead of waiting"
    );
}

#[test]
fn test_a_write_waiting_on_a_child_that_never_reads_gives_up_when_the_pty_goes() {
    // The child ignores the hangup and leaves a `sleep` that ignores it too
    // holding the slave, so ending the child closes nothing: the master never
    // reports a hang-up, and on Linux a blocking write in this state is still
    // asleep long after everything was killed. Dropping the pty is the one
    // thing that can still release it, and it has to.
    //
    // In the background and waited for, because a shell runs the last command
    // of `-c` in its own place, and the `sleep` would then be the child the
    // kill ends rather than something it leaves behind. It outlives the
    // deadline, so its own end cannot be what releases the write.
    let mut pty = spawn("trap '' HUP; stty raw -echo; sleep 60 & echo \"crook-orphan-$!.\"; wait");
    let mut reader = pty.take_reader().expect("a fresh pty has its reader");
    let mut heard = String::new();
    let mut buffer = [0; 256];
    let orphan = loop {
        if let Some(pid) = heard
            .split_once("crook-orphan-")
            .and_then(|(_, rest)| rest.split_once('.'))
            .map(|(pid, _)| pid.to_owned())
        {
            break pid;
        }
        let read = reader
            .read(&mut buffer)
            .expect("the child says what it left behind");
        assert!(read > 0, "the child ended before it was ready");
        heard.push_str(&String::from_utf8_lossy(&buffer[..read]));
    };

    let mut writer = pty.writer().expect("a pty hands out a writer");
    let (done, finished) = mpsc::channel();
    thread::spawn(move || {
        // Far more than the kernel holds for a child that reads none of it.
        let _ = done.send(writer.write_all(&vec![b'x'; 1024 * 1024]));
    });
    assert!(
        finished.recv_timeout(Duration::from_millis(300)).is_err(),
        "the child read a megabyte it was never going to read"
    );

    drop(pty);
    let written = finished.recv_timeout(DEADLINE);
    // Ended before anything is asserted, so a failure leaves nothing behind;
    // waited for, since dropping the pty would end the `kill` first.
    let _ = spawn(&format!("kill {orphan}")).wait();
    let written = written.expect("a write waiting on a closed pty never came back");
    assert!(
        written.is_err(),
        "a write that went nowhere reported success"
    );
}

#[test]
fn test_a_write_that_cannot_watch_for_the_pty_closing_still_waits_for_the_child() {
    // A process out of descriptors cannot make the pipe a waiting write
    // watches beside the master. That may cost the write its release when the
    // pane closes, and nothing more: it waits for the child as a write always
    // did and gets through once the child reads, and the next write does too.
    // Giving up instead would end the writer, and the pane would never take a
    // key again — not even the Ctrl-C that would stop what is not reading.
    let mut pty = spawn("stty raw -echo; echo crook-ready; sleep 0.5; exec cat >/dev/null");
    let mut reader = pty.take_reader().expect("a fresh pty has its reader");
    let mut heard = String::new();
    let mut buffer = [0; 256];
    while !heard.contains("crook-ready") {
        let read = reader
            .read(&mut buffer)
            .expect("the child says it is ready");
        assert!(read > 0, "the child ended before it was ready");
        heard.push_str(&String::from_utf8_lossy(&buffer[..read]));
    }

    pty.closing().refuse_pipes();
    let mut writer = pty.writer().expect("a pty hands out a writer");
    let (done, finished) = mpsc::channel();
    thread::spawn(move || {
        // Far more than the kernel holds for a child that has not read yet.
        let first = writer.write_all(&vec![b'x'; 256 * 1024]);
        let next = writer.write_all(b"y");
        let _ = done.send((first, next));
    });
    let (first, next) = finished
        .recv_timeout(DEADLINE)
        .expect("a write the child reads in the end never came back");

    assert!(
        pty.closing().pipes_refused() > 0,
        "the write never had to wait, so nothing was tried without the pipe"
    );
    assert!(
        first.is_ok(),
        "a write that could not watch for the pty closing gave up on a child that reads: {first:?}"
    );
    assert!(next.is_ok(), "the write after it was refused: {next:?}");
}

/// A file that is removed when the test is done with it, passed or not.
struct Removed(std::path::PathBuf);

impl Drop for Removed {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

#[test]
fn test_input_a_child_has_not_read_comes_through_whole_when_it_turns_canonical_mode_on() {
    // A shell running a pasted block turns canonical mode on to run each line,
    // with the lines after it still unread. macOS then feeds that input
    // through the line discipline again, garbling whatever of it had wrapped
    // around the end of the queue's ring — which, from a writer that kept the
    // queue full while the shell read a byte at a time, was nearly all of it.
    // See the `typeahead` module. This child reads the start of what it is
    // sent a byte at a time, as a line editor does, then turns canonical mode
    // on and keeps the rest, which has to be exactly what followed.
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let kept = Removed(std::env::temp_dir().join(format!(
        "crook-typeahead-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    )));
    const READ_FIRST: usize = 600;
    let mut pty = spawn(&format!(
        "stty raw -echo; echo crook-ready; \
         dd bs=1 count={READ_FIRST} of=/dev/null 2>/dev/null; \
         stty icanon; exec cat > '{}'",
        kept.0.display()
    ));
    let mut reader = pty.take_reader().expect("a fresh pty has its reader");
    let mut heard = String::new();
    let mut buffer = [0; 256];
    while !heard.contains("crook-ready") {
        let read = reader
            .read(&mut buffer)
            .expect("the child says it is ready");
        assert!(read > 0, "the child ended before it was ready");
        heard.push_str(&String::from_utf8_lossy(&buffer[..read]));
    }

    // Lines, since canonical mode holds at most a kilobyte of one.
    let mut sent = Vec::new();
    for line in 0..256 {
        sent.extend_from_slice(format!("line {line:05} {}\n", "x".repeat(50)).as_bytes());
    }
    let mut writer = pty.writer().expect("a pty hands out a writer");
    let sending = sent.clone();
    thread::spawn(move || writer.write_all(&sending));

    let wanted = &sent[READ_FIRST..];
    let deadline = Instant::now() + DEADLINE;
    let got = loop {
        let got = std::fs::read(&kept.0).unwrap_or_default();
        if got.len() >= wanted.len() || Instant::now() > deadline {
            break got;
        }
        thread::sleep(Duration::from_millis(20));
    };
    let differs = got
        .iter()
        .zip(wanted)
        .position(|(got, wanted)| got != wanted)
        .unwrap_or(got.len().min(wanted.len()));
    assert!(
        got == wanted,
        "{} of {} bytes came through, and from byte {differs} they read {:?} where {:?} was sent",
        got.len(),
        wanted.len(),
        from(&got, differs),
        from(wanted, differs),
    );
}

/// A few bytes of `bytes` from `at`, to show where two runs part.
fn from(bytes: &[u8], at: usize) -> String {
    let rest = bytes.get(at..).unwrap_or_default();
    String::from_utf8_lossy(&rest[..rest.len().min(40)]).into_owned()
}

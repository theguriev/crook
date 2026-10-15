//! Playing a sound a plugin handed over, without linking an audio library.
//!
//! The workspace has no build script and requires no system library, which
//! rules out `cpal` and everything built on it: on Linux those want ALSA's
//! development headers, and a terminal that will not build without them is a
//! terminal nobody can build. So the host does what a shell script would —
//! finds a player that is already on the machine and hands it a file.
//!
//! That is not a workaround. Every desktop this runs on ships a command that
//! plays a wav, they are the same commands a person would type, and a player
//! in another process cannot wedge the window however badly it behaves.
//!
//! # What arrives, and where it goes
//!
//! A plugin sends bytes, not a path — see [`Request::PlaySound`]. They are
//! written into the cache directory under a name derived from the bytes, so a
//! plugin ringing the same chime forty times a day writes one file once and
//! every ring after that is a spawn. Nothing is ever written outside that
//! directory and nothing a plugin sends is ever executed.
//!
//! # And how much of it, and how often
//!
//! Content-addressed is not bounded: a plugin sending a chime that differs by
//! one byte each time writes a new file each time. So the directory is held
//! under [`MAX_CACHE`], the sounds played longest ago going first, and one
//! plugin is played [`SOUNDS_PER_SECOND`] at most — past that a sound is
//! answered with why and not played, because each is a player started and a
//! person interrupted.
//!
//! [`Request::PlaySound`]: crook_plugin_api::Request::PlaySound

use std::collections::VecDeque;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::process::{Child, Stdio};
use std::time::{Duration, Instant, SystemTime};

use crook_plugin_api::Answer;

/// How long a player may take before it is killed.
///
/// A sound is at most a couple of seconds. Ten is long enough that nothing
/// legitimate is cut off and short enough that a player which wedged on a
/// dead audio server does not sit there for the rest of the session.
const PATIENCE: Duration = Duration::from_secs(10);

/// How often the reaper looks at the player it is waiting on.
const GLANCE: Duration = Duration::from_millis(50);

/// The largest sound the host will write down.
///
/// Four megabytes is about forty seconds of 16-bit mono at 44.1kHz, which is
/// far longer than anything anybody should be played at the end of a command.
/// It bounds one sound and not the directory they are kept in: that is
/// [`MAX_CACHE`].
const MAX_WAV: usize = 4 << 20;

/// The most the sounds directory holds, every sound in it together.
///
/// Four of the largest sound there can be, which is a hundred or more of the
/// chimes plugins actually send: crook-dziling's six come to under seven
/// hundred kilobytes together. So a cache this size is one that is always hit
/// by any plugin that rings the same few sounds. Past it the sounds played
/// longest ago are deleted, and the next time one of them is asked for it is
/// written down again.
///
/// What a sound counts against it is what it costs a disk, in whole
/// [`BLOCK`]s, and not its length: a wav can be twelve bytes, and counted by
/// length this would have held over a million of them — gigabytes of
/// blocks, and a directory every new sound had to look through whole.
const MAX_CACHE: u64 = 4 * MAX_WAV as u64;

/// What one file costs a disk at least, which is what [`MAX_CACHE`] counts
/// each sound as: its length rounded up to a whole number of these.
///
/// Four kilobytes is what ext4, APFS and NTFS allocate by default, and it
/// holds [`MAX_CACHE`] to about four thousand files however small each is.
const BLOCK: u64 = 4096;

/// How many sounds one plugin may have played in any one second.
///
/// Every sound is a player started and a reaper thread waiting on it for up
/// to [`PATIENCE`], and every one is somebody interrupted. Four a second is
/// more chimes than anybody can tell apart, and a plugin past it is one
/// ringing because it rings.
pub(super) const SOUNDS_PER_SECOND: usize = 4;

/// The sounds one plugin has played in the last second, which is what
/// [`SOUNDS_PER_SECOND`] counts.
///
/// A window of instants rather than a counter reset on a clock, because a
/// clock would be a timer, and a window never runs while nobody is asking.
#[derive(Default)]
pub(super) struct Chimes {
    /// When each was let through, oldest first; none older than a second
    /// survives a call to [`Chimes::let_through`].
    played: VecDeque<Instant>,
}

impl Chimes {
    /// Whether one more may be played at `now`, counting it if so.
    pub(super) fn let_through(&mut self, now: Instant) -> bool {
        while self
            .played
            .front()
            .is_some_and(|at| now.duration_since(*at) >= Duration::from_secs(1))
        {
            self.played.pop_front();
        }
        if self.played.len() >= SOUNDS_PER_SECOND {
            return false;
        }
        self.played.push_back(now);
        true
    }

    /// When each sound still counted was let through, oldest first.
    #[cfg(test)]
    pub(super) fn played(&self) -> Vec<Instant> {
        self.played.iter().copied().collect()
    }
}

/// Plays a sound, and says what became of it.
pub(super) fn play(wav: &[u8], volume: u8) -> Answer {
    if wav.len() > MAX_WAV {
        return Answer::Failed(format!(
            "the sound is {} bytes and the limit is {MAX_WAV}",
            wav.len()
        ));
    }
    if !is_wav(wav) {
        // Named rather than guessed at: the host decodes nothing, so a plugin
        // that sent an mp3 has to be told that is the problem and not left
        // wondering why its chime is silent.
        return Answer::Failed("the sound is not a RIFF/WAVE file".into());
    }

    let file = match store(wav) {
        Ok(file) => file,
        Err(why) => return Answer::Failed(format!("the sound could not be written down: {why}")),
    };

    let Some(player) = Player::found() else {
        return Answer::Failed("no sound player is installed".into());
    };

    match player.start(&file, volume.min(100)) {
        Ok(child) => {
            reap(child);
            Answer::Played
        }
        Err(why) => Answer::Failed(format!("{} could not be started: {why}", player.program)),
    }
}

/// Whether these bytes are a wav, by the only two markers a RIFF header has.
///
/// Checked because of what `aplay` does with a file that is not one: it reads
/// it as raw 8-bit PCM and plays it, at full scale, without an error. A plugin
/// sending the wrong bytes should get an answer saying so, not a burst of
/// noise on somebody's speakers.
fn is_wav(bytes: &[u8]) -> bool {
    bytes.len() >= 12 && &bytes[..4] == b"RIFF" && &bytes[8..12] == b"WAVE"
}

/// Writes the sound into the cache under a name derived from its bytes, and
/// answers where it went.
///
/// Content-addressed so that the same chime is written once and then only
/// spawned, and so that two plugins sending the same bytes share one file
/// rather than racing over a name one of them chose.
fn store(wav: &[u8]) -> io::Result<PathBuf> {
    let directory = dirs::cache_dir()
        .ok_or_else(|| io::Error::other("this machine has no cache directory"))?
        .join("crook")
        .join("sounds");
    store_in(&directory, wav, MAX_CACHE, SystemTime::now())
}

/// [`store`], into `directory`, which is then held under `cap` bytes, as of
/// `now`.
fn store_in(directory: &Path, wav: &[u8], cap: u64, now: SystemTime) -> io::Result<PathBuf> {
    fs::create_dir_all(directory)?;

    let file = directory.join(format!("{:016x}.wav", fingerprint(wav)));
    if file.exists() {
        played(&file, now);
        return Ok(file);
    }

    // Written beside and renamed, so a player never opens a half-written file:
    // a second chime arriving while the first is still writing finds either
    // nothing or the whole thing. The temporary carries the process id because
    // two Crooks may be doing this at once.
    let partial = directory.join(format!(
        "{:016x}.{}.partial",
        fingerprint(wav),
        std::process::id()
    ));
    fs::write(&partial, wav)?;
    if let Err(why) = fs::rename(&partial, &file) {
        let _ = fs::remove_file(&partial);
        return Err(why);
    }
    played(&file, now);
    evict(directory, cap, &file);
    Ok(file)
}

/// Marks a sound as played at `now`, which is what keeps it from being the
/// next to go.
///
/// The file's modification time is the mark, so that what was played when
/// survives a restart without a file of its own to keep it in. Set rather
/// than left to the write, because a filesystem's clock is coarse enough
/// that two sounds written a moment apart can share one. Best effort: a
/// sound that cannot be marked is only one that may go sooner than it
/// should, and Windows may refuse to open a file a player has open.
fn played(file: &Path, now: SystemTime) {
    let _ = fs::File::options()
        .write(true)
        .open(file)
        .and_then(|file| file.set_modified(now));
}

/// What a file `length` bytes long counts against [`MAX_CACHE`]. See
/// [`BLOCK`].
fn on_disk(length: u64) -> u64 {
    length.div_ceil(BLOCK).max(1) * BLOCK
}

/// Deletes the sounds in `directory` played longest ago until what is left
/// fits under `cap` — never `keep`, which is about to be played.
///
/// Only when a sound has just been written, which is the only time the
/// directory grows. Sounds only: a `.partial` is somebody's write in
/// progress, perhaps another Crook's.
fn evict(directory: &Path, cap: u64, keep: &Path) {
    let Ok(entries) = fs::read_dir(directory) else {
        return;
    };
    let mut sounds: Vec<(SystemTime, PathBuf, u64)> = entries
        .flatten()
        .filter(|entry| entry.path().extension().is_some_and(|kind| kind == "wav"))
        .filter_map(|entry| {
            let facts = entry.metadata().ok()?;
            facts.is_file().then(|| {
                let at = facts.modified().unwrap_or(SystemTime::UNIX_EPOCH);
                (at, entry.path(), on_disk(facts.len()))
            })
        })
        .collect();

    let mut total: u64 = sounds.iter().map(|(_, _, size)| size).sum();
    // Oldest first, and by name between two played at the same moment, so
    // that which of them goes is the same on every run.
    sounds.sort();
    for (_, path, size) in sounds {
        if total <= cap {
            return;
        }
        // A sound a player still has open cannot be deleted on Windows; it
        // is skipped, and the next oldest goes in its place.
        if path != keep && fs::remove_file(&path).is_ok() {
            total -= size;
        }
    }
}

/// FNV-1a over the bytes, which is a file name and not a promise.
///
/// Not a cryptographic hash and it does not need to be: the worst a collision
/// could do is play one of this machine's own cached chimes instead of
/// another, and the input is audio a person installed rather than something an
/// attacker is choosing against a name they can predict.
fn fingerprint(bytes: &[u8]) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x1000_0000_01b3);
    }
    hash
}

/// A command that plays a wav, and how to tell it how loud.
struct Player {
    /// What to run.
    program: &'static str,
    /// How this one takes its volume, if it takes one at all.
    volume: Volume,
    /// What it needs besides the file and the volume.
    flags: &'static [&'static str],
}

/// How a player wants to be told how loud to be.
///
/// Every one of these is a different scale, and a plugin should not have to
/// know that: it sends 0 to 100 and the host translates.
#[derive(Copy, Clone)]
enum Volume {
    /// It cannot be told. `aplay` is the case that matters — the fallback on
    /// ALSA-only machines, so containers and headless boxes.
    Fixed,
    /// `--volume=0.70`, the PipeWire scale.
    Fraction(&'static str),
    /// `--volume 45875`, PulseAudio's, where 65536 is unity.
    Pulse(&'static str),
    /// `-volume 70`, a plain percentage.
    Percent(&'static str),
    /// `-v 0.70` as a separate argument, which is macOS's.
    Split(&'static str),
}

/// The players, in the order they are looked for.
///
/// Ordered by how likely each is to be the one actually wired to the speakers
/// rather than by preference: a machine with PipeWire has `pw-play`, and
/// falling through to `ffplay` there would play the sound through a second
/// stack for no reason. `ffplay` and `mpv` are last because they are the ones
/// most likely to be installed for something else entirely.
const PLAYERS: &[Player] = &[
    Player {
        program: "pw-play",
        volume: Volume::Fraction("--volume="),
        flags: &[],
    },
    Player {
        program: "paplay",
        volume: Volume::Pulse("--volume="),
        flags: &[],
    },
    Player {
        program: "afplay",
        volume: Volume::Split("-v"),
        flags: &[],
    },
    Player {
        program: "aplay",
        volume: Volume::Fixed,
        flags: &["-q"],
    },
    Player {
        program: "play",
        volume: Volume::Split("-v"),
        flags: &["-q"],
    },
    Player {
        program: "ffplay",
        volume: Volume::Percent("-volume"),
        flags: &["-nodisp", "-autoexit", "-loglevel", "quiet"],
    },
    Player {
        program: "mpv",
        volume: Volume::Percent("--volume="),
        flags: &["--no-video", "--really-quiet"],
    },
];

impl Player {
    /// The first player on this machine, or `None` on one with none.
    fn found() -> Option<&'static Player> {
        PLAYERS.iter().find(|player| on_path(player.program))
    }

    /// Starts it on a file, detached from everything.
    fn start(&self, file: &Path, volume: u8) -> io::Result<Child> {
        // Through the workspace's own wrapper, which is what sets
        // `CREATE_NO_WINDOW`: without it every sound this plays flashes a
        // console window on Windows, once per finished command.
        let mut command = crate::process::command(self.program);
        command.args(self.flags);

        match self.volume {
            Volume::Fixed => {}
            Volume::Fraction(flag) => {
                command.arg(format!("{flag}{:.2}", f32::from(volume) / 100.0));
            }
            Volume::Pulse(flag) => {
                command.arg(format!("{flag}{}", u32::from(volume) * 65536 / 100));
            }
            Volume::Percent(flag) => {
                if flag.ends_with('=') {
                    command.arg(format!("{flag}{volume}"));
                } else {
                    command.arg(flag).arg(volume.to_string());
                }
            }
            Volume::Split(flag) => {
                command
                    .arg(flag)
                    .arg(format!("{:.2}", f32::from(volume) / 100.0));
            }
        }

        // `--` first would be tidier, but not every one of these accepts it.
        // The path is ours and always absolute — it comes out of the cache
        // directory — so it cannot start with a `-` and be read as a flag.
        command
            .arg(file)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
    }
}

/// Whether a program is on `PATH`.
///
/// Done here rather than with a crate because it is six lines and the answer
/// is only ever used to pick between commands that are all optional: a false
/// negative costs a fallback, not a failure.
fn on_path(program: &str) -> bool {
    let Some(path) = std::env::var_os("PATH") else {
        return false;
    };
    std::env::split_paths(&path).any(|directory| {
        let candidate = directory.join(program);
        candidate.is_file() && executable(&candidate)
    })
}

/// Whether a file has an execute bit for anybody.
#[cfg(unix)]
fn executable(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    fs::metadata(path).is_ok_and(|data| data.permissions().mode() & 0o111 != 0)
}

/// Everywhere else, being there is being runnable.
#[cfg(not(unix))]
fn executable(_: &Path) -> bool {
    true
}

/// Waits on the player so it does not become a zombie, and kills it if it
/// stops being a player and starts being a process that is stuck.
///
/// On its own thread rather than on the pool: the pool is where the host does
/// work a plugin asked for, and holding one of its threads for the length of a
/// sound would mean a chime could delay somebody's network request.
fn reap(mut child: Child) {
    std::thread::spawn(move || {
        let deadline = Instant::now() + PATIENCE;
        loop {
            match child.try_wait() {
                // Finished on its own, which is what happens every time.
                Ok(Some(_)) => return,
                Ok(None) => {}
                // It cannot be waited on, so there is nothing left to do but
                // stop looking at it.
                Err(_) => return,
            }
            if Instant::now() >= deadline {
                let _ = child.kill();
                let _ = child.wait();
                return;
            }
            std::thread::sleep(GLANCE);
        }
    });
}

#[cfg(test)]
#[path = "sound_tests.rs"]
mod tests;

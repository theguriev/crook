//! What can be checked about playing a sound without a sound card.
//!
//! The spawn itself is not tested here: a test that ran a real player would
//! need one installed and would make a noise in CI. What *is* testable is
//! everything that happens before the spawn, which is where the mistakes are.

use super::*;

/// The smallest thing that is a wav: a RIFF header and nothing in it.
fn header() -> Vec<u8> {
    let mut bytes = Vec::new();
    bytes.extend_from_slice(b"RIFF");
    bytes.extend_from_slice(&36u32.to_le_bytes());
    bytes.extend_from_slice(b"WAVE");
    bytes
}

#[test]
fn a_riff_header_is_a_wav() {
    assert!(is_wav(&header()));
}

#[test]
fn other_audio_is_refused_rather_than_guessed_at() {
    // An mp3, an ogg and a text file all reach `aplay` as raw PCM and come out
    // as noise at full scale, which is the reason this check exists at all.
    assert!(!is_wav(b"ID3\x04\x00\x00\x00\x00\x00\x00"));
    assert!(!is_wav(b"OggS\x00\x02\x00\x00\x00\x00\x00\x00"));
    assert!(!is_wav(b"not audio at all"));
}

#[test]
fn a_truncated_header_is_not_a_wav() {
    // Shorter than the twelve bytes the check reads, which must be answered
    // rather than panicked on.
    for length in 0..12 {
        assert!(!is_wav(&header()[..length.min(header().len())]));
    }
}

#[test]
fn riff_alone_is_not_enough() {
    // A RIFF container holding something else — an AVI, say — is not a wav,
    // and the second marker is what says so.
    let mut avi = header();
    avi[8..12].copy_from_slice(b"AVI ");
    assert!(!is_wav(&avi));
}

#[test]
fn a_sound_too_big_to_be_a_chime_is_refused() {
    let huge = vec![0u8; MAX_WAV + 1];
    assert!(matches!(play(&huge, 70), Answer::Failed(why) if why.contains("limit")));
}

#[test]
fn what_is_not_a_wav_is_never_written_down_or_played() {
    let Answer::Failed(why) = play(b"ID3 this is an mp3", 70) else {
        panic!("something that is not a wav was accepted");
    };
    assert!(why.contains("RIFF/WAVE"), "unhelpful answer: {why}");
}

#[test]
fn the_same_bytes_are_written_once() {
    let wav = header();
    let Ok(first) = store(&wav) else {
        return; // No cache directory here, which is not this test's subject.
    };
    let written = fs::metadata(&first)
        .expect("the sound was not written")
        .len();
    let second = store(&wav).expect("the second store failed");
    assert_eq!(first, second, "the same bytes went to two different files");
    assert_eq!(
        written,
        fs::metadata(&second).expect("it went missing").len(),
        "the second store rewrote the file"
    );
    let _ = fs::remove_file(&first);
}

#[test]
fn different_bytes_go_to_different_files() {
    let mut other = header();
    other.extend_from_slice(b"fmt ");
    let (Ok(one), Ok(two)) = (store(&header()), store(&other)) else {
        return; // No cache directory.
    };
    assert_ne!(one, two);
    let _ = fs::remove_file(&one);
    let _ = fs::remove_file(&two);
}

#[test]
fn a_stored_sound_lands_in_the_cache_and_nowhere_else() {
    let Ok(file) = store(&header()) else {
        return;
    };
    let cache = dirs::cache_dir().expect("there was a cache directory a moment ago");
    assert!(
        file.starts_with(cache.join("crook").join("sounds")),
        "a sound was written outside the cache: {}",
        file.display()
    );
    let _ = fs::remove_file(&file);
}

#[test]
fn no_partial_file_is_left_behind() {
    // Its own sound, and only its own leftovers counted. The cache is one
    // directory for the whole machine, and the test beside this one stores
    // a sound at the same moment: with the same bytes, the two wrote the
    // same `.partial` and raced each other's rename; counting every
    // `.partial` in the directory then saw the other's half-written file
    // and failed, which is what Crook CI's Linux job did once in a while.
    let mut wav = header();
    wav.extend_from_slice(b"no partial file is left behind");
    let Ok(file) = store(&wav) else {
        return;
    };
    let directory = file.parent().expect("it was written at the root");
    let mine = format!("{:016x}.", fingerprint(&wav));
    let leftovers = fs::read_dir(directory)
        .expect("the sounds directory went away")
        .filter_map(Result::ok)
        .filter(|entry| {
            let path = entry.path();
            path.extension().is_some_and(|kind| kind == "partial")
                && path
                    .file_name()
                    .and_then(|name| name.to_str())
                    .is_some_and(|name| name.starts_with(&mine))
        })
        .count();
    assert_eq!(leftovers, 0, "a half-written sound was left in the cache");
    let _ = fs::remove_file(&file);
}

#[test]
fn the_fingerprint_separates_sounds_that_differ_by_one_byte() {
    let mut other = header();
    let last = other.len() - 1;
    other[last] ^= 0x01;
    assert_ne!(fingerprint(&header()), fingerprint(&other));
}

#[test]
fn nothing_is_on_path_under_an_empty_path() {
    // The headless case: no player anywhere, which must be an answer and not a
    // panic.
    let restore = std::env::var_os("PATH");
    // SAFETY: single-threaded test process; restored before it returns.
    unsafe { std::env::set_var("PATH", "") };
    let found = Player::found().is_none();
    match restore {
        Some(path) => unsafe { std::env::set_var("PATH", path) },
        None => unsafe { std::env::remove_var("PATH") },
    }
    assert!(found, "a player was found on an empty PATH");
}

#[test]
fn a_plugin_is_let_through_four_sounds_in_a_second_and_no_more() {
    let mut chimes = Chimes::default();
    let start = Instant::now();

    for ring in 0..SOUNDS_PER_SECOND {
        assert!(
            chimes.let_through(start),
            "sound {ring} was not let through"
        );
    }
    assert!(
        !chimes.let_through(start),
        "one more than a second's worth was let through"
    );
    assert!(
        !chimes.let_through(start + Duration::from_millis(999)),
        "the second was over before it was"
    );
    // And a sound turned down is not counted: the second after the first
    // four is a second with room for four more.
    for ring in 0..SOUNDS_PER_SECOND {
        assert!(
            chimes.let_through(start + Duration::from_secs(1)),
            "sound {ring} of the next second was not let through"
        );
    }
}

/// A sound of its own, `size` bytes long, that differs from every other
/// `mark`.
fn sound(mark: u8, size: usize) -> Vec<u8> {
    let mut wav = header();
    wav.resize(size, mark);
    wav
}

/// The sounds `directory` holds, by the mark each was made with, and how
/// many bytes they come to.
fn kept(directory: &Path, marks: &[u8], size: usize) -> (Vec<u8>, u64) {
    let present: Vec<u8> = marks
        .iter()
        .copied()
        .filter(|mark| {
            directory
                .join(format!("{:016x}.wav", fingerprint(&sound(*mark, size))))
                .exists()
        })
        .collect();
    let total = fs::read_dir(directory)
        .expect("the sounds directory is there")
        .filter_map(Result::ok)
        .filter_map(|entry| entry.metadata().ok())
        .map(|facts| facts.len())
        .sum();
    (present, total)
}

#[test]
fn the_sounds_directory_stays_under_its_cap_by_losing_what_was_played_longest_ago() {
    // Content-addressed is not bounded. A plugin that sends a chime differing
    // by one byte each time wrote a new file each time, and nothing ever
    // deleted one. Each store is a second after the last, so that "longest
    // ago" does not depend on how fine this filesystem's clock is.
    let scratch = super::super::tests::Scratch::new("sounds");
    let directory = scratch.path();
    // Whole blocks, so that what each costs the disk is its length.
    let size = 2 * BLOCK as usize;
    let cap = 3 * size as u64 + size as u64 / 2;
    let at = |second: u64| SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000_000 + second);
    let store = |mark: u8, second: u64| {
        store_in(directory, &sound(mark, size), cap, at(second)).expect("a sound is written down")
    };

    store(1, 1);
    store(2, 2);
    store(3, 3);
    assert_eq!(
        kept(directory, &[1, 2, 3], size),
        (vec![1, 2, 3], 3 * size as u64)
    );

    // The first is played again, which makes the second the one played
    // longest ago — so the fourth costs the second its place, not the first.
    let again = store(1, 4);
    store(4, 5);

    let (present, total) = kept(directory, &[1, 2, 3, 4], size);
    assert!(total <= cap, "{total} bytes kept under a cap of {cap}");
    assert_eq!(
        present,
        vec![1, 3, 4],
        "what went was not the sound played longest ago"
    );
    assert!(again.exists());
}

#[test]
fn the_sound_about_to_be_played_is_never_the_one_that_goes() {
    // A cap smaller than the sound itself, which [`MAX_CACHE`] never is; the
    // rule it checks is the one every eviction relies on, that the file just
    // handed back is still there to be played.
    let scratch = super::super::tests::Scratch::new("sounds-kept");
    let at = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000_000);

    let file = store_in(scratch.path(), &sound(9, 1000), 10, at).expect("a sound is written down");

    assert!(file.exists(), "the sound was evicted before it was played");
}

#[test]
fn sounds_too_small_to_fill_the_cap_by_length_still_fill_it_by_what_they_cost_the_disk() {
    // The smallest wav is twelve bytes, and every file takes at least a block
    // whatever its length. Counted by length, a cap of sixteen megabytes held
    // over a million such sounds — gigabytes of disk — before it let one go.
    let scratch = super::super::tests::Scratch::new("sounds-small");
    let directory = scratch.path();
    let size = 64;
    let cap = 3 * BLOCK;
    let at = |second: u64| SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000_000 + second);

    for mark in 1..=5 {
        store_in(directory, &sound(mark, size), cap, at(u64::from(mark)))
            .expect("a sound is written down");
    }

    let (present, total) = kept(directory, &[1, 2, 3, 4, 5], size);
    assert_eq!(
        present,
        vec![3, 4, 5],
        "{total} bytes of sounds were kept as if they cost the disk no more than that"
    );
}

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
    let written = fs::metadata(&first).expect("the sound was not written").len();
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
    let Ok(file) = store(&header()) else {
        return;
    };
    let directory = file.parent().expect("it was written at the root");
    let leftovers = fs::read_dir(directory)
        .expect("the sounds directory went away")
        .filter_map(Result::ok)
        .filter(|entry| entry.path().extension().is_some_and(|kind| kind == "partial"))
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

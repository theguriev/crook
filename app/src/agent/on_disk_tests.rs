use std::os::unix::fs::symlink;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::{Duration, Instant};

use super::*;
use crate::agent::request::FileEdit;

/// A directory of the test's own, gone when it is dropped.
struct Scratch(PathBuf);

impl Scratch {
    fn new(label: &str) -> Self {
        static NEXT: AtomicU32 = AtomicU32::new(0);
        let path = std::env::temp_dir().join(format!(
            "crook-on-disk-{label}-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path).expect("the system temp directory is writable");
        Self(path)
    }

    /// `name` under it, as the text a hook is handed.
    fn at(&self, name: &str) -> String {
        self.0.join(name).to_str().expect("a UTF-8 path").to_owned()
    }

    fn cwd(&self) -> String {
        self.at("work")
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn edit(path: &str) -> AskRequest {
    AskRequest::Edit {
        path: path.to_owned(),
        edits: vec![FileEdit {
            old: "a".to_owned(),
            new: "b".to_owned(),
            replace_all: false,
        }],
        resolves_outside: false,
    }
}

fn write(path: &str) -> AskRequest {
    AskRequest::Write {
        path: path.to_owned(),
        content: "a".to_owned(),
        resolves_outside: false,
        replaces: None,
    }
}

#[test]
fn a_link_inside_the_working_directory_that_leads_out_of_it_is_outside() {
    let scratch = Scratch::new("links");
    let cwd = scratch.cwd();
    std::fs::create_dir_all(scratch.at("work/src")).unwrap();
    std::fs::create_dir_all(scratch.at("elsewhere")).unwrap();
    std::fs::write(scratch.at("work/src/main.rs"), "fn main() {}\n").unwrap();
    std::fs::write(scratch.at("elsewhere/passwd"), "root\n").unwrap();
    symlink(scratch.at("elsewhere"), scratch.at("work/vendor")).unwrap();
    symlink(scratch.at("elsewhere/passwd"), scratch.at("work/notes.txt")).unwrap();
    symlink(scratch.at("work/src"), scratch.at("work/source")).unwrap();
    symlink(scratch.at("nowhere/x"), scratch.at("work/dangling")).unwrap();

    let outside = |path: String| look(&edit(&path), &cwd).unwrap().resolves_outside;
    // Inside by their text and by where they lead: a file there, one not
    // there yet, one in a directory not there yet, and one through a link
    // that stays inside.
    for path in [
        "work/src/main.rs",
        "work/src/new.rs",
        "work/src/deeper/new.rs",
        "work/source/main.rs",
        "work/./src/main.rs",
    ] {
        assert!(!outside(scratch.at(path)), "{path}");
    }
    // Inside by their text, and out of it through a link: a directory, a
    // file not there yet under it, the file itself, and a link to nothing.
    for path in [
        "work/vendor/passwd",
        "work/vendor/new.txt",
        "work/vendor/deeper/new.txt",
        "work/notes.txt",
        "work/dangling",
    ] {
        assert!(outside(scratch.at(path)), "{path}");
        assert!(
            look(&write(&scratch.at(path)), &cwd)
                .unwrap()
                .resolves_outside,
            "{path}"
        );
    }
    // Outside by its text, and a working directory that is not there.
    assert!(outside(scratch.at("elsewhere/passwd")));
    assert!(
        look(&edit(&scratch.at("work/src/main.rs")), &scratch.at("gone"))
            .unwrap()
            .resolves_outside
    );
    // A working directory that is itself reached through a link is where it
    // leads, and so is what is under it.
    symlink(scratch.at("work"), scratch.at("linked-work")).unwrap();
    assert!(
        !look(
            &edit(&scratch.at("linked-work/src/main.rs")),
            &scratch.at("linked-work")
        )
        .unwrap()
        .resolves_outside
    );
}

#[test]
fn a_path_deeper_than_the_parts_followed_is_outside() {
    let scratch = Scratch::new("deep");
    let cwd = scratch.cwd();
    std::fs::create_dir_all(&cwd).unwrap();
    let deep = |parts: usize| format!("{cwd}/{}", vec!["d"; parts].join("/"));
    assert!(
        !look(&edit(&deep(MAX_PARTS)), &cwd)
            .unwrap()
            .resolves_outside
    );
    assert!(
        look(&edit(&deep(MAX_PARTS + 1)), &cwd)
            .unwrap()
            .resolves_outside
    );
}

#[test]
fn a_write_says_whether_it_replaces_a_file_and_how_big_it_is() {
    let scratch = Scratch::new("replaces");
    let cwd = scratch.cwd();
    std::fs::create_dir_all(&cwd).unwrap();
    let replaces = |name: &str| look(&write(&scratch.at(name)), &cwd).map(|found| found.replaces);

    assert_eq!(replaces("work/new.txt"), Some(None));
    std::fs::write(scratch.at("work/three.txt"), "one\ntwo\nthree\n").unwrap();
    assert_eq!(
        replaces("work/three.txt"),
        Some(Some(Replaced {
            // Counted as a change counts its text: the empty line after the
            // last newline is one.
            lines: 4,
            bytes: 14,
            more: false,
        }))
    );
    std::fs::write(scratch.at("work/empty.txt"), "").unwrap();
    assert_eq!(
        replaces("work/empty.txt"),
        Some(Some(Replaced {
            lines: 0,
            bytes: 0,
            more: false,
        }))
    );
    // Through a link, as the write would go.
    symlink(scratch.at("work/three.txt"), scratch.at("work/linked.txt")).unwrap();
    assert_eq!(replaces("work/linked.txt"), replaces("work/three.txt"));
    // Past what is read, counted as far as it was read.
    let long = "x\n".repeat(MAX_COUNTED as usize / 2 + 10);
    std::fs::write(scratch.at("work/long.txt"), &long).unwrap();
    assert_eq!(
        replaces("work/long.txt"),
        Some(Some(Replaced {
            lines: MAX_COUNTED / 2 + 1,
            bytes: long.len() as u64,
            more: true,
        }))
    );
    // Something that is not a file is not a write the hook can describe,
    // so it asks nothing; an edit is never measured.
    std::fs::create_dir_all(scratch.at("work/dir")).unwrap();
    assert_eq!(replaces("work/dir"), None);
    assert_eq!(
        look(&edit(&scratch.at("work/three.txt")), &cwd).map(|found| found.replaces),
        Some(None)
    );
}

#[test]
fn a_look_that_takes_too_long_is_given_up_without_waiting_for_it() {
    let started = Instant::now();
    assert_eq!(
        within(Duration::from_millis(50), || {
            std::thread::sleep(Duration::from_secs(1));
            1
        }),
        None
    );
    assert!(started.elapsed() < Duration::from_millis(500));
    assert_eq!(within(Duration::from_secs(5), || 1), Some(1));
}

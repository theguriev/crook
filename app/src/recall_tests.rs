use super::*;

fn ran(command: &str, directory: &str, exit: i32, after: Option<&str>) -> Record {
    Record {
        command: command.to_owned(),
        directory: Some(PathBuf::from(directory)),
        exit: Some(exit),
        at: 0,
        after: after.map(str::to_owned),
    }
}

fn here(directory: &str, previous: Option<&str>) -> Context {
    Context {
        directory: Some(PathBuf::from(directory)),
        previous: previous.map(str::to_owned),
    }
}

#[test]
fn test_the_command_run_in_this_directory_beats_a_newer_one_run_elsewhere() {
    let recall = Recall::in_memory(vec![
        ran("cargo test -p crook_app", "/work/crook", 0, None),
        ran(
            "cargo test --example fixture",
            "/work/crook-pirate",
            0,
            None,
        ),
    ]);

    assert_eq!(
        recall.best("cargo t", &[], &here("/work/crook", None)),
        Some("cargo test -p crook_app"),
        "the newer command is from another project"
    );
    assert_eq!(
        recall.best("cargo t", &[], &here("/work/crook-pirate", None)),
        Some("cargo test --example fixture")
    );
    assert_eq!(
        recall.best("cargo t", &[], &here("/elsewhere", None)),
        Some("cargo test --example fixture"),
        "nowhere either ran: the newest, as it always was"
    );
}

#[test]
fn test_what_followed_the_command_just_run_comes_next() {
    let recall = Recall::in_memory(vec![
        ran("git push", "/work/crook", 0, Some("git commit --amend")),
        ran("git pull --rebase", "/work/crook", 0, Some("git fetch")),
    ]);

    assert_eq!(
        recall.best(
            "git p",
            &[],
            &here("/work/crook", Some("git commit --amend"))
        ),
        Some("git push")
    );
    assert_eq!(
        recall.best("git p", &[], &here("/work/crook", None)),
        Some("git pull --rebase"),
        "with nothing run yet, the newest here"
    );
}

#[test]
fn test_a_command_that_failed_gives_way_to_its_correction() {
    // The typo is newer and run in the same directory; it failed.
    let recall = Recall::in_memory(vec![
        ran("cargo test -p crook", "/work/crook", 0, None),
        ran("cargo test -p crok", "/work/crook", 101, None),
    ]);

    assert_eq!(
        recall.best("cargo test", &[], &here("/work/crook", None)),
        Some("cargo test -p crook")
    );
}

#[test]
fn test_a_failed_command_does_not_come_back_through_the_plain_history() {
    // The field's own history holds every line sent, the failed one included.
    let plain = vec!["make deploy-prod".to_owned()];
    let recall = Recall::in_memory(vec![
        ran("make deploy", "/work/site", 0, None),
        ran("make deploy-prod", "/work/site", 2, None),
    ]);

    assert_eq!(
        recall.best("make d", &plain, &here("/work/site", None)),
        Some("make deploy")
    );
}

#[test]
fn test_with_nothing_recorded_the_plain_history_is_newest_first() {
    let plain = vec!["git status".to_owned(), "git stash".to_owned()];
    let recall = Recall::default();

    assert_eq!(
        recall.best("git st", &plain, &Context::default()),
        Some("git stash")
    );
    assert_eq!(recall.best("", &plain, &Context::default()), None);
    assert_eq!(
        recall.best("git stash", &plain, &Context::default()),
        None,
        "a line nothing extends has nothing to offer"
    );
}

#[test]
fn test_a_command_typed_with_a_leading_blank_is_not_remembered() {
    let mut recall = Recall::default();
    recall.remember(ran(" export TOKEN=secret", "/work", 0, None));
    recall.remember(ran("   ", "/work", 0, None));
    recall.remember(ran("ls", "/work", 0, None));

    let commands: Vec<_> = recall
        .records()
        .iter()
        .map(|record| record.command.as_str())
        .collect();
    assert_eq!(commands, ["ls"]);
}

#[test]
fn test_records_are_appended_to_the_file_and_read_back() {
    let directory = std::env::temp_dir().join(format!("crook-recall-{}", std::process::id()));
    let _ = fs::remove_dir_all(&directory);
    let path = directory.join("nested").join("history.jsonl");

    let mut recall = Recall::open(path.clone());
    recall.remember(ran("cargo build", "/work/crook", 0, None));
    recall.remember(ran("cargo test", "/work/crook", 1, Some("cargo build")));
    fs::write(
        &path,
        format!("{}not json\n", fs::read_to_string(&path).expect("written")),
    )
    .expect("a bad line can be added");

    let again = Recall::open(path);
    assert_eq!(
        again.records(),
        recall.records(),
        "one bad line costs one line"
    );
    let _ = fs::remove_dir_all(&directory);
}

#[test]
fn test_a_file_grown_past_its_bound_is_cut_to_its_newest_records() {
    let directory = std::env::temp_dir().join(format!("crook-recall-trim-{}", std::process::id()));
    let _ = fs::remove_dir_all(&directory);
    fs::create_dir_all(&directory).expect("a scratch directory");
    let path = directory.join("history.jsonl");

    let padding = "x".repeat(900);
    let mut text = String::new();
    let mut index = 0;
    while text.len() as u64 <= TRIM_AT {
        let record = ran(&format!("echo {index} {padding}"), "/work", 0, None);
        text.push_str(&serde_json::to_string(&record).expect("serializes"));
        text.push('\n');
        index += 1;
    }
    fs::write(&path, &text).expect("written");

    let recall = Recall::open(path.clone());
    let size = fs::metadata(&path).expect("still there").len();
    assert!(
        size <= TAIL,
        "the file was cut to what is read back: {size} bytes"
    );
    let newest = recall.records().last().expect("records survive");
    assert!(newest.command.starts_with(&format!("echo {} ", index - 1)));
    let _ = fs::remove_dir_all(&directory);
}

#[test]
fn test_the_history_list_puts_what_fits_here_first_and_says_where_each_ran() {
    let mut cargo_here = ran("cargo test", "/work/crook", 0, None);
    cargo_here.at = 1_000;
    let mut make_elsewhere = ran("make", "/work/other", 2, None);
    make_elsewhere.at = 4_000;
    let mut git_here_old = ran("git status", "/work/crook", 0, None);
    git_here_old.at = 500;
    let mut cargo_again = ran("cargo test", "/work/crook", 0, None);
    cargo_again.at = 3_000;
    let recall = Recall::in_memory(vec![git_here_old, cargo_here, make_elsewhere, cargo_again]);
    let plain = ["ls".to_owned(), "make".to_owned(), "htop".to_owned()];

    let listed = recall.listed(&plain, &here("/work/crook", None), 3_600 + 3_000);

    assert_eq!(
        listed.candidates,
        ["cargo test", "git status", "make", "htop", "ls"],
        "run here, newest first; then elsewhere; then the shell's own lines, \
         newest first and once each"
    );
    assert_eq!(listed.descriptions[0], "here · 1h", "from its newest run");
    assert_eq!(listed.descriptions[2], "/work/other · 43m · failed (2)");
    assert_eq!(
        listed.descriptions[3], "",
        "nothing is known about a plain line"
    );
}

#[test]
fn test_ago_reads_in_one_unit() {
    assert_eq!(ago(5), "now");
    assert_eq!(ago(90), "1m");
    assert_eq!(ago(7_200), "2h");
    assert_eq!(ago(86_400 * 3), "3d");
    assert_eq!(ago(86_400 * 30), "4w");
}

#[test]
fn test_the_list_order_is_outranked_only_by_a_clearly_better_match() {
    let mut old_commit = ran("git commit -m wip", "/work/crook", 0, None);
    old_commit.at = 100;
    let mut new_checkout = ran("git checkout main", "/work/crook", 0, None);
    new_checkout.at = 200;
    let mut cargo = ran("cargo test -p crook", "/work/crook", 0, None);
    cargo.at = 300;
    let recall = Recall::in_memory(vec![old_commit, new_checkout, cargo]);
    let listed = recall.listed(&[], &here("/work/crook", None), 400);

    let order: Vec<_> = listed
        .ranked("gco")
        .into_iter()
        .map(|found| found.candidate)
        .collect();
    assert_eq!(
        order,
        [
            "git checkout main",
            "git commit -m wip",
            "cargo test -p crook"
        ],
        "`co` side by side in `commit` is not worth a week; `g` in the middle \
         of `cargo` starts no word, and that is"
    );
}

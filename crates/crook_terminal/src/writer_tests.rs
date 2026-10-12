use std::sync::mpsc::{self, Receiver, Sender};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use super::*;

/// How long a test waits for the writer before calling it stuck rather than
/// hanging the suite.
const DEADLINE: Duration = Duration::from_secs(10);

/// A link that takes every write the moment it is made and reports it.
struct Taken(Sender<Vec<u8>>);

impl Write for Taken {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        let _ = self.0.send(bytes.to_vec());
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

/// A link whose child reads nothing until the test says so: the first write
/// waits for the gate, the way a write to a pty waits for a child that is not
/// reading, and says it has started waiting.
struct Gated {
    taken: Sender<Vec<u8>>,
    waiting: Option<Sender<()>>,
    gate: Option<Receiver<()>>,
}

impl Write for Gated {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if let Some(gate) = self.gate.take() {
            if let Some(waiting) = self.waiting.take() {
                let _ = waiting.send(());
            }
            let _ = gate.recv();
        }
        let _ = self.taken.send(bytes.to_vec());
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

/// A link that refuses everything, the way a pty whose child is gone does.
struct Refusing;

impl Write for Refusing {
    fn write(&mut self, _: &[u8]) -> io::Result<usize> {
        Err(io::Error::new(io::ErrorKind::BrokenPipe, "nobody is there"))
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

/// A queue over `link`, with its writer running on a thread.
fn running(link: impl Write + Send + 'static) -> (Outbox, JoinHandle<io::Result<()>>) {
    let (outbox, inbox) = queue();
    let writer = inbox.into_writer(Box::new(link));
    (outbox, thread::spawn(move || writer.run()))
}

/// A queue whose writer is already waiting on a child that reads nothing
/// until the gate opens.
struct Stuck {
    outbox: Outbox,
    writer: JoinHandle<io::Result<()>>,
    /// What the link has been handed.
    taken: Receiver<Vec<u8>>,
    /// Sending on it lets the child read.
    gate: Sender<()>,
}

/// A queue over a [`Gated`] link, its writer running on a thread and waiting
/// on the child with `first`.
fn stuck(first: &[u8]) -> Stuck {
    let (sent, taken) = mpsc::channel();
    let (waiting, started_waiting) = mpsc::channel();
    let (open, gate) = mpsc::channel();
    let (outbox, writer) = running(Gated {
        taken: sent,
        waiting: Some(waiting),
        gate: Some(gate),
    });
    outbox.input(first).expect("queued");
    started_waiting
        .recv_timeout(DEADLINE)
        .expect("the writer never started writing");
    Stuck {
        outbox,
        writer,
        taken,
        gate: open,
    }
}

/// Everything the link has been handed once `length` bytes have arrived, or
/// what had arrived when the deadline passed.
fn collect(taken: &Receiver<Vec<u8>>, length: usize) -> Vec<u8> {
    let deadline = Instant::now() + DEADLINE;
    let mut collected = Vec::new();
    while collected.len() < length {
        let left = deadline.saturating_duration_since(Instant::now());
        let Ok(bytes) = taken.recv_timeout(left) else {
            break;
        };
        collected.extend_from_slice(&bytes);
    }
    collected
}

/// One read's answers, numbered so the test can tell which ones survived:
/// `ESC [ <n> ; 1 R`, a cursor report, `count` times over.
fn answers(number: usize, count: usize) -> Vec<u8> {
    format!("\x1b[{number};1R").repeat(count).into_bytes()
}

#[test]
fn test_keys_pastes_and_answers_reach_the_link_in_the_order_they_were_queued() {
    let (sent, taken) = mpsc::channel();
    let (outbox, writer) = running(Taken(sent));

    outbox.input(b"ls").expect("queued");
    outbox.reply(b"\x1b[1;1R".to_vec()).expect("queued");
    outbox.input(b" -la").expect("queued");
    outbox.input(b"\r").expect("queued");
    outbox.reply(b"\x1b[?6c".to_vec()).expect("queued");

    let expected = b"ls\x1b[1;1R -la\r\x1b[?6c".to_vec();
    assert_eq!(expected, collect(&taken, expected.len()));

    drop(outbox);
    assert!(
        writer.join().expect("the writer panicked").is_ok(),
        "a terminal that went away is the end of the writer's work, not an error"
    );
}

#[test]
fn test_a_writer_waiting_on_its_child_holds_up_nobody_who_queues() {
    // The whole bug, without a process: the thread that queues — the UI
    // thread with a paste, the reader with an answer — must never wait on a
    // child that is not reading.
    let Stuck {
        outbox,
        writer,
        taken,
        gate,
    } = stuck(b"first");

    let paste = vec![b'x'; 1024 * 1024];
    let queueing = Instant::now();
    outbox
        .input(&paste)
        .expect("queued behind a write that waits");
    outbox
        .reply(b"\x1b[1;1R".to_vec())
        .expect("queued behind a write that waits");
    assert!(
        queueing.elapsed() < Duration::from_secs(1),
        "queueing waited on the child for {:?}",
        queueing.elapsed()
    );

    gate.send(()).expect("the writer is still waiting");
    let mut expected = b"first".to_vec();
    expected.extend_from_slice(&paste);
    expected.extend_from_slice(b"\x1b[1;1R");
    assert!(
        collect(&taken, expected.len()) == expected,
        "the paste did not arrive whole and in order"
    );

    drop(outbox);
    writer
        .join()
        .expect("the writer panicked")
        .expect("the link took everything");
}

#[test]
fn test_answers_a_child_is_not_reading_are_dropped_oldest_first_and_input_never() {
    // A program that floods queries and never reads would otherwise grow the
    // queue without end. What a person typed in between is kept, all of it,
    // where it was.
    let Stuck {
        outbox,
        writer,
        taken,
        gate,
    } = stuck(b"<stuck>");

    // A batch is a hundred reports of seven to nine bytes, so four hundred of
    // them are several times the backlog.
    let batches = 400;
    let mut typed = vec!["<stuck>".to_owned()];
    for number in 0..batches {
        outbox
            .reply(answers(number, 100))
            .expect("an answer is never refused");
        if number % 50 == 0 {
            let key = format!("<key {number}>");
            outbox.input(key.as_bytes()).expect("queued");
            typed.push(key);
        }
    }
    gate.send(()).expect("the writer is still waiting");

    // What arrives is every key in the order it was typed, and the newest
    // batches of answers that fit in the backlog.
    let kept: usize = (0..batches)
        .rev()
        .map(|number| answers(number, 100).len())
        .scan(0, |total, length| {
            *total += length;
            (*total <= REPLY_BACKLOG).then_some(length)
        })
        .sum();
    let length = typed.iter().map(String::len).sum::<usize>() + kept;
    let arrived = String::from_utf8(collect(&taken, length)).expect("all of it is text");
    drop(outbox);
    writer
        .join()
        .expect("the writer panicked")
        .expect("the link took everything");

    let keys: Vec<String> = arrived
        .split('<')
        .skip(1)
        .map(|key| format!("<{}>", key.split('>').next().unwrap_or_default()))
        .collect();
    assert_eq!(typed, keys, "a key was dropped or moved");

    let numbers: Vec<usize> = arrived
        .split("\x1b[")
        .skip(1)
        .filter_map(|report| report.split(';').next()?.parse().ok())
        .collect();
    assert_eq!(
        Some(&(batches - 1)),
        numbers.last(),
        "the newest answers were dropped"
    );
    assert!(
        !numbers.contains(&0),
        "the oldest answers were kept past the backlog"
    );
    let replies: usize = numbers
        .iter()
        .map(|number| format!("\x1b[{number};1R").len())
        .sum();
    assert!(
        replies <= REPLY_BACKLOG,
        "{replies} bytes of answers were kept for a child that read none"
    );
    // Dropped a batch at a time, so every batch that arrived arrived whole.
    assert_eq!(0, numbers.len() % 100, "a batch was cut");
}

#[test]
fn test_the_answers_to_one_read_are_kept_however_many_there_are() {
    // The bound is on what a child that is not reading leaves behind; one
    // batch on its own is what a child that is reading is about to read.
    let (outbox, inbox) = queue();
    let big = answers(1, REPLY_BACKLOG / 4);
    assert!(big.len() > REPLY_BACKLOG);

    outbox.reply(big.clone()).expect("queued");

    let (sent, taken) = mpsc::channel();
    let writer = inbox.into_writer(Box::new(Taken(sent)));
    let running = thread::spawn(move || writer.run());
    assert!(collect(&taken, big.len()) == big, "the batch was dropped");
    drop(outbox);
    running
        .join()
        .expect("the writer panicked")
        .expect("the link took everything");
}

#[test]
fn test_a_link_that_stops_taking_input_makes_every_later_write_an_error() {
    // The child closed the pty: queueing more for it would hold bytes that
    // can never be written, and the caller is owed the news.
    let (outbox, writer) = running(Refusing);
    outbox
        .input(b"lost")
        .expect("queued before the link failed");

    let ended = writer.join().expect("the writer panicked");
    assert!(ended.is_err(), "the link's failure was not reported");
    assert_eq!(
        io::ErrorKind::BrokenPipe,
        outbox.input(b"more").expect_err("queued for nobody").kind()
    );
    assert!(
        outbox.reply(b"\x1b[1;1R".to_vec()).is_err(),
        "an answer was queued for nobody"
    );
}

#[test]
fn test_a_writer_that_never_ran_makes_every_write_an_error() {
    // A thread that could not be started drops the writer unrun. The
    // terminal must not go on queueing into a queue nothing will empty.
    let (outbox, inbox) = queue();
    drop(inbox.into_writer(Box::new(io::sink())));
    assert!(outbox.input(b"typed").is_err());
}

#[test]
fn test_a_terminal_that_goes_away_ends_its_writer() {
    // Idle, on the queue: the terminal closing is the one thing it waits for
    // besides work.
    let (sent, _taken) = mpsc::channel();
    let (outbox, writer) = running(Taken(sent));
    thread::sleep(Duration::from_millis(20));

    drop(outbox);
    let (done, finished) = mpsc::channel();
    thread::spawn(move || {
        let _ = done.send(writer.join());
    });
    finished
        .recv_timeout(DEADLINE)
        .expect("the writer outlived its terminal")
        .expect("the writer panicked")
        .expect("ending with the terminal is not an error");
}

#[test]
fn test_input_past_the_backlog_is_refused_but_a_paste_of_any_size_is_not() {
    // Nothing is writing, so everything stays queued: a child that has read
    // nothing for a very long time.
    let (outbox, _inbox) = queue();
    let paste = vec![b'x'; INPUT_BACKLOG + 1];

    outbox
        .input(&paste)
        .expect("a paste is refused only behind a backlog, never for its own size");
    assert_eq!(
        io::ErrorKind::WouldBlock,
        outbox
            .input(b"more")
            .expect_err("queued behind a backlog nothing reads")
            .kind()
    );
    outbox
        .reply(b"\x1b[1;1R".to_vec())
        .expect("answers are bounded by dropping, not refusing");
}

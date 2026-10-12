use super::*;

#[test]
fn test_a_piece_ends_at_the_last_line_end_that_fits() {
    // Two lines and the start of a third fit: the third is left for the next
    // piece, so that no line is left half written in the queue.
    let bytes = b"one\ntwo\r\nthree four five";
    assert_eq!(b"one\ntwo\r\n", piece(bytes, 12));
}

#[test]
fn test_a_piece_of_a_line_longer_than_the_room_is_cut_at_the_room() {
    assert_eq!(b"abcd", piece(b"abcdefgh\n", 4));
}

#[test]
fn test_a_piece_that_is_everything_left_is_written_whole() {
    // Nothing follows it to keep apart from it, so a line it leaves unfinished
    // is the one the writer was given.
    assert_eq!(b"one\ntwo", piece(b"one\ntwo", ROOM));
}

#[test]
fn test_in_canonical_mode_a_piece_leaves_room_for_the_line_that_may_be_unfinished() {
    assert_eq!(ROOM, room(false, 300));
    assert_eq!(ROOM - 300, room(true, 300));
    assert_eq!(ROOM, room(true, 0));
}

#[test]
fn test_a_line_that_may_fill_the_queue_leaves_no_room_until_it_is_read() {
    // Anything more would be past the kernel's line limit, and thrown away.
    assert_eq!(0, room(true, ROOM));
    assert_eq!(0, room(true, 5 * ROOM));
    assert_eq!(ROOM, room(false, 5 * ROOM));
}

#[test]
fn test_what_was_written_since_the_last_line_end_is_counted_across_writes() {
    assert_eq!(7, unended(4, b"abc"));
    assert_eq!(2, unended(4, b"a\nbc"));
    assert_eq!(0, unended(4, b"abc\r"));
    assert_eq!(4, unended(4, b""));
}

use super::*;

fn store(lines: &[&str]) -> Arc<LogStore> {
    let s = LogStore::new(None).unwrap();
    for (i, l) in lines.iter().enumerate() {
        s.push(if i % 2 == 0 { "stdout" } else { "stderr" }, l);
    }
    s
}

fn texts(lines: &[Line]) -> Vec<&str> {
    lines.iter().map(|l| l.text.as_str()).collect()
}

#[test]
fn newest_lines_come_back_oldest_first() {
    let s = store(&["a", "b", "c", "d"]);
    let t = s.tail(2, None, None);
    assert_eq!(texts(&t.lines), vec!["c", "d"]);
    assert_eq!(t.cursor, 4, "the cursor is the next line to read");
    assert!(!t.dropped);
}

#[test]
fn filters_are_regular_expressions_over_the_text() {
    let s = store(&["info: loaded", "warn: slow", "info: done"]);
    let re = Regex::new("^info").unwrap();
    let t = s.tail(10, Some(&re), None);
    assert_eq!(texts(&t.lines), vec!["info: loaded", "info: done"]);
    assert_eq!(t.cursor, 3, "the cursor is the newest line, filtered or not");
}

#[test]
fn a_cursor_pages_through_the_lines() {
    let s = store(&["1", "2", "3", "4", "5"]);
    let first = s.tail(2, None, Some(u64::MAX - 1));
    assert!(first.lines.is_empty());
    let page = s.tail(2, None, Some(0));
    assert_eq!(texts(&page.lines), vec!["1", "2"]);
    assert_eq!(page.cursor, 2, "a full page resumes after its last line");
    let rest = s.tail(10, None, Some(page.cursor));
    assert_eq!(texts(&rest.lines), vec!["3", "4", "5"]);
    assert_eq!(rest.cursor, 5);
}

#[test]
fn a_cursor_taken_before_any_line_sees_the_first_line() {
    let s = LogStore::new(None).unwrap();
    let empty = s.tail(10, None, None);
    assert!(empty.lines.is_empty());
    s.push("stdout", "first");
    let next = s.tail(10, None, Some(empty.cursor));
    assert_eq!(texts(&next.lines), vec!["first"]);
    assert!(!next.dropped);
}

#[test]
fn lines_pushed_to_the_file_keep_their_sequence_numbers() {
    let dir = std::env::temp_dir().join(format!("k2-mcp-logs-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let path = dir.join("run.log");
    let s = LogStore::new(Some(&path)).unwrap();
    s.push("stdout", "hello");
    s.push("stderr", "oops");
    let written = std::fs::read_to_string(&path).unwrap();
    assert!(written.contains(&format!("{:>8} [stdout] hello", 0)), "the first line is number 0: {written}");
    assert!(written.contains(&format!("{:>8} [stderr] oops", 1)), "the second line is number 1: {written}");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_run_log_that_is_a_link_is_refused_and_its_target_is_left_alone() {
    let dir = std::env::temp_dir().join(format!("k2-mcp-logs-link-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let outside = dir.join("outside.txt");
    std::fs::write(&outside, b"keep").unwrap();
    let path = dir.join("run.log");
    std::os::unix::fs::symlink(&outside, &path).unwrap();
    assert!(LogStore::new(Some(&path)).is_err(), "a link at the log path is refused");
    assert_eq!(std::fs::read(&outside).unwrap(), b"keep", "the link's target is not written");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn the_ring_drops_the_oldest_lines() {
    let s = LogStore::new(None).unwrap();
    for i in 0..(RING_CAP + 5) {
        s.push("stdout", &i.to_string());
    }
    let t = s.tail(1, None, Some(0));
    assert!(t.dropped, "a cursor older than the ring reports the gap");
    assert_eq!(s.last(1)[0].text, (RING_CAP + 4).to_string());
}

#[test]
fn the_log_file_stops_at_its_cap_with_one_marker() {
    let dir = std::env::temp_dir().join(format!("k2-mcp-logcap-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let path = dir.join("run.log");
    let s = LogStore::with_file_cap(Some(&path), 1024).unwrap();
    let line = "y".repeat(100);
    for _ in 0..50 {
        s.push("stdout", &line);
    }
    let written = std::fs::read_to_string(&path).unwrap();
    assert!(written.len() as u64 <= 1024 + 200, "the file stops near its cap: {}", written.len());
    assert_eq!(written.matches("[log] the file reached").count(), 1, "one marker, then nothing");
    assert_eq!(s.last(1)[0].text, line, "the ring keeps the newest lines");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn the_ring_is_bounded_in_bytes_too() {
    let s = LogStore::new(None).unwrap();
    let big = "x".repeat(1024 * 1024);
    for _ in 0..40 {
        s.push("stdout", &big);
    }
    let held: usize = s.last(usize::MAX).iter().map(|l| l.text.len()).sum();
    assert!(held <= RING_BYTES, "held {held} bytes");
    let t = s.tail(1, None, Some(0));
    assert!(t.dropped, "the lines that went over the budget are reported as dropped");
}

#[test]
fn lines_split_on_newlines_and_lose_a_carriage_return() {
    let mut reader = io::Cursor::new(b"a\r\nbb\ncc".to_vec());
    let mut out = Vec::new();
    assert_eq!(read_capped_line(&mut reader, &mut out, 64).unwrap(), Some(0));
    assert_eq!(out, b"a");
    assert_eq!(read_capped_line(&mut reader, &mut out, 64).unwrap(), Some(0));
    assert_eq!(out, b"bb");
    assert_eq!(read_capped_line(&mut reader, &mut out, 64).unwrap(), Some(0), "a last line without a newline still counts");
    assert_eq!(out, b"cc");
    assert_eq!(read_capped_line(&mut reader, &mut out, 64).unwrap(), None, "the end of the stream");
}

#[test]
fn a_long_line_is_cut_to_the_cap_and_the_rest_is_skipped() {
    let mut reader = io::Cursor::new(b"abcdefgh\nok\n".to_vec());
    let mut out = Vec::new();
    assert_eq!(read_capped_line(&mut reader, &mut out, 3).unwrap(), Some(5));
    assert_eq!(out, b"abc");
    assert_eq!(read_capped_line(&mut reader, &mut out, 3).unwrap(), Some(0), "the next line starts clean");
    assert_eq!(out, b"ok");
}

#[test]
fn invalid_utf8_is_read_through() {
    let mut reader = io::Cursor::new(b"\xff\xfeok\nnext\n".to_vec());
    let mut out = Vec::new();
    assert_eq!(read_capped_line(&mut reader, &mut out, 64).unwrap(), Some(0));
    assert_eq!(String::from_utf8_lossy(&out), "\u{fffd}\u{fffd}ok");
    assert_eq!(read_capped_line(&mut reader, &mut out, 64).unwrap(), Some(0));
    assert_eq!(out, b"next");
}

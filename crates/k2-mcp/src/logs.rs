//! The game's output: a ring of recent lines that logs_tail reads with a
//! regex filter and a cursor, and a file per run that keeps everything.

use std::collections::VecDeque;
use std::fs::{File, OpenOptions};
use std::io::{self, BufRead, BufReader, Read, Write};
use std::os::unix::fs::OpenOptionsExt;
use std::path::Path;
use std::sync::{Arc, Mutex};

use regex::Regex;
use serde::Serialize;

/// Lines kept in memory per run.
pub const RING_CAP: usize = 20_000;
/// Text bytes kept in memory per run; the oldest lines go first past this.
pub const RING_BYTES: usize = 32 * 1024 * 1024;
/// Bytes of a run's log file. Past this the file gets one marker line and no
/// more; the ring keeps the newest lines.
pub const LOG_FILE_CAP: u64 = 64 * 1024 * 1024;
/// Bytes kept of one line; a longer line is cut and says how much was cut.
pub const LINE_CAP: usize = 16 * 1024;

/// One line of the game's output.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Line {
    /// Increases by one per line within a run.
    pub seq: u64,
    /// "stdout" or "stderr".
    pub stream: &'static str,
    pub text: String,
}

/// What logs_tail returns.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Tail {
    pub lines: Vec<Line>,
    /// Pass as `since` to continue with the lines after these.
    pub cursor: u64,
    /// Lines between `since` and the oldest kept line were dropped from the ring.
    pub dropped: bool,
}

#[derive(Default)]
struct Ring {
    lines: VecDeque<Line>,
    /// Text bytes held by `lines`.
    bytes: usize,
    /// Sequence number of the next line.
    next: u64,
}

/// The run's log file, with its size cap.
struct FileSink {
    file: File,
    written: u64,
    cap: u64,
    full: bool,
}

impl FileSink {
    fn write_line(&mut self, seq: u64, stream: &str, text: &str) {
        if self.full {
            return;
        }
        let line = format!("{seq:>8} [{stream}] {text}\n");
        if self.written + line.len() as u64 > self.cap {
            self.full = true;
            let _ = writeln!(self.file, "{seq:>8} [log] the file reached its {} MiB cap; later output is kept in memory only", self.cap / (1024 * 1024));
            return;
        }
        if self.file.write_all(line.as_bytes()).is_ok() {
            self.written += line.len() as u64;
        }
    }
}

/// Ring of recent lines plus the run's log file.
pub struct LogStore {
    ring: Mutex<Ring>,
    file: Mutex<Option<FileSink>>,
}

impl LogStore {
    /// A store that also appends every line to `path` (created with its folder,
    /// owner-only), up to LOG_FILE_CAP bytes.
    pub fn new(path: Option<&Path>) -> std::io::Result<Arc<Self>> {
        Self::with_file_cap(path, LOG_FILE_CAP)
    }

    /// [`LogStore::new`] with its own file cap.
    pub fn with_file_cap(path: Option<&Path>, cap: u64) -> std::io::Result<Arc<Self>> {
        let file = match path {
            Some(p) => {
                if let Some(dir) = p.parent() {
                    std::fs::create_dir_all(dir)?;
                }
                let file = OpenOptions::new().create(true).append(true).mode(0o600).custom_flags(libc::O_NOFOLLOW).open(p)?;
                let written = file.metadata()?.len();
                Some(FileSink { file, written, cap, full: written >= cap })
            }
            None => None,
        };
        Ok(Arc::new(Self { ring: Mutex::new(Ring::default()), file: Mutex::new(file) }))
    }

    /// Records one line.
    pub fn push(&self, stream: &'static str, text: &str) {
        let seq = {
            let mut ring = self.ring.lock().unwrap_or_else(|e| e.into_inner());
            let seq = ring.next;
            ring.next += 1;
            ring.lines.push_back(Line { seq, stream, text: text.to_owned() });
            ring.bytes += text.len();
            while ring.lines.len() > RING_CAP || ring.bytes > RING_BYTES {
                match ring.lines.pop_front() {
                    Some(old) => ring.bytes -= old.text.len(),
                    None => break,
                }
            }
            seq
        };
        if let Some(sink) = self.file.lock().unwrap_or_else(|e| e.into_inner()).as_mut() {
            sink.write_line(seq, stream, text);
        }
    }

    /// The newest `n` lines (oldest first), or with `since`, the lines from that
    /// cursor on (oldest first, at most `n`). Only lines matching `filter`.
    /// A cursor is the sequence number of the next line to read: a page that
    /// stopped early resumes after its last line.
    pub fn tail(&self, n: usize, filter: Option<&Regex>, since: Option<u64>) -> Tail {
        let ring = self.ring.lock().unwrap_or_else(|e| e.into_inner());
        let oldest = ring.lines.front().map_or(ring.next, |l| l.seq);
        let matches = |l: &&Line| filter.is_none_or(|re| re.is_match(&l.text));
        let (lines, dropped): (Vec<Line>, bool) = match since {
            None => {
                let mut picked: Vec<Line> = ring.lines.iter().rev().filter(matches).take(n).cloned().collect();
                picked.reverse();
                (picked, false)
            }
            Some(s) => {
                let dropped = s < oldest;
                let picked: Vec<Line> = ring.lines.iter().filter(|l| l.seq >= s).filter(matches).take(n).cloned().collect();
                (picked, dropped)
            }
        };
        let cursor = match (since, lines.last()) {
            (Some(_), Some(last)) => last.seq + 1,
            _ => ring.next,
        };
        Tail { lines, cursor, dropped }
    }

    /// The newest `n` lines, unfiltered (for crash reports).
    pub fn last(&self, n: usize) -> Vec<Line> {
        self.tail(n, None, None).lines
    }
}

/// Copies a child stream into the store, one line at a time, until it closes
/// or a read fails for real.
pub fn pump<R: Read + Send + 'static>(stream: R, name: &'static str, store: Arc<LogStore>) {
    std::thread::spawn(move || {
        let mut reader = BufReader::new(stream);
        let mut bytes = Vec::new();
        while let Ok(Some(cut)) = read_capped_line(&mut reader, &mut bytes, LINE_CAP) {
            let mut text = String::from_utf8_lossy(&bytes).into_owned();
            if cut > 0 {
                text.push_str(&format!(" [{cut} bytes cut]"));
            }
            store.push(name, &text);
        }
    });
}

/// Reads one line into `out`, keeping at most `cap` bytes of it and dropping
/// the rest, so a runaway line cannot grow without bound. Returns None at the
/// end of the stream, else how many bytes were cut. A line ends at "\n" (a "\r"
/// before it is dropped). Interrupted reads are retried; bytes need not be UTF-8.
fn read_capped_line<R: BufRead>(reader: &mut R, out: &mut Vec<u8>, cap: usize) -> io::Result<Option<usize>> {
    out.clear();
    let mut cut = 0;
    let mut saw_bytes = false;
    loop {
        let available = match reader.fill_buf() {
            Ok(bytes) => bytes,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(e),
        };
        if available.is_empty() {
            return Ok(saw_bytes.then_some(cut));
        }
        saw_bytes = true;
        let newline = available.iter().position(|&b| b == b'\n');
        let chunk = &available[..newline.unwrap_or(available.len())];
        let keep = chunk.len().min(cap.saturating_sub(out.len()));
        out.extend_from_slice(&chunk[..keep]);
        cut += chunk.len() - keep;
        let used = newline.map_or(available.len(), |i| i + 1);
        reader.consume(used);
        if newline.is_some() {
            if out.last() == Some(&b'\r') {
                out.pop();
            }
            return Ok(Some(cut));
        }
    }
}

#[cfg(test)]
mod tests;

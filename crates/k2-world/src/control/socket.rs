//! Control socket: newline-delimited JSON over a unix socket. The game serves
//! it (crates/kotor2 remote.rs) and the k2-mcp attach backend connects to it.
//! One request line per connection-request, one reply line back. A request
//! that starts a goal is answered when the goal finishes, so the connection
//! simply waits.

use std::collections::VecDeque;
use std::io::{self, BufRead, BufReader, ErrorKind, Read, Write};
use std::os::unix::fs::{FileTypeExt, MetadataExt, PermissionsExt};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::time::Duration;

use super::{Command, Reply};

/// File name of the socket inside the control directory.
pub const SOCKET_FILE: &str = "control.sock";
/// Longest request line accepted (bytes).
const MAX_LINE: usize = 1 << 20;
/// Clients connected at once. A connection past this is closed at once.
const MAX_CLIENTS: usize = 16;
/// Reply bytes a client may fall behind by before it is dropped.
const MAX_OUT: usize = 1 << 20;

/// The control directory: `K2_CTL_DIR`, else the game's default
/// (`$XDG_DATA_HOME/kotor2-rs/ctl`), the same one the restart channel uses.
pub fn default_dir() -> PathBuf {
    if let Some(dir) = std::env::var_os("K2_CTL_DIR").filter(|v| !v.is_empty()) {
        return PathBuf::from(dir);
    }
    crate::save::default_save_root().parent().map_or_else(|| PathBuf::from("ctl"), |p| p.join("ctl"))
}

/// Path of the socket inside a control directory.
pub fn socket_path(dir: &Path) -> PathBuf {
    dir.join(SOCKET_FILE)
}

/// A request that arrived and waits for [`Server::reply`].
#[derive(Debug)]
pub struct Incoming {
    /// Identifies the connection for the reply.
    pub client: u64,
    pub command: Command,
}

struct Client {
    id: u64,
    stream: UnixStream,
    /// Request bytes read but not yet a whole line.
    buf: Vec<u8>,
    /// Reply bytes the socket has not taken yet.
    out: VecDeque<u8>,
}

/// The serving end. Never blocks: call [`Server::poll`] once per frame.
pub struct Server {
    path: PathBuf,
    listener: UnixListener,
    /// Device and inode of the socket file this server created.
    ident: (u64, u64),
    clients: Vec<Client>,
    next: u64,
}

impl Server {
    /// Binds `path`. A socket left by an earlier run is replaced; a live server
    /// answering there, or a file that is not a socket, is refused. The socket
    /// is set to owner-only (0600) right after the bind: anyone who could
    /// connect would control the game. The folder is private as well (the game
    /// creates it 0700), so the brief window before the chmod is closed. A
    /// umask is not used: it is process-wide and would strip the execute bit
    /// from directories other threads create meanwhile.
    pub fn bind(path: &Path) -> io::Result<Self> {
        match std::fs::symlink_metadata(path) {
            Ok(meta) if meta.file_type().is_socket() => {
                if UnixStream::connect(path).is_ok() {
                    return Err(io::Error::new(ErrorKind::AddrInUse, format!("a live control socket answers at {}", path.display())));
                }
                std::fs::remove_file(path)?;
            }
            Ok(_) => return Err(io::Error::new(ErrorKind::AlreadyExists, format!("{} exists and is not a socket", path.display()))),
            Err(e) if e.kind() == ErrorKind::NotFound => {}
            Err(e) => return Err(e),
        }
        let listener = UnixListener::bind(path)?;
        if let Err(e) = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600)) {
            let _ = std::fs::remove_file(path);
            return Err(e);
        }
        listener.set_nonblocking(true)?;
        let meta = std::fs::symlink_metadata(path)?;
        Ok(Self { path: path.to_owned(), listener, ident: (meta.dev(), meta.ino()), clients: Vec::new(), next: 0 })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Accepts connections and reads what arrived. Lines that do not parse
    /// are answered at once; the rest are returned in arrival order.
    pub fn poll(&mut self) -> Vec<Incoming> {
        while let Ok((stream, _)) = self.listener.accept() {
            if self.clients.len() >= MAX_CLIENTS || stream.set_nonblocking(true).is_err() {
                continue;
            }
            self.next += 1;
            self.clients.push(Client { id: self.next, stream, buf: Vec::new(), out: VecDeque::new() });
        }
        let mut out = Vec::new();
        let mut gone = Vec::new();
        for c in &mut self.clients {
            let alive = read_waiting(c);
            while let Some(end) = c.buf.iter().position(|&b| b == b'\n') {
                let line: Vec<u8> = c.buf.drain(..=end).collect();
                match serde_json::from_slice::<Command>(&line) {
                    Ok(command) => out.push(Incoming { client: c.id, command }),
                    Err(e) => {
                        if !queue(c, &Reply::err(format!("bad request: {e}"))) {
                            gone.push(c.id);
                        }
                    }
                }
            }
            // A line that never ends is a client that cannot be served.
            if !alive || c.buf.len() > MAX_LINE || !flush(c) {
                gone.push(c.id);
            }
        }
        self.clients.retain(|c| !gone.contains(&c.id));
        out
    }

    /// Sends the reply for a request. A client that has gone away is ignored.
    pub fn reply(&mut self, client: u64, reply: &Reply) {
        let Some(i) = self.clients.iter().position(|c| c.id == client) else { return };
        let c = &mut self.clients[i];
        if !queue(c, reply) || !flush(c) {
            self.clients.swap_remove(i);
        }
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        // Only the file this server made: a newer server may own the path now.
        if std::fs::symlink_metadata(&self.path).is_ok_and(|m| (m.dev(), m.ino()) == self.ident) {
            let _ = std::fs::remove_file(&self.path);
        }
    }
}

/// Reads what is waiting, stopping once the buffer holds a line's worth.
/// False when the client has closed or failed.
fn read_waiting(c: &mut Client) -> bool {
    let mut chunk = [0u8; 4096];
    while c.buf.len() <= MAX_LINE {
        match c.stream.read(&mut chunk) {
            Ok(0) => return false,
            Ok(n) => c.buf.extend_from_slice(&chunk[..n]),
            Err(e) if e.kind() == ErrorKind::WouldBlock => return true,
            Err(e) if e.kind() == ErrorKind::Interrupted => {}
            Err(_) => return false,
        }
    }
    true
}

/// Queues a reply line. False when the client is too far behind to keep.
fn queue(c: &mut Client, reply: &Reply) -> bool {
    let Ok(mut line) = serde_json::to_string(reply) else { return true };
    line.push('\n');
    if c.out.len() + line.len() > MAX_OUT {
        return false;
    }
    c.out.extend(line.as_bytes());
    true
}

/// Writes queued reply bytes as far as the socket takes them, without
/// blocking. False when the client has gone.
fn flush(c: &mut Client) -> bool {
    while !c.out.is_empty() {
        match c.stream.write(c.out.make_contiguous()) {
            Ok(0) => return false,
            Ok(n) => {
                let _ = c.out.drain(..n);
            }
            Err(e) if e.kind() == ErrorKind::WouldBlock => return true,
            Err(e) if e.kind() == ErrorKind::Interrupted => {}
            Err(_) => return false,
        }
    }
    true
}

/// Client side: connects, sends one command and waits for its reply.
pub fn request(path: &Path, command: &Command) -> io::Result<Reply> {
    request_timeout(path, command, None)
}

/// [`request`] with a limit on how long to wait for the reply. Goals can
/// take game minutes, so the default is no limit.
pub fn request_timeout(path: &Path, command: &Command, timeout: Option<Duration>) -> io::Result<Reply> {
    let mut stream = UnixStream::connect(path)?;
    stream.set_read_timeout(timeout)?;
    stream.set_write_timeout(timeout)?;
    let mut line = serde_json::to_string(command).map_err(io::Error::other)?;
    line.push('\n');
    stream.write_all(line.as_bytes())?;
    let mut reader = BufReader::new(stream);
    loop {
        let mut text = String::new();
        if reader.read_line(&mut text)? == 0 {
            return Err(io::Error::new(ErrorKind::UnexpectedEof, "the control socket closed without a reply"));
        }
        let reply: Reply = serde_json::from_str(text.trim_end()).map_err(io::Error::other)?;
        match interim_wait(&reply) {
            // A held call says how long it may still wait; the read limit follows that, plus the limit.
            Some(wait) => {
                if let Some(limit) = timeout {
                    reader.get_ref().set_read_timeout(Some(Duration::from_secs_f64(wait.max(0.0)) + limit))?;
                }
            }
            None => return Ok(reply),
        }
    }
}

/// The seconds an interim frame says the call may still wait ("interim": "held");
/// None for the call's final reply.
fn interim_wait(reply: &Reply) -> Option<f64> {
    let data = reply.data.as_ref()?;
    (data.get("interim")?.as_str()? == "held").then(|| data.get("wait_s").and_then(|v| v.as_f64()).unwrap_or(0.0))
}

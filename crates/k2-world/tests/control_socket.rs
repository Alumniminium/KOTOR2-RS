//! The control socket (control/socket.rs) driven the way the game drives it:
//! a Server and a Control polled frame by frame over a live World, with
//! clients on their own threads. Requests that start goals are answered when
//! the goal finishes; read-only requests answer during a goal; `stop` cancels
//! a waiting goal and answers the client that was waiting. Skips (passes)
//! when no install is found at K2_INSTALL or ../.. .

use std::io::{BufRead, BufReader, ErrorKind, Read, Write};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use k2_res::Install;
use k2_world::control::socket::{self, Server};
use k2_world::control::{Command, Condition, Control, Env, MoveToArgs, ObjectArgs, Observation, ObserveArgs, Reply, WaitUntilArgs};
use k2_world::object::{Action, Target};

mod support;
use support::{beside_open_placeable, walkable_point_at, Session};

const DT: f32 = 1.0 / 30.0;
/// Upper bound on frames: the test fails instead of hanging.
const MAX_FRAMES: usize = 200_000;

fn install() -> Option<Install> {
    let dir = std::env::var("K2_INSTALL").unwrap_or_else(|_| concat!(env!("CARGO_MANIFEST_DIR"), "/../../..").into());
    Install::discover(dir).ok()
}

fn scratch(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("k2-control-socket-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn request(path: &Path, cmd: Command) -> Reply {
    socket::request(path, &cmd).unwrap_or_else(|e| panic!("socket request failed: {e}"))
}

/// Plays the awakening sequence through (its dialogs, then the scripted gap
/// after them) so the party can walk. Test setup only.
fn settle(s: &mut Session) {
    // The awakening's dialog is queued by the scripts that run on load: let the world run first.
    s.advance(0.5);
    assert!(s.execute(Command::WaitUntil(WaitUntilArgs { condition: Condition::Dialog, timeout: Some(60.0) })).ok);
    for _ in 0..4000 {
        let r = s.execute(Command::Observe(ObserveArgs { radius: Some(1.0) }));
        let obs: Observation = serde_json::from_value(r.data.expect("observe data")).unwrap();
        match &obs.conversation {
            Some(c) if c.replies.iter().all(|r| r.text == "[Continue]") => {
                s.execute(Command::ContinueDialog);
            }
            Some(_) => {
                s.execute(Command::ChooseReply(k2_world::control::ChooseReplyArgs { index: 0 }));
            }
            None => {
                if s.execute(Command::WaitUntil(WaitUntilArgs { condition: Condition::Idle, timeout: Some(2.0) })).ok {
                    return;
                }
                s.advance(0.5);
            }
        }
    }
    panic!("the awakening did not finish");
}

#[test]
fn live_goals_answer_over_the_socket() {
    let Some(install) = install() else {
        let _ = writeln!(std::io::stderr(), "SKIP live_goals_answer_over_the_socket: no install (set K2_INSTALL)");
        return;
    };
    let dir = scratch("live");
    let path = socket::socket_path(&dir);
    let mut server = Server::bind(&path).unwrap();

    let mut s = Session::new(install, dir.join("saves")).unwrap();
    s.new_game("101per", "Exile").unwrap();
    settle(&mut s);
    let mut control = Control::default();
    // A walk is pending, so the wait for the module to change may run; the stop ends it.
    beside_open_placeable(&mut s);
    let from = s.leader_position();
    let goal = [3.0, 4.0, 5.0, 6.0, 8.0].into_iter().find_map(|r| walkable_point_at(&s.world, from, r)).expect("a walkable point 3 to 8 m from the leader");
    let leader = s.world.party_leader();
    s.world.get_mut(leader).expect("the leader is in the world").actions.clear();
    s.world.push_action(leader, Action::MoveTo { target: Target::Point(goal), run: false, range: 0.5, timeout: None, elapsed: 0.0, stuck: 0.0 });

    // Client A: a wait that only a stop ends, then a walk that finishes.
    let a_path = path.clone();
    let a = std::thread::spawn(move || {
        let waited = request(&a_path, Command::WaitUntil(WaitUntilArgs { condition: Condition::AreaLoaded, timeout: Some(30.0) }));
        let observed = request(&a_path, Command::Observe(ObserveArgs { radius: Some(2.0) }));
        let here: Observation = serde_json::from_value(observed.data.expect("observe data")).unwrap();
        let point = here.party[0].position;
        let walked = request(&a_path, Command::MoveTo(MoveToArgs { point: Some(point), object: None, run: None, timeout: Some(20.0) }));
        (waited, walked)
    });
    // Client B: while A waits, observe answers and a second goal is refused; then stop.
    let b_path = path.clone();
    let b = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(60));
        let observed = request(&b_path, Command::Observe(ObserveArgs { radius: None }));
        let busy = request(&b_path, Command::Interact(ObjectArgs { object: 1 }));
        let stopped = request(&b_path, Command::Stop);
        (observed, busy, stopped)
    });

    // The game's frame: finished goals answer their waiter, requests are
    // submitted, then the world ticks.
    let env = Env { save_root: &dir, paused: false, movie: false };
    let mut waiter: Option<u64> = None;
    let mut frames = 0;
    while !(a.is_finished() && b.is_finished()) {
        frames += 1;
        assert!(frames < MAX_FRAMES, "the frame loop never saw the replies");
        if let Some(reply) = control.poll(&mut s.world, &env) {
            if let Some(client) = waiter.take() {
                server.reply(client, &reply);
            }
        }
        for incoming in server.poll() {
            let client = incoming.client;
            match control.submit(&mut s.world, &env, incoming.command) {
                Some(reply) => server.reply(client, &reply),
                None => waiter = Some(client),
            }
        }
        s.world.tick(DT);
        let _ = s.world.drain_events();
        std::thread::sleep(Duration::from_millis(1));
    }

    let (waited, walked) = a.join().unwrap();
    let (observed, busy, stopped) = b.join().unwrap();
    assert!(observed.ok, "observe answers during a goal: {}", observed.message);
    assert!(!busy.ok && busy.message.starts_with("busy:"), "a second goal is refused: {}", busy.message);
    assert!(stopped.ok, "{}", stopped.message);
    assert!(!waited.ok && waited.message.starts_with("cancelled:"), "stop answers the waiter: {}", waited.message);
    assert!(walked.ok, "the walk finishes: {}", walked.message);
    drop(server);
    assert!(!path.exists(), "the socket file is removed when the server drops");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn malformed_lines_are_answered_and_not_forwarded() {
    let dir = scratch("bad");
    let path = socket::socket_path(&dir);
    let mut server = Server::bind(&path).unwrap();
    let client_path = path.clone();
    let client = std::thread::spawn(move || {
        let mut stream = UnixStream::connect(&client_path).unwrap();
        stream.write_all(b"{\"cmd\":\"fly\"}\n").unwrap();
        let mut line = String::new();
        BufReader::new(stream).read_line(&mut line).unwrap();
        line
    });
    let mut forwarded = Vec::new();
    let started = Instant::now();
    while !client.is_finished() && started.elapsed() < Duration::from_secs(10) {
        forwarded.extend(server.poll());
        std::thread::sleep(Duration::from_millis(1));
    }
    let line = client.join().unwrap();
    assert!(forwarded.is_empty(), "nothing reaches the game");
    let reply: Reply = serde_json::from_str(line.trim()).unwrap();
    assert!(!reply.ok && reply.message.starts_with("bad request"), "{}", reply.message);
    drop(server);
    let _ = std::fs::remove_dir_all(&dir);
}

/// A server bound at a fresh path, then polled from a thread-free loop.
fn bound(tag: &str) -> (Server, PathBuf) {
    let dir = scratch(tag);
    let path = socket::socket_path(&dir);
    (Server::bind(&path).unwrap(), path)
}

/// Polls `server` for `for_ms` milliseconds, returning what arrived.
fn poll_for(server: &mut Server, for_ms: u64) -> Vec<socket::Incoming> {
    let mut out = Vec::new();
    let started = Instant::now();
    while started.elapsed() < Duration::from_millis(for_ms) {
        out.extend(server.poll());
        std::thread::sleep(Duration::from_millis(1));
    }
    out
}

#[test]
fn the_socket_is_owner_only() {
    let (server, path) = bound("mode");
    let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode, 0o600, "owner read and write only");
    drop(server);
    let _ = std::fs::remove_dir_all(path.parent().unwrap());
}

#[test]
fn bind_refuses_a_live_server_and_a_regular_file() {
    let (server, path) = bound("refuse");
    let second = Server::bind(&path);
    assert_eq!(second.err().map(|e| e.kind()), Some(ErrorKind::AddrInUse), "a live server keeps its socket");
    drop(server);
    let file = path.parent().unwrap().join("not-a-socket");
    std::fs::write(&file, b"keep me").unwrap();
    assert_eq!(Server::bind(&file).err().map(|e| e.kind()), Some(ErrorKind::AlreadyExists));
    assert_eq!(std::fs::read(&file).unwrap(), b"keep me", "a file that is not a socket is never removed");
    let _ = std::fs::remove_dir_all(path.parent().unwrap());
}

#[test]
fn bind_replaces_a_stale_socket_and_drop_keeps_a_newer_one() {
    let dir = scratch("stale");
    let path = socket::socket_path(&dir);
    // A socket file whose server has exited: the listener is gone, the file stays.
    drop(std::os::unix::net::UnixListener::bind(&path).unwrap());
    let server = Server::bind(&path).expect("a stale socket is replaced");
    drop(server);
    assert!(!path.exists(), "the server removes its own socket");

    let mut first = Server::bind(&path).unwrap();
    std::fs::remove_file(&path).unwrap();
    let newer = std::os::unix::net::UnixListener::bind(&path).unwrap();
    drop(first.poll());
    drop(first);
    assert!(path.exists(), "a server does not remove a socket that replaced its own");
    drop(newer);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn an_unfinished_line_past_the_limit_drops_only_that_client() {
    let (mut server, path) = bound("flood");
    let flood_path = path.clone();
    let flood = std::thread::spawn(move || {
        let mut stream = UnixStream::connect(&flood_path).unwrap();
        let chunk = vec![b'x'; 64 * 1024];
        // Writes until the server closes the connection.
        for _ in 0..64 {
            if stream.write_all(&chunk).is_err() {
                break;
            }
        }
    });
    let started = Instant::now();
    while !flood.is_finished() && started.elapsed() < Duration::from_secs(20) {
        assert!(server.poll().is_empty(), "nothing from a line that never ends");
        std::thread::sleep(Duration::from_millis(1));
    }
    assert!(flood.is_finished(), "the server closed the flooding client");
    flood.join().unwrap();

    let normal = std::thread::spawn({
        let path = path.clone();
        move || request(&path, Command::Info)
    });
    let got = poll_for(&mut server, 2000);
    assert_eq!(got.len(), 1, "a well-behaved client is still served");
    server.reply(got[0].client, &Reply::ok("pong"));
    assert_eq!(normal.join().unwrap().message, "pong");
    let _ = std::fs::remove_dir_all(path.parent().unwrap());
}

#[test]
fn a_client_that_never_reads_is_dropped_and_the_frame_does_not_wait() {
    let (mut server, path) = bound("slow");
    let slow_path = path.clone();
    let writer = std::thread::spawn(move || {
        let mut stream = UnixStream::connect(&slow_path).unwrap();
        let line = b"{\"cmd\":\"fly\"}\n";
        for _ in 0..40_000 {
            if stream.write_all(line).is_err() {
                break;
            }
        }
    });
    // Each bad line is answered at once; the replies pile up unread until the
    // client is dropped. No poll may block on the client.
    let started = Instant::now();
    while !writer.is_finished() && started.elapsed() < Duration::from_secs(20) {
        let t = Instant::now();
        let _ = server.poll();
        assert!(t.elapsed() < Duration::from_millis(500), "a poll waited on a client");
        std::thread::sleep(Duration::from_millis(1));
    }
    assert!(writer.is_finished(), "the server dropped the client that never reads");
    writer.join().unwrap();
    let _ = std::fs::remove_dir_all(path.parent().unwrap());
}

#[test]
fn connections_past_the_client_limit_are_closed() {
    let (mut server, path) = bound("cap");
    let mut held: Vec<UnixStream> = (0..16).map(|_| UnixStream::connect(&path).unwrap()).collect();
    let _ = poll_for(&mut server, 50);
    let mut extra = UnixStream::connect(&path).unwrap();
    extra.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
    let _ = poll_for(&mut server, 50);
    let mut buf = [0u8; 16];
    assert_eq!(extra.read(&mut buf).unwrap_or(0), 0, "the 17th client is closed");
    held.clear();
    let _ = std::fs::remove_dir_all(path.parent().unwrap());
}

#[test]
fn an_interim_frame_is_skipped_and_its_wait_extends_the_read() {
    let (mut server, path) = bound("interim");
    let client_path = path.clone();
    let client = std::thread::spawn(move || socket::request_timeout(&client_path, &Command::Info, Some(Duration::from_secs(5))));
    let got = poll_for(&mut server, 2000);
    assert_eq!(got.len(), 1, "the call arrived");
    // A held call says it is waiting, then answers.
    server.reply(got[0].client, &Reply::ok_with("held", serde_json::json!({ "interim": "held", "wait_s": 1.0 })));
    std::thread::sleep(Duration::from_millis(50));
    server.reply(got[0].client, &Reply::ok("done"));
    let reply = client.join().unwrap().expect("the final reply, not the interim frame");
    assert_eq!(reply.message, "done");
    let _ = std::fs::remove_dir_all(path.parent().unwrap());
}

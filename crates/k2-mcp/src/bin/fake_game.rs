//! A stand-in for the game, for the supervisor's tests (run through
//! K2_MCP_GAME_BIN). It opens the control socket in K2_CTL_DIR, answers the
//! host verbs it knows with canned replies, writes a few lines of output, and
//! exits when asked to quit. FAKE_CRASH=1 exits with code 3 at once, as a
//! crash would; FAKE_EXIT_OK=1 exits with code 0 at once, as a quit by the
//! player would. The window reports settled at once, after FAKE_SETTLE_MS, or
//! never with FAKE_NEVER_SETTLE=1. FAKE_GOAL=1 holds every move_to unanswered
//! (the game's time stands still) until a stop comes. Save is answered and
//! printed as "fake game: save <slot>"; FAKE_NO_SAVE=1 refuses it.

use std::path::Path;
use std::time::{Duration, Instant};

use k2_world::control::socket::Server;
use k2_world::control::{Command, Observation, Reply};
use serde_json::json;

/// What a PNG file starts with, followed by a marker the tests look for.
const FAKE_PNG: &[u8] = b"\x89PNG\r\n\x1a\nfake-game-frame";

fn set(var: &str) -> bool {
    std::env::var(var).is_ok_and(|v| v == "1")
}

fn main() {
    println!("fake game starting: {:?} mod_dir={:?}", std::env::args().skip(1).collect::<Vec<_>>(), std::env::var("K2_MOD_DIR").ok());
    eprintln!("fake game: a warning on stderr");
    if set("FAKE_CRASH") {
        println!("fake game: about to crash");
        std::process::exit(3);
    }
    if set("FAKE_EXIT_OK") {
        println!("fake game: quit on its own");
        std::process::exit(0);
    }
    let ctl = std::env::var_os("K2_CTL_DIR").map(std::path::PathBuf::from).expect("K2_CTL_DIR");
    std::fs::create_dir_all(&ctl).expect("control directory");
    let mut server = Server::bind(&ctl.join("control.sock")).expect("bind the control socket");
    println!("fake game: listening on {}", server.path().display());
    let shots = std::env::var_os("K2_SHOT_DIR").map(std::path::PathBuf::from).unwrap_or_else(std::env::temp_dir);
    let started = Instant::now();
    let mut held: Option<u64> = None;
    loop {
        for incoming in server.poll() {
            if set("FAKE_GOAL") && matches!(incoming.command, Command::MoveTo(_)) {
                // A goal that never finishes: the answer waits until a stop comes.
                held = Some(incoming.client);
                continue;
            }
            if matches!(incoming.command, Command::Stop) {
                if let Some(client) = held.take() {
                    server.reply(client, &Reply::err("the goal was stopped"));
                }
            }
            let reply = answer(&incoming.command, &shots, settled(started));
            let quit = matches!(incoming.command, Command::Quit);
            server.reply(incoming.client, &reply);
            if quit {
                println!("fake game: quitting");
                std::process::exit(0);
            }
        }
        std::thread::sleep(Duration::from_millis(5));
    }
}

/// Whether the fake window has settled by now.
fn settled(started: Instant) -> bool {
    if set("FAKE_NEVER_SETTLE") {
        return false;
    }
    let after = std::env::var("FAKE_SETTLE_MS").ok().and_then(|v| v.parse::<u64>().ok()).unwrap_or(0);
    started.elapsed() >= Duration::from_millis(after)
}

fn answer(cmd: &Command, shots: &Path, settled: bool) -> Reply {
    match cmd {
        Command::Info => Reply::ok_with(
            "fake game",
            json!({ "phase": "game", "state": "Game", "module": "001ebo", "time": 1.5, "paused": false,
                    "window": { "width": 1280.0, "height": 720.0, "physical_width": 1280, "physical_height": 720,
                                "scale_factor": 1.0, "settled": settled },
                    "log_filter": "info", "trace_file": null }),
        ),
        Command::Pick(_) => Reply::ok_with("nothing is under the pointer", json!({ "object": null, "name": null, "kind": null })),
        Command::Project(a) => Reply::ok_with(format!("object {} is on screen", a.object), json!({ "object": a.object, "x": 10.0, "y": 20.0, "on_screen": true })),
        Command::TraceStart(a) => Reply::ok_with("tracing", json!({ "path": "/tmp/fake-trace.json", "filter": a.filter })),
        Command::Observe(_) => {
            let obs = Observation { module: "001ebo".to_owned(), time: 1.5, ..Default::default() };
            Reply::ok_with("001ebo at 1.5 s", serde_json::to_value(obs).unwrap_or_default())
        }
        Command::Screenshot(args) => {
            let path = args.path.as_deref().map(Into::into).unwrap_or_else(|| shots.join("fake-shot.png"));
            if let Some(parent) = Path::new(&path).parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            match std::fs::write(&path, FAKE_PNG) {
                Ok(()) => Reply::ok_with(format!("screenshot {}", path.display()), json!({ "path": path })),
                Err(e) => Reply::err(format!("cannot write {}: {e}", path.display())),
            }
        }
        Command::SetLogFilter(a) => Reply::ok_with(format!("log filter {}", a.filter), json!({ "filter": a.filter })),
        Command::KeyPress(a) => {
            let held = f64::from(a.hold_ms.unwrap_or(60)) / 1000.0;
            Reply::ok_with(
                format!("key press held {held:.2} s of game time; the leader moved 0.00 m"),
                json!({ "held_s": held, "moved_m": 0.0, "position": [0.0, 0.0, 0.0] }),
            )
        }
        Command::Save(args) => {
            if set("FAKE_NO_SAVE") {
                return Reply::err("the fake game refuses to save");
            }
            println!("fake game: save {}", args.slot);
            Reply::ok_with(format!("saved {}", args.slot), json!({ "slot": args.slot }))
        }
        Command::Quit => Reply::ok("quitting"),
        other => Reply::err(format!("the fake game does not handle {other:?}")),
    }
}

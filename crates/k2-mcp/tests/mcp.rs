//! k2-mcp end to end over stdio, as Claude Code drives it. No window: the game
//! is the fake game (K2_MCP_GAME_BIN, honoured because K2_MCP_TEST=1), a small
//! binary that answers the control socket and exits on quit. Each test runs
//! with its own run root (K2_MCP_ROOT) and holds stdin open until the end.

use std::io::{BufRead, BufReader, Read, Write};
use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, ChildStdout, Command, ExitStatus, Stdio};
use std::sync::mpsc::{self, Receiver};
use std::time::{Duration, Instant};

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use serde_json::{Value, json};

/// The longest a response may take. Every test answer is quick; the slowest
/// waits are the 20 s settle wait and the 20 s quit wait of a game.
const TIMEOUT: Duration = Duration::from_secs(60);
/// The longest a server process may take to exit once it is asked to.
const EXIT_LIMIT: Duration = Duration::from_secs(30);

/// An MCP client over the child's stdio.
struct Mcp {
    child: Child,
    stdin: Option<ChildStdin>,
    lines: Receiver<String>,
    /// Messages read while waiting for another response.
    pending: Vec<Value>,
    next_id: u64,
}

/// Waits for a child to exit. A child still running after `limit` is killed
/// and the test fails, so a hang cannot stall the whole run.
fn wait_for_exit(child: &mut Child, limit: Duration) -> ExitStatus {
    let deadline = Instant::now() + limit;
    loop {
        if let Some(status) = child.try_wait().expect("poll the child") {
            return status;
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            panic!("the child did not exit within {limit:?}");
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

impl Drop for Mcp {
    /// A test that fails part way must not leave a server running.
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

impl Mcp {
    /// Starts k2-mcp with its run root under `root`, in test mode (K2_MCP_TEST=1,
    /// so K2_MCP_GAME_BIN is honoured). `game` is the fake game to run (None:
    /// no game is started by the tests that do not call game_start).
    fn spawn(root: &Path, game: Option<&str>, extra_env: &[(&str, &str)]) -> Self {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_k2-mcp"));
        cmd.env("K2_MCP_ROOT", root).env("K2_MCP_TEST", "1").stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::inherit());
        if let Some(bin) = game {
            cmd.env("K2_MCP_GAME_BIN", bin);
        }
        for (k, v) in extra_env {
            cmd.env(k, v);
        }
        let mut child = cmd.spawn().expect("start k2-mcp");
        let stdin = child.stdin.take();
        let stdout: ChildStdout = child.stdout.take().expect("stdout");
        let (tx, lines) = mpsc::channel();
        std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines().map_while(Result::ok) {
                if tx.send(line).is_err() {
                    break;
                }
            }
        });
        let mut mcp = Self { child, stdin, lines, pending: Vec::new(), next_id: 0 };
        mcp.request(
            "initialize",
            json!({ "protocolVersion": "2025-06-18", "capabilities": {}, "clientInfo": { "name": "k2-mcp-test", "version": "0" } }),
        );
        mcp.send(&json!({ "jsonrpc": "2.0", "method": "notifications/initialized" }));
        mcp
    }

    fn send(&mut self, message: &Value) {
        let stdin = self.stdin.as_mut().expect("stdin open");
        writeln!(stdin, "{message}").expect("write request");
        stdin.flush().expect("flush request");
    }

    /// Sends a request without waiting for its response; returns its id.
    fn send_request(&mut self, method: &str, params: Value) -> u64 {
        self.next_id += 1;
        let id = self.next_id;
        self.send(&json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params }));
        id
    }

    /// The result of the request with this id (other messages are kept).
    fn await_response(&mut self, id: u64, method: &str) -> Value {
        if let Some(i) = self.pending.iter().position(|v| v.get("id") == Some(&json!(id))) {
            let v = self.pending.remove(i);
            return self.check(v, method);
        }
        loop {
            let line = self.lines.recv_timeout(TIMEOUT).unwrap_or_else(|e| panic!("no response to {method}: {e}"));
            let v: Value = serde_json::from_str(&line).unwrap_or_else(|e| panic!("not JSON: {line}: {e}"));
            if v.get("id") == Some(&json!(id)) {
                return self.check(v, method);
            }
            self.pending.push(v);
        }
    }

    fn check(&self, v: Value, method: &str) -> Value {
        assert!(v.get("error").is_none(), "{method} failed: {v}");
        v["result"].clone()
    }

    /// Sends a request and returns its result.
    fn request(&mut self, method: &str, params: Value) -> Value {
        let id = self.send_request(method, params);
        self.await_response(id, method)
    }

    /// Calls a tool; returns the whole result (content and isError).
    fn call(&mut self, tool: &str, arguments: Value) -> Value {
        self.request("tools/call", json!({ "name": tool, "arguments": arguments }))
    }

    /// Two tool calls sent at once, as two clients would; returns both results.
    fn call_both(&mut self, first: (&str, Value), second: (&str, Value)) -> (Value, Value) {
        let a = self.send_request("tools/call", json!({ "name": first.0, "arguments": first.1 }));
        let b = self.send_request("tools/call", json!({ "name": second.0, "arguments": second.1 }));
        let ra = self.await_response(a, "tools/call");
        let rb = self.await_response(b, "tools/call");
        (ra, rb)
    }

    /// The structured reply of a tool call, asserting it has one.
    /// The result of a tool call with its text kept as `content`, and in the shape
    /// the tests read: `structuredContent` with `ok`, `message` (the text) and
    /// `data` (the text as JSON when it is an object or an array, else null). The
    /// server sends text only.
    fn reply(&mut self, tool: &str, arguments: Value) -> Value {
        let result = self.call(tool, arguments);
        let text = Mcp::text(&result);
        let ok = result["isError"] != true;
        let data = serde_json::from_str::<Value>(&text).ok().filter(|v| v.is_object() || v.is_array()).unwrap_or(Value::Null);
        json!({ "content": result["content"].clone(), "isError": !ok, "structuredContent": { "ok": ok, "message": text, "data": data } })
    }

    /// The text of a tool result (its first text block).
    fn text(result: &Value) -> String {
        result["content"].as_array().and_then(|c| c.iter().find_map(|b| b["text"].as_str())).unwrap_or_default().to_owned()
    }

    /// Ends the session by closing stdin and checks the server exits cleanly.
    fn finish(mut self) {
        drop(self.stdin.take());
        let status = wait_for_exit(&mut self.child, EXIT_LIMIT);
        assert!(status.success(), "{status}");
    }
}

/// A new run root for a test, under a folder this user owns (the server refuses
/// a root whose parent belongs to someone else, and the temp folder may not).
fn fresh_root(tag: &str) -> PathBuf {
    let base = std::env::temp_dir().join("k2-mcp-tests");
    match std::fs::DirBuilder::new().mode(0o700).create(&base) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
        Err(e) => panic!("cannot create {}: {e}", base.display()),
    }
    let root = base.join(format!("{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    root
}

fn fake_game() -> &'static str {
    env!("CARGO_BIN_EXE_k2-mcp-fake-game")
}

/// The pid of the game in a game_start reply.
fn pid_of(started: &Value) -> u32 {
    started["structuredContent"]["data"]["pid"].as_u64().expect("the reply has the game's pid") as u32
}

/// True when the process has exited (a zombie counts as exited).
fn process_gone(pid: u32) -> bool {
    match std::fs::read_to_string(format!("/proc/{pid}/stat")) {
        Err(_) => true,
        Ok(stat) => stat.rsplit(')').next().is_some_and(|rest| rest.trim_start().starts_with('Z')),
    }
}

fn wait_until_gone(pid: u32, limit: Duration) -> bool {
    let deadline = Instant::now() + limit;
    while Instant::now() < deadline {
        if process_gone(pid) {
            return true;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    process_gone(pid)
}

#[test]
fn handshake_and_tools_without_a_game() {
    let root = fresh_root("nogame");
    let mut mcp = Mcp::spawn(&root, None, &[]);

    let tools = mcp.request("tools/list", json!({}));
    let list = tools["tools"].as_array().unwrap();
    let names: Vec<&str> = list.iter().map(|t| t["name"].as_str().unwrap()).collect();
    for verb in [
        "game_start", "game_stop", "game_restart", "game_status", "logs_tail", "set_log_filter", "trace_start", "trace_stop", "screenshot",
        "key_press", "type_text", "mouse_move", "mouse_click", "mouse_drag", "scroll", "observe", "move_to",
        "interact", "choose_reply", "wait_until", "save", "load", "pick", "project",
    ] {
        assert!(names.contains(&verb), "tool {verb} missing from {names:?}");
    }
    assert!(!names.contains(&"advance_time") && !names.contains(&"new_game"), "headless verbs are gone: {names:?}");
    let click = list.iter().find(|t| t["name"] == "mouse_click").unwrap();
    assert!(click["inputSchema"]["properties"]["x"].is_object(), "typed schema: {click}");
    assert!(!names.contains(&"debug_teleport"), "debug verbs are hidden without K2_MCP_DEBUG=1: {names:?}");

    let status = mcp.reply("game_status", json!({}));
    assert_eq!(status["structuredContent"]["data"]["running"], false, "{status}");

    let observed = mcp.call("observe", json!({}));
    assert_eq!(observed["isError"], true, "{observed}");
    assert!(Mcp::text(&observed).contains("game_start"), "tells the user how to start: {observed}");

    let logs = mcp.reply("logs_tail", json!({ "n": 5 }));
    assert_eq!(logs["structuredContent"]["data"]["lines"], json!([]), "{logs}");

    let resources = mcp.request("resources/list", json!({}));
    let uris: Vec<&str> = resources["resources"].as_array().unwrap().iter().map(|r| r["uri"].as_str().unwrap()).collect();
    assert!(uris.contains(&"k2://state") && uris.contains(&"k2://journal"), "{uris:?}");

    mcp.finish();
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn refused_arguments_and_starts_never_launch_a_game() {
    let root = fresh_root("refused");
    let mut mcp = Mcp::spawn(&root, Some(fake_game()), &[]);

    let flag = mcp.call("game_start", json!({ "start": "new", "args": ["--sfx-survey"] }));
    assert_eq!(flag["isError"], true, "{flag}");
    assert!(Mcp::text(&flag).contains("not allowed"), "{flag}");

    let valued = mcp.call("game_start", json!({ "args": ["--module"] }));
    assert!(Mcp::text(&valued).contains("needs a value"), "{valued}");

    let env = mcp.call("game_start", json!({ "env": { "K2_SAVE_DIR": "/tmp/elsewhere" } }));
    assert_eq!(env["isError"], true, "{env}");
    assert!(Mcp::text(&env).contains("K2_SAVE_DIR"), "{env}");

    let home = mcp.call("game_start", json!({ "env": { "HOME=/x": "1" } }));
    assert_eq!(home["isError"], true, "{home}");
    assert!(Mcp::text(&home).contains("not valid"), "an entry with '=' is refused: {home}");

    let lower = mcp.call("game_start", json!({ "env": { "home": "/x" } }));
    assert!(Mcp::text(&lower).contains("not allowed"), "names are compared without case: {lower}");

    let start = mcp.call("game_start", json!({ "start": "sfx" }));
    assert!(Mcp::text(&start).contains("unknown start"), "{start}");

    let status = mcp.reply("game_status", json!({}));
    assert_eq!(status["structuredContent"]["data"]["running"], false, "{status}");
    mcp.finish();
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn the_game_binary_override_needs_test_mode() {
    let root = fresh_root("notest");
    let mut mcp = Mcp::spawn(&root, Some(fake_game()), &[("K2_MCP_TEST", "0")]);
    let refused = mcp.call("game_start", json!({ "build": false }));
    assert_eq!(refused["isError"], true, "{refused}");
    assert!(Mcp::text(&refused).contains("no build yet"), "the fake game is not used: {refused}");
    mcp.finish();
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn a_run_root_inside_the_player_folder_is_refused() {
    let base = fresh_root("player");
    let mut child = Command::new(env!("CARGO_BIN_EXE_k2-mcp"))
        .env("K2_MCP_ROOT", base.join("kotor2-rs").join("mcp"))
        .env("XDG_DATA_HOME", &base)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .expect("start k2-mcp");
    drop(child.stdin.take());
    let status = wait_for_exit(&mut child, EXIT_LIMIT);
    assert!(!status.success(), "{status}");
    let mut err = String::new();
    child.stderr.take().expect("stderr").read_to_string(&mut err).expect("read stderr");
    assert!(err.contains("inside the player's data folder"), "{err}");
}

#[test]
fn a_supervised_game_answers_commands_and_stops_cleanly() {
    let root = fresh_root("fake");
    let mut mcp = Mcp::spawn(&root, Some(fake_game()), &[]);

    let started = mcp.reply("game_start", json!({ "start": "chargen", "args": ["--debug", "--autowalk", "3,90"] }));
    assert_eq!(started["structuredContent"]["ok"], true, "{started}");
    let data = &started["structuredContent"]["data"];
    assert_eq!(data["mode"], "windowed");
    assert!(data["pid"].as_u64().unwrap() > 0, "{started}");
    assert_eq!(data["settled"], true, "the fake window settles at once: {started}");
    assert_eq!(data["game"]["state"], "Game", "the game answered Info: {started}");
    assert_eq!(data["game"]["phase"], "game", "{started}");
    assert!(data["game"]["window"].is_object(), "a settled window is reported: {started}");
    let log_file = PathBuf::from(data["log_file"].as_str().unwrap());
    assert!(log_file.is_file(), "the run has a log file: {}", log_file.display());
    assert_eq!(std::fs::metadata(&log_file).unwrap().permissions().mode() & 0o777, 0o600, "the log is owner-only");
    assert_eq!(std::fs::metadata(&root).unwrap().permissions().mode() & 0o777, 0o700, "the run root is owner-only");

    let status = mcp.reply("game_status", json!({}));
    assert_eq!(status["structuredContent"]["data"]["running"], true, "{status}");
    assert_eq!(status["structuredContent"]["data"]["game"]["module"], "001ebo");

    let again = mcp.reply("game_start", json!({}));
    assert_eq!(again["isError"], true, "a second start is refused while one runs: {again}");

    let observed = mcp.reply("observe", json!({}));
    assert!(Mcp::text(&observed).starts_with("001ebo t=1.50"), "observe is compact text: {observed}");
    let pressed = mcp.reply("key_press", json!({ "key": "KeyW", "hold_ms": 60 }));
    assert_eq!(pressed["structuredContent"]["ok"], true, "{pressed}");
    assert!(Mcp::text(&pressed).starts_with("key press held 0.06 s of game time"), "the hold is reported in game time: {pressed}");
    let picked = mcp.reply("pick", json!({ "x": 10.0, "y": 20.0 }));
    assert!(picked["structuredContent"]["data"]["object"].is_null(), "{picked}");
    let projected = mcp.reply("project", json!({ "object": 7 }));
    assert_eq!(projected["structuredContent"]["data"]["on_screen"], true, "{projected}");
    let traced = mcp.reply("trace_start", json!({ "filter": "debug" }));
    assert_eq!(traced["structuredContent"]["data"]["filter"], "debug", "{traced}");
    let filtered = mcp.reply("set_log_filter", json!({ "filter": "debug" }));
    assert_eq!(filtered["structuredContent"]["data"]["filter"], "debug", "{filtered}");

    let shot = mcp.call("screenshot", json!({}));
    assert_eq!(shot["isError"], false, "{shot}");
    let image = shot["content"].as_array().unwrap().iter().find(|b| b["type"] == "image").expect("an image block");
    assert_eq!(image["mimeType"], "image/png");
    let bytes = STANDARD.decode(image["data"].as_str().unwrap()).unwrap();
    assert!(bytes.starts_with(b"\x89PNG"), "the image is the PNG the game wrote");

    let logs = mcp.reply("logs_tail", json!({ "n": 50 }));
    let texts: Vec<String> = logs["structuredContent"]["data"]["lines"].as_array().unwrap().iter().map(|l| l["text"].as_str().unwrap().to_owned()).collect();
    assert!(texts.iter().any(|t| t.starts_with("fake game starting")), "{texts:?}");
    assert!(texts.iter().any(|t| t.contains("\"--chargen\"") && t.contains("\"--autowalk\"")), "the chosen start and flags reach the game: {texts:?}");
    assert!(texts.iter().any(|t| t.contains("mod_dir=Some") && t.contains("/mods\"")), "the game's mod folder is the run's own: {texts:?}");
    assert!(texts.iter().any(|t| t.contains("warning on stderr")), "stderr is captured too: {texts:?}");
    let filtered = mcp.reply("logs_tail", json!({ "filter": "stderr" }));
    assert_eq!(filtered["structuredContent"]["data"]["lines"].as_array().unwrap().len(), 1, "{filtered}");
    assert!(std::fs::read_to_string(&log_file).unwrap().contains("fake game starting"), "the run's file keeps the output");

    let stopped = mcp.reply("game_stop", json!({}));
    assert_eq!(stopped["structuredContent"]["ok"], true, "{stopped}");
    let status = mcp.reply("game_status", json!({}));
    assert_eq!(status["structuredContent"]["data"]["running"], false, "{status}");
    assert_eq!(status["structuredContent"]["data"]["last_exit"]["kind"], "stopped", "{status}");
    let after = mcp.call("observe", json!({}));
    assert_eq!(after["isError"], true, "{after}");
    assert!(Mcp::text(&after).contains("no game is running"), "{after}");

    // Without a game, a restart simply starts one from the newest save.
    let restarted = mcp.reply("game_restart", json!({ "build": false }));
    assert_eq!(restarted["structuredContent"]["ok"], true, "{restarted}");
    let status = mcp.reply("game_status", json!({}));
    assert_eq!(status["structuredContent"]["data"]["running"], true, "{status}");
    let stopped = mcp.reply("game_stop", json!({}));
    assert_eq!(stopped["structuredContent"]["ok"], true, "{stopped}");

    mcp.finish();
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn a_window_that_has_not_settled_has_no_size_in_the_reply() {
    let root = fresh_root("unsettled");
    let mut mcp = Mcp::spawn(&root, Some(fake_game()), &[("FAKE_NEVER_SETTLE", "1")]);
    let started = mcp.reply("game_start", json!({}));
    let data = &started["structuredContent"]["data"];
    assert_eq!(data["settled"], false, "{started}");
    assert!(data["game"]["window"].is_null(), "the size is withheld until settled: {started}");
    assert!(data["game"]["state"].is_string(), "the rest of Info is still there: {started}");
    mcp.reply("game_stop", json!({}));
    mcp.finish();
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn a_game_that_crashes_is_reported_with_its_last_output() {
    let root = fresh_root("crash");
    let mut mcp = Mcp::spawn(&root, Some(fake_game()), &[("FAKE_CRASH", "1")]);

    let started = mcp.call("game_start", json!({ "start": "new" }));
    assert_eq!(started["isError"], true, "{started}");
    let text = Mcp::text(&started);
    assert!(text.contains("exited unexpectedly") && text.contains("exited with code 3"), "{text}");
    assert!(text.contains("about to crash"), "the crash report carries the last output: {text}");

    let observed = mcp.call("observe", json!({}));
    assert_eq!(observed["isError"], true, "{observed}");
    assert!(Mcp::text(&observed).contains("exited unexpectedly"), "every later call reports it: {observed}");

    let status = mcp.reply("game_status", json!({}));
    assert_eq!(status["structuredContent"]["data"]["running"], false);
    assert_eq!(status["structuredContent"]["data"]["last_exit"]["kind"], "crashed", "{status}");
    assert_eq!(status["structuredContent"]["data"]["last_exit"]["code"], 3, "{status}");

    mcp.finish();
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn a_game_that_quits_on_its_own_is_not_a_crash() {
    let root = fresh_root("exitok");
    let mut mcp = Mcp::spawn(&root, Some(fake_game()), &[("FAKE_EXIT_OK", "1")]);

    let started = mcp.call("game_start", json!({ "start": "new" }));
    assert_eq!(started["isError"], true, "{started}");
    let text = Mcp::text(&started);
    assert!(text.contains("exited before it opened the control socket") && text.contains("exited with code 0"), "{text}");

    let observed = mcp.call("observe", json!({}));
    let text = Mcp::text(&observed);
    assert!(text.contains("exited on its own") && !text.contains("unexpectedly"), "{text}");

    let status = mcp.reply("game_status", json!({}));
    assert_eq!(status["structuredContent"]["data"]["last_exit"]["kind"], "exited_on_its_own", "{status}");
    assert!(Mcp::text(&status).contains("exited_on_its_own"), "the status text names the exit kind: {status}");

    mcp.finish();
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn a_log_cursor_belongs_to_its_run() {
    let root = fresh_root("cursor");
    let mut mcp = Mcp::spawn(&root, Some(fake_game()), &[]);
    mcp.reply("game_start", json!({ "start": "new" }));

    let first = mcp.reply("logs_tail", json!({ "n": 50 }));
    let cursor = first["structuredContent"]["data"]["cursor"].clone();
    let same_run = mcp.reply("logs_tail", json!({ "since": cursor.clone() }));
    assert_eq!(same_run["structuredContent"]["data"]["dropped"], false, "{same_run}");

    mcp.reply("game_stop", json!({}));
    mcp.reply("game_start", json!({ "start": "new" }));
    let other_run = mcp.reply("logs_tail", json!({ "since": cursor }));
    assert_eq!(other_run["structuredContent"]["data"]["dropped"], true, "a cursor from an earlier run is marked: {other_run}");
    assert!(Mcp::text(&other_run).contains("earlier run"), "{other_run}");

    mcp.finish();
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn start_and_stop_racing_leave_a_consistent_game() {
    let root = fresh_root("race");
    let mut mcp = Mcp::spawn(&root, Some(fake_game()), &[]);

    let (_, stopped) = mcp.call_both(("game_start", json!({ "start": "new" })), ("game_stop", json!({})));
    assert!(stopped["isError"] != true, "a stop never fails on a race: {stopped}");
    let running = mcp.reply("game_status", json!({}))["structuredContent"]["data"]["running"] == true;
    let observed = mcp.call("observe", json!({}));
    assert_eq!(observed["isError"] == false, running, "observe answers exactly when the game runs: {observed}");
    if running {
        mcp.reply("game_stop", json!({}));
    }

    let (first, second) = mcp.call_both(("game_start", json!({ "start": "new" })), ("game_start", json!({ "start": "new" })));
    let oks = [&first, &second].iter().filter(|r| r["isError"] != true).count();
    assert_eq!(oks, 1, "exactly one of two starts launches: {first} / {second}");
    mcp.reply("game_stop", json!({}));

    let (restart, start) = mcp.call_both(("game_restart", json!({ "build": false })), ("game_start", json!({ "start": "new" })));
    let oks = [&restart, &start].iter().filter(|r| r["isError"] != true).count();
    assert_eq!(oks, 1, "a restart and a start do not both launch: {restart} / {start}");
    let running = mcp.reply("game_status", json!({}))["structuredContent"]["data"]["running"] == true;
    assert!(running, "the one that launched is running");
    mcp.reply("game_stop", json!({}));

    mcp.finish();
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn a_game_outlives_its_server_only_until_the_server_is_killed() {
    let root = fresh_root("orphan");
    let mut mcp = Mcp::spawn(&root, Some(fake_game()), &[]);
    let pid = pid_of(&mcp.reply("game_start", json!({ "start": "new" })));
    assert!(!process_gone(pid), "the game runs");

    mcp.child.kill().expect("kill the server (SIGKILL)");
    wait_for_exit(&mut mcp.child, EXIT_LIMIT);
    assert!(wait_until_gone(pid, Duration::from_secs(10)), "the game ({pid}) exits when its server is killed");
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn a_terminated_server_stops_its_game_and_exits_cleanly() {
    let root = fresh_root("term");
    let mut mcp = Mcp::spawn(&root, Some(fake_game()), &[]);
    let pid = pid_of(&mcp.reply("game_start", json!({ "start": "new" })));

    // SAFETY: kill(2) on the server process this test started.
    unsafe {
        libc::kill(mcp.child.id() as i32, libc::SIGTERM);
    }
    let status = wait_for_exit(&mut mcp.child, EXIT_LIMIT);
    assert!(status.success(), "{status}");
    assert!(wait_until_gone(pid, Duration::from_secs(10)), "the game ({pid}) is stopped with its server");
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn replies_are_text_once_with_no_envelope() {
    let root = fresh_root("text");
    let mut mcp = Mcp::spawn(&root, Some(fake_game()), &[]);
    let no_game = mcp.call("observe", json!({}));
    assert_eq!(Mcp::text(&no_game), "no game is running: call game_start first", "{no_game}");
    assert!(no_game["structuredContent"].is_null(), "errors carry no structured copy: {no_game}");

    mcp.reply("game_start", json!({ "start": "new" }));
    let observed = mcp.call("observe", json!({}));
    assert_eq!(Mcp::text(&observed), "001ebo t=1.50", "the fake has no party or nearby objects, so only the header: {observed}");

    let status = mcp.call("game_status", json!({}));
    let text = Mcp::text(&status);
    assert!(!text.contains("log_filter") && !text.contains("\"socket\""), "game_status is trimmed unless verbose: {text}");
    let verbose = mcp.call("game_status", json!({ "verbose": true }));
    assert!(Mcp::text(&verbose).contains("log_filter"), "verbose game_status has the log filter");

    mcp.reply("game_stop", json!({}));
    mcp.finish();
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn debug_verbs_are_offered_only_with_the_server_flag() {
    let root = fresh_root("debugflag");
    let mut mcp = Mcp::spawn(&root, None, &[("K2_MCP_DEBUG", "1")]);
    let tools = mcp.request("tools/list", json!({}));
    let list = tools["tools"].as_array().unwrap();
    let debug = list.iter().find(|t| t["name"] == "debug_teleport").expect("offered with K2_MCP_DEBUG=1");
    assert!(debug["description"].as_str().unwrap().starts_with("DEBUG, not human-equivalent:"), "{debug}");
    mcp.finish();
    let _ = std::fs::remove_dir_all(&root);
}

/// A scratch layout for the link tests: `base/player/kotor2-rs` (a stand-in
/// player folder, mode 0755), `base/link` (a link to `base/player`).
fn player_layout(tag: &str) -> PathBuf {
    let base = fresh_root(tag);
    let player = base.join("player").join("kotor2-rs");
    std::fs::create_dir_all(&player).unwrap();
    std::fs::set_permissions(&player, std::fs::Permissions::from_mode(0o755)).unwrap();
    std::os::unix::fs::symlink(base.join("player"), base.join("link")).unwrap();
    base
}

#[test]
fn a_run_root_reached_through_a_link_is_refused_and_the_player_folder_keeps_its_mode() {
    let base = player_layout("linkroot");
    let player = base.join("player").join("kotor2-rs");
    let mut child = Command::new(env!("CARGO_BIN_EXE_k2-mcp"))
        .env("K2_MCP_ROOT", base.join("link").join("kotor2-rs").join("mcp"))
        .env("XDG_DATA_HOME", base.join("player"))
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .expect("start k2-mcp");
    drop(child.stdin.take());
    let status = wait_for_exit(&mut child, EXIT_LIMIT);
    assert!(!status.success(), "{status}");
    let mut err = String::new();
    child.stderr.take().expect("stderr").read_to_string(&mut err).expect("read stderr");
    assert!(err.contains("player's data folder"), "{err}");
    assert!(!player.join("mcp").exists(), "the run root is not created inside the player's folder");
    let mode = std::fs::metadata(&player).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode, 0o755, "the player's folder is not chmodded through a link");
    let _ = std::fs::remove_dir_all(&base);
}

#[test]
fn a_second_server_on_one_run_root_is_refused() {
    let root = fresh_root("twoservers");
    let mut first = Mcp::spawn(&root, None, &[]);
    let mut second = Command::new(env!("CARGO_BIN_EXE_k2-mcp"))
        .env("K2_MCP_ROOT", &root)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .expect("start a second k2-mcp");
    drop(second.stdin.take());
    let status = wait_for_exit(&mut second, EXIT_LIMIT);
    assert!(!status.success(), "{status}");
    let mut err = String::new();
    second.stderr.take().expect("stderr").read_to_string(&mut err).expect("read stderr");
    assert!(err.contains("another k2-mcp server"), "{err}");
    let status = first.request("tools/list", json!({}));
    assert!(status["tools"].is_array(), "the first server keeps working: {status}");
    first.finish();
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn a_screenshot_outside_the_shots_folder_is_not_read() {
    let root = fresh_root("outsideshot");
    let outside = std::env::temp_dir().join(format!("k2-mcp-outside-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&outside);
    let mut mcp = Mcp::spawn(&root, Some(fake_game()), &[]);
    mcp.reply("game_start", json!({ "start": "new" }));
    let shot = mcp.call("screenshot", json!({ "path": outside.join("frame.png") }));
    assert_eq!(shot["isError"], true, "{shot}");
    assert!(Mcp::text(&shot).contains("not under"), "{shot}");
    assert!(shot["content"].as_array().unwrap().iter().all(|b| b["type"] != "image"), "no image is returned: {shot}");
    mcp.reply("game_stop", json!({}));
    mcp.finish();
    let _ = std::fs::remove_dir_all(&root);
    let _ = std::fs::remove_dir_all(&outside);
}

/// Polls a file until it holds `needle`. The game's output reaches its log through
/// a pipe, so it can trail the reply by a moment.
fn wait_for_text(path: &Path, needle: &str) -> String {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let text = std::fs::read_to_string(path).unwrap_or_default();
        if text.contains(needle) || Instant::now() >= deadline {
            return text;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

#[test]
fn a_restart_saves_right_before_it_stops_and_relaunches_from_the_save() {
    // Under K2_MCP_GAME_BIN no build runs, so the build step (which restart does
    // first) is not observable here. The order checked is save, stop, relaunch.
    let root = fresh_root("restartorder");
    let mut mcp = Mcp::spawn(&root, Some(fake_game()), &[]);
    let started = mcp.reply("game_start", json!({ "start": "new" }));
    let first_log = PathBuf::from(started["structuredContent"]["data"]["log_file"].as_str().unwrap());
    let restarted = mcp.reply("game_restart", json!({ "build": false }));
    assert_eq!(restarted["structuredContent"]["ok"], true, "{restarted}");
    let second_log = PathBuf::from(restarted["structuredContent"]["data"]["log_file"].as_str().unwrap());

    let first = wait_for_text(&first_log, "fake game: quitting");
    let saved = first.find("fake game: save mcp-restart").expect("the game saved to the restart slot");
    let quit = first.find("fake game: quitting").expect("the game quit");
    assert!(saved < quit, "the save comes before the stop:\n{first}");

    let second = wait_for_text(&second_log, "fake game starting");
    assert!(second.contains("\"--load-slot\", \"mcp-restart\""), "the relaunch loads the restart save:\n{second}");
    mcp.reply("game_stop", json!({}));
    mcp.finish();
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn a_restart_that_cannot_save_leaves_the_game_running() {
    let root = fresh_root("nosave");
    let mut mcp = Mcp::spawn(&root, Some(fake_game()), &[("FAKE_NO_SAVE", "1")]);
    mcp.reply("game_start", json!({ "start": "new" }));
    let refused = mcp.call("game_restart", json!({ "build": false }));
    assert_eq!(refused["isError"], true, "{refused}");
    assert!(Mcp::text(&refused).contains("keeps running"), "{refused}");
    let status = mcp.reply("game_status", json!({}));
    assert_eq!(status["structuredContent"]["data"]["running"], true, "{status}");
    mcp.reply("game_stop", json!({}));
    mcp.finish();
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn a_goal_whose_game_stands_still_is_stopped_with_the_reason() {
    let root = fresh_root("stall");
    let mut mcp = Mcp::spawn(&root, Some(fake_game()), &[("FAKE_GOAL", "1")]);
    mcp.reply("game_start", json!({ "start": "new" }));
    let walk = mcp.call("move_to", json!({ "object": 7 }));
    assert_eq!(walk["isError"], true, "{walk}");
    assert!(Mcp::text(&walk).contains("game not advancing"), "the answer says why: {walk}");
    let status = mcp.reply("game_status", json!({}));
    assert_eq!(status["structuredContent"]["data"]["running"], true, "the game keeps running: {status}");
    mcp.reply("game_stop", json!({}));
    mcp.finish();
    let _ = std::fs::remove_dir_all(&root);
}

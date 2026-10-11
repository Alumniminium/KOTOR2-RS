//! k2-mcp: a Model Context Protocol server (stdio) that runs KotOR 2 in a
//! window and plays it.
//!
//! It is a thin supervisor. It builds and launches the game, keeps the game's
//! output, and forwards commands to the game over its control socket. All game
//! logic lives in the game process, so this server stays up while the game is
//! rebuilt and relaunched. Design, tools and safety: docs/notes/mcp-control.md.
//!
//! Register with Claude Code:
//!   claude mcp add k2 -- /path/to/kotor2-rs/target/release/k2-mcp

mod game_build;
mod logs;
mod paths;
mod policy;
mod render;
mod server;
mod supervisor;

use std::process::ExitCode;

use rmcp::{ServiceExt, transport::stdio};
use tokio::signal::unix::{Signal, SignalKind, signal};

#[tokio::main]
async fn main() -> ExitCode {
    if std::env::args().skip(1).any(|a| a == "--help" || a == "-h") {
        println!("usage: k2-mcp   (an MCP server on stdio; see docs/notes/mcp-control.md)");
        return ExitCode::SUCCESS;
    }
    if std::env::var_os("K2_MCP_GAME_BIN").is_some() && !policy::test_mode() {
        eprintln!("k2-mcp: K2_MCP_GAME_BIN is ignored outside tests (K2_MCP_TEST=1 enables it)");
    }
    let paths = match paths::Paths::from_env() {
        Ok(p) => p,
        Err(e) => {
            eprintln!("k2-mcp: {e}");
            return ExitCode::FAILURE;
        }
    };
    // Installed before the session starts, so a signal from the first request on is not lost.
    let mut term = signal(SignalKind::terminate()).expect("install the SIGTERM handler");
    let mut int = signal(SignalKind::interrupt()).expect("install the SIGINT handler");
    let mut hup = signal(SignalKind::hangup()).expect("install the SIGHUP handler");
    // One server per run root: the lock is held until this process exits.
    let _server_lock = match paths.lock_server() {
        Ok(lock) => lock,
        Err(e) => {
            eprintln!("k2-mcp: {e}");
            return ExitCode::FAILURE;
        }
    };
    let sup = supervisor::Supervisor::new(paths);
    let service = match server::K2::new(sup.clone()).serve(stdio()).await {
        Ok(s) => s,
        Err(e) => {
            eprintln!("k2-mcp: cannot start the MCP session: {e}");
            return ExitCode::FAILURE;
        }
    };
    // The session ends when the client closes stdin, or when this process is
    // signalled. Either way the game it started does not outlive it.
    let result = tokio::select! {
        r = service.waiting() => r.map(|_| ()).map_err(|e| e.to_string()),
        () = signalled(&mut term, &mut int, &mut hup) => Ok(()),
    };
    let ending = sup.clone();
    let _ = tokio::task::spawn_blocking(move || ending.shutdown()).await;
    let code = match result {
        Ok(()) => 0,
        Err(e) => {
            eprintln!("k2-mcp: {e}");
            1
        }
    };
    // Exit here rather than return: after a signal the stdin reader is still
    // blocked in a read, and the runtime's drop would wait for it.
    std::process::exit(code)
}

/// Resolves when the process is asked to end: SIGTERM, SIGINT or SIGHUP.
async fn signalled(term: &mut Signal, int: &mut Signal, hup: &mut Signal) {
    tokio::select! {
        _ = term.recv() => {}
        _ = int.recv() => {}
        _ = hup.recv() => {}
    }
}

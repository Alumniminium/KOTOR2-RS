use super::*;

#[test]
fn starts_map_to_their_flags() {
    assert!(Start::parse("menu").unwrap().args().is_empty());
    assert_eq!(Start::parse("chargen").unwrap().args(), vec!["--chargen"]);
    assert_eq!(Start::parse("load:quick").unwrap().args(), vec!["--load-slot", "quick"]);
    assert_eq!(Start::parse("module:001ebo").unwrap().args(), vec!["--module", "001ebo"]);
    assert!(Start::parse("sfx").is_err());
    assert!(Start::parse("load:../x").is_err(), "slot names are plain");
}

#[test]
fn settled_is_read_from_the_window() {
    assert!(is_settled(&json!({ "window": { "settled": true } })));
    assert!(!is_settled(&json!({ "settled": true })), "settled is a window field");
    assert!(!is_settled(&json!({ "window": { "settled": false } })));
}

#[test]
fn a_launch_after_shutdown_starts_no_game() {
    let root = std::env::temp_dir().join(format!("k2-mcp-closing-{}", std::process::id()));
    let paths = Paths { root: root.clone(), repo: root.clone(), install: root.clone(), player: None };
    let sup = Supervisor::new(paths);
    sup.shutdown();
    let build = Build { stamp: "test".into(), describe: "test".into(), binary: root.join("no-such-game"), warnings: 0, bytes: 0 };
    let err = sup.try_launch(build, Start::New, Extra::default()).unwrap_err();
    assert!(err.contains("shutting down"), "{err}");
    assert!(!root.exists(), "nothing is created once the server is closing");
}

#[test]
fn a_live_socket_is_left_alone_and_a_stale_one_is_cleared() {
    let dir = std::env::temp_dir().join(format!("k2-mcp-socket-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("control.sock");
    let listener = std::os::unix::net::UnixListener::bind(&path).unwrap();
    let err = clear_stale_socket(&path).unwrap_err();
    assert!(err.contains("still answers"), "{err}");
    assert!(path.exists(), "a live socket is not removed");
    drop(listener);
    // The same fork window as the lock test: a child that another test spawns while
    // the listener is open holds a copy of its descriptor until it execs, and
    // connect() keeps succeeding while any copy is open. Poll briefly for the release.
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    let mut cleared = clear_stale_socket(&path);
    while cleared.is_err() && std::time::Instant::now() < deadline {
        std::thread::sleep(std::time::Duration::from_millis(20));
        cleared = clear_stale_socket(&path);
    }
    cleared.expect("a stale socket is cleared");
    assert!(!path.exists());
    std::fs::write(&path, b"not a socket").unwrap();
    assert!(clear_stale_socket(&path).unwrap_err().contains("not a socket"), "a plain file is never removed");
    assert!(path.exists());
    let _ = std::fs::remove_dir_all(&dir);
}

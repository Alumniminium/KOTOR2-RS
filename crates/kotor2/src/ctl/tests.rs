use std::io::Write;
use std::time::{Duration, SystemTime};

use k2_res::Install;

use super::*;

fn temp(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("k2-ctl-test-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

/// A slot with a valid 16-byte header whose save.bin has this age.
fn fake_slot(root: &Path, slot: &str, age_secs: u64) {
    let dir = root.join(slot);
    std::fs::create_dir_all(&dir).unwrap();
    let mut head = save::SAVE_MAGIC.to_vec();
    head.extend_from_slice(&save::SAVE_VERSION.to_le_bytes());
    head.extend_from_slice(&0u32.to_le_bytes());
    std::fs::write(dir.join("save.bin"), head).unwrap();
    let f = std::fs::File::options().write(true).open(dir.join("save.bin")).unwrap();
    f.set_modified(SystemTime::now() - Duration::from_secs(age_secs)).unwrap();
}

#[test]
fn continue_takes_the_newest_slot_by_mtime() {
    let root = temp("slots");
    assert_eq!(continue_slot(&root), None);
    fake_slot(&root, "restart", 500);
    assert_eq!(continue_slot(&root).as_deref(), Some("restart"));
    fake_slot(&root, "autosave", 100);
    fake_slot(&root, "save-0001", 300);
    fake_slot(&root, "other", 1);
    assert_eq!(continue_slot(&root).as_deref(), Some("autosave"), "newer autosave beats an old restart; unknown slots are ignored");
    fake_slot(&root, "restart", 10);
    assert_eq!(continue_slot(&root).as_deref(), Some("restart"));
    fake_slot(&root, "quicksave", 5);
    assert_eq!(continue_slot(&root).as_deref(), Some("quicksave"));
    fake_slot(&root, crate::saves::TIMED_SLOT, 2);
    assert_eq!(continue_slot(&root).as_deref(), Some("timed"), "the rolling timed autosave counts");
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn exit_save_writes_a_restart_slot_that_loads_back() {
    let dir = std::env::var("K2_INSTALL").unwrap_or_else(|_| concat!(env!("CARGO_MANIFEST_DIR"), "/../../..").into());
    let Ok(install) = Install::discover(&dir) else {
        let _ = writeln!(std::io::stderr(), "SKIP exit_save_writes_a_restart_slot_that_loads_back: no install (set K2_INSTALL)");
        return;
    };
    let mut w = k2_world::World::new(install.clone()).unwrap();
    w.enter_module("101per", None).unwrap();
    for _ in 0..60 {
        w.tick(1.0 / 30.0);
    }
    let root = temp("exit");
    // An older restart slot exists with a stale thumbnail.
    std::fs::create_dir_all(root.join(RESTART_SLOT)).unwrap();
    std::fs::write(root.join(RESTART_SLOT).join("thumb.png"), b"old").unwrap();
    let digest = save_on_exit(&w, &root).expect("saved on exit");
    assert!(!root.join(RESTART_SLOT).join("thumb.png").exists());
    assert_eq!(read_digest(&root.join(RESTART_SLOT)), Some(digest));
    let loaded = k2_world::World::load(install, &save::read_slot(&root, RESTART_SLOT).unwrap()).unwrap();
    assert_eq!(loaded.state_digest(), digest);
    assert_eq!(leader_summary(&loaded), leader_summary(&w));
    assert_eq!(continue_slot(&root).as_deref(), Some(RESTART_SLOT));

    // No module, no save.
    let empty = k2_world::World::new(Install::discover(&dir).unwrap()).unwrap();
    assert!(save_on_exit(&empty, &root).is_err());
    let _ = std::fs::remove_dir_all(&root);
}

/// A game app with the window plugin the game runs with, the exit save
/// wiring the game uses, and a world in 101per saving into `root`.
fn exit_test_app(root: &Path) -> Option<App> {
    use bevy::window::{ExitCondition, Window, WindowPlugin};

    let dir = std::env::var("K2_INSTALL").unwrap_or_else(|_| concat!(env!("CARGO_MANIFEST_DIR"), "/../../..").into());
    let Ok(install) = Install::discover(&dir) else {
        let _ = writeln!(std::io::stderr(), "SKIP exit save tests: no install (set K2_INSTALL)");
        return None;
    };
    let mut w = k2_world::World::new(install).unwrap();
    w.enter_module("101per", None).unwrap();
    for _ in 0..30 {
        w.tick(1.0 / 30.0);
    }
    let mut app = App::new();
    app.add_plugins((
        MinimalPlugins,
        bevy::state::app::StatesPlugin,
        WindowPlugin { primary_window: Some(Window::default()), exit_condition: ExitCondition::OnAllClosed, close_when_requested: true, ..default() },
    ))
    .insert_state(AppState::Game)
    .insert_resource(Ctl { dir: None, ..Ctl::new() })
    .insert_resource(Args::default())
    .insert_resource(WorldRes(w))
    .insert_resource(SaveRoot(root.to_owned()));
    add_exit_systems(&mut app);
    app.update();
    Some(app)
}

/// Runs frames until the app asks to exit, returning whether it did.
fn run_until_exit(app: &mut App) -> bool {
    (0..4).any(|_| {
        app.update();
        app.should_exit().is_some()
    })
}

/// Closing the last window: Bevy writes AppExit in Last, after the window
/// is gone. The save must still happen in that same frame.
#[test]
fn closing_the_window_saves_the_restart_slot() {
    use bevy::window::{PrimaryWindow, WindowCloseRequested};

    let root = temp("window-close");
    let Some(mut app) = exit_test_app(&root) else { return };
    let window = app.world_mut().query_filtered::<Entity, With<PrimaryWindow>>().single(app.world()).unwrap();
    app.world_mut().write_message(WindowCloseRequested { window });
    assert!(run_until_exit(&mut app), "closing the last window ends the app");
    assert!(root.join(RESTART_SLOT).join("save.bin").is_file(), "the exit that closed the window saved the restart slot");
    let _ = std::fs::remove_dir_all(&root);
}

/// The other exits (quit from the menu, the control socket, MCP, signals)
/// write AppExit in Update, before Last.
#[test]
fn an_exit_requested_in_update_saves_the_restart_slot() {
    let root = temp("app-exit");
    let Some(mut app) = exit_test_app(&root) else { return };
    app.world_mut().write_message(AppExit::Success);
    assert!(run_until_exit(&mut app));
    assert!(root.join(RESTART_SLOT).join("save.bin").is_file(), "the requested exit saved the restart slot");
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn sigterm_becomes_an_exit_request() {
    install_signal_handlers();
    SIGNALLED.store(false, Ordering::SeqCst);
    // SAFETY: raising a signal whose handler we just installed.
    unsafe { libc::raise(libc::SIGTERM) };
    assert!(SIGNALLED.swap(false, Ordering::SeqCst));
}

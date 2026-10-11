use super::*;

/// A scratch layout: `base/player/kotor2-rs` (a stand-in player folder with
/// mode 0755), `base/link` (a link to `base/player`) and `base/other`.
struct Layout {
    base: PathBuf,
}

impl Layout {
    fn new(tag: &str) -> Self {
        let base = std::env::temp_dir().join(format!("k2-mcp-paths-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(base.join("player/kotor2-rs")).unwrap();
        // Set explicitly: the umask would otherwise leave group write bits, which the parent checks refuse.
        std::fs::set_permissions(&base, Permissions::from_mode(0o700)).unwrap();
        std::fs::set_permissions(base.join("player"), Permissions::from_mode(0o700)).unwrap();
        std::fs::set_permissions(base.join("player/kotor2-rs"), Permissions::from_mode(0o755)).unwrap();
        std::fs::create_dir_all(base.join("other")).unwrap();
        std::os::unix::fs::symlink(base.join("player"), base.join("link")).unwrap();
        Self { base }
    }

    fn player(&self) -> PathBuf {
        self.base.join("player/kotor2-rs")
    }
}

impl Drop for Layout {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.base);
    }
}

#[test]
fn stamps_are_utc_and_sortable() {
    assert_eq!(stamp(0), "19700101-000000-000");
    assert_eq!(stamp(1_000_000_000_123), "20010909-014640-123");
    assert!(stamp(1_000) < stamp(2_000));
}

#[test]
fn the_run_root_is_the_default_unless_overridden() {
    let p = Paths::from_env().expect("the default root is allowed");
    assert!(p.install.ends_with("Knights of the Old Republic II") || p.install.exists());
    assert_eq!(p.socket(), p.root.join("ctl").join("control.sock"));
}

#[test]
fn a_root_inside_the_player_folder_is_refused() {
    let player = Path::new("/home/someone/.local/share/kotor2-rs");
    assert!(check_root(PathBuf::from("/home/someone/.local/share/kotor2-rs/mcp"), Some(player)).is_err());
    assert!(check_root(PathBuf::from("/home/someone/.local/share/kotor2-rs"), Some(player)).is_err());
    assert!(check_root(PathBuf::from("/home/someone/.local/share/kotor2-rs/x/../../kotor2-rs/y"), Some(player)).is_err(), "'..' is resolved first");
    assert!(check_root(PathBuf::from("/home/someone/.local/share/kotor2-rs-other"), Some(player)).is_ok());
    assert!(check_root(PathBuf::from("/tmp/k2research/mcp"), Some(player)).is_ok());
}

#[test]
fn a_link_into_the_player_folder_is_refused() {
    let layout = Layout::new("links");
    let player = layout.player();
    let through_link = layout.base.join("link/kotor2-rs/mcp");
    assert!(check_root(through_link, Some(&player)).is_err(), "a link on the way is refused");
    let link_to_root = layout.base.join("rootlink");
    std::os::unix::fs::symlink(player.join("mcp"), &link_to_root).unwrap();
    assert!(check_root(link_to_root, Some(&player)).is_err(), "a root that is itself a link is refused");
}

#[test]
fn a_root_that_does_not_exist_yet_is_compared_by_where_it_resolves() {
    let layout = Layout::new("missing");
    let player = layout.player();
    let missing = layout.base.join("player/kotor2-rs/new/deeper");
    assert!(check_root(missing, Some(&player)).is_err());
    let elsewhere = layout.base.join("other/mcp");
    assert!(check_root(elsewhere, Some(&player)).is_ok());
}

#[test]
fn a_colon_in_the_root_is_refused() {
    assert!(check_root(PathBuf::from("/tmp/a:b/mcp"), None).is_err());
}

#[test]
fn a_link_is_never_chmodded_through() {
    let layout = Layout::new("chmod");
    let link = layout.base.join("linked");
    std::os::unix::fs::symlink(layout.player(), &link).unwrap();
    assert!(private_dir(&link).is_err(), "a link at the folder itself is refused");
    let mode = std::fs::metadata(layout.player()).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode, 0o755, "the player's folder keeps its mode");
}

/// A run root with its marker (version 1), owner-only whatever the umask.
fn marked_root(path: &Path) {
    std::fs::create_dir_all(path).unwrap();
    std::fs::set_permissions(path, Permissions::from_mode(0o700)).unwrap();
    std::fs::write(path.join(LAYOUT_MARKER), b"1\n").unwrap();
}

#[test]
fn a_root_without_a_marker_is_refused_and_nothing_is_created() {
    let layout = Layout::new("nomarker");
    let root = layout.base.join("nomarker");
    std::fs::create_dir_all(root.join("bin")).unwrap();
    std::fs::set_permissions(&root, Permissions::from_mode(0o700)).unwrap();
    let p = Paths { root: root.clone(), repo: layout.base.clone(), install: layout.base.clone(), player: None };
    let err = p.create_all().unwrap_err();
    assert!(err.to_string().contains("not a run root"), "{err}");
    assert!(!root.join("ctl").exists() && !root.join("logs").exists(), "no run folder is added to it");
}

#[test]
fn a_layout_name_that_is_a_file_is_refused_before_anything_is_created() {
    let layout = Layout::new("filelogs");
    let root = layout.base.join("filelogs");
    marked_root(&root);
    std::fs::write(root.join("logs"), b"not a folder").unwrap();
    let p = Paths { root: root.clone(), repo: layout.base.clone(), install: layout.base.clone(), player: None };
    let err = p.create_all().unwrap_err();
    assert!(err.to_string().contains("logs, which must be a folder"), "{err}");
    assert!(!root.join("bin").exists(), "nothing is created before the refusal");
}

#[test]
fn a_link_in_the_run_tree_leaves_no_folders_behind() {
    let layout = Layout::new("noleftover");
    let root = layout.base.join("tree");
    marked_root(&root);
    std::fs::create_dir_all(root.join("saves")).unwrap();
    std::fs::set_permissions(root.join("saves"), Permissions::from_mode(0o700)).unwrap();
    std::os::unix::fs::symlink(&layout.base.join("other"), root.join("saves").join("meta")).unwrap();
    let p = Paths { root: root.clone(), repo: layout.base.clone(), install: layout.base.clone(), player: None };
    let err = p.create_all().unwrap_err();
    assert!(err.to_string().contains("symbolic link inside the run root"), "{err}");
    for folder in ["bin", "ctl", "logs", "shots", "traces", "settings", "mods"] {
        assert!(!root.join(folder).exists(), "{folder} is not left behind by a refused tree");
    }
}

#[test]
fn a_symlinked_ancestor_of_the_root_is_accepted() {
    let layout = Layout::new("ancestor");
    let root = layout.base.join("link").join("run");
    let p = Paths { root: root.clone(), repo: layout.base.clone(), install: layout.base.clone(), player: Some(layout.player()) };
    p.create_all().expect("a link above the root is not refused");
    assert!(layout.base.join("player").join("run").join("bin").is_dir(), "the root resolves to the folder beside the player's");
}

#[test]
fn a_parent_writable_by_others_is_refused() {
    let layout = Layout::new("sharedparent");
    let shared = layout.base.join("shared");
    std::fs::create_dir_all(&shared).unwrap();
    std::fs::set_permissions(&shared, Permissions::from_mode(0o777)).unwrap();
    let p = Paths { root: shared.join("run"), repo: layout.base.clone(), install: layout.base.clone(), player: None };
    let err = p.create_all().unwrap_err();
    assert!(err.to_string().contains("writable by others"), "{err}");
    assert!(!shared.join("run").exists(), "the root is not created under it");
}

#[test]
fn missing_folders_above_the_root_are_created_owner_only() {
    let layout = Layout::new("missingparents");
    let root = layout.base.join("x").join("y").join("run");
    let p = Paths { root: root.clone(), repo: layout.base.clone(), install: layout.base.clone(), player: None };
    p.create_all().unwrap();
    for folder in [layout.base.join("x"), layout.base.join("x").join("y")] {
        assert_eq!(std::fs::metadata(&folder).unwrap().permissions().mode() & 0o777, 0o700, "{}", folder.display());
    }
}

#[test]
fn a_folder_with_other_files_is_not_taken_over() {
    let layout = Layout::new("notempty");
    let root = layout.base.join("notes");
    std::fs::create_dir_all(&root).unwrap();
    std::fs::set_permissions(&root, Permissions::from_mode(0o755)).unwrap();
    std::fs::write(root.join("notes.txt"), b"mine").unwrap();
    let p = Paths { root: root.clone(), repo: layout.base.clone(), install: layout.base.clone(), player: None };
    let err = p.create_all().unwrap_err();
    assert!(err.to_string().contains("not a run root"), "{err}");
    assert!(!root.join("bin").exists(), "no run folder is created in it");
    assert_eq!(std::fs::metadata(&root).unwrap().permissions().mode() & 0o777, 0o755, "its mode is untouched");
}

#[test]
fn an_existing_empty_root_is_used_without_a_chmod() {
    let layout = Layout::new("emptyroot");
    let root = layout.base.join("empty");
    std::fs::create_dir_all(&root).unwrap();
    std::fs::set_permissions(&root, Permissions::from_mode(0o755)).unwrap();
    let p = Paths { root: root.clone(), repo: layout.base.clone(), install: layout.base.clone(), player: None };
    p.create_all().expect("an empty folder is a run root in the making");
    assert_eq!(std::fs::metadata(&root).unwrap().permissions().mode() & 0o777, 0o755, "the root keeps its mode");
    assert_eq!(std::fs::metadata(p.bins()).unwrap().permissions().mode() & 0o777, 0o700, "the folders made here are owner-only");
}

#[test]
fn a_root_writable_by_others_is_refused() {
    let layout = Layout::new("open");
    let root = layout.base.join("open");
    std::fs::create_dir_all(&root).unwrap();
    std::fs::set_permissions(&root, Permissions::from_mode(0o777)).unwrap();
    let p = Paths { root: root.clone(), repo: layout.base.clone(), install: layout.base.clone(), player: None };
    let err = p.create_all().unwrap_err();
    assert!(err.to_string().contains("writable by others"), "{err}");
}

#[test]
fn a_run_root_that_already_exists_is_reused() {
    let layout = Layout::new("reuse");
    let root = layout.base.join("run");
    let p = Paths { root: root.clone(), repo: layout.base.clone(), install: layout.base.clone(), player: None };
    p.create_all().unwrap();
    p.create_all().expect("a second launch reuses the run root");
    marked_root(&layout.base.join("lockonly"));
    std::fs::write(layout.base.join("lockonly").join(LOCK_FILE), b"").unwrap();
    let lockonly = Paths { root: layout.base.join("lockonly"), repo: layout.base.clone(), install: layout.base.clone(), player: None };
    lockonly.create_all().expect("a marked folder holding server.lock is a run root");
}

#[test]
fn a_link_inside_the_run_tree_is_refused() {
    let layout = Layout::new("treelink");
    let outside = layout.base.join("other");
    let p = Paths { root: layout.base.join("run"), repo: layout.base.clone(), install: layout.base.clone(), player: None };
    p.create_all().unwrap();
    std::os::unix::fs::symlink(&outside, p.saves().join("meta")).unwrap();
    let err = p.create_all().unwrap_err();
    assert!(err.to_string().contains("symbolic link inside the run root"), "{err}");
}

#[test]
fn folders_are_owner_only() {
    let layout = Layout::new("owner");
    let p = Paths { root: layout.base.join("run"), repo: layout.base.clone(), install: layout.base.clone(), player: Some(layout.player()) };
    p.create_all().unwrap();
    for folder in [p.root.clone(), p.bins(), p.ctl(), p.logs(), p.mods()] {
        let mode = std::fs::metadata(&folder).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o700, "{}", folder.display());
    }
    let mode = std::fs::metadata(layout.player()).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode, 0o755, "the player's folder is untouched");
}

#[test]
fn a_writable_folder_in_the_run_tree_is_refused_and_left_alone() {
    let layout = Layout::new("writabletree");
    let root = layout.base.join("tree");
    marked_root(&root);
    std::fs::create_dir_all(root.join("saves")).unwrap();
    std::fs::set_permissions(root.join("saves"), Permissions::from_mode(0o777)).unwrap();
    let p = Paths { root: root.clone(), repo: layout.base.clone(), install: layout.base.clone(), player: None };
    let err = p.create_all().unwrap_err();
    assert!(err.to_string().contains("writable by others"), "{err}");
    assert!(err.to_string().contains("saves"), "the refusal names saves/: {err}");
    assert_eq!(std::fs::metadata(root.join("saves")).unwrap().permissions().mode() & 0o777, 0o777, "saves/ keeps its mode");
    assert!(!root.join("bin").exists(), "nothing is created");
}

#[test]
fn a_grandparent_writable_by_others_is_refused_and_nothing_is_created_under_it() {
    let layout = Layout::new("grandparent");
    let gp = layout.base.join("gp");
    std::fs::create_dir_all(&gp).unwrap();
    std::fs::set_permissions(&gp, Permissions::from_mode(0o777)).unwrap();
    let p = Paths { root: gp.join("parent").join("run"), repo: layout.base.clone(), install: layout.base.clone(), player: None };
    let err = p.create_all().unwrap_err();
    assert!(err.to_string().contains("writable by others"), "{err}");
    assert!(err.to_string().contains(&gp.display().to_string()), "the refusal names the open folder: {err}");
    assert!(!gp.join("parent").exists(), "no folder is created under the open one");
}

#[test]
fn create_all_under_the_player_folder_changes_nothing() {
    let layout = Layout::new("underplayer");
    let root = layout.player().join("mcp");
    let p = Paths { root: root.clone(), repo: layout.base.clone(), install: layout.base.clone(), player: Some(layout.player()) };
    let err = p.create_all().unwrap_err();
    assert!(err.to_string().contains("player's data folder"), "{err}");
    assert!(!root.exists(), "the root is not created");
    assert_eq!(std::fs::metadata(layout.player()).unwrap().permissions().mode() & 0o777, 0o755, "the player's folder keeps its mode");
    assert!(std::fs::read_dir(layout.player()).unwrap().next().is_none(), "the player's folder is unchanged");
}

#[test]
fn a_second_server_on_one_root_is_refused() {
    let layout = Layout::new("lock");
    let p = Paths { root: layout.base.join("run"), repo: layout.base.clone(), install: layout.base.clone(), player: None };
    let first = p.lock_server().expect("the first server takes the lock");
    let err = p.lock_server().unwrap_err();
    assert!(err.contains("another k2-mcp server"), "{err}");
    drop(first);
    // flock is released only when every copy of the open file is closed. A child
    // that another test spawns while the lock is open holds a copy until it execs
    // (the descriptor is close-on-exec), so the release can trail the drop briefly.
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    let mut freed = p.lock_server().is_ok();
    while !freed && std::time::Instant::now() < deadline {
        std::thread::sleep(std::time::Duration::from_millis(20));
        freed = p.lock_server().is_ok();
    }
    assert!(freed, "the lock is free once the first server is gone");
}

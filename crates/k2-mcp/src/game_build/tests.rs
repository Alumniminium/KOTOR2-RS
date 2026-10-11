use super::*;
use std::os::unix::fs::PermissionsExt;
use std::time::Duration;

fn scratch(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("k2-mcp-build-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

#[test]
fn a_green_build_is_snapshotted_with_a_record() {
    let dir = scratch("green");
    let repo = dir.join("tiny");
    std::fs::create_dir_all(repo.join("src")).unwrap();
    std::fs::write(repo.join("Cargo.toml"), "[package]\nname = \"tiny\"\nversion = \"0.1.0\"\nedition = \"2021\"\n").unwrap();
    std::fs::write(repo.join("src/main.rs"), "fn main() { println!(\"tiny\"); }\n").unwrap();
    let bins = dir.join("bin");
    let building = Mutex::new(None);
    let build = build_package(&repo, "tiny", "tiny", &bins, &building).expect("a green build snapshots");
    let mode = std::fs::metadata(build.binary.parent().unwrap()).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode, 0o700, "a snapshot folder is owner-only");
    assert!(build.binary.is_file(), "{}", build.binary.display());
    assert!(build.binary.starts_with(&bins));
    assert!(build.bytes > 0);
    assert_eq!(build.describe, "unversioned", "the scratch repo has no git");
    assert!(build.binary.parent().unwrap().join("build.json").is_file());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_red_build_is_reported_and_snapshots_nothing() {
    let dir = scratch("red");
    let repo = dir.join("broken");
    std::fs::create_dir_all(repo.join("src")).unwrap();
    std::fs::write(repo.join("Cargo.toml"), "[package]\nname = \"broken\"\nversion = \"0.1.0\"\nedition = \"2021\"\n").unwrap();
    std::fs::write(repo.join("src/main.rs"), "fn main() { let x: u32 = \"no\"; }\n").unwrap();
    let bins = dir.join("bin");
    let err = build_package(&repo, "broken", "broken", &bins, &Mutex::new(None)).unwrap_err();
    assert!(err.contains("was not launched"), "{err}");
    assert!(err.contains("mismatched types"), "rustc's own error text is carried: {err}");
    assert!(!bins.exists(), "nothing is snapshotted from a red build");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_build_in_progress_is_stopped_by_kill_build() {
    let dir = scratch("kill");
    let repo = dir.join("slow");
    std::fs::create_dir_all(repo.join("src")).unwrap();
    std::fs::write(repo.join("Cargo.toml"), "[package]\nname = \"slow\"\nversion = \"0.1.0\"\nedition = \"2021\"\nbuild = \"build.rs\"\n").unwrap();
    std::fs::write(repo.join("build.rs"), "fn main() { std::thread::sleep(std::time::Duration::from_secs(60)); }\n").unwrap();
    std::fs::write(repo.join("src/main.rs"), "fn main() {}\n").unwrap();
    let bins = dir.join("bin");
    let building = Mutex::new(None);
    let started = std::time::Instant::now();
    let result = std::thread::scope(|scope| {
        let build = scope.spawn(|| build_package(&repo, "slow", "slow", &bins, &building));
        let wait = std::time::Instant::now();
        while building.lock().unwrap().is_none() && wait.elapsed() < Duration::from_secs(30) {
            std::thread::sleep(Duration::from_millis(50));
        }
        assert!(building.lock().unwrap().is_some(), "the build registers its process group");
        std::thread::sleep(Duration::from_millis(500));
        kill_build(&building);
        build.join().unwrap()
    });
    assert!(result.is_err(), "a killed build is an error");
    assert!(started.elapsed() < Duration::from_secs(50), "the build did not run its 60 s script: {:?}", started.elapsed());
    assert!(!bins.join("slow").exists() || std::fs::read_dir(bins.join("slow")).map_or(true, |mut d| d.next().is_none()), "nothing is snapshotted");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn warnings_do_not_count_the_summary_line() {
    let text = "warning: unused variable: `x`\nwarning: `kotor2` (bin \"kotor2\") generated 1 warning\nwarning: field is never read\n";
    assert_eq!(warnings(text), 2);
}

#[test]
fn the_oldest_snapshots_are_pruned_and_the_one_in_use_is_kept() {
    let dir = scratch("prune");
    let bins = dir.join("bin");
    let names: Vec<String> = (1..=7).map(|i| format!("2026100{i}-120000-000")).collect();
    for name in &names {
        std::fs::create_dir_all(bins.join(name)).unwrap();
        std::fs::write(bins.join(name).join("kotor2"), b"x").unwrap();
    }
    std::fs::create_dir_all(bins.join("not-a-snapshot")).unwrap();
    let in_use = bins.join(&names[0]).join("kotor2");
    // Stands in for trash-put: the folder is renamed out of the bin folder.
    let trashed = dir.join("trash");
    let moved_away = |folder: &Path| -> Result<(), String> {
        std::fs::create_dir_all(&trashed).map_err(|e| e.to_string())?;
        std::fs::rename(folder, trashed.join(folder.file_name().unwrap())).map_err(|e| e.to_string())
    };
    let removed = prune_snapshots(&bins, KEEP_SNAPSHOTS, Some(&in_use), &moved_away).unwrap();
    assert_eq!(removed, 1, "two are oldest, one of them is in use");
    assert!(bins.join(&names[0]).is_dir(), "the running game's snapshot stays");
    assert!(!bins.join(&names[1]).exists(), "the next oldest is moved out");
    assert!(trashed.join(&names[1]).is_dir(), "and handed to the trash");
    for name in &names[2..] {
        assert!(bins.join(name).is_dir(), "{name} is among the newest five");
    }
    assert!(bins.join("not-a-snapshot").is_dir(), "folders that are not snapshots are left alone");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_failed_trash_stops_the_prune_and_reports_it() {
    let dir = scratch("prune-fail");
    let bins = dir.join("bin");
    for i in 1..=7 {
        std::fs::create_dir_all(bins.join(format!("2026100{i}-120000-000"))).unwrap();
    }
    let refuse = |_: &Path| -> Result<(), String> { Err("trash-put refused".to_owned()) };
    let err = prune_snapshots(&bins, KEEP_SNAPSHOTS, None, &refuse).unwrap_err();
    assert_eq!(err, "trash-put refused");
    assert!(bins.join("20261001-120000-000").is_dir(), "nothing is removed when the trash refuses");
    let _ = std::fs::remove_dir_all(&dir);
}

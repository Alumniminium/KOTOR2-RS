//! Building the game and snapshotting the binary. A red build is reported and
//! never launched. The snapshot lives in its own folder, so later cargo builds
//! cannot replace the file a running game came from. Old snapshots are pruned.

use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Mutex;

use serde::Serialize;

use crate::paths::{Paths, private_dir, stamp, unix_millis};

/// Lines of cargo output kept in a failed build's report.
const REPORT_LINES: usize = 120;
/// Snapshot folders kept; older ones are removed (see prune_snapshots).
pub const KEEP_SNAPSHOTS: usize = 5;

/// A snapshot of one build.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Build {
    /// The snapshot folder's name (a UTC time), or "override" for K2_MCP_GAME_BIN.
    pub stamp: String,
    /// `git describe` of the checkout when it has a commit, else "unversioned".
    pub describe: String,
    /// The binary the game runs from.
    pub binary: PathBuf,
    /// Compiler warnings in the build that produced it.
    pub warnings: usize,
    pub bytes: u64,
}

/// `cargo` from PATH, or `mise exec -- cargo` when PATH has none.
fn cargo() -> Command {
    let found = Command::new("cargo").arg("--version").output().is_ok_and(|o| o.status.success());
    if found {
        Command::new("cargo")
    } else {
        let mut c = Command::new("mise");
        c.args(["exec", "--", "cargo"]);
        c
    }
}

/// Builds the game (`cargo build --release -p kotor2`), snapshots it and moves
/// the old snapshots to the trash. `in_use` is the binary a running game runs
/// from; its folder is never moved. Without trash-put nothing is pruned.
/// `building` holds the cargo process group while it runs (see kill_build).
pub fn build_game(paths: &Paths, in_use: Option<&Path>, building: &Mutex<Option<u32>>) -> Result<Build, String> {
    let build = build_package(&paths.repo, "kotor2", "kotor2", &paths.bins(), building)?;
    if trash_put_available() {
        if let Err(e) = prune_snapshots(&paths.bins(), KEEP_SNAPSHOTS, in_use, &trash_put) {
            eprintln!("k2-mcp: cannot prune the old snapshots: {e}");
        }
    } else {
        eprintln!("k2-mcp: trash-put is not installed, so old snapshots under {} are kept", paths.bins().display());
    }
    Ok(build)
}

/// Whether trash-put runs (the snapshots are only ever moved to the trash).
fn trash_put_available() -> bool {
    Command::new("trash-put").arg("--version").output().is_ok_and(|o| o.status.success())
}

/// Moves one folder to the trash with trash-put. Nothing is deleted outright.
fn trash_put(folder: &Path) -> Result<(), String> {
    let status = Command::new("trash-put").arg(folder).status().map_err(|e| format!("cannot run trash-put: {e}"))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("trash-put {} failed ({status})", folder.display()))
    }
}

/// Builds `package` in release mode and copies its `bin` into a new snapshot
/// folder under `bins`. Errors carry the end of cargo's output. Cargo runs in
/// its own process group, recorded in `building` while it runs, so a shutdown
/// can stop the whole build (kill_build).
pub fn build_package(repo: &Path, package: &str, bin: &str, bins: &Path, building: &Mutex<Option<u32>>) -> Result<Build, String> {
    let child = cargo()
        .args(["build", "--release", "-p", package])
        .current_dir(repo)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .process_group(0)
        .spawn()
        .map_err(|e| format!("cannot run cargo in {}: {e}", repo.display()))?;
    *building.lock().unwrap_or_else(|e| e.into_inner()) = Some(child.id());
    let output = child.wait_with_output();
    *building.lock().unwrap_or_else(|e| e.into_inner()) = None;
    let out = output.map_err(|e| format!("cargo in {} did not finish: {e}", repo.display()))?;
    let text = format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
    if !out.status.success() {
        let tail: Vec<&str> = text.lines().collect();
        let start = tail.len().saturating_sub(REPORT_LINES);
        return Err(format!("cargo build -p {package} failed ({}); the game was not launched:\n{}", out.status, tail[start..].join("\n")));
    }
    let target = std::env::var_os("CARGO_TARGET_DIR").filter(|v| !v.is_empty()).map(PathBuf::from).unwrap_or_else(|| repo.join("target"));
    let source = target.join("release").join(bin);
    let folder = bins.join(stamp(unix_millis()));
    std::fs::create_dir_all(bins).map_err(|e| format!("cannot create {}: {e}", bins.display()))?;
    private_dir(&folder).map_err(|e| format!("cannot create {}: {e}", folder.display()))?;
    let binary = folder.join(bin);
    std::fs::copy(&source, &binary).map_err(|e| format!("cannot snapshot {}: {e}", source.display()))?;
    let bytes = std::fs::metadata(&binary).map(|m| m.len()).unwrap_or(0);
    let build = Build {
        stamp: folder.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default(),
        describe: describe(repo),
        binary,
        warnings: warnings(&text),
        bytes,
    };
    let record = serde_json::to_vec_pretty(&build).map_err(|e| e.to_string())?;
    std::fs::write(folder.join("build.json"), record).map_err(|e| format!("cannot write build.json: {e}"))?;
    Ok(build)
}

/// Hands all but the newest `keep` snapshot folders under `bins` to `remove`
/// (the trash, in the server), and never the folder that holds `in_use`. Only
/// folders named like a snapshot stamp count. Returns how many were handed over.
pub fn prune_snapshots(bins: &Path, keep: usize, in_use: Option<&Path>, remove: &dyn Fn(&Path) -> Result<(), String>) -> Result<usize, String> {
    let listing = std::fs::read_dir(bins).map_err(|e| format!("cannot read {}: {e}", bins.display()))?;
    let mut folders: Vec<PathBuf> = listing
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.is_dir() && p.file_name().and_then(|n| n.to_str()).is_some_and(is_stamp))
        .collect();
    folders.sort();
    let extra = folders.len().saturating_sub(keep);
    let mut moved = 0;
    for folder in folders.iter().take(extra) {
        if in_use.is_some_and(|binary| binary.starts_with(folder)) {
            continue;
        }
        remove(folder)?;
        moved += 1;
    }
    Ok(moved)
}

/// A snapshot folder name: a UTC stamp such as 20261009-134502-123.
fn is_stamp(name: &str) -> bool {
    let b = name.as_bytes();
    b.len() == 19 && b.iter().enumerate().all(|(i, c)| if i == 8 || i == 15 { *c == b'-' } else { c.is_ascii_digit() })
}

/// Stops a build in progress: SIGTERM to its whole process group. Nothing
/// happens when no build runs. The pid is cleared when cargo has been reaped,
/// so the group id is only reused after that.
pub fn kill_build(building: &Mutex<Option<u32>>) {
    let pid = *building.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(pid) = pid {
        // SAFETY: kill(2) with a negative pid signals a process group. The
        // group is the cargo build this server started and has not reaped.
        unsafe {
            libc::kill(-(pid as i32), libc::SIGTERM);
        }
    }
}

/// `git describe` of the checkout, or "unversioned" (no commits, or no git).
fn describe(repo: &Path) -> String {
    Command::new("git")
        .args(["-C"])
        .arg(repo)
        .args(["describe", "--always", "--dirty", "--tags"])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_owned())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "unversioned".to_owned())
}

/// Warning lines in cargo's output, not counting its summary line.
fn warnings(text: &str) -> usize {
    text.lines().filter(|l| l.starts_with("warning:") && !l.contains("generated")).count()
}

#[cfg(test)]
mod tests;

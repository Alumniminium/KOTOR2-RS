//! Where things live: this checkout, the game folder, and the run root
//! (/tmp/k2research/mcp by default) that holds every file the supervisor
//! writes: build snapshots, save and settings folders for the game, the
//! control socket, logs, screenshots and traces. The folders the server creates
//! are owner-only; an existing root keeps its mode. Nothing in the run tree may
//! be a link, and none of it may reach the player's folder.

use std::ffi::OsString;
use std::fs::{DirBuilder, File, OpenOptions, Permissions};
use std::io::{ErrorKind, Write};
use std::os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt, PermissionsExt};
use std::os::unix::io::AsRawFd;
use std::path::{Component, Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

/// The run root when K2_MCP_ROOT is not set.
pub const DEFAULT_ROOT: &str = "/tmp/k2research/mcp";
/// The file that holds the one server of a run root (flock).
pub const LOCK_FILE: &str = "server.lock";
/// The folders of a run root: what a folder must hold to count as one.
const LAYOUT: &[&str] = &["bin", "saves", "settings", "ctl", "shots", "traces", "logs", "mods"];
/// The layout marker: a run root holds this file, with LAYOUT_VERSION in it.
const LAYOUT_MARKER: &str = "layout";
const LAYOUT_VERSION: &str = "1";

#[derive(Debug, Clone, PartialEq)]
pub struct Paths {
    /// The run root.
    pub root: PathBuf,
    /// The kotor2-rs checkout (its Cargo workspace).
    pub repo: PathBuf,
    /// The game folder, the install root the game reads.
    pub install: PathBuf,
    /// The player's own data folder, which nothing under the run root may reach.
    pub player: Option<PathBuf>,
}

impl Paths {
    /// The paths of this build. K2_MCP_ROOT overrides the run root, but never
    /// to a place inside the player's own data folder, nor through a link.
    pub fn from_env() -> Result<Self, String> {
        let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
        let repo = manifest.join("../..");
        let install = repo.join("..");
        let requested = std::env::var_os("K2_MCP_ROOT").filter(|v| !v.is_empty()).map(PathBuf::from).unwrap_or_else(|| PathBuf::from(DEFAULT_ROOT));
        let player = player_data_dir();
        let root = check_root(requested, player.as_deref())?;
        Ok(Self {
            root,
            repo: repo.canonicalize().unwrap_or(repo),
            install: install.canonicalize().unwrap_or(install),
            player,
        })
    }

    /// Build snapshots: one folder per build, named by its time.
    pub fn bins(&self) -> PathBuf {
        self.root.join("bin")
    }

    /// Save slots of the game (K2_SAVE_DIR).
    pub fn saves(&self) -> PathBuf {
        self.root.join("saves")
    }

    /// The game's settings (K2_SETTINGS_DIR).
    pub fn settings(&self) -> PathBuf {
        self.root.join("settings")
    }

    /// The control directory (K2_CTL_DIR), holding control.sock.
    pub fn ctl(&self) -> PathBuf {
        self.root.join("ctl")
    }

    /// The control socket inside the control directory.
    pub fn socket(&self) -> PathBuf {
        self.ctl().join("control.sock")
    }

    pub fn shots(&self) -> PathBuf {
        self.root.join("shots")
    }

    pub fn traces(&self) -> PathBuf {
        self.root.join("traces")
    }

    /// One log file per run.
    pub fn logs(&self) -> PathBuf {
        self.root.join("logs")
    }

    /// Loose mod folders for the game (K2_MOD_DIR): empty, so no player mods
    /// are read.
    pub fn mods(&self) -> PathBuf {
        self.root.join("mods")
    }

    /// Creates the run folders. Folders this creates are owner-only; an existing
    /// root is used as it is, after checks (see check_existing_root and
    /// check_tree). Every check runs before anything is created. Afterwards the
    /// root is checked again, in case something raced in.
    pub fn create_all(&self) -> std::io::Result<()> {
        check_root(self.root.clone(), self.player.as_deref()).map_err(|e| std::io::Error::new(ErrorKind::PermissionDenied, e))?;
        check_existing_root(&self.root)?;
        if self.root.exists() {
            check_tree(&self.root)?;
        }
        prepare_parent(&self.root)?;
        private_dir(&self.root)?;
        write_layout_marker(&self.root)?;
        for dir in [self.bins(), self.saves(), self.settings(), self.ctl(), self.shots(), self.traces(), self.logs(), self.mods()] {
            private_dir(&dir)?;
        }
        check_root(self.root.clone(), self.player.as_deref()).map_err(|e| std::io::Error::new(ErrorKind::PermissionDenied, e))?;
        check_tree(&self.root)
    }

    /// Takes the run root's server lock (flock, held until this process exits).
    /// A second server on the same root is refused: it would remove the first
    /// server's control socket and mix its games' output.
    pub fn lock_server(&self) -> Result<File, String> {
        self.create_all().map_err(|e| format!("cannot create the run folders under {}: {e}", self.root.display()))?;
        let file = OpenOptions::new()
            .create(true)
            .write(true)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW)
            .open(self.root.join(LOCK_FILE))
            .map_err(|e| format!("cannot open the server lock under {}: {e}", self.root.display()))?;
        // SAFETY: flock(2) on a descriptor this function owns. It reads its arguments only.
        let locked = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } == 0;
        if !locked {
            return Err(format!("another k2-mcp server is already using {} ({LOCK_FILE} is held). One server per run root.", self.root.display()));
        }
        Ok(file)
    }
}

/// Creates `dir` (not its parents) owner-only and returns true. A folder that
/// exists already is checked (a folder, owned by you, not a link) and left as it
/// is: only folders this creates are chmodded. A link at `dir` itself is
/// refused, and the mode is set on the open directory, never through a path.
pub fn private_dir(dir: &Path) -> std::io::Result<bool> {
    refuse_link(dir).map_err(|e| std::io::Error::new(ErrorKind::PermissionDenied, e))?;
    let created = match DirBuilder::new().mode(0o700).create(dir) {
        Ok(()) => true,
        Err(e) if e.kind() == ErrorKind::AlreadyExists => false,
        Err(e) => return Err(e),
    };
    let open = OpenOptions::new().read(true).custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW).open(dir)?;
    if open.metadata()?.uid() != euid() {
        return Err(std::io::Error::new(ErrorKind::PermissionDenied, format!("{} is not owned by you", dir.display())));
    }
    if created {
        open.set_permissions(Permissions::from_mode(0o700))?;
    }
    Ok(created)
}

/// Checks a run root that exists before anything is written into it: a folder
/// this user owns, not writable by others, and either empty, or a run root. A
/// run root holds the layout marker (`layout`, with version 1) and nothing but
/// server.lock, the marker and the layout folders, each of which must be a
/// folder. Nothing is changed.
fn check_existing_root(root: &Path) -> std::io::Result<()> {
    let meta = match std::fs::symlink_metadata(root) {
        Ok(meta) => meta,
        Err(e) if e.kind() == ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(e),
    };
    let refuse = |why: String| std::io::Error::new(ErrorKind::PermissionDenied, format!("K2_MCP_ROOT {} {why}", root.display()));
    if !meta.is_dir() {
        return Err(refuse("is not a folder".to_owned()));
    }
    if meta.uid() != euid() {
        return Err(refuse("is not owned by you".to_owned()));
    }
    if meta.mode() & 0o022 != 0 {
        return Err(refuse(format!("is writable by others (mode {:o}); choose a folder only you can write", meta.mode() & 0o777)));
    }
    let mut names: Vec<String> = Vec::new();
    for entry in std::fs::read_dir(root)? {
        match entry?.file_name().into_string() {
            Ok(name) => names.push(name),
            Err(_) => return Err(refuse("holds a name that is not text; choose an empty folder".to_owned())),
        }
    }
    if names.is_empty() {
        return Ok(());
    }
    let marker = root.join(LAYOUT_MARKER);
    let marked = std::fs::symlink_metadata(&marker).is_ok_and(|m| m.is_file()) && std::fs::read_to_string(&marker).is_ok_and(|v| v.trim() == LAYOUT_VERSION);
    if !marked {
        return Err(refuse("is not a run root (no layout marker, or another version); choose an empty folder".to_owned()));
    }
    for name in &names {
        if name == LOCK_FILE || name == LAYOUT_MARKER {
            continue;
        }
        if !LAYOUT.contains(&name.as_str()) {
            return Err(refuse(format!("holds {name}, which is not part of a run root")));
        }
        if !std::fs::symlink_metadata(root.join(name))?.is_dir() {
            return Err(refuse(format!("has {name}, which must be a folder")));
        }
    }
    Ok(())
}

/// Writes the layout marker into a run root that has none yet (version 1).
fn write_layout_marker(root: &Path) -> std::io::Result<()> {
    match OpenOptions::new().write(true).create_new(true).mode(0o600).custom_flags(libc::O_NOFOLLOW).open(root.join(LAYOUT_MARKER)) {
        Ok(mut file) => file.write_all(format!("{LAYOUT_VERSION}\n").as_bytes()),
        Err(e) if e.kind() == ErrorKind::AlreadyExists => Ok(()),
        Err(e) => Err(e),
    }
}

/// The folders above the run root. Every existing one up to "/" is checked
/// first: the root's parent must be owned by you, and every folder above it by
/// you or by root. None may be writable by others unless it has the sticky bit.
/// An attacker's folder anywhere on that path could otherwise rename the root's
/// folders and put a link in their place. Only then are missing folders created,
/// owner-only. Links above the root are followed, not refused.
fn prepare_parent(root: &Path) -> std::io::Result<()> {
    let Some(parent) = root.parent() else {
        return Ok(());
    };
    for (depth, folder) in parent.ancestors().enumerate() {
        let Ok(meta) = std::fs::metadata(folder) else {
            continue;
        };
        let refuse = |why: &str| std::io::Error::new(ErrorKind::PermissionDenied, format!("the folder {} above K2_MCP_ROOT {why}", folder.display()));
        if !meta.is_dir() {
            return Err(refuse("is not a folder"));
        }
        let owner_ok = meta.uid() == euid() || (depth > 0 && meta.uid() == 0);
        if !owner_ok {
            return Err(refuse(if depth == 0 { "is not owned by you" } else { "is not owned by you or root" }));
        }
        if meta.mode() & 0o022 != 0 && meta.mode() & 0o1000 == 0 {
            return Err(refuse(format!("is writable by others (mode {:o}) and has no sticky bit", meta.mode() & 0o777).as_str()));
        }
    }
    let mut missing = Vec::new();
    let mut at = parent.to_path_buf();
    while std::fs::metadata(&at).is_err() {
        missing.push(at.clone());
        if !at.pop() {
            break;
        }
    }
    for dir in missing.iter().rev() {
        match DirBuilder::new().mode(0o700).create(dir) {
            Ok(()) => {}
            Err(e) if e.kind() == ErrorKind::AlreadyExists => {}
            Err(e) => return Err(e),
        }
    }
    Ok(())
}

/// Refused: a link anywhere in the run tree, an entry another user owns, or a
/// folder that others can write to. A link in the tree could send a save or a
/// setting outside it, and a writable folder lets someone else swap its contents.
fn check_tree(dir: &Path) -> std::io::Result<()> {
    for entry in std::fs::read_dir(dir)? {
        let path = entry?.path();
        let meta = std::fs::symlink_metadata(&path)?;
        if meta.file_type().is_symlink() {
            return Err(std::io::Error::new(ErrorKind::PermissionDenied, format!("{} is a symbolic link inside the run root; remove it and start again", path.display())));
        }
        if meta.uid() != euid() {
            return Err(std::io::Error::new(ErrorKind::PermissionDenied, format!("{} is not owned by you", path.display())));
        }
        if meta.is_dir() && meta.mode() & 0o022 != 0 {
            return Err(std::io::Error::new(ErrorKind::PermissionDenied, format!("{} is writable by others (mode {:o}); the run folders must not be", path.display(), meta.mode() & 0o777)));
        }
        if meta.is_dir() {
            check_tree(&path)?;
        }
    }
    Ok(())
}

/// The effective user id of this process.
fn euid() -> u32 {
    // SAFETY: geteuid(2) takes no arguments and cannot fail.
    unsafe { libc::geteuid() }
}

/// The player's own data folder, where a normal (unsupervised) run keeps its
/// saves, settings and control socket: $XDG_DATA_HOME/kotor2-rs, or
/// ~/.local/share/kotor2-rs.
pub fn player_data_dir() -> Option<PathBuf> {
    let base = std::env::var_os("XDG_DATA_HOME")
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/share")))?;
    Some(base.join("kotor2-rs"))
}

/// The run root as an absolute path. Refused when it is inside the player's
/// data folder (checked with every link resolved, so a link cannot lead
/// there), when the root itself is a symbolic link, or when it contains ':' (the
/// game splits K2_MOD_DIR on ':').
pub fn check_root(root: PathBuf, player: Option<&Path>) -> Result<PathBuf, String> {
    let lexical = normalise(&absolute(root)?);
    if lexical.to_string_lossy().contains(':') {
        return Err(format!("K2_MCP_ROOT {} contains ':', which the game's K2_MOD_DIR cannot carry", lexical.display()));
    }
    refuse_link(&lexical)?;
    if let Some(player) = player {
        let player = resolve(&normalise(&absolute(player.to_path_buf())?))?;
        if resolve(&lexical)?.starts_with(&player) {
            return Err(format!(
                "K2_MCP_ROOT {} is inside the player's data folder {}; the supervisor's files must stay out of it",
                lexical.display(),
                player.display()
            ));
        }
    }
    Ok(lexical)
}

/// `path` made absolute against the current directory.
fn absolute(path: PathBuf) -> Result<PathBuf, String> {
    if path.is_absolute() {
        return Ok(path);
    }
    std::env::current_dir().map(|d| d.join(&path)).map_err(|e| format!("cannot read the current directory: {e}"))
}

/// Refused when `path` itself is a symbolic link. Links above it are not refused:
/// the player-folder check resolves them instead.
fn refuse_link(path: &Path) -> Result<(), String> {
    if std::fs::symlink_metadata(path).is_ok_and(|m| m.file_type().is_symlink()) {
        return Err(format!("{} is a symbolic link; the run may not use one", path.display()));
    }
    Ok(())
}

/// `path` with its longest existing ancestor resolved (links followed), and
/// the part that does not exist yet appended as written.
fn resolve(path: &Path) -> Result<PathBuf, String> {
    let mut existing = path.to_path_buf();
    let mut missing: Vec<OsString> = Vec::new();
    loop {
        if let Ok(real) = existing.canonicalize() {
            return Ok(missing.iter().rev().fold(real, |acc, part| acc.join(part)));
        }
        let name = existing.file_name().ok_or_else(|| format!("cannot resolve {}", path.display()))?.to_owned();
        missing.push(name);
        if !existing.pop() {
            return Err(format!("cannot resolve {}", path.display()));
        }
    }
}

/// Removes `.` and resolves `..` without touching the file system.
fn normalise(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for part in path.components() {
        match part {
            Component::ParentDir => {
                out.pop();
            }
            Component::CurDir => {}
            other => out.push(other.as_os_str()),
        }
    }
    out
}

/// Seconds and milliseconds since the Unix epoch.
pub fn unix_millis() -> u128 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_millis())
}

/// A UTC timestamp for names: 20261009-134502-123.
pub fn stamp(unix_ms: u128) -> String {
    let secs = (unix_ms / 1000) as i64;
    let ms = (unix_ms % 1000) as u32;
    let days = secs.div_euclid(86_400);
    let in_day = secs.rem_euclid(86_400);
    let (y, m, d) = civil_from_days(days);
    format!("{y:04}{m:02}{d:02}-{:02}{:02}{:02}-{ms:03}", in_day / 3600, (in_day % 3600) / 60, in_day % 60)
}

/// Year, month and day for a count of days since 1970-01-01 (proleptic Gregorian).
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

#[cfg(test)]
mod tests;

//! Resource access for a KotOR2 install.
//!
//! Reads the player's own installed copy in place. Lookup order: mod entries
//! (Install::mod_dirs: loose folders and .mod capsules, first entry wins),
//! override, then loaded module capsules, then texture packs (TPC only), then
//! the chitin KEY/BIF set. The engine's own order is override, module,
//! texture pack, chitin; the mod layer above override is ours, not the
//! game's.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

use rakata_core::{ResRef, ResourceType, ResourceTypeCode};
use rakata_formats::bif::BifIndex;
use rakata_formats::erf::{write_erf_to_vec, Erf, ErfFileType, ErfIndex};
use rakata_formats::rim::RimIndex;

#[derive(Debug, thiserror::Error)]
pub enum ResError {
    #[error("no chitin.key under {0}")]
    NotAnInstall(PathBuf),
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("{path}: {msg}")]
    Format { path: PathBuf, msg: String },
}

pub type Result<T> = std::result::Result<T, ResError>;

fn fmt_err(path: &Path, e: impl std::fmt::Display) -> ResError {
    ResError::Format { path: path.to_owned(), msg: e.to_string() }
}

/// Lowercased resref plus raw type id. Resrefs are case-insensitive.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ResKey {
    pub name: String,
    pub ty: u16,
}

impl ResKey {
    pub fn new(name: &str, ty: ResourceType) -> Self {
        Self { name: name.to_ascii_lowercase(), ty: ty.type_id() }
    }

    fn from_parts(resref: &ResRef, ty: ResourceTypeCode) -> Self {
        Self { name: resref.to_string().to_ascii_lowercase(), ty: ty.raw_id() }
    }

    pub fn extension(&self) -> &'static str {
        ResourceTypeCode::from_raw_id(self.ty).known_type().map_or("bin", |t| t.extension())
    }
}

impl std::fmt::Display for ResKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}.{}", self.name, self.extension())
    }
}

/// An ERF-family (ERF/MOD/SAV) or RIM archive, indexed lazily.
pub enum Capsule {
    Erf(ErfIndex),
    Rim(RimIndex),
}

impl Capsule {
    pub fn open(path: &Path) -> Result<Self> {
        let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("").to_ascii_lowercase();
        if ext == "rim" {
            RimIndex::open(path).map(Capsule::Rim).map_err(|e| fmt_err(path, e))
        } else {
            ErfIndex::open(path).map(Capsule::Erf).map_err(|e| fmt_err(path, e))
        }
    }

    pub fn keys(&self) -> Vec<ResKey> {
        match self {
            Capsule::Erf(i) => i.entries().iter().map(|e| ResKey::from_parts(&e.resref, e.resource_type)).collect(),
            Capsule::Rim(i) => i.entries().iter().map(|e| ResKey::from_parts(&e.resref, e.resource_type)).collect(),
        }
    }

    fn read(&self, pos: usize) -> std::result::Result<Vec<u8>, String> {
        match self {
            Capsule::Erf(i) => i.read_entry(pos).map_err(|e| e.to_string()),
            Capsule::Rim(i) => i.read_entry(pos).map_err(|e| e.to_string()),
        }
    }
}

/// Where a resolved resource lives.
#[derive(Debug, Clone)]
pub enum Origin {
    Override(PathBuf),
    /// Inside a .mod capsule listed in Install::mod_dirs.
    Mod { capsule: usize, pos: usize },
    Module { capsule: usize, pos: usize },
    TexturePack { capsule: usize, pos: usize },
    Chitin { bif: usize, pos: usize },
}

struct Mounted {
    path: PathBuf,
    capsule: Capsule,
    index: HashMap<ResKey, usize>,
}

impl Mounted {
    fn open(path: PathBuf) -> Result<Self> {
        let capsule = Capsule::open(&path)?;
        // First entry wins inside a capsule.
        let mut index = HashMap::new();
        for (pos, key) in capsule.keys().into_iter().enumerate() {
            index.entry(key).or_insert(pos);
        }
        Ok(Self { path, capsule, index })
    }
}

/// The game's data directory (the one holding chitin.key).
#[derive(Debug, Clone)]
pub struct Install {
    pub root: PathBuf,
    /// Mod entries searched ahead of the game's override/, first entry wins.
    /// Each is a folder of loose files (override layout) or a .mod capsule.
    /// Never inside the game folder by convention; the editor writes
    /// ~/.local/share/kotor2-rs/mods/<name>.mod.
    pub mod_dirs: Vec<PathBuf>,
}

impl Install {
    /// Accepts the Steam game folder, its `steamassets` dir, or any dir that
    /// directly holds `chitin.key`.
    pub fn discover(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref();
        for cand in [path.to_owned(), path.join("steamassets")] {
            if find_ci(&cand, "chitin.key").is_some() {
                // K2_MOD_DIR (colon-separated) lets every tool see mods.
                let mod_dirs = std::env::var_os("K2_MOD_DIR")
                    .map(|v| std::env::split_paths(&v).filter(|p| !p.as_os_str().is_empty()).collect())
                    .unwrap_or_default();
                return Ok(Self { root: cand, mod_dirs });
            }
        }
        Err(ResError::NotAnInstall(path.to_owned()))
    }

    pub fn dir(&self, name: &str) -> Option<PathBuf> {
        find_ci(&self.root, name)
    }

    /// Files in a subdir, sorted, matching one of the given lowercase extensions.
    pub fn files(&self, dir: &str, exts: &[&str]) -> Vec<PathBuf> {
        let Some(d) = self.dir(dir) else { return Vec::new() };
        let mut out: Vec<PathBuf> = fs::read_dir(d)
            .into_iter()
            .flatten()
            .flatten()
            .map(|e| e.path())
            .filter(|p| {
                p.extension()
                    .and_then(|e| e.to_str())
                    .is_some_and(|e| exts.contains(&e.to_ascii_lowercase().as_str()))
            })
            .collect();
        out.sort();
        out
    }

    /// Module names found under modules/, deduplicated across .rim/_s.rim/_dlg.erf/.mod.
    pub fn module_names(&self) -> Vec<String> {
        let mut names: Vec<String> = self
            .files("modules", &["rim", "erf", "mod"])
            .iter()
            .filter_map(|p| p.file_stem()?.to_str().map(str::to_ascii_lowercase))
            .map(|s| s.trim_end_matches("_s").trim_end_matches("_dlg").to_owned())
            .collect();
        names.sort();
        names.dedup();
        names
    }

    /// Capsules making up a module. A .mod replaces the rim set when present.
    pub fn module_capsules(&self, name: &str) -> Vec<PathBuf> {
        let Some(dir) = self.dir("modules") else { return Vec::new() };
        let name = name.to_ascii_lowercase();
        if let Some(m) = find_ci(&dir, &format!("{name}.mod")) {
            return vec![m];
        }
        [format!("{name}.rim"), format!("{name}_s.rim"), format!("{name}_dlg.erf")]
            .iter()
            .filter_map(|f| find_ci(&dir, f))
            .collect()
    }

    /// Reads one resource from a module's capsules without mounting the
    /// module, for names listed before the module is opened.
    pub fn read_module_resource(&self, module: &str, key: &ResKey) -> Result<Option<Vec<u8>>> {
        for path in self.module_capsules(module) {
            let capsule = Capsule::open(&path)?;
            if let Some(pos) = capsule.keys().iter().position(|k| k == key) {
                return capsule.read(pos).map(Some).map_err(|e| fmt_err(&path, e));
            }
        }
        Ok(None)
    }
}

/// Case-insensitive child lookup. Aspyr's data is all lowercase, mods are not.
fn find_ci(dir: &Path, name: &str) -> Option<PathBuf> {
    let exact = dir.join(name);
    if exact.exists() {
        return Some(exact);
    }
    fs::read_dir(dir)
        .ok()?
        .flatten()
        .find(|e| e.file_name().to_str().is_some_and(|n| n.eq_ignore_ascii_case(name)))
        .map(|e| e.path())
}

/// Resolves resources across the whole install.
pub struct Resources {
    pub install: Install,
    bifs: Vec<(PathBuf, BifIndex)>,
    chitin: HashMap<ResKey, (usize, usize)>,
    overrides: HashMap<ResKey, Origin>,
    mod_capsules: Vec<Mounted>,
    texpacks: Vec<Mounted>,
    modules: Vec<Mounted>,
}

impl Resources {
    pub fn open(install: Install) -> Result<Self> {
        let key_path = find_ci(&install.root, "chitin.key").ok_or_else(|| ResError::NotAnInstall(install.root.clone()))?;
        let key = rakata_formats::read_key_from_bytes(&fs::read(&key_path)?).map_err(|e| fmt_err(&key_path, e))?;

        let mut bifs = Vec::with_capacity(key.bif_entries.len());
        for entry in &key.bif_entries {
            // KEY stores paths like "data\\models.bif".
            let rel = entry.filename.replace('\\', "/");
            let mut path = install.root.clone();
            for part in rel.split('/') {
                path = find_ci(&path, part).unwrap_or_else(|| path.join(part));
            }
            let idx = BifIndex::open(&path).map_err(|e| fmt_err(&path, e))?;
            bifs.push((path, idx));
        }

        // KEY ids carry the bif index; BIF entries may not, so match on the
        // resource index alone.
        let positions: Vec<HashMap<u32, usize>> = bifs
            .iter()
            .map(|(_, idx)| idx.entries().iter().enumerate().map(|(pos, e)| (e.resource_id.resource_index(), pos)).collect())
            .collect();
        let mut chitin = HashMap::new();
        for r in &key.resources {
            let bif = r.resource_id.bif_index() as usize;
            let Some(&pos) = positions.get(bif).and_then(|m| m.get(&r.resource_id.resource_index())) else { continue };
            chitin.entry(ResKey::from_parts(&r.resref, r.resource_type)).or_insert((bif, pos));
        }

        let (overrides, mod_capsules) = mount_overrides(&install);

        let mut texpacks = Vec::new();
        for p in install.files("texturepacks", &["erf"]) {
            texpacks.push(Mounted::open(p)?);
        }

        Ok(Self { install, bifs, chitin, overrides, mod_capsules, texpacks, modules: Vec::new() })
    }

    /// Mounts a module's capsules, replacing any previously loaded module.
    pub fn load_module(&mut self, name: &str) -> Result<()> {
        self.modules.clear();
        for p in self.install.module_capsules(name) {
            self.modules.push(Mounted::open(p)?);
        }
        Ok(())
    }

    pub fn locate(&self, key: &ResKey) -> Option<Origin> {
        if let Some(o) = self.overrides.get(key) {
            return Some(o.clone());
        }
        for (capsule, m) in self.modules.iter().enumerate() {
            if let Some(&pos) = m.index.get(key) {
                return Some(Origin::Module { capsule, pos });
            }
        }
        if key.ty == ResourceType::Tpc.type_id() {
            for (capsule, m) in self.texpacks.iter().enumerate() {
                if let Some(&pos) = m.index.get(key) {
                    return Some(Origin::TexturePack { capsule, pos });
                }
            }
        }
        self.chitin.get(key).map(|&(bif, pos)| Origin::Chitin { bif, pos })
    }

    pub fn read(&self, origin: &Origin) -> Result<Vec<u8>> {
        match origin {
            Origin::Override(p) => Ok(fs::read(p)?),
            Origin::Mod { capsule, pos } => {
                let m = &self.mod_capsules[*capsule];
                m.capsule.read(*pos).map_err(|e| fmt_err(&m.path, e))
            }
            Origin::Module { capsule, pos } => {
                let m = &self.modules[*capsule];
                m.capsule.read(*pos).map_err(|e| fmt_err(&m.path, e))
            }
            Origin::TexturePack { capsule, pos } => {
                let m = &self.texpacks[*capsule];
                m.capsule.read(*pos).map_err(|e| fmt_err(&m.path, e))
            }
            Origin::Chitin { bif, pos } => {
                let (path, idx) = &self.bifs[*bif];
                idx.read_entry(*pos).map_err(|e| fmt_err(path, e))
            }
        }
    }

    pub fn get(&self, key: &ResKey) -> Result<Option<Vec<u8>>> {
        self.locate(key).map(|o| self.read(&o)).transpose()
    }

    pub fn chitin_keys(&self) -> impl Iterator<Item = &ResKey> {
        self.chitin.keys()
    }

    pub fn override_keys(&self) -> impl Iterator<Item = &ResKey> {
        self.overrides.keys()
    }

    pub fn module_keys(&self) -> impl Iterator<Item = &ResKey> {
        self.modules.iter().flat_map(|m| m.index.keys())
    }

    pub fn texpack_keys(&self) -> impl Iterator<Item = &ResKey> {
        self.texpacks.iter().flat_map(|m| m.index.keys())
    }

    pub fn bif_count(&self) -> usize {
        self.bifs.len()
    }
}

/// Packs resources into a MOD capsule at `path` ("MOD V1.0", the ERF family
/// layout the engine reads from modules/). Written to a sibling `.tmp` file
/// and renamed into place, so a failed write leaves an old capsule intact.
pub fn write_mod(path: &Path, resources: Vec<(ResKey, Vec<u8>)>) -> Result<()> {
    let mut erf = Erf::new(ErfFileType::Mod);
    for (key, data) in resources {
        let resref = ResRef::new(&key.name).map_err(|e| fmt_err(path, format!("{}: {e}", key.name)))?;
        erf.push_resource(resref, ResourceTypeCode::from_raw_id(key.ty), data);
    }
    let bytes = write_erf_to_vec(&erf).map_err(|e| fmt_err(path, e))?;
    let mut tmp = path.as_os_str().to_owned();
    tmp.push(".tmp");
    let tmp = PathBuf::from(tmp);
    fs::write(&tmp, bytes)?;
    fs::rename(&tmp, path)?;
    Ok(())
}

/// Override table for the mod entries and override/, in lookup order (mod
/// entries first, first wins). Returns the table plus the .mod capsules it
/// points into. A capsule that fails to open is skipped with a warning.
fn mount_overrides(install: &Install) -> (HashMap<ResKey, Origin>, Vec<Mounted>) {
    let mut map = HashMap::new();
    let mut capsules = Vec::new();
    for dir in &install.mod_dirs {
        if dir.is_file() {
            let m = match Mounted::open(dir.clone()) {
                Ok(m) => m,
                Err(e) => {
                    eprintln!("warning: skipping mod capsule: {e}");
                    continue;
                }
            };
            let capsule = capsules.len();
            for (key, &pos) in &m.index {
                map.entry(key.clone()).or_insert(Origin::Mod { capsule, pos });
            }
            capsules.push(m);
            continue;
        }
        for p in loose_files(dir) {
            if let Some(key) = loose_key(&p) {
                map.entry(key).or_insert_with(|| Origin::Override(p.clone()));
            }
        }
    }
    for p in override_files(install) {
        if let Some(key) = loose_key(&p) {
            map.entry(key).or_insert_with(|| Origin::Override(p.clone()));
        }
    }
    (map, capsules)
}

/// Key of a loose file named `<resref>.<ext>`.
fn loose_key(p: &Path) -> Option<ResKey> {
    let (stem, ext) = (p.file_stem()?.to_str()?, p.extension()?.to_str()?);
    Some(ResKey::new(stem, ResourceType::from_extension(&ext.to_ascii_lowercase())))
}

/// Every regular file directly under override/ (any extension).
fn override_files(install: &Install) -> Vec<PathBuf> {
    let Some(d) = install.dir("override") else { return Vec::new() };
    loose_files(&d)
}

/// Regular files directly under `dir`, sorted (any extension).
fn loose_files(dir: &Path) -> Vec<PathBuf> {
    let mut v: Vec<PathBuf> = fs::read_dir(dir).into_iter().flatten().flatten().map(|e| e.path()).filter(|p| p.is_file()).collect();
    v.sort();
    v
}

#[cfg(test)]
mod tests;

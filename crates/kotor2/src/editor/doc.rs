//! The editor's document: one area's GIT as GFF structs, plus blueprints the
//! editor forked. No Bevy here.
//!
//! The GIT structs are the source of truth, so fields the editor does not know
//! survive a load/save round trip untouched. Each list entry is an `Inst`;
//! saving rebuilds the root's lists from them in order.
//!
//! Field edits that live in a blueprint (creature conversation, scripts,
//! placeable flags, ...) fork the blueprint into a new resref first and point
//! only this instance at it, so other instances sharing the template keep
//! the original.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use k2_data::module::{bearing_from_dir, dir_from_bearing};
use k2_data::{GffExt, Talk};
use k2_res::{Install, ResKey, Resources};
use rakata_core::{ResRef, ResourceType};
use rakata_formats::gff::{Gff, GffField, GffLabel, GffLocalizedString, GffStruct, GffValue};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Kind {
    Creature,
    Door,
    Placeable,
    Waypoint,
    Trigger,
    Sound,
    Store,
    Encounter,
    Camera,
}

impl Kind {
    pub const ALL: [Kind; 9] =
        [Kind::Creature, Kind::Door, Kind::Placeable, Kind::Waypoint, Kind::Trigger, Kind::Sound, Kind::Store, Kind::Encounter, Kind::Camera];

    /// The GIT list holding this kind.
    pub fn list(self) -> &'static str {
        match self {
            Kind::Creature => "Creature List",
            Kind::Door => "Door List",
            Kind::Placeable => "Placeable List",
            Kind::Waypoint => "WaypointList",
            Kind::Trigger => "TriggerList",
            Kind::Sound => "SoundList",
            Kind::Store => "StoreList",
            Kind::Encounter => "Encounter List",
            Kind::Camera => "CameraList",
        }
    }

    /// Blueprint type, if the kind has one.
    pub fn blueprint(self) -> Option<ResourceType> {
        Some(match self {
            Kind::Creature => ResourceType::Utc,
            Kind::Door => ResourceType::Utd,
            Kind::Placeable => ResourceType::Utp,
            Kind::Waypoint => ResourceType::Utw,
            Kind::Trigger => ResourceType::Utt,
            Kind::Sound => ResourceType::Uts,
            Kind::Store => ResourceType::Utm,
            Kind::Encounter => ResourceType::Ute,
            Kind::Camera => return None,
        })
    }

    pub fn from_blueprint(ty: u16) -> Option<Kind> {
        Kind::ALL.into_iter().find(|k| k.blueprint().is_some_and(|t| t.type_id() == ty))
    }

    pub fn name(self) -> &'static str {
        match self {
            Kind::Creature => "creature",
            Kind::Door => "door",
            Kind::Placeable => "placeable",
            Kind::Waypoint => "waypoint",
            Kind::Trigger => "trigger",
            Kind::Sound => "sound",
            Kind::Store => "store",
            Kind::Encounter => "encounter",
            Kind::Camera => "camera",
        }
    }

    /// Doors and placeables use X/Y/Z + Bearing; cameras a Position vector;
    /// the rest XPosition.. + XOrientation/YOrientation.
    fn plain_xyz(self) -> bool {
        matches!(self, Kind::Door | Kind::Placeable)
    }

    /// Geometry point labels for polygon kinds.
    pub fn polygon_labels(self) -> Option<(&'static str, &'static str, &'static str)> {
        match self {
            Kind::Trigger => Some(("PointX", "PointY", "PointZ")),
            Kind::Encounter => Some(("X", "Y", "Z")),
            _ => None,
        }
    }

    /// Kinds whose blueprint resref sits in `ResRef` instead of `TemplateResRef`.
    fn template_label(self) -> &'static str {
        if self == Kind::Store { "ResRef" } else { "TemplateResRef" }
    }

    /// Default GIT struct id for a new entry of this kind (used only when the
    /// GIT has no entry of the kind to clone).
    fn struct_id(self) -> i32 {
        match self {
            Kind::Creature => 4,
            Kind::Door => 8,
            Kind::Placeable => 9,
            Kind::Waypoint => 5,
            Kind::Trigger => 1,
            Kind::Sound => 6,
            Kind::Store => 11,
            Kind::Encounter => 7,
            Kind::Camera => 14,
        }
    }
}

/// Where an inspector field lives.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Src {
    Git,
    Blueprint,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FieldTy {
    Str,
    ResRef,
    Byte,
    Word,
}

#[derive(Clone, Copy, Debug)]
pub struct FieldSpec {
    pub label: &'static str,
    pub src: Src,
    pub ty: FieldTy,
}

const fn f(label: &'static str, src: Src, ty: FieldTy) -> FieldSpec {
    FieldSpec { label, src, ty }
}

/// Editable fields per kind (inspector order).
pub fn fields(kind: Kind) -> &'static [FieldSpec] {
    use FieldTy::*;
    use Src::*;
    match kind {
        Kind::Creature => {
            const F: &[FieldSpec] = &[
            f("Tag", Blueprint, Str),
            f("Conversation", Blueprint, ResRef),
            f("FactionID", Blueprint, Word),
            f("ScriptSpawn", Blueprint, ResRef),
            f("ScriptHeartbeat", Blueprint, ResRef),
            f("ScriptOnNotice", Blueprint, ResRef),
            f("ScriptDialogue", Blueprint, ResRef),
            f("ScriptDeath", Blueprint, ResRef),
            f("ScriptUserDefine", Blueprint, ResRef),
            f("Plot", Blueprint, Byte),
        ];
            F
        }
        Kind::Door => {
            const F: &[FieldSpec] = &[
            f("Tag", Git, Str),
            f("LinkedTo", Git, Str),
            f("LinkedToModule", Git, ResRef),
            f("Conversation", Blueprint, ResRef),
            f("OnOpen", Blueprint, ResRef),
            f("OnClick", Blueprint, ResRef),
            f("OnFailToOpen", Blueprint, ResRef),
            f("OnHeartbeat", Blueprint, ResRef),
            f("Locked", Blueprint, Byte),
            f("Plot", Blueprint, Byte),
        ];
            F
        }
        Kind::Placeable => {
            const F: &[FieldSpec] = &[
            f("Tag", Blueprint, Str),
            f("Conversation", Blueprint, ResRef),
            f("OnUsed", Blueprint, ResRef),
            f("OnHeartbeat", Blueprint, ResRef),
            f("OnOpen", Blueprint, ResRef),
            f("OnDeath", Blueprint, ResRef),
            f("Useable", Blueprint, Byte),
            f("Locked", Blueprint, Byte),
            f("Plot", Blueprint, Byte),
        ];
            F
        }
        Kind::Waypoint => {
            const F: &[FieldSpec] = &[f("Tag", Git, Str)];
            F
        }
        Kind::Trigger => {
            const F: &[FieldSpec] = &[
            f("Tag", Git, Str),
            f("LinkedTo", Git, Str),
            f("LinkedToModule", Git, ResRef),
            f("ScriptOnEnter", Blueprint, ResRef),
            f("ScriptOnExit", Blueprint, ResRef),
            f("ScriptHeartbeat", Blueprint, ResRef),
        ];
            F
        }
        Kind::Sound => {
            const F: &[FieldSpec] = &[f("Tag", Blueprint, Str), f("Active", Blueprint, Byte)];
            F
        }
        Kind::Store => {
            const F: &[FieldSpec] = &[f("Tag", Git, Str)];
            F
        }
        Kind::Encounter => {
            const F: &[FieldSpec] = &[f("Tag", Blueprint, Str), f("Active", Blueprint, Byte)];
            F
        }
        Kind::Camera => {
            const F: &[FieldSpec] = &[];
            F
        }
    }
}

/// One GIT list entry.
#[derive(Clone, Debug)]
pub struct Inst {
    pub uid: u64,
    pub kind: Kind,
    pub st: GffStruct,
}

impl Inst {
    pub fn position(&self) -> [f32; 3] {
        let s = &self.st;
        match self.kind {
            Kind::Camera => match s.field("Position") {
                Some(GffValue::Vector3(v)) => *v,
                _ => [0.0; 3],
            },
            k if k.plain_xyz() => [num(s, "X"), num(s, "Y"), num(s, "Z")],
            _ => [num(s, "XPosition"), num(s, "YPosition"), num(s, "ZPosition")],
        }
    }

    pub fn set_position(&mut self, p: [f32; 3]) {
        let k = self.kind;
        let s = &mut self.st;
        match k {
            Kind::Camera => set(s, "Position", GffValue::Vector3(p)),
            k if k.plain_xyz() => {
                set(s, "X", GffValue::Single(p[0]));
                set(s, "Y", GffValue::Single(p[1]));
                set(s, "Z", GffValue::Single(p[2]));
            }
            _ => {
                set(s, "XPosition", GffValue::Single(p[0]));
                set(s, "YPosition", GffValue::Single(p[1]));
                set(s, "ZPosition", GffValue::Single(p[2]));
            }
        }
    }

    /// Radians about +Z for a +Y-facing model (k2-data convention).
    pub fn bearing(&self) -> f32 {
        let s = &self.st;
        match self.kind {
            Kind::Camera => 0.0,
            k if k.plain_xyz() => num(s, "Bearing"),
            _ => {
                let (x, y) = (num(s, "XOrientation"), s.field("YOrientation").map_or(1.0, |_| num(s, "YOrientation")));
                bearing_from_dir(x, y)
            }
        }
    }

    pub fn set_bearing(&mut self, b: f32) {
        let k = self.kind;
        let s = &mut self.st;
        match k {
            Kind::Camera | Kind::Trigger | Kind::Encounter | Kind::Sound => {}
            k if k.plain_xyz() => set(s, "Bearing", GffValue::Single(b)),
            _ => {
                let [x, y] = dir_from_bearing(b);
                set(s, "XOrientation", GffValue::Single(x));
                set(s, "YOrientation", GffValue::Single(y));
            }
        }
    }

    pub fn template(&self) -> String {
        match self.st.field(self.kind.template_label()) {
            Some(GffValue::ResRef(r)) => r.to_string().to_ascii_lowercase(),
            Some(GffValue::String(s)) => s.to_ascii_lowercase(),
            _ => String::new(),
        }
    }

    pub fn set_template(&mut self, resref: &str) {
        let label = self.kind.template_label();
        set(&mut self.st, label, resref_value(resref));
    }

    /// The tag stored in the GIT entry, if this kind keeps one there.
    pub fn git_tag(&self) -> Option<String> {
        match self.st.field("Tag") {
            Some(GffValue::String(s)) => Some(s.clone()),
            _ => None,
        }
    }

    /// Polygon points relative to the position (triggers, encounters).
    pub fn polygon(&self) -> Vec<[f32; 3]> {
        let Some((lx, ly, lz)) = self.kind.polygon_labels() else { return Vec::new() };
        match self.st.field("Geometry") {
            Some(GffValue::List(pts)) => pts.iter().map(|p| [num(p, lx), num(p, ly), num(p, lz)]).collect(),
            _ => Vec::new(),
        }
    }

    pub fn set_polygon(&mut self, pts: &[[f32; 3]]) {
        let Some((lx, ly, lz)) = self.kind.polygon_labels() else { return };
        let id = match self.st.field("Geometry") {
            Some(GffValue::List(l)) => l.first().map_or(3, |s| s.struct_id),
            _ => 3,
        };
        let list = pts
            .iter()
            .map(|p| {
                let mut s = GffStruct::new(id);
                set(&mut s, lx, GffValue::Single(p[0]));
                set(&mut s, ly, GffValue::Single(p[1]));
                set(&mut s, lz, GffValue::Single(p[2]));
                s
            })
            .collect();
        set(&mut self.st, "Geometry", GffValue::List(list));
    }
}

fn label(l: &str) -> GffLabel {
    GffLabel::new(l).expect("valid GFF label")
}

fn num(s: &GffStruct, l: &str) -> f32 {
    match s.field(l) {
        Some(GffValue::Single(v)) => *v,
        Some(GffValue::Double(v)) => *v as f32,
        Some(GffValue::Int32(v)) => *v as f32,
        _ => 0.0,
    }
}

/// Replaces a field's value (keeping its slot) or appends it.
pub fn set(s: &mut GffStruct, l: &str, v: GffValue) {
    match s.fields.iter_mut().find(|f| f.label == l) {
        Some(f) => f.value = v,
        None => s.fields.push(GffField { label: label(l), value: v }),
    }
}

fn resref_value(r: &str) -> GffValue {
    let r: String = r.chars().filter(|c| c.is_ascii_graphic()).take(16).collect::<String>().to_ascii_lowercase();
    GffValue::ResRef(ResRef::new(&r).unwrap_or_else(|_| ResRef::new("").expect("empty resref")))
}

/// A field value as text for the inspector.
pub fn show(v: Option<&GffValue>) -> String {
    match v {
        None => String::new(),
        Some(GffValue::String(s)) => s.clone(),
        Some(GffValue::ResRef(r)) => r.to_string(),
        Some(GffValue::UInt8(n)) => n.to_string(),
        Some(GffValue::UInt16(n)) => n.to_string(),
        Some(GffValue::Int32(n)) => n.to_string(),
        Some(GffValue::UInt32(n)) => n.to_string(),
        Some(GffValue::Single(n)) => format!("{n:.3}"),
        Some(other) => format!("{other:?}").chars().take(40).collect(),
    }
}

/// Parses inspector text into the field's type.
fn parse(ty: FieldTy, text: &str) -> Result<GffValue, String> {
    let t = text.trim();
    match ty {
        FieldTy::Str => Ok(GffValue::String(t.to_owned())),
        FieldTy::ResRef => {
            if t.len() > 16 {
                return Err(format!("resref longer than 16: {t}"));
            }
            Ok(resref_value(t))
        }
        FieldTy::Byte => t.parse::<u8>().map(GffValue::UInt8).map_err(|e| format!("{t}: {e}")),
        FieldTy::Word => t.parse::<u16>().map(GffValue::UInt16).map_err(|e| format!("{t}: {e}")),
    }
}

#[derive(Clone)]
struct Snapshot {
    insts: Vec<Inst>,
    blueprints: BTreeMap<String, (Kind, Gff)>,
    next_uid: u64,
}

/// The area document.
pub struct EditDoc {
    pub module: String,
    /// GIT / ARE resref.
    pub area: String,
    file_type: [u8; 4],
    /// Root with every list field as found (lists are rebuilt on save).
    root: GffStruct,
    pub insts: Vec<Inst>,
    /// Forked blueprints by new resref.
    pub blueprints: BTreeMap<String, (Kind, Gff)>,
    next_uid: u64,
    undo: Vec<Snapshot>,
    redo: Vec<Snapshot>,
    pub dirty: bool,
}

impl EditDoc {
    /// Loads the module's area GIT. `res` gets the module mounted.
    pub fn load(res: &mut Resources, module: &str) -> Result<Self, String> {
        let module = module.to_ascii_lowercase();
        res.load_module(&module).map_err(|e| e.to_string())?;
        let ifo = k2_data::read_gff(res, "module", ResourceType::Ifo).map_err(|e| e.to_string())?;
        let info = k2_data::module::ModuleInfo::from_gff(&ifo.root);
        let area = info.entry_area.to_ascii_lowercase();
        let git = k2_data::read_gff(res, &area, ResourceType::Git).map_err(|e| e.to_string())?;
        let mut doc = Self {
            module,
            area,
            file_type: git.file_type,
            root: git.root.clone(),
            insts: Vec::new(),
            blueprints: BTreeMap::new(),
            next_uid: 1,
            undo: Vec::new(),
            redo: Vec::new(),
            dirty: false,
        };
        for kind in Kind::ALL {
            if let Some(GffValue::List(list)) = git.root.field(kind.list()) {
                for st in list {
                    let uid = doc.uid();
                    doc.insts.push(Inst { uid, kind, st: st.clone() });
                }
            }
        }
        Ok(doc)
    }

    fn uid(&mut self) -> u64 {
        self.next_uid += 1;
        self.next_uid
    }

    pub fn get(&self, uid: u64) -> Option<&Inst> {
        self.insts.iter().find(|i| i.uid == uid)
    }

    fn get_mut(&mut self, uid: u64) -> Option<&mut Inst> {
        self.insts.iter_mut().find(|i| i.uid == uid)
    }

    // -- undo ---------------------------------------------------------------

    fn snapshot(&self) -> Snapshot {
        Snapshot { insts: self.insts.clone(), blueprints: self.blueprints.clone(), next_uid: self.next_uid }
    }

    /// Call before every mutation: records an undo point.
    pub fn checkpoint(&mut self) {
        let s = self.snapshot();
        self.undo.push(s);
        if self.undo.len() > 200 {
            self.undo.remove(0);
        }
        self.redo.clear();
        self.dirty = true;
    }

    fn restore(&mut self, s: Snapshot) {
        self.insts = s.insts;
        self.blueprints = s.blueprints;
        self.next_uid = s.next_uid;
        self.dirty = true;
    }

    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }

    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }

    pub fn undo(&mut self) -> bool {
        let Some(s) = self.undo.pop() else { return false };
        let cur = self.snapshot();
        self.redo.push(cur);
        self.restore(s);
        true
    }

    pub fn redo(&mut self) -> bool {
        let Some(s) = self.redo.pop() else { return false };
        let cur = self.snapshot();
        self.undo.push(cur);
        self.restore(s);
        true
    }

    // -- edits (callers checkpoint first) ----------------------------------

    pub fn move_to(&mut self, uid: u64, p: [f32; 3]) {
        if let Some(i) = self.get_mut(uid) {
            i.set_position(p);
        }
    }

    pub fn rotate(&mut self, uid: u64, delta: f32) {
        if let Some(i) = self.get_mut(uid) {
            let b = i.bearing() + delta;
            i.set_bearing(b);
        }
    }

    /// Points an instance at another blueprint of its kind.
    pub fn retemplate(&mut self, uid: u64, template: &str) {
        if let Some(i) = self.get_mut(uid) {
            i.set_template(&template.to_ascii_lowercase());
        }
    }

    pub fn delete(&mut self, uid: u64) {
        self.insts.retain(|i| i.uid != uid);
    }

    /// Copies an instance, offset by `offset`; returns the new uid.
    pub fn duplicate(&mut self, uid: u64, offset: [f32; 3]) -> Option<u64> {
        let mut copy = self.get(uid)?.clone();
        copy.uid = self.uid();
        let p = copy.position();
        copy.set_position([p[0] + offset[0], p[1] + offset[1], p[2] + offset[2]]);
        let id = copy.uid;
        let at = self.insts.iter().position(|i| i.uid == uid).map_or(self.insts.len(), |n| n + 1);
        self.insts.insert(at, copy);
        Some(id)
    }

    /// Adds an instance of a blueprint at `p`; returns the new uid. Clones an
    /// existing entry of the same kind as the field template when there is
    /// one, so the entry has every field the GIT format carries.
    pub fn add(&mut self, res: &Resources, kind: Kind, template: &str, p: [f32; 3], bearing: f32, polygon: &[[f32; 3]]) -> Result<u64, String> {
        let bp = self.blueprint_gff(res, kind, template)?;
        let bp_tag = match bp.root.field("Tag") {
            Some(GffValue::String(t)) => t.clone(),
            _ => template.to_owned(),
        };
        let mut st = match self.insts.iter().find(|i| i.kind == kind) {
            Some(i) => i.st.clone(),
            None => GffStruct::new(kind.struct_id()),
        };
        if self.insts.iter().all(|i| i.kind != kind) {
            minimal_fields(kind, &mut st);
        }
        if kind.polygon_labels().is_some() {
            set(&mut st, "Geometry", GffValue::List(Vec::new()));
        }
        let uid = self.uid();
        let mut inst = Inst { uid, kind, st };
        inst.set_template(template);
        inst.set_position(p);
        inst.set_bearing(bearing);
        if inst.git_tag().is_some() || matches!(kind, Kind::Door | Kind::Waypoint | Kind::Trigger | Kind::Store) {
            set(&mut inst.st, "Tag", GffValue::String(bp_tag));
        }
        // Clear per-instance links copied from the cloned entry.
        for l in ["LinkedTo", "LinkedToModule"] {
            if inst.st.field(l).is_some() {
                let v = if l == "LinkedTo" { GffValue::String(String::new()) } else { resref_value("") };
                set(&mut inst.st, l, v);
            }
        }
        if !polygon.is_empty() {
            inst.set_polygon(polygon);
        }
        self.insts.push(inst);
        Ok(uid)
    }

    // -- fields -------------------------------------------------------------

    /// The blueprint GFF behind a template: a fork from this session, or the
    /// resource as installed (mod dirs, override, module, chitin).
    pub fn blueprint_gff(&self, res: &Resources, kind: Kind, template: &str) -> Result<Gff, String> {
        if let Some((_, g)) = self.blueprints.get(&template.to_ascii_lowercase()) {
            return Ok(g.clone());
        }
        let ty = kind.blueprint().ok_or("no blueprint")?;
        k2_data::read_gff(res, template, ty).map_err(|e| e.to_string())
    }

    pub fn field_text(&self, res: &Resources, uid: u64, spec: &FieldSpec) -> String {
        let Some(i) = self.get(uid) else { return String::new() };
        match spec.src {
            Src::Git => show(i.st.field(spec.label)),
            Src::Blueprint => self.blueprint_gff(res, i.kind, &i.template()).map(|g| show(g.root.field(spec.label))).unwrap_or_default(),
        }
    }

    /// Sets a field from text. Blueprint fields fork the blueprint for this
    /// instance (once) under a new resref. Callers checkpoint first.
    pub fn set_field(&mut self, res: &Resources, uid: u64, spec: &FieldSpec, text: &str) -> Result<(), String> {
        let value = parse(spec.ty, text)?;
        let (kind, template) = {
            let i = self.get(uid).ok_or("no such instance")?;
            (i.kind, i.template())
        };
        match spec.src {
            Src::Git => {
                let i = self.get_mut(uid).ok_or("no such instance")?;
                set(&mut i.st, spec.label, value);
            }
            Src::Blueprint => {
                let shared = self.insts.iter().filter(|o| o.kind == kind && o.template() == template).count() > 1;
                let target = if self.blueprints.contains_key(&template) && !shared {
                    template.clone()
                } else {
                    let mut g = self.blueprint_gff(res, kind, &template)?;
                    let fork = self.fork_name(res, &template);
                    set(&mut g.root, "TemplateResRef", resref_value(&fork));
                    self.blueprints.insert(fork.clone(), (kind, g));
                    self.get_mut(uid).ok_or("no such instance")?.set_template(&fork);
                    fork
                };
                let (_, g) = self.blueprints.get_mut(&target).ok_or("fork missing")?;
                set(&mut g.root, spec.label, value);
            }
        }
        Ok(())
    }

    /// A free resref derived from `base`: `<base[..12]>_eNN`.
    fn fork_name(&self, res: &Resources, base: &str) -> String {
        let stem: String = base.chars().take(12).collect();
        for n in 1..100 {
            let cand = format!("{stem}_e{n:02}");
            let taken = self.blueprints.contains_key(&cand)
                || Kind::ALL.iter().filter_map(|k| k.blueprint()).any(|ty| res.locate(&ResKey::new(&cand, ty)).is_some());
            if !taken {
                return cand;
            }
        }
        format!("{stem}_e99")
    }

    // -- save ---------------------------------------------------------------

    /// The GIT as it would be written.
    pub fn to_gff(&self) -> Gff {
        let mut root = self.root.clone();
        for kind in Kind::ALL {
            let list: Vec<GffStruct> = self.insts.iter().filter(|i| i.kind == kind).map(|i| i.st.clone()).collect();
            if root.field(kind.list()).is_some() || !list.is_empty() {
                set(&mut root, kind.list(), GffValue::List(list));
            }
        }
        Gff::new(self.file_type, root)
    }

    /// Writes the area as a MOD capsule at `path`: `<area>.git` plus the
    /// forked blueprints (override layout inside the capsule). Returns the
    /// number of resources written.
    pub fn save(&mut self, path: &Path) -> Result<usize, String> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        }
        let git = rakata_formats::write_gff_to_vec(&self.to_gff()).map_err(|e| e.to_string())?;
        let mut entries = vec![(ResKey::new(&self.area, ResourceType::Git), git)];
        for (name, (kind, g)) in &self.blueprints {
            let bytes = rakata_formats::write_gff_to_vec(g).map_err(|e| e.to_string())?;
            entries.push((ResKey::new(name, kind.blueprint().unwrap_or(ResourceType::Gff)), bytes));
        }
        let count = entries.len();
        k2_res::write_mod(path, entries).map_err(|e| e.to_string())?;
        self.dirty = false;
        Ok(count)
    }
}

/// Fields a GIT entry of `kind` needs when no entry of the kind exists to
/// clone (per the BioWare GIT format and what k2-data reads).
fn minimal_fields(kind: Kind, st: &mut GffStruct) {
    let z = || GffValue::Single(0.0);
    match kind {
        Kind::Door | Kind::Placeable => {
            for l in ["X", "Y", "Z", "Bearing"] {
                set(st, l, z());
            }
            if kind == Kind::Door {
                set(st, "Tag", GffValue::String(String::new()));
                set(st, "LinkedTo", GffValue::String(String::new()));
                set(st, "LinkedToFlags", GffValue::UInt8(0));
                set(st, "LinkedToModule", resref_value(""));
                set(st, "TransitionDestin", GffValue::LocalizedString(GffLocalizedString::default()));
            }
        }
        Kind::Camera => {}
        _ => {
            for l in ["XPosition", "YPosition", "ZPosition", "XOrientation"] {
                set(st, l, z());
            }
            set(st, "YOrientation", GffValue::Single(1.0));
            match kind {
                Kind::Waypoint => {
                    set(st, "Tag", GffValue::String(String::new()));
                    set(st, "LocalizedName", GffValue::LocalizedString(GffLocalizedString::default()));
                    set(st, "HasMapNote", GffValue::UInt8(0));
                    set(st, "MapNote", GffValue::LocalizedString(GffLocalizedString::default()));
                    set(st, "LinkedTo", GffValue::String(String::new()));
                }
                Kind::Trigger => {
                    set(st, "Tag", GffValue::String(String::new()));
                    set(st, "LinkedTo", GffValue::String(String::new()));
                    set(st, "LinkedToFlags", GffValue::UInt8(0));
                    set(st, "LinkedToModule", resref_value(""));
                    set(st, "TransitionDestin", GffValue::LocalizedString(GffLocalizedString::default()));
                }
                Kind::Sound => set(st, "GeneratedType", GffValue::UInt32(0)),
                Kind::Store => set(st, "Tag", GffValue::String(String::new())),
                _ => {}
            }
        }
    }
}

/// A module's name for the Load Map list: see `map_title`, else the resref.
pub fn map_name(install: &Install, talk: Option<&Talk>, module: &str) -> String {
    map_title(install, talk, module).unwrap_or_else(|| module.to_owned())
}

/// A module's real name: the entry area's ARE Name, else that area's name in
/// the module named after it (a copy in another module can be a placeholder:
/// 006ebo's entry area 003ebo is "Untitled" there), else the IFO Mod_Name.
/// Placeholders are skipped (`is_placeholder`). None when none of those is a
/// real name. Reads the module's capsules without mounting them.
pub fn map_title(install: &Install, talk: Option<&Talk>, module: &str) -> Option<String> {
    let ifo = read_gff(install, module, "module", ResourceType::Ifo)?;
    let area = ifo.root.resref("Mod_Entry_Area").or_else(|| ifo.root.list("Mod_Area_list").iter().find_map(|a| a.resref("Area_Name")));
    let area = area.as_deref();
    let real = |name: String| (!is_placeholder(&name, module, area)).then_some(name);
    let entry = area.and_then(|a| are_name(install, talk, module, a)).and_then(real);
    entry
        .or_else(|| area.and_then(|a| canonical_area_name(install, talk, a)))
        .or_else(|| gff_text(talk, &ifo.root, "Mod_Name").and_then(real))
}

/// The name of area `area` in the module of the same resref, when that is
/// real.
fn canonical_area_name(install: &Install, talk: Option<&Talk>, area: &str) -> Option<String> {
    let name = are_name(install, talk, area, area)?;
    (!is_placeholder(&name, area, Some(area))).then_some(name)
}

/// The Name of ARE `area` in `module`, through the TLK.
fn are_name(install: &Install, talk: Option<&Talk>, module: &str, area: &str) -> Option<String> {
    let are = read_gff(install, module, area, ResourceType::Are)?;
    gff_text(talk, &are.root, "Name")
}

/// A GFF resource from a module's capsules, None when absent or unreadable.
fn read_gff(install: &Install, module: &str, name: &str, ty: ResourceType) -> Option<Gff> {
    let bytes = install.read_module_resource(module, &ResKey::new(name, ty)).ok()??;
    rakata_formats::read_gff_from_bytes(&bytes).ok()
}

/// A localized string field, through the TLK when there is one.
fn gff_text(talk: Option<&Talk>, s: &GffStruct, label: &str) -> Option<String> {
    let ls = s.locstring(label);
    let t = match talk {
        Some(talk) => talk.resolve(&ls),
        None => ls.substrings.first().map(|(_, t)| t.clone()).unwrap_or_default(),
    };
    let t = t.trim();
    (!t.is_empty()).then(|| t.to_owned())
}

/// A name that stands for a missing one: the module's or entry area's resref
/// in any case ("101PER", and 907mal's Mod_Name "904MAL" for its area 904mal),
/// "Untitled", or "module" plus digits ("module001"). Any other name is real,
/// including one word ("Taris").
fn is_placeholder(name: &str, module: &str, area: Option<&str>) -> bool {
    let lower = name.to_ascii_lowercase();
    let numbered = lower.strip_prefix("module").is_some_and(|d| !d.is_empty() && d.bytes().all(|b| b.is_ascii_digit()));
    name.eq_ignore_ascii_case(module) || area.is_some_and(|a| name.eq_ignore_ascii_case(a)) || lower == "untitled" || numbered
}

/// The default mod capsule for a mod name: ~/.local/share/kotor2-rs/mods/<name>.mod,
/// or in the first K2_MOD_DIR folder when the run names one (a supervised run's
/// own mods folder, so it never writes the player's).
pub fn mod_path(name: &str) -> PathBuf {
    let mod_dir = std::env::var_os("K2_MOD_DIR");
    let xdg = std::env::var_os("XDG_DATA_HOME");
    let home = std::env::var_os("HOME");
    mod_path_in(name, mod_dir.as_deref(), xdg.as_deref(), home.as_deref())
}

/// [`mod_path`] with the environment passed in.
fn mod_path_in(name: &str, mod_dir: Option<&std::ffi::OsStr>, xdg: Option<&std::ffi::OsStr>, home: Option<&std::ffi::OsStr>) -> PathBuf {
    if let Some(dir) = mod_dir.and_then(|v| std::env::split_paths(v).find(|p| !p.as_os_str().is_empty())) {
        return dir.join(format!("{name}.mod"));
    }
    let base = xdg
        .map(PathBuf::from)
        .or_else(|| home.map(|h| PathBuf::from(h).join(".local/share")))
        .unwrap_or_else(|| PathBuf::from("."));
    base.join("kotor2-rs").join("mods").join(format!("{name}.mod"))
}

/// Every blueprint the install offers: (kind, resref), sorted, deduplicated.
pub fn blueprint_catalog(res: &Resources) -> Vec<(Kind, String)> {
    let mut v: Vec<(Kind, String)> = res
        .override_keys()
        .chain(res.module_keys())
        .chain(res.chitin_keys())
        .filter_map(|k| Kind::from_blueprint(k.ty).map(|kind| (kind, k.name.clone())))
        .collect();
    v.sort();
    v.dedup();
    v
}

#[cfg(test)]
mod tests;

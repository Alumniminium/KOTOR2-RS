//! Walkmesh queries for KotOR2 areas.
//!
//! All coordinates are game space: Z-up, meters. Room walkmeshes (WOK) are
//! already in world space and are added as-is. Placeable (PWK) and door (DWK)
//! walkmeshes are object-local and are added with a transform.
//!
//! Model: walkable room faces form the ground. Edges of the walkable region
//! (unshared edges with no walkable ground just across them) are blocking
//! segments. Object walkmeshes are blockers: their faces, projected onto the
//! XY plane, block movement; walkable object faces also act as ground. Actors
//! are vertical cylinders: a center point that must stay on walkable ground
//! and a radius kept clear of blocking segments.

use std::collections::HashMap;
use std::sync::OnceLock;

use glam::{Affine3A, Vec2, Vec3, Vec3Swizzles};
use rakata_formats::{Bwm, TwoDa};

/// Largest height an actor climbs in one move.
pub const STEP_UP: f32 = 0.6;
/// Largest height an actor drops in one move.
pub const STEP_DOWN: f32 = 1.0;
/// Height of the actor cylinder used to filter blockers by Z.
pub const ACTOR_HEIGHT: f32 = 1.8;

mod nav;
pub use nav::{FULL_BUDGET, NAV_CELL, NavStats, NodeRoute, RouteMiss, SpotSearch, SpotStep};

const CELL: f32 = 2.0;
const SEAM_PROBE: f32 = 0.05;
const SEAM_SUBSEG: f32 = 0.25;

/// Per-material flags from surfacemat.2da.
#[derive(Debug, Clone, Default)]
pub struct Materials {
    names: Vec<String>,
    walk: Vec<bool>,
    los: Vec<bool>,
}

impl Materials {
    /// Builds from surfacemat.2da. `walk` gives walkability; `lineofsight`
    /// = 1 means the face blocks line of sight.
    pub fn from_twoda(t: &TwoDa) -> Self {
        let flag = |row: usize, col: &str| t.cell(row, col).is_some_and(|v| v.trim() == "1");
        let rows = 0..t.row_count();
        Self {
            names: rows.clone().map(|r| t.cell(r, "label").unwrap_or("").to_owned()).collect(),
            walk: rows.clone().map(|r| flag(r, "walk")).collect(),
            los: rows.map(|r| flag(r, "lineofsight")).collect(),
        }
    }

    pub fn from_2da_bytes(bytes: &[u8]) -> Result<Self, rakata_formats::TwoDaBinaryError> {
        rakata_formats::read_twoda_from_bytes(bytes).map(|t| Self::from_twoda(&t))
    }

    /// Unknown ids are not walkable.
    pub fn walkable(&self, id: u32) -> bool {
        self.walk.get(id as usize).copied().unwrap_or(false)
    }

    pub fn blocks_los(&self, id: u32) -> bool {
        self.los.get(id as usize).copied().unwrap_or(false)
    }

    pub fn name(&self, id: u32) -> &str {
        self.names.get(id as usize).map_or("", String::as_str)
    }
}

/// A mesh added to a [`WalkMesh`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct MeshHandle(pub u32);

/// A face in a [`WalkMesh`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct FaceId(pub u32);

/// A door's three walkmeshes, toggled together.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct DoorHandle {
    pub closed: Option<MeshHandle>,
    pub open1: Option<MeshHandle>,
    pub open2: Option<MeshHandle>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DoorState {
    Closed,
    Open1,
    Open2,
}

/// Result of a ground or raycast query.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Hit {
    pub z: f32,
    pub face: FaceId,
    pub material: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    Room,
    Object,
}

/// How a route may pass a closed door: at a cost (metres), and whether the door is
/// locked (a walker stops at a locked door; an unlocked one it opens and goes on).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DoorRoute {
    pub cost: f32,
    pub locked: bool,
}

#[derive(Debug, Clone)]
struct Mesh {
    kind: Kind,
    enabled: bool,
    /// Set for a closed door that routes may pass (see `set_door_route`).
    door: Option<DoorRoute>,
}

impl Mesh {
    /// True when the mesh blocks a route node or edge: enabled, and not a door routes may pass.
    fn blocks(&self) -> bool {
        self.enabled && self.door.is_none()
    }

    /// True when the mesh blocks a straight leg (smoothing, start choice): enabled, and
    /// not an unlocked door (a walker opens that one on the way).
    fn hard_blocks(&self) -> bool {
        self.enabled && self.door.is_none_or(|d| d.locked)
    }

    /// The route cost of passing this mesh (0 for anything but an enabled door).
    fn door_cost(&self) -> f32 {
        if self.enabled { self.door.map_or(0.0, |d| d.cost) } else { 0.0 }
    }
}

#[derive(Debug, Clone)]
struct Face {
    v: [Vec3; 3],
    normal: Vec3,
    material: u32,
    mesh: u32,
}

#[derive(Debug, Clone, Copy)]
struct Seg {
    a: Vec3,
    b: Vec3,
    mesh: u32,
}

#[derive(Debug, Default, Clone)]
struct Grid {
    cells: HashMap<(i32, i32), Vec<u32>>,
    /// Items spanning more than BIG_CELLS cells (backdrop ground planes,
    /// e.g. a 6 km face in 421DXN), checked everywhere instead of gridded.
    big: Vec<u32>,
}

const BIG_CELLS: i64 = 4096;

impl Grid {
    fn key(p: Vec2) -> (i32, i32) {
        ((p.x / CELL).floor() as i32, (p.y / CELL).floor() as i32)
    }

    fn insert(&mut self, id: u32, min: Vec2, max: Vec2) {
        let (x0, y0) = Self::key(min);
        let (x1, y1) = Self::key(max);
        if i64::from(x1 - x0 + 1) * i64::from(y1 - y0 + 1) > BIG_CELLS {
            self.big.push(id);
            return;
        }
        for x in x0..=x1 {
            for y in y0..=y1 {
                self.cells.entry((x, y)).or_default().push(id);
            }
        }
    }

    fn at(&self, p: Vec2) -> impl Iterator<Item = &u32> {
        self.cells.get(&Self::key(p)).map_or(&[][..], Vec::as_slice).iter().chain(self.big.iter())
    }

    /// Ids in every cell touching the box, possibly with duplicates.
    fn visit(&self, min: Vec2, max: Vec2, mut f: impl FnMut(u32)) {
        let (x0, y0) = Self::key(min);
        let (x1, y1) = Self::key(max);
        for x in x0..=x1 {
            for y in y0..=y1 {
                if let Some(v) = self.cells.get(&(x, y)) {
                    v.iter().copied().for_each(&mut f);
                }
            }
        }
        self.big.iter().copied().for_each(&mut f);
    }
}

/// Builds the static room part of a [`WalkMesh`].
#[derive(Default)]
pub struct WalkMeshBuilder {
    materials: Materials,
    rooms: Vec<Vec<Face>>,
}

impl WalkMeshBuilder {
    pub fn new(materials: Materials) -> Self {
        Self { materials, rooms: Vec::new() }
    }

    /// Adds an area room walkmesh (WOK). Its vertices are world space.
    pub fn add_room(&mut self, bwm: &Bwm) -> &mut Self {
        let mesh = self.rooms.len() as u32;
        self.rooms.push(faces_of(bwm, &Affine3A::IDENTITY, mesh));
        self
    }

    pub fn build(self) -> WalkMesh {
        let mut wm = WalkMesh { materials: self.materials, ..WalkMesh::default() };
        for faces in self.rooms {
            wm.meshes.push(Mesh { kind: Kind::Room, enabled: true, door: None });
            for f in faces {
                wm.push_face(f);
            }
        }
        wm.build_room_boundaries();
        wm
    }
}

fn faces_of(bwm: &Bwm, xf: &Affine3A, mesh: u32) -> Vec<Face> {
    let verts: Vec<Vec3> = bwm.vertices.iter().map(|v| xf.transform_point3(Vec3::new(v.x, v.y, v.z))).collect();
    bwm.faces
        .iter()
        .filter_map(|f| {
            let [a, b, c] = f.vertex_indices.map(|i| verts.get(i as usize).copied());
            let v = [a?, b?, c?];
            let normal = (v[1] - v[0]).cross(v[2] - v[0]).normalize_or_zero();
            Some(Face { v, normal, material: f.material_id, mesh })
        })
        .collect()
}

/// A closed door's walk link: its centre, and the unit direction a walker passes
/// through its doorway. The grid joins the floor either side of the door.
#[derive(Debug, Clone, Copy)]
pub(crate) struct DoorLink {
    pub(crate) mesh: u32,
    pub(crate) centre: Vec3,
    pub(crate) normal: Vec2,
    /// The door's floor: the lowest height of its blocker, where a walker stands either side.
    pub(crate) floor: f32,
}

/// Queryable walkmesh for one area plus its placeables and doors.
#[derive(Debug, Default, Clone)]
pub struct WalkMesh {
    materials: Materials,
    meshes: Vec<Mesh>,
    faces: Vec<Face>,
    face_grid: Grid,
    segs: Vec<Seg>,
    /// One per closed door: where its doorway joins the grid (see `add_door_link`).
    door_links: Vec<DoorLink>,
    seg_grid: Grid,
    /// Navigation grid, built on first path query; reset when objects are
    /// added (door and placeable enable flags are read live).
    nav: OnceLock<nav::Nav>,
}

impl WalkMesh {
    pub fn materials(&self) -> &Materials {
        &self.materials
    }

    pub fn face_count(&self) -> usize {
        self.faces.len()
    }

    pub fn segment_count(&self) -> usize {
        self.segs.len()
    }

    /// Bounds of all room faces, as (min, max).
    pub fn bounds(&self) -> (Vec3, Vec3) {
        let mut mn = Vec3::splat(f32::MAX);
        let mut mx = Vec3::splat(f32::MIN);
        for f in self.faces.iter().filter(|f| self.meshes[f.mesh as usize].kind == Kind::Room) {
            for v in f.v {
                mn = mn.min(v);
                mx = mx.max(v);
            }
        }
        (mn, mx)
    }

    /// Bounds of walkable room faces, as (min, max).
    pub fn walkable_bounds(&self) -> (Vec3, Vec3) {
        let mut mn = Vec3::splat(f32::MAX);
        let mut mx = Vec3::splat(f32::MIN);
        for f in self.faces.iter().filter(|f| self.meshes[f.mesh as usize].kind == Kind::Room && self.materials.walkable(f.material)) {
            for v in f.v {
                mn = mn.min(v);
                mx = mx.max(v);
            }
        }
        (mn, mx)
    }

    fn push_face(&mut self, f: Face) {
        let id = self.faces.len() as u32;
        let (mn, mx) = tri_bounds2(&f.v);
        self.face_grid.insert(id, mn, mx);
        self.faces.push(f);
    }

    fn push_seg(&mut self, s: Seg) {
        let id = self.segs.len() as u32;
        self.seg_grid.insert(id, s.a.xy().min(s.b.xy()), s.a.xy().max(s.b.xy()));
        self.segs.push(s);
    }

    /// Blocking segments for the walkable room region: unshared edges of
    /// walkable faces, split into short pieces, kept only where no walkable
    /// ground lies just across. This tolerates seams between room WOKs whose
    /// vertices do not line up exactly.
    fn build_room_boundaries(&mut self) {
        let q = |p: Vec3| ((p.x * 100.0).round() as i64, (p.y * 100.0).round() as i64, (p.z * 100.0).round() as i64);
        let mut count: HashMap<((i64, i64, i64), (i64, i64, i64)), u32> = HashMap::new();
        let walk: Vec<u32> = (0..self.faces.len() as u32).filter(|&i| self.materials.walkable(self.faces[i as usize].material)).collect();
        for &i in &walk {
            let f = &self.faces[i as usize];
            for e in 0..3 {
                let (a, b) = (q(f.v[e]), q(f.v[(e + 1) % 3]));
                *count.entry(if a < b { (a, b) } else { (b, a) }).or_default() += 1;
            }
        }
        let mut pending = Vec::new();
        for &i in &walk {
            let f = &self.faces[i as usize];
            let centroid = (f.v[0] + f.v[1] + f.v[2]) / 3.0;
            for e in 0..3 {
                let (a, b) = (f.v[e], f.v[(e + 1) % 3]);
                let (qa, qb) = (q(a), q(b));
                if count[&if qa < qb { (qa, qb) } else { (qb, qa) }] > 1 {
                    continue;
                }
                let dir = (b - a).xy();
                let len = dir.length();
                if len < 1e-4 {
                    continue;
                }
                // Outward normal: away from the face centroid.
                let mut out = Vec2::new(dir.y, -dir.x) / len;
                if out.dot(centroid.xy() - a.xy()) > 0.0 {
                    out = -out;
                }
                let n = (len / SEAM_SUBSEG).ceil().max(1.0) as usize;
                for k in 0..n {
                    let (t0, t1) = (k as f32 / n as f32, (k + 1) as f32 / n as f32);
                    let (p0, p1) = (a.lerp(b, t0), a.lerp(b, t1));
                    let mid = p0.lerp(p1, 0.5);
                    let probe = mid.xy() + out * SEAM_PROBE;
                    let across = self.ground_filtered(probe.x, probe.y, mid.z + 0.3, |h| (h.z - mid.z).abs() < 0.3, Some(Kind::Room));
                    if across.is_none() {
                        pending.push(Seg { a: p0, b: p1, mesh: f.mesh });
                    }
                }
            }
        }
        for s in pending {
            self.push_seg(s);
        }
    }

    /// Adds a placeable (PWK) or door (DWK) walkmesh with its world transform.
    pub fn add_object(&mut self, bwm: &Bwm, xf: Affine3A) -> MeshHandle {
        // PWK/DWK vertices are local to the walkmesh node, whose offset in the
        // model is the header `position` (absolute hooks = relative hooks +
        // position). Open-door DWKs depend on it: dor_per011 sits at x -2.42
        // (the slid-open leaf); without it the leaf quad lands mid-doorway.
        // Room WOKs (add_room) are already in area space and skip this.
        let p = bwm.position;
        let xf = xf * Affine3A::from_translation(Vec3::new(p.x, p.y, p.z));
        let mesh = self.meshes.len() as u32;
        self.nav = OnceLock::new();
        self.meshes.push(Mesh { kind: Kind::Object, enabled: true, door: None });
        for f in faces_of(bwm, &xf, mesh) {
            let walkable = self.materials.walkable(f.material);
            if !walkable {
                // Projected edges of blocker faces; degenerate (vertical)
                // faces project to their silhouette, which is the same set.
                for e in 0..3 {
                    let (a, b) = (f.v[e], f.v[(e + 1) % 3]);
                    if (b - a).xy().length_squared() > 1e-8 {
                        self.push_seg(Seg { a, b, mesh });
                    }
                }
            }
            self.push_face(f);
        }
        MeshHandle(mesh)
    }

    /// Adds a door's walkmeshes (`<model>0/1/2.dwk`: closed, open1, open2).
    pub fn add_door(&mut self, closed: Option<&Bwm>, open1: Option<&Bwm>, open2: Option<&Bwm>, xf: Affine3A) -> DoorHandle {
        let h = DoorHandle {
            closed: closed.map(|b| self.add_object(b, xf)),
            open1: open1.map(|b| self.add_object(b, xf)),
            open2: open2.map(|b| self.add_object(b, xf)),
        };
        if let Some(closed) = h.closed {
            self.add_door_link(closed);
        }
        self.set_door(h, DoorState::Closed);
        h
    }

    /// Records where a closed door's walkmesh sits: its centre, and the way a walker
    /// passes its doorway (across the door's longest blocker segment). The doorway has
    /// no walkable faces, so without this the grid would be cut at every closed door.
    fn add_door_link(&mut self, closed: MeshHandle) {
        let segs: Vec<(Vec3, Vec3)> = self.segs.iter().filter(|s| s.mesh == closed.0).map(|s| (s.a, s.b)).collect();
        let Some(longest) = segs.iter().copied().max_by(|x, y| (x.1 - x.0).length_squared().total_cmp(&(y.1 - y.0).length_squared())) else { return };
        let along = (longest.1 - longest.0).xy().normalize_or_zero();
        if along == Vec2::ZERO {
            return;
        }
        let sum = segs.iter().fold(Vec3::ZERO, |acc, (a, b)| acc + *a + *b);
        let centre = sum / (2.0 * segs.len() as f32);
        let floor = segs.iter().fold(f32::INFINITY, |lo, (a, b)| lo.min(a.z).min(b.z));
        self.door_links.push(DoorLink { mesh: closed.0, centre, normal: Vec2::new(-along.y, along.x), floor });
    }

    pub fn set_door(&mut self, h: DoorHandle, state: DoorState) {
        for (m, s) in [(h.closed, DoorState::Closed), (h.open1, DoorState::Open1), (h.open2, DoorState::Open2)] {
            if let Some(m) = m {
                self.set_enabled(m, s == state);
            }
        }
    }

    pub fn set_enabled(&mut self, h: MeshHandle, enabled: bool) {
        if let Some(m) = self.meshes.get_mut(h.0 as usize) {
            m.enabled = enabled;
        }
    }

    /// Sets how routes may pass a closed door's walkmesh: `Some` for a door a walker can
    /// pass (at the route cost given, and stopping at it when locked), `None` for a
    /// blocker (a static door, or a closed door with no route through it).
    pub fn set_door_route(&mut self, h: MeshHandle, route: Option<DoorRoute>) {
        if let Some(m) = self.meshes.get_mut(h.0 as usize) {
            m.door = route;
        }
    }

    pub fn is_enabled(&self, h: MeshHandle) -> bool {
        self.meshes.get(h.0 as usize).is_some_and(|m| m.enabled)
    }

    fn face_live(&self, f: &Face) -> bool {
        self.meshes[f.mesh as usize].enabled
    }

    /// Faces whose XY projection contains (x, y), with the Z on each face.
    fn hits_at(&self, x: f32, y: f32) -> impl Iterator<Item = (u32, f32)> + '_ {
        let p = Vec2::new(x, y);
        self.face_grid.at(p).filter_map(move |&id| {
            let f = &self.faces[id as usize];
            if !self.face_live(f) {
                return None;
            }
            tri_z_at(&f.v, p).map(|z| (id, z))
        })
    }

    fn ground_filtered(&self, x: f32, y: f32, z_max: f32, ok: impl Fn(&Hit) -> bool, kind: Option<Kind>) -> Option<Hit> {
        let mut best: Option<Hit> = None;
        for (id, z) in self.hits_at(x, y) {
            let f = &self.faces[id as usize];
            if z > z_max || !self.materials.walkable(f.material) {
                continue;
            }
            if kind.is_some_and(|k| self.meshes[f.mesh as usize].kind != k) {
                continue;
            }
            let h = Hit { z, face: FaceId(id), material: f.material };
            if ok(&h) && best.is_none_or(|b| z > b.z) {
                best = Some(h);
            }
        }
        best
    }

    /// Highest walkable face under (x, y) at or below `z_hint + STEP_UP`.
    pub fn ground(&self, x: f32, y: f32, z_hint: f32) -> Option<Hit> {
        self.ground_filtered(x, y, z_hint + STEP_UP, |_| true, None)
    }

    /// The room floor under (x, y) nearest in height to `z`, within a step of it. Unlike
    /// `ground`, a surface stacked above or below the walker (a placeable's top) is not
    /// taken for the floor the walker stands on.
    pub fn ground_nearest(&self, x: f32, y: f32, z: f32) -> Option<Hit> {
        let mut best: Option<Hit> = None;
        for (id, hz) in self.hits_at(x, y) {
            let f = &self.faces[id as usize];
            if !self.materials.walkable(f.material) || self.meshes[f.mesh as usize].kind != Kind::Room || (hz - z).abs() > STEP_UP + 0.4 {
                continue;
            }
            if best.is_none_or(|b| (hz - z).abs() < (b.z - z).abs()) {
                best = Some(Hit { z: hz, face: FaceId(id), material: f.material });
            }
        }
        best
    }

    /// The entry point if it is on open walkable ground, else the nearest sampled point that
    /// is. Some entry points sit on small walkmesh islands (101PER: a 2-face platform under the
    /// Exile's bed, cut off from the floor by a face-less gap). The original engine relocates
    /// creatures to a safe location on spawn; this does the same. `radius` is the clearance the
    /// mover walks with.
    pub fn safe_spawn(&self, entry: Vec3, radius: f32) -> Option<Vec3> {
        let on = |x: f32, y: f32| self.ground(x, y, entry.z + 1.0).filter(|h| (h.z - entry.z).abs() < 2.0).map(|h| Vec3::new(x, y, h.z));
        let here = on(entry.x, entry.y);
        if let Some(p) = here
            && self.roomy(p, radius)
        {
            return Some(p);
        }
        for ring in 1..=40 {
            let r = ring as f32 * 0.25;
            let n = (ring * 8).max(8);
            for k in 0..n {
                let a = k as f32 / n as f32 * std::f32::consts::TAU;
                if let Some(p) = on(entry.x + r * a.cos(), entry.y + r * a.sin())
                    && self.roomy(p, radius)
                {
                    return Some(p);
                }
            }
        }
        here
    }

    /// True if a mover of `radius` at `p` can walk at least 2 m in two different directions,
    /// i.e. it is not trapped on a walkmesh island.
    fn roomy(&self, p: Vec3, radius: f32) -> bool {
        let mut free = 0;
        for k in 0..8 {
            let a = k as f32 * std::f32::consts::FRAC_PI_4;
            let d = Vec2::new(a.cos(), a.sin());
            let mut q = p;
            for _ in 0..10 {
                q = self.move_slide(q, d * 0.25, radius);
            }
            if (q - p).truncate().length() > 2.0 {
                free += 1;
                if free >= 2 {
                    return true;
                }
            }
        }
        false
    }

    /// First face of any material straight below (x, y, z_from).
    pub fn raycast_down(&self, x: f32, y: f32, z_from: f32) -> Option<Hit> {
        self.hits_at(x, y)
            .filter(|&(_, z)| z <= z_from)
            .max_by(|a, b| a.1.total_cmp(&b.1))
            .map(|(id, z)| Hit { z, face: FaceId(id), material: self.faces[id as usize].material })
    }

    /// The face (any material) under (x, y) closest in height to `z_hint`.
    pub fn face_at(&self, x: f32, y: f32, z_hint: f32) -> Option<FaceId> {
        self.hits_at(x, y).min_by(|a, b| (a.1 - z_hint).abs().total_cmp(&(b.1 - z_hint).abs())).map(|(id, _)| FaceId(id))
    }

    pub fn face_material(&self, f: FaceId) -> Option<u32> {
        self.faces.get(f.0 as usize).map(|f| f.material)
    }

    /// Unit face normal (zero for degenerate faces).
    pub fn face_normal(&self, f: FaceId) -> Option<Vec3> {
        self.faces.get(f.0 as usize).map(|f| f.normal)
    }

    pub fn face_vertices(&self, f: FaceId) -> Option<[Vec3; 3]> {
        self.faces.get(f.0 as usize).map(|f| f.v)
    }

    /// True if no line-of-sight-blocking face crosses the open segment a-b.
    pub fn line_of_sight(&self, a: Vec3, b: Vec3) -> bool {
        let d = b - a;
        let mut seen = std::collections::HashSet::new();
        let mut clear = true;
        self.face_grid.visit(a.xy().min(b.xy()), a.xy().max(b.xy()), |id| {
            if !clear || !seen.insert(id) {
                return;
            }
            let f = &self.faces[id as usize];
            if !self.face_live(f) || !self.materials.blocks_los(f.material) {
                return;
            }
            if let Some(t) = ray_tri(a, d, &f.v)
                && t > 1e-3
                && t < 1.0 - 1e-3
            {
                clear = false;
            }
        });
        clear
    }

    /// Live blocking segments within `reach` of `p` as (a, b, is_room, mesh), for
    /// diagnostics.
    pub fn blockers_near(&self, p: Vec3, reach: f32) -> Vec<(Vec3, Vec3, bool, u32)> {
        let mut ids = Vec::new();
        self.blocking_near(p, reach, &mut ids);
        ids.iter()
            .map(|&i| {
                let s = &self.segs[i as usize];
                (s.a, s.b, self.meshes[s.mesh as usize].kind == Kind::Room, s.mesh)
            })
            .collect()
    }

    fn blocking_near(&self, p: Vec3, reach: f32, out: &mut Vec<u32>) {
        out.clear();
        let r = Vec2::splat(reach);
        self.seg_grid.visit(p.xy() - r, p.xy() + r, |id| out.push(id));
        out.sort_unstable();
        out.dedup();
        out.retain(|&id| {
            let s = &self.segs[id as usize];
            if !self.meshes[s.mesh as usize].enabled {
                return false;
            }
            // Only blockers overlapping the actor's vertical extent.
            let (lo, hi) = (s.a.z.min(s.b.z), s.a.z.max(s.b.z));
            let room = self.meshes[s.mesh as usize].kind == Kind::Room;
            if room {
                lo <= p.z + STEP_UP && hi >= p.z - STEP_DOWN
            } else {
                // Flat object footprints sit at their own floor height.
                lo <= p.z + ACTOR_HEIGHT && hi >= p.z - STEP_DOWN
            }
        });
    }

    /// Moves an actor standing at `from` by `delta` (XY used; Z follows the
    /// ground), sliding along blocking edges. The result is always on
    /// walkable ground; if no valid move exists it returns `from`.
    pub fn move_slide(&self, from: Vec3, delta: Vec2, radius: f32) -> Vec3 {
        let len = delta.length();
        if len < 1e-6 {
            return from;
        }
        let steps = (len / (radius * 0.5).max(0.05)).ceil().clamp(1.0, 64.0) as usize;
        let step = delta / steps as f32;
        let mut pos = from;
        let mut near = Vec::new();
        for _ in 0..steps {
            let mut cand = pos.xy() + step;
            // Push out of nearby blockers, a few relaxation passes.
            for _ in 0..4 {
                self.blocking_near(Vec3::new(cand.x, cand.y, pos.z), radius + 0.5, &mut near);
                let mut moved = false;
                for &id in &near {
                    let s = &self.segs[id as usize];
                    let c = closest_on_seg(cand, s.a.xy(), s.b.xy());
                    let d = cand - c;
                    let dist = d.length();
                    if dist < radius {
                        let n = if dist > 1e-6 { d / dist } else { seg_normal_toward(s.a.xy(), s.b.xy(), pos.xy()) };
                        cand = c + n * (radius + 1e-3);
                        moved = true;
                    }
                }
                if !moved {
                    break;
                }
            }
            // Never move further than the requested step (push-out can overshoot).
            let off = cand - pos.xy();
            if off.length() > step.length() * 1.5 + 1e-3 {
                cand = pos.xy() + off.normalize_or_zero() * step.length();
            }
            match self.settle(pos, cand) {
                Some(p) => pos = p,
                None => {
                    // Axis-separated fallback so corners slide instead of stick.
                    let a = self.settle(pos, Vec2::new(pos.x + step.x, pos.y)).filter(|_| step.x.abs() > 1e-6);
                    let b = self.settle(pos, Vec2::new(pos.x, pos.y + step.y)).filter(|_| step.y.abs() > 1e-6);
                    match a.or(b) {
                        Some(p) => pos = p,
                        None => break,
                    }
                }
            }
        }
        pos
    }

    /// Ground under `xy` reachable from `from` (step limits), clear of blockers.
    fn settle(&self, from: Vec3, xy: Vec2) -> Option<Vec3> {
        let h = self.ground(xy.x, xy.y, from.z)?;
        if h.z < from.z - STEP_DOWN {
            return None;
        }
        let p = Vec3::new(xy.x, xy.y, h.z);
        // Must not cross a blocking segment between the two positions.
        let mut near = Vec::new();
        self.blocking_near(p, (xy - from.xy()).length() + 0.1, &mut near);
        for &id in &near {
            let s = &self.segs[id as usize];
            if segs_cross(from.xy(), xy, s.a.xy(), s.b.xy()) {
                return None;
            }
        }
        Some(p)
    }

    /// True if (x, y, z) stands on walkable ground (within step tolerance).
    pub fn is_walkable(&self, p: Vec3) -> bool {
        self.ground(p.x, p.y, p.z).is_some_and(|h| (h.z - p.z).abs() <= STEP_UP)
    }
}

fn tri_bounds2(v: &[Vec3; 3]) -> (Vec2, Vec2) {
    let mn = v[0].xy().min(v[1].xy()).min(v[2].xy());
    let mx = v[0].xy().max(v[1].xy()).max(v[2].xy());
    (mn, mx)
}

/// Z on the triangle's plane at XY `p`, if `p` lies inside its projection.
fn tri_z_at(v: &[Vec3; 3], p: Vec2) -> Option<f32> {
    let (a, b, c) = (v[0].xy(), v[1].xy(), v[2].xy());
    let d = (b - a).perp_dot(c - a);
    if d.abs() < 1e-9 {
        return None;
    }
    let w1 = (p - a).perp_dot(c - a) / d;
    let w2 = (b - a).perp_dot(p - a) / d;
    let w0 = 1.0 - w1 - w2;
    const E: f32 = -1e-5;
    if w0 < E || w1 < E || w2 < E {
        return None;
    }
    Some(w0 * v[0].z + w1 * v[1].z + w2 * v[2].z)
}

/// Moller-Trumbore; returns t along `d` (unnormalized) for a hit.
fn ray_tri(o: Vec3, d: Vec3, v: &[Vec3; 3]) -> Option<f32> {
    let e1 = v[1] - v[0];
    let e2 = v[2] - v[0];
    let p = d.cross(e2);
    let det = e1.dot(p);
    if det.abs() < 1e-9 {
        return None;
    }
    let inv = 1.0 / det;
    let s = o - v[0];
    let u = s.dot(p) * inv;
    if !(0.0..=1.0).contains(&u) {
        return None;
    }
    let q = s.cross(e1);
    let w = d.dot(q) * inv;
    if w < 0.0 || u + w > 1.0 {
        return None;
    }
    Some(e2.dot(q) * inv)
}

fn closest_on_seg(p: Vec2, a: Vec2, b: Vec2) -> Vec2 {
    let ab = b - a;
    let l2 = ab.length_squared();
    if l2 < 1e-12 {
        return a;
    }
    a + ab * ((p - a).dot(ab) / l2).clamp(0.0, 1.0)
}

fn seg_normal_toward(a: Vec2, b: Vec2, toward: Vec2) -> Vec2 {
    let d = (b - a).normalize_or_zero();
    let n = Vec2::new(-d.y, d.x);
    if n.dot(toward - a) >= 0.0 { n } else { -n }
}

/// Proper crossing of segments p0-p1 and q0-q1 (touching does not count).
fn segs_cross(p0: Vec2, p1: Vec2, q0: Vec2, q1: Vec2) -> bool {
    let r = p1 - p0;
    let s = q1 - q0;
    let den = r.perp_dot(s);
    if den.abs() < 1e-12 {
        return false;
    }
    let t = (q0 - p0).perp_dot(s) / den;
    let u = (q0 - p0).perp_dot(r) / den;
    t > 1e-4 && t < 1.0 - 1e-4 && u > 1e-4 && u < 1.0 - 1e-4
}

#[cfg(test)]
mod unit;

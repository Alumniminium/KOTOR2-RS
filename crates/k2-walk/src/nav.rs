//! Pathfinding over a [`WalkMesh`].
//!
//! The walkable room surface is sampled into a layered grid: one node per
//! (cell, distinct ground height), so bridges over floors get separate nodes.
//! Each node stores its clearance to static blockers (room boundary edges)
//! and its distance to every nearby object blocker (placeable PWK, door DWK).
//! Object blockers are checked against the mesh's enabled flag at query time,
//! so opening or closing a door needs no rebuild. Adding an object does.
//!
//! A query runs A* over nodes passable for the actor radius, then string-pulls
//! the node chain into a few straight segments with a grid line-of-walk test.

use std::cmp::Ordering;
use std::collections::{BinaryHeap, HashMap};

use glam::{Vec2, Vec3, Vec3Swizzles};

use crate::{closest_on_seg, segs_cross, tri_z_at, Kind, WalkMesh, ACTOR_HEIGHT, MeshHandle, STEP_DOWN, STEP_UP};

/// Grid cell size in meters.
pub const NAV_CELL: f32 = 0.4;
/// Clearance is measured up to this distance; larger is "open".
const MAX_CLEAR: f32 = 1.0;
/// Ground heights closer than this in one cell are one layer.
const LAYER_MERGE: f32 = 0.35;
/// Largest height change between neighbouring nodes.
const STEP: f32 = 0.6;
/// Actor radius limits used by queries.
const R_MIN: f32 = 0.1;
const R_MAX: f32 = 0.45;
/// Extra clearance demanded by straight segments, so string-pulled lines do
/// not hug walls closer than the node grid allows.
const LINE_MARGIN: f32 = 0.1;
/// Start and goal snapping distances.
const SNAP_START: f32 = 1.2;
const SNAP_GOAL: f32 = 3.0;
const MAX_EXPAND: usize = 400_000;
/// Heuristic weight. Above 1 trades a little optimality (string pulling
/// straightens the result anyway) for far fewer expansions.
const H_WEIGHT: f32 = 1.2;
const NONE: u32 = u32::MAX;

const DIRS: [(i32, i32); 8] = [(1, 0), (-1, 0), (0, 1), (0, -1), (1, 1), (1, -1), (-1, 1), (-1, -1)];

#[derive(Clone)]
struct Node {
    p: Vec3,
    /// Distance to the nearest static blocker, capped at MAX_CLEAR.
    clear: f32,
    /// Object mesh id and distance, for object blockers within MAX_CLEAR.
    dynamic: Vec<(u32, f32)>,
    /// Bit k set: edge in direction k has an entry in `edge_dynamic`.
    dyn_edges: u8,
}

/// Why a route search found no route. `OverBudget`: the budget ran out before
/// any goal candidate was reached, so a search with more budget may still find
/// one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RouteMiss {
    Unreachable,
    OverBudget,
}

/// The unsmoothed node chain of a route, from the start node to the goal node.
/// `cut`: the budget ran out first, so the goal may not be the candidate
/// nearest the target.
#[derive(Debug, Clone, PartialEq)]
pub struct NodeRoute {
    pub nodes: Vec<Vec3>,
    pub cut: bool,
}

/// A route search toward a set of spots, run in steps (`WalkMesh::spot_step`):
/// each step expands up to its budget, keeps the open list, and returns for the
/// next frame. The goals are the passable nodes near a spot that the search's
/// `goal` takes, each with an exit cost. The search ends at the goal whose route
/// cost plus exit cost is least: with zero exit costs, the first goal reached.
/// `max_expand` caps the whole search, over all its steps.
#[derive(Clone, Debug)]
pub struct SpotSearch {
    r: f32,
    max_expand: usize,
    expanded: usize,
    /// Exit cost of each goal node, by node index; infinity for other nodes.
    exit: Vec<f32>,
    /// The grid the search was built on: a rebuilt grid has other node indices.
    generation: u64,
    /// Planar box of the goal nodes, and per landmark that reaches every goal
    /// the least and greatest distance from the landmark to a goal.
    bbox: (Vec2, Vec2),
    lm_range: Vec<(usize, f32, f32)>,
    open: BinaryHeap<SpotOpen>,
    g: Vec<f32>,
    came: Vec<u32>,
}

impl SpotSearch {
    /// Nodes expanded so far, over all steps.
    pub fn expanded(&self) -> usize {
        self.expanded
    }
}

/// What one step of a `SpotSearch` found.
#[derive(Debug, Clone, PartialEq)]
pub enum SpotStep {
    /// The step's budget ran out with the search still open: step again.
    Pending,
    /// A goal was reached: the node positions from the start to the goal, goal last.
    Found(Vec<Vec3>),
    /// No goal is reachable (`Unreachable`), or the whole budget ran out first (`OverBudget`).
    Miss(RouteMiss),
}

/// The budget of `find_path`: the most nodes a plain search expands.
pub const FULL_BUDGET: usize = MAX_EXPAND;

pub(crate) struct Nav {
    origin: Vec2,
    w: i32,
    h: i32,
    /// Node ranges per cell: nodes of cell c are cell_start[c]..cell_start[c+1].
    cell_start: Vec<u32>,
    nodes: Vec<Node>,
    adj: Vec<[u32; 8]>,
    /// Least distance from each edge to a room boundary (`edge_wall_gap`),
    /// capped at R_MAX: a value of R_MAX means at least R_MAX. An edge is walked
    /// only when this is at least the radius, so routes keep clear of walls
    /// along their edges, not just at the nodes.
    edge_gap: Vec<[f32; 8]>,
    /// Object meshes whose blockers cross the edge (node, dir).
    edge_dynamic: HashMap<(u32, u8), Vec<u32>>,
    /// Doorway edges: node -> (the node across a closed door, that door's mesh).
    link_adj: HashMap<u32, Vec<(u32, u32)>>,
    /// Connected component per node, ignoring object blockers.
    comp: Vec<u32>,
    /// ALT landmarks: static shortest distances from a few far-apart nodes
    /// (f32::INFINITY where unreachable). Gives a much tighter A* heuristic
    /// than straight-line distance on winding maps.
    landmarks: Vec<Vec<f32>>,
    /// Tells this grid's build apart from every other, so a `SpotSearch` built
    /// on one grid is refused by another (their node indices differ).
    generation: u64,
    scratch: std::sync::Mutex<Scratch>,
}

static NAV_GENERATIONS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);

fn next_generation() -> u64 {
    NAV_GENERATIONS.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
}

/// Per-query A* arrays, reused; `stamp` marks entries valid for a query.
#[derive(Default)]
struct Scratch {
    stamp: Vec<u32>,
    g: Vec<f32>,
    came: Vec<u32>,
    epoch: u32,
}

impl Clone for Nav {
    fn clone(&self) -> Self {
        Nav {
            origin: self.origin,
            w: self.w,
            h: self.h,
            cell_start: self.cell_start.clone(),
            nodes: self.nodes.clone(),
            adj: self.adj.clone(),
            edge_gap: self.edge_gap.clone(),
            edge_dynamic: self.edge_dynamic.clone(),
            link_adj: self.link_adj.clone(),
            comp: self.comp.clone(),
            landmarks: self.landmarks.clone(),
            generation: self.generation,
            scratch: Default::default(),
        }
    }
}

impl std::fmt::Debug for Nav {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Nav {{ {}x{} cells, {} nodes }}", self.w, self.h, self.nodes.len())
    }
}

/// Size of a built navigation grid.
#[derive(Debug, Clone, Copy)]
pub struct NavStats {
    pub nodes: usize,
    pub edges: usize,
    pub components: usize,
}

impl Nav {
    fn cell_of(&self, p: Vec2) -> Option<(i32, i32)> {
        let c = ((p - self.origin) / NAV_CELL).floor();
        let (x, y) = (c.x as i32, c.y as i32);
        (x >= 0 && y >= 0 && x < self.w && y < self.h).then_some((x, y))
    }

    fn cell_nodes(&self, x: i32, y: i32) -> std::ops::Range<u32> {
        if x < 0 || y < 0 || x >= self.w || y >= self.h {
            return 0..0;
        }
        let c = (y * self.w + x) as usize;
        self.cell_start[c]..self.cell_start[c + 1]
    }

    /// The grid nodes within `reach` of `p` (2D), within a step of height `z`, and clear
    /// of walls (the nodes a walker standing at `p` on the floor at `z` can use).
    fn side_nodes(&self, p: Vec2, z: f32, reach: f32) -> Vec<u32> {
        let Some((cx, cy)) = self.cell_of(p) else { return Vec::new() };
        let k = (reach / NAV_CELL).ceil() as i32;
        let mut out = Vec::new();
        for y in cy - k..=cy + k {
            for x in cx - k..=cx + k {
                for n in self.cell_nodes(x, y) {
                    let q = &self.nodes[n as usize];
                    if q.p.xy().distance(p) <= reach && (q.p.z - z).abs() <= STEP_UP + 0.4 && q.clear >= R_MIN {
                        out.push(n);
                    }
                }
            }
        }
        out
    }

    pub(crate) fn build(wm: &WalkMesh) -> Nav {
        let (mn, mx) = nav_bounds(wm);
        let origin = mn.xy() - Vec2::splat(NAV_CELL);
        let w = (((mx.x - origin.x) / NAV_CELL).ceil() as i32 + 2).max(1);
        let h = (((mx.y - origin.y) / NAV_CELL).ceil() as i32 + 2).max(1);
        // Huge open planes (the 421DXN turret minigame) get no grid; moves
        // there fall back to straight lines.
        if !mn.x.is_finite() || mx.x < mn.x || i64::from(w) * i64::from(h) > MAX_CELLS {
            return Nav {
                origin,
                w: 1,
                h: 1,
                cell_start: vec![0, 0],
                nodes: Vec::new(),
                adj: Vec::new(),
                edge_gap: Vec::new(),
                edge_dynamic: HashMap::new(),
                link_adj: HashMap::new(),
                comp: Vec::new(),
                landmarks: Vec::new(),
                generation: next_generation(),
                scratch: Default::default(),
            };
        }

        // Nodes: one per distinct walkable room height under each cell center.
        let mut cell_start = Vec::with_capacity((w * h + 1) as usize);
        let mut nodes = Vec::new();
        let mut zs = Vec::new();
        for y in 0..h {
            for x in 0..w {
                cell_start.push(nodes.len() as u32);
                let c = origin + (Vec2::new(x as f32, y as f32) + 0.5) * NAV_CELL;
                zs.clear();
                for &id in wm.face_grid.at(c) {
                    let f = &wm.faces[id as usize];
                    if wm.meshes[f.mesh as usize].kind != Kind::Room || !wm.materials.walkable(f.material) {
                        continue;
                    }
                    if let Some(z) = tri_z_at(&f.v, c) {
                        zs.push(z);
                    }
                }
                zs.sort_by(|a, b| b.total_cmp(a));
                let mut last = f32::INFINITY;
                for &z in &zs {
                    if last - z > LAYER_MERGE {
                        nodes.push(Node { p: Vec3::new(c.x, c.y, z), clear: MAX_CLEAR, dynamic: Vec::new(), dyn_edges: 0 });
                        last = z;
                    }
                }
            }
        }
        cell_start.push(nodes.len() as u32);

        // Clearance against static and object blockers.
        let r = Vec2::splat(MAX_CLEAR);
        let mut near = Vec::new();
        for n in &mut nodes {
            near.clear();
            wm.seg_grid.visit(n.p.xy() - r, n.p.xy() + r, |id| near.push(id));
            near.sort_unstable();
            near.dedup();
            for &id in &near {
                let s = &wm.segs[id as usize];
                let room = wm.meshes[s.mesh as usize].kind == Kind::Room;
                if !seg_in_height(s.a.z, s.b.z, n.p.z, room) {
                    continue;
                }
                let d = n.p.xy().distance(closest_on_seg(n.p.xy(), s.a.xy(), s.b.xy()));
                if d >= MAX_CLEAR {
                    continue;
                }
                if room {
                    n.clear = n.clear.min(d);
                } else if let Some(e) = n.dynamic.iter_mut().find(|e| e.0 == s.mesh) {
                    e.1 = e.1.min(d);
                } else {
                    n.dynamic.push((s.mesh, d));
                }
            }
        }

        let mut nav = Nav {
            origin,
            generation: next_generation(),
            w,
            h,
            cell_start,
            nodes,
            adj: Vec::new(),
            edge_gap: Vec::new(),
            edge_dynamic: HashMap::new(),
            link_adj: HashMap::new(),
            comp: Vec::new(),
            landmarks: Vec::new(),
            scratch: Default::default(),
        };

        // Edges to the closest-height node of each neighbouring cell.
        let mut adj = vec![[NONE; 8]; nav.nodes.len()];
        let mut gap_arr = vec![[R_MAX; 8]; nav.nodes.len()];
        let mut gap_ids = Vec::new();
        let close = NAV_CELL * 1.5;
        for y in 0..h {
            for x in 0..w {
                for a in nav.cell_nodes(x, y) {
                    let pa = nav.nodes[a as usize].p;
                    for (k, (dx, dy)) in DIRS.iter().enumerate() {
                        let mut best = NONE;
                        let mut best_dz = STEP;
                        for b in nav.cell_nodes(x + dx, y + dy) {
                            let dz = (nav.nodes[b as usize].p.z - pa.z).abs();
                            if dz <= best_dz {
                                best = b;
                                best_dz = dz;
                            }
                        }
                        if best == NONE {
                            continue;
                        }
                        let pb = nav.nodes[best as usize].p;
                        // Midpoint must be on ground between the two heights,
                        // so edges never jump a gap between layers.
                        let mid = (pa + pb) * 0.5;
                        let on_ground = wm.face_grid.at(mid.xy()).any(|&id| {
                            let f = &wm.faces[id as usize];
                            wm.meshes[f.mesh as usize].kind == Kind::Room
                                && wm.materials.walkable(f.material)
                                && tri_z_at(&f.v, mid.xy()).is_some_and(|z| (z - mid.z).abs() <= STEP * 0.5 + 0.05)
                        });
                        if !on_ground {
                            continue;
                        }
                        let (na, nb) = (&nav.nodes[a as usize], &nav.nodes[best as usize]);
                        // A node at R_MAX + 0.29 or more from every wall keeps each point of its edges
                        // (at most 0.29 from an end) at R_MAX or more, so such edges need no search.
                        let gap = if na.clear.min(nb.clear) < R_MAX + 0.29 { edge_wall_gap(wm, pa, pb, mid.z, &mut gap_ids) } else { R_MAX };
                        gap_arr[a as usize][k] = gap;
                        let static_near = na.clear.min(nb.clear) < close;
                        let object_near = na.dynamic.iter().chain(nb.dynamic.iter()).any(|e| e.1 < close);
                        if static_near || object_near {
                            let mut blocked = false;
                            let mut meshes = Vec::new();
                            let lo = pa.xy().min(pb.xy());
                            let hi = pa.xy().max(pb.xy());
                            let mut ids = Vec::new();
                            wm.seg_grid.visit(lo, hi, |id| ids.push(id));
                            ids.sort_unstable();
                            ids.dedup();
                            for id in ids {
                                let s = &wm.segs[id as usize];
                                let room = wm.meshes[s.mesh as usize].kind == Kind::Room;
                                if !seg_in_height(s.a.z, s.b.z, mid.z, room) || !segs_cross(pa.xy(), pb.xy(), s.a.xy(), s.b.xy()) {
                                    continue;
                                }
                                if room {
                                    blocked = true;
                                    break;
                                }
                                if !meshes.contains(&s.mesh) {
                                    meshes.push(s.mesh);
                                }
                            }
                            if blocked {
                                continue;
                            }
                            if !meshes.is_empty() {
                                nav.edge_dynamic.insert((a, k as u8), meshes);
                                nav.nodes[a as usize].dyn_edges |= 1 << k;
                            }
                        }
                        adj[a as usize][k] = best;
                    }
                }
            }
        }
        // Keep edges symmetric: drop any edge whose reverse is missing.
        for a in 0..adj.len() {
            for k in 0..8 {
                let b = adj[a][k];
                if b != NONE && !adj[b as usize].contains(&(a as u32)) {
                    adj[a][k] = NONE;
                }
            }
        }
        nav.adj = adj;
        nav.edge_gap = gap_arr;
        nav.link_adj = link_doorways(&nav, wm);

        // Components over nodes passable for the smallest radius, joined by edges
        // clear of walls by that radius too (so same_region agrees with find_path).
        let mut comp = vec![NONE; nav.nodes.len()];
        let mut next = 0;
        let mut stack = Vec::new();
        for s in 0..nav.nodes.len() {
            if comp[s] != NONE || nav.nodes[s].clear < R_MIN {
                continue;
            }
            comp[s] = next;
            stack.push(s as u32);
            while let Some(n) = stack.pop() {
                for (k, &b) in nav.adj[n as usize].iter().enumerate() {
                    if b != NONE && comp[b as usize] == NONE && nav.nodes[b as usize].clear >= R_MIN && nav.edge_gap[n as usize][k] >= R_MIN {
                        comp[b as usize] = next;
                        stack.push(b);
                    }
                }
                for &(b, _) in nav.link_adj.get(&n).into_iter().flatten() {
                    if comp[b as usize] == NONE && nav.nodes[b as usize].clear >= R_MIN {
                        comp[b as usize] = next;
                        stack.push(b);
                    }
                }
            }
            next += 1;
        }
        nav.comp = comp;
        nav.landmarks = nav.pick_landmarks();
        nav
    }

    /// Static shortest distances from `src` over nodes passable for R_MIN.
    fn dijkstra(&self, src: u32) -> Vec<f32> {
        let mut d = vec![f32::INFINITY; self.nodes.len()];
        let mut open = BinaryHeap::new();
        d[src as usize] = 0.0;
        open.push(Open { f: 0.0, n: src });
        while let Some(Open { f, n }) = open.pop() {
            if f > d[n as usize] {
                continue;
            }
            let pn = self.nodes[n as usize].p;
            for &(b, _) in self.link_adj.get(&n).into_iter().flatten() {
                let c = f + pn.distance(self.nodes[b as usize].p);
                if c < d[b as usize] {
                    d[b as usize] = c;
                    open.push(Open { f: c, n: b });
                }
            }
            for &b in &self.adj[n as usize] {
                if b == NONE || self.nodes[b as usize].clear < R_MIN {
                    continue;
                }
                let c = f + pn.distance(self.nodes[b as usize].p);
                if c < d[b as usize] {
                    d[b as usize] = c;
                    open.push(Open { f: c, n: b });
                }
            }
        }
        d
    }

    /// Farthest-point landmark selection, spread over the big components.
    fn pick_landmarks(&self) -> Vec<Vec<f32>> {
        let mut sizes: HashMap<u32, usize> = HashMap::new();
        for &c in &self.comp {
            if c != NONE {
                *sizes.entry(c).or_default() += 1;
            }
        }
        let mut comps: Vec<(u32, usize)> = sizes.into_iter().filter(|e| e.1 >= 2000).collect();
        comps.sort_by_key(|e| std::cmp::Reverse(e.1));
        let total: usize = comps.iter().map(|e| e.1).sum();
        let mut out = Vec::new();
        for (c, size) in comps {
            let k = ((LANDMARKS * size) as f32 / total as f32).round().max(1.0) as usize;
            let Some(seed) = self.comp.iter().position(|&x| x == c) else { continue };
            // First landmark: farthest from an arbitrary node; then repeatedly
            // the node farthest from all chosen ones.
            let d0 = self.dijkstra(seed as u32);
            let mut far = argmax_finite(&d0);
            let mut min_d = vec![f32::INFINITY; self.nodes.len()];
            for _ in 0..k {
                let d = self.dijkstra(far);
                for (m, &x) in min_d.iter_mut().zip(&d) {
                    *m = m.min(x);
                }
                out.push(d);
                far = argmax_finite(&min_d);
            }
        }
        out
    }

    pub(crate) fn stats(&self) -> NavStats {
        let edges = self.adj.iter().map(|a| a.iter().filter(|&&b| b != NONE).count()).sum::<usize>() / 2;
        let components = self.comp.iter().filter(|&&c| c != NONE).max().map_or(0, |&c| c as usize + 1);
        NavStats { nodes: self.nodes.len(), edges, components }
    }
}

const LANDMARKS: usize = 8;
/// Walkable faces larger than this (m^2) are backdrop ground planes (421DXN
/// has a 6 km one); they do not widen the grid.
const BACKDROP_AREA: f32 = 2500.0;
/// Largest grid extent per axis, in meters.
const MAX_EXTENT: f32 = 800.0;
/// Grids larger than this are not built (about 1.7 km^2 of nodes).
const MAX_CELLS: i64 = 1_500_000;

/// Grid bounds: walkable room faces, ignoring backdrop planes, capped.
fn nav_bounds(wm: &WalkMesh) -> (Vec3, Vec3) {
    let mut mn = Vec3::splat(f32::MAX);
    let mut mx = Vec3::splat(f32::MIN);
    for f in &wm.faces {
        if wm.meshes[f.mesh as usize].kind != Kind::Room || !wm.materials.walkable(f.material) {
            continue;
        }
        let area = (f.v[1] - f.v[0]).xy().perp_dot((f.v[2] - f.v[0]).xy()).abs() * 0.5;
        if area > BACKDROP_AREA {
            continue;
        }
        for v in f.v {
            mn = mn.min(v);
            mx = mx.max(v);
        }
    }
    if mn.x > mx.x {
        (mn, mx) = wm.walkable_bounds();
    }
    if mn.x > mx.x {
        return (mn, mx);
    }
    let c = (mn + mx) * 0.5;
    let half = Vec3::new(MAX_EXTENT, MAX_EXTENT, f32::MAX) * 0.5;
    (mn.max(c - half), mx.min(c + half))
}

fn argmax_finite(d: &[f32]) -> u32 {
    let mut best = (0, -1.0);
    for (i, &x) in d.iter().enumerate() {
        if x.is_finite() && x > best.1 {
            best = (i, x);
        }
    }
    best.0 as u32
}

fn seg_in_height(za: f32, zb: f32, z: f32, room: bool) -> bool {
    let (lo, hi) = (za.min(zb), za.max(zb));
    if room { lo <= z + STEP_UP && hi >= z - STEP_DOWN } else { lo <= z + ACTOR_HEIGHT && hi >= z - STEP_DOWN }
}

/// Lower bound on the walk from node `n` to its nearest goal (unweighted, so
/// the search stays admissible): the planar distance to the goals' box, and for
/// each landmark how far the node's distance lies outside the goals' range (the
/// triangle inequality on the static landmark distances).
fn spot_bound(nav: &Nav, bbox: (Vec2, Vec2), lms: &[(usize, f32, f32)], n: u32) -> f32 {
    let p = nav.nodes[n as usize].p.xy();
    let mut b = p.clamp(bbox.0, bbox.1).distance(p);
    for &(li, least, greatest) in lms {
        let d = nav.landmarks[li][n as usize];
        if d.is_finite() {
            b = b.max((least - d).max(d - greatest));
        }
    }
    b
}

/// The node positions of the chain from a search's start to node `n`, in order.
fn spot_chain(nav: &Nav, came: &[u32], n: u32) -> Vec<Vec3> {
    let mut chain = vec![n];
    let mut c = n;
    while came[c as usize] != NONE {
        c = came[c as usize];
        chain.push(c);
    }
    chain.iter().rev().map(|&c| nav.nodes[c as usize].p).collect()
}

/// Distances either side of a doorway, along the door's passage direction, at which
/// the grid is looked up for the two sides of the door.
const DOOR_SIDES: [f32; 3] = [0.6, 0.9, 1.2];
/// How far from a side point a grid node may be and still stand for that side.
const DOOR_REACH: f32 = 1.5;
/// Longest span across a doorway that a link may bridge.
const DOOR_SPAN: f32 = 3.0;
/// Links a door gets: the shortest node pairs whose line crosses its blocker. One joins the
/// two sides; a second covers a node that stands badly.
const DOOR_LINKS: usize = 2;

/// The doorway edges of the closed doors: for each door, the nodes either side of
/// it, each recorded with the door's mesh. Their route cost and validity are the
/// door's (`edge_extra`, `blocks`).
fn link_doorways(nav: &Nav, wm: &WalkMesh) -> HashMap<u32, Vec<(u32, u32)>> {
    let mut out: HashMap<u32, Vec<(u32, u32)>> = HashMap::new();
    for link in &wm.door_links {
        let blockers: Vec<(Vec2, Vec2)> = wm.segs.iter().filter(|s| s.mesh == link.mesh).map(|s| (s.a.xy(), s.b.xy())).collect();
        // Node pairs (lower id, higher id) whose straight line passes through the door's own
        // blocker, with their lengths: the doorway between them. Several distances either side,
        // in case one is in a wall.
        let mut crossing: HashMap<(u32, u32), f32> = HashMap::new();
        let mut nearest: Option<(f32, u32, u32)> = None;
        for side in DOOR_SIDES {
            let plus = nav.side_nodes(link.centre.xy() + link.normal * side, link.floor, DOOR_REACH);
            let minus = nav.side_nodes(link.centre.xy() - link.normal * side, link.floor, DOOR_REACH);
            for &a in &plus {
                for &b in &minus {
                    let (pa, pb) = (nav.nodes[a as usize].p.xy(), nav.nodes[b as usize].p.xy());
                    let d = pa.distance(pb);
                    if a == b || d > DOOR_SPAN {
                        continue;
                    }
                    if nearest.is_none_or(|(nd, _, _)| d < nd) {
                        nearest = Some((d, a, b));
                    }
                    if blockers.iter().any(|&(q0, q1)| segs_cross(pa, pb, q0, q1)) {
                        crossing.entry((a.min(b), a.max(b))).or_insert(d);
                    }
                }
            }
        }
        let mut picked: Vec<((u32, u32), f32)> = crossing.into_iter().collect();
        picked.sort_by(|x, y| x.1.total_cmp(&y.1));
        picked.truncate(DOOR_LINKS);
        // A doorway whose blocker the lines miss still joins the two nearest nodes.
        if picked.is_empty()
            && let Some((d, a, b)) = nearest
        {
            picked.push(((a.min(b), a.max(b)), d));
        }
        for ((a, b), _) in picked {
            out.entry(a).or_default().push((b, link.mesh));
            out.entry(b).or_default().push((a, link.mesh));
        }
    }
    out
}

/// Least planar distance from the walk `pa` -> `pb` to a room boundary at
/// height `z`, capped at R_MAX. A boundary the walk crosses counts as 0 (the
/// build rejects such edges anyway). Only boundaries within R_MAX are searched.
/// `ids` is a scratch buffer, reused across calls.
fn edge_wall_gap(wm: &WalkMesh, pa: Vec3, pb: Vec3, z: f32, ids: &mut Vec<u32>) -> f32 {
    let reach = Vec2::splat(R_MAX);
    ids.clear();
    wm.seg_grid.visit(pa.xy().min(pb.xy()) - reach, pa.xy().max(pb.xy()) + reach, |id| ids.push(id));
    let (a, b) = (pa.xy(), pb.xy());
    let mut gap = R_MAX;
    for &id in ids.iter() {
        let s = &wm.segs[id as usize];
        if wm.meshes[s.mesh as usize].kind != Kind::Room || !seg_in_height(s.a.z, s.b.z, z, true) {
            continue;
        }
        let (c, d) = (s.a.xy(), s.b.xy());
        let dist = if segs_cross(a, b, c, d) {
            0.0
        } else {
            a.distance(closest_on_seg(a, c, d))
                .min(b.distance(closest_on_seg(b, c, d)))
                .min(c.distance(closest_on_seg(c, a, b)))
                .min(d.distance(closest_on_seg(d, a, b)))
        };
        gap = gap.min(dist);
    }
    gap
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct Open {
    f: f32,
    n: u32,
}

impl Eq for Open {}

impl Ord for Open {
    fn cmp(&self, o: &Self) -> Ordering {
        o.f.total_cmp(&self.f).then_with(|| self.n.cmp(&o.n))
    }
}

/// An entry of a `SpotSearch`'s open list: a node to expand, or (`exit`) the
/// stand at goal node `n`, taken at cost `f` (route plus exit cost).
#[derive(Clone, Copy, Debug, PartialEq)]
struct SpotOpen {
    f: f32,
    n: u32,
    exit: bool,
}

impl Eq for SpotOpen {}

impl Ord for SpotOpen {
    fn cmp(&self, o: &Self) -> Ordering {
        o.f.total_cmp(&self.f).then_with(|| self.n.cmp(&o.n)).then_with(|| self.exit.cmp(&o.exit))
    }
}

impl PartialOrd for SpotOpen {
    fn partial_cmp(&self, o: &Self) -> Option<Ordering> {
        Some(self.cmp(o))
    }
}

impl PartialOrd for Open {
    fn partial_cmp(&self, o: &Self) -> Option<Ordering> {
        Some(self.cmp(o))
    }
}

/// Query context: a nav grid plus the mesh enabled flags of the moment.
struct Query<'a> {
    nav: &'a Nav,
    wm: &'a WalkMesh,
    r: f32,
    /// A locked door is a blocker for this search (a walker that cannot open it), not a costed passage.
    locked_blocks: bool,
}

impl Query<'_> {
    fn blocks(&self, mesh: u32) -> bool {
        let m = &self.wm.meshes[mesh as usize];
        if self.locked_blocks { m.hard_blocks() } else { m.blocks() }
    }

    fn passable(&self, n: u32, margin: f32) -> bool {
        let node = &self.nav.nodes[n as usize];
        let need = self.r + margin;
        node.clear >= need && node.dynamic.iter().all(|&(m, d)| d >= need || !self.blocks(m))
    }

    fn edge_ok(&self, a: u32, k: usize) -> bool {
        let b = self.nav.adj[a as usize][k];
        b != NONE
            && self.nav.edge_gap[a as usize][k] >= self.r
            && self.passable(b, 0.0)
            && (self.nav.nodes[a as usize].dyn_edges & (1 << k) == 0
                || self.nav.edge_dynamic.get(&(a, k as u8)).is_none_or(|ms| ms.iter().all(|&m| !self.blocks(m))))
    }

    /// Extra route cost (metres) of the edge from `a` in direction `k` for the closed
    /// doors it passes: an unlocked door a little, a locked one a lot, so a route goes
    /// round a locked door when it can.
    fn edge_extra(&self, a: u32, k: usize) -> f32 {
        if self.nav.nodes[a as usize].dyn_edges & (1 << k) == 0 {
            return 0.0;
        }
        self.nav.edge_dynamic.get(&(a, k as u8)).map_or(0.0, |ms| ms.iter().map(|&m| self.wm.meshes[m as usize].door_cost()).sum())
    }

    /// The start node for a route from `from` and the goal candidates for `to`:
    /// every passable node near the target in the start's component, nearest
    /// first. A target inside a placeable or closed door footprint has a
    /// nearest candidate that no route enters, so `astar` takes the first
    /// candidate it reaches. None when there is no start or no candidate.
    fn endpoints(&self, from: Vec3, to: Vec3) -> Option<(u32, Vec<u32>)> {
        let start = self.start_node(from)?;
        let comp = self.nav.comp[start as usize];
        let goals: Vec<u32> = self.snap(to, SNAP_GOAL, 2.5).into_iter().filter(|&n| self.nav.comp[n as usize] == comp).collect();
        (!goals.is_empty()).then_some((start, goals))
    }

    /// Passable nodes within `reach` of `p` and `dz` in height, nearest first.
    fn snap(&self, p: Vec3, reach: f32, dz: f32) -> Vec<u32> {
        let mut out: Vec<(f32, u32)> = Vec::new();
        let Some((cx, cy)) = self.nav.cell_of(p.xy()).or_else(|| {
            let c = ((p.xy() - self.nav.origin) / NAV_CELL).floor();
            Some((c.x as i32, c.y as i32))
        }) else {
            return Vec::new();
        };
        let k = (reach / NAV_CELL).ceil() as i32;
        for y in cy - k..=cy + k {
            for x in cx - k..=cx + k {
                for n in self.nav.cell_nodes(x, y) {
                    let q = self.nav.nodes[n as usize].p;
                    let d = q.xy().distance(p.xy());
                    if d <= reach && (q.z - p.z).abs() <= dz && self.passable(n, 0.0) {
                        out.push((d + (q.z - p.z).abs() * 0.5, n));
                    }
                }
            }
        }
        out.sort_by(|a, b| a.0.total_cmp(&b.0));
        out.into_iter().map(|e| e.1).collect()
    }

    /// A* from `start` toward `goals[0]`, the candidate nearest the target.
    /// If that is unreachable, the search has seen every node `start` can
    /// reach, so the first reached candidate in list order is used. When the
    /// budget runs out first, the first candidate reached so far is used and
    /// flagged `cut`; a cut with no candidate reached is `OverBudget`. Returns
    /// the node chain from `start` to the goal.
    fn astar(&self, start: u32, goals: &[u32], max_expand: usize) -> Result<(Vec<u32>, bool), RouteMiss> {
        let Some(&goal) = goals.first() else { return Err(RouteMiss::Unreachable) };
        let nodes = &self.nav.nodes;
        let goal_p = nodes[goal as usize].p;
        // ALT bound from landmarks that reach the goal, or straight line.
        let lms: Vec<(&[f32], f32)> =
            self.nav.landmarks.iter().filter(|l| l[goal as usize].is_finite()).map(|l| (l.as_slice(), l[goal as usize])).collect();
        let h = |n: u32| {
            let mut b = nodes[n as usize].p.distance(goal_p);
            for &(l, lg) in &lms {
                let ln = l[n as usize];
                if ln.is_finite() {
                    b = b.max((lg - ln).abs());
                }
            }
            b * H_WEIGHT
        };
        let mut guard = self.nav.scratch.lock().unwrap_or_else(|e| e.into_inner());
        let sc = &mut *guard;
        if sc.stamp.len() != nodes.len() {
            *sc = Scratch { stamp: vec![0; nodes.len()], g: vec![0.0; nodes.len()], came: vec![NONE; nodes.len()], epoch: 0 };
        }
        sc.epoch = sc.epoch.wrapping_add(1);
        if sc.epoch == 0 {
            sc.stamp.fill(0);
            sc.epoch = 1;
        }
        let epoch = sc.epoch;
        let (stamp, g, came) = (&mut sc.stamp, &mut sc.g, &mut sc.came);
        let get_g = |stamp: &[u32], g: &[f32], n: u32| if stamp[n as usize] == epoch { g[n as usize] } else { f32::INFINITY };
        let chain = |came: &[u32], n: u32| {
            let mut path = vec![n];
            let mut c = n;
            while came[c as usize] != NONE {
                c = came[c as usize];
                path.push(c);
            }
            path.reverse();
            path
        };
        let mut open = BinaryHeap::new();
        stamp[start as usize] = epoch;
        g[start as usize] = 0.0;
        came[start as usize] = NONE;
        open.push(Open { f: h(start), n: start });
        let mut expanded = 0;
        while let Some(Open { f, n }) = open.pop() {
            if n == goal {
                return Ok((chain(came, n), false));
            }
            let gn = g[n as usize];
            if f > gn + h(n) + 1e-4 {
                continue; // stale entry
            }
            expanded += 1;
            if expanded > max_expand {
                // Out of budget: the first candidate reached so far still gives a
                // route (flagged `cut`); with none reached the search is over budget.
                return match goals.iter().find(|&&c| stamp[c as usize] == epoch) {
                    Some(&c) => Ok((chain(came, c), true)),
                    None => Err(RouteMiss::OverBudget),
                };
            }
            let pn = nodes[n as usize].p;
            for k in 0..8 {
                if !self.edge_ok(n, k) {
                    continue;
                }
                let b = self.nav.adj[n as usize][k];
                let pb = nodes[b as usize].p;
                // Prefer open floor a little over hugging walls.
                let wall = if nodes[b as usize].clear < self.r + 0.3 { 1.25 } else { 1.0 };
                let cost = gn + pn.distance(pb) * wall + self.edge_extra(n, k);
                if cost < get_g(stamp, g, b) {
                    stamp[b as usize] = epoch;
                    g[b as usize] = cost;
                    came[b as usize] = n;
                    open.push(Open { f: cost + h(b), n: b });
                }
            }
            // Doorway edges: a closed door joins the nodes either side of it.
            for &(b, mesh) in self.nav.link_adj.get(&n).into_iter().flatten() {
                if self.blocks(mesh) || !self.passable(b, 0.0) {
                    continue;
                }
                let cost = gn + pn.distance(nodes[b as usize].p) + self.wm.meshes[mesh as usize].door_cost();
                if cost < get_g(stamp, g, b) {
                    stamp[b as usize] = epoch;
                    g[b as usize] = cost;
                    came[b as usize] = n;
                    open.push(Open { f: cost + h(b), n: b });
                }
            }
        }
        // Goal unreachable: every node the start reaches has been expanded.
        goals.iter().find(|&&c| stamp[c as usize] == epoch).map(|&c| (chain(came, c), false)).ok_or(RouteMiss::Unreachable)
    }

    /// Straight walk from `a` to `b` stays on passable nodes (grid walk), on
    /// walkable ground under each sample, and clear of live blockers by the radius:
    /// the same tests the mover's slide applies, so a leg that passes is one it walks.
    fn line_ok(&self, a: Vec3, b: Vec3) -> bool {
        let d = b.xy() - a.xy();
        let len = d.length();
        let steps = (len / (NAV_CELL * 0.5)).ceil().max(1.0) as usize;
        let mut z = a.z;
        let mut prev = a.xy();
        for i in 1..=steps {
            let p = a.xy() + d * (i as f32 / steps as f32);
            // The mover needs ground under each sample (no drop past STEP_DOWN) and no
            // blocker nearer than the radius: its slide pushes it out of one.
            let Some(h) = self.wm.ground(p.x, p.y, z) else { return false };
            if h.z < z - STEP_DOWN || self.breached(a.xy(), p, h.z) {
                return false;
            }
            z = h.z;
            let Some((cx, cy)) = self.nav.cell_of(p) else { return false };
            let mut found = None;
            let mut best = STEP;
            for n in self.nav.cell_nodes(cx, cy) {
                let dz = (self.nav.nodes[n as usize].p.z - z).abs();
                if dz <= best {
                    best = dz;
                    found = Some(n);
                }
            }
            let Some(n) = found else { return false };
            if !self.passable(n, LINE_MARGIN) {
                return false;
            }
            z = self.nav.nodes[n as usize].p.z;
            let mut blocked = false;
            self.wm.seg_grid.visit(prev.min(p), prev.max(p), |id| {
                let s = &self.wm.segs[id as usize];
                let m = &self.wm.meshes[s.mesh as usize];
                if !blocked && m.hard_blocks() && seg_in_height(s.a.z, s.b.z, z, m.kind == Kind::Room) && segs_cross(prev, p, s.a.xy(), s.b.xy()) {
                    blocked = true;
                }
            });
            if blocked {
                return false;
            }
            prev = p;
        }
        true
    }

    /// True when a live blocker at height `z` lies nearer `p` than the radius and
    /// the leg did not start within the radius of it: the mover's slide pushes it
    /// out of a blocker it is entering. A blocker the leg starts inside is kept to
    /// its side by the slide, so the crossing test (`line_ok`) decides for it.
    fn breached(&self, start: Vec2, p: Vec2, z: f32) -> bool {
        let r = self.r;
        let mut hit = false;
        self.wm.seg_grid.visit(p - Vec2::splat(r), p + Vec2::splat(r), |id| {
            let s = &self.wm.segs[id as usize];
            let m = &self.wm.meshes[s.mesh as usize];
            if !hit && m.hard_blocks() && seg_in_height(s.a.z, s.b.z, z, m.kind == Kind::Room) {
                let near = |q: Vec2| q.distance(closest_on_seg(q, s.a.xy(), s.b.xy())) < r;
                hit = near(p) && !near(start);
            }
        });
        hit
    }

    /// True when the straight leg from `a` to `b` crosses a live blocker at the
    /// height of `a`: the slide refuses a step across one.
    fn crosses_blocker(&self, a: Vec3, b: Vec3) -> bool {
        self.wm.leg_crosses_blocker(a, b)
    }

    /// The node a route from `from` starts at: the nearest passable node whose
    /// leg from `from` the line check accepts, else the nearest one whose leg does
    /// not cross a blocker. None when no nearby node is reached on the leader's
    /// side of the blockers: a route that starts across one cannot be walked.
    fn start_node(&self, from: Vec3) -> Option<u32> {
        let near = self.snap(from, SNAP_START, STEP_UP + 0.4);
        near.iter()
            .copied()
            .find(|&n| self.line_ok(from, self.nav.nodes[n as usize].p))
            .or_else(|| near.iter().copied().find(|&n| !self.crosses_blocker(from, self.nav.nodes[n as usize].p)))
    }

    /// Farthest-visible string pulling with galloping search.
    fn smooth(&self, pts: &[Vec3]) -> Vec<Vec3> {
        let last = pts.len() - 1;
        let mut out = Vec::new();
        let mut i = 0;
        while i < last {
            let mut lo = i + 1; // known reachable (adjacent)
            let mut step = 1;
            let mut hi = None;
            while lo < last {
                let probe = (lo + step).min(last);
                if self.line_ok(pts[i], pts[probe]) {
                    lo = probe;
                    step *= 2;
                } else {
                    hi = Some(probe);
                    break;
                }
            }
            if let Some(mut hi) = hi {
                while hi - lo > 1 {
                    let mid = (lo + hi) / 2;
                    if self.line_ok(pts[i], pts[mid]) {
                        lo = mid;
                    } else {
                        hi = mid;
                    }
                }
            }
            out.push(pts[lo]);
            i = lo;
        }
        out
    }
}

impl WalkMesh {
    fn nav(&self) -> &Nav {
        self.nav.get_or_init(|| Nav::build(self))
    }

    /// Builds the navigation grid now (otherwise built on first use).
    pub fn prepare_nav(&self) -> NavStats {
        self.nav().stats()
    }

    /// Finds a walkable path from `from` to `to` for an actor of `radius`.
    /// Returns the waypoints after `from`, ending at `to` (or the nearest
    /// reachable spot within 3 m of it). None if unreachable.
    pub fn find_path(&self, from: Vec3, to: Vec3, radius: f32) -> Option<Vec<Vec3>> {
        self.find_path_within(from, to, radius, MAX_EXPAND)
    }

    /// True when the straight leg from `a` to `b` crosses an enabled blocker (a wall,
    /// a placeable, or a closed door's walkmesh) at the height of `a`: the slide refuses
    /// a step across one, so a route leg that does is one the mover cannot walk.
    pub fn leg_crosses_blocker(&self, a: Vec3, b: Vec3) -> bool {
        let mut hit = false;
        self.seg_grid.visit(a.xy().min(b.xy()), a.xy().max(b.xy()), |id| {
            let s = &self.segs[id as usize];
            let m = &self.meshes[s.mesh as usize];
            if !hit && m.hard_blocks() && seg_in_height(s.a.z, s.b.z, a.z, m.kind == Kind::Room) && segs_cross(a.xy(), b.xy(), s.a.xy(), s.b.xy()) {
                hit = true;
            }
        });
        hit
    }

    /// The closed door (a door mesh with a route, see `WalkMesh::set_door_route`) whose blocker the leg from
    /// `a` to `b` crosses, if any: the walker opens that door before it goes on.
    pub fn passable_crossing(&self, a: Vec3, b: Vec3) -> Option<MeshHandle> {
        let mut found = None;
        self.seg_grid.visit(a.xy().min(b.xy()), a.xy().max(b.xy()), |id| {
            let s = &self.segs[id as usize];
            let m = &self.meshes[s.mesh as usize];
            if found.is_none() && m.enabled && m.door.is_some() && seg_in_height(s.a.z, s.b.z, a.z, m.kind == Kind::Room) && segs_cross(a.xy(), b.xy(), s.a.xy(), s.b.xy()) {
                found = Some(MeshHandle(s.mesh));
            }
        });
        found
    }

    /// The unsmoothed node chain of a route from `from` to `to`, within
    /// `max_expand` nodes expanded: every passable node on the way, nearest
    /// `from` first, the goal node last. Each step is an open edge, clear of
    /// room walls by the radius, so following it stays on the walkmesh and
    /// clear of walls. Costs the search alone; `find_path` also string-pulls
    /// the chain, which runs line checks along all of it. Says why when there
    /// is no route. A helper for the k2-walk tests (the game routes with
    /// `SpotSearch`): it is the bounded A* whose cut rule the tests pin down.
    pub fn find_route_nodes_checked(&self, from: Vec3, to: Vec3, radius: f32, max_expand: usize) -> Result<NodeRoute, RouteMiss> {
        let nav = self.nav();
        if nav.nodes.is_empty() {
            return Err(RouteMiss::Unreachable);
        }
        let q = Query { nav, wm: self, r: radius.clamp(R_MIN, R_MAX), locked_blocks: false };
        let Some((start, goals)) = q.endpoints(from, to) else { return Err(RouteMiss::Unreachable) };
        let (chain, cut) = q.astar(start, &goals, max_expand)?;
        Ok(NodeRoute { nodes: chain.iter().map(|&n| nav.nodes[n as usize].p).collect(), cut })
    }

    /// `find_route_nodes_checked` without the reason and the cut flag.
    pub fn find_route_nodes_within(&self, from: Vec3, to: Vec3, radius: f32, max_expand: usize) -> Option<Vec<Vec3>> {
        self.find_route_nodes_checked(from, to, radius, max_expand).ok().map(|r| r.nodes)
    }

    /// Starts a `SpotSearch` from `from` toward `spots`, for an actor of
    /// `radius`, with at most `max_expand` nodes expanded in all. The goals are
    /// the passable nodes within reach of a spot, in `from`'s component, that
    /// `goal` takes; `goal` gives each one's exit cost (see `SpotSearch`). None
    /// when `from` has no node or there is no goal.
    pub fn spot_search(&self, from: Vec3, spots: &[Vec3], radius: f32, max_expand: usize, goal: impl Fn(Vec3) -> Option<f32>) -> Option<SpotSearch> {
        let nav = self.nav();
        if nav.nodes.is_empty() {
            return None;
        }
        let q = Query { nav, wm: self, r: radius.clamp(R_MIN, R_MAX), locked_blocks: false };
        let start = q.start_node(from)?;
        let comp = nav.comp[start as usize];
        let mut exit = vec![f32::INFINITY; nav.nodes.len()];
        let mut goals = Vec::new();
        for &spot in spots {
            for n in q.snap(spot, SNAP_GOAL, 2.5) {
                let i = n as usize;
                if exit[i].is_infinite() && nav.comp[i] == comp && let Some(cost) = goal(nav.nodes[i].p) {
                    exit[i] = cost;
                    goals.push(n);
                }
            }
        }
        if goals.is_empty() {
            return None;
        }
        let lo = goals.iter().fold(Vec2::splat(f32::INFINITY), |m, &g| m.min(nav.nodes[g as usize].p.xy()));
        let hi = goals.iter().fold(Vec2::splat(f32::NEG_INFINITY), |m, &g| m.max(nav.nodes[g as usize].p.xy()));
        let lm_range: Vec<(usize, f32, f32)> = nav
            .landmarks
            .iter()
            .enumerate()
            .filter_map(|(li, l)| {
                let (mut least, mut greatest) = (f32::INFINITY, f32::NEG_INFINITY);
                for &g in &goals {
                    let d = l[g as usize];
                    if !d.is_finite() {
                        return None;
                    }
                    least = least.min(d);
                    greatest = greatest.max(d);
                }
                Some((li, least, greatest))
            })
            .collect();
        let bbox = (lo, hi);
        let mut g = vec![f32::INFINITY; nav.nodes.len()];
        let came = vec![NONE; nav.nodes.len()];
        g[start as usize] = 0.0;
        let mut open = BinaryHeap::new();
        open.push(SpotOpen { f: spot_bound(nav, bbox, &lm_range, start), n: start, exit: false });
        Some(SpotSearch { r: q.r, max_expand, expanded: 0, exit, generation: nav.generation, bbox, lm_range, open, g, came })
    }

    /// One step of a `SpotSearch`: expands up to `budget` nodes, then returns
    /// `Pending` with the open list kept for the next step. Passability and
    /// the mesh's enabled flags are read at each expansion, so a door that
    /// moves between steps is seen by the rest of the search.
    pub fn spot_step(&self, s: &mut SpotSearch, budget: usize) -> SpotStep {
        let nav = self.nav();
        if s.generation != nav.generation {
            return SpotStep::Miss(RouteMiss::Unreachable);
        }
        let q = Query { nav, wm: self, r: s.r, locked_blocks: false };
        let nodes = &nav.nodes;
        let mut spent = 0;
        while let Some(SpotOpen { f, n, exit }) = s.open.pop() {
            if exit {
                return SpotStep::Found(spot_chain(nav, &s.came, n));
            }
            let gn = s.g[n as usize];
            if f > gn + spot_bound(nav, s.bbox, &s.lm_range, n) + 1e-4 {
                continue; // stale entry
            }
            if spent == budget {
                s.open.push(SpotOpen { f, n, exit: false });
                return SpotStep::Pending;
            }
            if s.expanded == s.max_expand {
                return SpotStep::Miss(RouteMiss::OverBudget);
            }
            spent += 1;
            s.expanded += 1;
            if s.exit[n as usize].is_finite() {
                s.open.push(SpotOpen { f: gn + s.exit[n as usize], n, exit: true });
            }
            let pn = nodes[n as usize].p;
            for k in 0..8 {
                if !q.edge_ok(n, k) {
                    continue;
                }
                let b = nav.adj[n as usize][k];
                let pb = nodes[b as usize].p;
                // Prefer open floor a little over hugging walls (as `astar`).
                let wall = if nodes[b as usize].clear < s.r + 0.3 { 1.25 } else { 1.0 };
                let cost = gn + pn.distance(pb) * wall + q.edge_extra(n, k);
                if cost < s.g[b as usize] {
                    s.g[b as usize] = cost;
                    s.came[b as usize] = n;
                    s.open.push(SpotOpen { f: cost + spot_bound(nav, s.bbox, &s.lm_range, b), n: b, exit: false });
                }
            }
            // Doorway edges: a closed door joins the nodes either side of it.
            for &(b, mesh) in nav.link_adj.get(&n).into_iter().flatten() {
                if q.blocks(mesh) || !q.passable(b, 0.0) {
                    continue;
                }
                let cost = gn + pn.distance(nodes[b as usize].p) + q.wm.meshes[mesh as usize].door_cost();
                if cost < s.g[b as usize] {
                    s.g[b as usize] = cost;
                    s.came[b as usize] = n;
                    s.open.push(SpotOpen { f: cost + spot_bound(nav, s.bbox, &s.lm_range, b), n: b, exit: false });
                }
            }
        }
        SpotStep::Miss(RouteMiss::Unreachable)
    }

    /// `find_path` with at most `max_expand` nodes expanded. When the budget
    /// runs out, the route to the first candidate already reached is returned
    /// (`find_route_nodes_checked` flags such a route `cut`); None only when no
    /// candidate was reached, which bounds the cost.
    pub fn find_path_within(&self, from: Vec3, to: Vec3, radius: f32, max_expand: usize) -> Option<Vec<Vec3>> {
        self.find_path_policy(from, to, radius, max_expand, false)
    }

    /// `find_path` for a walker that cannot open a locked door (an NPC's scripted move): a locked
    /// door is a blocker, so the route goes round it or there is none. The leader's walks keep the
    /// locked door as a costed passage (they stop at it with the locked-door feedback).
    pub fn find_path_avoiding_locked(&self, from: Vec3, to: Vec3, radius: f32) -> Option<Vec<Vec3>> {
        self.find_path_policy(from, to, radius, MAX_EXPAND, true)
    }

    fn find_path_policy(&self, from: Vec3, to: Vec3, radius: f32, max_expand: usize, locked_blocks: bool) -> Option<Vec<Vec3>> {
        let nav = self.nav();
        if nav.nodes.is_empty() {
            return None;
        }
        let q = Query { nav, wm: self, r: radius.clamp(R_MIN, R_MAX), locked_blocks };
        let (start, goals) = q.endpoints(from, to)?;
        let (chain, _) = q.astar(start, &goals, max_expand).ok()?;
        let goal = *chain.last()?;
        let mut pts: Vec<Vec3> = Vec::with_capacity(chain.len() + 2);
        pts.push(from);
        pts.extend(chain.iter().map(|&n| nav.nodes[n as usize].p));
        // End exactly on the target when it is itself walkable and in view.
        let end = nav.nodes[goal as usize].p;
        if end.xy().distance(to.xy()) > 1e-3 && self.ground(to.x, to.y, to.z + 0.5).is_some_and(|h| (h.z - end.z).abs() <= STEP) && q.line_ok(end, to) {
            let z = self.ground(to.x, to.y, to.z + 0.5).map_or(to.z, |h| h.z);
            pts.push(Vec3::new(to.x, to.y, z));
        }
        if pts.len() == 1 {
            return Some(Vec::new());
        }
        Some(q.smooth(&pts))
    }

    /// Diagnostics: for each closed door, its link (centre, passage direction, floor),
    /// the grid nodes either side with their components, and the links it made.
    pub fn door_link_report(&self) -> Vec<String> {
        let nav = self.nav();
        self.door_links
            .iter()
            .map(|link| {
                let plus = nav.side_nodes(link.centre.xy() + link.normal * DOOR_SIDES[1], link.floor, DOOR_REACH);
                let minus = nav.side_nodes(link.centre.xy() - link.normal * DOOR_SIDES[1], link.floor, DOOR_REACH);
                let made = nav.link_adj.values().flatten().filter(|&&(_, m)| m == link.mesh).count() / 2;
                let comps = |v: &[u32]| -> Vec<u32> {
                    let mut c: Vec<u32> = v.iter().map(|&n| nav.comp[n as usize]).collect();
                    c.sort_unstable();
                    c.dedup();
                    c
                };
                format!(
                    "door mesh {} centre ({:.2}, {:.2}, {:.2}) normal ({:.2}, {:.2}) floor {:.2}: {} nodes plus in components {:?}, {} minus in components {:?}, {} links",
                    link.mesh,
                    link.centre.x,
                    link.centre.y,
                    link.centre.z,
                    link.normal.x,
                    link.normal.y,
                    link.floor,
                    plus.len(),
                    comps(&plus),
                    minus.len(),
                    comps(&minus),
                    made
                )
            })
            .collect()
    }

    /// Diagnostics: the walk component of a grid node within 1.2 m of `p` at its height.
    pub fn component_at(&self, p: Vec3) -> Option<u32> {
        let nav = self.nav();
        nav.side_nodes(p.xy(), p.z, 1.2).first().map(|&n| nav.comp[n as usize])
    }

    /// True when `to` can be reached from `from` (same component, ignoring
    /// doors and placeables). Cheap. Components join edges clear of walls by
    /// the smallest radius, so a larger radius can still refuse such a route.
    pub fn same_region(&self, from: Vec3, to: Vec3) -> bool {
        let nav = self.nav();
        let q = Query { nav, wm: self, r: R_MIN, locked_blocks: false };
        q.endpoints(from, to).is_some()
    }
}

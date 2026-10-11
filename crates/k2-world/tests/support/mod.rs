//! Test driver for the control API: a World with its control channel, stepped
//! by the test. The game steps its own world once per frame and polls the
//! control channel the same way; this is that loop for test code. The two
//! host jobs the game does in its own way (loading a save, starting a new
//! game) are done here too. Not shipped: only the tests use it.

#![allow(dead_code)] // each test binary uses a different part of it

use std::io::Write;
use std::path::{Path, PathBuf};

use glam::{Vec2, Vec3};
use k2_res::Install;
use k2_world::control::{Command, Control, Env, Nearby, Observation, ObserveArgs, Reply};
use k2_world::save;
use k2_world::{ObjectKind, World};

/// Game seconds per step (30 Hz), as the game's frame.
pub const DT: f32 = 1.0 / 30.0;

/// The install under test: K2_INSTALL, else the repo inside the game folder. Returns
/// None (a skip, with a line on stderr) only when K2_INSTALL is unset and no install
/// is found there; a K2_INSTALL that is not an install is a failure.
pub fn install() -> Option<Install> {
    let env = std::env::var("K2_INSTALL").ok();
    let dir = env.clone().unwrap_or_else(|| concat!(env!("CARGO_MANIFEST_DIR"), "/../../..").into());
    match Install::discover(&dir) {
        Ok(install) => Some(install),
        Err(e) if env.is_some() => panic!("K2_INSTALL={dir} is not an install: {e}"),
        Err(e) => {
            let _ = writeln!(std::io::stderr(), "SKIP: no install at {dir} (set K2_INSTALL): {e}");
            None
        }
    }
}

/// Eight points round `centre` at `radius`, one every 45 degrees from +x.
pub fn ring(centre: Vec2, radius: f32) -> [Vec2; 8] {
    std::array::from_fn(|k| {
        let t = k as f32 * std::f32::consts::FRAC_PI_4;
        centre + Vec2::new(t.cos(), t.sin()) * radius
    })
}

/// Proper crossing of the open segments p0-p1 and q0-q1.
pub fn crosses(p0: Vec2, p1: Vec2, q0: Vec2, q1: Vec2) -> bool {
    let d1 = (p1 - p0).perp_dot(q0 - p0);
    let d2 = (p1 - p0).perp_dot(q1 - p0);
    let d3 = (q1 - q0).perp_dot(p0 - q0);
    let d4 = (q1 - q0).perp_dot(p1 - q0);
    d1 * d2 < 0.0 && d3 * d4 < 0.0
}

/// The closest point of the segment a-b to p.
pub fn closest_on_seg(p: Vec2, a: Vec2, b: Vec2) -> Vec2 {
    let ab = b - a;
    let len2 = ab.length_squared();
    let t = if len2 > 0.0 { ((p - a).dot(ab) / len2).clamp(0.0, 1.0) } else { 0.0 };
    a + ab * t
}

/// The distance from `p` to the segment `a`-`b`.
pub fn seg_dist(p: Vec2, a: Vec2, b: Vec2) -> f32 {
    let ab = b - a;
    let len2 = ab.length_squared();
    let t = if len2 > 0.0 { ((p - a).dot(ab) / len2).clamp(0.0, 1.0) } else { 0.0 };
    p.distance(a + ab * t)
}

/// The largest move the leader makes between two polls: a run at RUN_SPEED (5.4 m/s)
/// over one DT is 0.18 m, with a little slack.
pub const MAX_POLL_STEP: f32 = 0.2;

/// Asserts that a walk trace, the leader's position at each poll, never moves more
/// than MAX_POLL_STEP from the position before it, the first counted from `start`.
/// A bigger step is a teleport; the failure names the poll it happens at.
pub fn assert_walk_trace(start: Vec3, trace: &[Vec3]) {
    let mut prev = start;
    for (step, &p) in trace.iter().enumerate() {
        let moved = p.distance(prev);
        assert!(moved <= MAX_POLL_STEP, "poll {step}: the leader moved {moved:.3} m from {prev:?} to {p:?}, more than {MAX_POLL_STEP} m in one poll");
        prev = p;
    }
}

pub struct Session {
    install: Install,
    save_root: PathBuf,
    pub world: World,
    control: Control,
}

impl Session {
    /// An empty session (no module loaded). Saves go to `save_root`.
    pub fn new(install: Install, save_root: PathBuf) -> Result<Self, String> {
        let world = World::new(install.clone()).map_err(|e| e.to_string())?;
        Ok(Self { install, save_root, world, control: Control::default() })
    }

    pub fn save_root(&self) -> &Path {
        &self.save_root
    }

    /// Runs one command to completion: goals step the world until they answer.
    pub fn execute(&mut self, cmd: Command) -> Reply {
        self.execute_traced(cmd).0
    }

    /// As [`Session::execute`], and the leader's position after each world step
    /// the command took (none for a command that answers at once).
    pub fn execute_traced(&mut self, cmd: Command) -> (Reply, Vec<Vec3>) {
        if let Command::Load(a) = &cmd {
            return (self.load(&a.slot), Vec::new());
        }
        let env = Env { save_root: &self.save_root, paused: false, movie: false };
        let first = self.control.submit(&mut self.world, &env, cmd);
        let mut trace = Vec::new();
        let reply = match first {
            Some(reply) => reply,
            None => loop {
                if let Err(e) = self.step() {
                    self.control = Control::default();
                    break Reply::err(e);
                }
                let env = Env { save_root: &self.save_root, paused: false, movie: false };
                let done = self.control.poll(&mut self.world, &env);
                // After the poll: a step that a goal makes (a walk into reach) is in the trace.
                trace.push(self.leader_position());
                if let Some(reply) = done {
                    break reply;
                }
            },
        };
        match self.apply_pending() {
            Ok(()) => (reply, trace),
            Err(e) => (Reply { ok: false, message: format!("{}; {e}", reply.message), ..reply }, trace),
        }
    }

    /// The party leader's position (zero with no leader).
    pub fn leader_position(&self) -> Vec3 {
        self.world.get(self.world.party_leader()).map_or(Vec3::ZERO, |o| o.position)
    }

    /// Runs the world for `seconds` of game time with no goal, as the game's
    /// frames would (module changes are applied as the game applies them).
    pub fn advance(&mut self, seconds: f32) {
        let steps = (seconds / DT).round() as usize;
        for _ in 0..steps {
            self.step().expect("the world steps");
        }
    }

    /// Starts a new game the way the game's New Game does, with the Quick
    /// Guardian defaults and the given name, in `module`.
    pub fn new_game(&mut self, module: &str, name: &str) -> Result<(), String> {
        let mut w = World::new(self.install.clone()).map_err(|e| e.to_string())?;
        let mut spec = w.quick_spec(0, 3, None);
        spec.name = name.to_owned();
        w.check_spec(&spec)?;
        w.player_spec = spec;
        w.enter_module(module, None).map_err(|e| format!("cannot enter {module}: {e}"))?;
        self.world = w;
        self.control = Control::default();
        Ok(())
    }

    fn step(&mut self) -> Result<(), String> {
        self.world.tick(DT);
        let _ = self.world.drain_events();
        self.apply_pending()
    }

    /// Enters a module the world asked for (a door or trigger), as the game
    /// does after its fade.
    fn apply_pending(&mut self) -> Result<(), String> {
        let Some(t) = self.world.pending_transition.take() else { return Ok(()) };
        self.world.enter_module(&t.module, t.waypoint_tag.as_deref()).map_err(|e| format!("cannot enter {}: {e}", t.module))
    }

    fn load(&mut self, slot: &str) -> Reply {
        let data = match save::read_slot(&self.save_root, slot) {
            Ok(d) => d,
            Err(e) => return Reply::err(format!("cannot read slot {slot}: {e}")),
        };
        match World::load(self.install.clone(), &data) {
            Ok(w) => {
                self.world = w;
                self.control = Control::default();
                Reply::ok(format!("loaded {slot}"))
            }
            Err(e) => Reply::err(format!("cannot load {slot}: {e}")),
        }
    }
}

/// Ring radii (metres), smallest first, for putting the leader beside an object:
/// each is past the reach of the objects the walk tests use.
pub const BESIDE: [f32; 3] = [4.0, 5.0, 6.0];

/// Puts the leader on walk ground from which a walk reaches `anchor` (a stand
/// within use reach), on the first ring of `radii` that has such a point, and
/// returns the leader's new position. The walk from the settled 101per spot does
/// not reach the rest of the ship (the medbay door is shut), so the walk tests
/// start the leader beside the object first.
pub fn leader_beside(s: &mut Session, anchor: u32, radii: &[f32]) -> Vec3 {
    let c = s.world.get(anchor).expect("the anchor is in the world").position;
    for &r in radii {
        for p in ring(Vec2::new(c.x, c.y), r) {
            let Some(h) = s.world.walk.ground(p.x, p.y, c.z + 1.0) else { continue };
            let at = Vec3::new(p.x, p.y, h.z);
            if s.world.use_point(at, anchor).is_some() {
                let leader = s.world.party_leader();
                s.world.place(leader, at);
                return at;
            }
        }
    }
    panic!("no walk ground {radii:?} m round {c:?} has a stand within use reach of it");
}

/// Puts the leader beside a plain placeable (usable, no script on use, no lock,
/// no conversation), nearest first, from which its walk to the stand is clear of
/// doors. Returns the placeable. The walk tests need open floor: a route through
/// a shut door is a block that the route does not show.
pub fn beside_open_placeable(s: &mut Session) -> u32 {
    let r = s.execute(Command::Observe(ObserveArgs { radius: Some(100.0) }));
    assert!(r.ok, "{}", r.message);
    let o: Observation = serde_json::from_value(r.data.expect("observe data")).expect("observation");
    let mut plain: Vec<&Nearby> = o
        .nearby
        .iter()
        .filter(|n| n.kind == "placeable" && n.useable && !n.scripted_use && !n.conversation && n.locked != Some(true))
        .collect();
    plain.sort_by(|a, b| a.distance.total_cmp(&b.distance));
    for n in &plain {
        if try_beside(s, n.id, &BESIDE).is_some() {
            return n.id;
        }
    }
    panic!("none of the {} plain placeables has open walk ground beside it", plain.len());
}

/// As `leader_beside`, but None when no ring point has a stand whose walk is clear
/// of doors. The leader moves only on success.
pub fn try_beside(s: &mut Session, anchor: u32, radii: &[f32]) -> Option<Vec3> {
    let c = s.world.get(anchor)?.position;
    for &r in radii {
        for p in ring(Vec2::new(c.x, c.y), r) {
            let Some(h) = s.world.walk.ground(p.x, p.y, c.z + 1.0) else { continue };
            let at = Vec3::new(p.x, p.y, h.z);
            let Some(stand) = s.world.use_point(at, anchor) else { continue };
            if route_clear_of_doors(&s.world, at, stand) {
                let leader = s.world.party_leader();
                s.world.place(leader, at);
                return Some(at);
            }
        }
    }
    None
}

/// The route `find_path` gives from `from` to `to` passes no door: no door object
/// stands within 1.5 m of any leg of it.
pub fn route_clear_of_doors(w: &World, from: Vec3, to: Vec3) -> bool {
    let Some(route) = w.walk.find_path(from, to, leader_radius(w)) else { return false };
    let mut legs = vec![from];
    legs.extend(route);
    let doors: Vec<Vec2> = w.iter().filter(|o| o.kind == ObjectKind::Door).map(|o| o.position.truncate()).collect();
    doors.iter().all(|&d| legs.windows(2).all(|leg| seg_dist(d, leg[0].truncate(), leg[1].truncate()) > 1.5))
}

/// The clearance the leader's walks use (World::leader_clearance).
pub fn leader_radius(w: &World) -> f32 {
    w.leader_clearance()
}

/// A walkable point `radius` metres from `from` that a walk from `from` ends at
/// (within 0.6 m of it), or None.
pub fn walkable_point_at(w: &World, from: Vec3, radius: f32) -> Option<Vec3> {
    (0..16).find_map(|k| {
        let a = k as f32 * std::f32::consts::TAU / 16.0;
        let q = Vec2::new(from.x, from.y) + Vec2::new(a.cos(), a.sin()) * radius;
        let to = Vec3::new(q.x, q.y, w.walk.ground(q.x, q.y, from.z + 1.0)?.z);
        let end = *w.walk.find_path(from, to, leader_radius(w))?.last()?;
        (end.truncate().distance(to.truncate()) <= 0.6 && route_clear_of_doors(w, from, to)).then_some(to)
    })
}

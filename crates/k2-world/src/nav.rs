//! Path following on top of k2-walk's `find_path`, plus party follow and the
//! follow actions (ActionFollowLeader / ActionFollowOwner).
//!
//! Every scripted move (MoveTo, MoveAway, combat approach) goes through
//! `World::move_towards`, which steers toward the next waypoint of a cached
//! path. Paths are recomputed when the destination moves more than
//! `REPATH_MOVE`, when the mover gets blocked, or when it drifts off its path.
//! With no path the mover walks straight at the target for `FAIL_GRACE`
//! seconds, then stops, so the caller's stuck detection ends the action.

use std::collections::HashMap;

use glam::{Vec2, Vec3, Vec3Swizzles};
use k2_script::ObjectId;

use k2_walk::{FULL_BUDGET, SpotSearch, SpotStep};
use crate::object::{Action, FollowTarget, bearing_towards};
use crate::tick::Motion;
use crate::World;
use serde::{Deserialize, Serialize};

/// Destination change that triggers a new path.
const REPATH_MOVE: f32 = 1.0;
/// Shortest time between two path searches for one mover.
const REPATH_MIN: f32 = 0.25;
/// Waypoint counts as reached within this distance.
const WAYPOINT_REACHED: f32 = 0.35;
/// A route ending this close to its destination ends at it (see `steer`).
const AT_DEST: f32 = 0.5;
/// A steering search takes the goal nearest its destination. Each metre a goal
/// lies beyond AT_DEST adds this many metres of route cost (the exit cost of
/// `SpotSearch`): a goal 1 cm beyond AT_DEST costs 10 m more than one within
/// it, so it wins only when that one's route is more than 10 m longer.
const STEER_GAP_WEIGHT: f32 = 1000.0;
/// Distance from the current path segment that counts as drifted off.
const DRIFT: f32 = 2.0;
/// Straight-line attempts after a failed search before giving up.
const FAIL_GRACE: f32 = 2.0;
/// Party follow: start following beyond, stop within, run beyond.
const FOLLOW_START: f32 = 3.0;
/// Also the default stop distance of ActionFollowLeader.
pub(crate) const FOLLOW_STOP: f32 = 2.0;
const FOLLOW_RUN: f32 = 6.0;
/// Follow arrival: the two bodies' radii plus this margin, when the range is tighter.
const FOLLOW_MARGIN: f32 = 0.25;
/// A follower that makes no progress for this long stops trying...
const FOLLOW_STUCK: f32 = 1.5;
/// ...until the target is this much farther than where it gave up.
const FOLLOW_RESUME: f32 = 1.0;
/// Use spots (`World::use_candidates`): rings round the object at these
/// fractions of its use reach, with this many points each; the walker's radius;
/// and the margin kept inside reach so the client's own check passes at the end.
const USE_RINGS: [f32; 3] = [0.9, 0.6, 0.3];
const USE_RING_POINTS: usize = 16;
/// Nodes one click-walk frame may expand: one step of the use search
/// (`step_use_search`), and one step of the route search of a steering repath
/// (`steer_leader`). About 1 to 3 ms on 201tel without load. k2-walk's full
/// budget (400k, about 30 ms on a failing search) caps a whole search.
pub(crate) const USE_EXPAND: usize = 20_000;
/// Frames a use search may take in `use_point`: k2-walk's full budget is 20 frames of `USE_EXPAND`,
/// so the search ends well before this cap; the cap keeps `use_point`'s loop finite regardless.
const USE_STEP_CAP: usize = 64;
const USE_MARGIN: f32 = 0.1;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct PathState {
    dest: Vec3,
    points: Vec<Vec3>,
    next: usize,
    /// Seconds until a new search is allowed.
    cooldown: f32,
    /// Set when the last search found nothing; seconds since.
    failed_for: Option<f32>,
    /// Blocked last frame: search again as soon as allowed.
    blocked: bool,
    /// Follows: seconds without progress toward the target (FOLLOW_STUCK).
    /// A latch, not game state: never saved.
    #[serde(skip)]
    follow_stuck: f32,
    /// Follows: the distance at which the follower gave up, until the target
    /// is FOLLOW_RESUME farther. A latch, not game state: never saved.
    #[serde(skip)]
    follow_held: Option<f32>,
    /// The route search of a bounded steer (`steer`), stepped a frame at a
    /// time. A latch, not game state: never saved.
    #[serde(skip)]
    search: Option<SpotSearch>,
}

/// Per-mover path cache.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Paths {
    states: HashMap<ObjectId, PathState>,
}

impl Paths {
    pub fn clear(&mut self) {
        self.states.clear();
    }

    pub fn forget(&mut self, id: ObjectId) {
        self.states.remove(&id);
    }

    /// Remaining waypoints for `id` (for debug drawing).
    pub fn waypoints(&self, id: ObjectId) -> &[Vec3] {
        self.states.get(&id).map_or(&[], |s| &s.points[s.next.min(s.points.len())..])
    }

    /// Seconds since the last path search for `id` failed, if it did.
    pub fn failed_for(&self, id: ObjectId) -> Option<f32> {
        self.states.get(&id).and_then(|s| s.failed_for)
    }

    /// True when `pos` has reached the last point of the route for `id` and
    /// that point is not the destination (the destination itself is unreachable
    /// from it, e.g. inside a placeable's footprint).
    pub(crate) fn at_end(&self, id: ObjectId, pos: Vec3) -> bool {
        self.states.get(&id).is_some_and(|s| {
            let Some(&end) = s.points.last() else { return false };
            s.failed_for.is_none() && s.next + 1 >= s.points.len() && end.xy().distance(pos.xy()) <= WAYPOINT_REACHED && end.xy().distance(s.dest.xy()) > AT_DEST
        })
    }

    /// Advances cooldowns and failure timers.
    pub(crate) fn tick(&mut self, dt: f32) {
        for s in self.states.values_mut() {
            s.cooldown -= dt;
            if let Some(f) = &mut s.failed_for {
                *f += dt;
            }
        }
    }
}

/// What one frame of a use search found.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum UseStep {
    /// The frame's budget ran out with the search open; the next frame steps on.
    Pending,
    /// The walk ends at this spot, within use reach.
    Stand(Vec3),
    /// No spot within use reach is reachable from here.
    NoStand,
}

impl World {
    /// Point to steer at this frame on the way from `pos` to `dest`, or None
    /// to stand still (search failed and the grace period ran out). With a
    /// `budget` the route is a `SpotSearch` stepped `budget` expansions a frame
    /// (the unsmoothed node chain): the mover stands still until it has a
    /// route, and a search that uses up the full budget fails. Without one the
    /// route is k2-walk's `find_path`, in one call.
    pub(crate) fn steer(&mut self, id: ObjectId, pos: Vec3, dest: Vec3, radius: f32, budget: Option<usize>) -> Option<Vec3> {
        if self.walk.face_count() == 0 {
            return Some(dest);
        }
        let st = self.paths.states.entry(id).or_default();
        // The first search for this mover: no route, no search running, no failure yet.
        let first = st.points.is_empty() && st.failed_for.is_none() && st.search.is_none();
        let dest_moved = first || st.dest.xy().distance(dest.xy()) > REPATH_MOVE;
        let drifted = st.points.get(st.next).is_some_and(|&w| {
            let a = if st.next == 0 { pos } else { st.points[st.next - 1] };
            dist_to_seg(pos.xy(), a.xy(), w.xy()) > DRIFT
        });
        let want = dest_moved || drifted || st.blocked;
        if want && (st.cooldown <= 0.0 || dest_moved && st.points.is_empty()) {
            st.cooldown = REPATH_MIN;
            st.blocked = false;
            st.dest = dest;
            st.next = 0;
            st.search = None;
            match budget {
                Some(_) => match self.walk.spot_search(pos, &[dest], radius, FULL_BUDGET, |p| {
                    let gap = p.xy().distance(dest.xy());
                    Some((gap - AT_DEST).max(0.0) * STEER_GAP_WEIGHT + gap)
                }) {
                    Some(search) => {
                        // The old route goes: the mover stands still until the new one is found.
                        // A failure timer from an earlier search must not end this one (FAIL_GRACE).
                        st.points.clear();
                        st.failed_for = None;
                        st.search = Some(search);
                        tracing::debug!(id, from = ?pos, to = ?dest, "route search started");
                    }
                    None => {
                        st.points.clear();
                        st.failed_for.get_or_insert(0.0);
                        tracing::debug!(id, from = ?pos, to = ?dest, "route search: no start spot");
                    }
                },
                None => match self.walk.find_path_avoiding_locked(pos, dest, radius) {
                    Some(p) => {
                        st.points = p;
                        st.failed_for = None;
                    }
                    None => {
                        st.points.clear();
                        st.failed_for.get_or_insert(0.0);
                    }
                },
            }
        }
        // One step of the running search a frame, the frame it starts included.
        if let Some(max) = budget
            && let Some(search) = st.search.as_mut()
        {
            match self.walk.spot_step(search, max) {
                SpotStep::Pending => {}
                SpotStep::Found(route) => {
                    tracing::debug!(id, nodes = route.len(), "route search found a route");
                    st.points = route;
                    st.failed_for = None;
                    st.search = None;
                }
                SpotStep::Miss(_) => {
                    tracing::debug!(id, "route search missed: no route");
                    st.points.clear();
                    st.failed_for.get_or_insert(0.0);
                    st.search = None;
                }
            }
        }
        if st.failed_for.is_some_and(|f| f > FAIL_GRACE) {
            return None;
        }
        if st.points.is_empty() {
            // A search still running has no route yet: the mover stands still.
            return Some(if st.search.is_some() { pos } else { dest });
        }
        while st.next + 1 < st.points.len() && st.points[st.next].xy().distance(pos.xy()) <= WAYPOINT_REACHED {
            st.next += 1;
        }
        // The final leg aims at the live destination when the route ends at
        // it (the target may shift a little without triggering a repath). A
        // route that ends short, at the nearest reachable spot, aims there.
        let last = st.next + 1 >= st.points.len();
        let end = st.points.last().copied().unwrap_or(dest);
        // The last leg to the live destination is a straight line: taken only when no blocker is on it.
        Some(if last && end.xy().distance(dest.xy()) <= AT_DEST && !self.walk.leg_crosses_blocker(end, dest) { dest } else { st.points[st.next] })
    }

    /// Point the party leader should walk toward next to reach `dest` from
    /// `from`, for a walk the caller drives itself (click-to-move): the next
    /// waypoint of a route around walls and closed doors, or `dest` on a
    /// direct route. The route search runs a step of `route_step` nodes a frame
    /// (`USE_EXPAND` in the game, at least 1), so a long search spreads over frames while the
    /// leader stands still, and no frame runs two searches. None when there is no
    /// route, at once: a failed search does not hand the leader a straight line
    /// toward a wall (the NPC steering keeps that grace, `steer` with no budget).
    pub fn steer_leader(&mut self, from: Vec3, dest: Vec3) -> Option<Vec3> {
        let leader = self.party_leader();
        let aim = self.steer(leader, from, dest, self.leader_clearance(), Some(self.route_step.max(1)))?;
        if self.paths.failed_for(leader).is_some() {
            tracing::debug!(from = ?from, to = ?dest, "leader route failed");
            return None;
        }
        Some(aim)
    }

    /// The clearance the leader keeps from walls and creatures: its collision
    /// radius, at most 0.5. Routes, the stand search and the walk's movement all
    /// use this one value, so a route runs only where the walk can go.
    pub fn leader_clearance(&self) -> f32 {
        self.get(self.party_leader()).and_then(|o| o.creature()).map_or(0.3, |c| c.radius.min(0.5))
    }

    /// One frame of the leader's walk toward `aim` (a point of its route, from
    /// `steer_leader`): it moves `dt` seconds at walk speed (run speed when
    /// `run`) along the walkmesh, sliding round walls and living creatures as
    /// the player's own walk does (`player_move`). Returns the distance moved;
    /// the caller decides what the walk has achieved.
    pub fn walk_leader_toward(&mut self, aim: Vec3, dt: f32, run: bool) -> f32 {
        let leader = self.party_leader();
        let Some((pos, speed)) = self.get(leader).and_then(|o| o.creature().map(|c| (o.position, if run { c.run_speed } else { c.walk_speed }))) else {
            return 0.0;
        };
        let to = (aim - pos).truncate();
        let len = to.length();
        if len < 1e-4 || dt <= 0.0 {
            return 0.0;
        }
        let step = (speed * self.speed_factor(leader) * dt).min(len);
        let new = self.player_move(pos, to / len * step, self.leader_clearance());
        let moved = (new - pos).truncate().length();
        tracing::debug!(from = ?pos, aim = ?aim, to = ?new, moved, "leader step");
        if let Some(o) = self.get_mut(leader) {
            o.position = new;
            if let Some(b) = bearing_towards(pos, aim) {
                o.bearing = b;
            }
        }
        self.motion.insert(leader, if run { Motion::Run } else { Motion::Walk });
        moved
    }

    /// True while the leader's route search runs: `steer_leader` has no route
    /// yet, and the leader stands still meanwhile (not walking).
    pub fn leader_route_pending(&self) -> bool {
        let leader = self.party_leader();
        self.paths.states.get(&leader).is_some_and(|s| s.search.is_some())
    }

    /// Drops the leader's cached route, so the next `steer_leader` searches
    /// afresh (a new click, or a route that failed before).
    pub fn forget_leader_path(&mut self) {
        let leader = self.party_leader();
        self.paths.forget(leader);
    }

    /// True when `from` is within use reach of `target` (`control::reach_of`,
    /// the same test the client's interact uses).
    pub fn in_use_reach(&self, from: Vec3, target: ObjectId) -> bool {
        self.get(target).is_some_and(|t| from.truncate().distance(t.position.truncate()) <= crate::control::reach_of(self, target))
    }

    /// Starts the search for a spot within use reach of `target` from `from`:
    /// one route search whose goals are the candidate spots (`use_candidates`),
    /// ending at the first goal reached within use reach. It runs a frame at a
    /// time (`step_use_search`), so a click costs one search spread over frames.
    /// None when there is no candidate spot or `from` is off the walkmesh.
    pub fn begin_use_search(&self, from: Vec3, target: ObjectId) -> Option<SpotSearch> {
        self.begin_use_search_within(from, target, USE_MARGIN)
    }

    /// `begin_use_search` with the stand kept `margin` metres inside use reach:
    /// a walk that stops within `margin` of its stand is within reach.
    pub fn begin_use_search_within(&self, from: Vec3, target: ObjectId, margin: f32) -> Option<SpotSearch> {
        let t = self.get(target)?.position;
        let within = crate::control::reach_of(self, target) - margin;
        let spots = self.use_candidates(from, target);
        self.walk.spot_search(from, &spots, self.leader_clearance(), FULL_BUDGET, |p| (p.truncate().distance(t.truncate()) <= within).then_some(0.0))
    }

    /// One frame of a use search from `begin_use_search`: expands up to
    /// `USE_EXPAND` nodes, then reports `Pending` until the search ends.
    pub fn step_use_search(&self, search: &mut SpotSearch) -> UseStep {
        match self.walk.spot_step(search, USE_EXPAND) {
            SpotStep::Pending => UseStep::Pending,
            SpotStep::Found(route) => match route.last() {
                Some(&at) => UseStep::Stand(at),
                None => UseStep::NoStand,
            },
            SpotStep::Miss(_) => UseStep::NoStand,
        }
    }

    /// Where a walk to `target` from `from` should end: a spot within use
    /// reach, found by `begin_use_search` run to its end. None when no spot is
    /// reached.
    pub fn use_point(&self, from: Vec3, target: ObjectId) -> Option<Vec3> {
        if self.in_use_reach(from, target) {
            return Some(from);
        }
        let mut search = self.begin_use_search(from, target)?;
        for _ in 0..USE_STEP_CAP {
            match self.step_use_search(&mut search) {
                UseStep::Pending => {}
                UseStep::Stand(at) => return Some(at),
                UseStep::NoStand => return None,
            }
        }
        None
    }

    /// Every candidate spot round `target`, nearest `from` first; the use search
    /// takes them all as goals. A route to the object's own origin can end short
    /// of reach on the far side of its footprint, so spots on rings round the
    /// object are used. Spots on another walkmesh island are left out (they are
    /// not reachable from `from`).
    pub fn use_candidates(&self, from: Vec3, target: ObjectId) -> Vec<Vec3> {
        let Some(t) = self.get(target).map(|o| o.position) else { return Vec::new() };
        let reach = crate::control::reach_of(self, target);
        let mut spots: Vec<(f32, Vec3)> = Vec::with_capacity(USE_RINGS.len() * USE_RING_POINTS);
        for frac in USE_RINGS {
            for k in 0..USE_RING_POINTS {
                let a = k as f32 * std::f32::consts::TAU / USE_RING_POINTS as f32;
                let q = t + Vec3::new(a.cos(), a.sin(), 0.0) * frac * reach;
                // The floor the walker stands on, not a surface stacked on the object (its own top).
                if let Some(h) = self.walk.ground_nearest(q.x, q.y, from.z) {
                    spots.push((from.truncate().distance(q.truncate()), Vec3::new(q.x, q.y, h.z)));
                }
            }
        }
        spots.sort_by(|a, b| a.0.total_cmp(&b.0));
        spots.into_iter().map(|(_, spot)| spot).filter(|&spot| self.walk.same_region(from, spot)).collect()
    }

    /// The engine's "Unable to reach target!" (dialog.tlk 1516) for a walk
    /// that cannot get within use reach.
    pub fn report_unreachable(&mut self) {
        let text = self.talk_str(1516);
        self.feedback(text, Some("Error_Default"));
    }

    /// Tells the path cache whether `id` made progress this frame.
    pub(crate) fn note_progress(&mut self, id: ObjectId, moved: f32, wanted: f32) {
        if let Some(st) = self.paths.states.get_mut(&id) {
            st.blocked = wanted > 1e-4 && moved < wanted * 0.2;
        }
    }

    /// Queues a follow action, unless a follow of the same target is already
    /// queued: a heartbeat that repeats the call must not stack entries. The
    /// range of the queued entry is kept. A leader follow only counts when it
    /// is the last entry: one queued before a wait yields to it and must not
    /// block a follow that should resume after the wait.
    ///
    /// Refused: a follow of the follower itself, and any follow but
    /// ActionFollowLeader on the party leader. Those would hold control until
    /// ClearAllActions, and the leader's own follow of itself completes at once.
    pub(crate) fn queue_follow(&mut self, id: ObjectId, of: FollowTarget, range: f32) {
        let target = match of {
            FollowTarget::Leader => Some(self.party_leader()),
            FollowTarget::Owner => self.get(id).and_then(|o| o.owner),
            FollowTarget::Object(t) => Some(t),
        };
        if target == Some(id) || (of != FollowTarget::Leader && id == self.party_leader()) {
            return;
        }
        let queued = self.get(id).is_some_and(|o| match of {
            FollowTarget::Leader => matches!(o.actions.back(), Some(Action::Follow { of: q, .. }) if *q == of),
            _ => o.actions.iter().any(|a| matches!(a, Action::Follow { of: q, .. } if *q == of)),
        });
        if !queued {
            // A new entry starts with no hold left over from an earlier one
            // (ClearAllActions and the leader switches drop entries, not latches).
            if let Some(st) = self.paths.states.get_mut(&id) {
                st.follow_stuck = 0.0;
                st.follow_held = None;
            }
            self.push_action(id, Action::Follow { of, range });
        }
    }

    /// One frame of a follow action: walks after `target` until within the
    /// arrival distance. That is `range`, or the two bodies' radii plus
    /// FOLLOW_MARGIN when the range is tighter: creature collision holds a
    /// walker about 0.7 m off, so a 0.0 range would never complete. A follower
    /// that makes no progress for FOLLOW_STUCK seconds stands until the target
    /// is FOLLOW_RESUME m farther than where it gave up.
    pub(crate) fn follow_towards(&mut self, id: ObjectId, target: ObjectId, range: f32, dt: f32) {
        let (Some(me), Some(lp)) = (self.get(id).map(|o| o.position), self.get(target).map(|o| o.position)) else { return };
        let d = (lp - me).truncate().length();
        let arrive = range.max(self.body_radius(id) + self.body_radius(target) + FOLLOW_MARGIN);
        if d <= arrive {
            self.paths.forget(id);
            return;
        }
        if self.paths.states.get(&id).and_then(|s| s.follow_held).is_some_and(|h| d <= h + FOLLOW_RESUME) {
            return;
        }
        let (moved, _) = self.move_towards(id, lp, d > FOLLOW_RUN, arrive, dt);
        let st = self.paths.states.entry(id).or_default();
        st.follow_held = None;
        st.follow_stuck = if moved < 1e-3 { st.follow_stuck + dt } else { 0.0 };
        if st.follow_stuck > FOLLOW_STUCK {
            st.follow_stuck = 0.0;
            st.follow_held = Some(d);
        }
    }

    /// Collision radius of a creature; 0.3 m for anything else.
    fn body_radius(&self, id: ObjectId) -> f32 {
        self.get(id).and_then(|o| o.creature()).map_or(0.3, |c| c.radius)
    }

    /// Non-leader party members walk after the leader when idle.
    pub(crate) fn party_follow(&mut self, dt: f32) {
        if self.conversation.is_some() || self.solo_mode || self.party.len() < 2 {
            return;
        }
        let leader = self.party[0];
        let Some(lp) = self.get(leader).map(|o| o.position) else { return };
        let members: Vec<ObjectId> = self.party[1..].to_vec();
        for m in members {
            let Some(o) = self.get(m) else { continue };
            if !o.actions.is_empty() || o.creature().is_none_or(|c| c.dead || c.down) || self.in_combat(m) || self.motion.contains_key(&m) {
                continue;
            }
            let d = (lp - o.position).truncate().length();
            let following = self.paths.states.get(&m).is_some_and(|s| !s.points.is_empty() && s.failed_for.is_none());
            if d > FOLLOW_START || following && d > FOLLOW_STOP {
                let (_, arrived) = self.move_towards(m, lp, d > FOLLOW_RUN, FOLLOW_STOP, dt);
                if arrived {
                    self.paths.forget(m);
                }
            } else if following {
                self.paths.forget(m);
            }
        }
    }
}

fn dist_to_seg(p: Vec2, a: Vec2, b: Vec2) -> f32 {
    let ab = b - a;
    let l2 = ab.length_squared();
    let t = if l2 < 1e-12 { 0.0 } else { ((p - a).dot(ab) / l2).clamp(0.0, 1.0) };
    p.distance(a + ab * t)
}

#[cfg(test)]
mod tests;

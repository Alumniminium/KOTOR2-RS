//! Game over: when the last standing party member goes down (knocked out or
//! dead), the original gameover screen comes up after the death clip and offers
//! the newest save, the load list or the main menu. A fallen leader hands
//! control to the next standing member in the world itself (party.rs
//! party_member_down, called from combat.rs), so this only shows when the whole
//! party is down.

use bevy::prelude::*;
use k2_world::{WorldEvent, save};

use crate::game::{AppState, FrameEvents, GameSet, WorldRes};
use crate::gui::{GuiClick, GuiScreen, GuiSet, Placement, open_screen};
use crate::model::GameRes;
use crate::saves::{PendingLoad, SaveRoot};

/// Real seconds between the last knock-out or death and the screen (the death clip).
const DELAY: f32 = 3.0;

pub struct GameOverPlugin;

impl Plugin for GameOverPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<GameOver>()
            .add_systems(Update, (kill_leader_hook, kill_hostiles_hook, near_hostile_hook, combat_log, trigger, show, auto_click, release).chain().in_set(GameSet::Ui))
            .add_systems(Update, clicks.in_set(GuiSet::Logic).run_if(in_state(AppState::Game)));
    }
}

#[derive(Resource, Default)]
pub(crate) struct GameOver {
    /// Seconds until the screen opens.
    due: Option<f32>,
    screen: Option<Entity>,
}

impl GameOver {
    /// Whether the game over screen is due or up (the party is down).
    pub(crate) fn is_open(&self) -> bool {
        self.due.is_some() || self.screen.is_some()
    }
}

/// `K2_KILL_LEADER=SECS`: the leader dies SECS seconds after the area loads
/// (test hook for the game over flow).
fn kill_leader_hook(time: Res<Time<Real>>, mut world: ResMut<WorldRes>, mut t: Local<f32>, mut done: Local<bool>) {
    let Some(at) = std::env::var("K2_KILL_LEADER").ok().and_then(|v| v.parse::<f32>().ok()) else { return };
    if *done || world.0.module.is_none() {
        return;
    }
    *t += time.delta_secs();
    if *t >= at {
        *done = true;
        let leader = world.0.party_leader();
        info!("K2_KILL_LEADER: killing {}", world.0.describe(leader));
        world.0.hurt_creature(leader, i32::MAX / 2, k2_script::OBJECT_INVALID);
    }
}

/// `K2_KILL_HOSTILES=SECS`: every living hostile dies SECS seconds after the
/// area loads, the leader being the killer (test hook for combat ending).
fn kill_hostiles_hook(time: Res<Time<Real>>, mut world: ResMut<WorldRes>, mut t: Local<f32>, mut done: Local<bool>) {
    let Some(at) = std::env::var("K2_KILL_HOSTILES").ok().and_then(|v| v.parse::<f32>().ok()) else { return };
    if *done || world.0.module.is_none() {
        return;
    }
    *t += time.delta_secs();
    if *t < at {
        return;
    }
    *done = true;
    let w = &mut world.0;
    let leader = w.party_leader();
    let ids: Vec<k2_script::ObjectId> = w.iter().filter(|o| o.id != leader && o.creature().is_some_and(|c| !c.dead)).map(|o| o.id).collect();
    let mut n = 0;
    for id in ids {
        if w.is_enemy(id, leader) {
            w.hurt_creature(id, 10_000, leader);
            n += 1;
        }
    }
    info!("K2_KILL_HOSTILES: killed {n} hostiles");
}

/// `K2_NEAR_HOSTILE=DIST`: once the area is up, the idle leader is put DIST
/// m from the nearest hostile creature, on open ground with a clear line
/// (test hook: the hostile has to start the fight).
fn near_hostile_hook(time: Res<Time<Real>>, mut world: ResMut<WorldRes>, mut players: Query<&mut crate::play::Player>, mut t: Local<f32>, mut done: Local<bool>) {
    let Some(dist) = std::env::var("K2_NEAR_HOSTILE").ok().and_then(|v| v.parse::<f32>().ok()) else { return };
    let w = &mut world.0;
    if *done || w.module.is_none() {
        return;
    }
    *t += time.delta_secs();
    if *t < 2.0 {
        return;
    }
    *done = true;
    let leader = w.party_leader();
    let me = w.get(leader).map(|o| o.position).unwrap_or_default();
    let ids: Vec<(k2_script::ObjectId, f32)> = w
        .iter()
        .filter(|o| o.id != leader && o.scripts.contains_key("ScriptOnNotice") && o.creature().is_some_and(|c| !c.dead))
        .map(|o| (o.id, o.position.distance(me)))
        .collect();
    let mut hostile: Vec<(k2_script::ObjectId, f32)> = ids.into_iter().filter(|(id, _)| w.is_enemy(*id, leader)).collect();
    hostile.sort_by(|a, b| a.1.total_cmp(&b.1));
    for (id, d) in hostile {
        let tp = w.get(id).map(|o| o.position).unwrap_or_default();
        let spot = (0..36).find_map(|k| {
            let a = k as f32 / 36.0 * std::f32::consts::TAU;
            let q = tp + Vec3::new(a.cos(), a.sin(), 0.0) * dist;
            let h = w.walk.ground(q.x, q.y, tp.z + 1.5)?;
            let q = Vec3::new(q.x, q.y, h.z);
            w.walk.line_of_sight(q + Vec3::Z * 1.5, tp + Vec3::Z * 1.5).then_some(q)
        });
        let Some(q) = spot else { continue };
        info!("K2_NEAR_HOSTILE: {} was {d:.1} m away; leader now {dist} m from it at {q}", w.describe(id));
        if let Some(o) = w.get_mut(leader) {
            o.position = q;
        }
        if let Ok(mut p) = players.single_mut() {
            p.pos = q;
        }
        return;
    }
    warn!("K2_NEAR_HOSTILE: no hostile with open ground around it");
}

/// Logs when the leader enters and leaves combat, with the living hostiles
/// that are in combat at the time (diagnostics for stuck combat state).
fn combat_log(world: Res<WorldRes>, mut was: Local<bool>) {
    let w = &world.0;
    let leader = w.party_leader();
    let now = w.in_combat(leader);
    if now == *was {
        return;
    }
    *was = now;
    let engaged: Vec<String> = w
        .iter()
        .filter(|o| o.id != leader && o.creature().is_some_and(|c| !c.dead) && w.in_combat(o.id))
        .map(|o| w.describe(o.id))
        .collect();
    info!("leader {} combat; creatures still in combat: {engaged:?}", if now { "enters" } else { "leaves" });
}

fn trigger(events: Res<FrameEvents>, world: Res<WorldRes>, mut state: ResMut<GameOver>) {
    let leader = world.0.party_leader();
    for e in &events.0 {
        if let WorldEvent::Damaged { object, amount, by } = e
            && *object == leader
        {
            let hp = world.0.get(leader).and_then(|o| o.creature()).map_or(0, |c| c.hp);
            info!("party leader takes {amount} from {} ({hp} hp left)", world.0.describe(*by));
        }
    }
    if state.screen.is_none() && state.due.is_none() && events.0.iter().any(|e| matches!(e, WorldEvent::PartyDefeated)) {
        info!("party defeated");
        state.due = Some(DELAY);
    }
}

fn show(mut commands: Commands, time: Res<Time<Real>>, game: Res<GameRes>, mut state: ResMut<GameOver>) {
    let Some(t) = state.due.as_mut() else { return };
    *t -= time.delta_secs();
    if *t > 0.0 {
        return;
    }
    state.due = None;
    let Some(e) = open_screen(&mut commands, &game.0, "gameover", Placement::Center, 205) else {
        error!("gui: no gameover layout");
        return;
    };
    commands.entity(e).insert(BackgroundColor(Color::srgba(0.0, 0.0, 0.0, 0.85)));
    state.screen = Some(e);
    commands.queue(move |bw: &mut World| {
        let any_save = !save::list_slots(&bw.resource::<SaveRoot>().0).iter().all(|s| s.incompatible.is_some());
        if let Some(mut scr) = bw.get_mut::<GuiScreen>(e) {
            scr.set_enabled("BTN_LASTSAVE", any_save);
            scr.set_enabled("BTN_LOADGAME", any_save);
        }
    });
}

/// `K2_GAMEOVER_CLICK=BTN_LASTSAVE|BTN_QUIT`: clicks that button as soon as
/// the screen is up (test hook).
fn auto_click(state: Res<GameOver>, mut out: MessageWriter<GuiClick>, mut done: Local<bool>) {
    let Ok(tag) = std::env::var("K2_GAMEOVER_CLICK") else { return };
    if *done || state.screen.is_none() {
        return;
    }
    *done = true;
    info!("K2_GAMEOVER_CLICK: {tag}");
    out.write(GuiClick { screen: "gameover_p".into(), tag, item: None });
}

/// The slot "Load Last Saved Game" loads: the newest of the player's saves
/// (save-*, quicksave, autosave) whose leader is standing; failing that the
/// newest rolling slot (timed, restart) with a standing leader. Slots that
/// cannot be read are skipped.
pub fn last_good_slot(root: &std::path::Path, install: &k2_res::Install) -> Option<String> {
    let slots: Vec<save::SlotInfo> = save::list_slots(root).into_iter().filter(|s| s.incompatible.is_none()).collect();
    let player = |s: &&save::SlotInfo| s.slot == "quicksave" || s.slot == "autosave" || s.slot.starts_with("save-");
    let alive = |s: &save::SlotInfo| {
        let w = save::read_slot(root, &s.slot).and_then(|d| k2_world::World::load(install.clone(), &d)).ok()?;
        let leader = w.get(w.party_leader())?.creature()?;
        let ok = !leader.dead && !leader.down && leader.hp > 0;
        if !ok {
            info!("game over: skipping {} (leader dead or down in that save)", s.slot);
        }
        ok.then(|| s.slot.clone())
    };
    // list_slots is newest first.
    slots.iter().filter(player).find_map(alive).or_else(|| slots.iter().filter(|s| !player(&s)).find_map(alive))
}

/// Takes the screen down once the leader is standing again (a load) or the
/// area is gone (quit to the main menu).
fn release(mut commands: Commands, world: Res<WorldRes>, mut state: ResMut<GameOver>) {
    let Some(e) = state.screen else { return };
    let w = &world.0;
    let leader_alive = w.is_standing(w.party_leader());
    if leader_alive {
        let hp = w.get(w.party_leader()).and_then(|o| o.creature()).map_or(0, |c| c.hp);
        info!("game over cleared: leader {} alive with {hp} hp", w.describe(w.party_leader()));
        if let Ok(mut ec) = commands.get_entity(e) {
            ec.despawn();
        }
        state.screen = None;
    }
}

fn clicks(
    mut commands: Commands,
    mut clicks: MessageReader<GuiClick>,
    root: Res<SaveRoot>,
    game: Res<GameRes>,
    mut load: ResMut<PendingLoad>,
    mut state: ResMut<GameOver>,
    mut next: ResMut<NextState<AppState>>,
) {
    for c in clicks.read() {
        if !c.screen.starts_with("gameover") {
            continue;
        }
        match c.tag.as_str() {
            "BTN_LASTSAVE" => match last_good_slot(&root.0, &game.0.install) {
                Some(slot) => {
                    info!("game over: loading {slot}");
                    load.0 = Some(slot);
                }
                None => warn!("game over: no save with a living leader to load"),
            },
            // BTN_LOADGAME opens the load list: saves.rs.
            "BTN_QUIT" => {
                if let Some(e) = state.screen.take()
                    && let Ok(mut ec) = commands.get_entity(e)
                {
                    ec.despawn();
                }
                next.set(AppState::Menu);
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests;

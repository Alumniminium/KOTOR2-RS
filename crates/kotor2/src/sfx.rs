//! Combat and creature sound effects, resolved from the game's own tables.
//!
//! - Weapons: baseitems.2da `weaponmattype` selects a weaponsounds.2da row
//!   (swingshort/swinglong/swingtwirl, per-armour hit columns, parry, clash);
//!   `ammunitiontype` selects an ammunitiontypes.2da row (shotsound,
//!   impactsound). Creatures without a weapon use appearancesndset.2da
//!   `weapon`, else row 0 (Unarmed).
//! - Hit column: the target's armour (baseitems `armortype` of the body or
//!   creature-armour item), else appearancesndset `armortype`, else leather.
//! - Timing: melee swings and impacts fire on the attack clip's MDL events
//!   (`swingshort`, `swinglong`, `swingtwirl`, `hit`, `clash`). Clips without
//!   a `hit` event, and all ranged attacks, use fixed delays.
//! - Voices: UTC SoundSetFile -> soundset.2da `resref` -> SSF slot strrefs ->
//!   dialog.tlk sound resrefs (battle cry, pain, death).
//! - Footsteps: `snd_footstep` events -> footstepsounds.2da row from
//!   appearance `footsteptype`, column from the surfacemat.2da `sound` code of
//!   the walkmesh face underfoot. `snd_hitground` uses appearancesndset fall*.
//!
//!   kotor2 --sfx-survey   resolves every referenced sound and lists misses.

use std::collections::HashMap;
use std::sync::Arc;

use bevy::prelude::*;
use k2_data::{Creature, Tables};
use k2_res::{ResKey, Resources};
use k2_script::ObjectId;
use k2_world::stats::{SLOT_BODY, SLOT_CARMOUR, SLOT_CWEAPON_B, SLOT_CWEAPON_L, SLOT_CWEAPON_R, SLOT_RIGHTWEAPON};
use k2_world::projectiles::ProjectileEnd;
use k2_world::{World, WorldEvent};
use rakata_core::ResourceType;

use crate::anim::AnimPlayer;
use crate::audio::{AudioCache, SoundIndex, spawn_positional};
use crate::game::{FrameEvents, GameSet, ObjEntities, ObjId, WorldRes};

pub const SWITCHES: &[&str] = &["--sfx-survey"];

pub fn wants_survey() -> bool {
    std::env::args().any(|a| a == "--sfx-survey")
}

/// Fallback delays when a clip carries no events (s).
const MELEE_SWING_DELAY: f32 = 0.3;
const MELEE_IMPACT_DELAY: f32 = 0.45;
/// The bolt's launch (projectiles.rs draws it from the same event).
const RANGED_SHOT_DELAY: f32 = k2_world::projectiles::BOLT_LAUNCH;
const RANGED_IMPACT_DELAY: f32 = 0.4;
/// Pain/death grunts land just after the impact.
const VOICE_DELAY: f32 = 0.5;
const PAIN_COOLDOWN: f32 = 2.5;
const BATTLECRY_CHANCE: u32 = 30;
const BATTLECRY_QUIET: f32 = 20.0;

/// Attenuation ranges (min full volume, max silent, gain).
const COMBAT_RANGE: (f32, f32, f32) = (4.0, 40.0, 1.0);
const VOICE_RANGE: (f32, f32, f32) = (4.0, 35.0, 1.0);
const FOOT_RANGE: (f32, f32, f32) = (1.0, 15.0, 0.5);

pub struct SfxPlugin;

impl Plugin for SfxPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<SfxState>()
            .add_systems(Update, (combat_events, anim_events, fire_scheduled, expire_sfx).chain().in_set(GameSet::Sync).after(crate::game::ApplyEvents));
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Outcome {
    Pending,
    Hit,
    Miss,
    /// A bolt turned aside by a lightsaber.
    Deflect,
}

struct Strike {
    target: ObjectId,
    ranged: bool,
    outcome: Outcome,
    weapon: WeaponSfx,
    /// Attack clip carries a `hit` event: impact waits for it.
    anim_hit: bool,
    /// Attack clip carries a swing event: the swing waits for it.
    anim_swing: bool,
    started: f32,
    /// A bolt's landing, seconds after the attack started (its projectile event).
    landing: Option<f32>,
    /// The shot sound is scheduled (by the bolt's projectile event).
    shot_done: bool,
}

#[derive(Clone, Copy, Debug, Default)]
struct WeaponSfx {
    /// weaponsounds.2da row.
    row: usize,
    /// ammunitiontypes.2da row for ranged weapons.
    ammo: Option<usize>,
}

struct Scheduled {
    at: f32,
    emitter: ObjectId,
    resref: String,
    what: &'static str,
    range: (f32, f32, f32),
}

/// Resolved SSF slots (sound resrefs) per soundset.2da row.
type SoundSet = Arc<Vec<Option<String>>>;

#[derive(Resource, Default)]
pub struct SfxState {
    strikes: HashMap<ObjectId, Strike>,
    queue: Vec<Scheduled>,
    pain_ready: HashMap<ObjectId, f32>,
    last_attack: HashMap<ObjectId, f32>,
    soundsets: HashMap<u32, Option<SoundSet>>,
    /// UTC template -> soundset.2da row.
    template_soundset: HashMap<String, Option<u32>>,
    rng: u32,
}

impl SfxState {
    fn rand(&mut self, n: u32) -> u32 {
        // xorshift32; seeded lazily.
        if self.rng == 0 {
            self.rng = 0x2545_F491;
        }
        let mut x = self.rng;
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        self.rng = x;
        if n == 0 { 0 } else { x % n }
    }

    fn schedule(&mut self, at: f32, emitter: ObjectId, resref: Option<String>, what: &'static str, w: &World, range: (f32, f32, f32)) {
        match resref {
            Some(r) => self.queue.push(Scheduled { at, emitter, resref: r, what, range }),
            None => debug!("sfx: {what} {}: no sound in tables", w.describe(emitter)),
        }
    }

    fn soundset_of(&mut self, w: &World, obj: ObjectId) -> Option<SoundSet> {
        let o = w.get(obj)?;
        let template = o.template.to_ascii_lowercase();
        let row = (*self
            .template_soundset
            .entry(template.clone())
            .or_insert_with(|| if template.is_empty() { None } else { Creature::load(&w.res, &template).ok().map(|c| c.sound_set) }))?;
        if let Some(s) = self.soundsets.get(&row) {
            return s.clone();
        }
        let set = load_soundset(&w.res, &w.tables, w.talk.as_ref(), row).map(Arc::new);
        self.soundsets.insert(row, set.clone());
        set
    }

    fn voice(&mut self, w: &World, obj: ObjectId, slots: &[usize]) -> Option<String> {
        let set = self.soundset_of(w, obj)?;
        let avail: Vec<&String> = slots.iter().filter_map(|&i| set.get(i)?.as_ref()).collect();
        if avail.is_empty() {
            return None;
        }
        let i = self.rand(avail.len() as u32) as usize;
        Some(avail[i].clone())
    }
}

/// soundset.2da row -> SSF -> 28 sound resrefs (via dialog.tlk).
fn load_soundset(res: &Resources, tables: &Tables, talk: Option<&k2_data::Talk>, row: u32) -> Option<Vec<Option<String>>> {
    let t = tables.get(res, "soundset").ok()?;
    let resref = t.resref(row as usize, "resref")?;
    let bytes = res.get(&ResKey::new(&resref, ResourceType::Ssf)).ok().flatten()?;
    let ssf = rakata_formats::read_ssf_from_bytes(&bytes).ok()?;
    let talk = talk?;
    Some(
        rakata_formats::SsfSoundSlot::ALL
            .iter()
            .map(|&slot| {
                let raw = ssf.get_raw(slot);
                if raw < 0 { None } else { talk.sound(raw as u32).filter(|s| !s.is_empty()) }
            })
            .collect(),
    )
}

/// SSF slot indices.
const SLOT_BATTLECRY: [usize; 6] = [0, 1, 2, 3, 4, 5];
const SLOT_PAIN: [usize; 2] = [12, 13];
const SLOT_DEAD: [usize; 1] = [15];

fn base_item_row(w: &World, item: ObjectId) -> Option<usize> {
    w.get(item).and_then(|o| o.item()).map(|i| i.base_item as usize)
}

fn weapon_sfx(w: &World, obj: ObjectId) -> WeaponSfx {
    let tables = &w.tables;
    let Some(c) = w.get(obj).and_then(|o| o.creature()) else { return WeaponSfx::default() };
    let base = tables.get(&w.res, "baseitems").ok();
    for slot in [SLOT_RIGHTWEAPON, SLOT_CWEAPON_R, SLOT_CWEAPON_B, SLOT_CWEAPON_L] {
        let Some(&item) = c.equipped.get(&slot) else { continue };
        let (Some(row), Some(b)) = (base_item_row(w, item), base.as_ref()) else { continue };
        let ammo = b.int(row, "ammunitiontype").filter(|&a| a > 0).map(|a| a as usize);
        let mat = b.int(row, "weaponmattype").filter(|&m| m >= 0).map(|m| m as usize);
        if ammo.is_some() || mat.is_some() {
            return WeaponSfx { row: mat.unwrap_or(0), ammo };
        }
    }
    // No weapon item: the appearance's sound set may name a natural weapon.
    let row = appearance_sndset(w, c.appearance)
        .and_then(|r| tables.get(&w.res, "appearancesndset").ok()?.int(r, "weapon"))
        .filter(|&m| m >= 0)
        .map_or(0, |m| m as usize);
    WeaponSfx { row, ammo: None }
}

fn appearance_sndset(w: &World, appearance: u32) -> Option<usize> {
    w.tables.get(&w.res, "appearance").ok()?.int(appearance as usize, "soundapptype").filter(|&v| v >= 0).map(|v| v as usize)
}

/// weaponsounds.2da hit column family for a target: cloth, leather, armor,
/// forcefield, metal, wood or stone.
fn armor_column(w: &World, obj: ObjectId) -> String {
    let Some(c) = w.get(obj).and_then(|o| o.creature()) else { return "metal".into() };
    if let Ok(b) = w.tables.get(&w.res, "baseitems") {
        for slot in [SLOT_BODY, SLOT_CARMOUR] {
            if let Some(&item) = c.equipped.get(&slot)
                && let Some(row) = base_item_row(w, item)
                && let Some(a) = b.str(row, "armortype")
            {
                return a.to_ascii_lowercase();
            }
        }
    }
    appearance_sndset(w, c.appearance)
        .and_then(|r| w.tables.get(&w.res, "appearancesndset").ok()?.str(r, "armortype").map(str::to_ascii_lowercase))
        .unwrap_or_else(|| "leather".into())
}

/// Picks `<base>0..n-1` at random, skipping empty cells.
fn pick(t: &k2_data::tables::Table, row: usize, base: &str, n: u32, st: &mut SfxState) -> Option<String> {
    let cells: Vec<String> = (0..n).filter_map(|i| t.resref(row, &format!("{base}{i}"))).collect();
    if cells.is_empty() {
        return None;
    }
    let i = st.rand(cells.len() as u32) as usize;
    Some(cells[i].clone())
}

fn weapon_cell(w: &World, st: &mut SfxState, row: usize, base: &str, n: u32) -> Option<String> {
    let t = w.tables.get(&w.res, "weaponsounds").ok()?;
    pick(&t, row, base, n, st).or_else(|| if row != 0 { pick(&t, 0, base, n, st) } else { None })
}

fn ammo_cell(w: &World, st: &mut SfxState, row: usize, base: &str) -> Option<String> {
    let t = w.tables.get(&w.res, "ammunitiontypes").ok()?;
    // Slot 0 is the normal shot; slot 1 is the power-attack variant.
    t.resref(row, &format!("{base}0")).or_else(|| pick(&t, row, base, 2, st))
}

fn has_melee_weapon(w: &World, obj: ObjectId) -> bool {
    let Some(c) = w.get(obj).and_then(|o| o.creature()) else { return false };
    let Ok(b) = w.tables.get(&w.res, "baseitems") else { return false };
    c.equipped
        .get(&SLOT_RIGHTWEAPON)
        .and_then(|&i| base_item_row(w, i))
        .is_some_and(|row| b.int(row, "weaponmattype").is_some() && b.int(row, "ammunitiontype").is_none())
}

/// Impact sound for a resolved strike.
fn impact(w: &World, st: &mut SfxState, attacker: ObjectId, s: &Strike, now: f32, delay: f32) {
    match (s.outcome, s.weapon.ammo) {
        (Outcome::Hit, Some(ammo)) => {
            let r = ammo_cell(w, st, ammo, "impactsound");
            st.schedule(now + delay, s.target, r, "shot impact", w, COMBAT_RANGE);
        }
        (Outcome::Hit, None) => {
            let col = armor_column(w, s.target);
            let r = weapon_cell(w, st, s.weapon.row, &col, 2).or_else(|| weapon_cell(w, st, s.weapon.row, "leather", 2));
            st.schedule(now + delay, s.target, r, "melee hit", w, COMBAT_RANGE);
        }
        (Outcome::Miss, None) if has_melee_weapon(w, s.target) => {
            let r = weapon_cell(w, st, s.weapon.row, "parry", 2);
            st.schedule(now + delay, s.target, r, "parry", w, COMBAT_RANGE);
        }
        // The lightsaber that turned the bolt aside rings like a clash.
        (Outcome::Deflect, _) => {
            let row = weapon_sfx(w, s.target).row;
            let r = weapon_cell(w, st, row, "clash", 2);
            st.schedule(now + delay, s.target, r, "deflect", w, COMBAT_RANGE);
        }
        _ => {
            let _ = attacker;
        }
    }
}

/// World combat events: start strikes, attach outcomes, voices.
fn combat_events(time: Res<Time>, world: Res<WorldRes>, events: Res<FrameEvents>, mut st: ResMut<SfxState>, bodies: Query<&AnimPlayer>, ents: Res<ObjEntities>, roots: Query<&crate::game::ObjBody>) {
    let now = time.elapsed_secs();
    let w = &world.0;
    let st = &mut *st;
    // Shots of this frame's bolts: (shooter, seconds from now, ammunition).
    let mut shots: Vec<(ObjectId, f32, Option<usize>)> = Vec::new();
    for ev in &events.0 {
        match ev {
            WorldEvent::AttackStarted { attacker, target, ranged, .. } => {
                let weapon = weapon_sfx(w, *attacker);
                // The clip apply_events just started on the attacker.
                let clip_events: Vec<String> = ents
                    .0
                    .get(attacker)
                    .and_then(|&r| roots.get(r).ok()?.0)
                    .and_then(|b| bodies.get(b).ok())
                    .map(|p| p.current_events().iter().map(|(_, n)| n.clone()).collect())
                    .unwrap_or_default();
                let anim_hit = !*ranged && clip_events.iter().any(|n| n == "hit");
                let anim_swing = !*ranged && clip_events.iter().any(|n| n.starts_with("swing"));
                let quiet = st.last_attack.get(attacker).is_none_or(|&t| now - t > BATTLECRY_QUIET);
                st.last_attack.insert(*attacker, now);
                if quiet && st.rand(100) < BATTLECRY_CHANCE {
                    let r = st.voice(w, *attacker, &SLOT_BATTLECRY);
                    st.schedule(now, *attacker, r, "battlecry", w, VOICE_RANGE);
                }
                st.strikes.insert(*attacker, Strike { target: *target, ranged: *ranged, outcome: Outcome::Pending, weapon, anim_hit, anim_swing, started: now, landing: None, shot_done: false });
            }
            WorldEvent::AttackHit { attacker, .. } => {
                if let Some(s) = st.strikes.get_mut(attacker) {
                    s.outcome = Outcome::Hit;
                }
            }
            WorldEvent::AttackMissed { attacker, .. } => {
                if let Some(s) = st.strikes.get_mut(attacker) {
                    s.outcome = Outcome::Miss;
                }
            }
            WorldEvent::AttackDeflected { attacker, .. } => {
                if let Some(s) = st.strikes.get_mut(attacker) {
                    s.outcome = Outcome::Deflect;
                }
            }
            // The bolt is announced in the same tick as its attack, so its end is the
            // outcome (the hit itself may be held for presentation, presentation.rs)
            // and its landing is when the impact sound plays.
            WorldEvent::Projectile { shooter, bolt, launch, flight, end, ammo, .. } => {
                // Only a blaster bolt is the strike's own shot; grenades and darts are not.
                if let Some(s) = st.strikes.get_mut(shooter)
                    && s.ranged
                    && *bolt
                {
                    s.landing = Some(launch + flight);
                    match end {
                        ProjectileEnd::Impact => s.outcome = Outcome::Hit,
                        ProjectileEnd::Deflect => s.outcome = Outcome::Deflect,
                        ProjectileEnd::Miss => s.outcome = Outcome::Miss,
                        ProjectileEnd::Detonate => {}
                    }
                    // The weapon that fired the bolt (its hand's ammunition) sets both sounds.
                    s.weapon.ammo = ammo.or(s.weapon.ammo);
                    s.shot_done = true;
                    shots.push((*shooter, *launch, s.weapon.ammo));
                }
            }
            WorldEvent::Damaged { object, amount, .. } if *amount > 0 => {
                let dead = w.get(*object).and_then(|o| o.creature()).is_some_and(|c| c.dead || c.hp <= 0);
                if !dead && st.pain_ready.get(object).is_none_or(|&t| now >= t) {
                    st.pain_ready.insert(*object, now + PAIN_COOLDOWN);
                    let r = st.voice(w, *object, &SLOT_PAIN);
                    st.schedule(now + VOICE_DELAY, *object, r, "pain", w, VOICE_RANGE);
                }
            }
            WorldEvent::Died { object, .. } => {
                let r = st.voice(w, *object, &SLOT_DEAD).or_else(|| st.voice(w, *object, &SLOT_PAIN));
                st.schedule(now + VOICE_DELAY, *object, r, "death", w, VOICE_RANGE);
            }
            _ => {}
        }
    }
    for (shooter, at, ammo) in shots {
        let r = ammo.and_then(|a| ammo_cell(w, st, a, "shotsound"));
        st.schedule(now + at, shooter, r, "shot", w, COMBAT_RANGE);
    }
    // Strikes whose timing doesn't come from clip events.
    let ids: Vec<ObjectId> = st.strikes.keys().copied().collect();
    for id in ids {
        let s = &st.strikes[&id];
        let age = now - s.started;
        if s.ranged {
            let s = st.strikes.remove(&id).expect("present");
            if !s.shot_done {
                // No bolt was announced (no ammunition model): the shot at its fixed time.
                let r = s.weapon.ammo.and_then(|a| ammo_cell(w, st, a, "shotsound"));
                st.schedule(now + RANGED_SHOT_DELAY, id, r, "shot", w, COMBAT_RANGE);
            }
            impact(w, st, id, &s, now, s.landing.unwrap_or(RANGED_IMPACT_DELAY));
            continue;
        }
        if !s.anim_swing && age == 0.0 {
            let row = s.weapon.row;
            let r = weapon_cell(w, st, row, "swingshort", 3);
            st.schedule(now + MELEE_SWING_DELAY, id, r, "swing", w, COMBAT_RANGE);
        }
        let s = &st.strikes[&id];
        if !s.anim_hit {
            let s = st.strikes.remove(&id).expect("present");
            impact(w, st, id, &s, now, MELEE_IMPACT_DELAY);
        } else if age > 3.0 {
            // The clip was interrupted before its hit event.
            st.strikes.remove(&id);
        }
    }
}

fn footstep_column(code: &str, label: &str) -> &'static str {
    match (code, label) {
        (_, "puddles") => "puddles",
        ("DT", _) => "dirt",
        ("GR", _) => "grass",
        ("ST", _) => "stone",
        ("WD", _) => "wood",
        ("WT", _) => "water",
        ("CP", _) => "carpet",
        ("MT", _) => "metal",
        ("LV", _) => "leaves",
        _ => "stone",
    }
}

fn fall_column(code: &str) -> &'static str {
    match code {
        "DT" | "GR" | "LV" => "falldirt",
        "MT" => "fallmetal",
        "WT" => "fallwater",
        _ => "fallhard",
    }
}

/// surfacemat.2da `sound` code and label of the face under an object.
fn surface_under(w: &World, obj: ObjectId) -> (String, String) {
    let Some(o) = w.get(obj) else { return ("ST".into(), String::new()) };
    let p = o.position;
    let mat = w.walk.ground(p.x, p.y, p.z + 0.5).map(|h| h.material);
    let Some(t) = mat.and_then(|m| Some((m, w.tables.get(&w.res, "surfacemat").ok()?))) else { return ("ST".into(), String::new()) };
    let (m, t) = t;
    (t.str(m as usize, "sound").unwrap_or("ST").to_owned(), t.str(m as usize, "label").unwrap_or("").to_ascii_lowercase())
}

/// Clip events from creature animations: swings, impacts, footsteps.
fn anim_events(time: Res<Time>, world: Res<WorldRes>, mut st: ResMut<SfxState>, mut bodies: Query<(&mut AnimPlayer, &ChildOf)>, ids: Query<&ObjId>) {
    let now = time.elapsed_secs();
    let w = &world.0;
    let st = &mut *st;
    for (mut player, parent) in &mut bodies {
        let fired = player.drain_events();
        if fired.is_empty() {
            continue;
        }
        let Ok(&ObjId(obj)) = ids.get(parent.parent()) else { continue };
        for ev in fired {
            match ev.as_str() {
                "swingshort" | "swinglong" | "swingtwirl" => {
                    let row = st.strikes.get(&obj).map_or_else(|| weapon_sfx(w, obj).row, |s| s.weapon.row);
                    let r = weapon_cell(w, st, row, &ev, 3);
                    st.schedule(now, obj, r, "swing", w, COMBAT_RANGE);
                }
                "clash" => {
                    let row = st.strikes.get(&obj).map_or(0, |s| s.weapon.row);
                    let r = weapon_cell(w, st, row, "clash", 2);
                    st.schedule(now, obj, r, "clash", w, COMBAT_RANGE);
                }
                "hit" => {
                    if let Some(s) = st.strikes.remove(&obj) {
                        impact(w, st, obj, &s, now, 0.0);
                    }
                }
                "snd_footstep" => {
                    let Some(c) = w.get(obj).and_then(|o| o.creature()) else { continue };
                    let Ok(app) = w.tables.get(&w.res, "appearance") else { continue };
                    let Some(kind) = app.int(c.appearance as usize, "footsteptype").filter(|&k| k >= 0) else { continue };
                    let vol = app.float(c.appearance as usize, "footstepvolume").unwrap_or(1.0);
                    let (code, label) = surface_under(w, obj);
                    let col = footstep_column(&code, &label);
                    let Ok(t) = w.tables.get(&w.res, "footstepsounds") else { continue };
                    let r = pick(&t, kind as usize, col, 3, st);
                    let (min, max, gain) = FOOT_RANGE;
                    st.schedule(now, obj, r, "footstep", w, (min, max, gain * vol));
                }
                "snd_hitground" => {
                    let Some(c) = w.get(obj).and_then(|o| o.creature()) else { continue };
                    let Some(row) = appearance_sndset(w, c.appearance) else { continue };
                    let (code, _) = surface_under(w, obj);
                    let r = w.tables.get(&w.res, "appearancesndset").ok().and_then(|t| t.resref(row, fall_column(&code)));
                    st.schedule(now, obj, r, "hitground", w, COMBAT_RANGE);
                }
                _ => {}
            }
        }
    }
}

/// Seconds after which a combat one-shot is removed even if it never
/// finished (out-of-range clips are paused by the attenuator).
#[derive(Component)]
struct SfxExpiry(f32);

#[allow(clippy::too_many_arguments)]
fn fire_scheduled(
    mut commands: Commands,
    time: Res<Time>,
    world: Res<WorldRes>,
    game: Res<crate::model::GameRes>,
    index: Res<SoundIndex>,
    mut cache: ResMut<AudioCache>,
    mut assets: ResMut<Assets<bevy::audio::AudioSource>>,
    ents: Res<ObjEntities>,
    mut st: ResMut<SfxState>,
) {
    let now = time.elapsed_secs();
    let w = &world.0;
    let (due, keep): (Vec<Scheduled>, Vec<Scheduled>) = std::mem::take(&mut st.queue).into_iter().partition(|s| s.at <= now);
    st.queue = keep;
    for s in due {
        let Some(&parent) = ents.0.get(&s.emitter) else { continue };
        match spawn_positional(&mut commands, &mut cache, &game.0, &index, &mut assets, parent, &s.resref, s.range) {
            Some((d, e)) => {
                // Footsteps are frequent; keep them at debug.
                if s.what == "footstep" {
                    debug!("sfx: {} {} -> {} [{:.2}s]", s.what, w.describe(s.emitter), s.resref, d.as_secs_f32());
                } else {
                    info!("sfx: {} {} -> {} [{:.2}s]", s.what, w.describe(s.emitter), s.resref, d.as_secs_f32());
                }
                commands.entity(e).insert(SfxExpiry(now + d.as_secs_f32() + 1.0));
            }
            None => warn!("sfx: {} {} -> {} NOT FOUND", s.what, w.describe(s.emitter), s.resref),
        }
    }
}

/// Despawns combat one-shots that outlived their clip (paused out of range).
fn expire_sfx(mut commands: Commands, time: Res<Time>, exp: Query<(Entity, &SfxExpiry)>) {
    let now = time.elapsed_secs();
    for (e, x) in &exp {
        if now > x.0 {
            commands.entity(e).despawn();
        }
    }
}

/// `--sfx-survey`: resolves every sound the combat tables and soundsets
/// reference, printing misses.
pub fn run_survey(res: &Resources, install: &k2_res::Install) {
    let tables = Tables::new();
    let talk = k2_data::Talk::load(install).ok();
    let index = SoundIndex::build(install);
    let mut missing: Vec<String> = Vec::new();
    let cells = |table: &str, skip: &[&str]| {
        let Ok(t) = tables.get(res, table) else {
            println!("{table}: not found");
            return Vec::new();
        };
        let mut out = Vec::new();
        for row in 0..t.rows() {
            for col in t.columns() {
                if skip.contains(&col.as_str()) {
                    continue;
                }
                if let Some(v) = t.resref(row, &col) {
                    out.push((format!("{table}[{row}].{col}"), v));
                }
            }
        }
        out
    };
    let mut refs = Vec::new();
    refs.extend(cells("weaponsounds", &["label", "pitchoffset"]));
    refs.extend(cells("ammunitiontypes", &["label", "model", "model0", "model1", "muzzleflash", "shieldhit"]));
    refs.extend(cells("footstepsounds", &["label", "rumblepattern", "rumblecutoff", "pitchoffset"]));
    refs.extend(cells("appearancesndset", &["label", "armortype", "weapon", "missindex", "looping"]));
    let found = |r: &str| index.find(res, r, crate::audio::SoundKind::Effect).is_some();
    let mut seen = std::collections::HashSet::new();
    let mut total = 0usize;
    for (what, r) in &refs {
        if seen.insert(r.to_ascii_lowercase()) {
            total += 1;
            if !found(r) {
                missing.push(format!("{what}: {r}"));
            }
        }
    }
    let tables_checked = total;
    // Soundsets.
    let mut sets = 0;
    let mut slots = 0;
    if let Ok(t) = tables.get(res, "soundset") {
        for row in 0..t.rows() {
            let Some(resref) = t.resref(row, "resref") else { continue };
            match load_soundset(res, &tables, talk.as_ref(), row as u32) {
                Some(set) => {
                    sets += 1;
                    for (i, s) in set.iter().enumerate() {
                        if let Some(s) = s {
                            slots += 1;
                            if seen.insert(s.to_ascii_lowercase()) {
                                total += 1;
                                if !found(s) {
                                    missing.push(format!("{resref}.ssf slot {i}: {s}"));
                                }
                            }
                        }
                    }
                }
                None => missing.push(format!("soundset[{row}]: {resref}.ssf not loadable")),
            }
        }
    }
    println!("sfx survey: {tables_checked} unique table sounds, {sets} soundsets ({slots} filled slots), {total} unique resrefs checked, {} missing", missing.len());
    for m in &missing {
        println!("  MISSING {m}");
    }
}

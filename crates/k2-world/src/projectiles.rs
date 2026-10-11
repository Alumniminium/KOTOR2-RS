//! Projectiles in flight: blaster bolts from ranged attacks, and grenades,
//! rockets and darts thrown or launched by spells.
//!
//! The world decides when a projectile leaves, where it lands and how the
//! landing goes (`WorldEvent::Projectile`); the client draws it (kotor2
//! projectiles.rs). Positions are game space (Z up). Data:
//! - Bolt: the weapon that fired it (the attack's own main or off-hand
//!   weapon) has baseitems `ammunitiontype`; ammunitiontypes.2da `model` is the
//!   bolt MDL, `muzzleflash` the muzzle emitter model, and the row also gives
//!   the shot and impact sounds. The bolt leaves the hand that fired it
//!   (`rhand` or `lhand`), from the weapon's `bullethook` node. The clip has no
//!   fire event, so the launch time is fixed (BOLT_LAUNCH).
//! - Spell projectile: spells.2da `proj` 1, `projmodel` is the item model
//!   drawn in flight, `projtype` grenade (thrown in an arc) or homing (straight),
//!   `projspwnpoint` throw (the throwing hand, `rhand` on the client). The
//!   grenade's launch is at conjtime and it lands with the spell impact.
//! - Detonation: the VFX row k_sup_grenade.nss plays for the spell
//!   (visualeffects.2da `imp_root_m_node` explosion model, `soundimpact`).

use glam::Vec3;
use k2_script::ObjectId;

use crate::items::WeaponInfo;
use crate::object::Target;
use crate::stats::{SLOT_CWEAPON_B, SLOT_CWEAPON_L, SLOT_CWEAPON_R, SLOT_RIGHTWEAPON};
use crate::{World, WorldEvent};

/// Bolt speed in m/s. Tuned by eye, not measured against the original.
pub const BOLT_SPEED: f32 = 60.0;
/// Seconds from the attack's start to the bolt leaving the weapon (the shot's
/// place in the clip; sfx.rs plays the shot sound at the same time).
pub const BOLT_LAUNCH: f32 = 0.25;
/// Shortest flight, so a point-blank bolt is still seen leaving the weapon.
const MIN_FLIGHT: f32 = 0.1;
/// Height of the weapon hook and of the target's hit point, above the feet.
const HOOK_HEIGHT: f32 = 1.1;
const CHEST_HEIGHT: f32 = 1.0;
/// How far past a target a missed bolt keeps going, and how far off its line.
const MISS_OVERSHOOT: f32 = 6.0;
const MISS_OFFSET: f32 = 0.8;
/// A deflected bolt stops this far in front of the target's chest.
const DEFLECT_OFFSET: f32 = 0.5;
/// Range within which the ADVANCED lightsaber throw reaches the target's friends.
const SABER_FRIEND_RANGE: f32 = 5.0;
/// A lightsaber throw needs its target at least this far away (TLK 1248).
pub const SABER_MIN_RANGE: f32 = 5.0;

/// What happens where a projectile lands.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProjectileEnd {
    /// Hits the target: a small flash.
    Impact,
    /// Flies past the target: nothing at the end.
    Miss,
    /// Turned aside by the target's lightsaber: bounces back toward the shooter.
    Deflect,
    /// Explodes (grenade, rocket): the blast model and sound at the landing.
    Detonate,
}

/// A spell's projectile: the item model in flight and its trajectory.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SpellProjectile {
    pub model: String,
    /// Ballistic throw (grenade) rather than a straight shot (homing).
    pub arc: bool,
}

/// Model and sound of an explosion (visualeffects.2da row).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Blast {
    pub model: Option<String>,
    pub sound: Option<String>,
}

/// The ammunitiontypes row a baseitems row fires, if it is a ranged weapon.
pub fn ammo_of_base_item(w: &World, base_item: usize) -> Option<usize> {
    let base = w.tables.get(&w.res, "baseitems").ok()?;
    base.int(base_item, "ammunitiontype").filter(|&a| a > 0).map(|a| a as usize)
}

/// The ammunitiontypes row for the attacker's ranged weapon, if it has one.
fn ammo_row(w: &World, shooter: ObjectId) -> Option<usize> {
    let c = w.get(shooter)?.creature()?;
    for slot in [SLOT_RIGHTWEAPON, SLOT_CWEAPON_R, SLOT_CWEAPON_B, SLOT_CWEAPON_L] {
        let Some(&item) = c.equipped.get(&slot) else { continue };
        let Some(row) = w.get(item).and_then(|o| o.item()).map(|i| i.base_item as usize) else { continue };
        if let Some(ammo) = ammo_of_base_item(w, row) {
            return Some(ammo);
        }
    }
    None
}

/// Bolt MDL of an ammunitiontypes row (`model`).
pub fn bolt_model_of_ammo(w: &World, ammo: usize) -> Option<String> {
    w.tables.get(&w.res, "ammunitiontypes").ok()?.resref(ammo, "model")
}

/// Muzzle flash MDL of an ammunitiontypes row (`muzzleflash`).
pub fn muzzle_flash_of_ammo(w: &World, ammo: usize) -> Option<String> {
    w.tables.get(&w.res, "ammunitiontypes").ok()?.resref(ammo, "muzzleflash")
}

/// Bolt MDL of the attacker's ranged weapon (ammunitiontypes `model`).
pub fn bolt_model(w: &World, shooter: ObjectId) -> Option<String> {
    bolt_model_of_ammo(w, ammo_row(w, shooter)?)
}

/// The projectile a spell throws or launches (spells.2da `proj` rows with a
/// grenade or homing type). None for every other spell.
pub fn spell_projectile(w: &World, spell: i32) -> Option<SpellProjectile> {
    let t = w.tables.get(&w.res, "spells").ok()?;
    let row = usize::try_from(spell).ok()?;
    if t.int(row, "proj").unwrap_or(0) == 0 {
        return None;
    }
    let arc = match t.str(row, "projtype")? {
        s if s.eq_ignore_ascii_case("grenade") => true,
        s if s.eq_ignore_ascii_case("homing") => false,
        _ => return None,
    };
    Some(SpellProjectile { model: t.resref(row, "projmodel")?, arc })
}

/// The visualeffects.2da row k_sup_grenade.nss plays where a grenade or rocket
/// lands, by spell id (the script's nVFX per branch). Spells without one, such
/// as the darts and the Kor grenades, have no explosion.
pub fn blast_vfx(spell: i32) -> Option<i32> {
    Some(match spell {
        87 | 246 => 3003,                           // fragmentation
        88 | 247 => 3004,                           // stun
        89 | 248 | 185 | 190 => 3005,               // thermal detonator, explosive and buster rockets
        90 | 249 | 191 => 3006,                     // poison, poison rocket
        91 | 186 | 187 | 250 | 255 | 256 => 3007,   // sonic
        92 | 251 => 3008,                           // adhesive
        93 | 252 => 3009,                           // cryoban
        94 | 193 | 253 => 3010,                     // plasma (spell 94 is named fire in spells.2da)
        95 | 192 | 254 => 3011,                     // ion, ion rocket
        _ => return None,
    })
}

/// Explosion model and sound of a visualeffects.2da VFX id. The ids are row
/// labels, and visualeffects.2da has gaps, so the row is found by its label.
pub fn blast(w: &World, vfx: i32) -> Blast {
    let Ok(t) = w.tables.get(&w.res, "visualeffects") else { return Blast::default() };
    let Some(row) = (0..t.rows()).find(|&r| t.row_label(r).and_then(|l| l.parse::<i32>().ok()) == Some(vfx)) else {
        return Blast::default();
    };
    Blast { model: t.resref(row, "imp_root_m_node"), sound: t.resref(row, "soundimpact") }
}

impl World {
    /// The blaster bolt of a ranged attack by `attacker` on `target`, ending as
    /// `end` (hit, miss or deflected). `weapon` is the weapon that fired, the
    /// main or the off-hand one as the attack resolved, so its ammunition and
    /// muzzle are the ones drawn and the bolt leaves that hand (`offhand`).
    /// Emitted before the damage.
    pub(crate) fn emit_bolt(&mut self, attacker: ObjectId, target: ObjectId, end: ProjectileEnd, weapon: &WeaponInfo, offhand: bool) {
        let Some(ammo) = ammo_of_base_item(self, weapon.base_item as usize) else { return };
        let Some(model) = bolt_model_of_ammo(self, ammo) else { return };
        let muzzle = muzzle_flash_of_ammo(self, ammo);
        let Some(shooter) = self.get(attacker).map(|o| o.position) else { return };
        let Some(victim) = self.get(target).map(|o| o.position) else { return };
        let hook = shooter + Vec3::Z * HOOK_HEIGHT;
        let chest = victim + Vec3::Z * CHEST_HEIGHT;
        let dir = (chest - hook).normalize_or_zero();
        let to = match end {
            ProjectileEnd::Deflect => chest - dir * DEFLECT_OFFSET,
            ProjectileEnd::Miss => {
                // Past the target, a little to one side (by object id, so the
                // combat rolls are untouched).
                let side = Vec3::new(-dir.y, dir.x, 0.0).normalize_or_zero();
                let off = if attacker & 1 == 0 { MISS_OFFSET } else { -MISS_OFFSET };
                chest + dir * MISS_OVERSHOOT + side * off
            }
            ProjectileEnd::Impact | ProjectileEnd::Detonate => chest,
        };
        let flight = ((to - hook).length() / BOLT_SPEED).max(MIN_FLIGHT);
        self.emit(WorldEvent::Projectile {
            shooter: attacker,
            model,
            hook: if offhand { "lhand" } else { "rhand" }.into(),
            bolt: true,
            muzzle,
            ammo: Some(ammo),
            to,
            launch: BOLT_LAUNCH,
            flight,
            arc: false,
            end,
            blast_model: None,
            blast_sound: None,
        });
    }

    /// The projectile of a spell cast: a grenade, rocket or dart (spells.2da
    /// proj rows), or a thrown lightsaber (spells with a catchanim). The spell
    /// impact runs at `impact_at` and the cast ends at `cast_end` (both
    /// seconds after the cast starts); the projectile leaves `launch` seconds in.
    pub(crate) fn emit_spell_projectile(&mut self, caster: ObjectId, spell: i32, target: Target, launch: f32, impact_at: f32, cast_end: f32) {
        if let Some((catch_anim, catch_lead)) = saber_catch(self, spell) {
            self.emit_saber_throw(caster, spell, target, launch, impact_at, cast_end, catch_anim, catch_lead);
            return;
        }
        let Some(p) = spell_projectile(self, spell) else { return };
        let landing = match target {
            Target::Object(t) => self.get(t).map(|o| o.position),
            Target::Point(pt) => Some(pt),
        };
        let Some(landing) = landing else { return };
        let explosion = blast_vfx(spell).map(|v| blast(self, v)).unwrap_or_default();
        let end = if explosion.model.is_some() { ProjectileEnd::Detonate } else { ProjectileEnd::Impact };
        self.emit(WorldEvent::Projectile {
            shooter: caster,
            model: p.model,
            hook: "rhand".into(),
            bolt: false,
            muzzle: None,
            ammo: None,
            to: landing,
            launch,
            flight: (impact_at - launch).max(MIN_FLIGHT),
            arc: p.arc,
            end,
            blast_model: explosion.model,
            blast_sound: explosion.sound,
        });
    }

    /// A lightsaber throw (spells 49 and 4): the saber leaves the caster's
    /// right hand, visits each stop in turn (the target; for the ADVANCED throw
    /// also its friends within 5 m, k_inc_force.nss), then comes back to the hand
    /// by `cast_end`. The stops share the time up to `impact_at`.
    #[allow(clippy::too_many_arguments)]
    fn emit_saber_throw(&mut self, caster: ObjectId, spell: i32, target: Target, launch: f32, impact_at: f32, cast_end: f32, catch_anim: String, catch_lead: f32) {
        let Some(model) = thrown_saber(self, caster) else { return };
        let stops: Vec<Vec3> = match target {
            Target::Point(p) => vec![p + Vec3::Z * CHEST_HEIGHT],
            Target::Object(_) => self.saber_targets(spell, target).iter().filter_map(|&t| self.get(t).map(|o| o.position + Vec3::Z * CHEST_HEIGHT)).collect(),
        };
        if stops.is_empty() {
            return;
        }
        let leg = (impact_at - launch).max(MIN_FLIGHT) / stops.len() as f32;
        let back = (cast_end - impact_at).max(0.0);
        self.emit(WorldEvent::SaberThrow { shooter: caster, model, stops, launch, leg, back, catch_anim, catch_lead });
    }
}

/// The creatures a lightsaber throw reaches, in order: the target, then for the
/// ADVANCED throw its friends within 5 m of it (k_inc_force.nss 2639-2660, see
/// combat.md). Empty for a point target or an invalid one.
impl World {
    pub(crate) fn saber_targets(&mut self, spell: i32, target: Target) -> Vec<ObjectId> {
        let Target::Object(t) = target else { return Vec::new() };
        if !self.is_valid(t) {
            return Vec::new();
        }
        let mut out = vec![t];
        if spell_label(self, spell).contains("ADVANCED") {
            out.extend(saber_friends(self, t));
        }
        out
    }

    /// Whether a lightsaber throw can go: a saber in the right hand, and the
    /// target at least SABER_MIN_RANGE away unless the cast is a cheat (the TLK
    /// 1248 rule; a guess at the engine's check).
    pub(crate) fn saber_throw_ready(&mut self, caster: ObjectId, target_at: Vec3, cheat: bool) -> bool {
        let Some(me) = self.get(caster).map(|o| o.position) else { return false };
        thrown_saber(self, caster).is_some() && (cheat || target_at.distance(me) >= SABER_MIN_RANGE)
    }

    /// One stop's hit of a lightsaber throw: it always hits (TLK 1248), for one
    /// d6 per two levels of the thrower (at least one die; a guess for level 1).
    /// A creature killed or knocked down by an earlier stop takes no more.
    pub(crate) fn saber_hit(&mut self, caster: ObjectId, target: ObjectId) {
        let standing = self.get(target).and_then(|o| o.creature()).is_some_and(|c| !c.dead && !c.down);
        if !self.is_valid(target) || !standing {
            return;
        }
        let level = self.combat_stats(caster).map_or(1, |s| s.level).max(1);
        let damage = self.combat.dice((level / 2).max(1), 6);
        self.emit(WorldEvent::AttackHit { attacker: caster, target, damage, critical: false });
        self.apply_damage(target, damage, caster);
        self.pump();
    }
}

/// The spells.2da label of a spell row.
fn spell_label(w: &World, spell: i32) -> String {
    let row = usize::try_from(spell).unwrap_or(usize::MAX);
    w.tables.get(&w.res, "spells").ok().and_then(|t| t.str(row, "label").map(str::to_ascii_uppercase)).unwrap_or_default()
}

/// The catch animation name and its lead (seconds before the saber is back in
/// the hand) of a lightsaber throw: spells.2da catchanim and catchtime. Only
/// the two saber throws have a catchanim.
pub fn saber_catch(w: &World, spell: i32) -> Option<(String, f32)> {
    let t = w.tables.get(&w.res, "spells").ok()?;
    let row = usize::try_from(spell).ok()?;
    let anim = t.str(row, "catchanim")?.to_owned();
    let lead = t.int(row, "catchtime").unwrap_or(0) as f32 / 1000.0;
    Some((anim, lead))
}

/// The item model of the caster's right-hand lightsaber (not a lightsaber
/// crystal, which is an item of its own).
fn thrown_saber(w: &mut World, caster: ObjectId) -> Option<String> {
    let c = w.get(caster)?.creature()?;
    let item = *c.equipped.get(&SLOT_RIGHTWEAPON)?;
    let row = w.get(item).and_then(|o| o.item()).map(|i| i.base_item as usize)?;
    let label = w.tables.get(&w.res, "baseitems").ok()?.str(row, "label")?.to_ascii_lowercase();
    if !label.contains("lightsaber") || label.contains("crystal") {
        return None;
    }
    w.item_def_of(item)?.model.clone()
}

/// The target's friends that the ADVANCED throw also reaches: the two nearest
/// creatures the target regards as friends, and only while each is within 5 m
/// of it. k_inc_force.nss 2639-2643: GetNearestCreature(CREATURE_TYPE_REPUTATION,
/// REPUTATION_TYPE_FRIEND, oTarget, nth) for nth 1 and 2, each within 5.0 of
/// oTarget. The search is combat.rs's nearest_creatures around the target, so
/// the direction of the reputation is the one the combat searches use.
fn saber_friends(w: &mut World, target: ObjectId) -> Vec<ObjectId> {
    let Some(tpos) = w.get(target).map(|o| o.position) else { return Vec::new() };
    // Criterion 3 (reputation), value 0: friends.
    w.nearest_creatures(target, &[(3, 0)])
        .into_iter()
        .take(2)
        .filter(|&id| w.get(id).is_some_and(|o| o.position.distance(tpos) <= SABER_FRIEND_RANGE))
        .collect()
}

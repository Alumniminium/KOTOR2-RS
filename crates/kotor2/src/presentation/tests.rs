//! Presentation gate: which events a bolt holds, and until when.

use super::*;
use k2_world::projectiles::ProjectileEnd;

const S: ObjectId = 7;
const T: ObjectId = 9;

fn bolt(shooter: ObjectId, launch: f32, flight: f32, end: ProjectileEnd) -> WorldEvent {
    WorldEvent::Projectile {
        shooter,
        model: "w_laserfire_r".into(),
        hook: "rhand".into(),
        bolt: true,
        muzzle: None,
        ammo: Some(1),
        to: Vec3::ZERO,
        launch,
        flight,
        arc: false,
        end,
        blast_model: None,
        blast_sound: None,
    }
}

fn hit(shooter: ObjectId, target: ObjectId) -> Vec<WorldEvent> {
    vec![
        WorldEvent::AttackHit { attacker: shooter, target, damage: 4, critical: false },
        WorldEvent::Damaged { object: target, amount: 4, by: shooter },
        WorldEvent::Died { object: target, killer: shooter },
    ]
}

#[test]
fn a_bolts_hit_shows_when_the_bolt_lands_and_the_death_holds_the_pose() {
    let mut g = Gate::default();
    // Attack at 1.0 s: the bolt leaves at +0.25 s and flies 0.5 s, so it lands at 1.75 s.
    let mut evs = vec![bolt(S, 0.25, 0.5, ProjectileEnd::Impact)];
    evs.extend(hit(S, T));
    let out = g.filter(1.0, evs);
    assert_eq!(out.len(), 1, "only the bolt itself is shown: {out:?}");
    assert!(g.holds_pose(T), "the death waits for the bolt");

    assert!(g.filter(1.5, vec![]).is_empty(), "nothing shows before the bolt lands");
    assert!(g.holds_pose(T));

    let out = g.filter(1.8, vec![]);
    assert_eq!(out.len(), 3, "the hit, the damage and the death come out together: {out:?}");
    assert!(matches!(out[0], WorldEvent::AttackHit { .. }) && matches!(out[1], WorldEvent::Damaged { .. }) && matches!(out[2], WorldEvent::Died { .. }));
    assert!(!g.holds_pose(T), "the death is released");
}

#[test]
fn a_knock_down_holds_the_pose_like_a_death() {
    let mut g = Gate::default();
    let mut evs = vec![bolt(S, 0.25, 0.5, ProjectileEnd::Impact), WorldEvent::AttackHit { attacker: S, target: T, damage: 2, critical: false }];
    evs.push(WorldEvent::KnockedDown { object: T, by: S });
    g.filter(1.0, evs);
    assert!(g.holds_pose(T), "a knocked-down creature keeps its pose until the bolt lands");
    assert_eq!(g.filter(1.8, vec![]).len(), 2, "the hit and the knock-down come out together");
    assert!(!g.holds_pose(T));
}

#[test]
fn a_miss_waits_for_the_bolt_to_fly_past() {
    let mut g = Gate::default();
    let out = g.filter(0.0, vec![bolt(S, 0.25, 0.5, ProjectileEnd::Miss), WorldEvent::AttackMissed { attacker: S, target: T }]);
    assert_eq!(out.len(), 1);
    assert!(g.filter(0.6, vec![]).is_empty(), "the miss text waits for the bolt");
    assert_eq!(g.filter(0.8, vec![]).len(), 1);
}

#[test]
fn melee_and_spells_are_not_held() {
    let mut g = Gate::default();
    let out = g.filter(0.0, vec![WorldEvent::AttackHit { attacker: S, target: T, damage: 2, critical: false }, WorldEvent::Damaged { object: T, amount: 2, by: S }]);
    assert_eq!(out.len(), 2, "a melee hit shows as it happens");
    // A grenade is a projectile but not a bolt: its damage is not held either.
    let grenade = WorldEvent::Projectile { shooter: S, model: "w_fraggren_001".into(), hook: "rhand".into(), bolt: false, muzzle: None, ammo: None, to: Vec3::ZERO, launch: 0.17, flight: 0.665, arc: true, end: ProjectileEnd::Detonate, blast_model: None, blast_sound: None };
    let out = g.filter(0.0, vec![grenade, WorldEvent::Damaged { object: T, amount: 20, by: S }]);
    assert_eq!(out.len(), 2, "a grenade's damage is not held: {out:?}");
}

#[test]
fn a_bolt_holds_only_its_own_outcome_not_the_shooters_other_damage() {
    let mut g = Gate::default();
    // One batch: the bolt at T, then in the same tick a melee hit by the same shooter on another creature U.
    let mut evs = vec![bolt(S, 0.25, 0.5, ProjectileEnd::Impact)];
    evs.extend(hit(S, T));
    let u = 11;
    evs.push(WorldEvent::AttackHit { attacker: S, target: u, damage: 3, critical: false });
    evs.push(WorldEvent::Damaged { object: u, amount: 3, by: S });
    let out = g.filter(1.0, evs);
    assert_eq!(out.len(), 3, "the bolt and the melee hit on U show; the bolt's hit on T is held: {out:?}");
    assert!(out.iter().any(|e| matches!(e, WorldEvent::Damaged { object, .. } if *object == u)));
    assert!(!out.iter().any(|e| matches!(e, WorldEvent::Damaged { object, .. } if *object == T)));
    // Later, while the bolt is still flying, the shooter's grenade damage to T is not the bolt's: it shows.
    let out = g.filter(1.2, vec![WorldEvent::Damaged { object: T, amount: 20, by: S }]);
    assert_eq!(out.len(), 1, "damage from another attack passes while the bolt flies");
    assert_eq!(g.filter(1.8, vec![]).len(), 3, "the bolt's own hit, damage and death come out at the landing");
}

#[test]
fn each_bolt_has_its_own_landing() {
    let mut g = Gate::default();
    g.filter(0.0, [bolt(S, 0.25, 0.5, ProjectileEnd::Impact)].into_iter().chain(hit(S, T)).collect()); // lands at 0.75
    g.filter(0.1, [bolt(S, 0.25, 1.0, ProjectileEnd::Impact)].into_iter().chain(hit(S, 11)).collect()); // lands at 1.35
    assert_eq!(g.filter(0.8, vec![]).len(), 3, "the first bolt's hit is shown at its landing");
    assert!(g.filter(1.0, vec![]).is_empty(), "the second bolt's hit is still held");
    assert_eq!(g.filter(1.4, vec![]).len(), 3, "released at the second bolt's landing");
}

#[test]
fn a_new_area_drops_what_is_held() {
    let mut g = Gate::default();
    let mut evs = vec![bolt(S, 0.25, 0.5, ProjectileEnd::Impact)];
    evs.extend(hit(S, T));
    g.filter(1.0, evs);
    assert!(g.holds_pose(T));
    let out = g.filter(1.1, vec![WorldEvent::ModuleEntered { module: "101per".into() }]);
    assert_eq!(out.len(), 1, "the new area's events pass; the old area's held hit is dropped: {out:?}");
    assert!(!g.holds_pose(T));
    assert!(g.filter(2.0, vec![]).is_empty(), "nothing comes back later");
}

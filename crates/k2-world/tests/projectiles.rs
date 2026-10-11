//! Projectile data against the real install: every ranged weapon fires a bolt
//! with a model, a muzzle flash and its shot and impact sounds; every spell
//! with a projectile row (grenades, rockets, darts) has a model in flight and,
//! where k_sup_grenade.nss blows it up, the explosion model and sound. Also a
//! ranged attack in a real module emits one bolt per attack, in order, ending
//! as the attack did. Skips (passes) when no install is found.

use std::io::Write;
use k2_res::{Install, ResKey};
use k2_world::projectiles::{self, ProjectileEnd};
use k2_world::save::SaveData;
use k2_world::{World, WorldEvent};
use rakata_core::ResourceType;

const DT: f32 = 1.0 / 30.0;
const RIFLE: &str = "w_brifle_04";
const PISTOL: &str = "w_blaste_01";
const OFF_PISTOL: &str = "w_blaste_02";
const LEFT: i32 = 5;
const RIGHT: i32 = 4;

/// The game install, or None with a SKIP line when K2_INSTALL is unset and no
/// install is found there. A K2_INSTALL that is not an install fails.
fn install() -> Option<Install> {
    let set = std::env::var("K2_INSTALL").ok();
    let dir = set.clone().unwrap_or_else(|| concat!(env!("CARGO_MANIFEST_DIR"), "/../../..").into());
    let Ok(install) = Install::discover(&dir) else {
        assert!(set.is_none(), "K2_INSTALL={dir} is not a KotOR 2 install");
        let _ = writeln!(std::io::stderr(), "SKIP projectiles test: no install at {dir} (set K2_INSTALL)");
        return None;
    };
    Some(install)
}

fn world() -> Option<World> {
    let mut w = World::new(install()?).expect("the install loads");
    w.auto_dialog = true;
    w.enter_module("101per", None).unwrap();
    for _ in 0..60 {
        w.tick(DT);
        let _ = w.drain_events();
        if w.conversation.is_some() {
            w.end_conversation(true);
        }
    }
    Some(w)
}

/// Whether a resource of `kind` exists in the install (chitin, bifs or override).
fn exists(w: &World, name: &str, kind: ResourceType) -> bool {
    w.res.get(&ResKey::new(name, kind)).ok().flatten().is_some()
}

#[test]
fn every_ranged_weapon_fires_a_bolt_with_muzzle_and_sounds() {
    let Some(w) = world() else { return };
    let base = w.tables.get(&w.res, "baseitems").expect("baseitems.2da");
    let ammo_t = w.tables.get(&w.res, "ammunitiontypes").expect("ammunitiontypes.2da");
    let mut weapons = 0;
    let mut problems = Vec::new();
    for row in 0..base.rows() {
        if base.int(row, "rangedweapon").unwrap_or(0) == 0 {
            continue;
        }
        let label = base.str(row, "label").unwrap_or("?").to_owned();
        let Some(ammo) = projectiles::ammo_of_base_item(&w, row) else {
            problems.push(format!("{label}: no ammunitiontype"));
            continue;
        };
        weapons += 1;
        match projectiles::bolt_model_of_ammo(&w, ammo) {
            Some(m) if exists(&w, &m, ResourceType::Mdl) => {}
            other => problems.push(format!("{label}: bolt model {other:?} missing")),
        }
        match projectiles::muzzle_flash_of_ammo(&w, ammo) {
            Some(m) if exists(&w, &m, ResourceType::Mdl) => {}
            other => problems.push(format!("{label}: muzzle flash {other:?} missing")),
        }
        for col in ["shotsound0", "impactsound0"] {
            match ammo_t.resref(ammo, col) {
                Some(s) if exists(&w, &s, ResourceType::Wav) => {}
                other => problems.push(format!("{label}: {col} {other:?} missing")),
            }
        }
    }
    assert_eq!(weapons, 14, "ranged baseitems in Aspyr 1.0.2");
    assert!(problems.is_empty(), "ranged weapons without bolt data:\n{}", problems.join("\n"));
}

#[test]
fn every_spell_projectile_resolves_its_model_and_blast() {
    let Some(w) = world() else { return };
    let spells = w.tables.get(&w.res, "spells").expect("spells.2da");
    let mut found = 0;
    let mut problems = Vec::new();
    for row in 0..spells.rows() {
        // Spell ids are spells.2da row labels, which here equal the row index.
        assert_eq!(spells.row_label(row), Some(row.to_string().as_str()));
        let Some(p) = projectiles::spell_projectile(&w, row as i32) else { continue };
        found += 1;
        let label = spells.str(row, "label").unwrap_or("?").to_owned();
        if !exists(&w, &p.model, ResourceType::Mdl) {
            problems.push(format!("{label}: projmodel {} missing", p.model));
        }
        if let Some(vfx) = projectiles::blast_vfx(row as i32) {
            let b = projectiles::blast(&w, vfx);
            match &b.model {
                Some(m) if exists(&w, m, ResourceType::Mdl) => {}
                other => problems.push(format!("{label}: blast model {other:?} (VFX {vfx}) missing")),
            }
            match &b.sound {
                Some(s) if exists(&w, s, ResourceType::Wav) => {}
                other => problems.push(format!("{label}: blast sound {other:?} (VFX {vfx}) missing")),
            }
        }
    }
    // The 36 spells.2da rows with proj 1 are grenades, launched grenades, rockets and darts.
    assert_eq!(found, 36, "spells with a grenade or homing projectile");
    assert!(problems.is_empty(), "spell projectiles without data:\n{}", problems.join("\n"));
    // The thrown grenades explode with their own VFX row.
    assert_eq!(projectiles::blast_vfx(87), Some(3003));
    assert_eq!(projectiles::blast_vfx(95), Some(3011));
    assert!(projectiles::spell_projectile(&w, 87).is_some_and(|p| p.arc), "a fragmentation grenade is thrown in an arc");
    assert!(projectiles::spell_projectile(&w, 246).is_some_and(|p| !p.arc), "a launched grenade flies straight");
}

#[test]
fn ranged_attack_emits_one_bolt_per_attack_ending_as_the_attack_did() {
    let Some(mut w) = world() else { return };
    let leader = w.party_leader();
    for (slot, _) in w.equipment(leader) {
        if slot == RIGHT {
            w.unequip(leader, slot);
        }
    }
    let rifle = w.give_new_item(RIFLE, leader, 1, true).expect("rifle template");
    w.equip(leader, rifle, RIGHT).expect("equip rifle");
    let bolt = projectiles::bolt_model(&w, leader).expect("the rifle fires a bolt");

    // A hostile 4 m away, as in ranged_combat.rs: the leader shoots it until it dies.
    let from = w.get(leader).unwrap().position;
    let opponent = w
        .iter()
        .filter(|o| o.kind == k2_world::ObjectKind::Creature && !w.party.contains(&o.id) && o.scripts.contains_key("ScriptOnNotice"))
        .filter(|o| o.creature().is_some_and(|c| !c.dead))
        .map(|o| (o.position.distance(from), o.id))
        .min_by(|a, b| a.0.total_cmp(&b.0))
        .map(|(_, id)| id);
    let opponent = opponent.expect("a hostile creature in 101per");
    w.get_mut(opponent).unwrap().creature_mut().unwrap().faction = 1;
    let (opos, obearing) = w.get(opponent).map(|o| (o.position, o.bearing)).unwrap();
    let spot = opos + glam::Vec3::new(-obearing.sin(), obearing.cos(), 0.0) * 4.0;
    let ground = w.walk.ground(spot.x, spot.y, spot.z).map_or(spot, |h| glam::Vec3::new(spot.x, spot.y, h.z));
    w.set_player_position(ground, 0.0);

    let (mut shots, mut bolts, mut outcomes_ok) = (0, 0, true);
    let mut bolt_seen = false;
    let mut t = 0.0;
    while t < 30.0 {
        if w.get(leader).unwrap().actions.is_empty() && w.get(opponent).and_then(|o| o.creature()).is_some_and(|c| !c.dead) {
            w.player_attack(opponent);
        }
        w.tick(DT);
        t += DT;
        for e in w.drain_events() {
            match e {
                WorldEvent::AttackStarted { attacker, .. } if attacker == leader => {
                    shots += 1;
                    bolt_seen = false;
                }
                WorldEvent::Projectile { shooter, model, hook, bolt: is_bolt, muzzle, launch, flight, end, blast_model, .. } if shooter == leader => {
                    bolts += 1;
                    bolt_seen = true;
                    outcomes_ok &= model == bolt && hook == "rhand" && is_bolt && muzzle.is_some() && launch == projectiles::BOLT_LAUNCH && flight > 0.0 && blast_model.is_none();
                    outcomes_ok &= end != ProjectileEnd::Detonate;
                }
                WorldEvent::AttackHit { attacker, .. } | WorldEvent::AttackMissed { attacker, .. } if attacker == leader => {
                    // The bolt is announced before the outcome it carries.
                    outcomes_ok &= bolt_seen;
                }
                _ => {}
            }
        }
    }
    assert!(shots >= 2, "the leader fired {shots} times");
    assert_eq!(bolts, shots, "one bolt per ranged attack");
    assert!(outcomes_ok, "every bolt matches the rifle, its hook and its outcome");
}

#[test]
fn a_thrown_grenade_leaves_at_conjtime_and_lands_with_its_impact() {
    let Some(mut w) = world() else { return };
    let leader = w.party_leader();
    w.give_new_item("g_w_fraggren01", leader, 1, true).expect("fragmentation grenade template");
    // A creature in the open 4 m away, placed as in ranged_combat.rs.
    let from = w.get(leader).unwrap().position;
    let target = w
        .iter()
        .filter(|o| o.kind == k2_world::ObjectKind::Creature && !w.party.contains(&o.id))
        .filter(|o| o.creature().is_some_and(|c| !c.dead))
        .map(|o| (o.position.distance(from), o.id))
        .min_by(|a, b| a.0.total_cmp(&b.0))
        .map(|(_, id)| id)
        .expect("a creature in 101per");
    let (tpos, tbearing) = w.get(target).map(|o| (o.position, o.bearing)).unwrap();
    let spot = tpos + glam::Vec3::new(-tbearing.sin(), tbearing.cos(), 0.0) * 4.0;
    let ground = w.walk.ground(spot.x, spot.y, spot.z).map_or(spot, |h| glam::Vec3::new(spot.x, spot.y, h.z));
    w.set_player_position(ground, 0.0);
    w.use_talent(leader, k2_world::powers::TALENT_SPELL, 87, target);

    let (mut t, mut cast_at, mut impact_at) = (0.0f32, None, None);
    let mut projectile = None;
    while t < 20.0 && impact_at.is_none() {
        w.tick(DT);
        t += DT;
        for e in w.drain_events() {
            match e {
                WorldEvent::SpellCastStarted { caster, spell: 87, .. } if caster == leader => cast_at = Some(t),
                WorldEvent::Projectile { shooter, model, launch, flight, arc, end, blast_model, blast_sound, .. } if shooter == leader => {
                    projectile = Some((model, launch, flight, arc, end, blast_model, blast_sound));
                }
                WorldEvent::SpellImpact { caster, spell: 87, .. } if caster == leader => impact_at = Some(t),
                _ => {}
            }
        }
    }
    let (model, launch, flight, arc, end, blast_model, blast_sound) = projectile.expect("the grenade is thrown");
    assert_eq!(model, "w_fraggren_001");
    assert!(arc, "a grenade is thrown in an arc");
    assert_eq!(end, ProjectileEnd::Detonate);
    assert_eq!(blast_model.as_deref(), Some("v_grnfrag_fnf"));
    assert_eq!(blast_sound.as_deref(), Some("cb_gr_fragment"));
    // conjtime 170 ms to the release; the impact is conjtime + casttime / 2 after the start.
    assert!((launch - 0.17).abs() < 1e-3, "launch {launch}");
    assert!((launch + flight - 0.835).abs() < 1e-3, "lands at {}", launch + flight);
    let (cast, impact) = (cast_at.expect("cast started"), impact_at.expect("impact"));
    assert!(((impact - cast) - (launch + flight)).abs() <= 2.0 * DT + 1e-3, "landing {} vs impact {}", launch + flight, impact - cast);
}

/// Equips a lightsaber in the leader's right hand, with the Force to throw it.
fn armed_with_a_saber(w: &mut World, leader: k2_script::ObjectId) {
    for (slot, _) in w.equipment(leader) {
        if slot == RIGHT {
            w.unequip(leader, slot);
        }
    }
    let saber = w.give_new_item("g_w_lghtsbr01", leader, 1, true).expect("lightsaber template");
    w.equip(leader, saber, RIGHT).expect("equip the saber");
    if let Some(c) = w.get_mut(leader).and_then(|o| o.creature_mut()) {
        c.max_force = c.max_force.max(100);
        c.force = c.max_force;
    }
}

/// The creature nearest the leader, and the leader placed `metres` from it.
fn leader_placed_from_a_creature(w: &mut World, metres: f32) -> k2_script::ObjectId {
    let leader = w.party_leader();
    let from = w.get(leader).unwrap().position;
    let target = w
        .iter()
        .filter(|o| o.kind == k2_world::ObjectKind::Creature && !w.party.contains(&o.id) && o.id != leader)
        .filter(|o| o.creature().is_some_and(|c| !c.dead))
        .map(|o| (o.position.distance(from), o.id))
        .min_by(|a, b| a.0.total_cmp(&b.0))
        .map(|(_, id)| id)
        .expect("a creature in 101per");
    let (tpos, tbearing) = w.get(target).map(|o| (o.position, o.bearing)).unwrap();
    let spot = tpos + glam::Vec3::new(-tbearing.sin(), tbearing.cos(), 0.0) * metres;
    let ground = w.walk.ground(spot.x, spot.y, spot.z).map_or(spot, |h| glam::Vec3::new(spot.x, spot.y, h.z));
    w.set_player_position(ground, 0.0);
    target
}

/// Runs the world until the leader's cast ends and returns the saber events it sent.
fn saber_events_of_a_throw(w: &mut World, leader: k2_script::ObjectId, spell: i32, target: k2_script::ObjectId) -> Vec<WorldEvent> {
    w.use_talent(leader, k2_world::powers::TALENT_SPELL, spell, target);
    let mut out = Vec::new();
    let mut t = 0.0f32;
    while t < 6.0 {
        w.tick(DT);
        t += DT;
        for e in w.drain_events() {
            if matches!(e, WorldEvent::SaberThrow { shooter, .. } if shooter == leader) {
                out.push(e);
            }
        }
    }
    out
}

#[test]
fn a_lightsaber_throw_sends_the_saber_out_and_back_with_its_catch() {
    let Some(mut w) = world() else { return };
    let leader = w.party_leader();
    armed_with_a_saber(&mut w, leader);
    w.grant_power(leader, 49);
    let target = leader_placed_from_a_creature(&mut w, 6.0);

    let events = saber_events_of_a_throw(&mut w, leader, 49, target);
    assert_eq!(events.len(), 1, "one throw: {events:?}");
    let WorldEvent::SaberThrow { model, stops, launch, leg, back, catch_anim, catch_lead, .. } = &events[0] else { unreachable!() };
    let model = model.clone();
    assert_eq!(stops.len(), 1, "the plain throw visits the target only");
    assert!(exists(&w, &model, ResourceType::Mdl), "the saber model {model} is in the install");
    // conjtime 560 ms to the release; the impact at conjtime + casttime / 2 (1.53 s); the cast ends at 2.5 s.
    assert!((launch - 0.56).abs() < 1e-3, "launch {launch}");
    assert!((launch + leg - 1.53).abs() < 1e-3, "the target is reached at {}", launch + leg);
    assert!((launch + leg + back - 2.5).abs() < 1e-3, "back in the hand at {}", launch + leg + back);
    assert_eq!(catch_anim, "CATCH");
    assert!((catch_lead - 0.5).abs() < 1e-3, "catchtime 500 ms, got {catch_lead}");
}

#[test]
fn the_advanced_throw_also_visits_the_targets_friends_within_five_metres() {
    let Some(mut w) = world() else { return };
    let leader = w.party_leader();
    armed_with_a_saber(&mut w, leader);
    w.grant_power(leader, 4);
    let target = leader_placed_from_a_creature(&mut w, 6.0);
    // Two more creatures, of the target's faction, 2 m and 3 m from it.
    let faction = w.get(target).and_then(|o| o.creature()).map(|c| c.faction).expect("target faction");
    let others: Vec<k2_script::ObjectId> = w
        .iter()
        .filter(|o| o.kind == k2_world::ObjectKind::Creature && !w.party.contains(&o.id) && o.id != leader && o.id != target)
        .filter(|o| o.creature().is_some_and(|c| !c.dead))
        .map(|o| o.id)
        .take(2)
        .collect();
    assert_eq!(others.len(), 2, "two more creatures in 101per");
    let tpos = w.get(target).unwrap().position;
    for (i, &id) in others.iter().enumerate() {
        w.get_mut(id).unwrap().creature_mut().unwrap().faction = faction;
        w.place(id, tpos + glam::Vec3::new(2.0 + i as f32, 0.0, 0.0));
    }

    let events = saber_events_of_a_throw(&mut w, leader, 4, target);
    assert_eq!(events.len(), 1, "one throw: {events:?}");
    let WorldEvent::SaberThrow { stops, leg, back, .. } = &events[0] else { unreachable!() };
    assert_eq!(stops.len(), 3, "the target and its two friends within 5 m");
    assert!((leg * 3.0 - 0.97).abs() < 1e-3, "the three legs share the time to the impact: {leg}");
    assert!((back - 0.97).abs() < 1e-3);
}

#[test]
fn without_a_lightsaber_in_hand_the_throw_throws_nothing() {
    let Some(mut w) = world() else { return };
    let leader = w.party_leader();
    for (slot, _) in w.equipment(leader) {
        if slot == RIGHT {
            w.unequip(leader, slot);
        }
    }
    w.grant_power(leader, 49);
    if let Some(c) = w.get_mut(leader).and_then(|o| o.creature_mut()) {
        c.max_force = c.max_force.max(100);
        c.force = c.max_force;
    }
    let target = leader_placed_from_a_creature(&mut w, 6.0);
    // Without a saber the power cannot be used (the TLK text says "Requires Lightsaber"): the cast never starts.
    let (started, thrown) = cast_and_saber_throws(&mut w, leader, 49, target);
    assert!(!started, "the cast does not start without a saber");
    assert_eq!(thrown, 0, "and nothing is thrown");
    // Control: the same throw with a saber does start and throw, so the check above is not vacuous.
    armed_with_a_saber(&mut w, leader);
    let (started, thrown) = cast_and_saber_throws(&mut w, leader, 49, target);
    assert!(started && thrown == 1, "with a saber the throw starts and sends one saber (started {started}, thrown {thrown})");
}

/// Whether the leader's throw of `spell` at `target` started its cast, and how many sabers it sent.
fn cast_and_saber_throws(w: &mut World, leader: k2_script::ObjectId, spell: i32, target: k2_script::ObjectId) -> (bool, usize) {
    w.use_talent(leader, k2_world::powers::TALENT_SPELL, spell, target);
    let (mut started, mut thrown) = (false, 0);
    let mut t = 0.0f32;
    while t < 6.0 {
        w.tick(DT);
        t += DT;
        for e in w.drain_events() {
            match e {
                WorldEvent::SpellCastStarted { caster, spell: s, .. } if caster == leader && s == spell => started = true,
                WorldEvent::SaberThrow { shooter, .. } if shooter == leader => thrown += 1,
                _ => {}
            }
        }
    }
    (started, thrown)
}

#[test]
fn only_the_two_saber_throws_carry_a_catch_animation() {
    let Some(w) = world() else { return };
    let spells = w.tables.get(&w.res, "spells").expect("spells.2da");
    let with_catch: Vec<usize> = (0..spells.rows()).filter(|&r| projectiles::saber_catch(&w, r as i32).is_some()).collect();
    assert_eq!(with_catch, vec![4, 49]);
    assert_eq!(projectiles::saber_catch(&w, 49), Some(("CATCH".to_owned(), 0.5)));
}

#[test]
fn an_off_hand_blaster_fires_from_the_left_hand() {
    let Some(mut w) = world() else { return };
    let leader = w.party_leader();
    for (slot, _) in w.equipment(leader) {
        if slot == RIGHT || slot == LEFT {
            w.unequip(leader, slot);
        }
    }
    // Two one-handed blasters: a dual wield needs a partner of the same kind (inventory.rs).
    let right_item = w.give_new_item(PISTOL, leader, 1, true).expect("blaster template");
    w.equip(leader, right_item, RIGHT).expect("equip the blaster in the right hand");
    let left_item = w.give_new_item(OFF_PISTOL, leader, 1, true).expect("blaster template");
    w.equip(leader, left_item, LEFT).expect("equip the blaster in the left hand");
    let model_of = |w: &World, item: k2_script::ObjectId| {
        let base = w.get(item).and_then(|o| o.item()).map(|i| i.base_item as usize).expect("an item");
        projectiles::ammo_of_base_item(w, base).and_then(|a| projectiles::bolt_model_of_ammo(w, a)).expect("a bolt model")
    };
    let (right_bolt, left_bolt) = (model_of(&w, right_item), model_of(&w, left_item));
    assert_ne!(right_bolt, left_bolt, "the two blasters fire different bolts, so the model check is not vacuous");
    // The firing weapon's ammunition row: the off-hand blaster's shot and impact sounds come from it.
    let ammo_of = |w: &World, item: k2_script::ObjectId| {
        let base = w.get(item).and_then(|o| o.item()).map(|i| i.base_item as usize).expect("an item");
        projectiles::ammo_of_base_item(w, base).expect("a ranged item")
    };
    let (right_ammo, left_ammo) = (ammo_of(&w, right_item), ammo_of(&w, left_item));
    assert_ne!(right_ammo, left_ammo, "the two blasters fire different ammunition");

    let opponent = leader_placed_from_a_creature(&mut w, 4.0);
    // Tough enough to keep the off-hand shots coming for the 30 s.
    let target = w.get_mut(opponent).unwrap().creature_mut().unwrap();
    target.faction = 1;
    target.hp = 500;

    let (mut left_shots, mut right_shots, mut mismatched) = (0, 0, 0);
    let mut t = 0.0;
    while t < 30.0 {
        if w.get(leader).unwrap().actions.is_empty() && w.get(opponent).and_then(|o| o.creature()).is_some_and(|c| !c.dead) {
            w.player_attack(opponent);
        }
        w.tick(DT);
        t += DT;
        for e in w.drain_events() {
            if let WorldEvent::Projectile { shooter, model, hook, ammo, .. } = e
                && shooter == leader
            {
                match hook.as_str() {
                    "lhand" => {
                        left_shots += 1;
                        mismatched += usize::from(model != left_bolt) + usize::from(ammo != Some(left_ammo));
                    }
                    "rhand" => {
                        right_shots += 1;
                        mismatched += usize::from(model != right_bolt) + usize::from(ammo != Some(right_ammo));
                    }
                    _ => mismatched += 1,
                }
            }
        }
    }
    assert!(left_shots >= 1, "the off-hand blaster fires from the left hand ({left_shots} bolts)");
    assert!(right_shots >= 1, "the main-hand blaster fires from the right hand ({right_shots} bolts)");
    assert_eq!(mismatched, 0, "each bolt is the model and ammunition of the weapon in the hand it leaves");
}

#[test]
fn a_fake_grenade_cast_throws_nothing() {
    let Some(mut w) = world() else { return };
    let leader = w.party_leader();
    w.give_new_item("g_w_fraggren01", leader, 1, true).expect("grenade template");
    let target = leader_placed_from_a_creature(&mut w, 4.0);
    // A fake cast (ActionCastFakeSpellAtObject) runs the animation and no impact.
    let cast = k2_world::powers::CastAction {
        talent_type: k2_world::powers::TALENT_SPELL,
        id: 87,
        target: k2_world::Target::Object(target),
        fake: true,
        cheat: false,
        elapsed: 0.0,
        started: false,
        impacted: false,
        duration: 0.0,
        impact_at: 0.0,
        item: None,
        saber_hits: Vec::new(),
    };
    w.push_action(leader, k2_world::Action::Cast(Box::new(cast)));
    let (mut casts, mut thrown, mut impacts) = (0, 0, 0);
    let mut t = 0.0;
    while t < 4.0 {
        w.tick(DT);
        t += DT;
        for e in w.drain_events() {
            match e {
                WorldEvent::SpellCastStarted { caster, spell: 87, .. } if caster == leader => casts += 1,
                WorldEvent::Projectile { shooter, .. } if shooter == leader => thrown += 1,
                WorldEvent::SpellImpact { caster, spell: 87, .. } if caster == leader => impacts += 1,
                _ => {}
            }
        }
    }
    assert_eq!(casts, 1, "the fake cast starts");
    assert_eq!(thrown, 0, "nothing is thrown for a fake cast");
    assert_eq!(impacts, 0, "and no impact runs");
}

/// Damage the leader dealt (time, object, amount) after throwing `spell` at `target`,
/// and when the cast started.
fn saber_damage(w: &mut World, leader: k2_script::ObjectId, target: k2_script::ObjectId, spell: i32) -> (Vec<(f32, k2_script::ObjectId, i32)>, Option<f32>) {
    w.use_talent(leader, k2_world::powers::TALENT_SPELL, spell, target);
    let (mut t, mut dealt, mut cast_at) = (0.0f32, Vec::new(), None);
    while t < 6.0 {
        w.tick(DT);
        t += DT;
        for e in w.drain_events() {
            match e {
                WorldEvent::SpellCastStarted { caster, .. } if caster == leader => cast_at = cast_at.or(Some(t)),
                WorldEvent::Damaged { object, amount, by } if by == leader => dealt.push((t, object, amount)),
                _ => {}
            }
        }
    }
    (dealt, cast_at)
}

#[test]
fn a_lightsaber_throw_hits_once_for_d6_per_two_levels_when_the_saber_arrives() {
    let Some(mut w) = world() else { return };
    let leader = w.party_leader();
    armed_with_a_saber(&mut w, leader);
    w.grant_power(leader, 49);
    let target = leader_placed_from_a_creature(&mut w, 6.0);
    let level = w.combat_stats(leader).expect("the leader's stats").level;
    let (dealt, cast_at) = saber_damage(&mut w, leader, target, 49);
    let cast_at = cast_at.expect("the throw starts");
    assert_eq!(dealt.len(), 1, "one stop, one hit: {dealt:?}");
    let (at, object, amount) = dealt[0];
    assert_eq!(object, target);
    // The one stop is the impact: conjtime + casttime / 2 = 1.53 s after the cast starts.
    assert!(((at - cast_at) - 1.53).abs() <= 2.0 * DT + 1e-3, "the hit lands at the impact, {} s after the start", at - cast_at);
    let dice = (level / 2).max(1);
    assert!((dice..=6 * dice).contains(&amount), "{dice}d6 at level {level}, got {amount}");
}

#[test]
fn the_advanced_throw_hits_each_target_as_the_saber_reaches_it() {
    let Some(mut w) = world() else { return };
    let leader = w.party_leader();
    armed_with_a_saber(&mut w, leader);
    w.grant_power(leader, 4);
    let target = leader_placed_from_a_creature(&mut w, 6.0);
    let faction = w.get(target).and_then(|o| o.creature()).map(|c| c.faction).expect("target faction");
    let others: Vec<k2_script::ObjectId> = w
        .iter()
        .filter(|o| o.kind == k2_world::ObjectKind::Creature && !w.party.contains(&o.id) && o.id != leader && o.id != target)
        .filter(|o| o.creature().is_some_and(|c| !c.dead))
        .map(|o| o.id)
        .take(2)
        .collect();
    assert_eq!(others.len(), 2, "two more creatures in 101per");
    let tpos = w.get(target).unwrap().position;
    for (i, &id) in others.iter().enumerate() {
        w.get_mut(id).unwrap().creature_mut().unwrap().faction = faction;
        w.place(id, tpos + glam::Vec3::new(2.0 + i as f32, 0.0, 0.0));
    }
    let (dealt, cast_at) = saber_damage(&mut w, leader, target, 4);
    let cast_at = cast_at.expect("the throw starts");
    // Three stops, a leg each: the target at 0.56 + 0.97 / 3, then the friends, the last at the impact (1.53 s).
    let expected = [target, others[0], others[1]];
    let hit: Vec<k2_script::ObjectId> = dealt.iter().map(|d| d.1).collect();
    assert_eq!(hit, expected.to_vec(), "the target and its two friends, in order ({dealt:?})");
    for (k, (at, _, _)) in dealt.iter().enumerate() {
        let stop = 0.56 + 0.97 * (k as f32 + 1.0) / 3.0;
        assert!(((at - cast_at) - stop).abs() <= 2.0 * DT + 1e-3, "stop {k} at {} s, want {stop}", at - cast_at);
    }
}

#[test]
fn a_lightsaber_throw_at_a_target_closer_than_five_metres_does_not_happen() {
    let Some(mut w) = world() else { return };
    let leader = w.party_leader();
    armed_with_a_saber(&mut w, leader);
    w.grant_power(leader, 49);
    let target = leader_placed_from_a_creature(&mut w, 4.0);
    let (dealt, cast_at) = saber_damage(&mut w, leader, target, 49);
    assert!(cast_at.is_none(), "the cast does not start at 4 m");
    assert!(dealt.is_empty(), "and nothing is dealt");
}

#[test]
fn visualeffects_ids_are_row_labels_with_gaps() {
    let Some(w) = world() else { return };
    let vfx = w.tables.get(&w.res, "visualeffects").expect("visualeffects.2da");
    // The data rows (the probe's wc count included the header line).
    assert_eq!(vfx.rows(), 167, "visualeffects.2da data rows");
    assert_eq!(vfx.row_label(vfx.rows() - 1), Some("9015"), "the last VFX id");
    // The fragmentation grenade's VFX id 3003 is a label, not the row index.
    let row = (0..vfx.rows()).find(|&r| vfx.row_label(r) == Some("3003"));
    assert!(row.is_some_and(|r| r != 3003), "VFX 3003 is a row label, found on row {row:?}");
    assert_eq!(projectiles::blast(&w, 3003).model.as_deref(), Some("v_grnfrag_fnf"));
}

/// Places two creatures of the target's faction 2 m and 3 m from it, as the
/// ADVANCED throw's friends, and returns them in that order.
fn friends_of(w: &mut World, leader: k2_script::ObjectId, target: k2_script::ObjectId) -> [k2_script::ObjectId; 2] {
    let faction = w.get(target).and_then(|o| o.creature()).map(|c| c.faction).expect("target faction");
    let others: Vec<k2_script::ObjectId> = w
        .iter()
        .filter(|o| o.kind == k2_world::ObjectKind::Creature && !w.party.contains(&o.id) && o.id != leader && o.id != target)
        .filter(|o| o.creature().is_some_and(|c| !c.dead))
        .map(|o| o.id)
        .take(2)
        .collect();
    assert_eq!(others.len(), 2, "two more creatures in 101per");
    let tpos = w.get(target).unwrap().position;
    for (i, &id) in others.iter().enumerate() {
        w.get_mut(id).unwrap().creature_mut().unwrap().faction = faction;
        w.place(id, tpos + glam::Vec3::new(2.0 + i as f32, 0.0, 0.0));
    }
    [others[0], others[1]]
}

/// The objects `attacker` hit since the last call.
fn hits_by(w: &mut World, attacker: k2_script::ObjectId) -> Vec<k2_script::ObjectId> {
    w.drain_events()
        .into_iter()
        .filter_map(|e| match e {
            WorldEvent::AttackHit { attacker: a, target, .. } if a == attacker => Some(target),
            _ => None,
        })
        .collect()
}

/// Runs an ADVANCED throw at `target`. Once the first stop has hit the target,
/// `disable` is applied to the first friend, before its stop. Returns what the
/// leader hit, in order.
fn advanced_throw_disabling_the_first_friend(
    w: &mut World,
    leader: k2_script::ObjectId,
    target: k2_script::ObjectId,
    friends: [k2_script::ObjectId; 2],
    disable: impl Fn(&mut World, k2_script::ObjectId),
) -> Vec<k2_script::ObjectId> {
    w.use_talent(leader, k2_world::powers::TALENT_SPELL, 4, target);
    let (mut hit, mut disabled) = (Vec::new(), false);
    let mut t = 0.0f32;
    while t < 6.0 {
        w.tick(DT);
        t += DT;
        hit.extend(hits_by(w, leader));
        if !disabled && hit.first() == Some(&target) {
            disable(w, friends[0]);
            disabled = true;
        }
    }
    assert!(disabled, "the first stop hit the target: {hit:?}");
    hit
}

#[test]
fn a_stop_on_a_creature_killed_mid_throw_deals_nothing() {
    let Some(mut w) = world() else { return };
    let leader = w.party_leader();
    armed_with_a_saber(&mut w, leader);
    w.grant_power(leader, 4);
    let target = leader_placed_from_a_creature(&mut w, 6.0);
    let friends = friends_of(&mut w, leader, target);
    let hit = advanced_throw_disabling_the_first_friend(&mut w, leader, target, friends, |w, id| {
        w.get_mut(id).unwrap().creature_mut().unwrap().dead = true;
    });
    assert_eq!(hit, vec![target, friends[1]], "the killed friend takes no stop, and the last stop still lands");
}

#[test]
fn a_stop_on_a_creature_knocked_down_mid_throw_deals_nothing() {
    let Some(mut w) = world() else { return };
    let leader = w.party_leader();
    armed_with_a_saber(&mut w, leader);
    w.grant_power(leader, 4);
    let target = leader_placed_from_a_creature(&mut w, 6.0);
    let friends = friends_of(&mut w, leader, target);
    let hit = advanced_throw_disabling_the_first_friend(&mut w, leader, target, friends, |w, id| {
        w.get_mut(id).unwrap().creature_mut().unwrap().down = true;
    });
    assert_eq!(hit, vec![target, friends[1]], "the knocked-down friend takes no stop, and the last stop still lands");
}

/// A cheat cast of `spell` at `target`: whether it started, and how many hits
/// the caster landed.
fn cheat_cast(w: &mut World, caster: k2_script::ObjectId, spell: i32, target: k2_script::ObjectId) -> (bool, usize) {
    let cast = k2_world::powers::CastAction {
        talent_type: k2_world::powers::TALENT_SPELL,
        id: spell,
        target: k2_world::Target::Object(target),
        fake: false,
        cheat: true,
        elapsed: 0.0,
        started: false,
        impacted: false,
        duration: 0.0,
        impact_at: 0.0,
        item: None,
        saber_hits: Vec::new(),
    };
    w.push_action(caster, k2_world::Action::Cast(Box::new(cast)));
    let (mut started, mut hits) = (false, 0);
    let mut t = 0.0f32;
    while t < 6.0 {
        w.tick(DT);
        t += DT;
        for e in w.drain_events() {
            match e {
                WorldEvent::SpellCastStarted { caster: c, spell: s, .. } if c == caster && s == spell => started = true,
                WorldEvent::AttackHit { attacker, .. } if attacker == caster => hits += 1,
                _ => {}
            }
        }
    }
    (started, hits)
}

#[test]
fn a_cheat_throw_needs_a_saber_in_hand_but_not_the_five_metres() {
    let Some(mut w) = world() else { return };
    let leader = w.party_leader();
    armed_with_a_saber(&mut w, leader);
    let close = leader_placed_from_a_creature(&mut w, 4.0);
    let (started, hits) = cheat_cast(&mut w, leader, 49, close);
    assert!(started && hits == 1, "with a saber a cheat throw skips the 5 m rule (started {started}, hits {hits})");
    for (slot, _) in w.equipment(leader) {
        if slot == RIGHT {
            w.unequip(leader, slot);
        }
    }
    let far = leader_placed_from_a_creature(&mut w, 6.0);
    let (started, hits) = cheat_cast(&mut w, leader, 49, far);
    assert!(!started && hits == 0, "without a saber a cheat throw does not start (started {started}, hits {hits})");
}

#[test]
fn a_saber_throw_saved_mid_flight_still_hits_its_remaining_stops() {
    let Some(mut w) = world() else { return };
    let leader = w.party_leader();
    armed_with_a_saber(&mut w, leader);
    w.grant_power(leader, 4);
    let target = leader_placed_from_a_creature(&mut w, 6.0);
    let friends = friends_of(&mut w, leader, target);
    w.use_talent(leader, k2_world::powers::TALENT_SPELL, 4, target);
    // Run to the first stop: it hits the target, and the friends' stops are still to come.
    let mut first = Vec::new();
    let mut t = 0.0f32;
    while first.is_empty() && t < 6.0 {
        w.tick(DT);
        t += DT;
        first.extend(hits_by(&mut w, leader));
    }
    assert_eq!(first, vec![target], "the first stop hits the target before the save");
    assert!(w.save_blocked().is_none(), "{:?}", w.save_blocked());
    let bytes = w.save().to_bytes();
    let inst = install().expect("an install");
    let mut loaded = World::load(inst, &SaveData::from_bytes(&bytes).expect("the save reads back")).expect("the save loads");
    let mut rest = Vec::new();
    for _ in 0..(3.0 / DT) as usize {
        loaded.tick(DT);
        rest.extend(hits_by(&mut loaded, leader));
    }
    assert_eq!(rest, friends.to_vec(), "the loaded throw still reaches both friends, in order");
}

/// An ADVANCED throw at a target 6 m away, past a creature 2 m from the target.
/// The target's faction is 40 and the creature's 41: ids past repute.2da's 24
/// rows, so no other creature shares them and the only opinions set are the two
/// here. `toward` is the target's opinion of the creature and `back` the
/// creature's of the target. `down` knocks the creature out before the throw.
/// Returns the target, the creature and what the leader hit.
fn advanced_throw_past_a_neighbour(toward: i32, back: i32, down: bool) -> Option<(k2_script::ObjectId, k2_script::ObjectId, Vec<k2_script::ObjectId>)> {
    let mut w = world()?;
    let leader = w.party_leader();
    armed_with_a_saber(&mut w, leader);
    w.grant_power(leader, 4);
    let target = leader_placed_from_a_creature(&mut w, 6.0);
    let neighbour = w
        .iter()
        .filter(|o| o.kind == k2_world::ObjectKind::Creature && !w.party.contains(&o.id) && o.id != leader && o.id != target)
        .filter(|o| o.creature().is_some_and(|c| !c.dead))
        .map(|o| o.id)
        .next()
        .expect("a creature in 101per");
    w.get_mut(target).unwrap().creature_mut().unwrap().faction = 40;
    w.get_mut(neighbour).unwrap().creature_mut().unwrap().faction = 41;
    let tpos = w.get(target).unwrap().position;
    w.place(neighbour, tpos + glam::Vec3::new(2.0, 0.0, 0.0));
    // The table loads on first use, and an override set before that would be dropped.
    let _ = w.reputation(target, neighbour);
    w.combat.factions.set(40, 41, toward);
    w.combat.factions.set(41, 40, back);
    if down {
        w.get_mut(neighbour).unwrap().creature_mut().unwrap().down = true;
    }
    w.use_talent(leader, k2_world::powers::TALENT_SPELL, 4, target);
    let mut hits = Vec::new();
    let mut t = 0.0f32;
    while t < 6.0 {
        w.tick(DT);
        t += DT;
        hits.extend(hits_by(&mut w, leader));
    }
    Some((target, neighbour, hits))
}

#[test]
fn the_advanced_throw_follows_the_targets_opinion_of_a_neighbour() {
    // The target likes the neighbour, which dislikes the target: the neighbour is reached.
    let Some((target, neighbour, hits)) = advanced_throw_past_a_neighbour(95, 5, false) else { return };
    assert!(hits.contains(&target) && hits.contains(&neighbour), "the throw runs and reaches a friend of the target: {hits:?}");
    // The neighbour likes the target, which does not like it back: not the target's friend.
    let Some((target, neighbour, hits)) = advanced_throw_past_a_neighbour(5, 95, false) else { return };
    assert!(hits.contains(&target) && !hits.contains(&neighbour), "the throw runs and the neighbour is not the target's friend: {hits:?}");
}

#[test]
fn a_friend_knocked_down_before_the_throw_is_not_reached() {
    // Knocked-out creatures are skipped by the reputation search (combat.md: an assumption, not sourced).
    let Some((target, neighbour, hits)) = advanced_throw_past_a_neighbour(95, 5, true) else { return };
    assert!(hits.contains(&target), "the throw runs: {hits:?}");
    assert!(!hits.contains(&neighbour), "a knocked-down friend is not picked: {hits:?}");
}

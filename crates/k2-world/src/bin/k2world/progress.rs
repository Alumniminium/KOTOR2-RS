//! `k2world levelup` and `k2world powers`: XP, level up and Force power
//! harnesses (progression.rs, powers.rs).

use std::process::ExitCode;

use glam::Vec3;
use k2_res::Install;
use k2_script::ObjectId;
use k2_world::{World, WorldEvent};

const DT: f32 = 1.0 / 30.0;

fn load(install: &Install, module: &str) -> Option<World> {
    let mut w = World::new(install.clone()).map_err(|e| eprintln!("error: {e}")).ok()?;
    w.auto_dialog = true;
    w.enter_module(module, None).map_err(|e| eprintln!("error: {module}: {e}")).ok()?;
    for _ in 0..30 {
        w.tick(DT);
        if w.conversation.is_some() {
            w.end_conversation(true);
        }
    }
    let _ = w.drain_events();
    Some(w)
}

fn spell_name(w: &World, spell: i32) -> String {
    let t = w.tables.get(&w.res, "spells").ok();
    let label = t.as_ref().and_then(|t| t.str(spell as usize, "label").map(str::to_owned)).unwrap_or_default();
    format!("{spell} {label}")
}

fn feat_name(w: &World, feat: i32) -> String {
    let t = w.tables.get(&w.res, "feat").ok();
    t.as_ref().and_then(|t| t.str(feat as usize, "label").map(str::to_owned)).unwrap_or_else(|| feat.to_string())
}

fn sheet(w: &mut World, id: ObjectId) {
    let (classes, hp, fp, xp, ab) = {
        let c = w.get(id).and_then(|o| o.creature()).cloned().unwrap_or_default();
        (c.classes, (c.hp, c.max_hp), (c.force, c.max_force), c.xp, c.abilities)
    };
    println!(
        "  {}: level {} classes {:?} hp {}/{} fp {}/{} xp {} abilities {:?}",
        w.describe(id),
        w.level_of(id),
        classes,
        hp.0,
        hp.1,
        fp.0,
        fp.1,
        xp,
        ab
    );
    let feats: Vec<String> = w.known_feats(id).into_iter().map(|f| feat_name(w, i32::from(f))).collect();
    let powers: Vec<String> = w.known_powers(id).into_iter().map(|p| spell_name(w, p)).collect();
    let skills: Vec<i32> = (0..8).map(|s| w.skill_rank(id, s)).collect();
    println!("    feats: {}", feats.join(", "));
    println!("    powers: {}", powers.join(", "));
    println!("    skills: {skills:?}");
}

/// `levelup [MODULE] [--class N] [--level N]`
pub fn levelup(install: &Install, module: &str, class: i32, target_level: i32) -> ExitCode {
    let Some(mut w) = load(install, module) else { return ExitCode::FAILURE };
    let pc = w.party_leader();
    if let Some(c) = w.get_mut(pc).and_then(|o| o.creature_mut()) {
        c.classes = vec![(class, 1)];
    }
    w.progression.known.remove(&pc);
    // A companion that levels automatically.
    let ppos = w.get(pc).map_or(Vec3::ZERO, |o| o.position);
    let kreia = match w.spawn_creature("p_kreia", ppos + Vec3::new(1.5, 0.0, 0.0), 0.0) {
        Ok(id) => {
            w.party.push(id);
            Some(id)
        }
        Err(e) => {
            println!("no companion: {e}");
            None
        }
    };
    println!("before:");
    sheet(&mut w, pc);
    if let Some(k) = kreia {
        sheet(&mut w, k);
    }
    let xp = w.xp_for_level(target_level);
    println!("GiveXPToCreature(PC, {xp})");
    w.give_xp(pc, xp);
    for e in w.drain_events() {
        match e {
            WorldEvent::LevelUpAvailable { creature, level } => println!("  event: level up available for {} -> {level}", w.describe(creature)),
            WorldEvent::LevelledUp { creature, level, class } => println!("  event: {} levelled up to {level} (class {class})", w.describe(creature)),
            _ => {}
        }
    }
    let mut ups = 0;
    while w.level_up_available(pc) {
        let Some(o) = w.level_up_options(pc) else { break };
        println!(
            "level up {} -> {} ({} {}): hp +{} fp +{} skill points {} attribute {} feats {}/{} powers {}/{} granted [{}]",
            o.new_level - 1,
            o.new_level,
            o.class_name,
            o.class_level,
            o.hp_gain,
            o.force_gain,
            o.skill_points,
            o.attribute_points,
            o.feat_picks,
            o.feats.len(),
            o.power_picks,
            o.powers.len(),
            o.granted_feats.iter().map(|f| f.name.clone()).collect::<Vec<_>>().join(", ")
        );
        let choice = w.auto_choice(&o);
        println!(
            "  picks: attribute {:?} skills {:?} feats [{}] powers [{}]",
            choice.attribute,
            choice.skills,
            choice.feats.iter().map(|f| feat_name(&w, i32::from(*f))).collect::<Vec<_>>().join(", "),
            choice.powers.iter().map(|p| spell_name(&w, *p)).collect::<Vec<_>>().join(", ")
        );
        if let Err(e) = w.apply_level_up(pc, &choice) {
            println!("  error: {e}");
            return ExitCode::FAILURE;
        }
        ups += 1;
    }
    let _ = w.drain_events();
    println!("after ({ups} level ups):");
    sheet(&mut w, pc);
    if let Some(k) = kreia {
        sheet(&mut w, k);
    }
    let ok = w.level_of(pc) == target_level && kreia.is_none_or(|k| w.level_of(k) > 3);
    println!("vm errors: {}", w.vm_errors.len());
    if ok { ExitCode::SUCCESS } else { ExitCode::FAILURE }
}

/// `powers MODULE [--seconds N]`: Kreia joins the party with full Force
/// points, then a fight; prints every cast, impact, feat and effect.
pub fn powers(install: &Install, module: &str, seconds: f32) -> ExitCode {
    let Some(mut w) = load(install, module) else { return ExitCode::FAILURE };
    let pc = w.party_leader();
    let ppos = w.get(pc).map_or(Vec3::ZERO, |o| o.position);
    if let Ok(k) = w.spawn_creature("p_kreia", ppos + Vec3::new(1.0, 0.5, 0.0), 0.0) {
        w.party.push(k);
        if let Some(c) = w.get_mut(k).and_then(|o| o.creature_mut()) {
            c.max_force = c.max_force.max(80);
            c.force = c.max_force;
            c.faction = 2;
        }
        println!("Kreia joins: powers {:?}", w.known_powers(k).iter().map(|p| spell_name(&w, *p)).collect::<Vec<_>>());
    }
    // The Exile too: a Guardian with Force powers.
    if let Some(c) = w.get_mut(pc).and_then(|o| o.creature_mut()) {
        c.max_force = c.max_force.max(60);
        c.force = c.max_force;
    }
    println!("Exile powers {:?}", w.known_powers(pc).iter().map(|p| spell_name(&w, *p)).collect::<Vec<_>>());
    // The Exile can't die here, so the fight runs long enough to watch.
    if let Some(c) = w.get_mut(pc).and_then(|o| o.creature_mut()) {
        c.min1hp = true;
    }
    let ppos = w.get(pc).map_or(Vec3::ZERO, |o| o.position);
    let mut hostiles: Vec<(ObjectId, f32)> = w
        .iter()
        .filter(|o| o.kind == k2_world::object::ObjectKind::Creature && o.creature().is_some_and(|c| !c.dead))
        .map(|o| (o.id, o.position.distance(ppos)))
        .collect();
    hostiles.retain(|&(id, _)| w.is_enemy(id, pc));
    hostiles.sort_by(|a, b| a.1.total_cmp(&b.1));
    let Some(&(foe, _)) = hostiles.first() else {
        println!("no hostiles in {module}");
        return ExitCode::FAILURE;
    };
    let fpos = w.get(foe).map_or(Vec3::ZERO, |o| o.position);
    w.place(pc, fpos + Vec3::new(5.0, 0.0, 0.0));
    let party: Vec<ObjectId> = w.party.clone();
    for (i, &m) in party.iter().enumerate().skip(1) {
        w.place(m, fpos + Vec3::new(5.0, 1.0 + i as f32, 0.0));
    }
    println!("fight: party vs {} at {:.0} m", w.describe(foe), 5.0);
    let mut events = Vec::new();
    let mut t = 0.0;
    while t < seconds {
        let leader = w.party_leader();
        let busy = w.get(leader).is_some_and(|o| !o.actions.is_empty());
        if !busy {
            let alive: Vec<ObjectId> = w
                .iter()
                .filter(|o| o.kind == k2_world::object::ObjectKind::Creature && o.creature().is_some_and(|c| !c.dead))
                .map(|o| o.id)
                .filter(|&id| id != leader)
                .collect();
            let mut foes: Vec<(ObjectId, f32)> = Vec::new();
            for id in alive {
                if w.is_enemy(id, leader) {
                    foes.push((id, w.get(id).map_or(f32::MAX, |o| o.position.distance(ppos))));
                }
            }
            foes.sort_by(|a, b| a.1.total_cmp(&b.1));
            let target = foes.first().map(|f| f.0);
            match target {
                Some(tg) => w.player_attack(tg),
                None => break,
            }
        }
        w.tick(DT);
        if w.conversation.is_some() {
            w.end_conversation(true);
        }
        events.extend(w.drain_events());
        t += DT;
    }
    let mut casts = 0;
    let mut impacts = 0;
    let mut deaths = 0;
    for e in events {
        match e {
            WorldEvent::SpellCastStarted { caster, target, spell, anim, .. } => {
                casts += 1;
                println!("  cast  {} -> {}: {} ({anim})", w.describe(caster), w.describe(target), spell_name(&w, spell));
            }
            WorldEvent::SpellImpact { caster, target, spell, success, .. } => {
                impacts += 1;
                println!("  impact {} -> {}: {} success={success}", w.describe(caster), w.describe(target), spell_name(&w, spell));
            }
            WorldEvent::FeatUsed { creature, target, feat } => println!("  feat  {} -> {}: {}", w.describe(creature), w.describe(target), feat_name(&w, feat)),
            WorldEvent::VisualEffect { object, effect } => println!("  vfx   {} on {}", effect, w.describe(object)),
            WorldEvent::XpGained { creature, amount } if creature == pc => println!("  xp    +{amount}"),
            WorldEvent::Died { object, killer } => {
                deaths += 1;
                println!("  died  {} (by {})", w.describe(object), w.describe(killer));
            }
            _ => {}
        }
    }
    println!("{t:.1} s, deaths {deaths}");
    println!("casts {casts} impacts {impacts} vm errors {} unimplemented {:?}", w.vm_errors.len(), w.unimplemented);
    for e in w.vm_errors.iter().take(5) {
        println!("  vm error: {e}");
    }
    if w.vm_errors.is_empty() { ExitCode::SUCCESS } else { ExitCode::FAILURE }
}

/// `forceusers`: creatures with Force powers per module (to find fights).
pub fn force_users(install: &Install) -> ExitCode {
    let mut w = match World::new(install.clone()) {
        Ok(w) => w,
        Err(e) => {
            eprintln!("error: {e}");
            return ExitCode::FAILURE;
        }
    };
    for m in install.module_names() {
        if w.enter_module(&m, None).is_err() {
            continue;
        }
        let pc = w.party_leader();
        let ids: Vec<ObjectId> = w.iter().filter(|o| o.kind == k2_world::object::ObjectKind::Creature && o.id != pc).map(|o| o.id).collect();
        for id in ids {
            let powers = w.known_powers(id);
            if powers.is_empty() {
                continue;
            }
            let hostile = w.is_enemy(id, pc);
            println!("{m} {} hostile={hostile} powers={}", w.describe(id), powers.len());
        }
    }
    ExitCode::SUCCESS
}

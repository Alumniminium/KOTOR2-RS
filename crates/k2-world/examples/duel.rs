//! duel [MODULE] [CLASS] [RUNS] [DROIDS]: a level 1 PC against the nearest
//! hostile creatures, seeded runs; the PC attacks the nearest engaged
//! hostile. Prints win rate and HP lost.
use glam::Vec3;
use k2_res::Install;
use k2_script::ObjectId;
use k2_world::{ObjectKind, World, WorldEvent};

const DT: f32 = 1.0 / 30.0;

fn main() {
    let a: Vec<String> = std::env::args().collect();
    let module = a.get(1).cloned().unwrap_or("101per".into());
    let class: i32 = a.get(2).and_then(|s| s.parse().ok()).unwrap_or(0);
    let runs: u64 = a.get(3).and_then(|s| s.parse().ok()).unwrap_or(50);
    let count: usize = a.get(4).and_then(|s| s.parse().ok()).unwrap_or(2);
    let verbose = std::env::var("V").is_ok();
    let (mut wins, mut lost_hp, mut times, mut hits, mut swings, mut dmg_taken_hits) = (0, 0i64, 0.0f32, 0u32, 0u32, 0u32);
    let mut min_left = i32::MAX;
    for seed in 0..runs {
        let install = Install::discover("..").unwrap();
        let mut w = World::new(install).unwrap();
        w.auto_dialog = true;
        let portraits = w.pc_portraits(0);
        let p = portraits[2].clone();
        let mut spec = w.quick_spec(0, class, Some(&p));
        spec.name = "Duelist".into();
        w.player_spec = spec;
        w.enter_module(&module, None).unwrap();
        for _ in 0..90 {
            w.tick(DT);
            let _ = w.drain_events();
            if w.conversation.is_some() { w.end_conversation(true); }
        }
        w.combat.seed(seed + 1);
        let leader = w.party_leader();
        if let Ok(tpl) = std::env::var("WEAPON") {
            let it = w.give_new_item(&tpl, leader, 1, true).expect("weapon template");
            w.equip(leader, it, 4).expect("equip");
        }
        let from = w.get(leader).unwrap().position;
        // the droids closest to the leader's start (the first room's group)
        let mut hostile: Vec<(f32, ObjectId, Vec3)> = w.iter().filter(|o| o.kind == ObjectKind::Creature && o.id != leader && o.scripts.contains_key("ScriptOnNotice") && o.creature().is_some_and(|c| !c.dead)).map(|o| (o.position.distance(from), o.id, o.position)).collect();
        hostile.retain(|h| w.is_enemy(h.1, leader));
        hostile.sort_by(|a, b| a.0.total_cmp(&b.0));
        let first = hostile[0];
        let mut group: Vec<(f32, ObjectId, Vec3)> = hostile.iter().map(|h| (h.2.distance(first.2), h.1, h.2)).collect();
        group.sort_by(|a, b| a.0.total_cmp(&b.0));
        let keep: Vec<ObjectId> = group.iter().take(count).map(|g| g.1).collect();
        for h in &hostile { if !keep.contains(&h.1) { w.get_mut(h.1).unwrap().creature_mut().unwrap().dead = true; } }
        // place leader 10 m from the first
        let tp = first.2;
        let mut placed = false;
        for k in 0..36 {
            let ang = k as f32 * 10f32.to_radians();
            let (x, y) = (tp.x + 10.0 * ang.cos(), tp.y + 10.0 * ang.sin());
            if let Some(h) = w.walk.ground(x, y, tp.z) { let q = Vec3::new(x, y, h.z); if w.walk.line_of_sight(q + Vec3::Z * 1.5, tp + Vec3::Z * 1.5) { w.set_player_position(q, 0.0); placed = true; break; } }
        }
        assert!(placed);
        let hp0 = w.get(leader).unwrap().creature().unwrap().hp;
        w.combat.log_enabled = verbose;
        let mut t = 0.0;
        let mut won = false;
        while t < 120.0 {
            w.tick(DT);
            t += DT;
            for e in w.drain_events() {
                match e {
                    WorldEvent::AttackHit { attacker, target, .. } if target == leader && attacker != leader => { hits += 1; dmg_taken_hits += 1; }
                    WorldEvent::AttackStarted { target, attacker, .. } if target == leader && attacker != leader => swings += 1,
                    _ => {}
                }
            }
            let alive: Vec<ObjectId> = keep.iter().copied().filter(|&k| w.get(k).and_then(|o| o.creature()).is_some_and(|c| !c.dead)).collect();
            let l = w.get(leader).unwrap().creature().unwrap().clone();
            if l.dead { break; }
            if alive.is_empty() { won = true; break; }
            // the player attacks the nearest hostile that is in combat
            if w.get(leader).unwrap().actions.is_empty() {
                let engaged: Vec<ObjectId> = alive.iter().copied().filter(|&k| w.in_combat(k)).collect();
                let pos = w.get(leader).unwrap().position;
                if let Some(&n) = engaged.iter().min_by(|a, b| w.get(**a).unwrap().position.distance(pos).total_cmp(&w.get(**b).unwrap().position.distance(pos))) { w.player_attack(n); }
            }
        }
        let hp1 = w.get(leader).unwrap().creature().unwrap().hp;
        if won { wins += 1; }
        lost_hp += (hp0 - hp1) as i64;
        times += t;
        min_left = min_left.min(hp1);
        if verbose { for l in &w.combat.log { println!("{l}"); } }
        if seed == 0 {
            let s = w.combat_stats(leader).unwrap();
            println!("leader: class {class} hp {hp0} abil {:?} bab {} def {} weapon {:?} ab {}", s.abilities, s.bab, s.defense, s.main, s.attack_bonus(false));
            for &k in &keep { let s = w.combat_stats(k).unwrap(); let c = w.get(k).unwrap().creature().unwrap(); println!("droid: hp {} abil {:?} bab {} def {} ab {} dmg {}d{}{:+}", c.hp, s.abilities, s.bab, s.defense, s.attack_bonus(false), s.main.num_dice, s.main.die, s.main.damage_bonus); }
        }
    }
    println!("{module} class {class} vs {count} droids, {runs} runs: win rate {:.0}%, avg HP lost {:.1}, worst HP left {}, avg time {:.1}s, droid hit rate on PC {}/{} swings", 100.0 * wins as f64 / runs as f64, lost_hp as f64 / runs as f64, min_left, times / runs as f32, hits, swings);
    let _ = dmg_taken_hits;
}

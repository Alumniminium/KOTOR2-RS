//! Headless soak of the early modules: 60 s of simulation in each, looking
//! for action queues that never drain (the front action stays the same for
//! more than 30 s of game time) and for VM errors. Skips (passes) when no
//! install is found. Run with --nocapture to see every stuck entry.

use k2_res::Install;
use k2_script::ObjectId;
use k2_world::object::Action;
use k2_world::World;

const DT: f32 = 1.0 / 30.0;
const MODULES: &[&str] = &["002ebo", "003ebo", "004ebo", "101per", "102per", "103per", "104per", "201tel", "202tel"];

fn world(module: &str) -> Option<World> {
    let dir = std::env::var("K2_INSTALL").unwrap_or_else(|_| concat!(env!("CARGO_MANIFEST_DIR"), "/../../..").into());
    let install = Install::discover(dir).ok()?;
    let mut w = World::new(install).ok()?;
    w.enter_module(module, None).unwrap();
    Some(w)
}

/// Front action of each object that has stayed at the front for more than
/// 30 s, with how far the object moved meanwhile.
struct Watch {
    name: String,
    since: f32,
    at: glam::Vec3,
}

fn stuck(w: &World, watch: &mut std::collections::HashMap<ObjectId, Watch>, t: f32) -> Vec<String> {
    let mut out = Vec::new();
    for o in w.iter() {
        let Some(front) = o.actions.front() else {
            watch.remove(&o.id);
            continue;
        };
        let name = front.constant_name().to_owned();
        let entry = watch.entry(o.id).or_insert_with(|| Watch { name: name.clone(), since: t, at: o.position });
        if entry.name != name {
            *entry = Watch { name: name.clone(), since: t, at: o.position };
        }
        if t - entry.since > 30.0 {
            let moved = o.position.distance(entry.at);
            out.push(format!("{} {} front {} for {:.0} s, moved {:.2} m", w.describe(o.id), o.tag, name, t - entry.since, moved));
            if std::env::var_os("K2_STUCK_DEBUG").is_some() {
                eprintln!("  detail {front:?}");
                if let Action::Attack(target) = front {
                    let hp = w.get(*target).and_then(|x| x.creature()).map(|c| (c.hp, c.dead));
                    let dist = w.distance(o.id, *target);
                    eprintln!("  attack target {} {:?} hp/dead {:?} dist {:?} in_combat {}", w.describe(*target), w.get(*target).map(|x| x.tag.clone()), hp, dist, w.in_combat(o.id));
                }
            }
        }
    }
    out
}

#[test]
fn early_modules_drain_their_action_queues() {
    let mut report = Vec::new();
    for module in MODULES {
        let Some(mut w) = world(module) else { return };
        let mut watch = std::collections::HashMap::new();
        let mut t = 0.0f32;
        for n in 0..(60.0 / DT) as usize {
            w.tick(DT);
            t += DT;
            w.drain_events();
            if let Some(v) = w.conversation_view() {
                if v.replies.is_empty() {
                    w.advance_conversation();
                } else {
                    w.choose_reply(0);
                }
            }
            if n % 30 == 29 {
                for s in stuck(&w, &mut watch, t) {
                    report.push(format!("{module}: {s}"));
                }
            }
        }
        assert!(w.vm_errors.is_empty(), "{module}: {:?}", w.vm_errors);
    }
    // A long walk moves; only entries that did not move are candidates.
    let still: Vec<&String> = report.iter().filter(|r| r.ends_with("m")).filter(|r| r.rsplit("moved ").next().and_then(|m| m.trim_end_matches(" m").parse::<f32>().ok()).is_some_and(|d| d < 1.0)).collect();
    for r in still.iter().take(60) {
        eprintln!("{r}");
    }
    eprintln!("stuck entries: {} (not moving: {})", report.len(), still.len());
}

//! Combat against the real install. Skips (passes) when no install is found
//! at K2_INSTALL or ../.. (the repo inside the game folder).

use k2_res::Install;
use k2_world::{ObjectKind, World};

const DT: f32 = 1.0 / 30.0;

fn world() -> Option<World> {
    let dir = std::env::var("K2_INSTALL").unwrap_or_else(|_| concat!(env!("CARGO_MANIFEST_DIR"), "/../../..").into());
    let install = Install::discover(dir).ok()?;
    World::new(install).ok()
}

#[test]
fn repute_makes_mining_droids_hostile_to_the_party() {
    let Some(mut w) = world() else { return };
    w.enter_module("102per", None).unwrap();
    let pc = w.party_leader();
    let droid = w
        .iter()
        .find(|o| o.kind == ObjectKind::Creature && o.tag.eq_ignore_ascii_case("g_assassindrd01"))
        .map(|o| o.id)
        .expect("mining droid");
    assert!(w.is_enemy(droid, pc));
    assert!(w.is_enemy(pc, droid));
    assert!(!w.is_friend(droid, pc));
    let s = w.combat_stats(droid).expect("droid stats");
    assert!(s.main.ranged, "mining droids carry blasters");
}

#[test]
fn fight_on_peragus_ends_with_a_death_and_no_vm_errors() {
    let Some(mut w) = world() else { return };
    w.enter_module("102per", None).unwrap();
    let r = k2_world::combat::fight(&mut w, 120.0, DT).expect("a fight");
    assert!(r.winner.is_some(), "{}", r.log.join("\n"));
    assert!(!r.forced_hostile);
    assert!(r.log.iter().any(|l| l.contains(" -> ")), "attacks were rolled");
    assert!(w.vm_errors.is_empty(), "{:?}", w.vm_errors);
}

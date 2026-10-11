//! Character generation against the real install. Skips (passes) when no
//! install is found at K2_INSTALL or ../.. (the repo inside the game folder).

use k2_res::Install;
use k2_world::World;
use k2_world::chargen::{CLASS_BUTTONS, POINT_BUY, points_spent};
use k2_world::craft::SKILL_COUNT;

fn world() -> Option<World> {
    let dir = std::env::var("K2_INSTALL").unwrap_or_else(|_| concat!(env!("CARGO_MANIFEST_DIR"), "/../../..").into());
    let install = Install::discover(dir).ok()?;
    World::new(install).ok()
}

#[test]
fn every_class_button_makes_a_valid_level_1_pc() {
    let Some(w0) = world() else { return };
    drop(w0);
    for (gender, class) in CLASS_BUTTONS {
        let mut w = world().unwrap();
        let portraits = w.pc_portraits(gender);
        assert!(portraits.len() >= 10, "gender {gender}: {} portraits", portraits.len());
        let p = portraits[2].clone();
        let mut spec = w.quick_spec(gender, class, Some(&p));
        spec.name = w.random_name(gender);
        assert!(!spec.name.is_empty());
        assert!(points_spent(&spec.abilities.unwrap()) <= POINT_BUY, "class defaults overspend: {:?}", spec.abilities);
        w.check_spec(&spec).unwrap();
        let opts = w.chargen_options(&spec).unwrap();
        assert_eq!(opts.new_level, 1);
        assert!(opts.skill_points >= 2, "level 1 skill points {}", opts.skill_points);
        w.player_spec = spec.clone();
        w.enter_module("101per", None).unwrap();
        let pc = w.player;
        let o = w.get(pc).unwrap().clone();
        let c = o.creature().unwrap().clone();
        assert_eq!(o.name, spec.name);
        assert_eq!(c.gender, gender);
        assert_eq!(c.classes, vec![(class, 1)]);
        assert_eq!(c.appearance, World::appearance_for(&p, class));
        assert!(c.max_hp > 0 && c.max_force > 0, "hp {} fp {}", c.max_hp, c.max_force);
        let ranks: i32 = (0..SKILL_COUNT).map(|s| w.skill_rank(pc, s)).sum();
        assert!(ranks > 0, "no skills bought");
        assert_eq!(w.globals.boolean("000_PLAYER_GENDER"), gender == 0);
        println!(
            "{} {} {:?} hp {} fp {} skills {} feats {:?} powers {:?} app {}",
            spec.name,
            w.class_info(class).0,
            c.abilities,
            c.max_hp,
            c.max_force,
            ranks,
            spec.feats,
            spec.powers,
            c.appearance
        );
    }
}

#[test]
fn overspent_specs_are_refused() {
    let Some(mut w) = world() else { return };
    let mut spec = w.quick_spec(0, 3, None);
    spec.abilities = Some([18; 6]);
    assert!(w.check_spec(&spec).is_err());
    let mut spec = w.quick_spec(0, 3, None);
    spec.skills = Some([4; SKILL_COUNT]);
    assert!(w.check_spec(&spec).is_err());
}

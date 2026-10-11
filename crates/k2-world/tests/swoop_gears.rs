//! Swoop gears are manual: only the accelerate key shifts, and the shift
//! script's gear table (211tel `onaccelerate`) sets each gear's speed range.
//! Skips (passes) when no install is found.

use k2_res::Install;
use k2_world::minigame::MgInput;
use k2_world::World;

const DT: f32 = 1.0 / 30.0;

fn world() -> Option<World> {
    let dir = std::env::var("K2_INSTALL").unwrap_or_else(|_| concat!(env!("CARGO_MANIFEST_DIR"), "/../../..").into());
    let install = Install::discover(dir).ok()?;
    World::new(install).ok()
}

fn gear(w: &World) -> i32 {
    w.globals.numbers.get("min_race_gear").copied().unwrap_or(-1)
}

fn tick(w: &mut World, accelerate: bool) {
    w.minigame_input(MgInput { accelerate, ..Default::default() });
    w.tick(DT);
}

#[test]
fn swoop_gears_shift_only_on_the_key() {
    let Some(mut w) = world() else { return };
    w.enter_module("211tel", None).unwrap();
    assert!(!w.auto_gears, "shifting is manual by default");
    // Countdown (about 5 s) plus a second of racing with no key pressed.
    for _ in 0..(8.0 / DT) as usize {
        tick(&mut w, false);
    }
    let start_gear = gear(&w);
    assert!(start_gear <= 1, "no automatic shift, gear {start_gear}");
    let mut seen = vec![(start_gear, w.minigame.as_ref().unwrap().max_speed)];
    // Shift whenever the gear's top speed is reached.
    for _ in 0..(60.0 / DT) as usize {
        let (speed, max) = {
            let mg = w.minigame.as_ref().unwrap();
            (mg.speed, mg.max_speed)
        };
        let press = gear(&w) == 0 || (max > 0.0 && speed >= max - 1.0);
        tick(&mut w, press);
        let g = gear(&w);
        if seen.last().map(|s| s.0) != Some(g) {
            seen.push((g, w.minigame.as_ref().unwrap().max_speed));
        }
        if w.minigame.is_none() {
            break;
        }
    }
    println!("gears (gear, max speed): {seen:?}");
    // The script's table: gear 1 tops out at 70, gear 2 at 110, gear 3 at 155
    // (a crash drops back to gear 0, so only the first climb is checked).
    assert_eq!(&seen[..4], &[(0, 0.0), (1, 70.0), (2, 110.0), (3, 155.0)], "{seen:?}");
}

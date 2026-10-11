//! `k2world minigame MODULE` and `k2world pazaak MODULE SCRIPT`: headless
//! runs of the swoop/turret minigames and of a scripted pazaak match.

use std::process::ExitCode;

use glam::Vec2;
use k2_res::Install;
use k2_world::minigame::{MgEvent, MgInput};
use k2_world::pazaak::{Phase, TARGET};
use k2_world::{ScriptCtx, World, WorldEvent};

const DT: f32 = 1.0 / 30.0;

/// Plays the module's minigame with a simple autopilot: the turret aims at
/// the nearest live enemy and fires; the swoop steers and jumps (gears shift
/// by themselves). Prints kills, hits, globals and the result.
pub fn minigame(install: &Install, module: &str, seconds: f32, trace: bool) -> ExitCode {
    let mut w = match World::new(install.clone()) {
        Ok(w) => w,
        Err(e) => {
            eprintln!("error: {e}");
            return ExitCode::FAILURE;
        }
    };
    w.trace = trace;
    w.auto_dialog = true;
    w.auto_gears = true;
    if let Err(e) = w.enter_module(module, None) {
        eprintln!("error: {e}");
        return ExitCode::FAILURE;
    }
    let Some(mg) = w.minigame.as_ref() else {
        println!("{module}: no minigame");
        return ExitCode::FAILURE;
    };
    let turret = mg.is_turret();
    println!(
        "{module}: {} with {} enemies, {} obstacles; player track {} ({:.0} units, {:.1} s), camera {}",
        if turret { "turret" } else { "swoop" },
        mg.enemies.len(),
        mg.obstacles.len(),
        mg.player.track.name,
        mg.player.track.distance(),
        mg.player.track.length,
        mg.camera_model
    );
    let mut t = 0.0f32;
    let mut next_report = 0.0f32;
    let mut kills = 0;
    let start_hp = mg.player.hp;
    let mut sound_calls: std::collections::BTreeMap<String, u32> = Default::default();
    let mut last_variance = 0.0f32;
    while t < seconds {
        let mut input = MgInput::default();
        if let Some(mg) = w.minigame.as_ref() {
            if turret {
                // Aim at the nearest live enemy that is in front.
                let p = mg.player.position;
                let target = mg.enemies.iter().filter(|e| e.is_live() && !e.trigger).min_by(|a, b| a.position.distance(p).total_cmp(&b.position.distance(p)));
                if let Some(e) = target {
                    let local = mg.player.rotation.inverse() * (e.position - p).normalize_or_zero();
                    let pitch = local.z.clamp(-1.0, 1.0).asin().to_degrees();
                    let yaw = (-local.x).atan2(local.y).to_degrees();
                    let mut dyaw = (yaw - mg.offset.z + 540.0).rem_euclid(360.0) - 180.0;
                    let mut dpitch = pitch - mg.offset.x;
                    dyaw = dyaw.clamp(-20.0, 20.0);
                    dpitch = dpitch.clamp(-20.0, 20.0);
                    // Feed through the area's mouse axis mapping.
                    let mut aim = Vec2::ZERO;
                    for (axis, slot) in [(mg.mouse_axis_x, 0), (mg.mouse_axis_y, 1)] {
                        let d = match axis {
                            1 => dpitch,
                            3 => dyaw,
                            _ => 0.0,
                        };
                        let d = if (slot == 0 && mg.flip_x) || (slot == 1 && mg.flip_y) { -d } else { d };
                        if slot == 0 {
                            aim.x = d;
                        } else {
                            aim.y = d;
                        }
                    }
                    input.aim = aim;
                    input.fire = dyaw.abs() < 3.0 && dpitch.abs() < 3.0;
                }
            } else {
                // Gears shift by themselves (World::auto_gears, auto_shift).
                // Steer to the nearest lateral offset whose path ahead is
                // clear; jump low barriers and mines just ahead.
                let r = mg.player.sphere_radius + 0.5;
                let look = mg.speed.max(20.0) * 0.8;
                let z = mg.player.position.z;
                let blocked = |x: f32| -> Option<f32> {
                    let mut s = 3.0;
                    while s < look {
                        let at = mg.player_position_ahead(s, x);
                        if let Some((_, _, top)) = mg.obstacle_near(at, r)
                            && top > z + 6.0
                        {
                            return Some(s);
                        }
                        // Mines and rival bikes (non-trigger followers).
                        if mg.enemies.iter().any(|e| e.is_live() && !e.trigger && e.position.distance(at) < r + e.sphere_radius + 1.0) {
                            return Some(s);
                        }
                        s += 3.0;
                    }
                    None
                };
                if blocked(mg.offset.x).is_some() {
                    let mut cands: Vec<f32> = (-8..=8).map(|i| i as f32 * 2.0).collect();
                    cands.sort_by(|a, b| (a - mg.offset.x).abs().total_cmp(&(b - mg.offset.x).abs()));
                    if let Some(x) = cands.into_iter().find(|x| blocked(*x).is_none()) {
                        input.lateral = (x - mg.offset.x).signum();
                    }
                }
                let ahead = mg.player.rotation * glam::Vec3::Y;
                let hurdle = (1..6).any(|i| {
                    mg.obstacle_near(mg.player_position_ahead(i as f32 * mg.speed.max(10.0) * 0.06, mg.offset.x), r).is_some_and(|(_, _, top)| top <= z + 6.0)
                });
                input.brake = hurdle
                    || mg.enemies.iter().any(|e| {
                        let d = e.position - mg.player.position;
                        e.is_live() && !e.trigger && d.dot(ahead) > 0.0 && d.dot(ahead) < mg.speed.max(10.0) * 0.4 && (d - ahead * d.dot(ahead)).length() < 3.0
                    });
            }
        }
        let before = w.minigame.as_ref().map(|m| m.last_obstacle_hit);
        w.minigame_input(input);
        w.tick(DT);
        if trace
            && let Some(mg) = w.minigame.as_ref()
            && Some(mg.last_obstacle_hit) != before
            && let Some(o) = mg.object(mg.last_obstacle_hit)
        {
            let p = mg.player.position;
            let best = o.triangles.iter().min_by(|a, b| {
                let ca = (a[0] + a[1] + a[2]) / 3.0;
                let cb = (b[0] + b[1] + b[2]) / 3.0;
                ca.distance_squared(p).total_cmp(&cb.distance_squared(p))
            });
            let n = best.map(|t| (t[1] - t[0]).cross(t[2] - t[0]).normalize_or_zero());
            println!("[{t:6.2}] hit obstacle {} player {:?} near tri {:?} normal {:?}", o.name, p, best, n);
        }
        for ev in w.drain_events() {
            match ev {
                WorldEvent::SoundObject { object, playing, .. } => {
                    *sound_calls.entry(format!("{} {}", w.get(object).map_or("?", |o| o.tag.as_str()), if playing { "play" } else { "stop" })).or_insert(0) += 1;
                }
                WorldEvent::SoundObjectVolume { object, .. } => {
                    *sound_calls.entry(format!("{} volume", w.get(object).map_or("?", |o| o.tag.as_str()))).or_insert(0) += 1;
                }
                WorldEvent::SoundObjectVariance { object, variance } => {
                    *sound_calls.entry(format!("{} variance", w.get(object).map_or("?", |o| o.tag.as_str()))).or_insert(0) += 1;
                    last_variance = variance;
                    if trace {
                        println!("[{t:6.2}] variance {variance:.2} speed {:.0}", w.minigame.as_ref().map_or(0.0, |m| m.speed));
                    }
                }
                _ => {}
            }
        }
        for ev in w.drain_minigame_events() {
            if trace && let MgEvent::Animation { object, anim, overlay, .. } = &ev {
                let name = w.minigame.as_ref().and_then(|m| m.object(*object)).map_or("?".to_string(), |o| o.name.clone());
                println!("[{t:6.2}] animation {anim} on {object:#x} ({name}) overlay {overlay}");
            }
            if let MgEvent::Animation { object, anim, .. } = &ev
                && anim == "die"
            {
                kills += 1;
                if trace {
                    println!("[{t:6.2}] {object:#x} dies");
                }
            }
        }
        t += DT;
        if t >= next_report {
            next_report += 2.0;
            if let Some(mg) = w.minigame.as_ref() {
                let live = mg.enemies.iter().filter(|e| e.is_live() && !e.trigger).count();
                println!(
                    "[{t:6.2}] pos ({:.0}, {:.0}, {:.0}) speed {:.0} (min {:.0} max {:.0} accel {:.1}) lateral {:.0} tunnel +{:.0}/-{:.0} offset ({:.1}, {:.1}, {:.1}) hp {} live enemies {live} kills {kills} bullets {}",
                    mg.player.position.x,
                    mg.player.position.y,
                    mg.player.position.z,
                    mg.speed,
                    mg.min_speed,
                    mg.max_speed,
                    mg.accel_per_sec,
                    mg.lateral_aps,
                    mg.tunnel_pos.x,
                    mg.tunnel_neg.x,
                    mg.offset.x,
                    mg.offset.y,
                    mg.offset.z,
                    mg.player.hp,
                    mg.bullets.len()
                );
            }
        }
        if let Some(pt) = &w.pending_transition {
            println!("[{t:6.2}] transition -> {} / {:?}", pt.module, pt.waypoint_tag);
            break;
        }
    }
    let globals: Vec<String> = w
        .globals
        .numbers
        .iter()
        .map(|(k, v)| (k.clone(), *v))
        .chain(w.globals.booleans.iter().map(|(k, v)| (k.clone(), i32::from(*v))))
        .filter(|(k, _)| {
            let k = k.to_ascii_uppercase();
            k.contains(&module.to_ascii_uppercase()) || k.starts_with("MIN_") || k.contains("SWOOP") || k.contains("TURRET")
        })
        .map(|(k, v)| format!("{k}={v}"))
        .collect();
    println!("globals: {}", globals.join(" "));
    if let Some(mg) = w.minigame.as_ref() {
        println!("player hp {} -> {}, kills {kills}", start_hp, mg.player.hp);
    }
    println!(
        "sound object calls: {} (last pitch variance {last_variance:.1})",
        sound_calls.iter().map(|(k, v)| format!("{k} x{v}")).collect::<Vec<_>>().join(", ")
    );
    let un: Vec<String> = w.unimplemented.iter().filter(|(k, _)| k.starts_with("SWMG")).map(|(k, v)| format!("{k}x{v}")).collect();
    println!("vm errors {}, unimplemented SWMG: {}", w.vm_errors.len(), if un.is_empty() { "none".into() } else { un.join(" ") });
    for e in w.vm_errors.iter().take(5) {
        println!("  vm: {e}");
    }
    ExitCode::SUCCESS
}

/// `k2world minigame MODULE --audit`: lists collidable followers and
/// obstacles whose model is missing or has no drawn geometry behind its
/// collision volume.
pub fn audit(install: &Install, module: &str) -> ExitCode {
    let mut w = match World::new(install.clone()) {
        Ok(w) => w,
        Err(e) => {
            eprintln!("error: {e}");
            return ExitCode::FAILURE;
        }
    };
    if let Err(e) = w.enter_module(module, None) {
        eprintln!("error: {e}");
        return ExitCode::FAILURE;
    }
    for l in w.audit_minigame() {
        println!("{l}");
    }
    if let Ok(names) = std::env::var("K2_MDL_TREE") {
        for n in names.split(',') {
            println!("== {n}");
            for l in w.mdl_tree(n) {
                println!("{l}");
            }
        }
    }
    ExitCode::SUCCESS
}

/// `k2world minigame MODULE --probe`: no gear input at all, then full-width
/// steering crossings (right, left, right) early in the race and at top
/// speed, timed from the key press to the tunnel edge.
pub fn swoop_probe(install: &Install, module: &str) -> ExitCode {
    let mut w = match World::new(install.clone()) {
        Ok(w) => w,
        Err(e) => {
            eprintln!("error: {e}");
            return ExitCode::FAILURE;
        }
    };
    w.auto_dialog = true;
    w.auto_gears = true;
    if let Err(e) = w.enter_module(module, None) {
        eprintln!("error: {e}");
        return ExitCode::FAILURE;
    }
    if w.minigame.as_ref().is_none_or(|m| m.is_turret()) {
        println!("{module}: not a swoop");
        return ExitCode::FAILURE;
    }
    let mut t = 0.0f32;
    let mut next = 0.0f32;
    // (label, minimum speed to start at)
    let runs = [("start", 1.0f32), ("top speed (speed forced to 200)", 190.0)];
    let mut forced = false;
    let mut run = 0;
    let mut dir = 1.0f32;
    let mut legs = 0;
    let mut leg_t = 0.0f32;
    let mut steering = false;
    let mut speed0 = 0.0;
    while t < 90.0 && run < runs.len() {
        if run == 1 && !forced && w.minigame.as_ref().is_some_and(|m| m.offset.x.abs() < 0.5) {
            // The straight is full of rivals and mines and nobody dodges
            // here: jump to top speed to time the steering.
            forced = true;
            if let Some(mg) = w.minigame.as_mut() {
                mg.speed = 200.0;
                mg.lateral_aps = 200.0;
            }
        }
        let mg = w.minigame.as_ref().unwrap();
        let mut input = MgInput::default();
        // Stop a unit short of the tunnel edge: the walls sit there.
        let limit = mg.tunnel_pos.x.abs().max(mg.tunnel_neg.x.abs()) - 1.0;
        if !steering && mg.speed >= runs[run].1 && mg.lateral_aps > 0.0 && mg.offset.z <= 0.0 && mg.offset.x.abs() < 0.5 && w.pending_transition.is_none() {
            steering = true;
            dir = 1.0;
            legs = 0;
            leg_t = 0.0;
            speed0 = mg.speed;
            println!("[{t:6.2}] {}: steering begins at speed {:.0}, lateral accel {:.0}, lane +-{limit:.0}", runs[run].0, mg.speed, mg.lateral_aps);
        }
        if steering {
            input.lateral = dir;
            leg_t += DT;
            let reached = if dir > 0.0 { mg.offset.x >= limit - 0.01 } else { mg.offset.x <= -limit + 0.01 };
            if reached || leg_t > 6.0 {
                println!(
                    "[{t:6.2}]   leg {} ({}): {:.2} s{}",
                    legs + 1,
                    if dir > 0.0 { "to right edge" } else { "to left edge" },
                    leg_t,
                    if legs == 0 { " (from centre, half width)" } else { " (full width)" }
                );
                legs += 1;
                leg_t = 0.0;
                dir = -dir;
                if legs == 3 {
                    steering = false;
                    println!("          speed {:.0} -> {:.0}", speed0, mg.speed);
                    run += 1;
                }
            }
        }
        if !steering && mg.offset.x.abs() > 0.8 {
            // Back to the middle of the track between runs.
            input.lateral = -mg.offset.x.signum();
        }
        w.minigame_input(input);
        w.tick(DT);
        t += DT;
        if t >= next {
            next += 2.0;
            let mg = w.minigame.as_ref().unwrap();
            println!("[{t:6.2}] speed {:.0} gear {} offset {:.1} hp {}", mg.speed, w.globals.number("MIN_RACE_GEAR"), mg.offset.x, mg.player.hp);
        }
    }
    ExitCode::SUCCESS
}

/// Runs `script` in `module` (a PlayPazaak caller), then plays the match
/// with the AI on both sides and finishes it with `wager` credits.
pub fn pazaak(install: &Install, module: &str, script: &str, wager: i32, param: i32, trace: bool) -> ExitCode {
    let mut w = match World::new(install.clone()) {
        Ok(w) => w,
        Err(e) => {
            eprintln!("error: {e}");
            return ExitCode::FAILURE;
        }
    };
    w.trace = trace;
    if let Err(e) = w.enter_module(module, None) {
        eprintln!("error: {e}");
        return ExitCode::FAILURE;
    }
    w.add_credits(1000);
    let credits0 = w.credits();
    let leader = w.party_leader();
    w.run_script(script, leader, ScriptCtx { params: [param, 0, 0, 0, 0], ..Default::default() });
    w.pump();
    // Callers often defer PlayPazaak (DelayCommand).
    for _ in 0..90 {
        if w.pazaak.is_some() {
            break;
        }
        w.tick(DT);
    }
    let Some(req) = w.pazaak.clone() else {
        println!("{script} did not call PlayPazaak");
        return ExitCode::FAILURE;
    };
    println!(
        "PlayPazaak deck {} end {:?} max wager {} tutorial {} opponent {}",
        req.opponent_deck,
        req.end_script,
        req.max_wager,
        req.tutorial,
        w.describe(req.opponent)
    );
    let side = w.pazaak_default_side_deck();
    println!("player side deck: {}", side.iter().map(|c| c.label()).collect::<Vec<_>>().join(" "));
    println!("opponent deck:    {}", w.pazaak_opponent_deck(req.opponent_deck).iter().map(|c| c.label()).collect::<Vec<_>>().join(" "));
    let mut g = w.pazaak_start(side).expect("pazaak request");
    let mut steps = 0;
    let won = loop {
        match g.phase {
            Phase::Turn => {
                let p = g.turn;
                g.ai_turn(p);
            }
            Phase::SetOver { winner } => {
                println!(
                    "  set {}: {} vs {} -> {}",
                    g.set,
                    g.sides[0].total(),
                    g.sides[1].total(),
                    winner.map_or("tie".into(), |w| if w == 0 { "player".to_string() } else { "opponent".to_string() })
                );
                g.next_set();
            }
            Phase::MatchOver { winner } => {
                println!("  last set: {} vs {} (target {TARGET})", g.sides[0].total(), g.sides[1].total());
                break winner == 0;
            }
        }
        steps += 1;
        if steps > 5000 {
            println!("match did not finish");
            return ExitCode::FAILURE;
        }
    };
    println!("match: {} sets {} - {}", if won { "player wins" } else { "opponent wins" }, g.sides[0].sets_won, g.sides[1].sets_won);
    if trace {
        for l in &g.log {
            println!("    {l}");
        }
    }
    let wager = wager.min(req.max_wager.max(0));
    w.pazaak_finish(won, wager);
    println!(
        "GetLastPazaakResult = {}, credits {credits0} -> {} (wager {wager}), vm errors {}",
        w.last_pazaak,
        w.credits(),
        w.vm_errors.len()
    );
    ExitCode::SUCCESS
}

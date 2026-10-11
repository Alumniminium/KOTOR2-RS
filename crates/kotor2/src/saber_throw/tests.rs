//! Thrown saber tests: flight, catch, the new-area reset and the install's clips.

use std::io::Write;
use super::*;
use bevy::time::TimePlugin;

/// Systems up to the return, with no assets. The caster (object 7) is an
/// entity at the origin, as ObjEntities holds it.
fn app() -> App {
    let mut app = App::new();
    app.add_plugins(MinimalPlugins.build().disable::<TimePlugin>())
        .init_resource::<Time>()
        .init_resource::<FrameEvents>()
        .init_resource::<ObjEntities>()
        .init_resource::<Pending>()
        .init_resource::<Hits>()
        .add_systems(Update, (reset_on_module, queue, launch, fly).chain());
    let caster = app.world_mut().spawn(Transform::default()).id();
    app.world_mut().resource_mut::<ObjEntities>().0.insert(7, caster);
    app
}

fn step(app: &mut App, dt: f32) {
    app.world_mut().resource_mut::<Time>().advance_by(std::time::Duration::from_secs_f32(dt));
    app.update();
    app.world_mut().resource_mut::<FrameEvents>().0.clear();
}

/// Delivers `ev` at time 0, so the times below are exact.
fn fire(app: &mut App, ev: WorldEvent) {
    app.world_mut().resource_mut::<FrameEvents>().0.push(ev);
    step(app, 0.0);
}

fn thrown(app: &mut App) -> Vec<(Vec3, bool)> {
    let world = app.world_mut();
    let mut q = world.query::<(&Thrown, &Transform)>();
    q.iter(world).map(|(t, tf)| (tf.translation, t.caught)).collect()
}

#[test]
fn a_thrown_saber_visits_each_stop_then_comes_back_and_is_caught() {
    let mut app = app();
    // Launch at 0.5 s; two stops, 0.5 s a leg; back 0.5 s; so back at 2.0 s and caught at 1.5 s.
    let (a, b) = (Vec3::new(4.0, 0.0, 1.0), Vec3::new(6.0, 2.0, 1.0)); // game space
    fire(
        &mut app,
        WorldEvent::SaberThrow {
            shooter: 7,
            model: "w_lghtsbr_001".into(),
            stops: vec![a, b],
            launch: 0.5,
            leg: 0.5,
            back: 0.5,
            catch_anim: "CATCH".into(),
            catch_lead: 0.5,
        },
    );
    let hand = Vec3::Y * HAND_HEIGHT;
    step(&mut app, 0.25); // t = 0.25: not yet launched
    assert!(thrown(&mut app).is_empty(), "the saber waits for its launch");

    step(&mut app, 0.5); // t = 0.75: a quarter of the way out to the first stop
    let mid = thrown(&mut app);
    assert_eq!(mid.len(), 1, "one saber in the air");
    assert!((mid[0].0 - hand.lerp(to_bevy(a), 0.5)).length() < 1e-3, "halfway through the first leg: {:?}", mid[0].0);
    assert!(app.world().resource::<Hits>().0.is_empty());

    step(&mut app, 0.5); // t = 1.25: first stop reached at 1.0 s
    assert_eq!(app.world().resource::<Hits>().0.len(), 1, "a hit at the first stop");
    assert!((app.world().resource::<Hits>().0[0] - to_bevy(a)).length() < 1e-3);

    step(&mut app, 0.5); // t = 1.75: the second stop at 1.5 s, and the catch is due at 1.5 s
    assert_eq!(app.world().resource::<Hits>().0.len(), 2, "a hit at the second stop");
    let at = thrown(&mut app);
    assert!(at[0].1, "the catch is due at 1.5 s, 0.5 s before the saber is back");

    step(&mut app, 0.5); // t = 2.25: back at 2.0 s
    assert!(thrown(&mut app).is_empty(), "the saber is back in the hand and removed");
}

#[test]
fn a_single_stop_is_one_leg_out_and_one_back() {
    // Launch 0.0 s, one leg of 1.0 s, back 1.0 s: the hit at 1.0 s and the saber gone at 2.0 s.
    let mut app = app();
    fire(
        &mut app,
        WorldEvent::SaberThrow {
            shooter: 7,
            model: "w_lghtsbr_001".into(),
            stops: vec![Vec3::new(3.0, 0.0, 1.0)],
            launch: 0.0,
            leg: 1.0,
            back: 1.0,
            catch_anim: "CATCH".into(),
            catch_lead: 0.5,
        },
    );
    step(&mut app, 1.1);
    assert_eq!(app.world().resource::<Hits>().0.len(), 1);
    assert_eq!(thrown(&mut app).len(), 1, "still on its way back");
    step(&mut app, 1.0);
    assert!(thrown(&mut app).is_empty());
}

/// The game install's resources, or None with a SKIP line when K2_INSTALL is
/// unset and no install is found there. A K2_INSTALL that is not an install fails.
fn install() -> Option<k2_res::Resources> {
    let set = std::env::var("K2_INSTALL").ok();
    let dir = set.clone().unwrap_or_else(|| concat!(env!("CARGO_MANIFEST_DIR"), "/../../..").into());
    let Ok(install) = k2_res::Install::discover(&dir) else {
        assert!(set.is_none(), "K2_INSTALL={dir} is not a KotOR 2 install");
        let _ = writeln!(std::io::stderr(), "SKIP saber install test: no install at {dir} (set K2_INSTALL)");
        return None;
    };
    Some(k2_res::Resources::open(install).expect("the install opens"))
}

/// Animation names and lengths of a model in the install.
fn clips(res: &k2_res::Resources, model: &str) -> Vec<(String, f32)> {
    let mdl = res.get(&k2_res::ResKey::new(model, rakata_core::ResourceType::Mdl)).ok().flatten().expect("model in the install");
    let mdx = res.get(&k2_res::ResKey::new(model, rakata_core::ResourceType::Mdx)).ok().flatten();
    let m = k2_mdl::read_mdl_from_bytes(&mdl, mdx.as_deref()).expect("model parses");
    m.animations.iter().map(|a| (a.name.clone(), a.length)).collect()
}

#[test]
fn the_saber_spins_in_flight_and_the_caster_catches_it() {
    let Some(res) = install() else { return };
    // The saber model's own throwout clip is the spin; the body's catchsab is the catch.
    // Lengths are k2-mdl's Animation.length, read from each animation header
    // (k2-mdl reader.rs, anim_header_offsets::LENGTH) of the install's models.
    let saber = clips(&res, "w_lghtsbr_001");
    let throwout = saber.iter().find(|(n, _)| n == "throwout").expect("throwout in the saber");
    assert!((throwout.1 - 0.8).abs() < 1e-3, "throwout is the 0.8 s spin, got {}", throwout.1);
    let throwback = saber.iter().find(|(n, _)| n == "throwback").expect("throwback in the saber");
    assert!(throwback.1.abs() < 1e-3, "throwback is a single pose (length 0), got {}", throwback.1);
    let body = clips(&res, "s_male02");
    let catch = body.iter().find(|(n, _)| n == "catchsab").expect("catchsab in s_male02");
    assert!((catch.1 - 0.5).abs() < 1e-3, "catchsab is catchtime long, got {}", catch.1);
    let throw = body.iter().find(|(n, _)| n == "throwsab").expect("throwsab in s_male02");
    // conjtime is 560 ms: the throw clip runs 6.7 ms past the release.
    assert!((throw.1 - 0.5667).abs() < 1e-3, "throwsab is 0.5667 s, got {}", throw.1);
}

#[test]
fn a_new_area_drops_the_sabers_in_the_air() {
    let mut app = app();
    fire(&mut app, shot_saber());
    step(&mut app, 0.6);
    assert_eq!(thrown(&mut app).len(), 1, "a saber in the air");
    fire(&mut app, WorldEvent::ModuleEntered { module: "101per".into() });
    assert!(thrown(&mut app).is_empty(), "the saber is dropped with the area");
    assert!(app.world().resource::<Pending>().0.is_empty());
}

/// A throw by object 7 to one stop 4 m out: launched at once, a hit at 1.0 s, back at 2.0 s.
fn shot_saber() -> WorldEvent {
    WorldEvent::SaberThrow {
        shooter: 7,
        model: "w_lghtsbr_001".into(),
        stops: vec![Vec3::new(4.0, 0.0, 1.0)],
        launch: 0.0,
        leg: 1.0,
        back: 1.0,
        catch_anim: "CATCH".into(),
        catch_lead: 0.5,
    }
}

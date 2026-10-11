//! Client projectile tests: flight, landing, bounce, reset and the flash cache.

use super::*;
use bevy::time::TimePlugin;

/// Systems up to the landing, with no assets: the flight logic alone. The
/// shooter (object 7) is an entity at the origin, as ObjEntities holds it.
fn app() -> App {
    let mut app = App::new();
    app.add_plugins(MinimalPlugins.build().disable::<TimePlugin>())
        .init_resource::<Time>()
        .init_resource::<FrameEvents>()
        .init_resource::<ObjEntities>()
        .init_resource::<Queued>()
        .init_resource::<Landed>()
        .add_systems(Update, (reset_on_module, queue, launch, fly).chain());
    let shooter = app.world_mut().spawn(Transform::default()).id();
    app.world_mut().resource_mut::<ObjEntities>().0.insert(7, shooter);
    app
}

fn step(app: &mut App, dt: f32) {
    app.world_mut().resource_mut::<Time>().advance_by(std::time::Duration::from_secs_f32(dt));
    app.update();
    app.world_mut().resource_mut::<FrameEvents>().0.clear();
}

fn shot(end: ProjectileEnd, to: Vec3) -> WorldEvent {
    WorldEvent::Projectile {
        shooter: 7,
        model: "w_laserfire_r".into(),
        hook: "rhand".into(),
        bolt: true,
        muzzle: None,
        ammo: Some(1),
        to,
        launch: 0.25,
        flight: 0.5,
        arc: false,
        end,
        blast_model: None,
        blast_sound: None,
    }
}

/// Delivers `ev` in this frame (at time 0), so the launch times below are exact.
fn fire(app: &mut App, ev: WorldEvent) {
    app.world_mut().resource_mut::<FrameEvents>().0.push(ev);
    step(app, 0.0);
}

fn flights(app: &mut App) -> Vec<Vec3> {
    let world = app.world_mut();
    let mut q = world.query::<(&Flight, &Transform)>();
    q.iter(world).map(|(_, t)| t.translation).collect()
}

#[test]
fn a_bolt_event_flies_to_the_target_and_is_gone_on_landing() {
    let mut app = app();
    let to = Vec3::new(10.0, 0.0, 1.0); // game space: 10 m along +X, chest height
    let to_b = to_bevy(to);
    fire(&mut app, shot(ProjectileEnd::Impact, to));

    step(&mut app, 0.1);
    assert!(flights(&mut app).is_empty(), "the bolt waits for its launch time");

    step(&mut app, 0.2); // t = 0.3: 0.05 s into a 0.5 s flight
    let mid = flights(&mut app);
    assert_eq!(mid.len(), 1, "one bolt in the air");
    // Launched from the shooter's chest height (no weapon hook in the test).
    let from = Vec3::Y * HOOK_HEIGHT;
    assert!((mid[0] - (from + (to_b - from) * 0.1)).length() < 1e-3, "a tenth of the way: {:?}", mid[0]);

    step(&mut app, 0.6); // t = 0.9, past the landing at 0.75 s
    assert!(flights(&mut app).is_empty(), "the bolt is despawned on landing");
    let landed = &app.world().resource::<Landed>().0;
    assert_eq!(landed.len(), 1);
    assert_eq!(landed[0].end, ProjectileEnd::Impact);
    assert!((landed[0].at - to_b).length() < 1e-3, "lands on the target's chest");
}

#[test]
fn a_deflected_bolt_bounces_back_then_is_gone() {
    let mut app = app();
    // The shooter (object 7, not in ObjEntities) launches from the origin.
    fire(&mut app, shot(ProjectileEnd::Deflect, Vec3::new(2.0, 0.0, 1.0)));
    step(&mut app, 0.3); // launched at 0.25 s, 0.05 s into its flight
    step(&mut app, 0.5); // t = 0.8: landed on the saber at 0.75 s, bouncing
    assert_eq!(flights(&mut app).len(), 1, "the bolt bounces instead of ending");
    // The saber point is 2 m from the shooter's hook: the bounce is the distance back (2 m), not 12 m.
    let bounce = {
        let world = app.world_mut();
        let mut q = world.query::<&Flight>();
        q.iter(world).map(|f| (f.to - f.from).length()).next().expect("the bounce")
    };
    assert!((bounce - 2.0).abs() < 0.05, "the bounce is the distance back to the shooter's hook: {bounce}");
    assert_eq!(app.world().resource::<Landed>().0.iter().filter(|l| l.end == ProjectileEnd::Deflect).count(), 1);
    step(&mut app, 0.5); // t = 1.3: the bounce (0.4 s) is over
    assert!(flights(&mut app).is_empty(), "the bounce ends in a miss and is removed");
}

#[test]
fn a_missed_bolt_flies_past_and_leaves_no_landing() {
    let mut app = app();
    fire(&mut app, shot(ProjectileEnd::Miss, Vec3::new(16.0, 0.0, 1.0)));
    step(&mut app, 0.5); // t = 0.5: halfway along its 0.5 s flight
    assert_eq!(flights(&mut app).len(), 1);
    step(&mut app, 0.6); // t = 1.1: well past the target and gone
    assert!(flights(&mut app).is_empty());
    assert!(app.world().resource::<Landed>().0.is_empty(), "a miss makes no impact");
}

#[test]
fn flashes_share_one_sphere_and_one_material_per_colour() {
    let mut fx = FxCache::default();
    let mut meshes = Assets::<Mesh>::default();
    let mut smats = Assets::<StandardMaterial>::default();
    let (a, b) = (flash_mesh(&mut fx, &mut meshes), flash_mesh(&mut fx, &mut meshes));
    assert_eq!(a.id(), b.id());
    assert_eq!(meshes.len(), 1);
    let red = flash_material(&mut fx, &mut smats, Color::srgb(1.0, 0.0, 0.0));
    let red_again = flash_material(&mut fx, &mut smats, Color::srgb(1.0, 0.0, 0.0));
    let blue = flash_material(&mut fx, &mut smats, Color::srgb(0.0, 0.0, 1.0));
    assert_eq!(red.id(), red_again.id());
    assert_ne!(red.id(), blue.id());
    assert_eq!(smats.len(), 2, "one material per colour, not per flash");
}

#[test]
fn a_new_area_drops_the_flights_and_the_launch_queue() {
    let mut app = app();
    // A bolt in flight, and a second one still waiting to launch.
    fire(&mut app, shot(ProjectileEnd::Impact, Vec3::new(10.0, 0.0, 1.0)));
    step(&mut app, 0.3);
    assert_eq!(flights(&mut app).len(), 1, "one bolt in the air");
    app.world_mut().resource_mut::<Queued>().0.push(Due {
        at: 99.0,
        shooter: 7,
        model: "w_laserfire_r".into(),
        hand: "rhand".into(),
        bolt: true,
        muzzle: None,
        to: Vec3::ZERO,
        flight: 0.5,
        arc: false,
        end: ProjectileEnd::Impact,
        blast_model: None,
        blast_sound: None,
    });
    fire(&mut app, WorldEvent::ModuleEntered { module: "101per".into() });
    assert!(flights(&mut app).is_empty(), "the flight is dropped with the area");
    assert!(app.world().resource::<Queued>().0.is_empty(), "and so is the launch still waiting");
}


//! Thrown lightsaber (spells 4 and 49; k2-world `WorldEvent::SaberThrow`).
//!
//! The saber leaves the caster's right hand and flies to each stop of the
//! throw, with a flash at each stop. Then it comes back to the hand. In flight
//! it is the saber's own item model with its `throwout` clip looping, which is
//! the spin the model carries (the model's own clips only turn it; the path is
//! ours). The held saber is hidden while it is out. The caster plays the catch
//! clip `catch_lead` seconds before the saber is back in the hand.
//!
//! Guesses, marked for the user to judge: the return goes back to the hand's
//! position at launch, and the catch clip ends as the saber lands (catchtime
//! 500 ms equals catchsab's length, which is why the catch is played that way).
//!
//! Not here: the damage. The world applies it (the cast state machine in
//! powers.rs and saber_hit in projectiles.rs); the impact script's
//! EffectLightsaberThrow stays a recorded no-op. The blade is drawn by model.rs
//! (saber_blade) for every saber model, held or thrown.

use bevy::prelude::*;
use k2_script::ObjectId;
use k2_world::WorldEvent;

use crate::anim::{AnimBindings, AnimPlayer, CreatureSpec, spawn_creature};
use crate::coords::to_bevy;
use crate::game::{ApplyEvents, CreatureAnim, FrameEvents, GameSet, ObjBody, ObjEntities};
use crate::projectiles::{Art, FxCache, burst};

/// Hand height above the feet when there is no hand node to read (m).
const HAND_HEIGHT: f32 = 1.1;
/// The saber's own clip that spins it in flight.
const SPIN_CLIP: &str = "throwout";
/// Flash colour and light where the saber touches a stop.
const HIT_COLOR: (f32, f32, f32) = (0.5, 0.8, 1.0);
const HIT_LIGHT: f32 = 20_000.0;

pub struct SaberThrowPlugin;

impl Plugin for SaberThrowPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<Pending>()
            .init_resource::<Hits>()
            .add_systems(Update, (reset_on_module, queue, launch, fly, dress, spin, flash_hits).chain().in_set(GameSet::Sync).after(ApplyEvents));
    }
}

/// A thrown saber the world has launched, waiting for its launch time.
struct Due {
    at: f32,
    shooter: ObjectId,
    model: String,
    /// Bevy space.
    stops: Vec<Vec3>,
    leg: f32,
    back: f32,
    catch_anim: String,
    catch_lead: f32,
}

#[derive(Resource, Default)]
struct Pending(Vec<Due>);

/// Stops the sabers have reached this frame (bevy space), drained by `flash_hits`.
#[derive(Resource, Default)]
pub struct Hits(pub Vec<Vec3>);

/// A saber in the air. Its transform is its position in Bevy space.
#[derive(Component)]
struct Thrown {
    model: String,
    /// The shooter's object root: the catch and the hidden hand live there.
    root: Entity,
    /// Waypoints (time, point): the hand, each stop, then the hand again.
    path: Vec<(f32, Vec3)>,
    /// (time, point) of each stop, a hit when reached.
    hits: Vec<(f32, Vec3)>,
    next_hit: usize,
    /// The held saber models hidden while this one is out.
    hidden: Vec<Entity>,
    catch_clip: Option<&'static str>,
    catch_at: f32,
    caught: bool,
    end: f32,
}

/// The saber model of a thrown saber is spawned; its spin starts when its AnimPlayer exists.
#[derive(Component)]
struct PendingSpin;

/// The clip a spells.2da catchanim names. "CATCH" is the saber catch clip of
/// the S_Male02 body (catchsab, 0.50 s).
fn catch_clip(anim: &str) -> Option<&'static str> {
    anim.eq_ignore_ascii_case("catch").then_some("catchsab")
}

/// A new area drops the throws waiting to launch, the hits not yet shown, and
/// the sabers in the air (they are not under the area root, so they would stay).
fn reset_on_module(mut commands: Commands, events: Res<FrameEvents>, mut pending: ResMut<Pending>, mut hits: ResMut<Hits>, thrown: Query<Entity, With<Thrown>>) {
    if !events.0.iter().any(|e| matches!(e, WorldEvent::ModuleEntered { .. })) {
        return;
    }
    pending.0.clear();
    hits.0.clear();
    for e in &thrown {
        commands.entity(e).despawn();
    }
}

fn queue(time: Res<Time>, events: Res<FrameEvents>, mut pending: ResMut<Pending>) {
    let now = time.elapsed_secs();
    for ev in &events.0 {
        let WorldEvent::SaberThrow { shooter, model, stops, launch, leg, back, catch_anim, catch_lead } = ev else { continue };
        pending.0.push(Due {
            at: now + launch,
            shooter: *shooter,
            model: model.clone(),
            stops: stops.iter().map(|p| to_bevy(*p)).collect(),
            leg: *leg,
            back: *back,
            catch_anim: catch_anim.clone(),
            catch_lead: *catch_lead,
        });
    }
}

/// Sends due sabers out: the path, the hits, and the held saber hidden.
fn launch(
    mut commands: Commands,
    time: Res<Time>,
    mut pending: ResMut<Pending>,
    ents: Res<ObjEntities>,
    roots: Query<(&ObjBody, Option<&crate::weapons::HeldWeapons>)>,
    bindings: Query<&AnimBindings>,
    kids: Query<&Children>,
    transforms: Query<&GlobalTransform>,
) {
    let now = time.elapsed_secs();
    let (due, wait): (Vec<Due>, Vec<Due>) = std::mem::take(&mut pending.0).into_iter().partition(|d| d.at <= now);
    pending.0 = wait;
    for d in due {
        let Some(root) = ents.0.get(&d.shooter).copied() else { continue };
        let Ok(origin) = transforms.get(root).map(|t| t.translation()) else { continue };
        // The right hand: its node holds the held weapon, which is hidden while the saber is out.
        let hand_node = roots.get(root).ok().and_then(|(b, _)| b.0).and_then(|body| bindings.get(body).ok()).and_then(|b| b.entity("rhand"));
        let hand = hand_node.and_then(|n| transforms.get(n).ok()).map_or(origin + Vec3::Y * HAND_HEIGHT, |t| t.translation());
        let hidden: Vec<Entity> = hand_node.and_then(|n| kids.get(n).ok()).map(|c| c.iter().collect()).unwrap_or_default();

        let start = d.at;
        let mut path = vec![(start, hand)];
        let mut hits = Vec::new();
        for (i, stop) in d.stops.iter().enumerate() {
            let t = start + d.leg * (i + 1) as f32;
            path.push((t, *stop));
            hits.push((t, *stop));
        }
        let end = start + d.leg * d.stops.len() as f32 + d.back;
        path.push((end, hand));
        for &e in &hidden {
            // try_insert: the hand may have been dropped with its area since the query.
            commands.entity(e).try_insert(Visibility::Hidden);
        }
        commands.spawn((
            Name::new("thrown saber"),
            Transform::from_translation(hand),
            Visibility::default(),
            Thrown {
                model: d.model,
                root,
                path,
                hits,
                next_hit: 0,
                hidden,
                catch_clip: catch_clip(&d.catch_anim),
                catch_at: end - d.catch_lead,
                caught: false,
                end,
            },
        ));
    }
}

/// Position on the path at `now` (linear between waypoints, held at the ends).
fn sample(path: &[(f32, Vec3)], now: f32) -> Vec3 {
    for w in path.windows(2) {
        let ((t0, p0), (t1, p1)) = (w[0], w[1]);
        if now <= t1 {
            return p0.lerp(p1, ((now - t0) / (t1 - t0).max(1e-4)).clamp(0.0, 1.0));
        }
    }
    path.last().map_or(Vec3::ZERO, |p| p.1)
}

/// Moves the sabers along their paths, reports stops reached, plays the catch
/// clip when due, and removes a saber (restoring the held one) when it is back.
fn fly(
    mut commands: Commands,
    time: Res<Time>,
    mut hits: ResMut<Hits>,
    mut thrown: Query<(Entity, &mut Thrown, &mut Transform)>,
    mut roots: Query<(&ObjBody, Option<&mut CreatureAnim>)>,
    mut players: Query<&mut AnimPlayer>,
) {
    let now = time.elapsed_secs();
    for (e, mut t, mut tf) in &mut thrown {
        tf.translation = sample(&t.path, now);
        while t.next_hit < t.hits.len() && now >= t.hits[t.next_hit].0 {
            hits.0.push(t.hits[t.next_hit].1);
            t.next_hit += 1;
        }
        if !t.caught && now >= t.catch_at {
            t.caught = true;
            if let (Some(clip), Ok((body, anim))) = (t.catch_clip, roots.get_mut(t.root))
                && let Some(b) = body.0
                && let Ok(mut p) = players.get_mut(b)
                && p.set.has(clip)
            {
                p.fade_to(clip, false, 1.0, 0.1);
                // Held as scripted so the idle does not take over until the catch ends.
                if let Some(mut a) = anim {
                    a.scripted = Some((clip.to_owned(), false, None));
                }
            }
        }
        if now >= t.end {
            for &h in &t.hidden {
                commands.entity(h).try_insert(Visibility::Inherited);
            }
            commands.entity(e).despawn();
        }
    }
}

/// Gives each new thrown saber its model (the saber's own item model, with its clips).
fn dress(mut commands: Commands, mut art: Art, fresh: Query<(Entity, &Thrown), Added<Thrown>>) {
    for (e, t) in &fresh {
        let spec = CreatureSpec { body: t.model.clone(), head: None, texture: None, own_idle: false };
        if let Some(body) = spawn_creature(&mut commands, &mut art.loader(), &spec, e, Transform::IDENTITY) {
            commands.entity(body).insert(PendingSpin);
        }
    }
}

/// Starts the spin once the saber's AnimPlayer exists.
fn spin(mut commands: Commands, mut q: Query<(Entity, &mut AnimPlayer), With<PendingSpin>>) {
    for (e, mut p) in &mut q {
        p.play(SPIN_CLIP, true, 1.0);
        commands.entity(e).remove::<PendingSpin>();
    }
}

fn flash_hits(mut commands: Commands, time: Res<Time>, mut hits: ResMut<Hits>, mut art: Art, mut fx: ResMut<FxCache>) {
    let now = time.elapsed_secs();
    let color = Color::srgb(HIT_COLOR.0, HIT_COLOR.1, HIT_COLOR.2);
    for at in std::mem::take(&mut hits.0) {
        burst(&mut commands, &mut art, &mut fx, at, now, color, HIT_LIGHT);
    }
}

#[cfg(test)]
mod tests;

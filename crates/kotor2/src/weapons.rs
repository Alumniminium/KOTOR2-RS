//! Equipped weapons in hand, and creature respawn on armour changes.
//!
//! A creature's right/left weapon (INVENTORY_SLOT_RIGHTWEAPON 4 /
//! LEFTWEAPON 5) is drawn as its item model (k2-world `ItemDef::model`,
//! e.g. w_blstrpstl_001) parented to the body's `rhand` / `lhand` hook
//! node, so it follows the animation. Attached once the body's
//! `AnimBindings` exist, and rebuilt on `EquipmentChanged`.
//! `AppearanceChanged` (new armour = new body model) despawns the whole
//! object root; `sync_objects` respawns it with the new body next frame.
//!
//! X swaps the party leader's two weapon sets (`World::swap_weapon_sets`):
//! the live pair in slots 4/5 trades with the stored pair, the
//! EquipmentChanged events rebuild the held models. Ignored in dialog,
//! menus and pause.
//!
//! Drawing: a weapon change in the hands (equip, unequip, set swap; the
//! world marks it with `World::weapon_busy`, which also holds attacks for
//! `EQUIP_SECONDS`) plays the creature's `equip` clip (S_Male02, 1 s, with a
//! `draw_weapon` event at 0.57 s). The old models vanish when the clip
//! starts (a saber plays its baseitems powerdownsnd), the new ones appear
//! when the clip reaches the event (powerupsnd). Creatures without the clip,
//! and changes made while paused (a menu is open), swap models at once. The
//! draw sounds play in both cases.
//!
//! Test hook: `--give TEMPLATE[,TEMPLATE...]` creates the items for the
//! party once the game runs and equips what fits (weapons in the right
//! hand, armour on the body). A `@2` suffix puts a weapon in the second
//! weapon set instead. `K2_SWAP_WEAPONS=N` presses X once at frame N.

use bevy::prelude::*;
use k2_script::ObjectId;
use k2_world::WorldEvent;

use crate::anim::{AnimBindings, AnimPlayer};
use crate::audio::Voice;
use crate::game::{AppState, ApplyEvents, CreatureAnim, FrameEvents, GameSet, InConversation, ObjBody, ObjEntities, ObjId, Paused, PendingBarks, WorldRes};
use crate::gui::menus::{OpenMenu, PauseState};
use crate::uisfx::UiSfx;
use crate::material::KMaterial;
use crate::model::{self, AssetCache, GameRes, Loader};

const SLOT_RIGHTWEAPON: i32 = 4;
const SLOT_LEFTWEAPON: i32 = 5;

/// Flags owned here, each taking one value (main's parser skips them).
pub const FLAGS: [&str; 1] = ["--give"];

/// Weapon model roots currently attached to this object's hands.
/// The second field is the baseitems powerdownsnd of what is held (lightsaber
/// off sound when it is put away).
#[derive(Component, Default)]
pub struct HeldWeapons(pub Vec<Entity>, pub Option<String>);

/// The equip clip is playing; the new models attach at its draw_weapon event.
#[derive(Component)]
struct PendingDraw;

/// Event name in the equip clip, and where to draw when a clip lacks it.
const DRAW_EVENT: &str = "draw_weapon";
const DRAW_FALLBACK: f32 = 0.55;

pub struct WeaponsPlugin;

impl Plugin for WeaponsPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Update, (give_test_items, respawn_on_appearance, start_draw, finish_draw, attach_weapons).chain().in_set(GameSet::Sync).after(ApplyEvents))
            .add_systems(Update, swap_weapons_key.in_set(GameSet::Input).run_if(in_state(AppState::Game)));
    }
}

/// `--give`: hand the party items and equip them (testing).
fn give_test_items(mut world: ResMut<WorldRes>, mut done: Local<bool>) {
    if *done {
        return;
    }
    let argv: Vec<String> = std::env::args().collect();
    let Some(list) = argv.iter().position(|a| a == "--give").and_then(|i| argv.get(i + 1)) else {
        *done = true;
        return;
    };
    let w = &mut world.0;
    let leader = w.party_leader();
    if w.get(leader).is_none() {
        return;
    }
    *done = true;
    for entry in list.split(',') {
        let (template, second) = entry.strip_suffix("@2").map_or((entry, false), |t| (t, true));
        let Some(item) = w.give_new_item(template, leader, 1, false) else {
            warn!("--give {template}: no such item");
            continue;
        };
        let slot = w.item_def_of(item).and_then(|d| [SLOT_RIGHTWEAPON, 1, 0, 3, 10, 9, 7, 8].into_iter().find(|&s| d.fits(s)));
        let slot = if second && slot == Some(SLOT_RIGHTWEAPON) { Some(w.weapon_set_slots(leader, 1).0) } else { slot };
        if let Some(slot) = slot {
            let r = w.equip(leader, item, slot);
            info!("--give {template}: equip slot {slot}: {r:?}");
        }
    }
}

/// X: swap the leader's weapon sets, with a floating line and a click.
#[allow(clippy::too_many_arguments)]
fn swap_weapons_key(
    keys: Res<ButtonInput<KeyCode>>,
    conv: Res<InConversation>,
    open_menu: Res<OpenMenu>,
    pause: Res<PauseState>,
    paused: Res<Paused>,
    mut world: ResMut<WorldRes>,
    mut barks: ResMut<PendingBarks>,
    mut sfx: MessageWriter<UiSfx>,
    mut frame: Local<u32>,
) {
    *frame += 1;
    let hook = std::env::var("K2_SWAP_WEAPONS").ok().and_then(|v| v.parse::<u32>().ok()).is_some_and(|n| n == *frame);
    if !(keys.just_pressed(KeyCode::KeyX) || hook) || conv.0 || open_menu.0.is_some() || pause.user || pause.menu || pause.editor || paused.0 {
        return;
    }
    let w = &mut world.0;
    let leader = w.party_leader();
    if w.swap_weapon_sets(leader) {
        info!("{} switches to weapon set {}", w.describe(leader), w.weapon_set(leader) + 1);
        barks.0.push((leader, format!("Weapon Set {}", w.weapon_set(leader) + 1)));
        sfx.write(UiSfx("Inventory_Select"));
    } else {
        barks.0.push((leader, "No other weapon set".into()));
        sfx.write(UiSfx("Error_Default"));
    }
}

/// New armour changes the body model: drop the mirrored entity so it is
/// rebuilt from the world's current body/texture.
fn respawn_on_appearance(mut commands: Commands, events: Res<FrameEvents>, mut ents: ResMut<ObjEntities>) {
    for e in &events.0 {
        if let WorldEvent::AppearanceChanged { object } = e
            && let Some(root) = ents.0.remove(object)
        {
            commands.entity(root).despawn();
        }
    }
}

/// baseitems sound column (powerupsnd / powerdownsnd) of an item.
fn base_sound(w: &mut k2_world::World, item: ObjectId, column: &str) -> Option<String> {
    let row = w.item_def_of(item)?.base_item as usize;
    let t = w.tables.get(&w.res, "baseitems").ok()?;
    t.resref(row, column).filter(|r| !r.is_empty() && !r.starts_with("**"))
}

/// The first held weapon's baseitems sound in `column` (powerupsnd when
/// drawn, powerdownsnd when put away).
fn hand_sound(w: &mut k2_world::World, creature: ObjectId, column: &str) -> Option<String> {
    let items: Vec<ObjectId> = w.equipment(creature).into_iter().filter(|&(s, _)| s == SLOT_RIGHTWEAPON || s == SLOT_LEFTWEAPON).map(|(_, i)| i).collect();
    items.into_iter().find_map(|i| base_sound(w, i, column))
}

/// A hand slot changed while the world says the creature is drawing: put
/// the old models away and start the equip clip.
///
/// No clip runs while the game is paused (a menu is open: the clock is
/// stopped), and none exists on some bodies (Darth Malak, Gammorean, ...).
/// In both cases the models swap at once in `attach_weapons`, and the draw
/// sounds play here, paused or not, so scripted and menu equips sound the same.
#[allow(clippy::too_many_arguments)]
fn start_draw(
    mut commands: Commands,
    time: Res<Time>,
    events: Res<FrameEvents>,
    paused: Res<Paused>,
    mut world: ResMut<WorldRes>,
    ents: Res<ObjEntities>,
    mut roots: Query<(&ObjBody, &HeldWeapons, Option<&mut CreatureAnim>, Has<PendingDraw>)>,
    mut players: Query<&mut AnimPlayer>,
    mut voice: Voice,
) {
    let w = &mut world.0;
    let mut done: Vec<ObjectId> = Vec::new();
    for e in &events.0 {
        let WorldEvent::EquipmentChanged { creature, slot } = e else { continue };
        if (*slot != SLOT_RIGHTWEAPON && *slot != SLOT_LEFTWEAPON) || done.contains(creature) || w.weapon_busy(*creature) <= 0.0 {
            continue;
        }
        let Some(&root) = ents.0.get(creature) else { continue };
        let Ok((body, held, ca, drawing)) = roots.get_mut(root) else { continue };
        // A draw under way absorbs the change: finish_draw builds the current
        // equipment. While paused its clip is frozen, so the models swap now.
        if drawing && !paused.0 {
            continue;
        }
        let Some(mut p) = body.0.and_then(|b| players.get_mut(b).ok()) else { continue };
        // A change while the equip clip still plays past its draw point starts
        // it over (one before that point is absorbed above).
        let restart = p.current_clip().is_some_and(|(c, t)| c.name == "equip" && t > 0.0);
        if paused.0 || !p.fade_to("equip", false, 1.0, 0.1) {
            done.push(*creature);
            if drawing {
                commands.entity(root).remove::<(PendingDraw, HeldWeapons)>();
            }
            if let Some(down) = &held.1 {
                voice.play_sound_at(down, root, Vec3::Y, DRAW_RANGE);
            }
            if let Some(up) = hand_sound(w, *creature, "powerupsnd") {
                voice.play_sound_at(&up, root, Vec3::Y, DRAW_RANGE);
            }
            continue;
        }
        if restart {
            p.seek(0.0);
        }
        done.push(*creature);
        if let Some(mut a) = ca {
            a.scripted = Some(("equip".into(), false, None));
        }
        for &m in &held.0 {
            commands.entity(m).despawn();
        }
        if let Some(down) = &held.1 {
            voice.play_sound_at(down, root, Vec3::Y, DRAW_RANGE);
        }
        let length = p.current_clip().map_or(0.0, |(c, _)| c.length);
        info!("{} equip clip starts at {:.2} s (length {length:.2})", w.describe(*creature), time.elapsed_secs());
        commands.entity(root).insert((HeldWeapons(Vec::new(), None), PendingDraw));
    }
}

/// Spatial range (full, silent) of the saber on/off sounds, metres.
const DRAW_RANGE: (f32, f32) = (4.0, 30.0);

/// Once the equip clip reaches its draw_weapon event (or stops playing,
/// e.g. the creature walked off) let `attach_weapons` build the new models.
#[allow(clippy::too_many_arguments)]
fn finish_draw(
    mut commands: Commands,
    time: Res<Time>,
    mut world: ResMut<WorldRes>,
    roots: Query<(Entity, &ObjId, &ObjBody), With<PendingDraw>>,
    players: Query<&AnimPlayer>,
    mut voice: Voice,
) {
    let w = &mut world.0;
    for (root, id, body) in &roots {
        let Some(p) = body.0.and_then(|b| players.get(b).ok()) else { continue };
        let (ready, at) = match p.current_clip() {
            Some((c, t)) if c.name == "equip" => {
                let event = c.events.iter().find(|(_, n)| n == DRAW_EVENT).map_or(c.length * DRAW_FALLBACK, |(t, _)| *t);
                (t >= event, t)
            }
            _ => (true, -1.0),
        };
        if !ready {
            continue;
        }
        info!("{} draws at {:.2} s (clip time {at:.2}, playing {:?})", w.describe(id.0), time.elapsed_secs(), p.current());
        if let Some(up) = hand_sound(w, id.0, "powerupsnd") {
            voice.play_sound_at(&up, root, Vec3::Y, DRAW_RANGE);
        }
        commands.entity(root).remove::<(PendingDraw, HeldWeapons)>();
    }
}

#[allow(clippy::too_many_arguments)]
fn attach_weapons(
    mut commands: Commands,
    events: Res<FrameEvents>,
    mut world: ResMut<WorldRes>,
    game: Res<GameRes>,
    mut cache: ResMut<AssetCache>,
    mut images: ResMut<Assets<Image>>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<KMaterial>>,
    roots: Query<(Entity, &ObjId, &ObjBody, Option<&HeldWeapons>, Has<PendingDraw>)>,
    bindings: Query<&AnimBindings>,
) {
    let changed: Vec<ObjectId> = events
        .0
        .iter()
        .filter_map(|e| match e {
            WorldEvent::EquipmentChanged { creature, slot } if *slot == SLOT_RIGHTWEAPON || *slot == SLOT_LEFTWEAPON => Some(*creature),
            _ => None,
        })
        .collect();
    let w = &mut world.0;
    let mut loader = Loader { res: &game.0, cache: &mut cache, images: &mut images, meshes: &mut meshes, materials: &mut materials };
    for (root, id, body, held, drawing) in &roots {
        if drawing || (held.is_some() && !changed.contains(&id.0)) {
            continue;
        }
        if w.get(id.0).and_then(|o| o.creature()).is_none() {
            continue;
        }
        // The body's bindings appear a frame after spawn.
        let Some(b) = body.0.and_then(|e| bindings.get(e).ok()) else { continue };
        if let Some(h) = held {
            for &e in &h.0 {
                commands.entity(e).despawn();
            }
        }
        let mut attached = Vec::new();
        for (slot, item) in w.equipment(id.0) {
            let hook = match slot {
                SLOT_RIGHTWEAPON => "rhand",
                SLOT_LEFTWEAPON => "lhand",
                _ => continue,
            };
            let Some(model_name) = w.item_def_of(item).and_then(|d| d.model.clone()) else { continue };
            let Some(node) = b.entity(hook) else {
                debug!("{}: no {hook} node for {model_name}", w.describe(id.0));
                continue;
            };
            let Some(t) = loader.model(&model_name) else {
                warn!("weapon model {model_name} not loadable");
                continue;
            };
            attached.push(model::spawn_template(&mut commands, &t, node, Transform::IDENTITY));
            info!("{} holds {model_name} in {hook}", w.describe(id.0));
        }
        let power_down = hand_sound(w, id.0, "powerdownsnd");
        commands.entity(root).insert(HeldWeapons(attached, power_down));
    }
}

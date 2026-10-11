//! The in-game HUD: K2's `mipc28x6_p` layout. Party portraits with
//! vitality/force bars (bottom right), the menu button bar (top right), the
//! action bar and mode toggles (bottom), and the target name and health
//! bar floating over the current target.
//!
//! Controls the engine only shows on events (level up, combat queue,
//! item received, light/dark shift, ...) start hidden. The minimap is
//! hidden until area maps are implemented.

use bevy::prelude::*;
use k2_script::ObjectId;
use k2_world::World;

use super::{GuiScale, GuiScreen, GuiSet, Placement, open_screen};
use crate::area::Interactable;
use crate::game::{AppState, InConversation, ObjId, WorldRes};
use crate::gui::menus::OpenMenu;
use crate::hover::Target;
use crate::hud::LastTarget;
use crate::model::GameRes;

pub struct GuiHudPlugin;

impl Plugin for GuiHudPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(OnEnter(AppState::Game), spawn_hud)
            .add_systems(OnExit(AppState::Game), despawn_hud)
            .add_systems(Update, (update_party, update_target).in_set(GuiSet::Logic).run_if(in_state(AppState::Game)));
    }
}

pub const HUD: &str = "mipc28x6_p";

#[derive(Component)]
pub struct GameHud;

/// Shown only when the engine has something to say.
const EVENT_ONLY: &[&str] = &[
    "LBL_CMBTMSGBG",
    "LBL_CMBTMODEMSG",
    "LBL_COMBATBG",
    "BTN_CLEAR",
    "LBL_QUEUE",
    "LBL_DISABLE",
    "LBL_DEBILATATED",
    "LBL_LEVELUP",
    "LBL_CMBTEFCT",
    "LBL_STEALTHXP",
    "LBL_DARKSHIFT",
    "LBL_LIGHTSHIFT",
    "LBL_JOURNAL",
    "LBL_CASH",
    "LBL_PLOTXP",
    "LBL_ITEMRCVD",
    "LBL_ITEMLOST",
    "LBL_ACTIONDESC",
    // Target name / health group and the target action menu.
    "LBL_NAMEBG",
    "LBL_NAME",
    "LBL_HEALTHBG",
    "PB_HEALTH",
    "BTN_TARGET",
    "LBL_TARGET",
    // Minimap (no area maps yet).
    "LBL_MAP",
    "LBL_ARROW",
    "BTN_MINIMAP",
];

/// The floating target group and its offsets inside it (layout px).
const TARGET_GROUP: &[(&str, f32, f32)] = &[("LBL_NAMEBG", 0.0, 0.0), ("LBL_NAME", 0.0, 1.0), ("LBL_HEALTHBG", 0.0, 27.0), ("PB_HEALTH", 0.0, 27.0)];

fn spawn_hud(mut commands: Commands, game: Res<GameRes>) {
    let Some(e) = open_screen(&mut commands, &game.0, HUD, Placement::Anchor, 40) else {
        error!("gui: no HUD layout {HUD}");
        return;
    };
    commands.entity(e).insert(GameHud);
    commands.queue(move |world: &mut bevy::prelude::World| {
        if let Some(mut scr) = world.get_mut::<GuiScreen>(e) {
            scr.hide_prefixed(EVENT_ONLY);
            scr.set_text("LBL_NAME", "");
        }
    });
}

fn despawn_hud(mut commands: Commands, q: Query<Entity, With<GameHud>>) {
    for e in &q {
        commands.entity(e).despawn();
    }
}

/// portraits.2da row whose appearance columns match, as a texture resref.
pub fn portrait_for(w: &World, appearance: u32) -> Option<String> {
    let t = w.tables.get(&w.res, "portraits").ok()?;
    let a = appearance as i32;
    (0..t.rows())
        .find(|&r| ["appearancenumber", "appearance_s", "appearance_l"].iter().any(|c| t.int(r, c) == Some(a)))
        .and_then(|r| t.resref(r, "baseresref"))
}

/// Party slots 1-3: portrait, vitality and force bars. Hidden during
/// conversations and while a menu screen is up.
fn update_party(
    world: Res<WorldRes>,
    conv: Res<InConversation>,
    open: Res<OpenMenu>,
    mut huds: Query<(&mut GuiScreen, &mut Node), With<GameHud>>,
    // Per object: chargen / blueprint portrait (party.rs portrait_resref).
    mut cache: Local<std::collections::HashMap<k2_script::ObjectId, Option<String>>>,
) {
    let Ok((mut scr, mut node)) = huds.single_mut() else { return };
    let show = !conv.0 && open.0.is_none();
    let display = if show { Display::Flex } else { Display::None };
    if node.display != display {
        node.display = display;
    }
    if !show {
        return;
    }
    let w = &world.0;
    let leader = w.party_leader();
    let mut party = vec![leader];
    party.extend(w.party.iter().copied().filter(|&p| p != leader));
    for slot in 0..3 {
        let n = slot + 1;
        let member = party.get(slot).and_then(|&id| w.get(id)).and_then(|o| o.creature());
        let tags = [format!("LBL_BACK{n}"), format!("LBL_CHAR{n}"), format!("BTN_CHAR{n}"), format!("PB_VIT{n}"), format!("PB_FORCE{n}")];
        for t in &tags {
            scr.set_visible(t, member.is_some());
        }
        let Some(c) = member else { continue };
        let id = party[slot];
        let portrait = cache.entry(id).or_insert_with(|| w.portrait_resref(id)).clone();
        scr.set_fill(&tags[1], portrait);
        scr.set_progress(&tags[3], c.hp as f32 / c.max_hp.max(1) as f32);
        // Droids and non-Force users show an empty Force bar.
        scr.set_progress(&tags[4], if c.max_force > 0 { c.force as f32 / c.max_force as f32 } else { 0.0 });
    }
    scr.set_selected("TB_SOLO", w.solo_mode);
}

/// Name (and health, for creatures) over the hovered target, or the last
/// creature the leader attacked.
#[allow(clippy::too_many_arguments)]
fn update_target(
    mut world: ResMut<WorldRes>,
    target: Res<Target>,
    last: Res<LastTarget>,
    scale: Res<GuiScale>,
    conv: Res<InConversation>,
    ids: Query<&ObjId>,
    objects: Query<(Entity, &GlobalTransform, &Interactable, &ObjId)>,
    cams: Query<(&Camera, &GlobalTransform), With<Camera3d>>,
    mut huds: Query<&mut GuiScreen, With<GameHud>>,
    mut was: Local<bool>,
) {
    let Ok(mut scr) = huds.single_mut() else { return };
    let w = &mut world.0;
    let hovered = target.0.and_then(|e| ids.get(e).ok()).map(|i| i.0);
    let attacked = last.0.filter(|id| w.get(*id).and_then(|o| o.creature()).is_some_and(|c| !c.dead));
    let cam = cams.single().ok();
    let on_screen = |p: Vec2| p.x >= 0.0 && p.y >= 0.0 && p.x <= scale.win.x && p.y <= scale.win.y;
    // Label anchor over an object, when it is on screen.
    let anchor = |id: ObjectId| -> Option<(Vec2, String)> {
        let (_, gt, it, _) = objects.iter().find(|(_, _, _, o)| o.0 == id)?;
        let (cam, cam_gt) = cam?;
        let p = cam.world_to_viewport(cam_gt, gt.translation() + Vec3::Y * (it.height + 0.25)).ok()?;
        on_screen(p).then(|| (p, it.name.clone()))
    };
    // Hovered target first, then the creature being fought. The hover
    // fallback (nearest in front of the player) can be behind the camera.
    let mut found = None;
    if !conv.0 {
        found = [hovered, attacked].into_iter().flatten().find_map(|id| anchor(id).map(|(p, n)| (id, p, n)));
        // Test hook: K2_GUI_TARGET=1 labels the interactable nearest the
        // screen centre when nothing else is (screenshots can't hover).
        if found.is_none() && std::env::var_os("K2_GUI_TARGET").is_some() {
            let centre = scale.win * 0.5;
            found = objects
                .iter()
                .filter_map(|(_, _, _, o)| anchor(o.0).map(|(p, n)| (o.0, p, n)))
                .min_by(|a, b| a.1.distance(centre).total_cmp(&b.1.distance(centre)));
        }
    }
    if found.is_some() != *was {
        *was = found.is_some();
        debug!("gui: target label {:?} (hover {hovered:?}, last {:?})", found.as_ref().map(|f| &f.2), last.0);
    }
    let Some((id, p, name)) = found else {
        for (t, _, _) in TARGET_GROUP {
            scr.set_visible(t, false);
        }
        return;
    };
    let s = scale.s;
    let hostile = w.get(id).is_some_and(|o| o.creature().is_some()) && {
        let leader = w.party_leader();
        w.is_enemy(leader, id)
    };
    let creature = w.get(id).and_then(|o| o.creature());
    // Doors and containers that can be bashed show their HP too (bash.rs).
    let object_hp = w.object_hp(id).filter(|h| h.bashable && !h.destroyed && w.can_bash(id));
    let name = w.get(id).map(|o| w.substitute(&o.name)).filter(|n| !n.is_empty()).unwrap_or(name);
    // Group is 200 x 33 layout px, centred over the point.
    let origin = Vec2::new(p.x - 100.0 * s, p.y - 33.0 * s);
    for (t, dx, dy) in TARGET_GROUP {
        let health = t.contains("HEALTH");
        scr.set_visible(t, !health || creature.is_some() || object_hp.is_some());
        scr.set_screen_pos(t, Some(origin + Vec2::new(*dx, *dy) * s));
    }
    scr.set_text("LBL_NAME", name);
    if let Some(c) = creature {
        scr.set_progress("PB_HEALTH", c.hp as f32 / c.max_hp.max(1) as f32);
        // The bar art (uibit_fill_16g) is already the game's teal; hostile
        // targets get the name in red instead.
        scr.set_text_color("LBL_NAME", hostile.then_some([0.9, 0.25, 0.2]));
    } else if let Some(h) = object_hp {
        scr.set_progress("PB_HEALTH", h.current as f32 / h.max.max(1) as f32);
        scr.set_text_color("LBL_NAME", None);
    }
}

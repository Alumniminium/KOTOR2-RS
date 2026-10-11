//! map_p (area map with party and map-note markers) and the HUD minimap.
//!
//! World to map: the ARE Map struct pairs two world points with two map
//! points (fractions of the lbl_map<area> texture, v down). NorthAxis 0/1
//! maps world X to u and Y to v; 2/3 swap them. The scale per axis comes
//! from the two point pairs, so signs (which way is north) fall out.

use bevy::prelude::*;
use k2_data::module::AreaMap;
use k2_script::ObjectId;
use k2_world::World;

use super::{ScreenState, party};
use crate::game::WorldRes;
use crate::gui::hud::GameHud;
use crate::gui::menus::{MenuScreen, OpenMenu, open_menu};
use crate::gui::{GuiAssets, GuiClick, GuiScale, GuiScreen};
use crate::model::GameRes;

/// Map texture fraction (u right, v down) of a world XY point.
pub fn world_to_map(m: &AreaMap, x: f32, y: f32) -> Option<Vec2> {
    let (w1, w2, p1, p2) = (m.world_pt1, m.world_pt2, m.map_pt1, m.map_pt2);
    let (u, v) = match m.north_axis {
        0 | 1 => {
            let (dx, dy) = (w1[0] - w2[0], w1[1] - w2[1]);
            if dx.abs() < 1e-4 || dy.abs() < 1e-4 {
                return None;
            }
            ((x - w1[0]) * (p1[0] - p2[0]) / dx + p1[0], (y - w1[1]) * (p1[1] - p2[1]) / dy + p1[1])
        }
        _ => {
            let (dx, dy) = (w1[0] - w2[0], w1[1] - w2[1]);
            if dx.abs() < 1e-4 || dy.abs() < 1e-4 {
                return None;
            }
            ((y - w1[1]) * (p1[0] - p2[0]) / dy + p1[0], (x - w1[0]) * (p1[1] - p2[1]) / dx + p1[1])
        }
    };
    Some(Vec2::new(u, v))
}

/// Screen angle (radians, clockwise from up) of an object's facing on the
/// map texture of size `tex`.
fn facing_angle(m: &AreaMap, pos: Vec2, bearing: f32, tex: Vec2) -> f32 {
    let f = Vec2::new(-bearing.sin(), bearing.cos());
    let (Some(a), Some(b)) = (world_to_map(m, pos.x, pos.y), world_to_map(m, pos.x + f.x, pos.y + f.y)) else { return 0.0 };
    let d = (b - a) * tex;
    d.x.atan2(-d.y)
}

fn area_map(w: &World) -> Option<(String, AreaMap, String)> {
    let m = w.module.as_ref()?;
    let name = w.talk.as_ref().map(|t| t.resolve(&m.area.name)).unwrap_or_default();
    Some((format!("lbl_map{}", m.area.resref), m.area.map.clone(), name))
}

/// Map notes: (text, world xy) of waypoints with HasMapNote.
fn map_notes(w: &World) -> Vec<(String, Vec2)> {
    let Some(m) = w.module.as_ref() else { return Vec::new() };
    m.git
        .waypoints
        .iter()
        .filter(|wp| wp.has_map_note)
        .map(|wp| (w.talk.as_ref().map(|t| t.resolve(&wp.map_note)).unwrap_or_default(), Vec2::new(wp.position[0], wp.position[1])))
        .filter(|(t, _)| !t.is_empty())
        .collect()
}

/// Party markers: (id, world xy, bearing).
fn party_marks(w: &World) -> Vec<(ObjectId, Vec2, f32)> {
    party(w).into_iter().filter_map(|id| w.get(id).map(|o| (id, Vec2::new(o.position.x, o.position.y), o.bearing))).collect()
}

fn node(rect: [f32; 4]) -> Node {
    Node {
        position_type: PositionType::Absolute,
        left: Val::Px(rect[0]),
        top: Val::Px(rect[1]),
        width: Val::Px(rect[2]),
        height: Val::Px(rect[3]),
        ..default()
    }
}

#[derive(Component)]
pub struct MapLayer;

#[allow(clippy::too_many_arguments)]
pub fn map(
    mut commands: Commands,
    game: Res<GameRes>,
    world: Res<WorldRes>,
    open: Res<OpenMenu>,
    mut st: ResMut<ScreenState>,
    scale: Res<GuiScale>,
    mut assets: ResMut<GuiAssets>,
    mut images: ResMut<Assets<Image>>,
    mut clicks: MessageReader<GuiClick>,
    mut screens: Query<&mut GuiScreen, With<MenuScreen>>,
    mut note_sel: Local<usize>,
    mut layer: Local<Option<Entity>>,
    layers: Query<(), With<MapLayer>>,
) {
    if open.0.as_deref() != Some("map") {
        *layer = None;
        return;
    }
    let Some(mut scr) = screens.iter_mut().find(|s| s.name.starts_with("map")) else { return };
    let w = &world.0;
    let notes = map_notes(w);
    for c in clicks.read().filter(|c| c.screen.starts_with("map")) {
        match c.tag.as_str() {
            "BTN_UP" if !notes.is_empty() => *note_sel = (*note_sel + notes.len() - 1) % notes.len(),
            "BTN_DOWN" if !notes.is_empty() => *note_sel = (*note_sel + 1) % notes.len(),
            _ => {}
        }
    }
    let _ = &mut st;
    scr.set_visible("BTN_RETURN", false);
    let Some((tex_name, m, area_name)) = area_map(w) else { return };
    scr.set_text("LBL_Area", area_name);
    scr.set_text("LBL_MapNote", notes.get(*note_sel).map(|n| n.0.clone()).unwrap_or_default());
    scr.set_visible("BTN_UP", notes.len() > 1);
    scr.set_visible("BTN_DOWN", notes.len() > 1);
    let Some(parent) = scr.control("LBL_Map") else { return };
    let Some(tex) = assets.texture(&game.0, &mut images, &tex_name) else { return };
    // Fit the texture into the control, keeping its aspect.
    let idx = scr.index("LBL_Map").unwrap_or(0);
    let r = scr.def_of(idx).rect;
    let (cw, ch) = (r[2] * scale.s, r[3] * scale.s);
    let k = (cw / tex.size.x).min(ch / tex.size.y);
    let size = tex.size * k;
    let origin = Vec2::new((cw - size.x) * 0.5, (ch - size.y) * 0.5);
    if let Some(e) = layer.take()
        && layers.get(e).is_ok()
    {
        commands.entity(e).despawn();
    }
    let root = commands.spawn((MapLayer, node([0.0, 0.0, cw, ch]), ZIndex(50), ChildOf(parent))).id();
    *layer = Some(root);
    commands.spawn((node([origin.x, origin.y, size.x, size.y]), ImageNode::new(tex.image.clone()), ChildOf(root)));
    let mut mark = |pos: Vec2, icon: &str, px: f32, angle: Option<f32>, tint: Color| {
        let Some(uv) = world_to_map(&m, pos.x, pos.y) else { return };
        let Some(t) = assets.texture(&game.0, &mut images, icon) else { return };
        let c = origin + uv * size;
        let s = px * scale.s;
        let mut e = commands.spawn((node([c.x - s * 0.5, c.y - s * 0.5, s, s]), ImageNode { color: tint, ..ImageNode::new(t.image) }, ChildOf(root)));
        if let Some(a) = angle {
            e.insert(UiTransform::from_rotation(Rot2::radians(a)));
        }
    };
    for (i, (_, p)) in notes.iter().enumerate() {
        let tint = if i == *note_sel { Color::srgb(1.0, 1.0, 0.4) } else { Color::srgb(0.6, 0.9, 1.0) };
        mark(*p, "lbl_mapcircle", 14.0, None, tint);
    }
    for (i, (_, p, b)) in party_marks(w).into_iter().enumerate() {
        let icon = if i == 0 { "mm_barrow" } else { "friendlyarrow" };
        mark(p, icon, if i == 0 { 20.0 } else { 16.0 }, Some(facing_angle(&m, p, b, size)), Color::WHITE);
    }
}

/// The HUD minimap: the area map at its native scale, scrolled so the
/// leader sits in the middle of LBL_MAPVIEW, clipped to it, with the
/// leader's arrow on top. Clicking it opens the map screen.
#[allow(clippy::too_many_arguments)]
pub fn minimap(
    mut commands: Commands,
    game: Res<GameRes>,
    world: Res<WorldRes>,
    mut open: ResMut<OpenMenu>,
    scale: Res<GuiScale>,
    mut assets: ResMut<GuiAssets>,
    mut images: ResMut<Assets<Image>>,
    mut clicks: MessageReader<GuiClick>,
    mut huds: Query<&mut GuiScreen, With<GameHud>>,
    existing: Query<Entity, With<MenuScreen>>,
    mut layer: Local<Option<Entity>>,
    layers: Query<(), With<MapLayer>>,
) {
    let Ok(mut scr) = huds.single_mut() else { return };
    if clicks.read().any(|c| c.screen.starts_with("mipc") && c.tag == "BTN_MINIMAP") {
        open_menu(&mut commands, &game, &mut open, &existing, "map");
    }
    let w = &world.0;
    let map = area_map(w);
    let tex = map.as_ref().and_then(|(n, _, _)| assets.texture(&game.0, &mut images, n));
    let shown = tex.is_some();
    for t in ["LBL_MAPBORDER", "LBL_MAPVIEW", "BTN_MINIMAP"] {
        scr.set_visible(t, shown);
    }
    if let Some(e) = layer.take()
        && layers.get(e).is_ok()
    {
        commands.entity(e).despawn();
    }
    let (Some((_, m, _)), Some(tex)) = (map, tex) else { return };
    let Some(parent) = scr.control("LBL_MAPVIEW") else { return };
    let idx = scr.index("LBL_MAPVIEW").unwrap_or(0);
    let r = scr.def_of(idx).rect;
    let (vw, vh) = (r[2] * scale.s, r[3] * scale.s);
    let leader = w.party_leader();
    let Some(o) = w.get(leader) else { return };
    let p = Vec2::new(o.position.x, o.position.y);
    let Some(uv) = world_to_map(&m, p.x, p.y) else { return };
    let size = tex.size * scale.s;
    let off = Vec2::new(vw * 0.5, vh * 0.5) - uv * size;
    let root = commands
        .spawn((MapLayer, Node { overflow: Overflow::clip(), ..node([0.0, 0.0, vw, vh]) }, ZIndex(50), ChildOf(parent)))
        .id();
    *layer = Some(root);
    commands.spawn((node([off.x, off.y, size.x, size.y]), ImageNode::new(tex.image.clone()), ChildOf(root)));
    for (i, (_, q, b)) in party_marks(w).into_iter().enumerate() {
        let icon = if i == 0 { "mm_barrow" } else { "friendlyarrow" };
        let Some(t) = assets.texture(&game.0, &mut images, icon) else { continue };
        let Some(quv) = world_to_map(&m, q.x, q.y) else { continue };
        let c = off + quv * size;
        let s = (if i == 0 { 20.0 } else { 14.0 }) * scale.s;
        commands.spawn((
            node([c.x - s * 0.5, c.y - s * 0.5, s, s]),
            ImageNode::new(t.image),
            UiTransform::from_rotation(Rot2::radians(facing_angle(&m, q, b, size))),
            ChildOf(root),
        ));
    }
}

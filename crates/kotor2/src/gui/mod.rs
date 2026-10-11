//! The game's own GUI: .gui layouts rendered with Bevy UI nodes and KotOR
//! bitmap fonts.
//!
//! A screen is a `GuiScreen` entity (one full-window root node). Its
//! controls become child nodes, built from the layout plus per-control
//! runtime state (`CtlState`); changing state through the setters marks the
//! control dirty and `build_controls` redraws only its pieces. Clicks arrive
//! as `GuiClick` messages carrying the screen name and control tag.
//!
//! Layouts are authored for 800x600. `Placement::Center` scales uniformly
//! (fit) and centers, for full-screen menus. `Placement::Anchor` keeps the
//! same scale but pins each control to the left, centre or right third (and
//! top/middle/bottom) of the window by where its centre sits, which is how
//! the HUD stays in the corners on wide windows.

pub mod assets;
pub mod hud;
pub mod layout;
pub mod menus;
pub mod options;
pub mod screens;
pub mod text;

use std::sync::Arc;

use bevy::prelude::*;
use bevy::ui::FocusPolicy;
use bevy::window::PrimaryWindow;
use k2_res::Resources;

use crate::game::WorldRes;
use crate::model::GameRes;
pub use assets::GuiAssets;
use assets::{BitmapFont, GuiTex};
use layout::{Border, Control, GuiDef, Kind};
#[allow(unused_imports)]
pub use text::BitmapText;

pub const LAYOUT_W: f32 = 800.0;
pub const LAYOUT_H: f32 = 600.0;

pub struct GuiPlugin;

impl Plugin for GuiPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<GuiAssets>()
            .init_resource::<GuiScale>()
            .add_message::<GuiClick>()
            .add_message::<GuiSlide>()
            .configure_sets(Update, (GuiSet::Logic, GuiSet::Input, GuiSet::Build).chain())
            .add_systems(Update, update_scale.before(GuiSet::Logic))
            .add_systems(Update, (control_interaction, item_interaction, list_scroll, slider_drag).in_set(GuiSet::Input))
            .add_systems(Update, (build_screens, build_controls, build_items, text::build_bitmap_text).chain().in_set(GuiSet::Build))
            .add_plugins((menus::MenusPlugin, options::OptionsPlugin, hud::GuiHudPlugin, screens::ScreensPlugin));
    }
}

/// Logic (screens set state) -> Input (hover/click) -> Build (draw).
#[derive(SystemSet, Debug, Clone, PartialEq, Eq, Hash)]
pub enum GuiSet {
    Logic,
    Input,
    Build,
}

/// Layout pixels to window pixels, plus the window size (logical px).
#[derive(Resource, Clone, Copy, PartialEq)]
pub struct GuiScale {
    pub s: f32,
    pub win: Vec2,
}

impl Default for GuiScale {
    fn default() -> Self {
        Self { s: 1.0, win: Vec2::new(LAYOUT_W, LAYOUT_H) }
    }
}

fn update_scale(windows: Query<&Window, With<PrimaryWindow>>, mut scale: ResMut<GuiScale>) {
    let Ok(w) = windows.single() else { return };
    let win = Vec2::new(w.width(), w.height());
    let s = (win.x / LAYOUT_W).min(win.y / LAYOUT_H).max(0.25);
    let new = GuiScale { s, win };
    if *scale != new {
        *scale = new;
    }
}

/// A control was clicked (buttons, toggles, list items).
#[allow(dead_code)] // `item` is for the list screens
#[derive(Message, Clone, Debug)]
pub struct GuiClick {
    pub screen: String,
    pub tag: String,
    pub item: Option<usize>,
}

/// A slider was pressed or dragged; `value` is 0..1 along the track.
#[derive(Message, Clone, Debug)]
pub struct GuiSlide {
    pub screen: String,
    pub tag: String,
    pub value: f32,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Placement {
    Center,
    Anchor,
}

/// One list box row. `icon` is a texture resref drawn in a square at the
/// row's left (inventory-style rows); `count` is a stack size shown over
/// the icon.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ListItem {
    pub text: String,
    pub icon: Option<String>,
    pub count: Option<u32>,
    pub enabled: bool,
}

impl ListItem {
    pub fn text(t: impl Into<String>) -> Self {
        Self { text: t.into(), enabled: true, ..Self::default() }
    }

    pub fn with_icon(mut self, icon: Option<String>) -> Self {
        self.icon = icon;
        self
    }

    pub fn with_count(mut self, n: Option<u32>) -> Self {
        self.count = n.filter(|&n| n > 1);
        self
    }
}

/// Runtime state of one control. Index 0 is the root panel.
#[derive(Clone, Debug)]
pub struct CtlState {
    pub visible: bool,
    pub enabled: bool,
    pub selected: bool,
    /// Replaces the layout text / strref.
    pub text: Option<String>,
    /// Replaces the border fill texture.
    pub fill: Option<String>,
    pub text_color: Option<[f32; 3]>,
    pub border_color: Option<[f32; 3]>,
    /// Progress bars: 0..1 (None = layout CURVALUE/MAXVALUE).
    pub progress: Option<f32>,
    pub progress_color: Option<[f32; 3]>,
    /// List boxes: rows, first visible row, the selected row. A list box
    /// with `text` set and no rows shows that text wrapped inside it
    /// (description panes).
    pub items: Vec<ListItem>,
    pub scroll: usize,
    pub selected_item: Option<usize>,
    /// Window-pixel top-left override (floating controls).
    pub screen_pos: Option<Vec2>,
    hovered: bool,
    item_hover: Option<usize>,
    dirty: bool,
    items_dirty: bool,
}

impl Default for CtlState {
    fn default() -> Self {
        Self {
            visible: true,
            enabled: true,
            selected: false,
            text: None,
            fill: None,
            text_color: None,
            border_color: None,
            progress: None,
            progress_color: None,
            items: Vec::new(),
            scroll: 0,
            selected_item: None,
            screen_pos: None,
            hovered: false,
            item_hover: None,
            dirty: true,
            items_dirty: true,
        }
    }
}

#[derive(Component)]
pub struct GuiScreen {
    pub name: String,
    pub def: Arc<GuiDef>,
    pub placement: Placement,
    pub ctl: Vec<CtlState>,
    controls: Vec<Entity>,
    built_scale: Option<GuiScale>,
    /// Layout px for the root's left, top corner when the whole layout is
    /// moved (barks, nBarkX / nBarkY); None keeps the layout's own position.
    origin: Option<[f32; 2]>,
}

/// One control node. `index` 0 is the root panel, n is `def.controls[n - 1]`.
#[derive(Component)]
pub struct GuiControl {
    pub screen: Entity,
    pub index: usize,
}

/// A list box row.
#[derive(Component)]
struct GuiItem {
    screen: Entity,
    index: usize,
    item: usize,
}

/// Drawn pieces (border, fill, glyphs); despawned on rebuild.
#[derive(Component)]
struct GuiPiece;

#[allow(dead_code)] // API for the menu screens built on this
impl GuiScreen {
    pub fn new(def: GuiDef, placement: Placement) -> Self {
        let n = def.controls.len() + 1;
        let mut ctl = vec![CtlState::default(); n];
        for (i, c) in def.controls.iter().enumerate() {
            ctl[i + 1].selected = c.is_selected;
        }
        Self { name: def.name.clone(), def: Arc::new(def), placement, ctl, controls: Vec::new(), built_scale: None, origin: None }
    }

    /// Moves the whole layout so its root's left, top corner sits at `origin`
    /// (layout px); None puts it back where the layout has it.
    pub fn set_origin(&mut self, origin: Option<[f32; 2]>) {
        if self.origin != origin {
            self.origin = origin;
            for st in &mut self.ctl {
                st.dirty = true;
            }
        }
    }

    pub fn def_of(&self, index: usize) -> &Control {
        if index == 0 { &self.def.root } else { &self.def.controls[index - 1] }
    }

    pub fn index(&self, tag: &str) -> Option<usize> {
        self.def.index(tag).map(|i| i + 1)
    }

    /// The node entity of a control, once built.
    pub fn control(&self, tag: &str) -> Option<Entity> {
        self.index(tag).and_then(|i| self.controls.get(i).copied())
    }

    fn state(&mut self, tag: &str) -> Option<&mut CtlState> {
        let i = self.index(tag)?;
        Some(&mut self.ctl[i])
    }

    fn update<T: PartialEq>(&mut self, tag: &str, get: impl Fn(&mut CtlState) -> &mut T, v: T) {
        if let Some(st) = self.state(tag) {
            let slot = get(st);
            if *slot != v {
                *slot = v;
                st.dirty = true;
            }
        }
    }

    pub fn set_visible(&mut self, tag: &str, v: bool) {
        self.update(tag, |s| &mut s.visible, v);
    }

    pub fn set_enabled(&mut self, tag: &str, v: bool) {
        self.update(tag, |s| &mut s.enabled, v);
    }

    pub fn set_selected(&mut self, tag: &str, v: bool) {
        self.update(tag, |s| &mut s.selected, v);
    }

    pub fn set_text(&mut self, tag: &str, v: impl Into<String>) {
        self.update(tag, |s| &mut s.text, Some(v.into()));
    }

    pub fn set_fill(&mut self, tag: &str, v: Option<String>) {
        self.update(tag, |s| &mut s.fill, v);
    }

    pub fn set_text_color(&mut self, tag: &str, v: Option<[f32; 3]>) {
        self.update(tag, |s| &mut s.text_color, v);
    }

    pub fn set_progress(&mut self, tag: &str, v: f32) {
        // Quantize so a creeping value does not redraw every frame.
        let v = (v.clamp(0.0, 1.0) * 200.0).round() / 200.0;
        self.update(tag, |s| &mut s.progress, Some(v));
    }

    pub fn set_progress_color(&mut self, tag: &str, v: Option<[f32; 3]>) {
        self.update(tag, |s| &mut s.progress_color, v);
    }

    pub fn set_screen_pos(&mut self, tag: &str, v: Option<Vec2>) {
        let v = v.map(|p| p.round());
        self.update(tag, |s| &mut s.screen_pos, v);
    }

    /// Text-only rows.
    pub fn set_items(&mut self, tag: &str, items: Vec<String>) {
        self.set_list(tag, items.into_iter().map(ListItem::text).collect());
    }

    /// Rows with optional icons and counts. Keeps the scroll position when
    /// it is still valid, so refreshing a list in place does not jump.
    pub fn set_list(&mut self, tag: &str, items: Vec<ListItem>) {
        if let Some(st) = self.state(tag)
            && st.items != items
        {
            st.items = items;
            if st.scroll >= st.items.len() {
                st.scroll = 0;
            }
            if st.selected_item.is_some_and(|i| i >= st.items.len()) {
                st.selected_item = None;
            }
            st.dirty = true;
            st.items_dirty = true;
        }
    }

    pub fn set_list_selected(&mut self, tag: &str, item: Option<usize>) {
        if let Some(st) = self.state(tag)
            && st.selected_item != item
        {
            st.selected_item = item;
            st.dirty = true;
            st.items_dirty = true;
        }
    }

    pub fn list_selected(&self, tag: &str) -> Option<usize> {
        self.index(tag).and_then(|i| self.ctl[i].selected_item)
    }

    /// The row under the mouse, if any.
    pub fn hovered_item(&self, tag: &str) -> Option<usize> {
        self.index(tag).and_then(|i| self.ctl[i].item_hover)
    }

    /// Whether the mouse is over the control itself.
    pub fn is_hovered(&self, tag: &str) -> bool {
        self.index(tag).is_some_and(|i| self.ctl[i].hovered)
    }

    pub fn is_visible(&self, tag: &str) -> bool {
        self.index(tag).is_some_and(|i| self.ctl[i].visible)
    }

    /// Hides every control whose tag starts with one of the prefixes.
    pub fn hide_prefixed(&mut self, prefixes: &[&str]) {
        let tags: Vec<String> = self.def.controls.iter().map(|c| c.tag.clone()).filter(|t| prefixes.iter().any(|p| t.starts_with(p))).collect();
        for t in tags {
            self.set_visible(&t, false);
        }
    }

    /// Layout rect (root-relative children made absolute) in layout px.
    fn layout_rect(&self, index: usize) -> [f32; 4] {
        let r = self.def.root.rect;
        let c = self.def_of(index).rect;
        // A moved layout shifts every control by the same offset.
        let [dx, dy] = self.origin.map_or([0.0, 0.0], |o| [o[0] - r[0], o[1] - r[1]]);
        if index == 0 { [r[0] + dx, r[1] + dy, r[2], r[3]] } else { [r[0] + dx + c[0], r[1] + dy + c[1], c[2], c[3]] }
    }

    /// Window-space rect (left, top, width, height) in logical px.
    fn screen_rect(&self, index: usize, sc: GuiScale) -> [f32; 4] {
        let [l, t, w, h] = self.layout_rect(index);
        let s = sc.s;
        if let Some(p) = self.ctl[index].screen_pos {
            return [p.x, p.y, w * s, h * s];
        }
        match self.placement {
            Placement::Center => {
                let ox = (sc.win.x - LAYOUT_W * s) * 0.5;
                let oy = (sc.win.y - LAYOUT_H * s) * 0.5;
                [ox + l * s, oy + t * s, w * s, h * s]
            }
            Placement::Anchor => {
                let pin = |pos: f32, size: f32, layout: f32, win: f32| {
                    let c = pos + size * 0.5;
                    if c < layout / 3.0 {
                        pos * s
                    } else if c > layout * 2.0 / 3.0 {
                        win - (layout - pos) * s
                    } else {
                        win * 0.5 - (layout * 0.5 - pos) * s
                    }
                };
                if index == 0 {
                    return [0.0, 0.0, sc.win.x, sc.win.y];
                }
                [pin(l, w, LAYOUT_W, sc.win.x), pin(t, h, LAYOUT_H, sc.win.y), w * s, h * s]
            }
        }
    }
}

/// Loads `name` (exact resref, or a base name resolved to the 800x600 PC
/// variant) and spawns it as a screen at `z`. Returns the screen entity.
pub fn open_screen(commands: &mut Commands, res: &Resources, name: &str, placement: Placement, z: i32) -> Option<Entity> {
    let resolved = layout::resolve(res, name)?;
    let def = layout::load(res, &resolved)?;
    info!("gui: open {resolved} ({} controls)", def.controls.len());
    Some(
        commands
            .spawn((
                Name::new(format!("gui {resolved}")),
                Node { position_type: PositionType::Absolute, width: Val::Percent(100.0), height: Val::Percent(100.0), ..default() },
                GlobalZIndex(z),
                GuiScreen::new(def, placement),
            ))
            .id(),
    )
}

/// Spawns control nodes for new screens; marks everything dirty when the
/// window scale changed.
fn build_screens(mut commands: Commands, scale: Res<GuiScale>, mut screens: Query<(Entity, &mut GuiScreen)>) {
    for (e, mut scr) in &mut screens {
        if scr.controls.is_empty() {
            let n = scr.ctl.len();
            let mut ids = Vec::with_capacity(n);
            for i in 0..n {
                let kind = scr.def_of(i).kind;
                let mut ec = commands.spawn((
                    Name::new(format!("ctl {}", scr.def_of(i).tag)),
                    Node { position_type: PositionType::Absolute, ..default() },
                    ZIndex(i as i32),
                    GuiControl { screen: e, index: i },
                    ChildOf(e),
                ));
                if kind.clickable() {
                    ec.insert((Interaction::default(), FocusPolicy::Block));
                } else if kind == Kind::ListBox {
                    ec.insert((Interaction::default(), FocusPolicy::Pass));
                }
                ids.push(ec.id());
            }
            scr.controls = ids;
        }
        if scr.built_scale != Some(*scale) {
            scr.built_scale = Some(*scale);
            for st in &mut scr.ctl {
                st.dirty = true;
                st.items_dirty = true;
            }
        }
    }
}

/// Everything a control build needs from the world.
struct Ctx<'a> {
    res: &'a Resources,
    images: &'a mut Assets<Image>,
    assets: &'a mut GuiAssets,
    talk: Option<&'a k2_data::Talk>,
    s: f32,
}

impl Ctx<'_> {
    fn tex(&mut self, name: &str) -> Option<GuiTex> {
        self.assets.texture(self.res, self.images, name)
    }

    fn font(&mut self, name: &str) -> Option<Arc<BitmapFont>> {
        self.assets.font_or_default(self.res, self.images, name)
    }
}

fn color(c: [f32; 3]) -> Color {
    Color::srgb(c[0].max(0.0), c[1].max(0.0), c[2].max(0.0))
}

fn piece(commands: &mut Commands, parent: Entity, rect: [f32; 4], img: ImageNode, transform: UiTransform) {
    commands.spawn((
        Node {
            position_type: PositionType::Absolute,
            left: Val::Px(rect[0]),
            top: Val::Px(rect[1]),
            width: Val::Px(rect[2].max(0.0)),
            height: Val::Px(rect[3].max(0.0)),
            ..default()
        },
        img,
        transform,
        GuiPiece,
        ChildOf(parent),
    ));
}

fn image(t: &GuiTex, c: Color) -> ImageNode {
    ImageNode { image: t.image.clone(), color: c, image_mode: NodeImageMode::Stretch, ..default() }
}

/// Fill (inset by the frame), four edges (the edge art is a top edge;
/// the others are mirrored or rotated copies) and four corners.
#[allow(clippy::too_many_arguments)]
fn draw_border(commands: &mut Commands, parent: Entity, ctx: &mut Ctx, b: &Border, w: f32, h: f32, tint: [f32; 3], fill_override: Option<&str>, flip_fill: bool) {
    let c = color(tint);
    let d = (b.dimension * ctx.s).min(w * 0.5).min(h * 0.5);
    let fill = fill_override.or(b.fill.as_deref());
    if let Some(t) = fill.and_then(|f| ctx.tex(f)) {
        let (iw, ih) = ((w - 2.0 * d).max(0.0), (h - 2.0 * d).max(0.0));
        if iw > 0.0 && ih > 0.0 {
            piece(commands, parent, [d, d, iw, ih], ImageNode { flip_y: flip_fill, ..image(&t, c) }, UiTransform::IDENTITY);
        }
    }
    if d <= 0.0 {
        return;
    }
    if let Some(t) = b.edge.as_deref().and_then(|e| ctx.tex(e)) {
        let (iw, ih) = (w - 2.0 * d, h - 2.0 * d);
        if iw > 0.0 {
            piece(commands, parent, [d, 0.0, iw, d], image(&t, c), UiTransform::IDENTITY);
            piece(commands, parent, [d, h - d, iw, d], ImageNode { flip_y: true, ..image(&t, c) }, UiTransform::IDENTITY);
        }
        if ih > 0.0 {
            // A horizontal strip centred on the side, turned a quarter.
            let (l, t0) = (d * 0.5 - ih * 0.5, h * 0.5 - d * 0.5);
            piece(commands, parent, [l, t0, ih, d], image(&t, c), UiTransform::from_rotation(Rot2::degrees(-90.0)));
            piece(commands, parent, [w - d * 0.5 - ih * 0.5, t0, ih, d], image(&t, c), UiTransform::from_rotation(Rot2::degrees(90.0)));
        }
    }
    if let Some(t) = b.corner.as_deref().and_then(|e| ctx.tex(e)) {
        piece(commands, parent, [0.0, 0.0, d, d], image(&t, c), UiTransform::IDENTITY);
        piece(commands, parent, [w - d, 0.0, d, d], ImageNode { flip_x: true, ..image(&t, c) }, UiTransform::IDENTITY);
        piece(commands, parent, [0.0, h - d, d, d], ImageNode { flip_y: true, ..image(&t, c) }, UiTransform::IDENTITY);
        piece(commands, parent, [w - d, h - d, d, d], ImageNode { flip_x: true, flip_y: true, ..image(&t, c) }, UiTransform::IDENTITY);
    }
}

/// The partial bar: horizontal from the left, vertical from the bottom,
/// showing the matching part of the texture.
#[allow(clippy::too_many_arguments)]
fn draw_progress(commands: &mut Commands, parent: Entity, ctx: &mut Ctx, b: &Border, w: f32, h: f32, frac: f32, tint: [f32; 3]) {
    let Some(t) = b.fill.as_deref().and_then(|f| ctx.tex(f)) else { return };
    let frac = frac.clamp(0.0, 1.0);
    if frac <= 0.0 {
        return;
    }
    let (rect, uv) = if w >= h {
        // STARTFROMLEFT is 0 on the shipped horizontal bars too, and they
        // still grow from the left in the game.
        ([0.0, 0.0, w * frac, h], Rect::new(0.0, 0.0, t.size.x * frac, t.size.y))
    } else {
        let ph = h * frac;
        ([0.0, h - ph, w, ph], Rect::new(0.0, t.size.y * (1.0 - frac), t.size.x, t.size.y))
    };
    piece(commands, parent, rect, ImageNode { rect: Some(uv), ..image(&t, color(tint)) }, UiTransform::IDENTITY);
}

fn ctl_text(def: &Control, st: &CtlState, talk: Option<&k2_data::Talk>) -> String {
    if let Some(t) = &st.text {
        return t.clone();
    }
    let Some(td) = &def.text else { return String::new() };
    match td.strref {
        Some(r) => talk.and_then(|t| t.get(r)).map(str::to_owned).unwrap_or_default(),
        None => td.text.clone(),
    }
}

/// Which frame a control shows in its current state.
fn active_border<'a>(def: &'a Control, st: &CtlState, hovered: bool) -> Option<&'a Border> {
    let base = def.border.as_ref();
    if !st.enabled {
        return base;
    }
    let pick = match (st.selected, hovered) {
        (true, true) => def.hilight_selected.as_ref().or(def.selected.as_ref()).or(def.hilight.as_ref()),
        (true, false) => def.selected.as_ref(),
        (false, true) => def.hilight.as_ref(),
        (false, false) => None,
    };
    pick.filter(|b| !b.is_empty()).or(base)
}

fn text_color(def: &Control, st: &CtlState, hovered: bool) -> [f32; 3] {
    let mut c = st.text_color.unwrap_or_else(|| {
        let base = def.text.as_ref().map_or([1.0; 3], |t| t.color);
        if hovered && st.enabled { def.hilight.as_ref().map_or(base, |h| h.color) } else { base }
    });
    if !st.enabled {
        c = [c[0] * 0.45, c[1] * 0.45, c[2] * 0.45];
    }
    c
}

/// Draws one control's own pieces into `parent` (size w x h window px).
#[allow(clippy::too_many_arguments)]
fn draw_control(commands: &mut Commands, parent: Entity, ctx: &mut Ctx, def: &Control, st: &CtlState, hovered: bool, w: f32, h: f32) {
    let tint_of = |b: &Border| st.border_color.unwrap_or(b.color);
    // Scroll-down arrows reuse the up-arrow art; the engine flips it.
    let flip = def.tag.contains("DOWN");
    if let Some(b) = active_border(def, st, hovered) {
        draw_border(commands, parent, ctx, b, w, h, tint_of(b), st.fill.as_deref(), flip);
    } else if let Some(f) = st.fill.as_deref()
        && let Some(t) = ctx.tex(f)
    {
        piece(commands, parent, [0.0, 0.0, w, h], image(&t, Color::WHITE), UiTransform::IDENTITY);
    }
    if def.kind == Kind::Progress
        && let Some(p) = &def.progress
    {
        let frac = st.progress.unwrap_or(def.cur_value as f32 / def.max_value.max(1) as f32);
        draw_progress(commands, parent, ctx, p, w, h, frac, st.progress_color.unwrap_or(p.color));
    }
    if def.kind == Kind::Slider
        && let Some(t) = def.thumb.as_deref().and_then(|f| ctx.tex(f))
    {
        // The thumb rides the track; its centre is at `frac` of the width.
        let frac = st.progress.unwrap_or(def.cur_value as f32 / def.max_value.max(1) as f32).clamp(0.0, 1.0);
        let th = (t.size.y * ctx.s).min(h);
        let tw = t.size.x * ctx.s * (th / (t.size.y * ctx.s));
        piece(commands, parent, [(w - tw) * frac, (h - th) * 0.5, tw, th], image(&t, Color::WHITE), UiTransform::IDENTITY);
    }
    if def.kind == Kind::ListBox {
        // Description panes: wrapped text in the box, proto item's font.
        if st.items.is_empty()
            && let Some(t) = st.text.as_deref().filter(|t| !t.is_empty())
            && let Some(td) = def.proto.as_ref().and_then(|p| p.text.clone())
            && let Some(font) = ctx.font(&td.font)
        {
            let pad = (def.border.as_ref().map_or(0.0, |b| b.dimension) * 0.5 + 4.0) * ctx.s;
            text::spawn_glyphs(commands, parent, &font, t, [pad, pad, w - 2.0 * pad, h - 2.0 * pad], 9, color(td.color), ctx.s, true);
        }
        return;
    }
    let text = ctl_text(def, st, ctx.talk);
    if text.is_empty() {
        return;
    }
    let Some(td) = &def.text else { return };
    let Some(font) = ctx.font(&td.font) else { return };
    text::spawn_glyphs(commands, parent, &font, &text, [0.0, 0.0, w, h], td.align, color(text_color(def, st, hovered)), ctx.s, true);
}

#[allow(clippy::too_many_arguments)]
fn build_controls(
    mut commands: Commands,
    scale: Res<GuiScale>,
    game: Option<Res<GameRes>>,
    world: Option<Res<WorldRes>>,
    mut images: ResMut<Assets<Image>>,
    mut assets: ResMut<GuiAssets>,
    mut screens: Query<(Entity, &mut GuiScreen)>,
    mut nodes: Query<(&mut Node, Option<&Children>)>,
    pieces: Query<(), With<GuiPiece>>,
    items: Query<(), With<GuiItem>>,
) {
    let Some(game) = game else { return };
    let talk = world.as_ref().and_then(|w| w.0.talk.as_ref());
    let mut ctx = Ctx { res: &game.0, images: &mut images, assets: &mut assets, talk, s: scale.s };
    for (screen, mut scr) in &mut screens {
        if scr.controls.is_empty() {
            continue;
        }
        for i in 0..scr.ctl.len() {
            if !scr.ctl[i].dirty {
                continue;
            }
            scr.ctl[i].dirty = false;
            let e = scr.controls[i];
            let [l, t, w, h] = scr.screen_rect(i, *scale);
            let Ok((mut node, children)) = nodes.get_mut(e) else { continue };
            node.left = Val::Px(l);
            node.top = Val::Px(t);
            node.width = Val::Px(w);
            node.height = Val::Px(h);
            node.display = if scr.ctl[i].visible { Display::Flex } else { Display::None };
            // Long description text must not spill out of its list box.
            if scr.def_of(i).kind == Kind::ListBox {
                node.overflow = Overflow::clip();
            }
            for c in children.into_iter().flatten() {
                if pieces.contains(*c) {
                    commands.entity(*c).despawn();
                }
                if scr.ctl[i].items_dirty && items.contains(*c) {
                    commands.entity(*c).despawn();
                }
            }
            if !scr.ctl[i].visible || (i == 0 && scr.placement == Placement::Anchor) {
                continue;
            }
            let def = scr.def_of(i).clone();
            let st = scr.ctl[i].clone();
            draw_control(&mut commands, e, &mut ctx, &def, &st, st.hovered, w, h);
            if def.kind == Kind::ListBox && st.items_dirty {
                scr.ctl[i].items_dirty = false;
                spawn_items(&mut commands, e, screen, &scr, i, *scale);
            }
        }
    }
}

/// List rows: the PROTOITEM repeated down the box from its own offset.
fn spawn_items(commands: &mut Commands, list: Entity, screen: Entity, scr: &GuiScreen, index: usize, sc: GuiScale) {
    let def = scr.def_of(index);
    let Some(proto) = &def.proto else { return };
    let st = &scr.ctl[index];
    let s = sc.s;
    let (dx, dy) = (proto.rect[0] - def.rect[0], (proto.rect[1] - def.rect[1]).max(0.0));
    let step = proto.rect[3] + def.padding;
    let mut y = dy;
    for (k, _) in st.items.iter().enumerate().skip(st.scroll) {
        if y + proto.rect[3] > def.rect[3] + 0.5 {
            break;
        }
        commands.spawn((
            Name::new(format!("item {k}")),
            Node {
                position_type: PositionType::Absolute,
                left: Val::Px(dx * s),
                top: Val::Px(y * s),
                width: Val::Px(proto.rect[2] * s),
                height: Val::Px(proto.rect[3] * s),
                ..default()
            },
            Interaction::default(),
            FocusPolicy::Block,
            GuiItem { screen, index, item: k },
            ItemDirty,
            ChildOf(list),
        ));
        y += step;
    }
}

#[derive(Component)]
struct ItemDirty;

/// (Re)draws list rows marked dirty (new, or hover changed).
#[allow(clippy::too_many_arguments)]
fn build_items(
    mut commands: Commands,
    scale: Res<GuiScale>,
    game: Option<Res<GameRes>>,
    world: Option<Res<WorldRes>>,
    mut images: ResMut<Assets<Image>>,
    mut assets: ResMut<GuiAssets>,
    screens: Query<&GuiScreen>,
    dirty: Query<(Entity, &GuiItem, &Node, Option<&Children>), With<ItemDirty>>,
) {
    let Some(game) = game else { return };
    let talk = world.as_ref().and_then(|w| w.0.talk.as_ref());
    let mut ctx = Ctx { res: &game.0, images: &mut images, assets: &mut assets, talk, s: scale.s };
    for (e, it, node, children) in &dirty {
        commands.entity(e).remove::<ItemDirty>();
        for c in children.into_iter().flatten() {
            commands.entity(*c).despawn();
        }
        let Ok(scr) = screens.get(it.screen) else { continue };
        let def = scr.def_of(it.index);
        let Some(proto) = &def.proto else { continue };
        let st = &scr.ctl[it.index];
        let Some(item) = st.items.get(it.item) else { continue };
        let hovered = st.item_hover == Some(it.item);
        let selected = st.selected_item == Some(it.item);
        let row = CtlState { enabled: item.enabled, selected, ..CtlState::default() };
        let (Val::Px(w), Val::Px(h)) = (node.width, node.height) else { continue };
        let mut proto = (**proto).clone();
        // Dialog-style lists have no HILIGHT; brighten the text instead.
        if (hovered || selected) && proto.hilight.is_none() {
            proto.hilight = Some(Border { color: [1.0, 0.92, 0.55], ..Border::default() });
        }
        // A selected row without SELECTED art shows its highlight frame.
        if proto.selected.is_none() {
            proto.selected = proto.hilight.clone();
        }
        // Frame only; the text goes in after the icon square.
        let label = CtlState { text: Some(String::new()), ..row.clone() };
        draw_control(&mut commands, e, &mut ctx, &proto, &label, hovered, w, h);
        let mut text_area = [0.0, 0.0, w, h];
        if let Some(icon) = &item.icon {
            let inset = (proto.border.as_ref().map_or(0.0, |b| b.dimension) * 0.35 * ctx.s).max(3.0 * ctx.s);
            let side = h - 2.0 * inset;
            if let Some(t) = ctx.tex(icon) {
                piece(&mut commands, e, [inset, inset, side, side], image(&t, Color::WHITE), UiTransform::IDENTITY);
            }
            if let Some(n) = item.count
                && let Some(font) = ctx.font("dialogfont10x10")
            {
                text::spawn_glyphs(&mut commands, e, &font, &n.to_string(), [inset, inset, side, side], 4 | 32, Color::WHITE, ctx.s, false);
            }
            text_area = [h + 4.0 * ctx.s, 0.0, w - h - 8.0 * ctx.s, h];
        }
        if let Some(td) = &proto.text
            && let Some(font) = ctx.font(&td.font)
        {
            let align = if item.icon.is_some() { 1 | 16 } else { td.align };
            let c = color(text_color(&proto, &row, hovered || selected));
            text::spawn_glyphs(&mut commands, e, &font, &item.text, text_area, align, c, ctx.s, true);
        }
    }
}

fn control_interaction(
    mut screens: Query<&mut GuiScreen>,
    q: Query<(&Interaction, &GuiControl), Changed<Interaction>>,
    mut clicks: MessageWriter<GuiClick>,
    mut sfx: MessageWriter<crate::uisfx::UiSfx>,
) {
    for (i, c) in &q {
        let Ok(mut scr) = screens.get_mut(c.screen) else { continue };
        let hovered = *i != Interaction::None;
        let st = &mut scr.ctl[c.index];
        if !st.visible {
            continue;
        }
        if st.hovered != hovered {
            st.hovered = hovered;
            st.dirty = true;
            if hovered && *i == Interaction::Hovered && st.enabled {
                sfx.write(crate::uisfx::UiSfx("Entered_Default"));
            }
        }
        if *i == Interaction::Pressed && st.enabled {
            let tag = scr.def_of(c.index).tag.clone();
            clicks.write(GuiClick { screen: scr.name.clone(), tag, item: None });
        }
    }
}

/// Holding the mouse on a slider moves its thumb to the cursor.
fn slider_drag(
    windows: Query<&Window, With<PrimaryWindow>>,
    scale: Res<GuiScale>,
    mut screens: Query<&mut GuiScreen>,
    q: Query<(&Interaction, &GuiControl)>,
    mut out: MessageWriter<GuiSlide>,
) {
    let Some(cursor) = windows.single().ok().and_then(|w| w.cursor_position()) else { return };
    for (i, c) in &q {
        if *i != Interaction::Pressed {
            continue;
        }
        let Ok(mut scr) = screens.get_mut(c.screen) else { continue };
        if scr.def_of(c.index).kind != Kind::Slider || !scr.ctl[c.index].enabled {
            continue;
        }
        let [l, _, w, _] = scr.screen_rect(c.index, *scale);
        let v = ((cursor.x - l) / w.max(1.0)).clamp(0.0, 1.0);
        let st = &mut scr.ctl[c.index];
        if st.progress != Some(v) {
            st.progress = Some(v);
            st.dirty = true;
            let tag = scr.def_of(c.index).tag.clone();
            out.write(GuiSlide { screen: scr.name.clone(), tag, value: v });
        }
    }
}

fn item_interaction(
    mut commands: Commands,
    mut screens: Query<&mut GuiScreen>,
    q: Query<(Entity, &Interaction, &GuiItem), Changed<Interaction>>,
    all: Query<(Entity, &GuiItem)>,
    mut clicks: MessageWriter<GuiClick>,
    mut sfx: MessageWriter<crate::uisfx::UiSfx>,
) {
    for (_, i, it) in &q {
        let Ok(mut scr) = screens.get_mut(it.screen) else { continue };
        let st = &mut scr.ctl[it.index];
        let before = st.item_hover;
        if *i == Interaction::None {
            if st.item_hover == Some(it.item) {
                st.item_hover = None;
            }
        } else {
            st.item_hover = Some(it.item);
        }
        if before != st.item_hover {
            if st.item_hover.is_some() && *i == Interaction::Hovered {
                sfx.write(crate::uisfx::UiSfx("Entered_Default"));
            }
            for (e, other) in &all {
                if other.screen == it.screen && other.index == it.index && (Some(other.item) == before || Some(other.item) == st.item_hover) {
                    commands.entity(e).insert(ItemDirty);
                }
            }
        }
        if *i == Interaction::Pressed {
            // Clicking a row selects it (screens may override).
            let st = &mut scr.ctl[it.index];
            if st.selected_item != Some(it.item) {
                let old = st.selected_item.replace(it.item);
                for (e, other) in &all {
                    if other.screen == it.screen && other.index == it.index && (Some(other.item) == old || other.item == it.item) {
                        commands.entity(e).insert(ItemDirty);
                    }
                }
            }
            let tag = scr.def_of(it.index).tag.clone();
            clicks.write(GuiClick { screen: scr.name.clone(), tag, item: Some(it.item) });
        }
    }
}

/// Mouse wheel over a list box scrolls it by a row.
fn list_scroll(
    wheel: Res<bevy::input::mouse::AccumulatedMouseScroll>,
    mut screens: Query<&mut GuiScreen>,
    lists: Query<(&Interaction, &GuiControl)>,
    rows: Query<(&Interaction, &GuiItem)>,
) {
    if wheel.delta.y == 0.0 {
        return;
    }
    let over = lists
        .iter()
        .filter(|(i, _)| **i != Interaction::None)
        .map(|(_, c)| (c.screen, c.index))
        .chain(rows.iter().filter(|(i, _)| **i != Interaction::None).map(|(_, r)| (r.screen, r.index)))
        .next();
    let Some((screen, index)) = over else { return };
    let Ok(mut scr) = screens.get_mut(screen) else { return };
    let st = &mut scr.ctl[index];
    let n = st.items.len();
    let new = if wheel.delta.y > 0.0 { st.scroll.saturating_sub(1) } else { (st.scroll + 1).min(n.saturating_sub(1)) };
    if new != st.scroll {
        st.scroll = new;
        st.dirty = true;
        st.items_dirty = true;
    }
}

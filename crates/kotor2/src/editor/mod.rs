//! In-engine area editor: `kotor2 --edit MODULE`, or F10 in game.
//!
//! The editor works on its own document (`doc::EditDoc`, the area's GIT),
//! not on the live world: the world pauses, its objects hide, and the
//! document's instances are shown instead. Saving (Ctrl+S) writes the GIT
//! and any forked blueprints as a MOD capsule,
//! `~/.local/share/kotor2-rs/mods/<name>.mod`; run the game with
//! `--mod-dir <that file>` to play the edits.
//!
//! The user interface is egui (`ui.rs`, through bevy_egui): menu bar, object
//! tree (left), inspector (right), status bar, a palette window and the Load
//! Map modal. This file holds the state, camera, picking, move/rotate
//! handles and the world views.
//!
//! Controls (no text field focused):
//!   WASD / Space / C   fly (Shift faster), right mouse look
//!   left click        select; drag moves on the walkmesh (Shift: Z only)
//!   drag a handle     axis arrows move along X/Y/Z, the ring knob turns
//!   R + wheel, Q / E  rotate selection
//!   Del               delete      Ctrl+D duplicate
//!   Ctrl+Z / Ctrl+Y   undo / redo Ctrl+S save
//!   Tab               palette (search, click a template, click the walkmesh)
//!   Enter             finish a trigger polygon       Esc cancel tool
//!   Ctrl+O            load another map (module list)
//!   F10               leave the editor

pub mod doc;
mod ui;

use std::collections::HashMap;

use bevy::input::ButtonState;
use bevy::input::keyboard::{Key, KeyboardInput};
use bevy::input::mouse::{AccumulatedMouseMotion, AccumulatedMouseScroll, MouseButtonInput, MouseScrollUnit};
use bevy::prelude::*;
use bevy::window::PrimaryWindow;
use bevy_egui::{EguiContext, EguiGlobalSettings, EguiPlugin, EguiPrimaryContextPass, EguiSchedule, PrimaryEguiContext};
use k2_res::{Install, Resources};

use crate::Args;
use crate::anim::{CreatureSpec, spawn_creature};
use crate::area::GameData;
use crate::coords;
use crate::flycam::FlyCam;
use crate::game::{AreaRoot, Fade, GameSet, NeedAreaLoad, ObjId, WorldRes, enter_module};
use crate::gui::hud::GameHud;
use crate::gui::menus::PauseState;
use crate::material::KMaterial;
use crate::messages::AreaLoaded;
use crate::model::{AssetCache, GameRes, Loader};
use crate::play::{CamMode, PlayCam};
use doc::{EditDoc, Kind, blueprint_catalog, fields, map_name, mod_path};

/// The area the main menu's Editor button opens (the default `--edit` module).
pub const START_MODULE: &str = "101per";

pub struct EditorPlugin;

impl Plugin for EditorPlugin {
    fn build(&self, app: &mut App) {
        // The egui context is attached to the camera only while editing
        // (`toggle`), so the plugin's systems have nothing to run on in play.
        app.insert_resource(EguiGlobalSettings { auto_create_primary_context: false, ..default() })
            .add_plugins(EguiPlugin::default())
            .init_gizmo_group::<HandleGizmos>()
            .init_resource::<Editor>()
            .init_resource::<EdViews>()
            .add_systems(Startup, setup_gizmo_config)
            .add_systems(Update, (auto_enter, toggle).chain())
            .add_systems(Update, clear_fade.before(GameSet::Ui).run_if(|e: Res<Editor>| e.active))
            .add_systems(Update, on_area_loaded.after(toggle))
            .add_systems(Update, script_hook.after(toggle))
            .add_systems(
                Update,
                (load_hook, hotkeys, edit_cam, mouse, cursor_probe, sync_views, hide_world, gizmos).chain().after(toggle).run_if(|e: Res<Editor>| e.active),
            )
            .add_systems(EguiPrimaryContextPass, ui::ui_system.run_if(|e: Res<Editor>| e.active));
    }
}

/// Move/rotate handles draw on top of the scene.
#[derive(Default, Reflect, GizmoConfigGroup)]
struct HandleGizmos;

fn setup_gizmo_config(mut store: ResMut<GizmoConfigStore>) {
    let (cfg, _) = store.config_mut::<HandleGizmos>();
    cfg.depth_bias = -1.0;
    cfg.line.width = 3.0;
}

#[derive(Default, Clone, Debug, PartialEq)]
enum Tool {
    #[default]
    Select,
    /// Click on the walkmesh places this blueprint.
    Place(Kind, String),
    /// Clicks add polygon points; Enter finishes (triggers, encounters).
    Polygon(Kind, String, Vec<Vec3>),
}

/// A viewport handle on the selection: an axis arrow (0 X, 1 Y, 2 Z) or the
/// bearing knob.
#[derive(Clone, Copy, PartialEq, Debug)]
enum Handle {
    Axis(usize),
    Yaw,
}

#[derive(Clone, Copy, Debug)]
enum DragMode {
    /// Slides on the walkmesh (Shift: Z only).
    Free,
    /// Along an axis line through `origin`; `t0` is where the cursor ray
    /// grabbed it.
    Axis { axis: Vec3, origin: Vec3, t0: f32 },
    Yaw,
}

struct Drag {
    uid: u64,
    started: bool,
    press: Vec2,
    last: Vec2,
    mode: DragMode,
}

#[derive(Resource)]
pub struct Editor {
    pub active: bool,
    auto_done: bool,
    /// Set by the main menu's Editor button: the next area load opens the
    /// editor on its Load Map list.
    from_menu: bool,
    doc: Option<EditDoc>,
    res: Option<Resources>,
    sel: Option<u64>,
    tool: Tool,
    /// (kind, resref, display name) for every blueprint in the install.
    catalog: Vec<(Kind, String, String)>,
    /// Script (NCS) and conversation (DLG) names in the install, sorted.
    scripts: Vec<String>,
    convs: Vec<String>,
    status: String,
    /// Bumped on every document change; views follow it.
    rev: u64,
    drag: Option<Drag>,
    hover_handle: Option<Handle>,
    saved_cam: Option<PlayCam>,
    /// Every module in the install (lowercase resrefs).
    modules: Vec<String>,
    /// Area name per module (same order), for the Load Map list.
    map_names: Vec<String>,
    /// A module the user asked to open while the document had unsaved edits;
    /// choosing the same one again discards them.
    confirm: Option<String>,
    /// The module being loaded; the document is rebuilt on `AreaLoaded`.
    loading: Option<String>,
    /// `K2_EDIT_LOAD` already fired.
    load_hook_done: bool,
    /// `K2_EDIT_AT` fired: behave as if F10 was pressed.
    f10_hook: bool,
    saved_fade: Option<(f32, f32, f32, f32, [f32; 3])>,
    specs: HashMap<(Kind, String), Option<CreatureSpec>>,
    /// Blueprint Tag per (kind, template) for the object tree.
    tags: HashMap<(Kind, String), String>,
    mod_name: String,

    // -- UI state (ui.rs) ---------------------------------------------------
    /// The Load Map modal is open.
    picker_open: bool,
    pick_search: String,
    pick_cursor: usize,
    palette_open: bool,
    palette_search: String,
    palette_kind: Option<Kind>,
    obj_search: String,
    /// View menu: show each kind (views, markers, picking).
    show_kind: [bool; 9],
    show_markers: bool,
    show_handles: bool,
    show_grid: bool,
    show_walk: bool,
    /// Ground point under the cursor (game space), when it is over the view.
    cursor_world: Option<Vec3>,
    /// A camera fly-to request (game space point), taken by `edit_cam`.
    fly_to: Option<Vec3>,
    /// An undo step is open for the inspector drag/typing in progress, so
    /// more changes from it join that step.
    step_open: bool,
    /// The palette search takes focus on the next frame.
    palette_focus: bool,
    /// The Revert confirmation dialog is open.
    revert_confirm: bool,
    /// Screen rectangles covered by egui panels/windows (logical px).
    ui_rects: Vec<[f32; 4]>,
    /// egui holds the pointer (modal, open popup, widget drag) or the
    /// keyboard (text field focus); the 3D view ignores that input.
    ui_ptr: bool,
    ui_kbd: bool,
}

impl Default for Editor {
    fn default() -> Self {
        Self {
            active: false,
            auto_done: false,
            from_menu: false,
            doc: None,
            res: None,
            sel: None,
            tool: Tool::Select,
            catalog: Vec::new(),
            scripts: Vec::new(),
            convs: Vec::new(),
            status: String::new(),
            rev: 0,
            drag: None,
            hover_handle: None,
            saved_cam: None,
            modules: Vec::new(),
            map_names: Vec::new(),
            confirm: None,
            loading: None,
            load_hook_done: false,
            f10_hook: false,
            saved_fade: None,
            specs: HashMap::new(),
            tags: HashMap::new(),
            mod_name: String::new(),
            picker_open: false,
            pick_search: String::new(),
            pick_cursor: 0,
            palette_open: false,
            palette_search: String::new(),
            palette_kind: None,
            obj_search: String::new(),
            show_kind: [true; 9],
            show_markers: true,
            show_handles: true,
            show_grid: false,
            show_walk: false,
            cursor_world: None,
            fly_to: None,
            step_open: false,
            palette_focus: false,
            revert_confirm: false,
            ui_rects: Vec::new(),
            ui_ptr: false,
            ui_kbd: false,
        }
    }
}

impl Editor {
    /// The main menu's Editor button: the editor opens on the next area load.
    /// Ignored while the editor is already open, so the request cannot wait
    /// there for a later area load.
    pub fn request_from_menu(&mut self) {
        if self.active {
            return;
        }
        self.from_menu = true;
        self.auto_done = false;
    }

    fn bump(&mut self) {
        self.rev += 1;
    }

    fn kind_shown(&self, kind: Kind) -> bool {
        self.show_kind[kind as usize]
    }

    /// Cursor (logical px) over an egui panel or window.
    fn over_ui(&self, p: Vec2) -> bool {
        self.ui_ptr || self.ui_rects.iter().any(|r| p.x >= r[0] && p.y >= r[1] && p.x <= r[2] && p.y <= r[3])
    }

    fn pick_filtered(&self) -> Vec<usize> {
        let q = self.pick_search.to_ascii_lowercase();
        let hit = |i: usize| self.modules[i].contains(&q) || self.map_names[i].to_ascii_lowercase().contains(&q);
        (0..self.modules.len()).filter(|&i| q.is_empty() || hit(i)).collect()
    }

    /// Catalog indices matching the palette's kind and search.
    fn filtered(&self) -> Vec<usize> {
        let q = self.palette_search.to_ascii_lowercase();
        self.catalog
            .iter()
            .enumerate()
            .filter(|(_, (k, r, n))| {
                self.palette_kind.is_none_or(|pk| pk == *k) && (q.is_empty() || r.contains(&q) || n.to_ascii_lowercase().contains(&q) || k.name().starts_with(&q))
            })
            .map(|(i, _)| i)
            .collect()
    }

    /// Script, conversation and blueprint-tag lists depend on the mounted
    /// module and on blueprint edits.
    fn refresh_lists(&mut self) {
        use rakata_core::ResourceType;
        let Some(res) = self.res.as_ref() else { return };
        let (ncs, dlg) = (ResourceType::Ncs.type_id(), ResourceType::Dlg.type_id());
        let mut scripts = Vec::new();
        let mut convs = Vec::new();
        for k in res.override_keys().chain(res.module_keys()).chain(res.chitin_keys()) {
            if k.ty == ncs {
                scripts.push(k.name.clone());
            } else if k.ty == dlg {
                convs.push(k.name.clone());
            }
        }
        scripts.sort();
        scripts.dedup();
        convs.sort();
        convs.dedup();
        self.scripts = scripts;
        self.convs = convs;
        self.tags.clear();
    }

    /// Applies an inspector value to a field of the selection. A typed edit
    /// is one undo step; `live` edits (drag values, checkboxes) join the open
    /// step until the widget lets go.
    fn commit_field(&mut self, uid: u64, row: usize, text: &str, live: bool) {
        let (Some(doc), Some(res)) = (self.doc.as_mut(), self.res.as_ref()) else { return };
        let Some(kind) = doc.get(uid).map(|i| i.kind) else { return };
        let Some(spec) = fields(kind).get(row).copied() else { return };
        let fresh = !live || !self.step_open;
        if fresh {
            doc.checkpoint();
        }
        self.step_open = live;
        match doc.set_field(res, uid, &spec, text) {
            Ok(()) => self.status = format!("{} = {}", spec.label, text.trim()),
            Err(e) => {
                if fresh {
                    doc.undo();
                }
                self.status = format!("{}: {e}", spec.label);
            }
        }
        self.specs.clear();
        self.tags.clear();
        self.bump();
    }
}

/// An editor-side view of one document instance.
#[derive(Component)]
struct EdView;

#[derive(Resource, Default)]
struct EdViews {
    rev: u64,
    /// uid -> (root entity, template the model was built from)
    map: HashMap<u64, (Entity, String)>,
}

#[derive(Component)]
struct EditCam {
    yaw: f32,
    pitch: f32,
}

// ---------------------------------------------------------------------------
// enter / leave
// ---------------------------------------------------------------------------

fn auto_enter(args: Res<Args>, mut ed: ResMut<Editor>, mut loaded: MessageReader<AreaLoaded>, mut want: Local<bool>, mut frames: Local<Option<u32>>, mut left: Local<u32>) {
    // Test hook: `K2_EDIT_AT=FRAMES` opens the editor that many frames after
    // the area loads, as F10 would (the --edit path frames the camera
    // itself; this one keeps the in-game view).
    if let Some(at) = std::env::var("K2_EDIT_AT").ok().and_then(|v| v.parse::<u32>().ok()) {
        let n = frames.get_or_insert(0);
        *n += 1;
        if *n == at && !ed.active {
            ed.f10_hook = true;
        }
    }
    // Test hook: `K2_EDIT_LEAVE=FRAMES` leaves the editor (F10) that many
    // frames after it opened.
    if ed.active
        && let Some(n) = std::env::var("K2_EDIT_LEAVE").ok().and_then(|v| v.parse::<u32>().ok())
    {
        *left += 1;
        if *left == n {
            ed.f10_hook = true;
        }
    }
    if loaded.read().count() > 0 && (args.edit || ed.from_menu) && !ed.auto_done {
        ed.auto_done = true;
        *want = true;
    }
    if *want && !ed.active {
        // Picked up by `toggle` this frame.
        ed.status = "enter".into();
    }
    *want = false;
}

#[allow(clippy::too_many_arguments)]
fn toggle(
    mut commands: Commands,
    keys: Res<ButtonInput<KeyCode>>,
    args: Res<Args>,
    game: Res<GameRes>,
    world: Res<WorldRes>,
    mut ed: ResMut<Editor>,
    mut pause: ResMut<PauseState>,
    mut mode: ResMut<CamMode>,
    mut fade: ResMut<Fade>,
    cams: Query<(Entity, &Transform), With<Camera3d>>,
    mut hidden: Query<&mut Visibility, With<ObjId>>,
    mut huds: Query<&mut Visibility, (With<GameHud>, Without<ObjId>)>,
    views: Query<Entity, With<EdView>>,
    mut ev: ResMut<EdViews>,
) {
    let auto = !ed.active && ed.status == "enter";
    let pressed = (keys.just_pressed(KeyCode::F10) && !ed.ui_kbd) || std::mem::take(&mut ed.f10_hook);
    if !(auto || pressed) {
        return;
    }
    if !ed.active {
        // Taken here, not at the open, so a failed open cannot leave it set for a later F10.
        let from_menu = std::mem::take(&mut ed.from_menu);
        let module = world.0.module.as_ref().map(|m| m.name.clone());
        let Some(module) = module else {
            ed.status = "no module loaded".into();
            return;
        };
        let install: Install = game.0.install.clone();
        let mut res = match Resources::open(install) {
            Ok(r) => r,
            Err(e) => {
                ed.status = format!("open install: {e}");
                return;
            }
        };
        let doc = match EditDoc::load(&mut res, &module) {
            Ok(d) => d,
            Err(e) => {
                ed.status = format!("load {module}: {e}");
                return;
            }
        };
        let t0 = std::time::Instant::now();
        ed.catalog = blueprint_catalog(&res).into_iter().map(|(k, r)| (k, r, String::new())).collect();
        info!("editor: {} {} instances, {} blueprints ({:?})", doc.area, doc.insts.len(), ed.catalog.len(), t0.elapsed());
        ed.mod_name = args.mod_name.clone().unwrap_or_else(|| "editor".into());
        ed.modules = res.install.module_names();
        ed.map_names = ed.modules.iter().map(|m| map_name(&res.install, world.0.talk.as_ref(), m)).collect();
        ed.picker_open = args.edit_pick || from_menu;
        ed.pick_search.clear();
        ed.pick_cursor = ed.modules.iter().position(|m| *m == doc.module).unwrap_or(0);
        ed.palette_open = false;
        ed.palette_search.clear();
        ed.obj_search.clear();
        ed.cursor_world = None;
        ed.ui_rects.clear();
        ed.confirm = None;
        ed.loading = None;
        ed.doc = Some(doc);
        ed.res = Some(res);
        ed.refresh_lists();
        ed.sel = None;
        ed.tool = Tool::Select;
        ed.active = true;
        ed.status = format!("editing {module} - Tab palette, Ctrl+S save to {}", mod_path(&ed.mod_name).display());
        ed.bump();
        pause.editor = true;
        ed.saved_fade = Some((fade.alpha, fade.target, fade.rate, fade.wait, fade.color));
        *mode = CamMode::Fly;
        let w = &world.0;
        let focus = w.get(w.party_leader()).map(|o| o.position).or_else(|| w.module.as_ref().map(|m| Vec3::from(m.info.entry_position))).unwrap_or(Vec3::ZERO);
        let focus_b = coords::to_bevy(focus);
        info!("editor: entering via {}, party at {focus:?}", if auto { "--edit" } else { "F10" });
        for (cam, t) in &cams {
            // --edit enters on the frame the area loads, before the follow
            // camera has ever placed the camera (it still sits at the spawn
            // default near the origin, usually inside or outside the area).
            // Frame the entry point from above; F10 in game keeps the view
            // unless it is nowhere near the party.
            let mut t = *t;
            if auto || t.translation.distance(focus_b) > 100.0 {
                t = frame_transform(focus_b);
                commands.entity(cam).insert(t);
            }
            let (yaw, pitch, _) = t.rotation.to_euler(EulerRot::YXZ);
            commands.entity(cam).remove::<FlyCam>().insert((EditCam { yaw, pitch }, PrimaryEguiContext));
            commands.queue(move |w: &mut World| {
                let pc = w.entity_mut(cam).take::<PlayCam>();
                w.resource_mut::<Editor>().saved_cam = pc;
            });
        }
        apply_test_hook(&mut ed);
    } else {
        if ed.doc.as_ref().is_some_and(|d| d.dirty) {
            info!("editor: leaving with unsaved changes");
        }
        ed.active = false;
        ed.picker_open = false;
        ed.ui_ptr = false;
        ed.ui_kbd = false;
        ed.ui_rects.clear();
        ed.loading = None;
        ed.doc = None;
        ed.res = None;
        ed.drag = None;
        pause.editor = false;
        if let Some((alpha, target, rate, wait, color)) = ed.saved_fade.take() {
            *fade = Fade { alpha, target, rate, wait, color };
        }
        let saved = ed.saved_cam.take();
        *mode = if saved.is_some() { CamMode::Play } else { CamMode::Fly };
        for (cam, t) in &cams {
            let mut e = commands.entity(cam);
            e.remove::<EditCam>().remove::<(PrimaryEguiContext, EguiContext, EguiSchedule)>();
            match saved {
                Some(ref _pc) => {}
                None => {
                    let (yaw, pitch, _) = t.rotation.to_euler(EulerRot::YXZ);
                    e.insert(FlyCam { yaw, pitch, speed: 6.0 });
                }
            }
        }
        if let Some(pc) = saved
            && let Some((cam, _)) = cams.iter().next()
        {
            commands.entity(cam).insert(pc);
        }
        for mut v in &mut hidden {
            *v = Visibility::Inherited;
        }
        for mut v in &mut huds {
            *v = Visibility::Inherited;
        }
        for e in &views {
            commands.entity(e).despawn();
        }
        ev.map.clear();
        ev.rev = 0;
    }
}

/// The start view over a point (bevy space): 14 m up, 9 m back, looking down.
fn frame_transform(focus: Vec3) -> Transform {
    Transform::from_translation(focus + Vec3::new(0.0, 14.0, 9.0)).looking_at(focus, Vec3::Y)
}

/// The area's on-enter script (101per: the awakening cutscene) fades the
/// screen to black and the paused world never fades it back in, so the
/// editor held a black overlay over everything. Keep the fade clear while
/// editing; `toggle` restores the game's fade on leaving.
fn clear_fade(mut fade: ResMut<Fade>) {
    if fade.alpha != 0.0 || fade.target != 0.0 {
        fade.alpha = 0.0;
        fade.target = 0.0;
        fade.wait = 0.0;
    }
}

/// `K2_EDIT_TEST="select=TEMPLATE;palette;search=TEXT;kind=NAME;pick;walk;grid"`: drives the editor
/// for screenshots without a mouse.
fn apply_test_hook(ed: &mut Editor) {
    let Ok(spec) = std::env::var("K2_EDIT_TEST") else { return };
    for cmd in spec.split(';') {
        let (k, v) = cmd.split_once('=').unwrap_or((cmd, ""));
        match k {
            "select" => {
                ed.sel = ed.doc.as_ref().and_then(|d| d.insts.iter().find(|i| i.template() == v.to_ascii_lowercase()).map(|i| i.uid));
            }
            "browser" | "palette" => ed.palette_open = true,
            "search" => ed.palette_search = v.to_owned(),
            "kind" => ed.palette_kind = Kind::ALL.into_iter().find(|k| k.name() == v),
            "pick" => ed.picker_open = true,
            "walk" => ed.show_walk = true,
            "grid" => ed.show_grid = true,
            _ => {}
        }
    }
    ed.bump();
}

// ---------------------------------------------------------------------------
// input
// ---------------------------------------------------------------------------

impl Editor {
    fn undo(&mut self) {
        let Some(doc) = self.doc.as_mut() else { return };
        let ok = doc.undo();
        self.status = if ok { "undo".into() } else { "nothing to undo".into() };
        self.after_restore();
    }

    fn redo(&mut self) {
        let Some(doc) = self.doc.as_mut() else { return };
        let ok = doc.redo();
        self.status = if ok { "redo".into() } else { "nothing to redo".into() };
        self.after_restore();
    }

    fn after_restore(&mut self) {
        self.tags.clear();
        if let (Some(doc), Some(uid)) = (self.doc.as_ref(), self.sel)
            && doc.get(uid).is_none()
        {
            self.sel = None;
        }
        self.bump();
    }

    fn save(&mut self) {
        let Some(doc) = self.doc.as_mut() else { return };
        save_doc(doc, &self.mod_name, &mut self.status);
    }

    fn delete_sel(&mut self) {
        let (Some(doc), Some(uid)) = (self.doc.as_mut(), self.sel) else { return };
        doc.checkpoint();
        doc.delete(uid);
        self.sel = None;
        self.status = "deleted".into();
        self.bump();
    }

    fn duplicate_sel(&mut self) {
        let (Some(doc), Some(uid)) = (self.doc.as_mut(), self.sel) else { return };
        doc.checkpoint();
        if let Some(n) = doc.duplicate(uid, [1.0, 0.0, 0.0]) {
            self.sel = Some(n);
            self.status = "duplicated".into();
        }
        self.bump();
    }

    /// Swaps the selection's blueprint for another of the same kind.
    fn set_template(&mut self, uid: u64, template: &str) {
        let t = template.trim().to_ascii_lowercase();
        let Some(kind) = self.doc.as_ref().and_then(|d| d.get(uid)).map(|i| i.kind) else { return };
        let Some(doc) = self.doc.as_mut() else { return };
        if !self.catalog.iter().any(|(k, r, _)| *k == kind && *r == t) && !doc.blueprints.contains_key(&t) {
            self.status = format!("no {} blueprint named {t}", kind.name());
            return;
        }
        doc.checkpoint();
        doc.retemplate(uid, &t);
        self.status = format!("template = {t}");
        self.specs.clear();
        self.tags.clear();
        self.bump();
    }

    fn rotate_sel(&mut self, turn: f32) {
        let (Some(doc), Some(uid)) = (self.doc.as_mut(), self.sel) else { return };
        doc.checkpoint();
        doc.rotate(uid, turn);
        self.bump();
    }

    /// Starts placing a blueprint from the palette (polygon kinds collect
    /// points first).
    fn pick_blueprint(&mut self, kind: Kind, resref: &str) {
        self.tool = if kind.polygon_labels().is_some() { Tool::Polygon(kind, resref.to_owned(), Vec::new()) } else { Tool::Place(kind, resref.to_owned()) };
        self.status = if kind.polygon_labels().is_some() {
            format!("{} {resref}: click the walkmesh for points, Enter finishes", kind.name())
        } else {
            format!("place {} {resref}: click the walkmesh", kind.name())
        };
        self.bump();
    }

    fn finish_polygon(&mut self) {
        let Tool::Polygon(kind, r, pts) = self.tool.clone() else { return };
        if pts.len() >= 3 {
            let (Some(doc), Some(res)) = (self.doc.as_mut(), self.res.as_ref()) else { return };
            let origin = pts[0];
            let rel: Vec<[f32; 3]> = pts.iter().map(|p| (*p - origin).to_array()).collect();
            doc.checkpoint();
            match doc.add(res, kind, &r, origin.to_array(), 0.0, &rel) {
                Ok(uid) => {
                    self.sel = Some(uid);
                    self.status = format!("added {} {r} ({} points)", kind.name(), pts.len());
                }
                Err(e) => self.status = format!("add {r}: {e}"),
            }
            self.tool = Tool::Select;
        } else {
            self.status = "a polygon needs 3 points".into();
        }
        self.bump();
    }
}

fn save_doc(doc: &mut EditDoc, mod_name: &str, status: &mut String) {
    let path = mod_path(mod_name);
    *status = match doc.save(&path) {
        Ok(count) => {
            info!("editor: saved {count} resource(s) to {}", path.display());
            format!("saved {count} resource(s) to {} - play with --mod-dir {}", path.display(), path.display())
        }
        Err(e) => format!("save failed: {e}"),
    };
}

/// Keyboard shortcuts for the 3D view. Ctrl+S, Ctrl+O and Tab are taken in
/// the egui pass (they work with a text field focused / need to be hidden
/// from egui's focus handling); everything here yields to egui typing.
fn hotkeys(keys: Res<ButtonInput<KeyCode>>, scroll: Res<AccumulatedMouseScroll>, mut ed: ResMut<Editor>) {
    if ed.ui_kbd || ed.picker_open {
        return;
    }
    let ctrl = keys.pressed(KeyCode::ControlLeft) || keys.pressed(KeyCode::ControlRight);
    if ctrl && keys.just_pressed(KeyCode::KeyZ) {
        ed.undo();
    }
    if ctrl && keys.just_pressed(KeyCode::KeyY) {
        ed.redo();
    }
    if ed.sel.is_some() {
        if ctrl && keys.just_pressed(KeyCode::KeyD) {
            ed.duplicate_sel();
        }
        if keys.just_pressed(KeyCode::Delete) {
            ed.delete_sel();
        }
        let mut turn = 0.0;
        if !ctrl && keys.just_pressed(KeyCode::KeyQ) {
            turn += 15f32.to_radians();
        }
        if !ctrl && keys.just_pressed(KeyCode::KeyE) {
            turn -= 15f32.to_radians();
        }
        if keys.pressed(KeyCode::KeyR) && !ed.ui_ptr {
            let notches = match scroll.unit {
                MouseScrollUnit::Line => scroll.delta.y,
                MouseScrollUnit::Pixel => scroll.delta.y / 40.0,
            };
            turn += notches * 15f32.to_radians();
        }
        if turn != 0.0 {
            ed.rotate_sel(turn);
        }
    }
    if keys.just_pressed(KeyCode::Enter) && matches!(ed.tool, Tool::Polygon(..)) {
        ed.finish_polygon();
    }
    if keys.just_pressed(KeyCode::Escape) {
        ed.tool = Tool::Select;
        ed.status = "select".into();
        ed.bump();
    }
}

// ---------------------------------------------------------------------------
// load map
// ---------------------------------------------------------------------------

/// Opens `module` through the world (rooms, walkmesh, objects), as a module
/// transition does; `on_area_loaded` rebuilds the document when it is ready.
/// Unsaved edits need the same choice made twice.
fn request_load(ed: &mut Editor, world: &mut WorldRes, need: &mut NeedAreaLoad, module: &str) {
    if ed.doc.as_ref().is_some_and(|d| d.dirty) && ed.confirm.as_deref() != Some(module) {
        ed.confirm = Some(module.to_owned());
        ed.status = format!("unsaved edits: Enter again discards them and loads {module}, Ctrl+S saves first, Esc cancels");
        return;
    }
    ed.confirm = None;
    if !enter_module(&mut world.0, need, module, None) {
        ed.status = format!("cannot load {module}");
        return;
    }
    info!("editor: loading {module}");
    ed.status = format!("loading {module}...");
    ed.loading = Some(module.to_owned());
    ed.picker_open = false;
    ed.doc = None;
    ed.sel = None;
    ed.drag = None;
    ed.tool = Tool::Select;
    ed.bump();
}

/// Test hook: `K2_EDIT_SCRIPT="FRAME:cmd,args;..."` feeds the editor input
/// through Bevy's own input messages (what winit would send), counted in
/// frames since the app started. Commands: `move,X,Y` (logical px),
/// `down` / `up` (left button), `type,TEXT`, `key,NAME,down|up` (ctrl shift
/// tab del enter esc f10 q e r z y s d o), `dump` (log the selection and document state).
#[allow(clippy::too_many_arguments)]
fn script_hook(
    ed: Res<Editor>,
    mut windows: Query<(Entity, &mut Window), With<PrimaryWindow>>,
    mut moved: MessageWriter<CursorMoved>,
    mut buttons: MessageWriter<MouseButtonInput>,
    mut keys: MessageWriter<KeyboardInput>,
    mut window_events: MessageWriter<bevy::window::WindowEvent>,
    mut frame: Local<u32>,
    mut script: Local<Option<Vec<(u32, String)>>>,
) {
    let Ok(spec) = std::env::var("K2_EDIT_SCRIPT") else { return };
    let cmds = script.get_or_insert_with(|| {
        spec.split(';').filter_map(|c| c.split_once(':')).filter_map(|(n, c)| Some((n.trim().parse().ok()?, c.trim().to_owned()))).collect()
    });
    *frame += 1;
    let Ok((win, mut window)) = windows.single_mut() else { return };
    for (_, cmd) in cmds.iter().filter(|(n, _)| n == &*frame) {
        let parts: Vec<&str> = cmd.split(',').collect();
        match parts[0] {
            "move" => {
                let p = Vec2::new(parts[1].parse().unwrap_or(0.0), parts[2].parse().unwrap_or(0.0));
                window.set_cursor_position(Some(p));
                let m = CursorMoved { window: win, position: p, delta: None };
                moved.write(m.clone());
                window_events.write(bevy::window::WindowEvent::CursorMoved(m));
            }
            "down" | "up" => {
                let state = if parts[0] == "down" { ButtonState::Pressed } else { ButtonState::Released };
                let m = MouseButtonInput { button: MouseButton::Left, state, window: win };
                buttons.write(m);
                window_events.write(bevy::window::WindowEvent::MouseButtonInput(m));
            }
            "key" => {
                let name = parts.get(1).copied().unwrap_or("");
                let state = if parts.get(2) == Some(&"up") { ButtonState::Released } else { ButtonState::Pressed };
                let (code, logical) = match name {
                    "ctrl" => (KeyCode::ControlLeft, Key::Control),
                    "shift" => (KeyCode::ShiftLeft, Key::Shift),
                    "tab" => (KeyCode::Tab, Key::Tab),
                    "del" => (KeyCode::Delete, Key::Delete),
                    "enter" => (KeyCode::Enter, Key::Enter),
                    "esc" => (KeyCode::Escape, Key::Escape),
                    "f10" => (KeyCode::F10, Key::F10),
                    "q" => (KeyCode::KeyQ, Key::Character("q".into())),
                    "e" => (KeyCode::KeyE, Key::Character("e".into())),
                    "r" => (KeyCode::KeyR, Key::Character("r".into())),
                    "z" => (KeyCode::KeyZ, Key::Character("z".into())),
                    "y" => (KeyCode::KeyY, Key::Character("y".into())),
                    "s" => (KeyCode::KeyS, Key::Character("s".into())),
                    "d" => (KeyCode::KeyD, Key::Character("d".into())),
                    "o" => (KeyCode::KeyO, Key::Character("o".into())),
                    _ => continue,
                };
                let m = KeyboardInput { key_code: code, logical_key: logical, state, text: None, repeat: false, window: win };
                keys.write(m.clone());
                window_events.write(bevy::window::WindowEvent::KeyboardInput(m));
            }
            "type" => {
                for c in parts.get(1).copied().unwrap_or("").chars() {
                    let txt = c.to_string();
                    for state in [ButtonState::Pressed, ButtonState::Released] {
                        let m = KeyboardInput {
                            key_code: KeyCode::Unidentified(bevy::input::keyboard::NativeKeyCode::Unidentified),
                            logical_key: Key::Character(txt.as_str().into()),
                            state,
                            text: Some(txt.as_str().into()),
                            repeat: false,
                            window: win,
                        };
                        keys.write(m.clone());
                        window_events.write(bevy::window::WindowEvent::KeyboardInput(m));
                    }
                }
            }
            "dump" => {
                let sel = ed.sel.and_then(|u| ed.doc.as_ref()?.get(u)).map(|i| format!("{} #{} {} pos {:?} bearing {:.1} deg", i.kind.name(), i.uid, i.template(), i.position(), i.bearing().to_degrees()));
                info!(
                    "editor-script: frame {} window {}x{} scale {:.3} sel [{}] insts {} dirty {} tool {:?} picker {} palette {} ui_ptr {} ui_kbd {} status {}",
                    *frame,
                    window.width(),
                    window.height(),
                    window.scale_factor(),
                    sel.unwrap_or_default(),
                    ed.doc.as_ref().map_or(0, |d| d.insts.len()),
                    ed.doc.as_ref().is_some_and(|d| d.dirty),
                    ed.tool,
                    ed.picker_open,
                    ed.palette_open,
                    ed.ui_ptr,
                    ed.ui_kbd,
                    ed.status
                );
            }
            _ => {}
        }
    }
}

/// `K2_EDIT_LOAD=MODULE` opens that module 30 frames after the editor is up
/// (test hook).
fn load_hook(mut ed: ResMut<Editor>, mut world: ResMut<WorldRes>, mut need: ResMut<NeedAreaLoad>, mut frames: Local<u32>) {
    let Ok(m) = std::env::var("K2_EDIT_LOAD") else { return };
    if ed.doc.is_none() || ed.load_hook_done {
        return;
    }
    *frames += 1;
    if *frames >= 30 {
        ed.load_hook_done = true;
        request_load(&mut ed, &mut world, &mut need, &m.to_ascii_lowercase());
    }
}

/// The world finished loading the module the picker asked for: build its
/// document and frame the camera over the party's start.
fn on_area_loaded(
    mut loaded: MessageReader<AreaLoaded>,
    mut ed: ResMut<Editor>,
    world: Res<WorldRes>,
    mut ev: ResMut<EdViews>,
    mut cams: Query<(&mut Transform, &mut EditCam)>,
) {
    if loaded.read().count() == 0 || !ed.active {
        return;
    }
    let Some(module) = ed.loading.take() else { return };
    let ed = &mut *ed;
    let Some(mut res) = ed.res.take() else { return };
    let t0 = std::time::Instant::now();
    match EditDoc::load(&mut res, &module) {
        Ok(doc) => {
            ed.catalog = blueprint_catalog(&res).into_iter().map(|(k, r)| (k, r, String::new())).collect();
            info!("editor: {} {} instances, {} blueprints ({:?})", doc.area, doc.insts.len(), ed.catalog.len(), t0.elapsed());
            ed.status = format!("loaded {module} - Ctrl+O loads another map, Ctrl+S saves to {}", mod_path(&ed.mod_name).display());
            ed.doc = Some(doc);
        }
        Err(e) => ed.status = format!("load {module}: {e}"),
    }
    ed.res = Some(res);
    ed.refresh_lists();
    ed.specs.clear();
    ev.map.clear();
    ev.rev = 0;
    let w = &world.0;
    let focus = w.get(w.party_leader()).map(|o| o.position).or_else(|| w.module.as_ref().map(|m| Vec3::from(m.info.entry_position))).unwrap_or(Vec3::ZERO);
    for (mut t, mut cam) in &mut cams {
        *t = frame_transform(coords::to_bevy(focus));
        let (yaw, pitch, _) = t.rotation.to_euler(EulerRot::YXZ);
        cam.yaw = yaw;
        cam.pitch = pitch;
    }
    ed.bump();
}

/// Fly camera on real time (the virtual clock is paused while editing).
fn edit_cam(
    time: Res<Time<Real>>,
    keys: Res<ButtonInput<KeyCode>>,
    buttons: Res<ButtonInput<MouseButton>>,
    motion: Res<AccumulatedMouseMotion>,
    mut ed: ResMut<Editor>,
    mut q: Query<(&mut Transform, &mut EditCam)>,
) {
    let dt = time.delta_secs();
    let fly_to = ed.fly_to.take();
    let ctrl = keys.pressed(KeyCode::ControlLeft) || keys.pressed(KeyCode::ControlRight);
    for (mut t, mut cam) in &mut q {
        if buttons.pressed(MouseButton::Right) {
            cam.yaw -= motion.delta.x * 0.003;
            cam.pitch = (cam.pitch - motion.delta.y * 0.003).clamp(-1.54, 1.54);
        }
        if let Some(p) = fly_to {
            let target = coords::to_bevy(p);
            let f = frame_transform(target);
            t.translation = f.translation - Vec3::new(0.0, 8.0, 4.0);
            let (yaw, pitch, _) = t.looking_at(target, Vec3::Y).rotation.to_euler(EulerRot::YXZ);
            cam.yaw = yaw;
            cam.pitch = pitch;
        }
        t.rotation = Quat::from_euler(EulerRot::YXZ, cam.yaw, cam.pitch, 0.0);
        if ed.ui_kbd || ed.picker_open || ctrl {
            continue;
        }
        let (fwd, right) = (*t.forward(), *t.right());
        let mut v = Vec3::ZERO;
        for (k, d) in [(KeyCode::KeyW, fwd), (KeyCode::KeyS, -fwd), (KeyCode::KeyD, right), (KeyCode::KeyA, -right), (KeyCode::Space, Vec3::Y), (KeyCode::KeyC, -Vec3::Y)] {
            if keys.pressed(k) {
                v += d;
            }
        }
        let boost = if keys.pressed(KeyCode::ShiftLeft) { 4.0 } else { 1.0 };
        t.translation += v.normalize_or_zero() * 6.0 * boost * dt;
    }
}

/// Cursor ray in game space (Z-up, area coordinates).
fn cursor_ray(
    windows: &Query<&Window, With<PrimaryWindow>>,
    cams: &Query<(&Camera, &GlobalTransform), With<EditCam>>,
    area: &Query<&GlobalTransform, With<AreaRoot>>,
) -> Option<(Vec3, Vec3, Vec2)> {
    let w = windows.single().ok()?;
    let c = w.cursor_position()?;
    let (cam, cgt) = cams.single().ok()?;
    let ray = cam.viewport_to_world(cgt, c).ok()?;
    let inv = area.single().ok()?.affine().inverse();
    Some((inv.transform_point3(ray.origin), inv.transform_vector3(*ray.direction).normalize(), c))
}

/// First walkmesh point along a game-space ray.
fn ground_hit(world: &WorldRes, o: Vec3, d: Vec3) -> Option<Vec3> {
    let walk = &world.0.walk;
    let mut t = 0.0;
    while t < 250.0 {
        let p = o + d * t;
        if let Some(h) = walk.ground(p.x, p.y, p.z + 0.05)
            && h.z >= p.z - 0.1
        {
            return Some(Vec3::new(p.x, p.y, h.z));
        }
        t += 0.08;
    }
    None
}

/// Pick radius and center height per kind.
fn pick_shape(kind: Kind) -> (f32, f32) {
    match kind {
        Kind::Creature => (0.5, 0.9),
        Kind::Door => (1.0, 1.2),
        Kind::Placeable => (0.6, 0.5),
        _ => (0.4, 0.4),
    }
}

/// Kinds with a facing the bearing knob can turn.
fn has_facing(kind: Kind) -> bool {
    matches!(kind, Kind::Creature | Kind::Door | Kind::Placeable | Kind::Waypoint)
}

/// Handle length (game space) for a camera at `cam` (area-local): about the
/// same size on screen at any distance.
fn handle_len(p: Vec3, cam: Vec3) -> f32 {
    (p.distance(cam) * 0.13).clamp(0.6, 8.0)
}

const AXES: [Vec3; 3] = [Vec3::X, Vec3::Y, Vec3::Z];

/// Where the bearing knob sits (area-local) for an instance at `p`.
fn knob_pos(p: Vec3, bearing: f32, len: f32) -> Vec3 {
    let [dx, dy] = k2_data::module::dir_from_bearing(bearing);
    p + Vec3::new(dx, dy, 0.0) * len * 0.75 + Vec3::Z * 0.05
}

fn seg_dist(p: Vec2, a: Vec2, b: Vec2) -> f32 {
    let ab = b - a;
    let l2 = ab.length_squared();
    let t = if l2 < 1e-6 { 0.0 } else { ((p - a).dot(ab) / l2).clamp(0.0, 1.0) };
    p.distance(a + ab * t)
}

/// The handle under the cursor, if any (screen-space test, logical px).
#[allow(clippy::too_many_arguments)]
fn handle_at(cam: &Camera, cgt: &GlobalTransform, agt: &GlobalTransform, p: Vec3, bearing: f32, kind: Kind, cursor: Vec2) -> Option<Handle> {
    let cam_local = agt.affine().inverse().transform_point3(cgt.translation());
    let len = handle_len(p, cam_local);
    let proj = |q: Vec3| cam.world_to_viewport(cgt, agt.transform_point(q)).ok();
    let o = proj(p)?;
    if has_facing(kind)
        && let Some(k) = proj(knob_pos(p, bearing, len))
        && k.distance(cursor) < 10.0
    {
        return Some(Handle::Yaw);
    }
    let mut best: Option<(f32, usize)> = None;
    for (i, a) in AXES.iter().enumerate() {
        let Some(tip) = proj(p + *a * len) else { continue };
        let d = seg_dist(cursor, o, tip);
        if d < 8.0 && best.is_none_or(|b| d < b.0) {
            best = Some((d, i));
        }
    }
    best.map(|(_, i)| Handle::Axis(i))
}

/// Parameter along the line `p0 + axis * t` closest to the ray `o + d * s`.
fn axis_param(p0: Vec3, axis: Vec3, o: Vec3, d: Vec3) -> Option<f32> {
    let w0 = p0 - o;
    let b = axis.dot(d);
    let denom = 1.0 - b * b;
    if denom < 1e-4 {
        return None;
    }
    Some((b * d.dot(w0) - axis.dot(w0)) / denom)
}

fn wrap_pi(a: f32) -> f32 {
    let t = std::f32::consts::TAU;
    (a + std::f32::consts::PI).rem_euclid(t) - std::f32::consts::PI
}

#[allow(clippy::too_many_arguments)]
fn mouse(
    buttons: Res<ButtonInput<MouseButton>>,
    keys: Res<ButtonInput<KeyCode>>,
    windows: Query<&Window, With<PrimaryWindow>>,
    cams: Query<(&Camera, &GlobalTransform), With<EditCam>>,
    area: Query<&GlobalTransform, With<AreaRoot>>,
    world: Res<WorldRes>,
    mut ed: ResMut<Editor>,
) {
    let Some((o, d, cursor)) = cursor_ray(&windows, &cams, &area) else { return };
    let ed = &mut *ed;
    let over_ui = ed.over_ui(cursor);
    let sel_info = ed.sel.and_then(|u| ed.doc.as_ref()?.get(u)).map(|i| (i.uid, i.kind, Vec3::from(i.position()), i.bearing()));
    ed.hover_handle = None;
    if ed.show_handles
        && ed.drag.is_none()
        && !over_ui
        && matches!(ed.tool, Tool::Select)
        && let Some((_, kind, p, b)) = sel_info
        && ed.kind_shown(kind)
        && let (Ok((cam, cgt)), Ok(agt)) = (cams.single(), area.single())
    {
        ed.hover_handle = handle_at(cam, cgt, agt, p, b, kind, cursor);
    }

    if buttons.just_pressed(MouseButton::Left) && !over_ui {
        match ed.tool.clone() {
            Tool::Select => {
                if let (Some(h), Some((uid, _, p, _))) = (ed.hover_handle, sel_info) {
                    let mode = match h {
                        Handle::Axis(i) => {
                            let axis = AXES[i];
                            DragMode::Axis { axis, origin: p, t0: axis_param(p, axis, o, d).unwrap_or(0.0) }
                        }
                        Handle::Yaw => DragMode::Yaw,
                    };
                    ed.drag = Some(Drag { uid, started: false, press: cursor, last: cursor, mode });
                } else {
                    let doc = ed.doc.as_ref();
                    let hit = doc.and_then(|doc| {
                        doc.insts
                            .iter()
                            .filter(|i| ed.kind_shown(i.kind))
                            .filter_map(|i| {
                                let (r, h) = pick_shape(i.kind);
                                let c = Vec3::from(i.position()) + Vec3::Z * h;
                                ray_sphere(o, d, c, r).map(|t| (t, i.uid))
                            })
                            .min_by(|a, b| a.0.total_cmp(&b.0))
                            .map(|(_, uid)| uid)
                    });
                    ed.sel = hit;
                    if let Some(uid) = hit {
                        ed.drag = Some(Drag { uid, started: false, press: cursor, last: cursor, mode: DragMode::Free });
                    }
                    ed.bump();
                }
            }
            Tool::Place(kind, r) => {
                if let (Some(p), Some(doc), Some(res)) = (ground_hit(&world, o, d), ed.doc.as_mut(), ed.res.as_ref()) {
                    doc.checkpoint();
                    match doc.add(res, kind, &r, p.to_array(), 0.0, &[]) {
                        Ok(uid) => {
                            ed.sel = Some(uid);
                            ed.status = format!("added {} {r} at ({:.1}, {:.1}, {:.1})", kind.name(), p.x, p.y, p.z);
                        }
                        Err(e) => {
                            doc.undo();
                            ed.status = format!("add {r}: {e}");
                        }
                    }
                    ed.bump();
                }
            }
            Tool::Polygon(kind, r, mut pts) => {
                if let Some(p) = ground_hit(&world, o, d) {
                    pts.push(p);
                    ed.status = format!("{} {r}: {} points, Enter finishes", kind.name(), pts.len());
                    ed.tool = Tool::Polygon(kind, r, pts);
                    ed.bump();
                }
            }
        }
    }
    if !buttons.pressed(MouseButton::Left) {
        ed.drag = None;
        return;
    }
    let Some(drag) = ed.drag.as_mut() else { return };
    let handle_drag = !matches!(drag.mode, DragMode::Free);
    if !drag.started && !handle_drag && cursor.distance(drag.press) < 4.0 {
        return;
    }
    let first = !drag.started;
    drag.started = true;
    let dy = cursor.y - drag.last.y;
    drag.last = cursor;
    let (uid, mode) = (drag.uid, drag.mode);
    let Some(doc) = ed.doc.as_mut() else { return };
    let Some(inst) = doc.get(uid) else { return };
    let (cur, bearing) = (Vec3::from(inst.position()), inst.bearing());
    let shift = keys.pressed(KeyCode::ShiftLeft) || keys.pressed(KeyCode::ShiftRight);
    match mode {
        DragMode::Free => {
            let to = if shift { Some(cur - Vec3::Z * (dy * 0.01)) } else { ground_hit(&world, o, d) };
            if let Some(p) = to {
                if first {
                    doc.checkpoint();
                }
                doc.move_to(uid, p.to_array());
                ed.status = format!("move to ({:.2}, {:.2}, {:.2})", p.x, p.y, p.z);
                ed.bump();
            }
        }
        DragMode::Axis { axis, origin, t0 } => {
            if let Some(t) = axis_param(origin, axis, o, d) {
                if first {
                    doc.checkpoint();
                }
                let p = origin + axis * (t - t0);
                doc.move_to(uid, p.to_array());
                ed.status = format!("move to ({:.2}, {:.2}, {:.2})", p.x, p.y, p.z);
                ed.bump();
            }
        }
        DragMode::Yaw => {
            if d.z.abs() > 1e-4 {
                let s = (cur.z - o.z) / d.z;
                let h = o + d * s;
                let target = k2_data::module::bearing_from_dir(h.x - cur.x, h.y - cur.y);
                let delta = wrap_pi(target - bearing);
                if delta.abs() > 1e-4 {
                    if first {
                        doc.checkpoint();
                    }
                    doc.rotate(uid, delta);
                    ed.status = format!("bearing {:.0} deg", (bearing + delta).to_degrees());
                    ed.bump();
                }
            }
        }
    }
}

/// The ground point under the cursor, for the status bar and the placement
/// marker. Only when the cursor moved (the march is a few thousand lookups).
fn cursor_probe(
    motion: Res<AccumulatedMouseMotion>,
    windows: Query<&Window, With<PrimaryWindow>>,
    cams: Query<(&Camera, &GlobalTransform), With<EditCam>>,
    area: Query<&GlobalTransform, With<AreaRoot>>,
    world: Res<WorldRes>,
    mut ed: ResMut<Editor>,
    mut last_rev: Local<u64>,
) {
    let Some((o, d, cursor)) = cursor_ray(&windows, &cams, &area) else {
        ed.cursor_world = None;
        return;
    };
    if ed.over_ui(cursor) {
        ed.cursor_world = None;
        return;
    }
    if motion.delta == Vec2::ZERO && ed.cursor_world.is_some() && *last_rev == ed.rev {
        return;
    }
    *last_rev = ed.rev;
    ed.cursor_world = ground_hit(&world, o, d);
}

fn ray_sphere(o: Vec3, d: Vec3, c: Vec3, r: f32) -> Option<f32> {
    let oc = o - c;
    let b = oc.dot(d);
    let disc = b * b - (oc.length_squared() - r * r);
    if disc < 0.0 {
        return None;
    }
    let t = -b - disc.sqrt();
    (t > 0.0).then_some(t)
}

// ---------------------------------------------------------------------------
// views
// ---------------------------------------------------------------------------

fn model_spec(ed: &mut Editor, gd: &GameData, kind: Kind, template: &str) -> Option<CreatureSpec> {
    let key = (kind, template.to_owned());
    if let Some(s) = ed.specs.get(&key) {
        return s.clone();
    }
    let res = ed.res.as_ref()?;
    let doc = ed.doc.as_ref()?;
    // Forked blueprints exist only in the document until saved; their model
    // fields are the source's, so fall back to loading the source.
    let t = template;
    let spec = match kind {
        Kind::Creature => k2_data::Creature::load(res, t)
            .or_else(|_| fork_source(doc, res, t, |r, n| k2_data::Creature::load(r, n)))
            .ok()
            .and_then(|c| c.models(res, &gd.tables).ok())
            .map(|m| CreatureSpec { body: m.body, head: m.head, texture: m.body_texture, own_idle: false }),
        Kind::Placeable => k2_data::Placeable::load(res, t)
            .or_else(|_| fork_source(doc, res, t, |r, n| k2_data::Placeable::load(r, n)))
            .ok()
            .and_then(|p| p.model(res, &gd.tables).ok())
            .map(|m| CreatureSpec { body: m, head: None, texture: None, own_idle: true }),
        Kind::Door => k2_data::Door::load(res, t)
            .or_else(|_| fork_source(doc, res, t, |r, n| k2_data::Door::load(r, n)))
            .ok()
            .and_then(|p| p.model(res, &gd.tables).ok())
            .map(|m| CreatureSpec { body: m, head: None, texture: None, own_idle: false }),
        _ => None,
    };
    ed.specs.insert(key, spec.clone());
    spec
}

/// Loads a forked blueprint's source (the part before `_eNN`), since model
/// fields are never edited.
fn fork_source<T, E>(doc: &EditDoc, res: &Resources, fork: &str, load: impl Fn(&Resources, &str) -> Result<T, E>) -> Result<T, E> {
    let base = if doc.blueprints.contains_key(fork) { fork.rsplit_once("_e").map_or(fork, |(b, _)| b) } else { fork };
    // The source may itself be longer than the 12 chars kept in the fork name.
    let full = doc
        .insts
        .iter()
        .map(|i| i.template())
        .chain(res.override_keys().chain(res.module_keys()).chain(res.chitin_keys()).map(|k| k.name.clone()))
        .find(|n| n.starts_with(base) && !doc.blueprints.contains_key(n))
        .unwrap_or_else(|| base.to_owned());
    load(res, &full)
}

#[allow(clippy::too_many_arguments)]
fn sync_views(
    mut commands: Commands,
    game: Res<GameRes>,
    gd: Res<GameData>,
    mut cache: ResMut<AssetCache>,
    mut images: ResMut<Assets<Image>>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<KMaterial>>,
    mut ed: ResMut<Editor>,
    mut views: ResMut<EdViews>,
    area: Query<Entity, With<AreaRoot>>,
    mut tfs: Query<(&mut Transform, &mut Visibility), With<EdView>>,
) {
    if views.rev == ed.rev {
        return;
    }
    views.rev = ed.rev;
    let Ok(area) = area.single() else { return };
    let shown = ed.show_kind;
    let Some(doc) = ed.doc.as_ref() else { return };
    let insts: Vec<(u64, Kind, String, [f32; 3], f32)> = doc.insts.iter().map(|i| (i.uid, i.kind, i.template(), i.position(), i.bearing())).collect();
    let live: std::collections::HashSet<u64> = insts.iter().map(|i| i.0).collect();
    views.map.retain(|uid, (e, _)| {
        let keep = live.contains(uid);
        if !keep {
            commands.entity(*e).despawn();
        }
        keep
    });
    let mut loader = Loader { res: &game.0, cache: &mut cache, images: &mut images, meshes: &mut meshes, materials: &mut materials };
    for (uid, kind, template, pos, bearing) in insts {
        let xf = Transform::from_translation(Vec3::from(pos)).with_rotation(Quat::from_rotation_z(bearing));
        let vis = if shown[kind as usize] { Visibility::Inherited } else { Visibility::Hidden };
        if let Some((e, t)) = views.map.get(&uid)
            && *t == template
        {
            if let Ok((mut tf, mut v)) = tfs.get_mut(*e) {
                *tf = xf;
                if *v != vis {
                    *v = vis;
                }
            }
            continue;
        }
        if let Some((e, _)) = views.map.remove(&uid) {
            commands.entity(e).despawn();
        }
        let root = commands.spawn((Name::new(format!("edit {} {template}", kind.name())), EdView, xf, vis, ChildOf(area))).id();
        if let Some(spec) = model_spec(&mut ed, &gd, kind, &template) {
            spawn_creature(&mut commands, &mut loader, &spec, root, Transform::IDENTITY);
        }
        views.map.insert(uid, (root, template));
    }
}

fn hide_world(mut q: Query<&mut Visibility, (With<ObjId>, Without<EdView>)>, mut huds: Query<&mut Visibility, (With<GameHud>, Without<ObjId>, Without<EdView>)>) {
    for mut v in q.iter_mut().chain(huds.iter_mut()) {
        if *v != Visibility::Hidden {
            *v = Visibility::Hidden;
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn gizmos(
    mut g: Gizmos,
    mut hg: Gizmos<HandleGizmos>,
    ed: Res<Editor>,
    world: Res<WorldRes>,
    area: Query<&GlobalTransform, With<AreaRoot>>,
    cams: Query<&GlobalTransform, With<EditCam>>,
) {
    let Ok(agt) = area.single() else { return };
    let Some(doc) = ed.doc.as_ref() else { return };
    let w = |p: Vec3| agt.transform_point(p);
    let cam_local = cams.single().map(|c| agt.affine().inverse().transform_point3(c.translation())).unwrap_or(Vec3::ZERO);
    if ed.show_walk {
        let walk = &world.0.walk;
        let mats = walk.materials();
        for n in 0..walk.face_count() {
            let f = k2_walk::FaceId(n as u32);
            let Some(vs) = walk.face_vertices(f) else { continue };
            if (vs[0] - cam_local).length_squared() > 45.0 * 45.0 {
                continue;
            }
            let ok = walk.face_material(f).is_some_and(|m| mats.walkable(m));
            let c = if ok { Color::srgba(0.2, 0.9, 0.4, 0.55) } else { Color::srgba(0.9, 0.25, 0.25, 0.4) };
            let lift = Vec3::Z * 0.03;
            g.linestrip([w(vs[0] + lift), w(vs[1] + lift), w(vs[2] + lift), w(vs[0] + lift)], c);
        }
    }
    if ed.show_grid {
        let base = ed.sel.and_then(|u| doc.get(u)).map(|i| Vec3::from(i.position())).or(ed.cursor_world).unwrap_or(cam_local);
        let z = world.0.walk.ground(base.x, base.y, base.z + 1.0).map_or(base.z, |h| h.z) + 0.02;
        let centre = Vec3::new(base.x.round(), base.y.round(), z);
        let (_, rot, _) = agt.to_scale_rotation_translation();
        g.grid(Isometry3d::new(w(centre), rot), UVec2::splat(40), Vec2::splat(1.0), Color::srgba(0.8, 0.8, 0.9, 0.25));
    }
    if ed.show_markers {
        for i in &doc.insts {
            if !ed.kind_shown(i.kind) {
                continue;
            }
            let p = Vec3::from(i.position());
            let sel = ed.sel == Some(i.uid);
            let c = if sel { Color::srgb(1.0, 0.9, 0.2) } else { marker_color(i.kind) };
            match i.kind {
                Kind::Waypoint => {
                    g.line(w(p), w(p + Vec3::Z * 1.6), c);
                    let [dx, dy] = k2_data::module::dir_from_bearing(i.bearing());
                    g.line(w(p + Vec3::Z * 1.6), w(p + Vec3::Z * 1.3 + Vec3::new(dx, dy, 0.0) * 0.4), c);
                }
                Kind::Trigger | Kind::Encounter => {
                    let pts: Vec<Vec3> = i.polygon().iter().map(|q| w(p + Vec3::from(*q) + Vec3::Z * 0.05)).collect();
                    if pts.len() > 1 {
                        g.linestrip(pts.iter().copied().chain(pts.first().copied()), c);
                    }
                    g.sphere(Isometry3d::from_translation(w(p)), 0.15, c);
                }
                Kind::Sound => {
                    g.sphere(Isometry3d::from_translation(w(p)), 0.35, c);
                }
                Kind::Store => {
                    g.cube(Transform::from_translation(w(p + Vec3::Z * 0.3)).with_scale(Vec3::splat(0.5)), c);
                }
                Kind::Camera => {
                    g.sphere(Isometry3d::from_translation(w(p)), 0.2, c);
                }
                _ => {
                    if sel {
                        let (r, h) = pick_shape(i.kind);
                        g.cube(Transform::from_translation(w(p + Vec3::Z * h)).with_scale(Vec3::new(r * 2.0, r.max(h) * 2.0, r * 2.0)), c);
                    }
                }
            }
            if sel {
                g.sphere(Isometry3d::from_translation(w(p)), 0.08, c);
            }
        }
    }
    // Move / rotate handles on the selection.
    if ed.show_handles
        && matches!(ed.tool, Tool::Select)
        && let Some(i) = ed.sel.and_then(|u| doc.get(u))
        && ed.kind_shown(i.kind)
    {
        let p = Vec3::from(i.position());
        let len = handle_len(p, cam_local);
        let active = match ed.drag.as_ref().map(|d| d.mode) {
            Some(DragMode::Axis { axis, .. }) => AXES.iter().position(|a| *a == axis).map(Handle::Axis),
            Some(DragMode::Yaw) => Some(Handle::Yaw),
            _ => ed.hover_handle,
        };
        let cols = [Color::srgb(1.0, 0.25, 0.25), Color::srgb(0.3, 1.0, 0.3), Color::srgb(0.35, 0.5, 1.0)];
        for (n, a) in AXES.iter().enumerate() {
            let c = if active == Some(Handle::Axis(n)) { Color::srgb(1.0, 1.0, 0.4) } else { cols[n] };
            hg.arrow(w(p), w(p + *a * len), c).with_tip_length(len * 0.2);
        }
        if has_facing(i.kind) {
            let c = if active == Some(Handle::Yaw) { Color::srgb(1.0, 1.0, 0.4) } else { Color::srgb(1.0, 0.6, 0.15) };
            let ring: Vec<Vec3> = (0..=48)
                .map(|k| {
                    let a = k as f32 / 48.0 * std::f32::consts::TAU;
                    w(p + Vec3::new(a.cos(), a.sin(), 0.0) * len * 0.75 + Vec3::Z * 0.05)
                })
                .collect();
            hg.linestrip(ring, c);
            hg.sphere(Isometry3d::from_translation(w(knob_pos(p, i.bearing(), len))), len * 0.07, c);
        }
    }
    match &ed.tool {
        Tool::Polygon(_, _, pts) => {
            let pts: Vec<Vec3> = pts.iter().map(|p| w(*p + Vec3::Z * 0.05)).collect();
            if pts.len() > 1 {
                g.linestrip(pts.iter().copied(), Color::srgb(0.3, 1.0, 0.3));
            }
        }
        Tool::Place(..) => {
            if let Some(p) = ed.cursor_world {
                let c = Color::srgb(0.3, 1.0, 0.3);
                g.circle(Isometry3d::new(w(p + Vec3::Z * 0.05), agt.rotation()), 0.4, c);
                g.line(w(p), w(p + Vec3::Z * 1.0), c);
            }
        }
        Tool::Select => {}
    }
}

fn marker_color(kind: Kind) -> Color {
    match kind {
        Kind::Waypoint => Color::srgb(0.3, 0.9, 1.0),
        Kind::Trigger => Color::srgb(1.0, 0.4, 0.2),
        Kind::Encounter => Color::srgb(1.0, 0.2, 0.6),
        Kind::Sound => Color::srgb(0.6, 0.5, 1.0),
        Kind::Store => Color::srgb(0.4, 1.0, 0.4),
        _ => Color::srgb(0.8, 0.8, 0.8),
    }
}

/// A blueprint's display name: its localized name through the TLK (or the
/// embedded text), else its Tag.
fn display_name(res: &Resources, talk: Option<&k2_data::Talk>, kind: Kind, resref: &str) -> String {
    use k2_data::GffExt;
    let Some(ty) = kind.blueprint() else { return String::new() };
    let Ok(g) = k2_data::read_gff(res, resref, ty) else { return String::new() };
    let s = &g.root;
    for l in ["FirstName", "LocName", "LocalizedName"] {
        let ls = s.locstring(l);
        let t = match talk {
            Some(talk) => talk.resolve(&ls),
            None => ls.substrings.first().map(|(_, t)| t.clone()).unwrap_or_default(),
        };
        if !t.trim().is_empty() {
            return t;
        }
    }
    s.string("Tag").unwrap_or_default()
}

// ---------------------------------------------------------------------------
// headless self test
// ---------------------------------------------------------------------------

/// `kotor2 --edit-selftest MODULE`: load the area document, save it
/// unchanged and compare, then apply scripted edits and save those as a mod
/// capsule. Each capsule is read back with it mounted as a mod, so the
/// game's own lookup path is checked. Prints the results; returns the
/// process exit code. Writes to K2_EDIT_SELFTEST_DIR (default
/// /tmp/k2research/edit_selftest).
pub fn selftest(install: Install, module: &str) -> i32 {
    let out = std::path::PathBuf::from(std::env::var("K2_EDIT_SELFTEST_DIR").unwrap_or_else(|_| "/tmp/k2research/edit_selftest".into()));
    let mut res = match Resources::open(install.clone()) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("open: {e}");
            return 1;
        }
    };
    let mut doc = match EditDoc::load(&mut res, module) {
        Ok(d) => d,
        Err(e) => {
            eprintln!("load: {e}");
            return 1;
        }
    };
    let orig = k2_data::read_gff(&res, &doc.area, rakata_core::ResourceType::Git).expect("git");
    let same_tree = doc.to_gff() == orig;
    let unchanged = out.join("unchanged.mod");
    let count = match doc.save(&unchanged) {
        Ok(n) => n,
        Err(e) => {
            eprintln!("save: {e}");
            return 1;
        }
    };
    let unchanged_res = mounted(&install, &unchanged).expect("open unchanged");
    let back = k2_data::read_gff(&unchanged_res, &doc.area, rakata_core::ResourceType::Git).expect("parse back");
    let reparsed_same = back == orig;
    let a = k2_data::module::Git::from_gff(&orig.root);
    let b = k2_data::module::Git::from_gff(&back.root);
    let counts = |g: &k2_data::module::Git| {
        (g.creatures.len(), g.doors.len(), g.placeables.len(), g.waypoints.len(), g.triggers.len(), g.sounds.len(), g.stores.len(), g.encounters.len(), g.cameras.len())
    };
    println!("unchanged: tree identical {same_tree}, written+reparsed identical {reparsed_same}, counts {:?} vs {:?}", counts(&a), counts(&b));
    println!("  wrote {} ({count} resources)", unchanged.display());
    let talk = k2_data::Talk::load(&install).ok();
    let modules = install.module_names();
    let named = modules.iter().filter(|m| doc::map_title(&install, talk.as_ref(), m).is_some()).count();
    println!("map name: {} is \"{}\"; {named} of {} modules have a real name", module, doc::map_name(&install, talk.as_ref(), module), modules.len());

    // Scripted edits: move the first placeable +2 m in X, delete the last
    // placeable, add a copy of the first placeable's blueprint 3 m from the
    // first creature, fork the first creature's conversation.
    let pl: Vec<u64> = doc.insts.iter().filter(|i| i.kind == Kind::Placeable).map(|i| i.uid).collect();
    let mut log = Vec::new();
    if let (Some(&first), Some(&last)) = (pl.first(), pl.last()) {
        let p = doc.get(first).unwrap().position();
        doc.checkpoint();
        doc.move_to(first, [p[0] + 2.0, p[1], p[2]]);
        log.push(format!("moved placeable {} by +2 x", doc.get(first).unwrap().template()));
        let t = doc.get(last).unwrap().template();
        doc.checkpoint();
        doc.delete(last);
        log.push(format!("deleted placeable {t}"));
        let src = doc.get(first).unwrap().template();
        let at = doc.insts.iter().find(|i| i.kind == Kind::Creature).map(|i| i.position()).unwrap_or(p);
        doc.checkpoint();
        match doc.add(&res, Kind::Placeable, &src, [at[0] + 3.0, at[1], at[2]], 1.0, &[]) {
            Ok(_) => log.push(format!("added placeable {src} at ({:.1}, {:.1}, {:.1})", at[0] + 3.0, at[1], at[2])),
            Err(e) => log.push(format!("add failed: {e}")),
        }
    }
    // A creature copy next to the moved placeable, easy to spot on screen.
    if let (Some(c), Some(&first)) = (doc.insts.iter().find(|i| i.kind == Kind::Creature).map(|i| i.template()), pl.first()) {
        let p = doc.get(first).unwrap().position();
        doc.checkpoint();
        match doc.add(&res, Kind::Creature, &c, [p[0] + 1.5, p[1] + 1.5, p[2]], 0.5, &[]) {
            Ok(_) => log.push(format!("added creature {c} at ({:.1}, {:.1}, {:.1})", p[0] + 1.5, p[1] + 1.5, p[2])),
            Err(e) => log.push(format!("add creature failed: {e}")),
        }
    }
    if let Some(c) = doc.insts.iter().find(|i| i.kind == Kind::Creature).map(|i| i.uid) {
        let spec = fields(Kind::Creature)[1];
        doc.checkpoint();
        match doc.set_field(&res, c, &spec, "edtest") {
            Ok(()) => log.push(format!("creature conversation -> edtest, template now {}", doc.get(c).unwrap().template())),
            Err(e) => log.push(format!("field edit failed: {e}")),
        }
    }
    // Undo/redo sanity: undo the last edit and redo it.
    let before = doc.to_gff();
    let undone = doc.undo() && doc.to_gff() != before;
    let redone = doc.redo() && doc.to_gff() == before;
    log.push(format!("undo changes state {undone}, redo restores {redone}"));
    let edited = out.join("edited.mod");
    let count = match doc.save(&edited) {
        Ok(n) => n,
        Err(e) => {
            eprintln!("save edited: {e}");
            return 1;
        }
    };
    for l in &log {
        println!("edit: {l}");
    }
    println!("  wrote {} ({count} resources)", edited.display());
    let edited_res = mounted(&install, &edited).expect("open edited");
    let edited_same = k2_data::read_gff(&edited_res, &doc.area, rakata_core::ResourceType::Git).is_ok_and(|g| g == doc.to_gff());
    let forks_same = doc.blueprints.iter().all(|(name, (kind, g))| {
        k2_data::read_gff(&edited_res, name, kind.blueprint().unwrap_or(rakata_core::ResourceType::Gff)).is_ok_and(|back| back == *g)
    });
    println!("  read back: git identical {edited_same}, {} forked blueprint(s) identical {forks_same}", doc.blueprints.len());
    if same_tree && reparsed_same && edited_same && forks_same { 0 } else { 2 }
}

/// Resources for the install with `capsule` mounted as the first mod.
fn mounted(install: &Install, capsule: &std::path::Path) -> Result<Resources, String> {
    let mut install = install.clone();
    install.mod_dirs.insert(0, capsule.to_owned());
    Resources::open(install).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests;

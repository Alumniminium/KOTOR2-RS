//! The editor's user interface: egui through bevy_egui.
//!
//! One system (`ui_system`, in `EguiPrimaryContextPass`) draws everything:
//! menu bar, status bar, object tree (left), inspector (right), the palette
//! window and the Load Map modal. Widgets never touch the document while
//! it is borrowed for drawing: they push `Act`s that are applied after the
//! frame, and read from a per-frame snapshot (`Snap`).

use bevy::prelude::*;
use bevy_egui::{EguiContexts, egui};
use egui::{Color32, Id, Key, Modifiers, RichText};

use super::doc::{FieldSpec, FieldTy, Kind, Src, fields};
use super::{EdViews, Editor, Tool, display_name, marker_color, request_load};
use crate::game::{NeedAreaLoad, WorldRes};

/// Something the UI asks for; applied once the frame is drawn.
enum Act {
    Select(Option<u64>),
    Frame(u64),
    /// Ends the open undo step (a drag or a typed edit started or stopped).
    Arm,
    Template(u64, String),
    Move(u64, [f32; 3]),
    Bearing(u64, f32),
    /// Inspector field; `live` edits (drag values, checkboxes) share an undo step.
    Field { uid: u64, row: usize, text: String, live: bool },
    Delete,
    Duplicate,
    Undo,
    Redo,
    Save,
    /// Asks first when there are unsaved edits.
    RevertAsk,
    Revert,
    CancelTool,
    Exit,
    TogglePicker,
    ClosePicker,
    Load(String),
    Pick(Kind, String),
    TogglePalette,
}

struct RowData {
    uid: u64,
    kind: Kind,
    template: String,
    tag: String,
}

struct SelData {
    uid: u64,
    kind: Kind,
    template: String,
    /// The source blueprint when this instance uses a fork from this session.
    forked_from: Option<String>,
    /// Blueprint resrefs of the instance's kind, for the template drop-down.
    templates: Vec<String>,
    pos: [f32; 3],
    bearing_deg: f32,
    fields: Vec<(usize, FieldSpec, String)>,
    polygon_points: usize,
}

/// What the panels show, read from the document once per frame.
struct Snap {
    rows: Vec<RowData>,
    sel: Option<SelData>,
    module: String,
    area: String,
    dirty: bool,
    can_undo: bool,
    can_redo: bool,
    count: usize,
}

fn col(c: Color) -> Color32 {
    let s = c.to_srgba();
    Color32::from_rgb((s.red * 255.0) as u8, (s.green * 255.0) as u8, (s.blue * 255.0) as u8)
}

fn snapshot(ed: &mut Editor) -> Snap {
    let Editor { doc, res, tags, sel, loading, catalog, .. } = ed;
    let Some(doc) = doc.as_ref() else {
        return Snap {
            rows: Vec::new(),
            sel: None,
            module: loading.clone().unwrap_or_else(|| "-".into()),
            area: String::new(),
            dirty: false,
            can_undo: false,
            can_redo: false,
            count: 0,
        };
    };
    let mut rows = Vec::with_capacity(doc.insts.len());
    for i in &doc.insts {
        let template = i.template();
        let tag = match i.git_tag() {
            Some(t) => t,
            None => match (fields(i.kind).first(), res.as_ref()) {
                (Some(spec), Some(res)) if spec.label == "Tag" => tags.entry((i.kind, template.clone())).or_insert_with(|| doc.field_text(res, i.uid, spec)).clone(),
                _ => String::new(),
            },
        };
        rows.push(RowData { uid: i.uid, kind: i.kind, template, tag });
    }
    let sel_data = (*sel).and_then(|u| doc.get(u)).and_then(|i| {
        let res = res.as_ref()?;
        let template = i.template();
        let forked_from = doc.blueprints.contains_key(&template).then(|| template.rsplit_once("_e").map_or(template.clone(), |(b, _)| b.to_owned()));
        let fields = fields(i.kind).iter().enumerate().map(|(n, spec)| (n, *spec, doc.field_text(res, i.uid, spec))).collect();
        let mut templates: Vec<String> = catalog.iter().filter(|(k, _, _)| *k == i.kind).map(|(_, r, _)| r.clone()).collect();
        templates.extend(doc.blueprints.iter().filter(|(_, (k, _))| *k == i.kind).map(|(r, _)| r.clone()));
        Some(SelData {
            uid: i.uid,
            kind: i.kind,
            template,
            forked_from,
            templates,
            pos: i.position(),
            bearing_deg: i.bearing().to_degrees(),
            fields,
            polygon_points: i.polygon().len(),
        })
    });
    Snap { rows, sel: sel_data, module: doc.module.clone(), area: doc.area.clone(), dirty: doc.dirty, can_undo: doc.can_undo(), can_redo: doc.can_redo(), count: doc.insts.len() }
}

#[allow(clippy::too_many_arguments)]
pub(super) fn ui_system(
    mut contexts: EguiContexts,
    mut ed: ResMut<Editor>,
    mut world: ResMut<WorldRes>,
    mut need: ResMut<NeedAreaLoad>,
    mut ev: ResMut<EdViews>,
    mut commands: Commands,
    mut was_picker: Local<bool>,
) {
    let Ok(ctx) = contexts.ctx_mut() else { return };
    let ctx = ctx.clone();
    let ed = &mut *ed;
    ctx.global_style_mut(|s| {
        s.visuals.panel_fill = Color32::from_rgba_unmultiplied(22, 24, 30, 238);
        s.visuals.window_fill = Color32::from_rgba_unmultiplied(26, 28, 36, 245);
    });
    let mut acts: Vec<Act> = Vec::new();
    shortcuts(&ctx, ed, &mut acts);

    let snap = snapshot(ed);
    let mut rects: Vec<egui::Rect> = Vec::new();
    let mut root = egui::Ui::new(ctx.clone(), "ed_root".into(), egui::UiBuilder::new().layer_id(egui::LayerId::background()).max_rect(ctx.viewport_rect()));
    rects.push(menu_bar(&mut root, ed, &snap, &mut acts));
    rects.push(status_bar(&mut root, ed, &snap));
    rects.push(left_panel(&mut root, ed, &snap, &mut acts));
    rects.push(right_panel(&mut root, ed, &snap, &mut acts));
    if let Some(r) = palette(&ctx, ed, &world, &mut acts) {
        rects.push(r);
    }
    let picker_now = ed.picker_open;
    picker(&ctx, ed, !*was_picker && picker_now, &mut acts);
    *was_picker = picker_now;
    if ed.revert_confirm {
        match revert_dialog(&ctx) {
            Some(true) => {
                ed.revert_confirm = false;
                acts.push(Act::Revert);
            }
            Some(false) => ed.revert_confirm = false,
            None => {}
        }
    }

    for a in acts.drain(..) {
        apply(a, ed, snap.dirty, &mut world, &mut need, &mut ev, &mut commands);
    }

    ed.ui_rects = rects.iter().map(|r| [r.min.x, r.min.y, r.max.x, r.max.y]).collect();
    ed.ui_ptr = ctx.egui_is_using_pointer() || egui::Popup::is_any_open(&ctx) || ed.picker_open || ed.revert_confirm;
    ed.ui_kbd = ctx.egui_wants_keyboard_input() || ed.picker_open;
}

fn apply(a: Act, ed: &mut Editor, dirty: bool, world: &mut WorldRes, need: &mut NeedAreaLoad, ev: &mut EdViews, commands: &mut Commands) {
    match a {
        Act::Select(s) => {
            ed.sel = s;
            ed.bump();
        }
        Act::Frame(uid) => {
            if let Some(i) = ed.doc.as_ref().and_then(|d| d.get(uid)) {
                ed.fly_to = Some(Vec3::from(i.position()));
            }
        }
        Act::Arm => ed.step_open = false,
        Act::Template(uid, t) => ed.set_template(uid, &t),
        Act::Move(uid, p) => {
            let Some(doc) = ed.doc.as_mut() else { return };
            if !ed.step_open {
                doc.checkpoint();
                ed.step_open = true;
            }
            doc.move_to(uid, p);
            ed.status = format!("move to ({:.2}, {:.2}, {:.2})", p[0], p[1], p[2]);
            ed.bump();
        }
        Act::Bearing(uid, deg) => {
            let Some(doc) = ed.doc.as_mut() else { return };
            let Some(cur) = doc.get(uid).map(|i| i.bearing()) else { return };
            if !ed.step_open {
                doc.checkpoint();
                ed.step_open = true;
            }
            doc.rotate(uid, super::wrap_pi(deg.to_radians() - cur));
            ed.status = format!("bearing {deg:.0} deg");
            ed.bump();
        }
        Act::Field { uid, row, text, live } => ed.commit_field(uid, row, &text, live),
        Act::Delete => ed.delete_sel(),
        Act::Duplicate => ed.duplicate_sel(),
        Act::Undo => ed.undo(),
        Act::Redo => ed.redo(),
        Act::Save => ed.save(),
        Act::RevertAsk => {
            if dirty {
                ed.revert_confirm = true;
            } else {
                ed.status = "nothing to revert".into();
            }
        }
        Act::CancelTool => {
            ed.tool = Tool::Select;
            ed.status = "select".into();
            ed.bump();
        }
        Act::Revert => {
            let module = ed.doc.as_ref().map(|d| d.module.clone());
            if let (Some(module), Some(mut res)) = (module, ed.res.take()) {
                match super::EditDoc::load(&mut res, &module) {
                    Ok(d) => {
                        ed.doc = Some(d);
                        ed.sel = None;
                        ed.status = format!("reverted {module} to the installed area");
                        for (_, (e, _)) in ev.map.drain() {
                            commands.entity(e).despawn();
                        }
                        ev.rev = 0;
                    }
                    Err(e) => ed.status = format!("revert {module}: {e}"),
                }
                ed.res = Some(res);
                ed.specs.clear();
                ed.tags.clear();
                ed.bump();
            }
        }
        Act::Exit => ed.f10_hook = true,
        Act::TogglePicker => {
            if ed.loading.is_none() {
                ed.picker_open = !ed.picker_open;
                ed.confirm = None;
                if ed.picker_open {
                    ed.pick_search.clear();
                    let cur = ed.doc.as_ref().map(|d| d.module.clone());
                    ed.pick_cursor = cur.and_then(|c| ed.modules.iter().position(|m| *m == c)).unwrap_or(0);
                }
            }
        }
        Act::ClosePicker => {
            ed.picker_open = false;
            ed.confirm = None;
        }
        Act::Load(m) => request_load(ed, world, need, &m),
        Act::Pick(k, r) => ed.pick_blueprint(k, &r),
        Act::TogglePalette => {
            ed.palette_open = !ed.palette_open;
            ed.palette_focus = ed.palette_open;
        }
    }
}

/// Global shortcuts. They are consumed here so egui's own focus handling
/// (Tab) and text editing (Ctrl+S) never see them.
fn shortcuts(ctx: &egui::Context, ed: &Editor, acts: &mut Vec<Act>) {
    ctx.input_mut(|i| {
        if i.consume_key(Modifiers::COMMAND, Key::S) {
            acts.push(Act::Save);
        }
        if i.consume_key(Modifiers::COMMAND, Key::O) {
            acts.push(Act::TogglePicker);
        }
        if !ed.picker_open && i.consume_key(Modifiers::NONE, Key::Tab) {
            acts.push(Act::TogglePalette);
        }
    });
}

// ---------------------------------------------------------------------------
// menu and status bars
// ---------------------------------------------------------------------------

/// A one-line selectable row (truncated, not wrapped).
fn row(ui: &mut egui::Ui, selected: bool, text: impl Into<String>) -> egui::Response {
    ui.add(egui::Button::selectable(selected, text.into()).truncate())
}

fn menu_item(ui: &mut egui::Ui, enabled: bool, text: &str, shortcut: &str) -> bool {
    let b = egui::Button::new(text).shortcut_text(shortcut);
    let r = ui.add_enabled(enabled, b).clicked();
    if r {
        ui.close();
    }
    r
}

fn menu_bar(root: &mut egui::Ui, ed: &mut Editor, snap: &Snap, acts: &mut Vec<Act>) -> egui::Rect {
    egui::Panel::top("ed_menu")
        .show(root, |ui| {
            egui::MenuBar::new().ui(ui, |ui| {
                ui.menu_button("File", |ui| {
                    if menu_item(ui, ed.loading.is_none(), "Load Map...", "Ctrl+O") {
                        acts.push(Act::TogglePicker);
                    }
                    if menu_item(ui, snap.count > 0 || snap.dirty, "Save mod", "Ctrl+S") {
                        acts.push(Act::Save);
                    }
                    if menu_item(ui, snap.dirty, "Revert", "") {
                        acts.push(Act::RevertAsk);
                    }
                    ui.separator();
                    if menu_item(ui, true, "Exit editor", "F10") {
                        acts.push(Act::Exit);
                    }
                });
                ui.menu_button("Edit", |ui| {
                    if menu_item(ui, snap.can_undo, "Undo", "Ctrl+Z") {
                        acts.push(Act::Undo);
                    }
                    if menu_item(ui, snap.can_redo, "Redo", "Ctrl+Y") {
                        acts.push(Act::Redo);
                    }
                    ui.separator();
                    if menu_item(ui, snap.sel.is_some(), "Delete", "Del") {
                        acts.push(Act::Delete);
                    }
                    if menu_item(ui, snap.sel.is_some(), "Duplicate", "Ctrl+D") {
                        acts.push(Act::Duplicate);
                    }
                });
                ui.menu_button("View", |ui| {
                    let mut changed = false;
                    for kind in Kind::ALL {
                        let label = RichText::new(format!("{}s", kind.name())).color(col(marker_color(kind)));
                        changed |= ui.checkbox(&mut ed.show_kind[kind as usize], label).changed();
                    }
                    ui.separator();
                    ui.checkbox(&mut ed.show_markers, "Markers (outlines)");
                    ui.checkbox(&mut ed.show_handles, "Move/rotate handles");
                    ui.checkbox(&mut ed.show_grid, "Grid");
                    ui.checkbox(&mut ed.show_walk, "Walkmesh overlay");
                    ui.separator();
                    if menu_item(ui, true, if ed.palette_open { "Hide palette" } else { "Show palette" }, "Tab") {
                        acts.push(Act::TogglePalette);
                    }
                    if changed {
                        ed.bump();
                    }
                });
            });
        })
        .response
        .rect
}

fn status_bar(root: &mut egui::Ui, ed: &Editor, snap: &Snap) -> egui::Rect {
    egui::Panel::bottom("ed_status")
        .show(root, |ui| {
            ui.horizontal(|ui| {
                ui.label(RichText::new(format!("module {}", snap.module)).strong());
                ui.separator();
                ui.label(format!("mod {}", ed.mod_name));
                if snap.dirty {
                    ui.label(RichText::new("* unsaved").color(Color32::from_rgb(255, 200, 80)));
                }
                ui.separator();
                let tool = match &ed.tool {
                    Tool::Select => "select".to_string(),
                    Tool::Place(k, r) => format!("place {} {r}", k.name()),
                    Tool::Polygon(k, r, p) => format!("polygon {} {r} ({})", k.name(), p.len()),
                };
                ui.label(format!("tool {tool}"));
                ui.separator();
                match &snap.sel {
                    Some(s) => ui.label(format!("selected {} #{} {}", s.kind.name(), s.uid, s.template)),
                    None => ui.label("selected -"),
                };
                ui.separator();
                match ed.cursor_world {
                    Some(p) => ui.label(format!("cursor {:.1} {:.1} {:.1}", p.x, p.y, p.z)),
                    None => ui.label("cursor -"),
                };
                ui.separator();
                ui.add(egui::Label::new(RichText::new(&ed.status).color(Color32::from_rgb(190, 235, 190))).truncate());
            });
        })
        .response
        .rect
}

// ---------------------------------------------------------------------------
// object tree
// ---------------------------------------------------------------------------

fn left_panel(root: &mut egui::Ui, ed: &mut Editor, snap: &Snap, acts: &mut Vec<Act>) -> egui::Rect {
    egui::Panel::left("ed_left")
        .default_size(250.0)
        .size_range(170.0..=460.0)
        .resizable(true)
        .show(root, |ui| {
            ui.horizontal(|ui| {
                ui.heading("Objects");
                ui.weak(format!("{}", snap.count));
            });
            ui.add(egui::TextEdit::singleline(&mut ed.obj_search).hint_text("filter: template, tag, kind").desired_width(f32::INFINITY));
            ui.separator();
            let q = ed.obj_search.to_ascii_lowercase();
            let searching = !q.is_empty();
            // Scroll to and open the selection's row when it changed from
            // outside the list (a click in the view).
            let last_id = Id::new("ed_last_sel");
            let last: Option<u64> = ui.data(|d| d.get_temp::<Option<u64>>(last_id)).flatten();
            let pend_id = Id::new("ed_scroll_pending");
            if last != ed.sel {
                ui.data_mut(|d| {
                    d.insert_temp(last_id, ed.sel);
                    d.insert_temp(pend_id, ed.sel.is_some());
                });
            }
            // Stays set until the row has been laid out (its header may need
            // a frame to open) and scrolled to.
            let sel_changed = ui.data(|d| d.get_temp::<bool>(pend_id)).unwrap_or(false);
            let mut scrolled = false;
            egui::ScrollArea::vertical().auto_shrink(false).show(ui, |ui| {
                for kind in Kind::ALL {
                    let rows: Vec<&RowData> = snap
                        .rows
                        .iter()
                        .filter(|r| r.kind == kind)
                        .filter(|r| !searching || r.template.contains(&q) || r.tag.to_ascii_lowercase().contains(&q) || kind.name().starts_with(&q))
                        .collect();
                    let sel_here = ed.sel.is_some_and(|u| rows.iter().any(|r| r.uid == u));
                    let title = RichText::new(format!("{}s ({})", kind.name(), rows.len())).color(col(marker_color(kind)));
                    let mut h = egui::CollapsingHeader::new(title).id_salt(("ed_kind", kind.name())).default_open(rows.len() <= 30 && !rows.is_empty());
                    if searching && !rows.is_empty() || sel_changed && sel_here {
                        h = h.open(Some(true));
                    }
                    h.show(ui, |ui| {
                        ui.with_layout(egui::Layout::top_down_justified(egui::Align::LEFT), |ui| {
                        for r in rows {
                            let selected = ed.sel == Some(r.uid);
                            let text = if r.tag.is_empty() || r.tag.eq_ignore_ascii_case(&r.template) { r.template.clone() } else { format!("{}  [{}]", r.template, r.tag) };
                            let resp = row(ui, selected, text);
                            if selected && sel_changed {
                                resp.scroll_to_me(Some(egui::Align::Center));
                                scrolled = true;
                            }
                            if resp.clicked() {
                                acts.push(Act::Select(Some(r.uid)));
                            }
                            if resp.double_clicked() {
                                acts.push(Act::Frame(r.uid));
                            }
                        }
                        });
                    });
                }
            });
            if scrolled {
                ui.data_mut(|d| d.insert_temp(pend_id, false));
            }
        })
        .response
        .rect
}

// ---------------------------------------------------------------------------
// inspector
// ---------------------------------------------------------------------------

/// A text field that keeps its edit buffer while focused. Returns the new
/// text on commit (Enter, or focus lost); Escape discards.
fn text_field(ui: &mut egui::Ui, id: Id, text: &str, width: f32) -> Option<String> {
    let buf_id = id.with("buf");
    let focused = ui.memory(|m| m.has_focus(id));
    let mut buf: String = if focused { ui.data(|d| d.get_temp::<String>(buf_id)).unwrap_or_else(|| text.to_owned()) } else { text.to_owned() };
    let r = ui.add(egui::TextEdit::singleline(&mut buf).id(id).desired_width(width));
    if r.changed() {
        ui.data_mut(|d| d.insert_temp(buf_id, buf.clone()));
    }
    let mut out = None;
    if r.lost_focus() {
        if !ui.input(|i| i.key_pressed(Key::Escape)) && buf != text {
            out = Some(buf);
        }
        ui.data_mut(|d| d.remove_temp::<String>(buf_id));
    }
    out
}

/// A resref field: free text plus a drop-down with a filter over `options`
/// (scripts, conversations, modules).
fn resref_field(ui: &mut egui::Ui, id: Id, text: &str, options: &[String]) -> Option<String> {
    let mut out = None;
    ui.horizontal(|ui| {
        let w = (ui.available_width() - 28.0).max(60.0);
        out = text_field(ui, id, text, w);
        let filter_id = id.with("flt");
        ui.menu_button("v", |ui| {
            let mut flt: String = ui.data(|d| d.get_temp(filter_id)).unwrap_or_default();
            let r = ui.add(egui::TextEdit::singleline(&mut flt).hint_text("filter").desired_width(220.0));
            if r.changed() {
                ui.data_mut(|d| d.insert_temp(filter_id, flt.clone()));
            }
            let q = flt.to_ascii_lowercase();
            let shown: Vec<&String> = options.iter().filter(|o| q.is_empty() || o.contains(&q)).collect();
            ui.weak(format!("{} of {}", shown.len(), options.len()));
            let rh = ui.text_style_height(&egui::TextStyle::Body);
            egui::ScrollArea::vertical().max_height(260.0).show_rows(ui, rh, shown.len() + 1, |ui, range| {
                ui.with_layout(egui::Layout::top_down_justified(egui::Align::LEFT), |ui| {
                for n in range {
                    let name = if n == 0 { "" } else { shown[n - 1].as_str() };
                    let label = if n == 0 { "(none)" } else { name };
                    if row(ui, name == text, label).clicked() {
                        out = Some(name.to_owned());
                        ui.data_mut(|d| d.remove_temp::<String>(filter_id));
                        ui.close();
                    }
                }
                });
            });
        });
    });
    out
}

fn right_panel(root: &mut egui::Ui, ed: &mut Editor, snap: &Snap, acts: &mut Vec<Act>) -> egui::Rect {
    egui::Panel::right("ed_right")
        .default_size(300.0)
        .size_range(220.0..=520.0)
        .resizable(true)
        .show(root, |ui| {
            ui.heading("Inspector");
            ui.weak(format!("area {} ({} objects){}", snap.area, snap.count, if snap.dirty { " *" } else { "" }));
            ui.separator();
            egui::ScrollArea::vertical().auto_shrink(false).show(ui, |ui| match &snap.sel {
                None => {
                    ui.label("Click an object in the view or in the list.");
                    ui.add_space(6.0);
                    ui.weak("Tab: palette (add objects)");
                    ui.weak("Ctrl+O: load map   Ctrl+S: save mod");
                    ui.weak("Drag: move   Q/E or R+wheel: rotate");
                    ui.weak("Del: delete   Ctrl+D: duplicate");
                    ui.weak("F10: leave the editor");
                }
                Some(s) => inspector(ui, ed, s, acts),
            });
        })
        .response
        .rect
}

fn inspector(ui: &mut egui::Ui, ed: &Editor, s: &SelData, acts: &mut Vec<Act>) {
    ui.horizontal(|ui| {
        ui.label(RichText::new(format!("{} #{}", s.kind.name(), s.uid)).heading().color(col(marker_color(s.kind))));
    });
    ui.horizontal(|ui| {
        ui.label("template");
        if let Some(t) = resref_field(ui, Id::new(("ed_template", s.uid)), &s.template, &s.templates) {
            acts.push(Act::Template(s.uid, t));
        }
    });
    match &s.forked_from {
        Some(src) => ui.colored_label(Color32::from_rgb(255, 170, 70), format!("forked blueprint (from {src}), only this object uses it")),
        None => ui.weak("shared blueprint: editing a [bp] field forks it for this object"),
    };
    ui.horizontal(|ui| {
        if ui.button("Duplicate").clicked() {
            acts.push(Act::Duplicate);
        }
        if ui.button("Delete").clicked() {
            acts.push(Act::Delete);
        }
        if ui.button("Frame").clicked() {
            acts.push(Act::Frame(s.uid));
        }
    });
    ui.separator();

    // Transform: drag values (drag, or click and type).
    ui.label(RichText::new("Transform").strong());
    let mut pos = s.pos;
    let mut moved = false;
    ui.horizontal(|ui| {
        for (n, name) in ["x", "y", "z"].iter().enumerate() {
            let r = ui.add(egui::DragValue::new(&mut pos[n]).speed(0.05).fixed_decimals(2).prefix(format!("{name} ")));
            if r.drag_started() || r.gained_focus() || r.drag_stopped() || r.lost_focus() {
                acts.push(Act::Arm);
            }
            moved |= r.changed();
        }
    });
    if moved {
        acts.push(Act::Move(s.uid, pos));
    }
    if super::has_facing(s.kind) || matches!(s.kind, Kind::Trigger | Kind::Encounter | Kind::Sound | Kind::Store) {
        let mut deg = super::wrap_pi(s.bearing_deg.to_radians()).to_degrees();
        let r = ui.add(egui::DragValue::new(&mut deg).speed(1.0).fixed_decimals(0).suffix(" deg").prefix("bearing "));
        if r.drag_started() || r.gained_focus() || r.drag_stopped() || r.lost_focus() {
            acts.push(Act::Arm);
        }
        if r.changed() {
            acts.push(Act::Bearing(s.uid, deg));
        }
    }
    if s.polygon_points > 0 {
        ui.weak(format!("geometry: {} points", s.polygon_points));
    }
    ui.separator();

    if s.fields.is_empty() {
        return;
    }
    ui.label(RichText::new("Fields").strong());
    egui::Grid::new(("ed_fields", s.uid)).num_columns(2).spacing([8.0, 4.0]).striped(true).show(ui, |ui| {
        for (row, spec, text) in &s.fields {
            let tag = match spec.src {
                Src::Git => ("git", Color32::from_rgb(120, 190, 255)),
                Src::Blueprint => ("bp", Color32::from_rgb(255, 170, 70)),
            };
            ui.horizontal(|ui| {
                ui.label(spec.label);
                ui.label(RichText::new(tag.0).small().color(tag.1));
            });
            let id = Id::new(("ed_field", s.uid, *row));
            let (uid, row) = (s.uid, *row);
            match spec.ty {
                FieldTy::Byte => {
                    let mut on = !(text.is_empty() || text == "0");
                    if ui.checkbox(&mut on, "").changed() {
                        acts.push(Act::Field { uid, row, text: if on { "1" } else { "0" }.into(), live: true });
                    }
                }
                FieldTy::Word => {
                    let mut v: u16 = text.parse().unwrap_or(0);
                    let r = ui.add(egui::DragValue::new(&mut v).speed(0.2));
                    if r.drag_started() || r.gained_focus() || r.drag_stopped() || r.lost_focus() {
                        acts.push(Act::Arm);
                    }
                    if r.changed() {
                        acts.push(Act::Field { uid, row, text: v.to_string(), live: true });
                    }
                }
                FieldTy::ResRef => {
                    let options: Option<&[String]> = if spec.label == "Conversation" {
                        Some(&ed.convs)
                    } else if spec.label == "LinkedToModule" {
                        Some(&ed.modules)
                    } else if spec.label.starts_with("Script") || spec.label.starts_with("On") {
                        Some(&ed.scripts)
                    } else {
                        None
                    };
                    let out = match options {
                        Some(o) => resref_field(ui, id, text, o),
                        None => text_field(ui, id, text, ui.available_width().max(60.0)),
                    };
                    if let Some(text) = out {
                        acts.push(Act::Field { uid, row, text, live: false });
                    }
                }
                FieldTy::Str => {
                    if let Some(text) = text_field(ui, id, text, ui.available_width().max(60.0)) {
                        acts.push(Act::Field { uid, row, text, live: false });
                    }
                }
            }
            ui.end_row();
        }
    });
}

// ---------------------------------------------------------------------------
// palette
// ---------------------------------------------------------------------------

fn palette(ctx: &egui::Context, ed: &mut Editor, world: &WorldRes, acts: &mut Vec<Act>) -> Option<egui::Rect> {
    if !ed.palette_open || ed.doc.is_none() {
        return None;
    }
    let idx = ed.filtered();
    let Editor { catalog, res, palette_search, palette_kind, palette_open, palette_focus, tool, .. } = &mut *ed;
    let mut open = *palette_open;
    let talk = world.0.talk.as_ref();
    let mut pick: Option<(Kind, String)> = None;
    let w = egui::Window::new("Palette").open(&mut open).default_pos([270.0, 60.0]).default_size([340.0, 420.0]).resizable(true).show(ctx, |ui| {
        ui.horizontal_wrapped(|ui| {
            if ui.selectable_label(palette_kind.is_none(), "all").clicked() {
                *palette_kind = None;
            }
            for kind in Kind::ALL {
                if kind.blueprint().is_none() {
                    continue;
                }
                let t = RichText::new(kind.name()).color(col(marker_color(kind)));
                if ui.selectable_label(*palette_kind == Some(kind), t).clicked() {
                    *palette_kind = Some(kind);
                }
            }
        });
        let search_id = Id::new("ed_palette_search");
        let r = ui.add(egui::TextEdit::singleline(palette_search).id(search_id).hint_text("search templates and names").desired_width(f32::INFINITY));
        if std::mem::take(palette_focus) {
            r.request_focus();
        }
        if r.lost_focus()
            && ui.input(|i| i.key_pressed(Key::Enter))
            && let Some(&i) = idx.first()
        {
            pick = Some((catalog[i].0, catalog[i].1.clone()));
        }
        ui.horizontal(|ui| {
            ui.weak(format!("{} templates", idx.len()));
            match &*tool {
                Tool::Place(k, r) | Tool::Polygon(k, r, _) => {
                    ui.label(RichText::new(format!("placing {} {r}", k.name())).color(Color32::from_rgb(120, 230, 120)));
                    if ui.small_button("cancel").clicked() {
                        acts.push(Act::CancelTool);
                    }
                }
                Tool::Select => {
                    ui.weak("click one, then click the walkmesh");
                }
            }
        });
        ui.separator();
        let rh = ui.text_style_height(&egui::TextStyle::Body);
        egui::ScrollArea::vertical().auto_shrink(false).show_rows(ui, rh, idx.len(), |ui, range| {
            ui.with_layout(egui::Layout::top_down_justified(egui::Align::LEFT), |ui| {
            for n in range {
                let i = idx[n];
                if catalog[i].2.is_empty() {
                    let name = res.as_ref().map(|res| display_name(res, talk, catalog[i].0, &catalog[i].1)).unwrap_or_default();
                    catalog[i].2 = if name.is_empty() { "-".into() } else { name };
                }
                let (kind, r, name) = &catalog[i];
                let active = matches!(&*tool, Tool::Place(k, t) | Tool::Polygon(k, t, _) if k == kind && t == r);
                let text = if palette_kind.is_some() { format!("{r}   {name}") } else { format!("{:<9} {r}   {name}", kind.name()) };
                if row(ui, active, text).clicked() {
                    pick = Some((*kind, r.clone()));
                }
            }
            });
        });
    });
    *palette_open = open;
    if let Some((k, r)) = pick {
        acts.push(Act::Pick(k, r));
    }
    w.map(|w| w.response.rect)
}

// ---------------------------------------------------------------------------
// load map, revert
// ---------------------------------------------------------------------------

/// The Load Map modal: typing filters, Up/Down/PageUp/PageDown move, Enter or
/// a click opens, Esc closes (or cancels a pending discard).
fn picker(ctx: &egui::Context, ed: &mut Editor, just_opened: bool, acts: &mut Vec<Act>) {
    if !ed.picker_open {
        return;
    }
    let all = ed.pick_filtered();
    let n = all.len();
    const PAGE: usize = 12;
    let (mut go, mut moved, mut esc) = (false, just_opened, false);
    ctx.input_mut(|i| {
        if i.consume_key(Modifiers::NONE, Key::Enter) {
            go = true;
        }
        if i.consume_key(Modifiers::NONE, Key::Escape) {
            esc = true;
        }
        if i.consume_key(Modifiers::NONE, Key::ArrowDown) {
            ed.pick_cursor = (ed.pick_cursor + 1).min(n.saturating_sub(1));
            moved = true;
        }
        if i.consume_key(Modifiers::NONE, Key::ArrowUp) {
            ed.pick_cursor = ed.pick_cursor.saturating_sub(1);
            moved = true;
        }
        if i.consume_key(Modifiers::NONE, Key::PageDown) {
            ed.pick_cursor = (ed.pick_cursor + PAGE).min(n.saturating_sub(1));
            moved = true;
        }
        if i.consume_key(Modifiers::NONE, Key::PageUp) {
            ed.pick_cursor = ed.pick_cursor.saturating_sub(PAGE);
            moved = true;
        }
    });
    if esc {
        if ed.confirm.take().is_some() {
            ed.status = "load cancelled".into();
        } else {
            acts.push(Act::ClosePicker);
            return;
        }
    }
    if moved {
        ed.confirm = None;
    }
    let cur_module = ed.doc.as_ref().map(|d| d.module.clone());
    let mut clicked: Option<usize> = None;
    let mut discard: Option<String> = None;
    let mut save_first = false;
    let mut cancel = false;
    let resp = egui::Modal::new(Id::new("ed_load_map")).show(ctx, |ui| {
        ui.set_width(380.0);
        ui.heading("Load Map");
        let r = ui.add(egui::TextEdit::singleline(&mut ed.pick_search).hint_text("type to filter modules").desired_width(f32::INFINITY));
        r.request_focus();
        if r.changed() {
            ed.pick_cursor = 0;
            ed.confirm = None;
        }
        ui.weak(format!("{} of {} modules - Enter or click opens, Esc closes", n, ed.modules.len()));
        if let Some(m) = &ed.confirm {
            ui.colored_label(Color32::from_rgb(255, 200, 80), format!("Unsaved edits. Loading {m} discards them."));
            ui.horizontal(|ui| {
                if ui.button("Discard and load").clicked() {
                    discard = Some(m.clone());
                }
                if ui.button("Save and load").clicked() {
                    save_first = true;
                    discard = Some(m.clone());
                }
                if ui.button("Cancel").clicked() {
                    cancel = true;
                }
            });
        }
        ui.separator();
        let rh = ui.text_style_height(&egui::TextStyle::Body);
        let mut area = egui::ScrollArea::vertical().max_height(340.0).auto_shrink([false, true]);
        if moved {
            area = area.vertical_scroll_offset((ed.pick_cursor as f32 - 6.0).max(0.0) * (rh + ui.spacing().item_spacing.y));
        }
        area.show_rows(ui, rh, n, |ui, range| {
            ui.with_layout(egui::Layout::top_down_justified(egui::Align::LEFT), |ui| {
            for k in range {
                let i = all[k];
                let m = &ed.modules[i];
                let name = &ed.map_names[i];
                let cur = cur_module.as_deref() == Some(m.as_str());
                let label = if name == m { m.clone() } else { format!("{name}   ({m})") };
                let text = if cur { format!("{label}   (open)") } else { label };
                if row(ui, k == ed.pick_cursor, text).clicked() {
                    clicked = Some(k);
                }
            }
            });
        });
    });
    if cancel {
        ed.confirm = None;
        ed.status = "load cancelled".into();
    }
    if save_first {
        acts.push(Act::Save);
    }
    if let Some(m) = discard {
        acts.push(Act::Load(m));
        return;
    }
    if let Some(k) = clicked {
        ed.pick_cursor = k;
        go = true;
    }
    if go && let Some(&i) = all.get(ed.pick_cursor) {
        acts.push(Act::Load(ed.modules[i].clone()));
    }
    if resp.should_close() && ed.confirm.is_none() {
        acts.push(Act::ClosePicker);
    }
}

/// Some(true) discard, Some(false) cancel, None still open.
fn revert_dialog(ctx: &egui::Context) -> Option<bool> {
    let mut out = None;
    let r = egui::Modal::new(Id::new("ed_revert")).show(ctx, |ui| {
        ui.heading("Revert");
        ui.label("Discard all unsaved edits and reload the installed area?");
        ui.horizontal(|ui| {
            if ui.button("Discard edits").clicked() {
                out = Some(true);
            }
            if ui.button("Cancel").clicked() {
                out = Some(false);
            }
        });
    });
    if out.is_none() && r.should_close() {
        out = Some(false);
    }
    out
}

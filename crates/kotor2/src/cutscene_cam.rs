//! Conversation cutscene direction: DLG camera angles, animated cameras,
//! participant animations and lip sync.
//!
//! CameraAngle on an entry node (counts over all shipped DLGs):
//! - 0 auto / 2 shoulder: the over-the-shoulder shot in `dialog_ui.rs`.
//! - 1 face: close-up of the speaker.
//! - 3 wide: both participants from the side.
//! - 4 animated: the DLG CameraModel (e.g. 101percam) holds a `camerahook`
//!   node animated by clips CUT001W, CUT002W, ...; CameraAnimation 1200+n
//!   plays CUT{n+1:03}W from its start. Other values (1000, 1001, 1401)
//!   keep the clip that is running.
//! - 5 no change: keep the previous shot.
//! - 6 static: GIT CameraList entry with CameraID.
//!
//! Camera space (static and animated): the camera looks down its local -Z
//! with +Y up (the GL convention, same as Bevy), placed in game space. A
//! static camera is `T(Position + Height * Z) * R(Orientation) *
//! Rx(Pitch deg)`, so Pitch 90 is level. Camera models sit at the area
//! origin. FieldOfView is vertical, in degrees.
//!
//! Node AnimList entries name a participant by tag and an Animation id;
//! ids 10000+n are rows of dialoganimations.2da, whose names match the
//! animations.2da description column (which gives the clip name). Ids
//! 1000-1499 are cutscene clips cut{N}/cut{N}L (see `legacy_cut_clip`).
//!
//! `--autodialog DLG[,OWNER_TAG]` starts a conversation 1 s after an area
//! loads, and `--force-angle N` overrides every node's CameraAngle (tests).
//! `K2_DLG_SHOT=PATH,ENTRY:SECS[,ENTRY:SECS...]` (with K2_TEST_WINDOW=1)
//! saves a screenshot SECS after the conversation reaches entry ENTRY (`end`
//! = after the conversation ended; `{}` in PATH becomes ENTRY_SECS, use fresh
//! paths) and exits after the last one.

use std::collections::HashMap;
use std::f32::consts::FRAC_PI_2;
use std::sync::Arc;

use bevy::prelude::*;
use bevy::render::view::screenshot::{Screenshot, save_to_disk};
use bevy::transform::TransformSystems;
use k2_res::ResKey;
use k2_script::ObjectId;
use rakata_core::ResourceType;

use crate::anim::clip::{Clip, convert_all};
use crate::anim::lipsync::{LipIndex, LipSync, synthetic_lip};
use crate::area::{AreaModule, GameData};
use crate::audio::AlienLine;
use crate::coords;
use crate::game::{CreatureAnim, GameSet, ObjBody, ObjEntities, WorldRes};
use crate::messages::AreaLoaded;
use crate::model::GameRes;

/// Flags owned by this module, each taking one value.
pub const FLAGS: [&str; 2] = ["--autodialog", "--force-angle"];

pub struct CutsceneCamPlugin;

impl Plugin for CutsceneCamPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<CutCam>()
            .init_resource::<DialogAnims>()
            .init_resource::<AutoDialog>()
            // on_line reads the alien clip length that the line's voice start records.
            .add_systems(Update, ((autodialog, on_line.after(crate::dialog_ui::LineShown)).chain().in_set(GameSet::Ui), report_voice, dialog_shots))
            .add_systems(PostUpdate, drive_camera.before(TransformSystems::Propagate));
    }
}

#[derive(Clone)]
enum Shot {
    /// dialog_ui's over-the-shoulder camera.
    Default,
    Face,
    Wide,
    Static { pos: Vec3, rot: Quat, fov: f32 },
    Animated,
    /// Hold this Bevy-space transform.
    Frozen(Transform),
}

/// A loaded camera model: camerahook rest pos and rot, anim scale, clips.
type CamModel = (Vec3, Quat, f32, HashMap<String, Arc<Clip>>);

/// The line being reported on: (dialog, entry), start time, logged.
type ReportedLine = Option<((String, usize), f64, bool)>;

struct CamAnim {
    clip: Arc<Clip>,
    rest_t: Vec3,
    rest_r: Quat,
    scale: f32,
    time: f32,
}

#[derive(Resource)]
struct CutCam {
    key: Option<(String, usize)>,
    shot: Shot,
    anim: Option<CamAnim>,
    /// Camera models: (hook rest pos, rest rot, anim scale, clips).
    models: HashMap<String, Option<CamModel>>,
    default_fov: Option<f32>,
    last: Option<Transform>,
}

impl Default for CutCam {
    fn default() -> Self {
        Self { key: None, shot: Shot::Default, anim: None, models: HashMap::new(), default_fov: None, last: None }
    }
}

/// Participant animations set by the current line, to undo on the next.
#[derive(Resource, Default)]
struct DialogAnims(Vec<(ObjectId, String)>);

#[derive(Resource, Default)]
struct AutoDialog {
    since: f32,
    done: bool,
}

fn arg_value(name: &str) -> Option<String> {
    let argv: Vec<String> = std::env::args().collect();
    argv.iter().position(|a| a == name).and_then(|i| argv.get(i + 1)).cloned()
}

fn autodialog(time: Res<Time<Real>>, mut auto: ResMut<AutoDialog>, mut loaded: MessageReader<AreaLoaded>, mut world: ResMut<WorldRes>) {
    let Some(spec) = arg_value("--autodialog") else { return };
    if loaded.read().count() > 0 {
        auto.since = 0.0;
        auto.done = false;
    }
    if auto.done {
        return;
    }
    auto.since += time.delta_secs();
    let w = &mut world.0;
    if auto.since < 1.0 || w.conversation.is_some() {
        return;
    }
    auto.done = true;
    let (dlg, owner_tag) = match spec.split_once(',') {
        Some((d, o)) => (d.to_ascii_lowercase(), Some(o.to_owned())),
        None => (spec.to_ascii_lowercase(), None),
    };
    let leader = w.party_leader();
    let owner = owner_tag.and_then(|t| w.by_tag(&t, 0)).unwrap_or(leader);
    let ok = w.start_conversation(owner, leader, &dlg);
    info!("autodialog: {dlg} owner {} started={ok}", w.describe(owner));
}

/// One `K2_DLG_SHOT` request: entry (None = after the end), seconds, path.
type DlgShot = (Option<usize>, f32, std::path::PathBuf);

#[derive(Default)]
struct DlgShots {
    shots: Option<Vec<DlgShot>>,
    entry: Option<usize>,
    since: f64,
    ended: Option<f64>,
    taken: Vec<std::path::PathBuf>,
}

fn dialog_shots(
    mut commands: Commands,
    time: Res<Time<Real>>,
    world: Res<WorldRes>,
    mut exit: MessageWriter<AppExit>,
    mut st: Local<DlgShots>,
) {
    let Ok(spec) = std::env::var("K2_DLG_SHOT") else { return };
    let now = time.elapsed_secs_f64();
    if st.shots.is_none() {
        let mut parts = spec.split(',');
        let path = parts.next().unwrap_or_default().to_owned();
        let shots = parts
            .filter_map(|p| {
                let (e, s) = p.split_once(':')?;
                let secs: f32 = s.parse().ok()?;
                let entry = if e == "end" { None } else { Some(e.parse().ok()?) };
                Some((entry, secs, std::path::PathBuf::from(path.replace("{}", &format!("{e}_{s}")))))
            })
            .collect();
        st.shots = Some(shots);
    }
    let entry = world.0.conversation.as_ref().and_then(|c| c.entry);
    if world.0.conversation.is_some() {
        if entry != st.entry {
            st.entry = entry;
            st.since = now;
        }
    } else if st.entry.is_some() && st.ended.is_none() {
        st.ended = Some(now);
    }
    let (cur, since, ended) = (st.entry, st.since, st.ended);
    let mut fire = Vec::new();
    for (e, secs, path) in st.shots.as_ref().into_iter().flatten() {
        let due = match e {
            Some(e) => world.0.conversation.is_some() && cur == Some(*e) && now - since >= f64::from(*secs),
            None => ended.is_some_and(|t| now - t >= f64::from(*secs)),
        };
        if due && !st.taken.contains(path) {
            fire.push(path.clone());
        }
    }
    for path in fire {
        info!("dlg shot: {}", path.display());
        commands.spawn(Screenshot::primary_window()).observe(save_to_disk(path.clone()));
        st.taken.push(path);
    }
    let all = st.shots.as_ref().is_some_and(|s| s.len() == st.taken.len());
    if all && st.taken.iter().all(|p| p.exists()) {
        exit.write(AppExit::Success);
    }
}

fn cam_model(res: &GameRes, name: &str) -> Option<CamModel> {
    let b = res.0.get(&ResKey::new(name, ResourceType::Mdl)).ok()??;
    let x = res.0.get(&ResKey::new(name, ResourceType::Mdx)).ok().flatten();
    let mdl = k2_mdl::read_mdl_from_bytes(&b, x.as_deref()).ok()?;
    fn find(n: &k2_mdl::MdlNode) -> Option<&k2_mdl::MdlNode> {
        if n.name.eq_ignore_ascii_case("camerahook") {
            return Some(n);
        }
        n.children.iter().find_map(find)
    }
    let hook = find(&mdl.root_node)?;
    // MdlNode rotation is scalar-first [w, x, y, z].
    let [w, x, y, z] = hook.rotation;
    let rest_r = Quat::from_xyzw(x, y, z, w).normalize();
    let scale = if mdl.animation_scale > 0.0 { mdl.animation_scale } else { 1.0 };
    Some((Vec3::from(hook.position), rest_r, scale, convert_all(&mdl.animations)))
}

/// Legacy cutscene ids (no 2da row): 1000+N is the one-shot clip cut{N+1}
/// and 1400+N the looping clip cut{N+1}L of the creature's supermodel chain
/// (101awake: 1409 floating in the tank = cut010L, 1410 twitching = cut011L,
/// 1011 sinking to the knees = cut012, 1412 face down on the floor =
/// cut013L, 1013 getting up = cut014; Sion 1027/1428/1029 = cut028/029L/030).
fn legacy_cut_clip(id: u32) -> Option<(String, bool)> {
    match id {
        1000..=1199 => Some((format!("cut{:03}", id - 999), false)),
        1300..=1399 => Some((format!("cut{:03}", id - 999), false)),
        1400..=1499 => Some((format!("cut{:03}l", id - 1399), true)),
        // A stunt model's own cutscene clip, like the camera model's:
        // 1200+n is CUT{n+1}W (101kreia: 1201 = Kreia getting up, CUT002W).
        1200..=1299 => Some((format!("cut{:03}w", id - 1199), false)),
        _ => None,
    }
}

/// dialoganimations.2da row name -> clip candidates.
fn dialog_anim_clips(gd: &GameData, res: &GameRes, id: u32) -> (Vec<String>, bool) {
    if id < 10000 {
        return match legacy_cut_clip(id) {
            Some((clip, looping)) => (vec![clip], looping),
            None => (Vec::new(), false),
        };
    }
    let row = (id - 10000) as usize;
    let Ok(da) = gd.tables.get(&res.0, "dialoganimations") else { return (Vec::new(), false) };
    let Some(name) = da.str(row, "name").filter(|s| !s.is_empty()).map(str::to_owned) else { return (Vec::new(), false) };
    let looping = da.int(row, "looping") == Some(1);
    let mut out = Vec::new();
    if let Ok(anims) = gd.tables.get(&res.0, "animations") {
        for r in 0..anims.rows() {
            if anims.str(r, "description").is_some_and(|d| d.eq_ignore_ascii_case(&name))
                && let Some(clip) = anims.str(r, "name")
            {
                out.push(clip.to_ascii_lowercase());
            }
        }
    }
    let lower = name.to_ascii_lowercase();
    let extra: &[&str] = match lower.as_str() {
        "listen" => &["listen"],
        "pause" => &["pause1"],
        "ready" => &["ready", "g1r1", "pause1"],
        "dead" => &["dead", "dead1"],
        "worship" => &["worship"],
        "activate" => &["activate"],
        "inject" => &["inject"],
        "persuade" => &["persuade"],
        "victory" => &["victory"],
        "scratch_head" => &["pausesh"],
        "drunk" => &["pausedrnk", "pausetrd"],
        "choked" => &["choke"],
        "talk_injured" => &["talkinj"],
        "listen_injured" => &["listeninj"],
        "sleep" => &["sleep"],
        "prone" => &["prone"],
        "use_computer_lp" => &["usecomplp", "usecomp"],
        "kneel_talk_angry" | "kneel_talk_sad" => &["kneel"],
        "point_loop" => &["pointloop"],
        "point_down" => &["pointdown"],
        "talk_prone_loop" => &["talkprone"],
        "talk_prone_die" => &["talkpronedie"],
        s if s.starts_with("sit_chair") => &["sit"],
        _ => &[],
    };
    out.extend(extra.iter().map(|s| (*s).to_owned()));
    out.push(lower.replace('_', ""));
    (out, looping)
}

fn read_seconds(text: &str) -> f32 {
    if text.trim().is_empty() { 0.0 } else { (1.2 + text.len() as f32 * 0.055).clamp(2.0, 12.0) - 0.4 }
}

/// Length of a line's synthetic lip track: the clip length of an alien line
/// (it has no LIP file), else the text estimate.
fn lip_seconds(alien: &AlienLine, dialog: &str, entry: usize, text: &str) -> f32 {
    match &alien.line {
        Some((d, e)) if d == dialog && *e == entry => alien.seconds,
        _ => read_seconds(text),
    }
}

/// On each new NPC line: pick the shot, start participant animations and
/// lip sync.
#[allow(clippy::too_many_arguments)]
fn on_line(
    mut commands: Commands,
    res: Res<GameRes>,
    gd: Option<Res<GameData>>,
    area: Option<Res<AreaModule>>,
    world: Res<WorldRes>,
    ents: Res<ObjEntities>,
    mut cam: ResMut<CutCam>,
    mut danims: ResMut<DialogAnims>,
    mut lips: ResMut<LipIndex>,
    alien_line: Res<AlienLine>,
    bodies: Query<&ObjBody>,
    mut canims: Query<&mut CreatureAnim>,
    active_lips: Query<Entity, With<LipSync>>,
    mut players: Query<&mut crate::anim::AnimPlayer>,
) {
    let w = &world.0;
    let Some(c) = &w.conversation else {
        if cam.key.is_some() {
            cam.key = None;
            cam.shot = Shot::Default;
            cam.anim = None;
            for (id, clip) in danims.0.drain(..) {
                clear_dialog_anim(&ents, &mut canims, id, &clip);
            }
            for e in &active_lips {
                commands.entity(e).remove::<LipSync>();
            }
        }
        return;
    };
    let Some(entry) = c.entry else { return };
    let key = (c.dialog.resref.clone(), entry);
    if cam.key.as_ref() == Some(&key) {
        return;
    }
    cam.key = Some(key);
    let Some(node) = c.dialog.entries.get(entry) else { return };
    let Some(view) = w.conversation_view() else { return };

    // Shot.
    let angle = arg_value("--force-angle").and_then(|v| v.parse().ok()).unwrap_or(node.camera_angle);
    let prev = cam.shot.clone();
    cam.shot = match angle {
        1 => Shot::Face,
        3 => Shot::Wide,
        5 => match prev {
            Shot::Default => cam.last.map_or(Shot::Default, Shot::Frozen),
            other => other,
        },
        6 => {
            let id = node.camera_id.unwrap_or(-1);
            match area.as_ref().and_then(|a| a.0.git.cameras.iter().find(|k| k.id == id)) {
                Some(k) => {
                    let [qw, qx, qy, qz] = k.orientation;
                    let rot = Quat::from_xyzw(qx, qy, qz, qw).normalize() * Quat::from_rotation_x(k.pitch.to_radians());
                    let pos = Vec3::from(k.position) + Vec3::Z * k.height;
                    Shot::Static { pos, rot, fov: if k.fov > 1.0 { k.fov } else { 55.0 } }
                }
                None => {
                    warn!("cutscene: {} entry {entry}: static camera {id} not in GIT", c.dialog.resref);
                    Shot::Default
                }
            }
        }
        4 => {
            let model = c.dialog.camera_model.clone().unwrap_or_default().to_ascii_lowercase();
            if !cam.models.contains_key(&model) {
                let m = if model.is_empty() { None } else { cam_model(&res, &model) };
                if m.is_none() {
                    warn!("cutscene: camera model {model:?} unusable");
                }
                cam.models.insert(model.clone(), m);
            }
            let id = node.camera_animation.unwrap_or(0);
            let start = (1200..1300).contains(&id).then(|| format!("cut{:03}w", id - 1199));
            match (start, cam.models.get(&model).cloned().flatten()) {
                (Some(name), Some((rest_t, rest_r, scale, clips))) => match clips.get(&name) {
                    Some(clip) => {
                        cam.anim = Some(CamAnim { clip: clip.clone(), rest_t, rest_r, scale, time: 0.0 });
                        info!("cutscene: {} entry {entry}: {model} {name} ({:.1}s)", c.dialog.resref, clip.length);
                        Shot::Animated
                    }
                    None => {
                        warn!("cutscene: {model} has no {name}");
                        Shot::Default
                    }
                },
                // Keep the running clip (1000/1001/1401 style ids).
                (None, _) if cam.anim.is_some() => Shot::Animated,
                _ => Shot::Default,
            }
        }
        _ => Shot::Default,
    };
    if !matches!(cam.shot, Shot::Animated) && angle != 5 {
        cam.anim = None;
    }
    if angle != 0 {
        info!("cutscene: {} entry {entry} camera angle {angle} id {:?} anim {:?}", c.dialog.resref, node.camera_id, node.camera_animation);
    }

    // Participant animations. A held cutscene pose (the legacy one-shots)
    // stays through the lines that follow until the conversation ends or
    // another animation replaces it.
    for (id, clip) in danims.0.drain(..).collect::<Vec<_>>() {
        if is_held(&ents, &canims, id, &clip) {
            danims.0.push((id, clip));
        } else {
            clear_dialog_anim(&ents, &mut canims, id, &clip);
        }
    }
    if let Some(gd) = gd.as_ref() {
        for a in &node.anims {
            let target = if a.participant.is_empty() || a.participant.eq_ignore_ascii_case("owner") {
                Some(c.owner)
            } else if a.participant.eq_ignore_ascii_case("player") {
                // The PC has no usable tag; DLGs address it as PLAYER.
                Some(w.party_leader())
            } else {
                w.by_tag(&a.participant, 0)
            };
            let Some(target) = target else { continue };
            let (cands, looping) = dialog_anim_clips(gd, &res, a.animation);
            let Some(&root) = ents.0.get(&target) else { continue };
            let Ok(body) = bodies.get(root) else { continue };
            let Some(b) = body.0 else { continue };
            let Ok(mut p) = players.get_mut(b) else { continue };
            let Some(clip) = cands.iter().find(|n| p.set.has(n)).cloned() else {
                debug!("cutscene: no clip for dialog animation {} ({cands:?})", a.animation);
                continue;
            };
            p.fade_to(&clip, looping, 1.0, 0.2);
            if let Ok(mut ca) = canims.get_mut(root) {
                // Legacy cutscene one-shots keep their last frame (the
                // collapse pose) until the next line replaces them.
                let hold = (a.animation < 10000 && !looping).then_some(f32::INFINITY);
                ca.scripted = Some((clip.clone(), looping, hold));
            }
            debug!("cutscene: {} plays {clip} (looping {looping})", w.describe(target));
            danims.0.push((target, clip));
        }
    }

    // Lip sync on the speaker.
    for e in &active_lips {
        commands.entity(e).remove::<LipSync>();
    }
    if view.text.trim().is_empty() {
        return;
    }
    let lip = view.vo.as_deref().and_then(|vo| lips.get(&res.0, vo));
    let found = lip.is_some();
    let seconds = lip_seconds(&alien_line, &view.dialog, view.entry, &view.text);
    let data = lip.unwrap_or_else(|| synthetic_lip(&view.text, seconds));
    if let Some(&root) = ents.0.get(&view.speaker)
        && let Ok(body) = bodies.get(root)
        && let Some(b) = body.0
    {
        debug!("lipsync: {} vo {:?} lip {} ({} keys, {:.1}s)", w.describe(view.speaker), view.vo, if found { "file" } else { "synthetic" }, data.keys.len(), data.length);
        commands.entity(b).insert(LipSync::new(data));
    }
}

/// Logs the voice sink's live state 1.5 s into each line (proof VO plays
/// under cutscene cameras).
fn report_voice(
    time: Res<Time<Real>>,
    world: Res<WorldRes>,
    sinks: Query<(&Name, &bevy::audio::AudioSink), With<crate::audio::VoiceLine>>,
    mut line: Local<ReportedLine>,
) {
    let Some(c) = &world.0.conversation else {
        *line = None;
        return;
    };
    let Some(entry) = c.entry else { return };
    let key = (c.dialog.resref.clone(), entry);
    let now = time.elapsed_secs_f64();
    if line.as_ref().is_none_or(|l| l.0 != key) {
        *line = Some((key, now, false));
        return;
    }
    let Some(l) = line.as_mut() else { return };
    if l.2 || now - l.1 < 1.5 {
        return;
    }
    l.2 = true;
    let vo = c.dialog.entries.get(entry).and_then(|n| n.vo_resref.clone()).unwrap_or_default();
    let states: Vec<String> = sinks
        .iter()
        .map(|(n, s)| format!("{n} paused={} muted={} vol={:.2} pos={:.2}s empty={}", s.is_paused(), s.is_muted(), s.volume().to_linear(), s.position().as_secs_f32(), s.empty()))
        .collect();
    info!("voice: {}[{}] vo {vo:?}: {} sink(s) {states:?}", c.dialog.resref, entry, states.len());
}

/// The participant's scripted animation is `clip` held at its last frame.
fn is_held(ents: &ObjEntities, canims: &Query<&mut CreatureAnim>, id: ObjectId, clip: &str) -> bool {
    ents.0.get(&id).and_then(|&root| canims.get(root).ok()).is_some_and(|ca| ca.scripted.as_ref().is_some_and(|s| s.0 == clip && !s.1 && s.2.is_some_and(f32::is_infinite)))
}

fn clear_dialog_anim(ents: &ObjEntities, canims: &mut Query<&mut CreatureAnim>, id: ObjectId, clip: &str) {
    if let Some(&root) = ents.0.get(&id)
        && let Ok(mut ca) = canims.get_mut(root)
        && ca.scripted.as_ref().is_some_and(|s| s.0 == clip)
    {
        ca.scripted = None;
    }
}

/// Bevy transform for a camera placed in game space (local -Z forward,
/// +Y up in both spaces).
fn game_cam(pos: Vec3, rot: Quat) -> Transform {
    Transform { translation: coords::to_bevy(pos), rotation: Quat::from_rotation_x(-FRAC_PI_2) * rot, scale: Vec3::ONE }
}

#[allow(clippy::too_many_arguments)]
fn drive_camera(
    time: Res<Time>,
    world: Res<WorldRes>,
    ents: Res<ObjEntities>,
    objs: Query<&GlobalTransform>,
    mut cam: ResMut<CutCam>,
    mut cams: Query<(&mut Transform, &mut Projection), With<Camera3d>>,
    mode: Res<crate::play::CamMode>,
) {
    // The debug fly camera (F1, --cam) wins over dialog cameras.
    if *mode == crate::play::CamMode::Fly {
        return;
    }
    let Ok((mut t, mut proj)) = cams.single_mut() else { return };
    let Projection::Perspective(pp) = &mut *proj else { return };
    let default_fov = *cam.default_fov.get_or_insert(pp.fov);
    let w = &world.0;
    let view = w.conversation_view();
    if view.is_none() {
        pp.fov = default_fov;
        cam.last = None;
        return;
    }
    let v = view.unwrap();
    let pos = |id: ObjectId| -> Option<Vec3> {
        ents.0.get(&id).and_then(|e| objs.get(*e).ok()).map(|g| coords::to_game(g.translation())).or_else(|| w.get(id).map(|o| o.position))
    };
    let dt = time.delta_secs();
    let mut fov = default_fov;
    let target = match cam.shot.clone() {
        Shot::Default => None,
        Shot::Frozen(tf) => Some(tf),
        Shot::Static { pos, rot, fov: f } => {
            fov = f.to_radians();
            Some(game_cam(pos, rot))
        }
        Shot::Animated => {
            let Some(a) = &mut cam.anim else { return };
            a.time = (a.time + dt).min(a.clip.length);
            let tr = a.clip.tracks.get("camerahook");
            let p = a.rest_t + tr.and_then(|tr| tr.sample_pos(a.time)).unwrap_or(Vec3::ZERO) * a.scale;
            let r = tr.and_then(|tr| tr.sample_rot(a.time)).unwrap_or(a.rest_r);
            Some(game_cam(p, r))
        }
        Shot::Face => pos(v.speaker).map(|s| {
            // From the listener's side, so the shot has a clear line; the
            // speaker's facing only when talking to itself.
            let other = if v.listener != v.speaker { v.listener } else { w.party_leader() };
            let towards = pos(other).map(|o| (o - s).truncate()).filter(|d| d.length() > 0.3 && d.length() < crate::dialog_ui::SOLO_SHOT_DIST);
            let f = match towards {
                Some(d) => d.normalize().extend(0.0),
                None => {
                    let b = w.get(v.speaker).map_or(0.0, |o| o.bearing + FRAC_PI_2);
                    Vec3::new(b.cos(), b.sin(), 0.0)
                }
            };
            let eye = s + f * 1.0 + Vec3::Z * 1.62;
            Transform::from_translation(coords::to_bevy(eye)).looking_at(coords::to_bevy(s + Vec3::Z * 1.58), Vec3::Y)
        }),
        Shot::Wide => {
            let other = if v.listener != v.speaker { v.listener } else { w.party_leader() };
            match (pos(v.speaker), pos(other)) {
                (Some(a), Some(b)) if (b - a).truncate().length() < crate::dialog_ui::SOLO_SHOT_DIST => {
                    let mid = (a + b) * 0.5;
                    let d = (b - a).truncate();
                    let side = if d.length() > 0.1 { Vec3::new(-d.y, d.x, 0.0).normalize() } else { Vec3::X };
                    let eye = mid + side * (3.0 + d.length() * 0.8) + Vec3::Z * 2.0;
                    Some(Transform::from_translation(coords::to_bevy(eye)).looking_at(coords::to_bevy(mid + Vec3::Z * 1.2), Vec3::Y))
                }
                _ => None,
            }
        }
    };
    match target {
        Some(tf) => {
            *t = tf;
            pp.fov = fov;
        }
        None => pp.fov = default_fov,
    }
    cam.last = Some(*t);
}

#[cfg(test)]
mod tests;

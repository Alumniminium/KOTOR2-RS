//! Modules: IFO (module info), ARE (area), GIT (instances), LYT, VIS.
//!
//! Orientation convention: every instance carries `bearing`, a rotation in
//! radians about +Z applied to a model that faces +Y. Doors and placeables
//! store it directly (`Bearing`); creatures, waypoints and the module entry
//! store a facing vector (x, y), converted as `-atan2(x, y)`.

use k2_res::Resources;
use rakata_core::ResourceType;
use rakata_formats::gff::GffStruct;

use crate::gff::{GffExt, LocString, color_from_u32};
use crate::vis::Visibility;
use crate::{DataError, Result, parse_err, read_bytes, read_gff};

/// Facing vector (x, y) to bearing about +Z for a +Y-facing model.
pub fn bearing_from_dir(x: f32, y: f32) -> f32 {
    if x == 0.0 && y == 0.0 { 0.0 } else { -x.atan2(y) }
}

/// Unit facing vector for a bearing (inverse of `bearing_from_dir`).
pub fn dir_from_bearing(bearing: f32) -> [f32; 2] {
    [-bearing.sin(), bearing.cos()]
}

/// module.ifo
#[derive(Debug, Clone, Default)]
pub struct ModuleInfo {
    pub name: LocString,
    pub tag: String,
    pub vo_id: String,
    pub entry_area: String,
    pub entry_position: [f32; 3],
    pub entry_bearing: f32,
    pub areas: Vec<String>,
    pub start_movie: Option<String>,
    pub dawn_hour: u32,
    pub dusk_hour: u32,
    pub min_per_hour: u32,
    pub start_hour: u32,
    /// Mod_On* label to script resref.
    pub scripts: Vec<(String, String)>,
}

const IFO_SCRIPTS: &[&str] = &[
    "Mod_OnHeartbeat", "Mod_OnModLoad", "Mod_OnModStart", "Mod_OnClientEntr", "Mod_OnClientLeav",
    "Mod_OnActvtItem", "Mod_OnAcquirItem", "Mod_OnUsrDefined", "Mod_OnUnAqreItem", "Mod_OnPlrDeath",
    "Mod_OnPlrDying", "Mod_OnPlrLvlUp", "Mod_OnSpawnBtnDn", "Mod_OnPlrRest",
];

fn scripts(s: &GffStruct, labels: &[&str]) -> Vec<(String, String)> {
    labels.iter().filter_map(|l| s.resref(l).map(|r| ((*l).to_owned(), r))).collect()
}

fn script<'a>(list: &'a [(String, String)], label: &str) -> Option<&'a str> {
    list.iter().find(|(l, _)| l == label).map(|(_, r)| r.as_str())
}

impl ModuleInfo {
    pub fn from_gff(s: &GffStruct) -> Self {
        let areas: Vec<String> = s.list("Mod_Area_list").iter().filter_map(|a| a.resref("Area_Name")).collect();
        Self {
            name: s.locstring("Mod_Name"),
            tag: s.string_or_empty("Mod_Tag"),
            vo_id: s.string_or_empty("Mod_VO_ID"),
            entry_area: s.resref("Mod_Entry_Area").or_else(|| areas.first().cloned()).unwrap_or_default(),
            entry_position: s.xyz("Mod_Entry_X", "Mod_Entry_Y", "Mod_Entry_Z"),
            entry_bearing: bearing_from_dir(s.float_or("Mod_Entry_Dir_X", 0.0), s.float_or("Mod_Entry_Dir_Y", 1.0)),
            areas,
            start_movie: s.resref("Mod_StartMovie"),
            dawn_hour: s.uint("Mod_DawnHour").unwrap_or(6),
            dusk_hour: s.uint("Mod_DuskHour").unwrap_or(18),
            min_per_hour: s.uint("Mod_MinPerHour").unwrap_or(2),
            start_hour: s.uint("Mod_StartHour").unwrap_or(12),
            scripts: scripts(s, IFO_SCRIPTS),
        }
    }

    pub fn script(&self, label: &str) -> Option<&str> {
        script(&self.scripts, label)
    }
}

/// One ARE room entry.
#[derive(Debug, Clone, Default)]
pub struct AreaRoom {
    pub name: String,
    pub env_audio: i32,
    pub ambient_scale: f32,
    pub force_rating: i32,
    pub disable_weather: bool,
}

/// Fog parameters.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Fog {
    pub enabled: bool,
    pub near: f32,
    pub far: f32,
    pub color: [f32; 3],
}

/// Area minimap mapping (ARE Map struct).
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct AreaMap {
    pub north_axis: i32,
    pub map_res_x: i32,
    pub world_pt1: [f32; 2],
    pub world_pt2: [f32; 2],
    pub map_pt1: [f32; 2],
    pub map_pt2: [f32; 2],
}

/// Area (ARE).
#[derive(Debug, Clone, Default)]
pub struct Area {
    pub resref: String,
    pub tag: String,
    pub name: LocString,
    pub rooms: Vec<AreaRoom>,
    pub ambient: [f32; 3],
    pub dynamic_ambient: [f32; 3],
    pub diffuse: [f32; 3],
    pub fog: Fog,
    pub sun_shadows: bool,
    pub shadow_opacity: u32,
    pub is_night: bool,
    pub alpha_test: f32,
    /// camerastyle.2da row.
    pub camera_style: u32,
    pub default_env_map: Option<String>,
    pub grass_texture: Option<String>,
    pub unescapable: bool,
    pub disable_transit: bool,
    pub no_rest: bool,
    pub map: AreaMap,
    pub scripts: Vec<(String, String)>,
}

impl Area {
    pub fn from_gff(s: &GffStruct, resref: &str) -> Self {
        let night = s.flag("IsNight");
        let (amb, dif, fog_on, near, far, fogc) = if night {
            ("MoonAmbientColor", "MoonDiffuseColor", "MoonFogOn", "MoonFogNear", "MoonFogFar", "MoonFogColor")
        } else {
            ("SunAmbientColor", "SunDiffuseColor", "SunFogOn", "SunFogNear", "SunFogFar", "SunFogColor")
        };
        let m = s.child("Map");
        let map = m
            .map(|m| AreaMap {
                north_axis: m.int_or("NorthAxis", 0),
                map_res_x: m.int_or("MapResX", 0),
                world_pt1: [m.float_or("WorldPt1X", 0.0), m.float_or("WorldPt1Y", 0.0)],
                world_pt2: [m.float_or("WorldPt2X", 0.0), m.float_or("WorldPt2Y", 0.0)],
                map_pt1: [m.float_or("MapPt1X", 0.0), m.float_or("MapPt1Y", 0.0)],
                map_pt2: [m.float_or("MapPt2X", 0.0), m.float_or("MapPt2Y", 0.0)],
            })
            .unwrap_or_default();
        Self {
            resref: resref.to_ascii_lowercase(),
            tag: s.string_or_empty("Tag"),
            name: s.locstring("Name"),
            rooms: s
                .list("Rooms")
                .iter()
                .map(|r| AreaRoom {
                    name: r.string_or_empty("RoomName").to_ascii_lowercase(),
                    env_audio: r.int_or("EnvAudio", 0),
                    ambient_scale: r.float_or("AmbientScale", 1.0),
                    force_rating: r.int_or("ForceRating", 0),
                    disable_weather: r.flag("DisableWeather"),
                })
                .collect(),
            ambient: color_from_u32(s.uint(amb).unwrap_or(0)),
            dynamic_ambient: color_from_u32(s.uint("DynAmbientColor").unwrap_or(0)),
            diffuse: color_from_u32(s.uint(dif).unwrap_or(0)),
            fog: Fog {
                enabled: s.flag(fog_on),
                near: s.float_or(near, 99.0),
                far: s.float_or(far, 100.0),
                color: color_from_u32(s.uint(fogc).unwrap_or(0)),
            },
            sun_shadows: s.flag(if night { "MoonShadows" } else { "SunShadows" }),
            shadow_opacity: s.uint("ShadowOpacity").unwrap_or(0),
            is_night: night,
            alpha_test: s.float_or("AlphaTest", 0.2),
            camera_style: s.uint("CameraStyle").unwrap_or(0),
            default_env_map: s.resref("DefaultEnvMap"),
            grass_texture: s.resref("Grass_TexName"),
            unescapable: s.flag("Unescapable"),
            disable_transit: s.flag("DisableTransit"),
            no_rest: s.flag("NoRest"),
            map,
            scripts: scripts(s, &["OnEnter", "OnExit", "OnHeartbeat", "OnUserDefined"]),
        }
    }

    pub fn script(&self, label: &str) -> Option<&str> {
        script(&self.scripts, label)
    }
}

/// GIT creature instance.
#[derive(Debug, Clone, Default)]
pub struct CreatureInstance {
    pub template: String,
    pub position: [f32; 3],
    pub bearing: f32,
}

/// GIT door instance. Instance fields override the blueprint.
#[derive(Debug, Clone, Default)]
pub struct DoorInstance {
    pub template: String,
    pub tag: String,
    pub linked_to: String,
    pub linked_to_module: Option<String>,
    pub linked_to_flags: u32,
    pub transition_destination: LocString,
    pub position: [f32; 3],
    pub bearing: f32,
    pub tweak_color: Option<[f32; 3]>,
}

/// GIT placeable instance.
#[derive(Debug, Clone, Default)]
pub struct PlaceableInstance {
    pub template: String,
    pub position: [f32; 3],
    pub bearing: f32,
    pub tweak_color: Option<[f32; 3]>,
}

/// GIT waypoint instance (waypoints carry their data inline).
#[derive(Debug, Clone, Default)]
pub struct WaypointInstance {
    pub template: String,
    pub tag: String,
    pub name: LocString,
    pub linked_to: String,
    pub position: [f32; 3],
    pub bearing: f32,
    pub has_map_note: bool,
    pub map_note: LocString,
}

/// GIT trigger instance. Geometry points are relative to `position`.
#[derive(Debug, Clone, Default)]
pub struct TriggerInstance {
    pub template: String,
    pub tag: Option<String>,
    pub linked_to: Option<String>,
    pub linked_to_module: Option<String>,
    pub transition_destination: LocString,
    pub position: [f32; 3],
    pub geometry: Vec<[f32; 3]>,
}

impl TriggerInstance {
    /// Polygon in world coordinates.
    pub fn world_polygon(&self) -> Vec<[f32; 3]> {
        self.geometry
            .iter()
            .map(|p| [p[0] + self.position[0], p[1] + self.position[1], p[2] + self.position[2]])
            .collect()
    }

    /// Point-in-polygon test on the XY plane (world coordinates).
    pub fn contains_xy(&self, x: f32, y: f32) -> bool {
        point_in_polygon(&self.world_polygon(), x, y)
    }
}

/// Even-odd point-in-polygon test on XY.
pub fn point_in_polygon(poly: &[[f32; 3]], x: f32, y: f32) -> bool {
    let mut inside = false;
    let n = poly.len();
    for i in 0..n {
        let (a, b) = (poly[i], poly[(i + n - 1) % n]);
        if (a[1] > y) != (b[1] > y) && x < (b[0] - a[0]) * (y - a[1]) / (b[1] - a[1]) + a[0] {
            inside = !inside;
        }
    }
    inside
}

/// GIT sound instance.
#[derive(Debug, Clone, Default)]
pub struct SoundInstance {
    pub template: String,
    pub position: [f32; 3],
    pub generated_type: u32,
}

/// GIT store instance.
#[derive(Debug, Clone, Default)]
pub struct StoreInstance {
    pub template: String,
    pub tag: String,
    pub position: [f32; 3],
    pub bearing: f32,
}

/// GIT encounter instance.
#[derive(Debug, Clone, Default)]
pub struct EncounterInstance {
    pub template: String,
    pub position: [f32; 3],
    pub geometry: Vec<[f32; 3]>,
    pub spawn_points: Vec<([f32; 3], f32)>,
}

/// GIT static camera.
#[derive(Debug, Clone, Default)]
pub struct CameraInstance {
    pub id: i32,
    pub position: [f32; 3],
    /// Scalar first [w, x, y, z].
    pub orientation: [f32; 4],
    pub pitch: f32,
    pub height: f32,
    pub fov: f32,
    pub mic_range: f32,
}

/// Area music and ambience (GIT AreaProperties).
#[derive(Debug, Clone, Copy, Default)]
pub struct AreaProperties {
    pub ambient_day: i32,
    pub ambient_night: i32,
    pub ambient_day_volume: i32,
    pub ambient_night_volume: i32,
    pub env_audio: i32,
    /// ambientmusic.2da rows.
    pub music_day: i32,
    pub music_night: i32,
    pub music_battle: i32,
    pub music_delay: i32,
}

/// Instances in an area (GIT).
#[derive(Debug, Clone, Default)]
pub struct Git {
    pub properties: AreaProperties,
    pub creatures: Vec<CreatureInstance>,
    pub doors: Vec<DoorInstance>,
    pub placeables: Vec<PlaceableInstance>,
    pub waypoints: Vec<WaypointInstance>,
    pub triggers: Vec<TriggerInstance>,
    pub sounds: Vec<SoundInstance>,
    pub stores: Vec<StoreInstance>,
    pub encounters: Vec<EncounterInstance>,
    pub cameras: Vec<CameraInstance>,
}

fn pos_xyz(s: &GffStruct) -> [f32; 3] {
    s.xyz("XPosition", "YPosition", "ZPosition")
}

fn pos_plain(s: &GffStruct) -> [f32; 3] {
    s.xyz("X", "Y", "Z")
}

fn tweak(s: &GffStruct) -> Option<[f32; 3]> {
    s.flag("UseTweakColor").then(|| color_from_u32(s.uint("TweakColor").unwrap_or(0)))
}

fn template(s: &GffStruct) -> String {
    s.resref("TemplateResRef").unwrap_or_default()
}

fn opt_string(s: &GffStruct, label: &str) -> Option<String> {
    s.string(label).filter(|v| !v.is_empty())
}

impl Git {
    pub fn from_gff(s: &GffStruct) -> Self {
        let props = s.child("AreaProperties");
        let p = |l: &str| props.map_or(0, |p| p.int_or(l, 0));
        Self {
            properties: AreaProperties {
                ambient_day: p("AmbientSndDay"),
                ambient_night: p("AmbientSndNight"),
                ambient_day_volume: p("AmbientSndDayVol"),
                ambient_night_volume: p("AmbientSndNitVol"),
                env_audio: p("EnvAudio"),
                music_day: p("MusicDay"),
                music_night: p("MusicNight"),
                music_battle: p("MusicBattle"),
                music_delay: p("MusicDelay"),
            },
            creatures: s
                .list("Creature List")
                .iter()
                .map(|c| CreatureInstance {
                    template: template(c),
                    position: pos_xyz(c),
                    bearing: bearing_from_dir(c.float_or("XOrientation", 0.0), c.float_or("YOrientation", 1.0)),
                })
                .collect(),
            doors: s
                .list("Door List")
                .iter()
                .map(|d| DoorInstance {
                    template: template(d),
                    tag: d.string_or_empty("Tag"),
                    linked_to: d.string_or_empty("LinkedTo"),
                    linked_to_module: d.resref("LinkedToModule"),
                    linked_to_flags: d.uint("LinkedToFlags").unwrap_or(0),
                    transition_destination: d.locstring("TransitionDestin"),
                    position: pos_plain(d),
                    bearing: d.float_or("Bearing", 0.0),
                    tweak_color: tweak(d),
                })
                .collect(),
            placeables: s
                .list("Placeable List")
                .iter()
                .map(|d| PlaceableInstance {
                    template: template(d),
                    position: pos_plain(d),
                    bearing: d.float_or("Bearing", 0.0),
                    tweak_color: tweak(d),
                })
                .collect(),
            waypoints: s
                .list("WaypointList")
                .iter()
                .map(|w| WaypointInstance {
                    template: template(w),
                    tag: w.string_or_empty("Tag"),
                    name: w.locstring("LocalizedName"),
                    linked_to: w.string_or_empty("LinkedTo"),
                    position: pos_xyz(w),
                    bearing: bearing_from_dir(w.float_or("XOrientation", 0.0), w.float_or("YOrientation", 1.0)),
                    has_map_note: w.flag("HasMapNote"),
                    map_note: w.locstring("MapNote"),
                })
                .collect(),
            triggers: s
                .list("TriggerList")
                .iter()
                .map(|t| TriggerInstance {
                    template: template(t),
                    tag: opt_string(t, "Tag"),
                    linked_to: opt_string(t, "LinkedTo"),
                    linked_to_module: t.resref("LinkedToModule"),
                    transition_destination: t.locstring("TransitionDestin"),
                    position: pos_xyz(t),
                    geometry: t.list("Geometry").iter().map(|g| g.xyz("PointX", "PointY", "PointZ")).collect(),
                })
                .collect(),
            sounds: s
                .list("SoundList")
                .iter()
                .map(|t| SoundInstance { template: template(t), position: pos_xyz(t), generated_type: t.uint("GeneratedType").unwrap_or(0) })
                .collect(),
            stores: s
                .list("StoreList")
                .iter()
                .map(|t| StoreInstance {
                    template: s_resref(t),
                    tag: t.string_or_empty("Tag"),
                    position: pos_xyz(t),
                    bearing: bearing_from_dir(t.float_or("XOrientation", 0.0), t.float_or("YOrientation", 1.0)),
                })
                .collect(),
            encounters: s
                .list("Encounter List")
                .iter()
                .map(|t| EncounterInstance {
                    template: template(t),
                    position: pos_xyz(t),
                    geometry: t.list("Geometry").iter().map(|g| g.xyz("X", "Y", "Z")).collect(),
                    spawn_points: t
                        .list("SpawnPointList")
                        .iter()
                        .map(|p| (p.xyz("X", "Y", "Z"), p.float_or("Orientation", 0.0)))
                        .collect(),
                })
                .collect(),
            cameras: s
                .list("CameraList")
                .iter()
                .map(|c| CameraInstance {
                    id: c.int_or("CameraID", 0),
                    position: c.vec3("Position").unwrap_or_default(),
                    orientation: c.quat("Orientation").unwrap_or([1.0, 0.0, 0.0, 0.0]),
                    pitch: c.float_or("Pitch", 0.0),
                    height: c.float_or("Height", 0.0),
                    fov: c.float_or("FieldOfView", 55.0),
                    mic_range: c.float_or("MicRange", 0.0),
                })
                .collect(),
        }
    }
}

/// Stores name their blueprint ResRef rather than TemplateResRef.
fn s_resref(s: &GffStruct) -> String {
    s.resref("ResRef").or_else(|| s.resref("TemplateResRef")).unwrap_or_default()
}

/// LYT room placement.
#[derive(Debug, Clone, Default)]
pub struct LayoutRoom {
    pub model: String,
    pub position: [f32; 3],
}

/// LYT door hook.
#[derive(Debug, Clone, Default)]
pub struct LayoutDoorHook {
    pub room: String,
    pub door: String,
    pub position: [f32; 3],
    /// Scalar last [x, y, z, w], as LYT stores it.
    pub orientation: [f32; 4],
}

/// Area layout (LYT).
#[derive(Debug, Clone, Default)]
pub struct Layout {
    pub rooms: Vec<LayoutRoom>,
    pub door_hooks: Vec<LayoutDoorHook>,
    pub tracks: Vec<LayoutRoom>,
    pub obstacles: Vec<LayoutRoom>,
}

impl Layout {
    pub fn parse(bytes: &[u8]) -> std::result::Result<Self, String> {
        let lyt = rakata_formats::read_lyt_from_bytes(bytes).map_err(|e| e.to_string())?;
        let v = |p: &rakata_formats::lyt::Vec3| [p.x, p.y, p.z];
        let room = |model: &str, p: &rakata_formats::lyt::Vec3| LayoutRoom { model: model.to_ascii_lowercase(), position: v(p) };
        Ok(Self {
            rooms: lyt.rooms.iter().map(|r| room(&r.model, &r.position)).collect(),
            door_hooks: lyt
                .doorhooks
                .iter()
                .map(|d| LayoutDoorHook {
                    room: d.room.to_ascii_lowercase(),
                    door: d.door.to_ascii_lowercase(),
                    position: v(&d.position),
                    orientation: [d.orientation.x, d.orientation.y, d.orientation.z, d.orientation.w],
                })
                .collect(),
            tracks: lyt.tracks.iter().map(|r| room(&r.model, &r.position)).collect(),
            obstacles: lyt.obstacles.iter().map(|r| room(&r.model, &r.position)).collect(),
        })
    }
}

/// A loaded module: its info plus the entry area's ARE, GIT, LYT and VIS.
#[derive(Debug, Clone, Default)]
pub struct Module {
    pub name: String,
    pub info: ModuleInfo,
    pub area: Area,
    pub git: Git,
    pub layout: Layout,
    pub vis: Visibility,
}

impl Module {
    /// Mount the module's capsules on `res` and load it.
    pub fn load(res: &mut Resources, name: &str) -> Result<Self> {
        let name = name.to_ascii_lowercase();
        if res.install.module_capsules(&name).is_empty() {
            return Err(DataError::Missing(format!("module {name}")));
        }
        res.load_module(&name)?;
        Self::from_mounted(res, &name)
    }

    /// Load from a module already mounted on `res`.
    pub fn from_mounted(res: &Resources, name: &str) -> Result<Self> {
        let info = ModuleInfo::from_gff(&read_gff(res, "module", ResourceType::Ifo)?.root);
        let area_name = info.entry_area.clone();
        let area = Area::from_gff(&read_gff(res, &area_name, ResourceType::Are)?.root, &area_name);
        let git = Git::from_gff(&read_gff(res, &area_name, ResourceType::Git)?.root);
        let lyt_bytes = read_bytes(res, &area_name, ResourceType::Lyt)?;
        let layout = Layout::parse(&lyt_bytes).map_err(|e| parse_err(format!("{area_name}.lyt"), e))?;
        let vis = match read_bytes(res, &area_name, ResourceType::Vis) {
            Ok(b) => Visibility::parse(&b),
            Err(DataError::Missing(_)) => Visibility::default(),
            Err(e) => return Err(e),
        };
        Ok(Self { name: name.to_ascii_lowercase(), info, area, git, layout, vis })
    }

    /// Waypoint by tag (case-insensitive).
    pub fn waypoint(&self, tag: &str) -> Option<&WaypointInstance> {
        self.git.waypoints.iter().find(|w| w.tag.eq_ignore_ascii_case(tag))
    }

    /// Door by tag (case-insensitive).
    pub fn door(&self, tag: &str) -> Option<&DoorInstance> {
        self.git.doors.iter().find(|d| d.tag.eq_ignore_ascii_case(tag))
    }

    /// Room containing an XY point, by nearest room origin. A coarse
    /// fallback; the walkmesh gives the exact room.
    pub fn nearest_room(&self, x: f32, y: f32) -> Option<&LayoutRoom> {
        self.layout.rooms.iter().min_by(|a, b| {
            let da = (a.position[0] - x).powi(2) + (a.position[1] - y).powi(2);
            let db = (b.position[0] - x).powi(2) + (b.position[1] - y).powi(2);
            da.total_cmp(&db)
        })
    }
}

#[cfg(test)]
mod tests;

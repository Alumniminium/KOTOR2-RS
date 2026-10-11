//! KotOR-style material: diffuse texture times lightmap (UV2), or a simple
//! ambient+directional term when the mesh has no lightmap.

use bevy::asset::uuid_handle;
use bevy::prelude::*;
use bevy::render::render_asset::RenderAssets;
use bevy::render::render_resource::{AsBindGroup, AsBindGroupShaderType, ShaderType};
use bevy::render::texture::GpuImage;
use bevy::shader::ShaderRef;

pub const SHADER: Handle<Shader> = uuid_handle!("6c1f6b8e-3f0a-4d8e-9a43-1c2b5e7d9f10");
/// Fragment shader of the shadow-map pass (see PREPASS_SRC).
pub const PREPASS_SHADER: Handle<Shader> = uuid_handle!("6c1f6b8e-3f0a-4d8e-9a43-1c2b5e7d9f11");

const SHADER_SRC: &str = r#"
#import bevy_pbr::forward_io::VertexOutput
#import bevy_pbr::mesh_view_bindings::{globals, view, lights}
#import bevy_pbr::mesh_bindings::mesh
#import bevy_pbr::mesh_types::MESH_FLAGS_SHADOW_RECEIVER_BIT
#import bevy_pbr::mesh_view_types::DIRECTIONAL_LIGHT_FLAGS_SHADOWS_ENABLED_BIT
#import bevy_pbr::shadows

struct KParams {
    flags: u32,
    alpha_cut: f32,
    _pad0: f32,
    _pad1: f32,
    tint: vec4<f32>,
    // Mesh animate_uv: xy = the file's uv direction (times tune.x gives UV
    // units per second), zw = jitter amplitude and speed.
    scroll: vec4<f32>,
    // TXI proceduretype cycle: x = columns, y = rows, z = frames per second.
    cycle: vec4<f32>,
    // TXI proceduretype water: x = distortion amplitude, y = speed.
    water: vec4<f32>,
    // Live tuning (UvTuning): x = scroll scale, y = cycle fps scale,
    // z = water speed scale.
    tune: vec4<f32>,
};

const WATER_PHASE_RATE: f32 = 2.0;

@group(#{MATERIAL_BIND_GROUP}) @binding(0) var<uniform> params: KParams;
@group(#{MATERIAL_BIND_GROUP}) @binding(1) var diffuse_tex: texture_2d<f32>;
@group(#{MATERIAL_BIND_GROUP}) @binding(2) var diffuse_samp: sampler;
@group(#{MATERIAL_BIND_GROUP}) @binding(3) var light_tex: texture_2d<f32>;
@group(#{MATERIAL_BIND_GROUP}) @binding(4) var light_samp: sampler;

// Dynamic shadow of the area's one shadow-casting directional light (see
// shadows.rs): a multiplier on the surface colour, 1.0 where lit. The
// light's colour carries the shadow darkness (area ShadowOpacity). Only
// surfaces turned towards the light take it, so the far side of a placeable
// is not blackened by its own shadow map.
fn shadow_mul(in: VertexOutput) -> f32 {
    if (lights.n_directional_lights == 0u) {
        return 1.0;
    }
#ifdef VERTEX_OUTPUT_INSTANCE_INDEX
    if ((mesh[in.instance_index].flags & MESH_FLAGS_SHADOW_RECEIVER_BIT) == 0u) {
        return 1.0;
    }
#endif
    let light = &lights.directional_lights[0];
    if (((*light).flags & DIRECTIONAL_LIGHT_FLAGS_SHADOWS_ENABLED_BIT) == 0u) {
        return 1.0;
    }
    let n = normalize(in.world_normal);
    let facing = smoothstep(0.0, 0.35, dot(n, (*light).direction_to_light));
    if (facing <= 0.0) {
        return 1.0;
    }
    let view_z = dot(vec4<f32>(
        view.view_from_world[0].z,
        view.view_from_world[1].z,
        view.view_from_world[2].z,
        view.view_from_world[3].z
    ), in.world_position);
    let vis = shadows::fetch_directional_shadow(0u, in.world_position, n, view_z, in.position.xy);
    return 1.0 - (*light).color.r * facing * (1.0 - vis);
}

@fragment
fn fragment(in: VertexOutput) -> @location(0) vec4<f32> {
#ifdef VERTEX_UVS_A
    var uv = in.uv;
    let t = globals.time;
    let ddx = dpdx(in.uv);
    let ddy = dpdy(in.uv);
    if ((params.flags & 32u) != 0u) {
        // Mesh UV scroll, with a small sinusoidal jitter on top.
        uv += fract(params.scroll.xy * params.tune.x * t);
        uv += params.scroll.z * 0.05 * vec2<f32>(sin(t * params.scroll.w * 6.2832), cos(t * params.scroll.w * 5.1));
    }
    if ((params.flags & 128u) != 0u) {
        // Water procedure: a shimmering warp of the texture, evaluated per
        // pixel from a continuous phase (two octaves so it does not look
        // like a single travelling sine).
        let amp = params.water.x * 0.1;
        let sp = params.water.y * WATER_PHASE_RATE * params.tune.z;
        uv += amp * vec2<f32>(sin(uv.y * 6.0 + t * sp), cos(uv.x * 6.0 + t * sp * 0.8));
        uv += amp * 0.35 * vec2<f32>(sin(uv.y * 13.0 - t * sp * 1.3 + 1.7), cos(uv.x * 11.0 + t * sp * 1.1));
    }
    var c: vec4<f32>;
    if ((params.flags & 64u) != 0u) {
        // Cycle procedure: the texture is a grid of frames played in row
        // order at fps. Adjacent frames are cross-faded by the fractional
        // frame so the animation does not step. Each frame is sampled inside
        // its own cell, inset by half a texel of the current mip level so
        // bilinear filtering never reads the neighbouring cell. Gradients
        // come from the unwrapped UVs (no mip seam at the cell edge).
        let n = vec2<f32>(max(params.cycle.x, 1.0), max(params.cycle.y, 1.0));
        let total = n.x * n.y;
        let pos = t * params.cycle.z * params.tune.y;
        let f0 = floor(pos) % total;
        let f1 = (f0 + 1.0) % total;
        let mixw = fract(pos);
        let gx = ddx / n;
        let gy = ddy / n;
        let dims = vec2<f32>(textureDimensions(diffuse_tex));
        let cell_px = dims / n;
        let texels = max(length(gx * dims), length(gy * dims));
        let lod = clamp(log2(max(texels, 1.0)), 0.0, log2(min(cell_px.x, cell_px.y)));
        let inset = min(0.5 * exp2(ceil(lod)) / cell_px, vec2<f32>(0.5));
        let local = clamp(fract(uv), inset, vec2<f32>(1.0) - inset);
        let c0 = textureSampleGrad(diffuse_tex, diffuse_samp, (local + vec2<f32>(f0 % n.x, floor(f0 / n.x))) / n, gx, gy);
        let c1 = textureSampleGrad(diffuse_tex, diffuse_samp, (local + vec2<f32>(f1 % n.x, floor(f1 / n.x))) / n, gx, gy);
        c = mix(c0, c1, mixw) * params.tint;
    } else {
        c = textureSampleGrad(diffuse_tex, diffuse_samp, uv, ddx, ddy) * params.tint;
    }
#else
    var c = params.tint;
#endif
    if ((params.flags & 2u) != 0u && c.a < params.alpha_cut) {
        discard;
    }
    var lit = vec3<f32>(1.0);
#ifdef VERTEX_UVS_B
    if ((params.flags & 1u) != 0u) {
        lit = textureSample(light_tex, light_samp, in.uv_b).rgb;
    }
#endif
    if ((params.flags & 1u) == 0u) {
        let n = normalize(in.world_normal);
        let d = max(dot(n, normalize(vec3<f32>(0.3, 0.8, 0.5))), 0.0);
        lit = vec3<f32>(0.55) + vec3<f32>(0.6) * d;
    }
    lit *= shadow_mul(in);
    if ((params.flags & 4u) != 0u) {
        // Hologram: luminance in projector blue with scanline gaps and a
        // faint flicker. Drawn in the depth-writing (alpha-tested) pass so
        // inner and far-side geometry stays hidden; the gaps give the
        // see-through look instead of blending.
        if (fract(in.position.y / 3.0) < 0.34) {
            discard;
        }
        let l = dot(c.rgb, vec3<f32>(0.3, 0.59, 0.11));
        let scan = 0.8 + 0.2 * step(0.5, fract(in.position.y / 3.0 + 0.5));
        let rim = 1.0 - abs(dot(normalize(in.world_normal), vec3<f32>(0.0, 1.0, 0.0))) * 0.3;
        let col = vec3<f32>(0.35, 0.65, 1.0) * (0.25 + l * 1.1) * scan * rim * params._pad0;
        return vec4<f32>(col, 1.0);
    }
    if ((params.flags & 8u) != 0u) {
        // Stealth field: a dithered see-through body (every other pixel
        // dropped, depth still written), with a band of shimmer running
        // up the figure. _pad1 carries the time.
        let p = floor(in.position.xy);
        let band = sin(in.world_position.y * 4.0 - params._pad1 * 5.0);
        if (fract((p.x + p.y) * 0.5) < 0.25 || (band > 0.85 && fract(p.y * 0.5) < 0.5)) {
            discard;
        }
        let shimmer = 0.15 * max(band, 0.0);
        return vec4<f32>(c.rgb * lit * 0.55 + vec3<f32>(0.12, 0.16, 0.2) + shimmer, 1.0);
    }
    if ((params.flags & 16u) != 0u) {
        // Additive (TXI "blending additive"): Bevy's AlphaMode::Add is
        // premultiplied blending, so the colour is premultiplied here and
        // alpha written as 0; dst survives untouched under black texels.
        return vec4<f32>(c.rgb * lit * c.a, 0.0);
    }
    return vec4<f32>(c.rgb * lit, c.a);
}
"#;

/// Shadow-map fragment shader. Only runs for materials that may discard
/// (alpha mask, blend, additive): cutouts (hair, grates) cast a cut-out
/// shadow, blended and additive surfaces and holograms cast none.
const PREPASS_SRC: &str = r#"
#import bevy_pbr::prepass_io::VertexOutput
#ifdef PREPASS_FRAGMENT
#import bevy_pbr::prepass_io::FragmentOutput
#endif

struct KParams {
    flags: u32,
    alpha_cut: f32,
    _pad0: f32,
    _pad1: f32,
    tint: vec4<f32>,
    scroll: vec4<f32>,
    cycle: vec4<f32>,
    water: vec4<f32>,
    tune: vec4<f32>,
};

@group(#{MATERIAL_BIND_GROUP}) @binding(0) var<uniform> params: KParams;
@group(#{MATERIAL_BIND_GROUP}) @binding(1) var diffuse_tex: texture_2d<f32>;
@group(#{MATERIAL_BIND_GROUP}) @binding(2) var diffuse_samp: sampler;

fn cut(in: VertexOutput) {
    // blend 256, additive 16, hologram 4
    if ((params.flags & 276u) != 0u) {
        discard;
    }
    if ((params.flags & 2u) != 0u) {
        var a = params.tint.a;
#ifdef VERTEX_UVS_A
        // Level 0 keeps the cut stable under the shadow camera's own
        // derivatives; cycle sheets and scrolled textures are left uncut.
        if ((params.flags & 224u) == 0u) {
            a *= textureSampleLevel(diffuse_tex, diffuse_samp, in.uv, 0.0).a;
        }
#endif
        if (a < max(params.alpha_cut, 0.02)) {
            discard;
        }
    }
}

#ifdef PREPASS_FRAGMENT
@fragment
fn fragment(in: VertexOutput) -> FragmentOutput {
    cut(in);
    var out: FragmentOutput;
    return out;
}
#else
@fragment
fn fragment(in: VertexOutput) {
    cut(in);
}
#endif
"#;

pub const FLAG_LIGHTMAP: u32 = 1;
pub const FLAG_ALPHA_MASK: u32 = 2;
/// Hologram projection look (additive blue); `_pad0` carries the intensity.
pub const FLAG_HOLO: u32 = 4;
/// Stealth field look (dithered shimmer); `_pad1` carries the time.
pub const FLAG_STEALTH: u32 = 8;
/// Additive blend (premultiplied output with alpha 0, see the shader).
pub const FLAG_ADDITIVE: u32 = 16;
/// Mesh `animate_uv` scrolling (`KParams::scroll`).
pub const FLAG_UV_SCROLL: u32 = 32;
/// TXI `proceduretype cycle` frame grid (`KParams::cycle`).
pub const FLAG_UV_CYCLE: u32 = 64;
/// TXI `proceduretype water` shimmer (`KParams::water`).
pub const FLAG_UV_WATER: u32 = 128;
/// Alpha-blended material (set on the GPU copy from `KMaterial::blend`): the
/// shadow pass skips it, so glass and fades cast no shadow.
pub const FLAG_BLEND: u32 = 256;

#[derive(Clone, Copy, Debug, ShaderType)]
pub struct KParams {
    pub flags: u32,
    pub alpha_cut: f32,
    pub _pad0: f32,
    pub _pad1: f32,
    pub tint: LinearRgba,
    pub scroll: Vec4,
    pub cycle: Vec4,
    pub water: Vec4,
    pub tune: Vec4,
}

/// Radians per second of the water warp phase per unit of TXI `speed`
/// (mirrors WATER_PHASE_RATE in the shader).
pub const WATER_PHASE_RATE: f32 = 2.0;

/// Global multipliers for animated materials, adjustable at runtime.
/// `scroll` turns the mesh `animate_uv` direction (0.01 to 0.02 in the cage
/// files) into UV units per second (reone uses 250, which looked too fast);
/// `cycle` scales the TXI fps; `water` scales the TXI water speed.
/// Env overrides: K2_UV_SCALE, K2_CYCLE_SCALE, K2_WATER_SCALE. Keys:
/// Numpad 1/2 scroll slower/faster, 4/5 cycle, 7/8 water (x0.8 / x1.25).
#[derive(Resource, Clone, Copy, Debug, PartialEq)]
pub struct UvTuning {
    pub scroll: f32,
    pub cycle: f32,
    pub water: f32,
}

impl UvTuning {
    pub const DEFAULT: Self = Self { scroll: 60.0, cycle: 0.5, water: 1.0 };

    pub fn vec(self) -> Vec4 {
        Vec4::new(self.scroll, self.cycle, self.water, 0.0)
    }

    fn from_env() -> Self {
        let get = |k: &str, d: f32| std::env::var(k).ok().and_then(|v| v.parse::<f32>().ok()).filter(|v| v.is_finite() && *v >= 0.0).unwrap_or(d);
        Self { scroll: get("K2_UV_SCALE", Self::DEFAULT.scroll), cycle: get("K2_CYCLE_SCALE", Self::DEFAULT.cycle), water: get("K2_WATER_SCALE", Self::DEFAULT.water) }
    }

    fn line(self) -> String {
        format!("uv scroll x{:.1}  cycle x{:.2}  water x{:.2}", self.scroll, self.cycle, self.water)
    }
}

impl Default for UvTuning {
    fn default() -> Self {
        Self::from_env()
    }
}

const TUNE_STEP: f32 = 1.25;

/// A floating line showing the tuning values for a couple of seconds.
#[derive(Component)]
struct TuneToast(Timer);

fn tune_keys(mut commands: Commands, keys: Res<ButtonInput<KeyCode>>, mut tuning: ResMut<UvTuning>, mut toast: Query<(Entity, &mut Text, &mut TuneToast)>) {
    let step = |down: KeyCode, up: KeyCode, v: &mut f32| {
        if keys.just_pressed(down) {
            *v /= TUNE_STEP;
            true
        } else if keys.just_pressed(up) {
            *v *= TUNE_STEP;
            true
        } else {
            false
        }
    };
    let mut t = *tuning;
    let mut changed = step(KeyCode::Numpad1, KeyCode::Numpad2, &mut t.scroll);
    changed |= step(KeyCode::Numpad4, KeyCode::Numpad5, &mut t.cycle);
    changed |= step(KeyCode::Numpad7, KeyCode::Numpad8, &mut t.water);
    if !changed {
        return;
    }
    *tuning = t;
    let line = t.line();
    info!("{line}");
    if let Some((_, mut text, mut timer)) = toast.iter_mut().next() {
        text.0 = line;
        timer.0.reset();
    } else {
        commands.spawn((
            Node { position_type: PositionType::Absolute, left: Val::Percent(35.0), bottom: Val::Px(120.0), ..default() },
            Text::new(line),
            TextFont::from_font_size(18.0),
            TextColor(Color::srgb(0.9, 0.9, 0.6)),
            TextShadow::default(),
            GlobalZIndex(60),
            TuneToast(Timer::from_seconds(2.5, TimerMode::Once)),
        ));
    }
}

fn expire_toast(mut commands: Commands, time: Res<Time<Real>>, mut q: Query<(Entity, &mut TuneToast)>) {
    for (e, mut t) in &mut q {
        if t.0.tick(time.delta()).is_finished() {
            commands.entity(e).despawn();
        }
    }
}

/// Pushes the tuning into every animated material that does not carry it yet
/// (new materials start from the defaults).
fn apply_tuning(tuning: Res<UvTuning>, mut materials: ResMut<Assets<KMaterial>>) {
    let want = tuning.vec();
    let stale: Vec<AssetId<KMaterial>> = materials
        .iter()
        .filter(|(_, m)| m.params.flags & (FLAG_UV_SCROLL | FLAG_UV_CYCLE | FLAG_UV_WATER) != 0 && m.params.tune != want)
        .map(|(id, _)| id)
        .collect();
    for id in stale {
        if let Some(mut m) = materials.get_mut(id) {
            m.params.tune = want;
        }
    }
}

impl KParams {
    /// Text of what the shader computes at time `t` (K2_UVDBG=1 prints it for
    /// every animated material): cycle frame index, scroll offset, water phase.
    pub fn describe_at(&self, t: f32) -> String {
        let mut out = format!("t={t:.2}s");
        if self.flags & FLAG_UV_CYCLE != 0 {
            let (cols, rows) = (self.cycle.x.max(1.0), self.cycle.y.max(1.0));
            let fps = self.cycle.z * self.tune.y;
            let frame = (t * fps).floor().rem_euclid(cols * rows);
            out += &format!(" cycle frame {frame} of {} ({fps} fps, {:.2} toward next)", cols * rows, (t * fps).fract());
        }
        if self.flags & FLAG_UV_SCROLL != 0 {
            let rate = self.scroll.truncate().truncate() * self.tune.x;
            let off = (rate * t).to_array();
            out += &format!(" scroll ({:.3}, {:.3}) uv/s ({:.2}, {:.2}) wrapped ({:.3}, {:.3})", rate.x, rate.y, off[0], off[1], off[0].rem_euclid(1.0), off[1].rem_euclid(1.0));
        }
        if self.flags & FLAG_UV_WATER != 0 {
            out += &format!(" water phase {:.2} rad amp {:.3}", t * self.water.y * WATER_PHASE_RATE * self.tune.z, self.water.x * 0.1);
        }
        out
    }

    pub fn new(flags: u32, alpha_cut: f32, tint: LinearRgba) -> Self {
        Self { flags, alpha_cut, _pad0: 0.0, _pad1: 0.0, tint, scroll: Vec4::ZERO, cycle: Vec4::ZERO, water: Vec4::ZERO, tune: UvTuning::default().vec() }
    }
}

#[derive(Asset, TypePath, AsBindGroup, Debug, Clone)]
#[uniform(0, KParams)]
pub struct KMaterial {
    pub params: KParams,
    #[texture(1)]
    #[sampler(2)]
    pub diffuse: Handle<Image>,
    #[texture(3)]
    #[sampler(4)]
    pub lightmap: Handle<Image>,
    pub blend: Blend,
}

/// The uniform is the material's params plus the blend flag, so the shadow
/// shader follows `blend` changes made at runtime (fades, stealth).
impl AsBindGroupShaderType<KParams> for KMaterial {
    fn as_bind_group_shader_type(&self, _images: &RenderAssets<GpuImage>) -> KParams {
        let mut p = self.params;
        if self.blend == Blend::Alpha {
            p.flags |= FLAG_BLEND;
        }
        p
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Blend {
    Opaque,
    Alpha,
    Additive,
}

impl Material for KMaterial {
    fn fragment_shader() -> ShaderRef {
        SHADER.into()
    }

    fn prepass_fragment_shader() -> ShaderRef {
        PREPASS_SHADER.into()
    }

    fn alpha_mode(&self) -> AlphaMode {
        match self.blend {
            Blend::Additive => AlphaMode::Add,
            Blend::Alpha => AlphaMode::Blend,
            Blend::Opaque if self.params.flags & FLAG_ALPHA_MASK != 0 => AlphaMode::Mask(self.params.alpha_cut),
            Blend::Opaque => AlphaMode::Opaque,
        }
    }
}

pub struct KMaterialPlugin;

impl Plugin for KMaterialPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins(MaterialPlugin::<KMaterial>::default())
            .init_resource::<UvTuning>()
            .add_systems(Update, (tune_keys, expire_toast, apply_tuning).chain());
        let _ = app
            .world_mut()
            .resource_mut::<Assets<Shader>>()
            .insert(SHADER.id(), Shader::from_wgsl(SHADER_SRC, "kotor2://kmaterial.wgsl"));
        let _ = app
            .world_mut()
            .resource_mut::<Assets<Shader>>()
            .insert(PREPASS_SHADER.id(), Shader::from_wgsl(PREPASS_SRC, "kotor2://kmaterial_prepass.wgsl"));
    }
}

#[cfg(test)]
mod tests;

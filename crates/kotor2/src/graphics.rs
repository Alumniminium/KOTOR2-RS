//! Graphics settings: the `GraphicsSettings` resource behind the options
//! screens (gui/options.rs), its settings file, and the systems that apply it
//! to the window and the 3D camera.
//!
//! The file is `settings.json` next to the saves folder
//! (`~/.local/share/kotor2-rs/`), or in `K2_SETTINGS_DIR` when set. Missing
//! fields and bad values fall back to the defaults one by one. `--screenshot`
//! runs neither read nor write the player's file unless `K2_SETTINGS_DIR` is
//! set. The window mode, size and vsync are read before the window is
//! created (main.rs); everything else is applied live when the resource
//! changes.

use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, Ordering};

use bevy::light::ShadowFilteringMethod;
use bevy::prelude::*;
use bevy::render::view::{ColorGrading, Msaa};
use bevy::window::{MonitorSelection, PresentMode, PrimaryWindow, WindowMode};
use serde::{Deserialize, Serialize};

use crate::model::AssetCache;

/// Shadow quality; the renderer reads this (`GraphicsSettings::shadows`).
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default, Serialize, Deserialize)]
pub enum ShadowQuality {
    Off,
    Low,
    #[default]
    High,
}

impl ShadowQuality {
    /// The Shadows toggle in the options screen goes Off <-> High; Low is
    /// reachable from the settings file.
    pub fn is_on(self) -> bool {
        self != ShadowQuality::Off
    }
}

pub const MSAA_STEPS: [u32; 4] = [1, 2, 4, 8];
pub const ANISO_STEPS: [u32; 5] = [1, 2, 4, 8, 16];
/// Texture quality names, index = mip levels dropped from the top.
pub const TEXTURE_QUALITY: [&str; 3] = ["High", "Medium", "Low"];

pub const MIN_WIDTH: u32 = 640;
pub const MIN_HEIGHT: u32 = 480;

#[derive(Resource, Clone, PartialEq, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct GraphicsSettings {
    pub shadows: ShadowQuality,
    /// Soft shadows: Gaussian shadow filtering (off: hardware 2x2).
    pub soft_shadows: bool,
    /// Stored for the grass pass (there is none yet).
    pub grass: bool,
    /// Stored for the force speed screen effect (none yet).
    pub force_speed_fx: bool,
    /// Frame buffer effects (screen-space distortion and glow passes).
    pub frame_buffer_fx: bool,
    pub vsync: bool,
    /// Borderless fullscreen on the current monitor.
    pub fullscreen: bool,
    /// Window size when not fullscreen (logical px).
    pub width: u32,
    pub height: u32,
    /// MSAA samples: 1, 2, 4 or 8.
    pub msaa: u32,
    /// Anisotropic filtering: 1, 2, 4, 8 or 16.
    pub anisotropy: u32,
    /// Index into TEXTURE_QUALITY: mip levels dropped from the top of new
    /// textures.
    pub texture_quality: u32,
    /// 0..100 slider, 50 is neutral.
    pub gamma: u32,
}

impl Default for GraphicsSettings {
    fn default() -> Self {
        Self {
            shadows: ShadowQuality::High,
            soft_shadows: true,
            grass: true,
            force_speed_fx: true,
            frame_buffer_fx: true,
            vsync: true,
            fullscreen: false,
            width: 1600,
            height: 900,
            msaa: 4,
            anisotropy: 16,
            texture_quality: 0,
            gamma: 50,
        }
    }
}

fn nearest(steps: &[u32], v: u32) -> u32 {
    steps.iter().copied().min_by_key(|s| s.abs_diff(v)).unwrap_or(v)
}

impl GraphicsSettings {
    /// Snaps every field into its valid range.
    pub fn sanitize(&mut self) {
        self.msaa = nearest(&MSAA_STEPS, self.msaa);
        self.anisotropy = nearest(&ANISO_STEPS, self.anisotropy);
        self.texture_quality = self.texture_quality.min(TEXTURE_QUALITY.len() as u32 - 1);
        self.gamma = self.gamma.min(100);
        self.width = self.width.clamp(MIN_WIDTH, 16384);
        self.height = self.height.clamp(MIN_HEIGHT, 16384);
    }

    /// Parses a settings file; an unreadable file gives the defaults and a
    /// bad or missing field keeps its default.
    pub fn from_json(text: &str) -> Self {
        let mut cur = Self::default();
        let Ok(serde_json::Value::Object(file)) = serde_json::from_str::<serde_json::Value>(text) else {
            warn!("graphics: settings file is not a JSON object, using defaults");
            return cur;
        };
        for (k, v) in file {
            let Ok(serde_json::Value::Object(mut obj)) = serde_json::to_value(&cur) else { continue };
            obj.insert(k.clone(), v);
            match serde_json::from_value::<Self>(serde_json::Value::Object(obj)) {
                Ok(n) => cur = n,
                Err(e) => warn!("graphics: ignoring settings field {k}: {e}"),
            }
        }
        cur.sanitize();
        cur
    }

    pub fn to_json(&self) -> String {
        serde_json::to_string_pretty(self).unwrap_or_default() + "\n"
    }

    /// The settings file, or None for test runs that must not touch it.
    pub fn path() -> Option<PathBuf> {
        if let Some(d) = std::env::var_os("K2_SETTINGS_DIR").filter(|v| !v.is_empty()) {
            return Some(PathBuf::from(d).join("settings.json"));
        }
        if std::env::args().any(|a| a == "--screenshot") {
            return None;
        }
        k2_world::save::default_save_root().parent().map(|p| p.join("settings.json"))
    }

    pub fn load() -> Self {
        let s = Self::read();
        publish(&s);
        s
    }

    fn read() -> Self {
        let Some(path) = Self::path() else { return Self::default() };
        std::fs::read_to_string(&path).map(|t| Self::from_json(&t)).unwrap_or_default()
    }

    /// Writes the file atomically; failures are logged.
    pub fn save(&self) {
        let Some(path) = Self::path() else { return };
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let tmp = path.with_extension("json.tmp");
        match std::fs::write(&tmp, self.to_json()).and_then(|_| std::fs::rename(&tmp, &path)) {
            Ok(()) => info!("graphics: saved {}", path.display()),
            Err(e) => warn!("graphics: cannot write {}: {e}", path.display()),
        }
    }

    pub fn present_mode(&self) -> PresentMode {
        if self.vsync { PresentMode::AutoVsync } else { PresentMode::AutoNoVsync }
    }

    pub fn window_mode(&self) -> WindowMode {
        if self.fullscreen { WindowMode::BorderlessFullscreen(MonitorSelection::Current) } else { WindowMode::Windowed }
    }

    pub fn msaa_mode(&self) -> Msaa {
        Msaa::from_samples(self.msaa)
    }

    /// Mip levels dropped from the top of newly loaded textures.
    pub fn mips_dropped(&self) -> u32 {
        self.texture_quality
    }

    /// Slider 0..100 to the colour-grading gamma exponent: 50 is 1.0,
    /// 0 is about 0.62 (darker), 100 about 1.62 (brighter).
    pub fn gamma_value(&self) -> f32 {
        2f32.powf((self.gamma as f32 - 50.0) / 50.0 * 0.7)
    }

    pub fn shadow_filtering(&self) -> ShadowFilteringMethod {
        if self.soft_shadows { ShadowFilteringMethod::Gaussian } else { ShadowFilteringMethod::Hardware2x2 }
    }

    /// The camera colour grading for the gamma slider.
    pub fn color_grading(&self) -> ColorGrading {
        let g = self.gamma_value();
        let mut c = ColorGrading::default();
        for s in [&mut c.shadows, &mut c.midtones, &mut c.highlights] {
            s.gamma = g;
        }
        c
    }
}

// Texture sampler and mip settings read by the texture loader (model.rs),
// which has no access to the ECS resource.
static ANISO: AtomicU32 = AtomicU32::new(16);
static MIPS_DROPPED: AtomicU32 = AtomicU32::new(0);

/// Anisotropy clamp for new textures.
pub fn anisotropy() -> u16 {
    ANISO.load(Ordering::Relaxed) as u16
}

/// Mip levels to drop from the top of a new texture of this size (never
/// below 64 px on the long side, never the last level).
pub fn mips_to_drop(width: u32, height: u32, mips: u32) -> u32 {
    drop_for(MIPS_DROPPED.load(Ordering::Relaxed), width, height, mips)
}

fn drop_for(want: u32, width: u32, height: u32, mips: u32) -> u32 {
    let mut drop = 0;
    while drop < want && drop + 1 < mips && (width >> (drop + 1)).max(height >> (drop + 1)) >= 64 {
        drop += 1;
    }
    drop
}

/// Texture quality: drops the top mip levels of a decoded texture (procedure
/// textures are left alone, their frame cells depend on the size).
pub fn apply_texture_quality(d: &mut crate::tpc::Decoded) {
    let drop = mips_to_drop(d.width, d.height, d.mips);
    if drop == 0 {
        return;
    }
    let (mut w, mut h, mut off) = (d.width as usize, d.height as usize, 0usize);
    for _ in 0..drop {
        off += w * h * 4;
        w = (w / 2).max(1);
        h = (h / 2).max(1);
    }
    d.data.drain(..off);
    d.width = w as u32;
    d.height = h as u32;
    d.mips -= drop;
}

pub struct GraphicsPlugin;

impl Plugin for GraphicsPlugin {
    fn build(&self, app: &mut App) {
        // main.rs inserts the loaded settings before the window exists.
        app.init_resource::<GraphicsSettings>().add_systems(Startup, log_loaded).add_systems(Update, (apply_window, apply_cameras, apply_textures));
    }
}

fn publish(s: &GraphicsSettings) {
    ANISO.store(s.anisotropy, Ordering::Relaxed);
    MIPS_DROPPED.store(s.mips_dropped(), Ordering::Relaxed);
}

fn log_loaded(s: Res<GraphicsSettings>) {
    let file = GraphicsSettings::path().map_or("none".to_owned(), |p| p.display().to_string());
    info!(
        "graphics: settings from {file}: {}x{} fullscreen {} vsync {} msaa {} aniso {} texture {} gamma {} shadows {:?} soft {}",
        s.width, s.height, s.fullscreen, s.vsync, s.msaa, s.anisotropy, TEXTURE_QUALITY[s.texture_quality as usize], s.gamma, s.shadows, s.soft_shadows
    );
}

/// Window mode, size and present mode follow the settings. Only fields that
/// differ are written, so a user-resized window is left alone until the
/// settings change.
fn apply_window(s: Res<GraphicsSettings>, mut windows: Query<&mut Window, With<PrimaryWindow>>, mut last: Local<Option<GraphicsSettings>>) {
    if !s.is_changed() {
        return;
    }
    let Ok(mut w) = windows.single_mut() else { return };
    let prev = last.replace(s.clone());
    if prev.as_ref().is_none_or(|p| p.vsync != s.vsync) && w.present_mode != s.present_mode() {
        info!("graphics: present mode {:?}", s.present_mode());
        w.present_mode = s.present_mode();
    }
    if prev.as_ref().is_none_or(|p| p.fullscreen != s.fullscreen) && w.mode != s.window_mode() {
        info!("graphics: window mode {:?}", s.window_mode());
        w.mode = s.window_mode();
    }
    if !s.fullscreen && prev.as_ref().is_none_or(|p| (p.width, p.height, p.fullscreen) != (s.width, s.height, s.fullscreen)) && (w.width() as u32, w.height() as u32) != (s.width, s.height) {
        info!("graphics: window size {}x{}", s.width, s.height);
        w.resolution.set(s.width as f32, s.height as f32);
    }
}

/// MSAA, shadow filtering and gamma on every 3D camera, now and for cameras
/// spawned later.
fn apply_cameras(s: Res<GraphicsSettings>, mut commands: Commands, cams: Query<Entity, With<Camera3d>>, new: Query<(), Added<Camera3d>>, mut logged: Local<bool>) {
    if !s.is_changed() && new.is_empty() {
        return;
    }
    for e in &cams {
        commands.entity(e).insert((s.msaa_mode(), s.shadow_filtering(), s.color_grading()));
    }
    if !cams.is_empty() && !*logged || s.is_changed() && !s.is_added() {
        *logged = true;
        info!("graphics: cameras msaa {}x, {:?}, gamma {:.2} (slider {})", s.msaa, s.shadow_filtering(), s.gamma_value(), s.gamma);
    }
}

/// Refreshes the shared statics and re-samples the textures already loaded
/// (new textures read the statics when they are built).
fn apply_textures(s: Res<GraphicsSettings>, cache: Res<AssetCache>, mut images: ResMut<Assets<Image>>, mut last: Local<Option<(u32, u32)>>) {
    if !s.is_changed() {
        return;
    }
    publish(&s);
    let now = (s.anisotropy, s.texture_quality);
    if last.replace(now).is_none_or(|p| p.0 == now.0) {
        return;
    }
    let mut n = 0;
    for h in cache.texture_images() {
        if let Some(mut img) = images.get_mut(&h)
            && let bevy::image::ImageSampler::Descriptor(d) = &mut img.sampler
        {
            d.anisotropy_clamp = s.anisotropy as u16;
            n += 1;
        }
    }
    info!("graphics: anisotropy {}x on {n} loaded textures", s.anisotropy);
}

#[cfg(test)]
mod tests;

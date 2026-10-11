//! MDL animations converted to sampled tracks keyed by node name.
//!
//! Binary MDL animation data, as read by k2-mdl:
//! - Position keys are offsets from the animated node's rest position, scaled
//!   by the animated model's `animation_scale` (kotorblender agrees).
//! - Orientation keys are (x, y, z, w) quaternions, or one packed u32 per key
//!   (raw column count 2) holding a compressed quaternion.
//! - Bezier controllers (column flag 0x10) carry 3 points per key; the first
//!   is the value, the tangents are ignored here.
//! - Mesh alpha (controller 132) is a single float per key, 1 = opaque. It
//!   fades force cages, console screens and holograms in and out.

use std::collections::HashMap;
use std::sync::Arc;

use bevy::prelude::*;
use k2_mdl::{MdlAnimNode, MdlAnimation, MdlController, MdlControllerType};

#[derive(Default, Debug)]
pub struct Track {
    pub pos: Vec<(f32, Vec3)>,
    pub rot: Vec<(f32, Quat)>,
    pub scale: Vec<(f32, f32)>,
    /// Mesh node alpha, 0 (clear) to 1.
    pub alpha: Vec<(f32, f32)>,
    /// Emitter birthrate (controller 88, keyed on emitter nodes).
    pub birthrate: Vec<(f32, f32)>,
}

#[derive(Debug)]
#[allow(dead_code)] // transition and events are for gameplay (footsteps, hits)
pub struct Clip {
    pub name: String,
    pub length: f32,
    pub transition: f32,
    pub events: Vec<(f32, String)>,
    /// Lowercased node name -> track.
    pub tracks: HashMap<String, Track>,
}

pub fn convert_all(anims: &[MdlAnimation]) -> HashMap<String, Arc<Clip>> {
    anims.iter().map(|a| (a.name.to_ascii_lowercase(), Arc::new(convert(a)))).collect()
}

fn convert(a: &MdlAnimation) -> Clip {
    let mut tracks = HashMap::new();
    // Zero-length clips are static poses (the swoop HUD digit clips move a
    // digit mesh 92 units into view); the junk-key filter is for real clips.
    collect(&a.root_node, &mut tracks, a.length > 0.0);
    Clip {
        name: a.name.to_ascii_lowercase(),
        length: a.length.max(0.0),
        transition: a.transition_time.max(0.0),
        events: a.events.iter().map(|e| (e.time, e.name.to_ascii_lowercase())).collect(),
        tracks,
    }
}

fn collect(n: &MdlAnimNode, out: &mut HashMap<String, Track>, filter_junk: bool) {
    let mut t = Track::default();
    for c in &n.controllers {
        read_controller(c, &mut t);
    }
    // Stunt clips key cutscenedummy in area coordinates, so it is exempt.
    if filter_junk && !n.name.eq_ignore_ascii_case("cutscenedummy") && t.pos.iter().any(|(_, p)| p.abs().max_element() > MAX_POS_KEY) {
        t.pos.clear();
    }
    if !(t.pos.is_empty() && t.rot.is_empty() && t.scale.is_empty() && t.alpha.is_empty() && t.birthrate.is_empty()) {
        out.insert(n.name.to_ascii_lowercase(), t);
    }
    for c in &n.children {
        collect(c, out, filter_junk);
    }
}

/// Largest believable position key (game units, before animation_scale).
/// Some clips carry junk position keys: c_drdassassin cdie/cdead put Box01,
/// the parent of the whole upper body, at (474, 97, -74), which flings it
/// away. No creature moves 20 m inside a clip, so such a track is dropped
/// and the node keeps its rest position.
const MAX_POS_KEY: f32 = 20.0;

fn read_controller(c: &MdlController, t: &mut Track) {
    let ty = c.controller_type;
    if ty == MdlControllerType::POSITION {
        t.pos = c.keys.iter().filter(|k| k.values.len() >= 3).map(|k| (k.time, Vec3::new(k.values[0], k.values[1], k.values[2]))).collect();
    } else if ty == MdlControllerType::ORIENTATION {
        t.rot = c
            .keys
            .iter()
            .filter_map(|k| {
                let q = match k.values.len() {
                    1 => decompress_quat(k.values[0].to_bits()),
                    n if n >= 4 => Quat::from_xyzw(k.values[0], k.values[1], k.values[2], k.values[3]),
                    _ => return None,
                };
                let q = if q.length_squared() > 1e-8 { q.normalize() } else { Quat::IDENTITY };
                Some((k.time, q))
            })
            .collect();
    } else if ty == MdlControllerType::SCALE {
        t.scale = c.keys.iter().filter(|k| !k.values.is_empty()).map(|k| (k.time, k.values[0])).collect();
    } else if ty == MdlControllerType::ALPHA {
        t.alpha = c.keys.iter().filter(|k| !k.values.is_empty()).map(|k| (k.time, k.values[0].clamp(0.0, 1.0))).collect();
    } else if ty == MdlControllerType::BIRTHRATE {
        t.birthrate = c.keys.iter().filter(|k| !k.values.is_empty()).map(|k| (k.time, k.values[0].max(0.0))).collect();
    }
}

/// Packed quaternion: x and y in 11 bits each, z in 10 bits, w implied.
pub fn decompress_quat(c: u32) -> Quat {
    let x = (c & 0x7FF) as f32 / 1023.0 - 1.0;
    let y = ((c >> 11) & 0x7FF) as f32 / 1023.0 - 1.0;
    let z = (c >> 22) as f32 / 511.0 - 1.0;
    let m2 = x * x + y * y + z * z;
    let w = if m2 < 1.0 { (1.0 - m2).sqrt() } else { 0.0 };
    Quat::from_xyzw(x, y, z, w)
}

fn bracket<T>(keys: &[(f32, T)], time: f32) -> (usize, usize, f32) {
    let n = keys.len();
    if n == 1 || time <= keys[0].0 {
        return (0, 0, 0.0);
    }
    if time >= keys[n - 1].0 {
        return (n - 1, n - 1, 0.0);
    }
    let i = keys.partition_point(|k| k.0 <= time) - 1;
    let (t0, t1) = (keys[i].0, keys[i + 1].0);
    let f = if t1 > t0 { (time - t0) / (t1 - t0) } else { 0.0 };
    (i, i + 1, f)
}

impl Track {
    pub fn sample_pos(&self, time: f32) -> Option<Vec3> {
        if self.pos.is_empty() {
            return None;
        }
        let (a, b, f) = bracket(&self.pos, time);
        Some(self.pos[a].1.lerp(self.pos[b].1, f))
    }

    pub fn sample_rot(&self, time: f32) -> Option<Quat> {
        if self.rot.is_empty() {
            return None;
        }
        let (a, b, f) = bracket(&self.rot, time);
        Some(self.rot[a].1.slerp(self.rot[b].1, f))
    }

    pub fn sample_scale(&self, time: f32) -> Option<f32> {
        if self.scale.is_empty() {
            return None;
        }
        let (a, b, f) = bracket(&self.scale, time);
        Some(self.scale[a].1 + (self.scale[b].1 - self.scale[a].1) * f)
    }

    pub fn sample_birthrate(&self, time: f32) -> Option<f32> {
        if self.birthrate.is_empty() {
            return None;
        }
        let (a, b, f) = bracket(&self.birthrate, time);
        Some(self.birthrate[a].1 + (self.birthrate[b].1 - self.birthrate[a].1) * f)
    }

    pub fn sample_alpha(&self, time: f32) -> Option<f32> {
        if self.alpha.is_empty() {
            return None;
        }
        let (a, b, f) = bracket(&self.alpha, time);
        Some(self.alpha[a].1 + (self.alpha[b].1 - self.alpha[a].1) * f)
    }
}

#[cfg(test)]
mod tests;

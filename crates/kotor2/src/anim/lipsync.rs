//! Lip sync: LIP keyframes drive the head's face nodes.
//!
//! LIP V1.0: "LIP V1.0", f32 length, u32 count, then `count` x (f32 time,
//! u8 shape). Shapes are the 16 visemes (0 EE, 1 EH, 2 schwa, 3 AH, 4 OH,
//! 5 OOH, 6 Y, 7 S, 8 FV, 9 NG, 10 TH, 11 MPB, 12 TD, 13 SH, 14 L, 15 KG).
//! Files live in lips/<module>_loc.mod, named like the VO resref.
//!
//! Heads have no per-viseme clips: the humanoid supermodel (S_Male02)
//! carries one `talk` animation, 0.5 s long, rooted at `talkdummy`, with 16
//! keys per face node (f_jaw_g, f_um_g, lip corners, tongue). Key `s` is the
//! pose for shape `s`. We sample `talk` at shape times and blend between
//! consecutive LIP keys, after the body animation has posed the creature.
//! Lines without a LIP file get a synthetic track from the text.

use std::collections::HashMap;
use std::sync::Arc;

use bevy::prelude::*;
use bevy::transform::TransformSystems;
use k2_res::Resources;
use rakata_formats::erf::ErfIndex;

use super::{AnimBindings, AnimPlayer, animate};

pub struct LipSyncPlugin;

impl Plugin for LipSyncPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<LipIndex>().add_systems(PostUpdate, apply_lips.after(animate).before(TransformSystems::Propagate));
    }
}

#[derive(Debug, Clone)]
pub struct LipData {
    pub length: f32,
    pub keys: Vec<(f32, u8)>,
}

pub fn parse_lip(b: &[u8]) -> Option<LipData> {
    if b.len() < 16 || &b[..8] != b"LIP V1.0" {
        return None;
    }
    let length = f32::from_le_bytes(b[8..12].try_into().ok()?);
    let count = u32::from_le_bytes(b[12..16].try_into().ok()?) as usize;
    let keys: Vec<(f32, u8)> = b[16..]
        .as_chunks::<5>()
        .0
        .iter()
        .take(count)
        .map(|c| (f32::from_le_bytes([c[0], c[1], c[2], c[3]]), c[4].min(15)))
        .collect();
    (!keys.is_empty()).then_some(LipData { length, keys })
}

/// Viseme track from text when there is no LIP file: one shape per letter
/// group, about 11 per second, closed mouth at word gaps.
pub fn synthetic_lip(text: &str, seconds: f32) -> LipData {
    let letters: Vec<char> = text.chars().filter(|c| c.is_ascii_alphabetic() || *c == ' ').collect();
    let step = 0.09f32;
    let n = ((seconds / step) as usize).max(1);
    let mut keys = Vec::with_capacity(n + 1);
    for i in 0..n {
        let c = letters.get(i * letters.len().max(1) / n).copied().unwrap_or(' ').to_ascii_lowercase();
        let shape = match c {
            'a' => 3,
            'e' => 1,
            'i' | 'y' => 0,
            'o' => 4,
            'u' | 'w' => 5,
            'm' | 'b' | 'p' | ' ' => 11,
            'f' | 'v' => 8,
            's' | 'z' | 'c' | 'x' => 7,
            't' | 'd' => 12,
            'l' | 'r' => 14,
            'k' | 'g' | 'q' => 15,
            'n' => 9,
            'h' => 2,
            'j' => 13,
            _ => 2,
        };
        keys.push((i as f32 * step, shape));
    }
    keys.push((n as f32 * step, 11));
    LipData { length: n as f32 * step, keys }
}

/// Every LIP in the install: lips/*.mod capsules plus chitin.
#[derive(Resource, Default)]
pub struct LipIndex {
    built: bool,
    caps: Vec<ErfIndex>,
    map: HashMap<String, (usize, usize)>,
}

const LIP_TYPE: u16 = 3004;

impl LipIndex {
    fn build(&mut self, res: &Resources) {
        self.built = true;
        for path in res.install.files("lips", &["mod", "erf"]) {
            let Ok(idx) = ErfIndex::open(&path) else {
                warn!("lipsync: cannot open {}", path.display());
                continue;
            };
            let ci = self.caps.len();
            for (pos, e) in idx.entries().iter().enumerate() {
                if e.resource_type.raw_id() == LIP_TYPE {
                    self.map.entry(e.resref.to_string().to_ascii_lowercase()).or_insert((ci, pos));
                }
            }
            self.caps.push(idx);
        }
        info!("lipsync: {} LIP files in {} capsules", self.map.len(), self.caps.len());
    }

    pub fn get(&mut self, res: &Resources, resref: &str) -> Option<LipData> {
        if !self.built {
            self.build(res);
        }
        let key = resref.to_ascii_lowercase();
        let bytes = match self.map.get(&key) {
            Some(&(c, p)) => self.caps[c].read_entry(p).ok(),
            None => res.get(&k2_res::ResKey { name: key, ty: LIP_TYPE }).ok().flatten(),
        }?;
        parse_lip(&bytes)
    }
}

/// Active lip track on a creature body (the entity with `AnimPlayer`).
#[derive(Component)]
pub struct LipSync {
    pub data: Arc<LipData>,
    pub t: f32,
    /// Logged viseme changes (debug).
    last_shape: Option<u8>,
}

impl LipSync {
    pub fn new(data: LipData) -> Self {
        Self { data: Arc::new(data), t: 0.0, last_shape: None }
    }
}

fn apply_lips(
    mut commands: Commands,
    time: Res<Time>,
    mut q: Query<(Entity, &mut LipSync, &AnimPlayer, &AnimBindings)>,
    mut transforms: Query<&mut Transform>,
) {
    let dt = time.delta_secs();
    for (e, mut lip, player, bindings) in &mut q {
        lip.t += dt;
        let t = lip.t;
        if t > lip.data.length + 0.15 {
            commands.entity(e).remove::<LipSync>();
            continue;
        }
        let Some(talk) = player.set.clip("talk") else { continue };
        let keys = &lip.data.keys;
        let i = keys.partition_point(|k| k.0 <= t).saturating_sub(1);
        let (t0, s0) = keys[i];
        let (t1, s1) = keys.get(i + 1).copied().unwrap_or((t0, s0));
        let f = if t1 > t0 { ((t - t0) / (t1 - t0)).clamp(0.0, 1.0) } else { 0.0 };
        if lip.last_shape != Some(s0) {
            debug!("lipsync: {e} t {t:.2} viseme {s0}");
            lip.last_shape = Some(s0);
        }
        // Shape s is key s of the 16-key talk clip.
        let frame = talk.length / 15.0;
        let (ta, tb) = (s0 as f32 * frame, s1 as f32 * frame);
        for b in &bindings.nodes {
            let Some(tr) = talk.tracks.get(&b.key) else { continue };
            let Ok(mut tf) = transforms.get_mut(b.entity) else { continue };
            if let (Some(a), Some(c)) = (tr.sample_pos(ta), tr.sample_pos(tb)) {
                tf.translation = b.rest_t + a.lerp(c, f) * bindings.anim_scale;
            }
            if let (Some(a), Some(c)) = (tr.sample_rot(ta), tr.sample_rot(tb)) {
                tf.rotation = a.slerp(c, f);
            }
        }
    }
}

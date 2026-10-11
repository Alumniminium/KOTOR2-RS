//! Game audio: format sniffing, area music and ambience, positional sound
//! objects, and voice-over playback for dialog.
//!
//! KotOR2 ships three kinds of ".wav":
//! - streamvoice/ and streammusic/: a 58-byte RIFF header claiming PCM, with
//!   MP3 frames from 0x3A on.
//! - streamsounds/ (and many SFX): a 470-byte junk block (starts like an MP3
//!   frame) followed by a real RIFF WAV.
//! - plain WAV (PCM, or IMA ADPCM, which we expand to PCM ourselves).
//!
//! `sniff` turns any of them into bytes Bevy/rodio can decode, plus a
//! duration computed from the stream itself.
//!
//!   kotor2 ... [--mute] [--volume F] [--audio-test MODULE] [--audio-survey]

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use bevy::audio::{Decodable, PlaybackMode, SpatialScale, Volume};
use bevy::ecs::system::SystemParam;
use bevy::prelude::*;
use k2_res::{Install, ResKey, Resources};
use k2_script::ObjectId;
use k2_world::WorldEvent;
use rakata_core::ResourceType;

use crate::area::{AreaModule, GameData};
use crate::game::{ApplyEvents, AreaRoot, FrameEvents, GameSet, WorldRes};
use crate::messages::AreaLoaded;
use crate::model::GameRes;

/// Flags that take a value; parsed here from the process args.
pub const FLAGS: &[&str] = &["--volume", "--audio-test"];
/// Flags without a value.
pub const SWITCHES: &[&str] = &["--mute", "--audio-survey"];

fn arg_value(name: &str) -> Option<String> {
    let argv: Vec<String> = std::env::args().collect();
    argv.iter().position(|a| a == name).and_then(|i| argv.get(i + 1).cloned())
}

fn has_switch(name: &str) -> bool {
    std::env::args().any(|a| a == name)
}

// ---------------------------------------------------------------------------
// Sniffing and decoding
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Format {
    Mp3,
    Wav,
}

/// Playable bytes plus what we learned about them.
#[derive(Debug, Clone)]
pub struct Sniffed {
    pub bytes: Arc<[u8]>,
    pub format: Format,
    pub duration: Duration,
    /// Which container variant it was, for logs.
    pub variant: &'static str,
}

/// Offset of the MP3 payload behind the fake RIFF header on stream files.
const FAKE_RIFF_MP3_OFFSET: usize = 0x3A;
/// Size of the junk block in front of the real RIFF on SFX files.
const SFX_JUNK_HEADER: usize = 470;

pub fn sniff(raw: &[u8]) -> Result<Sniffed, String> {
    let mut b = raw;
    let mut junk = false;
    if !b.starts_with(b"RIFF") && b.len() > SFX_JUNK_HEADER + 12 && &b[SFX_JUNK_HEADER..SFX_JUNK_HEADER + 4] == b"RIFF" {
        b = &b[SFX_JUNK_HEADER..];
        junk = true;
    }
    if b.len() >= 12 && b.starts_with(b"RIFF") && &b[8..12] == b"WAVE" {
        if b.len() > FAKE_RIFF_MP3_OFFSET + 4 && mp3_chain_ok(&b[FAKE_RIFF_MP3_OFFSET..], 3) {
            return mp3(&b[FAKE_RIFF_MP3_OFFSET..], "riff+mp3");
        }
        return wav(b, if junk { "junk470+wav" } else { "wav" });
    }
    let mut m = b;
    if m.starts_with(b"ID3") && m.len() > 10 {
        let size = ((m[6] as usize & 0x7f) << 21) | ((m[7] as usize & 0x7f) << 14) | ((m[8] as usize & 0x7f) << 7) | (m[9] as usize & 0x7f);
        m = &m[(10 + size).min(m.len())..];
    }
    if mp3_chain_ok(m, 3) {
        return mp3(m, "mp3");
    }
    // Last resort: find a frame chain in the first 8 KiB.
    if let Some(off) = (0..m.len().min(8192)).find(|&i| mp3_chain_ok(&m[i..], 4)) {
        return mp3(&m[off..], "mp3+skip");
    }
    Err(format!("unrecognised audio ({} bytes, starts {:02x?})", raw.len(), &raw[..raw.len().min(8)]))
}

fn mp3(b: &[u8], variant: &'static str) -> Result<Sniffed, String> {
    let (frames, duration) = mp3_scan(b);
    if frames == 0 {
        return Err("no mp3 frames".into());
    }
    Ok(Sniffed { bytes: b.into(), format: Format::Mp3, duration, variant })
}

/// (frame length in bytes, samples per frame, sample rate)
fn mp3_frame(h: &[u8]) -> Option<(usize, u32, u32)> {
    if h.len() < 4 || h[0] != 0xFF || h[1] & 0xE0 != 0xE0 {
        return None;
    }
    let version = (h[1] >> 3) & 3; // 0 = 2.5, 1 = reserved, 2 = v2, 3 = v1
    let layer = (h[1] >> 1) & 3; // 1 = III, 2 = II, 3 = I
    if version == 1 || layer == 0 {
        return None;
    }
    let br_idx = (h[2] >> 4) as usize;
    let sr_idx = ((h[2] >> 2) & 3) as usize;
    if br_idx == 0 || br_idx == 15 || sr_idx == 3 {
        return None;
    }
    let pad = ((h[2] >> 1) & 1) as usize;
    const V1_L1: [u32; 15] = [0, 32, 64, 96, 128, 160, 192, 224, 256, 288, 320, 352, 384, 416, 448];
    const V1_L2: [u32; 15] = [0, 32, 48, 56, 64, 80, 96, 112, 128, 160, 192, 224, 256, 320, 384];
    const V1_L3: [u32; 15] = [0, 32, 40, 48, 56, 64, 80, 96, 112, 128, 160, 192, 224, 256, 320];
    const V2_L1: [u32; 15] = [0, 32, 48, 56, 64, 80, 96, 112, 128, 144, 160, 176, 192, 224, 256];
    const V2_L23: [u32; 15] = [0, 8, 16, 24, 32, 40, 48, 56, 64, 80, 96, 112, 128, 144, 160];
    const SR: [[u32; 3]; 4] = [[11025, 12000, 8000], [0, 0, 0], [22050, 24000, 16000], [44100, 48000, 32000]];
    let v1 = version == 3;
    let kbps = match (v1, layer) {
        (true, 3) => V1_L1[br_idx],
        (true, 2) => V1_L2[br_idx],
        (true, _) => V1_L3[br_idx],
        (false, 3) => V2_L1[br_idx],
        (false, _) => V2_L23[br_idx],
    } as usize;
    let rate = SR[version as usize][sr_idx];
    let (len, samples) = match layer {
        3 => ((12 * kbps * 1000 / rate as usize + pad) * 4, 384),
        2 => (144 * kbps * 1000 / rate as usize + pad, 1152),
        _ if v1 => (144 * kbps * 1000 / rate as usize + pad, 1152),
        _ => (72 * kbps * 1000 / rate as usize + pad, 576),
    };
    (len >= 4).then_some((len, samples, rate))
}

fn mp3_chain_ok(b: &[u8], frames: usize) -> bool {
    let mut off = 0;
    for _ in 0..frames {
        let Some((len, _, _)) = mp3_frame(&b[off..]) else { return false };
        off += len;
        if off >= b.len() {
            // A short clip may end inside the chain; what was there was valid.
            return true;
        }
    }
    true
}

/// Counts frames and total play time, resyncing over junk between frames.
fn mp3_scan(b: &[u8]) -> (usize, Duration) {
    let (mut off, mut frames, mut secs) = (0usize, 0usize, 0f64);
    while off + 4 <= b.len() {
        match mp3_frame(&b[off..]) {
            Some((len, samples, rate)) => {
                frames += 1;
                secs += samples as f64 / rate as f64;
                off += len;
            }
            None => match (off + 1..b.len().saturating_sub(4)).find(|&i| mp3_chain_ok(&b[i..], 2)) {
                Some(next) => off = next,
                None => break,
            },
        }
    }
    (frames, Duration::from_secs_f64(secs))
}

struct WavFmt {
    tag: u16,
    channels: u16,
    rate: u32,
    block_align: u16,
    bits: u16,
    extra: Vec<u8>,
}

fn riff_chunks(b: &[u8]) -> (Option<WavFmt>, Option<&[u8]>) {
    let (mut fmt, mut data) = (None, None);
    let mut off = 12;
    while off + 8 <= b.len() {
        let id = &b[off..off + 4];
        let size = u32::from_le_bytes(b[off + 4..off + 8].try_into().unwrap()) as usize;
        let body = &b[off + 8..(off + 8 + size).min(b.len())];
        if id == b"fmt " && body.len() >= 16 {
            let u16_at = |i: usize| u16::from_le_bytes([body[i], body[i + 1]]);
            fmt = Some(WavFmt {
                tag: u16_at(0),
                channels: u16_at(2),
                rate: u32::from_le_bytes(body[4..8].try_into().unwrap()),
                block_align: u16_at(12),
                bits: u16_at(14),
                extra: body.get(18..).map(<[u8]>::to_vec).unwrap_or_default(),
            });
        } else if id == b"data" {
            data = Some(body);
        }
        off += 8 + size + (size & 1);
    }
    (fmt, data)
}

fn wav(b: &[u8], variant: &'static str) -> Result<Sniffed, String> {
    let (fmt, data) = riff_chunks(b);
    let fmt = fmt.ok_or("wav without fmt chunk")?;
    let data = data.ok_or("wav without data chunk")?;
    if fmt.rate == 0 || fmt.channels == 0 || fmt.block_align == 0 {
        return Err("wav with zero rate/channels/block align".into());
    }
    match fmt.tag {
        1 | 3 => {
            let frames = data.len() / fmt.block_align as usize;
            let duration = Duration::from_secs_f64(frames as f64 / fmt.rate as f64);
            // Rebuild a minimal RIFF so odd trailing chunks or bad sizes can't trip the decoder.
            let samples_fmt = (fmt.tag, fmt.bits);
            let bytes = build_wav(fmt.channels, fmt.rate, samples_fmt, &data[..frames * fmt.block_align as usize]);
            Ok(Sniffed { bytes: bytes.into(), format: Format::Wav, duration, variant })
        }
        0x11 => {
            let pcm = ima_adpcm_to_pcm(&fmt, data)?;
            let frames = pcm.len() / 2 / fmt.channels as usize;
            let duration = Duration::from_secs_f64(frames as f64 / fmt.rate as f64);
            let bytes = build_wav(fmt.channels, fmt.rate, (1, 16), &pcm);
            Ok(Sniffed { bytes: bytes.into(), format: Format::Wav, duration, variant: "ima-adpcm" })
        }
        0x55 => mp3(data, "wav+mp3"),
        t => Err(format!("unsupported wav format tag {t:#x}")),
    }
}

fn build_wav(channels: u16, rate: u32, (tag, bits): (u16, u16), data: &[u8]) -> Vec<u8> {
    let block = channels * bits / 8;
    let mut out = Vec::with_capacity(44 + data.len());
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&(36 + data.len() as u32).to_le_bytes());
    out.extend_from_slice(b"WAVEfmt ");
    out.extend_from_slice(&16u32.to_le_bytes());
    out.extend_from_slice(&tag.to_le_bytes());
    out.extend_from_slice(&channels.to_le_bytes());
    out.extend_from_slice(&rate.to_le_bytes());
    out.extend_from_slice(&(rate * block as u32).to_le_bytes());
    out.extend_from_slice(&block.to_le_bytes());
    out.extend_from_slice(&bits.to_le_bytes());
    out.extend_from_slice(b"data");
    out.extend_from_slice(&(data.len() as u32).to_le_bytes());
    out.extend_from_slice(data);
    out
}

/// Standard IMA ADPCM (wave format 0x11) to interleaved 16-bit PCM bytes.
fn ima_adpcm_to_pcm(fmt: &WavFmt, data: &[u8]) -> Result<Vec<u8>, String> {
    const STEPS: [i32; 89] = [
        7, 8, 9, 10, 11, 12, 13, 14, 16, 17, 19, 21, 23, 25, 28, 31, 34, 37, 41, 45, 50, 55, 60, 66, 73, 80, 88, 97, 107, 118, 130, 143,
        157, 173, 190, 209, 230, 253, 279, 307, 337, 371, 408, 449, 494, 544, 598, 658, 724, 796, 876, 963, 1060, 1166, 1282, 1411,
        1552, 1707, 1878, 2066, 2272, 2499, 2749, 3024, 3327, 3660, 4026, 4428, 4871, 5358, 5894, 6484, 7132, 7845, 8630, 9493,
        10442, 11487, 12635, 13899, 15289, 16818, 18500, 20350, 22385, 24623, 27086, 29794, 32767,
    ];
    const IDX: [i32; 16] = [-1, -1, -1, -1, 2, 4, 6, 8, -1, -1, -1, -1, 2, 4, 6, 8];
    let ch = fmt.channels as usize;
    let block = fmt.block_align as usize;
    if ch == 0 || ch > 2 || block < 4 * ch {
        return Err(format!("ima adpcm: bad layout ({ch} ch, block {block})"));
    }
    let per_block = if fmt.extra.len() >= 2 {
        u16::from_le_bytes([fmt.extra[0], fmt.extra[1]]) as usize
    } else {
        (block - 4 * ch) * 8 / (4 * ch) + 1
    };
    let mut out: Vec<i16> = Vec::with_capacity(data.len() * 4);
    for blk in data.chunks(block) {
        if blk.len() < 4 * ch {
            break;
        }
        let mut pred = [0i32; 2];
        let mut idx = [0i32; 2];
        let mut chans: [Vec<i16>; 2] = [Vec::with_capacity(per_block), Vec::with_capacity(per_block)];
        for c in 0..ch {
            pred[c] = i16::from_le_bytes([blk[4 * c], blk[4 * c + 1]]) as i32;
            idx[c] = (blk[4 * c + 2] as i32).clamp(0, 88);
            chans[c].push(pred[c] as i16);
        }
        let body = &blk[4 * ch..];
        // Interleaved in 4-byte words per channel, low nibble first.
        for (w, word) in body.chunks(4).enumerate() {
            let c = w % ch;
            for &byte in word {
                for nib in [byte & 0x0f, byte >> 4] {
                    let step = STEPS[idx[c] as usize];
                    let mut diff = step >> 3;
                    if nib & 1 != 0 {
                        diff += step >> 2;
                    }
                    if nib & 2 != 0 {
                        diff += step >> 1;
                    }
                    if nib & 4 != 0 {
                        diff += step;
                    }
                    pred[c] = if nib & 8 != 0 { pred[c] - diff } else { pred[c] + diff }.clamp(-32768, 32767);
                    idx[c] = (idx[c] + IDX[nib as usize]).clamp(0, 88);
                    if chans[c].len() < per_block {
                        chans[c].push(pred[c] as i16);
                    }
                }
            }
        }
        let n = chans[..ch].iter().map(Vec::len).min().unwrap_or(0);
        for i in 0..n {
            for c in chans.iter().take(ch) {
                out.push(c[i]);
            }
        }
    }
    Ok(out.iter().flat_map(|s| s.to_le_bytes()).collect())
}

/// Decodes up to `max_samples` through rodio, exactly as Bevy will. Catches
/// the panic Bevy's decoder raises on data it can't open.
pub fn decode_check(s: &Sniffed, max_samples: usize) -> Result<usize, String> {
    let src = bevy::audio::AudioSource { bytes: s.bytes.clone() };
    std::panic::catch_unwind(move || src.decoder().take(max_samples).count()).map_err(|_| "rodio could not open the stream".to_string())
}

// ---------------------------------------------------------------------------
// Resolution
// ---------------------------------------------------------------------------

/// Where sound files live on disk outside the resource system.
#[derive(Resource, Default)]
pub struct SoundIndex {
    voice: HashMap<String, PathBuf>,
    music: HashMap<String, PathBuf>,
    sounds: HashMap<String, PathBuf>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SoundKind {
    Music,
    Ambient,
    Effect,
    Voice,
}

fn index_dir(dir: &Path, out: &mut HashMap<String, PathBuf>) {
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    for e in rd.flatten() {
        let p = e.path();
        if p.is_dir() {
            index_dir(&p, out);
        } else if let Some(stem) = p.file_stem().and_then(|s| s.to_str()) {
            out.entry(stem.to_ascii_lowercase()).or_insert(p);
        }
    }
}

impl SoundIndex {
    /// streamvoice is nested (<module>/<dialog>/<resref>.wav); the others are flat.
    pub fn build(install: &Install) -> Self {
        let mut s = Self::default();
        if let Some(d) = install.dir("streamvoice") {
            index_dir(&d, &mut s.voice);
        }
        if let Some(d) = install.dir("streammusic") {
            index_dir(&d, &mut s.music);
        }
        if let Some(d) = install.dir("streamsounds") {
            index_dir(&d, &mut s.sounds);
        }
        s
    }

    pub fn counts(&self) -> (usize, usize, usize) {
        (self.voice.len(), self.music.len(), self.sounds.len())
    }

    /// Finds raw bytes for a sound resref. Returns the bytes and a short
    /// description of where they came from.
    pub fn find(&self, res: &Resources, name: &str, kind: SoundKind) -> Option<(Vec<u8>, String)> {
        let name = name.to_ascii_lowercase();
        if name.is_empty() {
            return None;
        }
        let dirs: [&HashMap<String, PathBuf>; 3] = match kind {
            SoundKind::Music => [&self.music, &self.sounds, &self.voice],
            SoundKind::Ambient | SoundKind::Effect => [&self.sounds, &self.music, &self.voice],
            SoundKind::Voice => [&self.voice, &self.sounds, &self.music],
        };
        // Override and module resources win, as everywhere else.
        let key = ResKey::new(&name, ResourceType::Wav);
        let from_res = |res: &Resources| res.locate(&key).and_then(|o| res.read(&o).ok().map(|b| (b, format!("res {o:?}"))));
        if let Some(found) = from_res(res).filter(|(_, w)| w.starts_with("res Override") || w.starts_with("res Module")) {
            return Some(found);
        }
        for d in dirs {
            if let Some(p) = d.get(&name)
                && let Ok(b) = std::fs::read(p)
            {
                return Some((b, p.display().to_string()));
            }
        }
        from_res(res)
    }
}

/// What the alien set has for a dialog line with no voice file of its own,
/// for race `row` at emotion `emotion`.
pub enum AlienSlot {
    /// emotion.2da "Silence": the line is spoken without a sound.
    Silence,
    /// No alienvo.2da cell for this race and category.
    NoCell,
    /// The cell's clips avo_<cell>1, 2, ... found in streamvoice. Empty when
    /// the cell names no clip that ships in the install.
    Clips(Vec<String>),
}

/// The alien set for a line: the alienvo.2da cell for `row` (the alien race)
/// in the emotion.2da column named by `emotion` (Comment_Generic_Short,
/// Angry_Medium, ...).
pub fn alien_slot(tables: &k2_data::Tables, res: &Resources, index: &SoundIndex, row: u32, emotion: i32) -> AlienSlot {
    let (Ok(avo), Ok(emo)) = (tables.get(res, "alienvo"), tables.get(res, "emotion")) else { return AlienSlot::NoCell };
    let Some(label) = emo.str(emotion as usize, "label") else { return AlienSlot::NoCell };
    if label.eq_ignore_ascii_case("silence") {
        return AlienSlot::Silence;
    }
    let Some(cell) = avo.resref(row as usize, &label.to_ascii_lowercase()) else { return AlienSlot::NoCell };
    AlienSlot::Clips((1..=9).map(|n| format!("{cell}{n}")).filter(|v| index.voice.contains_key(v)).collect())
}

/// The playable clips of `alien_slot`; empty for silence or no cell.
pub fn alien_clips(tables: &k2_data::Tables, res: &Resources, index: &SoundIndex, row: u32, emotion: i32) -> Vec<String> {
    match alien_slot(tables, res, index, row, emotion) {
        AlienSlot::Clips(clips) => clips,
        AlienSlot::Silence | AlienSlot::NoCell => Vec::new(),
    }
}

/// Index of the next variant: random, but not `last` while another variant
/// exists. `rng` is xorshift state (not zero).
fn pick_variant(clips: &[String], last: Option<&str>, rng: &mut u32) -> usize {
    let fresh: Vec<usize> = (0..clips.len()).filter(|&i| Some(clips[i].as_str()) != last).collect();
    let pool: Vec<usize> = if fresh.is_empty() { (0..clips.len()).collect() } else { fresh };
    pool[xorshift(rng) as usize % pool.len()]
}

/// A seed that differs from launch to launch.
fn launch_seed() -> u32 {
    let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(1, |d| d.subsec_nanos());
    (nanos ^ 0x9E37_79B9).max(1)
}

/// The alien line playing now (see `dialog_ui`): its clip length drives the
/// synthetic lip track, so the mouth stops with the audio.
#[derive(Resource, Default)]
pub struct AlienLine {
    /// Dialog resref and entry of the line.
    pub line: Option<(String, usize)>,
    pub seconds: f32,
}

/// Decoded, validated clips by lowercase resref.
#[derive(Resource, Default)]
pub struct AudioCache {
    clips: HashMap<String, Option<(Handle<bevy::audio::AudioSource>, Duration)>>,
    tables: k2_data::Tables,
    rng: u32,
    last_alien: Option<String>,
}

impl AudioCache {
    pub(crate) fn get(
        &mut self,
        res: &Resources,
        index: &SoundIndex,
        assets: &mut Assets<bevy::audio::AudioSource>,
        name: &str,
        kind: SoundKind,
    ) -> Option<(Handle<bevy::audio::AudioSource>, Duration)> {
        let key = name.to_ascii_lowercase();
        if let Some(c) = self.clips.get(&key) {
            return c.clone();
        }
        let loaded = (|| {
            let (raw, from) = index.find(res, &key, kind).or_else(|| {
                warn!("audio: {key} ({kind:?}) not found");
                None
            })?;
            let s = sniff(&raw).map_err(|e| warn!("audio: {key} from {from}: {e}")).ok()?;
            decode_check(&s, 64).map_err(|e| warn!("audio: {key} from {from}: {e}")).ok()?;
            info!("audio: {key} <- {from} [{} {:?}, {:.2}s]", s.variant, s.format, s.duration.as_secs_f32());
            let h = assets.add(bevy::audio::AudioSource { bytes: s.bytes });
            Some((h, s.duration))
        })();
        self.clips.insert(key, loaded.clone());
        loaded
    }

    /// One alien clip for `row` at `emotion`: random among the variants
    /// (seeded per launch), never the clip the last alien line used while
    /// another variant exists.
    pub(crate) fn alien(
        &mut self,
        res: &Resources,
        index: &SoundIndex,
        assets: &mut Assets<bevy::audio::AudioSource>,
        row: u32,
        emotion: i32,
    ) -> Option<(Handle<bevy::audio::AudioSource>, Duration)> {
        let clips = alien_clips(&self.tables, res, index, row, emotion);
        if clips.is_empty() {
            return None;
        }
        if self.rng == 0 {
            self.rng = launch_seed();
        }
        let pick = clips[pick_variant(&clips, self.last_alien.as_deref(), &mut self.rng)].clone();
        self.last_alien = Some(pick.clone());
        self.get(res, index, assets, &pick, SoundKind::Voice)
    }
}

// ---------------------------------------------------------------------------
// Bevy side
// ---------------------------------------------------------------------------

#[derive(Resource, Clone, Copy)]
pub struct AudioSettings {
    pub master: f32,
}

impl AudioSettings {
    pub fn from_args() -> Self {
        let master = if has_switch("--mute") { 0.0 } else { arg_value("--volume").and_then(|v| v.parse().ok()).unwrap_or(0.7f32).max(0.0) };
        Self { master }
    }
}

pub struct KAudioPlugin;

impl Plugin for KAudioPlugin {
    fn build(&self, app: &mut App) {
        let settings = AudioSettings::from_args();
        app.insert_resource(settings)
            // Master volume is applied once, globally; per-sink volumes exclude it.
            .insert_resource(GlobalVolume::new(Volume::Linear(settings.master)))
            .init_resource::<AudioCache>()
            .init_resource::<AlienLine>()
            .init_resource::<SoundIndex>()
            .add_systems(Startup, build_sound_index)
            .add_systems(Update, setup_area_audio)
            .add_systems(Update, (attach_listener, run_emitters, attenuate_emitters, report_sinks))
            .add_systems(Update, drive_sound_objects.in_set(GameSet::Sync).after(ApplyEvents).before(run_emitters));
    }
}

/// Plays dialog voice-over and one-shot UI/effect sounds.
#[derive(SystemParam)]
pub struct Voice<'w, 's> {
    commands: Commands<'w, 's>,
    assets: ResMut<'w, Assets<bevy::audio::AudioSource>>,
    cache: ResMut<'w, AudioCache>,
    index: Res<'w, SoundIndex>,
    game: Res<'w, GameRes>,
    current: Query<'w, 's, Entity, With<VoiceLine>>,
}

/// The voice line currently playing.
#[derive(Component)]
pub struct VoiceLine;

#[allow(dead_code)] // API for the dialog UI
impl Voice<'_, '_> {
    /// Stops any current line and plays `resref`. Returns its duration so the
    /// dialog can auto-advance; None when the line has no audio.
    pub fn play_voice(&mut self, resref: &str) -> Option<Duration> {
        self.stop_voice();
        let (h, d) = self.cache.get(&self.game.0, &self.index, &mut self.assets, resref, SoundKind::Voice)?;
        self.commands.spawn((
            Name::new(format!("voice {resref}")),
            VoiceLine,
            AudioPlayer::new(h),
            PlaybackSettings::DESPAWN,
        ));
        Some(d)
    }

    /// Stops any current line and plays the alien generic voice for `row`
    /// (alienvo.2da) at `emotion` (emotion.2da). Same return as `play_voice`.
    pub fn play_alien(&mut self, row: u32, emotion: i32) -> Option<Duration> {
        self.stop_voice();
        let (h, d) = self.cache.alien(&self.game.0, &self.index, &mut self.assets, row, emotion)?;
        self.commands.spawn((
            Name::new(format!("alien {row} emotion {emotion}")),
            VoiceLine,
            AudioPlayer::new(h),
            PlaybackSettings::DESPAWN,
        ));
        Some(d)
    }

    pub fn stop_voice(&mut self) {
        for e in &self.current {
            self.commands.entity(e).despawn();
        }
    }

    /// Non-positional one-shot.
    pub fn play_sound(&mut self, resref: &str) -> Option<Duration> {
        let (h, d) = self.cache.get(&self.game.0, &self.index, &mut self.assets, resref, SoundKind::Effect)?;
        self.commands.spawn((AudioPlayer::new(h), PlaybackSettings::DESPAWN));
        Some(d)
    }

    /// Spatial one-shot at `position` (local to `parent`), linear falloff
    /// from `min` (full) to `max` (silent) metres.
    pub fn play_sound_at(&mut self, resref: &str, parent: Entity, position: Vec3, range: (f32, f32)) -> Option<Duration> {
        let (h, d) = self.cache.get(&self.game.0, &self.index, &mut self.assets, resref, SoundKind::Effect)?;
        let (min, max) = range;
        let ps = PlaybackSettings { mode: PlaybackMode::Despawn, ..PlaybackSettings::ONCE }
            .with_volume(Volume::Linear(0.0))
            .with_spatial(true)
            .with_spatial_scale(SpatialScale::new(1.0 / max.max(1.0)));
        self.commands.spawn((
            Name::new(format!("sfx {resref}")),
            EmitterVoice { emitter: parent, min, max, gain: 1.0 },
            AudioPlayer::new(h),
            ps,
            Transform::from_translation(position),
            ChildOf(parent),
        ));
        Some(d)
    }

    /// Spatial loop that follows `parent` (an engine hum on a moving
    /// vehicle); despawns with it.
    pub fn play_loop_on(&mut self, resref: &str, parent: Entity, range: (f32, f32)) -> Option<Entity> {
        let (h, _) = self.cache.get(&self.game.0, &self.index, &mut self.assets, resref, SoundKind::Effect)?;
        let (min, max) = range;
        let ps = PlaybackSettings::LOOP
            .with_volume(Volume::Linear(0.0))
            .with_spatial(true)
            .with_spatial_scale(SpatialScale::new(1.0 / max.max(1.0)));
        Some(
            self.commands
                .spawn((
                    Name::new(format!("loop {resref}")),
                    EmitterVoice { emitter: parent, min, max, gain: 1.0 },
                    AudioPlayer::new(h),
                    ps,
                    Transform::default(),
                    ChildOf(parent),
                ))
                .id(),
        )
    }
}

/// Spawns a positional one-shot as a child of `parent`, so it follows a
/// moving creature. Volume ramps linearly from `min` (full) to `max` (silent)
/// via `attenuate_emitters`. Returns the clip duration and the spawned
/// entity; None when the resref doesn't resolve.
#[allow(clippy::too_many_arguments)]
pub(crate) fn spawn_positional(
    commands: &mut Commands,
    cache: &mut AudioCache,
    res: &Resources,
    index: &SoundIndex,
    assets: &mut Assets<bevy::audio::AudioSource>,
    parent: Entity,
    resref: &str,
    (min, max, gain): (f32, f32, f32),
) -> Option<(Duration, Entity)> {
    let (h, d) = cache.get(res, index, assets, resref, SoundKind::Effect)?;
    let ps = PlaybackSettings { mode: PlaybackMode::Despawn, ..PlaybackSettings::ONCE }
        .with_volume(Volume::Linear(0.0))
        .with_spatial(true)
        .with_spatial_scale(SpatialScale::new(1.0 / max.max(1.0)));
    let e = commands
        .spawn((
            Name::new(format!("sfx {resref}")),
            EmitterVoice { emitter: parent, min, max, gain },
            AudioPlayer::new(h),
            ps,
            Transform::from_xyz(0.0, 0.0, 1.0),
            ChildOf(parent),
        ))
        .id();
    Some((d, e))
}

/// A GIT sound object.
#[derive(Component)]
pub struct SoundEmitter {
    /// For SoundObjectPlay/Stop by tag once k2-world drives sounds.
    #[allow(dead_code)]
    pub tag: String,
    bp: k2_data::SoundBlueprint,
    next_at: f32,
    playing: Option<Entity>,
    seq: usize,
    rng: u32,
    /// Started by a script (SoundObjectPlay): every clip start is logged.
    scripted: bool,
    /// Playback speed from SoundObjectSetFixedVariance.
    pitch: f32,
    /// Last speed written to the log (log again on a 10% change).
    logged_pitch: f32,
}

/// A clip fading out (SoundObjectFadeAndStop); despawned at 0.
#[derive(Component)]
struct FadeOut {
    left: f32,
    total: f32,
}

/// Child entity playing one clip of an emitter.
#[derive(Component)]
struct EmitterVoice {
    #[allow(dead_code)] // kept for debugging/inspection
    emitter: Entity,
    min: f32,
    max: f32,
    gain: f32,
}

#[derive(Component)]
pub struct AreaMusic;

#[derive(Component)]
pub struct AreaAmbient;

/// Music and ambience entities replaced on each area load.
type AreaBed = Or<(With<AreaMusic>, With<AreaAmbient>)>;

fn xorshift(s: &mut u32) -> u32 {
    *s ^= *s << 13;
    *s ^= *s >> 17;
    *s ^= *s << 5;
    *s
}

/// Area plan resolved from GIT AreaProperties plus the 2DAs.
pub struct AreaAudioPlan {
    pub music: Option<String>,
    pub ambient: Option<String>,
    pub ambient_volume: f32,
}

pub fn area_plan(res: &Resources, gd: &GameData, module: &k2_data::Module) -> AreaAudioPlan {
    let p = &module.git.properties;
    let night = module.area.is_night;
    let pick = |table: &str, row: i32| -> Option<String> {
        if row <= 0 {
            return None;
        }
        gd.tables.get(res, table).ok()?.resref(row as usize, "resource").filter(|r| !r.is_empty())
    };
    let music = pick("ambientmusic", if night { p.music_night } else { p.music_day });
    let ambient = pick("ambientsound", if night { p.ambient_night } else { p.ambient_day });
    let vol = if night { p.ambient_night_volume } else { p.ambient_day_volume };
    AreaAudioPlan { music, ambient, ambient_volume: (vol as f32 / 127.0).clamp(0.0, 1.0) }
}

fn build_sound_index(game: Res<GameRes>, mut index: ResMut<SoundIndex>) {
    let t0 = std::time::Instant::now();
    *index = SoundIndex::build(&game.0.install);
    let (v, m, s) = index.counts();
    info!("audio: indexed {v} voice, {m} music, {s} sound files in {:?}", t0.elapsed());
}

/// Area bed: background music, ambience and GIT sound objects. Rebuilt on
/// every `AreaLoaded`; sound objects live under the area root, so they go
/// away with it, and music/ambience are replaced here.
#[allow(clippy::too_many_arguments)]
fn setup_area_audio(
    mut commands: Commands,
    mut loaded: MessageReader<AreaLoaded>,
    module: Option<Res<AreaModule>>,
    gd: Option<Res<GameData>>,
    game: Res<GameRes>,
    index: Res<SoundIndex>,
    mut cache: ResMut<AudioCache>,
    mut assets: ResMut<Assets<bevy::audio::AudioSource>>,
    beds: Query<Entity, AreaBed>,
    area: Query<Entity, With<AreaRoot>>,
) {
    let Some(area_name) = loaded.read().last().map(|l| l.module.clone()) else { return };
    info!("audio: area {area_name}");
    let t0 = std::time::Instant::now();
    for e in &beds {
        commands.entity(e).despawn();
    }
    let (Some(module), Some(gd)) = (module, gd) else { return };
    let plan = area_plan(&game.0, &gd, &module.0);
    if let Some(name) = &plan.music
        && let Some((h, d)) = cache.get(&game.0, &index, &mut assets, name, SoundKind::Music)
    {
        {
            info!("audio: music {name} ({:.1}s) looping", d.as_secs_f32());
            commands.spawn((Name::new(format!("music {name}")), AreaMusic, AudioPlayer::new(h), PlaybackSettings::LOOP.with_volume(Volume::Linear(0.6))));
        }
    }
    if let Some(name) = &plan.ambient
        && let Some((h, _)) = cache.get(&game.0, &index, &mut assets, name, SoundKind::Ambient)
    {
        {
            info!("audio: ambient bed {name} at {:.2}", plan.ambient_volume);
            commands.spawn((Name::new(format!("ambient {name}")), AreaAmbient, AudioPlayer::new(h), PlaybackSettings::LOOP.with_volume(Volume::Linear(plan.ambient_volume))));
        }
    }
    let Ok(root) = area.single() else { return };
    let mut n = 0;
    for (i, inst) in module.0.git.sounds.iter().enumerate() {
        let bp = match k2_data::SoundBlueprint::load(&game.0, &inst.template) {
            Ok(bp) => bp,
            Err(e) => {
                warn!("audio: sound {}: {e}", inst.template);
                continue;
            }
        };
        if bp.sounds.is_empty() {
            continue;
        }
        let pos = Vec3::from(inst.position) + Vec3::Z * bp.elevation;
        commands.spawn((
            Name::new(format!("sound {}", bp.tag)),
            SoundEmitter {
                tag: bp.tag.clone(),
                // Stagger interval sounds so identical emitters don't fire in unison.
                next_at: if bp.looping { 0.0 } else { (i as f32 * 0.618).fract() * bp.interval as f32 / 1000.0 },
                playing: None,
                seq: 0,
                rng: 0x9E37_79B9 ^ (i as u32 + 1).wrapping_mul(2_654_435_761),
                scripted: false,
                pitch: 1.0,
                logged_pitch: 1.0,
                bp,
            },
            Transform::from_translation(pos),
            Visibility::default(),
            ChildOf(root),
        ));
        n += 1;
    }
    info!("audio: {n} sound objects in {:?}", t0.elapsed());
}

fn attach_listener(mut commands: Commands, cams: Query<Entity, (With<Camera3d>, Without<SpatialListener>)>) {
    for e in &cams {
        commands.entity(e).insert(SpatialListener::new(0.3));
    }
}

/// Starts the next clip of each active emitter when its interval is up.
#[allow(clippy::too_many_arguments)]
fn run_emitters(
    mut commands: Commands,
    time: Res<Time>,
    game: Res<GameRes>,
    index: Res<SoundIndex>,
    mut cache: ResMut<AudioCache>,
    mut assets: ResMut<Assets<bevy::audio::AudioSource>>,
    mut emitters: Query<(Entity, &mut SoundEmitter)>,
    alive: Query<(), With<EmitterVoice>>,
) {
    let now = time.elapsed_secs();
    for (entity, mut em) in &mut emitters {
        if !em.bp.active {
            continue;
        }
        if let Some(p) = em.playing {
            if alive.get(p).is_ok() {
                continue;
            }
            em.playing = None;
            // Finished: schedule the next one.
            let var = em.bp.interval_variation as f32;
            let r = (xorshift(&mut em.rng) % 1000) as f32 / 1000.0;
            em.next_at = now + (em.bp.interval as f32 + (r * 2.0 - 1.0) * var).max(0.0) / 1000.0;
            // Continuous=0: the list plays through once, then the object goes
            // inactive. Continuous=1 repeats forever with the interval between.
            if !em.bp.continuous && em.seq >= em.bp.sounds.len() {
                em.bp.active = false;
                continue;
            }
        }
        if now < em.next_at {
            continue;
        }
        let n = em.bp.sounds.len();
        let idx = if em.bp.random && !em.bp.looping { (xorshift(&mut em.rng) as usize) % n } else { em.seq % n };
        em.seq += 1;
        let name = em.bp.sounds[idx].clone();
        let Some((h, _)) = cache.get(&game.0, &index, &mut assets, &name, SoundKind::Ambient) else {
            em.bp.active = false;
            continue;
        };
        // UTS flags, as the shipped data uses them: Looping=1 is one wave looped
        // seamlessly (hums, interval 0); Continuous=1 is a repeating sequence
        // with Interval +- IntervalVrtn gaps (chatter, beeps, alarms).
        let seamless = em.bp.looping;
        if em.scripted {
            info!("audio: sound object {} starts {name} at {now:.2}s ({})", em.tag, if seamless { "loop" } else { "once" });
        } else {
            debug!("audio: emitter {} starts {name} at {now:.1}s seamless={seamless}", em.tag);
        }
        let gain = em.bp.volume as f32 / 127.0;
        let mode = if seamless { PlaybackMode::Loop } else { PlaybackMode::Despawn };
        let mut ps = PlaybackSettings { mode, ..PlaybackSettings::ONCE }.with_volume(Volume::Linear(0.0));
        if em.bp.positional {
            // Scale space so rodio's own 1/d^2 falloff stays at 1 inside max
            // distance; attenuate_emitters applies the KotOR min/max ramp.
            ps = ps.with_spatial(true).with_spatial_scale(SpatialScale::new(1.0 / em.bp.max_distance.max(1.0)));
        }
        let (min, max) = if em.bp.positional { (em.bp.min_distance, em.bp.max_distance) } else { (f32::MAX, f32::MAX) };
        let child = commands
            .spawn((Name::new(format!("clip {name}")), EmitterVoice { emitter: entity, min, max, gain }, AudioPlayer::new(h), ps, Transform::default(), ChildOf(entity)))
            .id();
        em.playing = Some(child);
    }
}

type VoiceItem<'a> = (Entity, &'a EmitterVoice, &'a GlobalTransform, Option<&'a mut FadeOut>, Option<&'a mut SpatialAudioSink>, Option<&'a mut AudioSink>);

fn attenuate_emitters(
    mut commands: Commands,
    time: Res<Time>,
    listener: Query<&GlobalTransform, With<SpatialListener>>,
    emitters: Query<&SoundEmitter>,
    mut voices: Query<VoiceItem>,
) {
    let Ok(lt) = listener.single() else { return };
    let lp = lt.translation();
    for (entity, v, gt, fade, spatial, plain) in &mut voices {
        let d = gt.translation().distance(lp);
        let ramp = if d <= v.min {
            1.0
        } else if d >= v.max {
            0.0
        } else {
            (v.max - d) / (v.max - v.min).max(0.001)
        };
        // Sound objects follow their live volume and pitch (script calls).
        let (gain, pitch) = emitters.get(v.emitter).map_or((v.gain, 1.0), |e| (e.bp.volume as f32 / 127.0, e.pitch));
        let mut fade_gain = 1.0;
        if let Some(mut f) = fade {
            f.left -= time.delta_secs();
            if f.left <= 0.0 {
                commands.entity(entity).try_despawn();
                continue;
            }
            fade_gain = f.left / f.total.max(0.001);
        }
        let vol = Volume::Linear(gain * ramp * fade_gain);
        // Out-of-range clips are paused so far emitters cost no decoding.
        let audible = ramp > 0.0;
        if let Some(mut s) = spatial {
            s.set_volume(vol);
            if s.speed() != pitch {
                s.set_speed(pitch);
            }
            match (audible, s.is_paused()) {
                (false, false) => s.pause(),
                (true, true) => s.play(),
                _ => {}
            }
        } else if let Some(mut s) = plain {
            s.set_volume(vol);
            if s.speed() != pitch {
                s.set_speed(pitch);
            }
        }
    }
}

/// Scripts driving GIT sound objects: SoundObjectPlay / Stop / FadeAndStop,
/// SetVolume (0..127) and SetFixedVariance (pitch shift in semitones; the swoop
/// engine loops step up about 2.7 semitones per gear and the script revs each
/// 1.2..4 semitones above its loop, which lines up across shifts). The
/// swoop minigames play all their engine, gear, countdown, wind and crash
/// sounds this way. Events whose sound object has no emitter yet (a script
/// ran while the area was still loading) wait up to 3 s.
#[allow(clippy::too_many_arguments)]
fn drive_sound_objects(
    mut commands: Commands,
    time: Res<Time>,
    events: Res<FrameEvents>,
    world: Res<WorldRes>,
    mut emitters: Query<(Entity, &Transform, &mut SoundEmitter)>,
    mut pending: Local<Vec<(f32, WorldEvent)>>,
    mut warned: Local<std::collections::HashSet<String>>,
) {
    let now = time.elapsed_secs();
    let mut work: Vec<(f32, WorldEvent)> = std::mem::take(&mut *pending);
    work.extend(events.0.iter().filter(|e| matches!(e, WorldEvent::SoundObject { .. } | WorldEvent::SoundObjectVolume { .. } | WorldEvent::SoundObjectVariance { .. })).map(|e| (now, e.clone())));
    for (since, ev) in work {
        let object: ObjectId = match &ev {
            WorldEvent::SoundObject { object, .. } | WorldEvent::SoundObjectVolume { object, .. } | WorldEvent::SoundObjectVariance { object, .. } => *object,
            _ => continue,
        };
        let Some(o) = world.0.get(object) else { continue };
        let at = o.position;
        let near = |t: &Transform| (t.translation.x - at.x).powi(2) + (t.translation.y - at.y).powi(2);
        let found = emitters
            .iter()
            .filter(|(_, _, e)| e.tag.eq_ignore_ascii_case(&o.tag))
            .min_by(|a, b| near(a.1).total_cmp(&near(b.1)))
            .map(|(e, _, _)| e);
        let Some(entity) = found else {
            if now - since < 3.0 {
                pending.push((since, ev));
            } else if warned.insert(o.tag.clone()) {
                warn!("audio: script sound object {:?} has no emitter in this area", o.tag);
            }
            continue;
        };
        let Ok((_, _, mut em)) = emitters.get_mut(entity) else { continue };
        match ev {
            WorldEvent::SoundObject { playing: true, .. } => {
                let running = em.playing.is_some() && em.bp.active;
                em.scripted = true;
                if em.bp.looping || em.bp.continuous {
                    // A loop that is already running keeps running.
                    if running {
                        continue;
                    }
                } else if let Some(p) = em.playing.take() {
                    // A one-shot plays again from the start.
                    commands.entity(p).try_despawn();
                }
                info!("audio: sound object {} play at {now:.2}s", em.tag);
                em.bp.active = true;
                em.seq = 0;
                em.next_at = 0.0;
            }
            WorldEvent::SoundObject { playing: false, fade, .. } => {
                info!("audio: sound object {} stop at {now:.2}s (fade {fade:.1}s)", em.tag);
                em.bp.active = false;
                if let Some(p) = em.playing.take() {
                    if fade > 0.0 {
                        commands.entity(p).try_insert(FadeOut { left: fade, total: fade });
                    } else {
                        commands.entity(p).try_despawn();
                    }
                }
            }
            WorldEvent::SoundObjectVolume { volume, .. } => {
                debug!("audio: sound object {} volume {volume}", em.tag);
                em.bp.volume = volume.clamp(0, 127) as u32;
            }
            WorldEvent::SoundObjectVariance { variance, .. } => {
                em.pitch = 2f32.powf(variance / 12.0).clamp(0.5, 2.5);
                if (em.pitch - em.logged_pitch).abs() >= 0.1 {
                    em.logged_pitch = em.pitch;
                    info!("audio: sound object {} pitch x{:.2} (variance {variance:.1}) at {now:.2}s", em.tag, em.pitch);
                }
            }
            _ => {}
        }
    }
}

/// Logs once, 3 s in, how many sinks are live: proof audio actually started.
fn report_sinks(time: Res<Time>, mut done: Local<bool>, plain: Query<&AudioSink>, spatial: Query<(&SpatialAudioSink, &EmitterVoice)>) {
    if *done || time.elapsed_secs() < 3.0 {
        return;
    }
    *done = true;
    let audible = spatial.iter().filter(|(s, _)| s.volume().to_linear() > 0.0).count();
    info!("audio: {} plain sinks, {} spatial sinks ({audible} audible from here)", plain.iter().count(), spatial.iter().count());
}

// ---------------------------------------------------------------------------
// CLI modes
// ---------------------------------------------------------------------------

/// `--audio-test MODULE`: resolve and decode the area's audio, print the plan.
pub fn audio_test_module() -> Option<String> {
    arg_value("--audio-test").map(|m| m.to_ascii_lowercase())
}

pub fn wants_survey() -> bool {
    has_switch("--audio-survey")
}

fn describe(res: &Resources, index: &SoundIndex, name: &str, kind: SoundKind) -> String {
    match index.find(res, name, kind) {
        None => "NOT FOUND".into(),
        Some((raw, from)) => match sniff(&raw) {
            Err(e) => format!("{from}: SNIFF FAIL {e}"),
            Ok(s) => match decode_check(&s, 4096) {
                Ok(_) => format!("{from} [{} {:?} {:.2}s]", s.variant, s.format, s.duration.as_secs_f32()),
                Err(e) => format!("{from}: DECODE FAIL {e}"),
            },
        },
    }
}

pub fn run_audio_test(res: &mut Resources, module: &str) {
    let index = SoundIndex::build(&res.install);
    let (v, m, s) = index.counts();
    println!("index: {v} voice, {m} music, {s} sound files");
    let module = match k2_data::Module::load(res, module) {
        Ok(m) => m,
        Err(e) => {
            println!("module {module}: {e}");
            return;
        }
    };
    let gd = GameData { tables: k2_data::Tables::new() };
    let p = &module.git.properties;
    println!(
        "area {} night={} music day/night/battle {}/{}/{} ambient day/night {}/{} vol {}/{}",
        module.area.resref, module.area.is_night, p.music_day, p.music_night, p.music_battle, p.ambient_day, p.ambient_night, p.ambient_day_volume, p.ambient_night_volume
    );
    let plan = area_plan(res, &gd, &module);
    for (label, name, kind) in [("music", &plan.music, SoundKind::Music), ("ambient", &plan.ambient, SoundKind::Ambient)] {
        match name {
            Some(n) => println!("{label}: {n} -> {}", describe(res, &index, n, kind)),
            None => println!("{label}: none"),
        }
    }
    for inst in &module.git.sounds {
        match k2_data::SoundBlueprint::load(res, &inst.template) {
            Err(e) => println!("sound {}: blueprint error {e}", inst.template),
            Ok(bp) => {
                println!(
                    "sound {} tag {} at {:?} active={} looping={} continuous={} positional={} random={} vol={} dist {}..{} interval {}+-{}ms",
                    inst.template, bp.tag, inst.position, bp.active, bp.looping, bp.continuous, bp.positional, bp.random, bp.volume, bp.min_distance, bp.max_distance, bp.interval, bp.interval_variation
                );
                for n in &bp.sounds {
                    println!("    {n} -> {}", describe(res, &index, n, SoundKind::Ambient));
                }
            }
        }
    }
}

/// `--audio-survey`: sniff and decode every stream file and every WAV resource.
pub fn run_survey(res: &Resources) {
    let t0 = std::time::Instant::now();
    let mut files: Vec<PathBuf> = Vec::new();
    for d in ["streamvoice", "streammusic", "streamsounds"] {
        if let Some(dir) = res.install.dir(d) {
            let mut m = HashMap::new();
            index_dir(&dir, &mut m);
            files.extend(m.into_values());
        }
    }
    files.sort();
    let mut variants: HashMap<String, usize> = HashMap::new();
    let mut fails: Vec<String> = Vec::new();
    let mut total = Duration::ZERO;
    let mut check = |label: String, raw: &[u8], group: &str| -> Result<(), String> {
        let s = sniff(raw).map_err(|e| format!("{label}: {e}"))?;
        match decode_check(&s, 2048).map_err(|e| format!("{label}: {e}"))? {
            0 => Err(format!("{label}: decoded 0 samples")),
            _ => {
                total += s.duration;
                *variants.entry(format!("{group} {}", s.variant)).or_default() += 1;
                Ok(())
            }
        }
    };
    for f in &files {
        let group = f
            .ancestors()
            .filter_map(|a| a.file_name()?.to_str())
            .find(|n| n.starts_with("stream"))
            .unwrap_or("?")
            .to_owned();
        let r = std::fs::read(f).map_err(|e| format!("{}: {e}", f.display())).and_then(|b| check(f.display().to_string(), &b, &group));
        if let Err(e) = r {
            fails.push(e);
        }
    }
    let wav = ResourceType::Wav.type_id();
    let mut keys: Vec<ResKey> = res.chitin_keys().filter(|k| k.ty == wav).cloned().collect();
    keys.sort();
    for k in &keys {
        let r = match res.get(k) {
            Ok(Some(b)) => check(format!("chitin {k}"), &b, "chitin"),
            Ok(None) => Ok(()),
            Err(e) => Err(format!("chitin {k}: {e}")),
        };
        if let Err(e) = r {
            fails.push(e);
        }
    }
    // Module capsules can carry their own WAVs.
    let mut module_wavs = 0;
    let mut res_m = match Resources::open(res.install.clone()) {
        Ok(r) => Some(r),
        Err(e) => {
            fails.push(format!("reopen install: {e}"));
            None
        }
    };
    if let Some(rm) = res_m.as_mut() {
        for m in rm.install.module_names() {
            if rm.load_module(&m).is_err() {
                continue;
            }
            let keys: Vec<ResKey> = rm.module_keys().filter(|k| k.ty == wav).cloned().collect();
            for k in keys {
                module_wavs += 1;
                let r = match rm.get(&k) {
                    Ok(Some(b)) => check(format!("{m} {k}"), &b, "module"),
                    Ok(None) => Ok(()),
                    Err(e) => Err(format!("{m} {k}: {e}")),
                };
                if let Err(e) = r {
                    fails.push(e);
                }
            }
        }
    }
    println!("module wavs: {module_wavs}");
    // Dialog VO coverage: every entry/reply VO_ResRef and Sound across all modules.
    if let Some(rm) = res_m.as_mut() {
        let index = SoundIndex::build(&rm.install);
        let dlg = ResourceType::Dlg.type_id();
        let (mut refs, mut found) = (0usize, 0usize);
        let mut missing: Vec<String> = Vec::new();
        // Per module: [alien lines, voiced by own file, voiced by alien clip, clip decode failures,
        // silent: Silence, no alienvo cell, cell with no clip in the install].
        let tables = k2_data::Tables::new();
        let mut alien: HashMap<String, [usize; 7]> = HashMap::new();
        for m in rm.install.module_names() {
            if rm.load_module(&m).is_err() {
                continue;
            }
            let names: Vec<String> = rm.module_keys().filter(|k| k.ty == dlg).map(|k| k.name.clone()).collect();
            for n in names {
                let Ok(d) = k2_data::Dialog::load(rm, &n) else { continue };
                for node in d.entries.iter().chain(d.replies.iter()) {
                    for r in [&node.vo_resref, &node.sound].into_iter().flatten() {
                        refs += 1;
                        if index.find(rm, r, SoundKind::Voice).is_some() {
                            found += 1;
                        } else if missing.len() < 10 {
                            missing.push(format!("{m}/{n}: {r}"));
                        }
                    }
                }
                // Alien race: the node's, else the owner's for a line with no
                // speaker tag (the owner speaks). A named speaker that is the
                // owner is only known in a running conversation.
                for node in &d.entries {
                    let race = match node.alien_race {
                        0 if node.speaker.is_empty() => d.alien_race_owner,
                        r => r,
                    };
                    if race == 0 {
                        continue;
                    }
                    let c = alien.entry(m.clone()).or_default();
                    c[0] += 1;
                    if [&node.vo_resref, &node.sound].into_iter().flatten().any(|r| index.find(rm, r, SoundKind::Voice).is_some()) {
                        c[1] += 1;
                        continue;
                    }
                    match alien_slot(&tables, rm, &index, race, node.emotion) {
                        AlienSlot::Silence => c[4] += 1,
                        AlienSlot::NoCell => c[5] += 1,
                        AlienSlot::Clips(clips) => match clips.first().and_then(|c| index.find(rm, c, SoundKind::Voice)) {
                            Some((raw, _)) if sniff(&raw).and_then(|s| decode_check(&s, 64)).is_ok() => c[2] += 1,
                            Some(_) => c[3] += 1,
                            None => c[6] += 1,
                        },
                    }
                }
            }
        }
        println!("dialog VO: {found}/{refs} resrefs resolve; first missing: {missing:?}");
        let mut rows: Vec<(String, [usize; 7])> = alien.into_iter().collect();
        rows.sort();
        let total = rows.iter().fold([0usize; 7], |mut t, (_, c)| {
            for i in 0..7 {
                t[i] += c[i];
            }
            t
        });
        println!(
            "alien lines {}: voiced after {} (own file {} + alien clip {}), silent {} (Silence {}, no cell {}, no clip {}); clip decode failures {}",
            total[0],
            total[1] + total[2],
            total[1],
            total[2],
            total[4] + total[5] + total[6],
            total[4],
            total[5],
            total[6],
            total[3]
        );
        println!("  (a lower bound: only an empty speaker tag counts as the owner here; named owners are checked at runtime)");
        // Which avo clips some alien line can reach, over every race and emotion.
        let races = tables.get(rm, "alienvo").map_or(0, |t| t.rows());
        let emotions = tables.get(rm, "emotion").map_or(0, |t| t.rows());
        let mut reach: HashSet<String> = HashSet::new();
        for row in 0..races {
            for e in 0..emotions {
                if let AlienSlot::Clips(clips) = alien_slot(&tables, rm, &index, row as u32, e as i32) {
                    reach.extend(clips);
                }
            }
        }
        let avo: Vec<&String> = index.voice.keys().filter(|k| k.starts_with("avo_")).collect();
        let unreachable = avo.iter().filter(|k| !reach.contains(k.as_str())).count();
        println!("avo clips in streamvoice: {}, reached by alienvo: {}, unreachable: {unreachable}", avo.len(), reach.len());
        for (m, c) in &rows {
            println!("  {m:>8}: lines {:>4}  voiced {:>4}  alien clip {:>4}  silent {:>3}  decode-fail {}", c[0], c[1] + c[2], c[2], c[4] + c[5] + c[6], c[3]);
        }
    }
    let mut v: Vec<_> = variants.into_iter().collect();
    v.sort();
    println!("audio survey: {} files + {} chitin wavs in {:?}, {:.1} h of audio", files.len(), keys.len(), t0.elapsed(), total.as_secs_f64() / 3600.0);
    for (k, n) in v {
        println!("  {n:>6} {k}");
    }
    println!("  {:>6} failures", fails.len());
    for f in fails.iter().take(40) {
        println!("    {f}");
    }
}

#[cfg(test)]
mod tests;

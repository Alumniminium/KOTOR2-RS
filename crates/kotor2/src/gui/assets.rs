//! GUI textures and KotOR bitmap fonts.
//!
//! A font is a TPC (or TGA) whose TXI lists, per character code 0..255,
//! `upperleftcoords` and `lowerrightcoords` in UV space with V = 1 at the
//! top of the texture. `fontheight` and `texturewidth` are in the same
//! units, 100 per texel at the native size (2.56 = a 256 px texture), so a
//! line is `fontheight * 100` layout pixels tall.

use std::collections::HashMap;
use std::sync::Arc;

use bevy::asset::RenderAssetUsages;
use bevy::image::{ImageAddressMode, ImageFilterMode, ImageSampler, ImageSamplerDescriptor};
use bevy::prelude::*;
use bevy::render::render_resource::{Extent3d, TextureDimension, TextureFormat};
use k2_res::{ResKey, Resources};
use rakata_core::ResourceType;

use crate::tpc;

#[derive(Clone)]
pub struct GuiTex {
    pub image: Handle<Image>,
    pub size: Vec2,
}

pub struct Glyph {
    /// Pixel rect in the texture.
    pub rect: Rect,
}

pub struct BitmapFont {
    pub tex: GuiTex,
    pub glyphs: Vec<Option<Glyph>>,
    /// Line height in layout pixels.
    pub height: f32,
    /// Extra advance after each glyph, layout pixels.
    pub spacing: f32,
}

impl BitmapFont {
    pub fn glyph(&self, c: char) -> Option<&Glyph> {
        let code = encode_1252(c)?;
        self.glyphs.get(code as usize)?.as_ref()
    }

    /// Advance width of a character in layout pixels.
    pub fn advance(&self, c: char) -> f32 {
        self.glyph(c).map_or(0.0, |g| g.rect.width() + self.spacing)
    }

    pub fn width(&self, s: &str) -> f32 {
        s.chars().map(|c| self.advance(c)).sum()
    }

    /// Greedy word wrap to `max` layout pixels; explicit newlines kept.
    pub fn wrap(&self, text: &str, max: f32) -> Vec<String> {
        let mut out = Vec::new();
        for para in text.split('\n') {
            let mut line = String::new();
            let mut w = 0.0;
            for word in para.split(' ') {
                let ww = self.width(word);
                let sp = if line.is_empty() { 0.0 } else { self.advance(' ') };
                if !line.is_empty() && max > 0.0 && w + sp + ww > max {
                    out.push(std::mem::take(&mut line));
                    w = 0.0;
                } else if !line.is_empty() {
                    line.push(' ');
                    w += sp;
                }
                line.push_str(word);
                w += ww;
            }
            out.push(line);
        }
        out
    }
}

/// Windows-1252, the game's codepage for English text.
fn encode_1252(c: char) -> Option<u8> {
    let u = c as u32;
    if u < 0x80 || (0xA0..=0xFF).contains(&u) {
        return Some(u as u8);
    }
    Some(match c {
        '\u{20AC}' => 0x80,
        '\u{2018}' => 0x91,
        '\u{2019}' => 0x92,
        '\u{201C}' => 0x93,
        '\u{201D}' => 0x94,
        '\u{2022}' => 0x95,
        '\u{2013}' => 0x96,
        '\u{2014}' => 0x97,
        '\u{2026}' => 0x85,
        _ => b'?',
    })
}

/// Cached GUI textures and fonts. `None` entries remember misses.
#[derive(Resource, Default)]
pub struct GuiAssets {
    textures: HashMap<String, Option<GuiTex>>,
    fonts: HashMap<String, Option<Arc<BitmapFont>>>,
}

/// Decodes a TPC (preferred) or TGA into a top-down RGBA8 base level.
/// TPC rows are stored bottom-up (OpenGL order; model UVs rely on that),
/// so GUI copies flip them; rakata already normalizes TGA to top-left.
fn decode(res: &Resources, name: &str) -> Option<tpc::Decoded> {
    let flip = |mut d: tpc::Decoded| {
        let row = (d.width * 4) as usize;
        let base = row * d.height as usize;
        d.data.truncate(base);
        let flipped: Vec<u8> = d.data.chunks_exact(row).rev().flatten().copied().collect();
        d.data = flipped;
        d.mips = 1;
        d
    };
    res.get(&ResKey::new(name, ResourceType::Tpc))
        .ok()
        .flatten()
        .and_then(|b| tpc::decode_tpc(&b))
        .map(flip)
        .or_else(|| res.get(&ResKey::new(name, ResourceType::Tga)).ok().flatten().and_then(|b| tpc::decode_tga(&b)))
}

fn make_image(d: &tpc::Decoded) -> Image {
    // Base level only: GUI art is drawn near its native size.
    let base = (d.width * d.height * 4) as usize;
    let mut img = Image::new(
        Extent3d { width: d.width, height: d.height, depth_or_array_layers: 1 },
        TextureDimension::D2,
        d.data[..base.min(d.data.len())].to_vec(),
        TextureFormat::Rgba8UnormSrgb,
        RenderAssetUsages::RENDER_WORLD,
    );
    img.sampler = ImageSampler::Descriptor(ImageSamplerDescriptor {
        address_mode_u: ImageAddressMode::ClampToEdge,
        address_mode_v: ImageAddressMode::ClampToEdge,
        mag_filter: ImageFilterMode::Linear,
        min_filter: ImageFilterMode::Linear,
        ..default()
    });
    img
}

impl GuiAssets {
    pub fn texture(&mut self, res: &Resources, images: &mut Assets<Image>, name: &str) -> Option<GuiTex> {
        let name = name.to_ascii_lowercase();
        if let Some(t) = self.textures.get(&name) {
            return t.clone();
        }
        let t = decode(res, &name).map(|d| GuiTex { size: Vec2::new(d.width as f32, d.height as f32), image: images.add(make_image(&d)) });
        if t.is_none() {
            debug!("gui: texture {name} not found");
        }
        self.textures.insert(name, t.clone());
        t
    }

    pub fn font(&mut self, res: &Resources, images: &mut Assets<Image>, name: &str) -> Option<Arc<BitmapFont>> {
        let name = name.to_ascii_lowercase();
        if let Some(f) = self.fonts.get(&name) {
            return f.clone();
        }
        let f = load_font(res, images, &name).map(Arc::new);
        if f.is_none() {
            warn!("gui: font {name} not found");
        }
        self.fonts.insert(name, f.clone());
        f
    }

    /// A font by name, falling back to the stock dialog font.
    pub fn font_or_default(&mut self, res: &Resources, images: &mut Assets<Image>, name: &str) -> Option<Arc<BitmapFont>> {
        if !name.is_empty()
            && let Some(f) = self.font(res, images, name)
        {
            return Some(f);
        }
        self.font(res, images, DEFAULT_FONT)
    }
}

pub const DEFAULT_FONT: &str = "dialogfont16x16";

fn load_font(res: &Resources, images: &mut Assets<Image>, name: &str) -> Option<BitmapFont> {
    let d = decode(res, name)?;
    let txi = res
        .get(&ResKey::new(name, ResourceType::Txi))
        .ok()
        .flatten()
        .map(|b| String::from_utf8_lossy(&b).into_owned())
        .unwrap_or_else(|| d.txi.clone());
    let (w, h) = (d.width as f32, d.height as f32);
    let mut height = 0.16;
    let mut spacing_r = 0.0;
    let mut ul: Vec<[f32; 2]> = Vec::new();
    let mut lr: Vec<[f32; 2]> = Vec::new();
    let mut lines = txi.lines().map(str::trim);
    while let Some(line) = lines.next() {
        let mut it = line.split_whitespace();
        let key = it.next().unwrap_or("").to_ascii_lowercase();
        let num = |it: &mut std::str::SplitWhitespace| it.next().and_then(|v| v.parse::<f32>().ok());
        match key.as_str() {
            "fontheight" => height = num(&mut it).unwrap_or(height),
            "spacingr" => spacing_r = num(&mut it).unwrap_or(0.0),
            "upperleftcoords" | "lowerrightcoords" => {
                let n = num(&mut it).unwrap_or(0.0) as usize;
                let v: Vec<[f32; 2]> = lines
                    .by_ref()
                    .take(n)
                    .map(|l| {
                        let mut p = l.split_whitespace().filter_map(|x| x.parse::<f32>().ok());
                        [p.next().unwrap_or(0.0), p.next().unwrap_or(0.0)]
                    })
                    .collect();
                if key == "upperleftcoords" {
                    ul = v;
                } else {
                    lr = v;
                }
            }
            _ => {}
        }
    }
    if ul.is_empty() || lr.is_empty() {
        return None;
    }
    let glyphs = ul
        .iter()
        .zip(&lr)
        .map(|(a, b)| {
            // V = 1 is the top row.
            let rect = Rect::new(a[0] * w, (1.0 - a[1]) * h, b[0] * w, (1.0 - b[1]) * h);
            (rect.width() > 0.0 && rect.height() > 0.0).then_some(Glyph { rect })
        })
        .collect();
    let tex = GuiTex { size: Vec2::new(w, h), image: images.add(make_image(&d)) };
    Some(BitmapFont { tex, glyphs, height: (height * 100.0).max(1.0), spacing: spacing_r * 100.0 })
}

//! Lenient TPC and TGA decoding into RGBA8 mip chains.
//!
//! TPC header (128 bytes): data_size u32 (0 = uncompressed, else byte size of
//! the base level), alpha_test f32, width u16, height u16, pixel_type u8
//! (1 grey, 2 RGB/DXT1, 4 RGBA/DXT5), mip_count u8. Payload is the mip chain,
//! per layer for cube maps (height == 6 * width). Anything after the payload
//! is TXI text.
//!
//! Shipped data has truncated chains (script_c/script_h), so we keep every
//! complete mip that is present and zero-fill a short base level rather than
//! fail.

/// Decoded texture: RGBA8, mips concatenated largest first.
pub struct Decoded {
    pub width: u32,
    pub height: u32,
    pub mips: u32,
    pub data: Vec<u8>,
    /// True when any alpha < 255 appears in the base level.
    pub has_alpha: bool,
    pub txi: String,
}

#[derive(Clone, Copy, PartialEq)]
enum Fmt {
    Grey,
    Rgb,
    Rgba,
    Dxt1,
    Dxt5,
}

impl Fmt {
    fn level_size(self, w: u32, h: u32) -> usize {
        let (w, h) = (w.max(1) as usize, h.max(1) as usize);
        match self {
            Fmt::Grey => w * h,
            Fmt::Rgb => w * h * 3,
            Fmt::Rgba => w * h * 4,
            Fmt::Dxt1 => w.div_ceil(4) * h.div_ceil(4) * 8,
            Fmt::Dxt5 => w.div_ceil(4) * h.div_ceil(4) * 16,
        }
    }
}

fn u16_at(b: &[u8], o: usize) -> u16 {
    u16::from_le_bytes([b[o], b[o + 1]])
}

fn u32_at(b: &[u8], o: usize) -> u32 {
    u32::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]])
}

pub fn decode_tpc(bytes: &[u8]) -> Option<Decoded> {
    if bytes.len() < 128 {
        return None;
    }
    let data_size = u32_at(bytes, 0);
    let width = u32::from(u16_at(bytes, 8));
    let mut height = u32::from(u16_at(bytes, 10));
    let pixel_type = bytes[12];
    let mip_count = u32::from(bytes[13]).max(1);
    let fmt = match (data_size != 0, pixel_type) {
        (false, 1) => Fmt::Grey,
        (false, 2) => Fmt::Rgb,
        (false, 4) => Fmt::Rgba,
        (true, 2) => Fmt::Dxt1,
        (true, 4) => Fmt::Dxt5,
        // Some compressed greyscale headers exist in the wild; treat as DXT1.
        (true, _) => Fmt::Dxt1,
        _ => return None,
    };
    if width == 0 || height == 0 {
        return None;
    }
    // Cube map: keep only the first face.
    let layers = if data_size != 0 && height == width * 6 { 6 } else { 1 };
    if layers == 6 {
        height = width;
    }

    let payload = &bytes[128..];
    let mut levels: Vec<(u32, u32, &[u8])> = Vec::new();
    let mut off = 0usize;
    let (mut w, mut h) = (width, height);
    for _ in 0..mip_count {
        let size = fmt.level_size(w, h);
        if off + size > payload.len() {
            break;
        }
        levels.push((w, h, &payload[off..off + size]));
        off += size;
        if w == 1 && h == 1 {
            break;
        }
        w = (w / 2).max(1);
        h = (h / 2).max(1);
    }
    // Skip remaining mips of face 0 and other faces to reach the TXI footer.
    let mut chain = 0usize;
    let (mut w2, mut h2) = (width, height);
    for _ in 0..mip_count {
        chain += fmt.level_size(w2, h2);
        w2 = (w2 / 2).max(1);
        h2 = (h2 / 2).max(1);
    }
    let txi_start = chain * layers;
    let txi = payload
        .get(txi_start..)
        .map(|t| String::from_utf8_lossy(t).trim_matches(char::from(0)).to_owned())
        .unwrap_or_default();

    let short_base;
    if levels.is_empty() {
        // Not even a full base level: zero-pad what is there.
        let mut v = payload.to_vec();
        v.resize(fmt.level_size(width, height), 0);
        short_base = v;
        levels.push((width, height, &short_base));
    }

    let mut data = Vec::new();
    for &(w, h, src) in &levels {
        data.extend_from_slice(&to_rgba(fmt, w, h, src));
    }
    let base_len = (width * height * 4) as usize;
    let has_alpha = matches!(fmt, Fmt::Rgba | Fmt::Dxt5) && data[..base_len].chunks_exact(4).any(|p| p[3] < 250);
    Some(Decoded { width, height, mips: levels.len() as u32, data, has_alpha, txi })
}

fn to_rgba(fmt: Fmt, w: u32, h: u32, src: &[u8]) -> Vec<u8> {
    let n = (w * h) as usize;
    match fmt {
        Fmt::Grey => src.iter().flat_map(|&g| [g, g, g, 255]).collect(),
        Fmt::Rgb => src.chunks_exact(3).flat_map(|p| [p[0], p[1], p[2], 255]).collect(),
        Fmt::Rgba => src[..n * 4].to_vec(),
        Fmt::Dxt1 | Fmt::Dxt5 => {
            let tf = if fmt == Fmt::Dxt1 { texpresso::Format::Bc1 } else { texpresso::Format::Bc3 };
            let mut out = vec![0u8; n * 4];
            tf.decompress(src, w as usize, h as usize, &mut out);
            if fmt == Fmt::Dxt1 {
                // KotOR DXT1 is opaque; ignore 1-bit punch-through alpha.
                for p in out.chunks_exact_mut(4) {
                    p[3] = 255;
                }
            }
            out
        }
    }
}

/// TGA via rakata (rgba_pixels are normalized to top-left origin).
pub fn decode_tga(bytes: &[u8]) -> Option<Decoded> {
    let tga = rakata_formats::tga::read_tga_from_bytes(bytes).ok()?;
    let (width, height) = (u32::from(tga.header.width), u32::from(tga.header.height));
    let data = tga.rgba_pixels;
    if data.len() != (width * height * 4) as usize || width == 0 {
        return None;
    }
    let has_alpha = data.chunks_exact(4).any(|p| p[3] < 250);
    Some(Decoded { width, height, mips: 1, data, has_alpha, txi: String::new() })
}

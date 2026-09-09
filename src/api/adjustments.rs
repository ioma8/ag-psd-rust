//! Adjustment layer binary data parsing and serialisation.

use crate::support::error::{PsdError, Result};

// ── Lab colour helpers ────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq)]
pub struct LabColor {
    pub lmnc: f64,
    pub a: f64,
    pub b: f64,
}

fn xyz_to_lab(x: f64, y: f64, z: f64) -> LabColor {
    let epsilon = 0.008856_f64;
    let kappa = 903.3_f64;
    let f = |t: f64| {
        if t > epsilon {
            t.cbrt()
        } else {
            (kappa * t + 16.0) / 116.0
        }
    };
    let fx = f(x);
    let fy = f(y);
    let fz = f(z);
    LabColor {
        lmnc: 116.0 * fy - 16.0,
        a: 500.0 * (fx - fy),
        b: 200.0 * (fy - fz),
    }
}

// ── Simple adjustment structs ─────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq)]
pub struct BrightnessContrast {
    pub brightness: i16,
    pub contrast: i16,
    pub use_legacy: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Posterize {
    pub levels: u16,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Threshold {
    pub level: u16,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Exposure {
    pub exposure: f32,
    pub offset: f32,
    pub gamma_correction: f32,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ColorBalance {
    pub shadows: [i16; 3],
    pub midtones: [i16; 3],
    pub highlights: [i16; 3],
    pub preserve_luminosity: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct PhotoFilter {
    pub color: LabColor,
    pub density: u32,
    pub preserve_luminosity: bool,
}

// ── Levels ────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq)]
pub struct LevelsChannel {
    pub input_black: u16,
    pub input_white: u16,
    pub output_black: u16,
    pub output_white: u16,
    pub gamma: u16,
}

impl Default for LevelsChannel {
    fn default() -> Self {
        LevelsChannel {
            input_black: 0,
            input_white: 255,
            output_black: 0,
            output_white: 255,
            gamma: 100,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Levels {
    pub channels: Vec<LevelsChannel>,
}

// ── Hue/Saturation ────────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq)]
pub struct HueSatRange {
    pub range: [i16; 4],
    pub adjust: [i16; 3],
}

#[derive(Debug, Clone, PartialEq)]
pub struct HueSaturation {
    pub colorize: bool,
    pub colorized_master: [i16; 3],
    pub master: [i16; 3],
    pub ranges: Vec<HueSatRange>,
}

// ── Selective Colour ──────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq)]
pub struct SelectiveColor {
    pub absolute: bool,
    pub adjustments: Vec<[i16; 4]>,
}

// ── Channel Mixer ─────────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq)]
pub struct ChannelMixer {
    pub monochrome: bool,
    pub values: Vec<i16>,
}

// ── Curves ────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq)]
pub enum CurvesChannel {
    Points(Vec<u16>),
    Mapping(Vec<u8>),
}

#[derive(Debug, Clone, PartialEq)]
pub struct Curves {
    pub mode: u8,
    pub channels: Vec<CurvesChannel>,
}

// ── Gradient Map ──────────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq)]
pub struct GradientColorStop {
    pub location: u32,
    pub midpoint: u32,
    pub color: [u8; 3],
}

#[derive(Debug, Clone, PartialEq)]
pub struct GradientTransparencyStop {
    pub location: u32,
    pub midpoint: u32,
    pub opacity: u16,
}

#[derive(Debug, Clone, PartialEq)]
pub struct GradientDefinition {
    pub name: String,
    pub interpolation: u16,
    pub color_stops: Vec<GradientColorStop>,
    pub transparency_stops: Vec<GradientTransparencyStop>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct GradientMap {
    pub reverse: bool,
    pub dither: bool,
    pub interpolation_method_type: String,
    pub gradient: GradientDefinition,
}

// ── AdjustmentLayer enum ──────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq)]
pub enum AdjustmentLayer {
    BrightnessContrast(BrightnessContrast),
    Invert,
    Posterize(Posterize),
    Threshold(Threshold),
    Exposure(Exposure),
    ColorBalance(ColorBalance),
    PhotoFilter(PhotoFilter),
    Levels(Levels),
    HueSaturation(HueSaturation),
    SelectiveColor(SelectiveColor),
    ChannelMixer(ChannelMixer),
    Curves(Curves),
    GradientMap(GradientMap),
    /// Fallback for adjustment keys with formats not yet decoded (e.g. blwh)
    Raw {
        key: String,
        data: Vec<u8>,
    },
}

impl AdjustmentLayer {
    pub fn key(&self) -> &str {
        match self {
            AdjustmentLayer::BrightnessContrast(_) => "brit",
            AdjustmentLayer::Invert => "nvrt",
            AdjustmentLayer::Posterize(_) => "post",
            AdjustmentLayer::Threshold(_) => "thrs",
            AdjustmentLayer::Exposure(_) => "expA",
            AdjustmentLayer::ColorBalance(_) => "blnc",
            AdjustmentLayer::PhotoFilter(_) => "phfl",
            AdjustmentLayer::Levels(_) => "levl",
            AdjustmentLayer::HueSaturation(_) => "hue2",
            AdjustmentLayer::SelectiveColor(_) => "selc",
            AdjustmentLayer::ChannelMixer(_) => "mixr",
            AdjustmentLayer::Curves(_) => "curv",
            AdjustmentLayer::GradientMap(_) => "grdm",
            AdjustmentLayer::Raw { key, .. } => key.as_str(),
        }
    }

    pub fn from_key_and_bytes(key: &str, bytes: &[u8]) -> Result<Self> {
        match key {
            "brit" => Ok(AdjustmentLayer::BrightnessContrast(
                read_brightness_contrast(bytes)?,
            )),
            "nvrt" => Ok(AdjustmentLayer::Invert),
            "post" => Ok(AdjustmentLayer::Posterize(read_posterize(bytes)?)),
            "thrs" => Ok(AdjustmentLayer::Threshold(read_threshold(bytes)?)),
            "expA" => Ok(AdjustmentLayer::Exposure(read_exposure(bytes)?)),
            "blnc" => Ok(AdjustmentLayer::ColorBalance(read_color_balance(bytes)?)),
            "phfl" => Ok(AdjustmentLayer::PhotoFilter(read_photo_filter(bytes)?)),
            "levl" => Ok(AdjustmentLayer::Levels(read_levels(bytes)?)),
            "hue2" => Ok(AdjustmentLayer::HueSaturation(read_hue_saturation(bytes)?)),
            "selc" => Ok(AdjustmentLayer::SelectiveColor(read_selective_color(
                bytes,
            )?)),
            "mixr" => Ok(AdjustmentLayer::ChannelMixer(read_channel_mixer(bytes)?)),
            "curv" => Ok(AdjustmentLayer::Curves(read_curves(bytes)?)),
            "grdm" => Ok(AdjustmentLayer::GradientMap(read_gradient_map(bytes)?)),
            _ => Ok(AdjustmentLayer::Raw {
                key: key.to_string(),
                data: bytes.to_vec(),
            }),
        }
    }

    pub fn to_bytes(&self) -> Result<Vec<u8>> {
        match self {
            AdjustmentLayer::BrightnessContrast(v) => write_brightness_contrast(v),
            AdjustmentLayer::Invert => Ok(Vec::new()),
            AdjustmentLayer::Posterize(v) => write_posterize(v),
            AdjustmentLayer::Threshold(v) => write_threshold(v),
            AdjustmentLayer::Exposure(v) => write_exposure(v),
            AdjustmentLayer::ColorBalance(v) => write_color_balance(v),
            AdjustmentLayer::PhotoFilter(v) => write_photo_filter(v),
            AdjustmentLayer::Levels(v) => write_levels(v),
            AdjustmentLayer::HueSaturation(v) => write_hue_saturation(v),
            AdjustmentLayer::SelectiveColor(v) => write_selective_color(v),
            AdjustmentLayer::ChannelMixer(v) => write_channel_mixer(v),
            AdjustmentLayer::Curves(v) => write_curves(v),
            AdjustmentLayer::GradientMap(v) => write_gradient_map(v),
            AdjustmentLayer::Raw { data, .. } => Ok(data.clone()),
        }
    }
}

// ── Tiny cursor helpers ───────────────────────────────────────────────────────

struct Cursor<'a> {
    data: &'a [u8],
    pos: usize,
}

impl<'a> Cursor<'a> {
    fn new(data: &'a [u8]) -> Self {
        Cursor { data, pos: 0 }
    }

    fn read_u8(&mut self) -> Result<u8> {
        if self.pos >= self.data.len() {
            return Err(PsdError::InvalidFormat(
                "unexpected EOF in adjustment".into(),
            ));
        }
        let v = self.data[self.pos];
        self.pos += 1;
        Ok(v)
    }

    fn read_u16(&mut self) -> Result<u16> {
        if self.pos + 2 > self.data.len() {
            return Err(PsdError::InvalidFormat(
                "unexpected EOF in adjustment".into(),
            ));
        }
        let v = u16::from_be_bytes([self.data[self.pos], self.data[self.pos + 1]]);
        self.pos += 2;
        Ok(v)
    }

    fn read_i16(&mut self) -> Result<i16> {
        Ok(self.read_u16()? as i16)
    }

    fn read_u32(&mut self) -> Result<u32> {
        if self.pos + 4 > self.data.len() {
            return Err(PsdError::InvalidFormat(
                "unexpected EOF in adjustment".into(),
            ));
        }
        let v = u32::from_be_bytes([
            self.data[self.pos],
            self.data[self.pos + 1],
            self.data[self.pos + 2],
            self.data[self.pos + 3],
        ]);
        self.pos += 4;
        Ok(v)
    }

    fn read_f32(&mut self) -> Result<f32> {
        Ok(f32::from_bits(self.read_u32()?))
    }

    fn read_ascii(&mut self, len: usize) -> Result<String> {
        if self.pos + len > self.data.len() {
            return Err(PsdError::InvalidFormat(
                "unexpected EOF in adjustment".into(),
            ));
        }
        let s = String::from_utf8_lossy(&self.data[self.pos..self.pos + len]).to_string();
        self.pos += len;
        Ok(s)
    }

    fn read_unicode_string(&mut self) -> Result<String> {
        let len = self.read_u32()? as usize;
        let mut units: Vec<u16> = Vec::with_capacity(len);
        for _ in 0..len {
            units.push(self.read_u16()?);
        }
        Ok(String::from_utf16_lossy(&units))
    }

    fn remaining(&self) -> usize {
        self.data.len() - self.pos
    }
}

struct Writer {
    buf: Vec<u8>,
}

impl Writer {
    fn new() -> Self {
        Writer { buf: Vec::new() }
    }

    fn write_u8(&mut self, v: u8) {
        self.buf.push(v);
    }

    fn write_u16(&mut self, v: u16) {
        self.buf.extend_from_slice(&v.to_be_bytes());
    }

    fn write_i16(&mut self, v: i16) {
        self.write_u16(v as u16);
    }

    fn write_u32(&mut self, v: u32) {
        self.buf.extend_from_slice(&v.to_be_bytes());
    }

    fn write_f32(&mut self, v: f32) {
        self.write_u32(v.to_bits());
    }

    fn write_ascii(&mut self, s: &str) {
        self.buf.extend_from_slice(s.as_bytes());
    }

    fn write_zeros(&mut self, n: usize) {
        self.buf.extend(std::iter::repeat(0u8).take(n));
    }

    fn write_unicode_string(&mut self, s: &str) {
        let units: Vec<u16> = s.encode_utf16().collect();
        self.write_u32(units.len() as u32);
        for unit in units {
            self.write_u16(unit);
        }
    }

    fn into_vec(self) -> Vec<u8> {
        self.buf
    }
}

// ── Readers ───────────────────────────────────────────────────────────────────

fn read_brightness_contrast(bytes: &[u8]) -> Result<BrightnessContrast> {
    let mut c = Cursor::new(bytes);
    let brightness = c.read_i16()?;
    let contrast = c.read_i16()?;
    Ok(BrightnessContrast {
        brightness,
        contrast,
        use_legacy: true,
    })
}

fn read_posterize(bytes: &[u8]) -> Result<Posterize> {
    let mut c = Cursor::new(bytes);
    Ok(Posterize {
        levels: c.read_u16()?,
    })
}

fn read_threshold(bytes: &[u8]) -> Result<Threshold> {
    let mut c = Cursor::new(bytes);
    Ok(Threshold {
        level: c.read_u16()?,
    })
}

fn read_exposure(bytes: &[u8]) -> Result<Exposure> {
    let mut c = Cursor::new(bytes);
    let _ = c.read_u16()?; // skip
    let exposure = c.read_f32()?;
    let offset = c.read_f32()?;
    let gamma_correction = c.read_f32()?;
    Ok(Exposure {
        exposure,
        offset,
        gamma_correction,
    })
}

fn read_color_balance(bytes: &[u8]) -> Result<ColorBalance> {
    let mut c = Cursor::new(bytes);
    let shadows = [c.read_i16()?, c.read_i16()?, c.read_i16()?];
    let midtones = [c.read_i16()?, c.read_i16()?, c.read_i16()?];
    let highlights = [c.read_i16()?, c.read_i16()?, c.read_i16()?];
    let preserve_luminosity = c.read_u8()? == 1;
    Ok(ColorBalance {
        shadows,
        midtones,
        highlights,
        preserve_luminosity,
    })
}

fn read_photo_filter(bytes: &[u8]) -> Result<PhotoFilter> {
    let mut c = Cursor::new(bytes);
    let mode = c.read_u16()?;
    let color = if mode == 3 {
        let scale = 32768.0_f64;
        let x = c.read_u32()? as f64 / scale;
        let y = c.read_u32()? as f64 / scale;
        let z = c.read_u32()? as f64 / scale;
        xyz_to_lab(x, y, z)
    } else if mode == 2 {
        let _color_space = c.read_u16()?; // expect 7 (Lab)
        let lmnc = c.read_i16()? as f64 / 100.0;
        let a = c.read_i16()? as f64 / 100.0;
        let b = c.read_i16()? as f64 / 100.0;
        let _ = c.read_u16()?;
        LabColor { lmnc, a, b }
    } else {
        LabColor {
            lmnc: 0.0,
            a: 0.0,
            b: 0.0,
        }
    };
    let density = c.read_u32()?;
    let preserve_luminosity = c.read_u8()? == 1;
    Ok(PhotoFilter {
        color,
        density,
        preserve_luminosity,
    })
}

fn read_levels(bytes: &[u8]) -> Result<Levels> {
    let mut c = Cursor::new(bytes);
    let _ = c.read_u16()?; // version

    let mut channels = Vec::with_capacity(29);
    for _ in 0..29 {
        channels.push(LevelsChannel {
            input_black: c.read_u16()?,
            input_white: c.read_u16()?,
            output_black: c.read_u16()?,
            output_white: c.read_u16()?,
            gamma: c.read_u16()?,
        });
    }

    if c.remaining() > 0 {
        let _ = c.read_ascii(4)?; // "Lvls"
        let _ = c.read_u16()?; // version 3
        let count = c.read_u16()? as usize;
        for _ in 29..count {
            channels.push(LevelsChannel {
                input_black: c.read_u16()?,
                input_white: c.read_u16()?,
                output_black: c.read_u16()?,
                output_white: c.read_u16()?,
                gamma: c.read_u16()?,
            });
        }
    }

    // Only expose first 4 channels to callers
    channels.truncate(4);
    Ok(Levels { channels })
}

fn read_hue_saturation(bytes: &[u8]) -> Result<HueSaturation> {
    let mut c = Cursor::new(bytes);
    let _ = c.read_u16()?; // version
    let colorize = c.read_u8()? == 1;
    let _ = c.read_u8()?;
    let colorized_master = [c.read_i16()?, c.read_i16()?, c.read_i16()?];
    let master = [c.read_i16()?, c.read_i16()?, c.read_i16()?];

    let mut ranges = Vec::with_capacity(6);
    for _ in 0..6 {
        let range = [c.read_i16()?, c.read_i16()?, c.read_i16()?, c.read_i16()?];
        let adjust = [c.read_i16()?, c.read_i16()?, c.read_i16()?];
        ranges.push(HueSatRange { range, adjust });
    }

    Ok(HueSaturation {
        colorize,
        colorized_master,
        master,
        ranges,
    })
}

fn read_selective_color(bytes: &[u8]) -> Result<SelectiveColor> {
    let mut c = Cursor::new(bytes);
    let _ = c.read_u16()?; // version
    let absolute = c.read_u16()? == 1;

    // 4 skipped i16 values
    for _ in 0..4 {
        let _ = c.read_i16()?;
    }

    let mut adjustments = Vec::with_capacity(9);
    for _ in 0..9 {
        adjustments.push([c.read_i16()?, c.read_i16()?, c.read_i16()?, c.read_i16()?]);
    }

    Ok(SelectiveColor {
        absolute,
        adjustments,
    })
}

fn read_channel_mixer(bytes: &[u8]) -> Result<ChannelMixer> {
    let mut c = Cursor::new(bytes);
    let _ = c.read_u16()?; // version
    let monochrome = c.read_u16()? == 1;
    let mut values = Vec::with_capacity(20);
    for _ in 0..20 {
        values.push(c.read_i16()?);
    }
    Ok(ChannelMixer { monochrome, values })
}

fn read_curves(bytes: &[u8]) -> Result<Curves> {
    let mut c = Cursor::new(bytes);
    let mode = c.read_u8()?;
    let _ = c.read_u8()?;
    let _ = c.read_u8()?;
    let bitmask = c.read_u32()?;

    let mut channels = Vec::with_capacity(4);
    for i in 0..4usize {
        let enabled = ((bitmask >> i) & 1) == 1;
        if !enabled {
            let ch = if mode == 0 {
                CurvesChannel::Points(vec![0, 0, 255, 255])
            } else {
                CurvesChannel::Mapping((0u8..=255).collect())
            };
            channels.push(ch);
            continue;
        }

        if mode == 0 {
            let count = c.read_u16()? as usize;
            let mut points = Vec::with_capacity(count * 2);
            for _ in 0..count {
                let y = c.read_u16()?;
                let x = c.read_u16()?;
                points.push(x);
                points.push(y);
            }
            channels.push(CurvesChannel::Points(points));
        } else {
            let mut mapping = Vec::with_capacity(256);
            for _ in 0..256 {
                mapping.push(c.read_u8()?);
            }
            channels.push(CurvesChannel::Mapping(mapping));
        }
    }

    Ok(Curves { mode, channels })
}

fn read_gradient_definition(c: &mut Cursor) -> Result<GradientDefinition> {
    let color_stop_count = c.read_u16()? as usize;
    let mut color_stops = Vec::with_capacity(color_stop_count);
    for _ in 0..color_stop_count {
        let location = c.read_u32()?;
        let midpoint = c.read_u32()?;
        let _ = c.read_u16()?; // colour space tag
        let r = ((c.read_u16()? as f64 / 65535.0) * 255.0).round() as u8;
        let g = ((c.read_u16()? as f64 / 65535.0) * 255.0).round() as u8;
        let b = ((c.read_u16()? as f64 / 65535.0) * 255.0).round() as u8;
        let _ = c.read_u16()?; // padding
        color_stops.push(GradientColorStop {
            location,
            midpoint,
            color: [r, g, b],
        });
    }

    let trans_count = c.read_u16()? as usize;
    let mut transparency_stops = Vec::with_capacity(trans_count);
    for _ in 0..trans_count {
        let location = c.read_u32()?;
        let midpoint = c.read_u32()?;
        let raw_opacity = c.read_u16()?;
        let opacity = ((raw_opacity as u32 * 100 + 127) / 255) as u16;
        transparency_stops.push(GradientTransparencyStop {
            location,
            midpoint,
            opacity,
        });
    }

    let _ = c.read_u16()?; // pad
    let interpolation = c.read_u16()?;
    let _ = c.read_u16()?; // pad

    Ok(GradientDefinition {
        name: String::new(),
        interpolation,
        color_stops,
        transparency_stops,
    })
}

fn read_gradient_map(bytes: &[u8]) -> Result<GradientMap> {
    let mut c = Cursor::new(bytes);
    let _ = c.read_u16()?; // version
    let reverse = c.read_u8()? == 1;
    let dither = c.read_u8()? == 1;

    let method_raw = c.read_ascii(4)?;
    let interpolation_method_type = if method_raw == "Lnr " {
        "Lnr".to_string()
    } else if method_raw.starts_with('\0') {
        "stripes".to_string()
    } else {
        let t = method_raw.trim().to_string();
        if t.is_empty() {
            "Gcls".to_string()
        } else {
            t
        }
    };

    let name = c.read_unicode_string()?;
    let mut gradient = read_gradient_definition(&mut c)?;
    gradient.name = name;

    // Skip trailing fields
    let _ = c.read_u16()?;
    let _ = c.read_u32()?;
    let _ = c.read_u16()?;
    let _ = c.read_u16()?;
    let _ = c.read_u32()?;
    let _ = c.read_u16()?;
    // 8 + 8 bytes
    for _ in 0..8 {
        let _ = c.read_u16()?;
    }
    let _ = c.read_u16()?;

    Ok(GradientMap {
        reverse,
        dither,
        interpolation_method_type,
        gradient,
    })
}

// ── Writers ───────────────────────────────────────────────────────────────────

fn write_brightness_contrast(_v: &BrightnessContrast) -> Result<Vec<u8>> {
    // TS writer emits an 8-byte zero-filled legacy block
    Ok(vec![0u8; 8])
}

fn write_posterize(v: &Posterize) -> Result<Vec<u8>> {
    let mut w = Writer::new();
    w.write_u16(v.levels);
    Ok(w.into_vec())
}

fn write_threshold(v: &Threshold) -> Result<Vec<u8>> {
    let mut w = Writer::new();
    w.write_u16(v.level);
    Ok(w.into_vec())
}

fn write_exposure(v: &Exposure) -> Result<Vec<u8>> {
    let mut w = Writer::new();
    w.write_u16(1);
    w.write_f32(v.exposure);
    w.write_f32(v.offset);
    w.write_f32(v.gamma_correction);
    Ok(w.into_vec())
}

fn write_color_balance(v: &ColorBalance) -> Result<Vec<u8>> {
    let mut w = Writer::new();
    for &x in &v.shadows {
        w.write_i16(x);
    }
    for &x in &v.midtones {
        w.write_i16(x);
    }
    for &x in &v.highlights {
        w.write_i16(x);
    }
    w.write_u8(if v.preserve_luminosity { 1 } else { 0 });
    Ok(w.into_vec())
}

fn write_photo_filter(v: &PhotoFilter) -> Result<Vec<u8>> {
    let mut w = Writer::new();
    // Always write mode 2 (Lab) on output, matching TS writer
    w.write_u16(2);
    w.write_u16(7); // Lab colour space
    w.write_i16((v.color.lmnc * 100.0).round() as i16);
    w.write_i16((v.color.a * 100.0).round() as i16);
    w.write_i16((v.color.b * 100.0).round() as i16);
    w.write_u16(0);
    w.write_u32(v.density);
    w.write_u8(if v.preserve_luminosity { 1 } else { 0 });
    Ok(w.into_vec())
}

fn write_levels(v: &Levels) -> Result<Vec<u8>> {
    let mut w = Writer::new();
    w.write_u16(2); // version

    let mut channels = v.channels.clone();
    while channels.len() < 29 {
        channels.push(LevelsChannel::default());
    }

    for ch in channels.iter().take(29) {
        w.write_u16(ch.input_black);
        w.write_u16(ch.input_white);
        w.write_u16(ch.output_black);
        w.write_u16(ch.output_white);
        w.write_u16(ch.gamma);
    }

    if channels.len() > 29 {
        w.write_ascii("Lvls");
        w.write_u16(3);
        w.write_u16(channels.len() as u16);
        for ch in channels.iter().skip(29) {
            w.write_u16(ch.input_black);
            w.write_u16(ch.input_white);
            w.write_u16(ch.output_black);
            w.write_u16(ch.output_white);
            w.write_u16(ch.gamma);
        }
    }

    Ok(w.into_vec())
}

fn write_hue_saturation(v: &HueSaturation) -> Result<Vec<u8>> {
    let mut w = Writer::new();
    w.write_u16(2);
    w.write_u8(if v.colorize { 1 } else { 0 });
    w.write_u8(0);
    for &x in &v.colorized_master {
        w.write_i16(x);
    }
    for &x in &v.master {
        w.write_i16(x);
    }
    for r in &v.ranges {
        for &x in &r.range {
            w.write_i16(x);
        }
        for &x in &r.adjust {
            w.write_i16(x);
        }
    }
    Ok(w.into_vec())
}

fn write_selective_color(v: &SelectiveColor) -> Result<Vec<u8>> {
    let mut w = Writer::new();
    w.write_u16(1);
    w.write_u16(if v.absolute { 1 } else { 0 });
    for _ in 0..4 {
        w.write_i16(0);
    }
    for adj in &v.adjustments {
        for &x in adj {
            w.write_i16(x);
        }
    }
    Ok(w.into_vec())
}

fn write_channel_mixer(v: &ChannelMixer) -> Result<Vec<u8>> {
    let mut w = Writer::new();
    w.write_u16(1);
    w.write_u16(if v.monochrome { 1 } else { 0 });
    for i in 0..20usize {
        w.write_i16(v.values.get(i).copied().unwrap_or(0));
    }
    Ok(w.into_vec())
}

fn write_curves(v: &Curves) -> Result<Vec<u8>> {
    let mut w = Writer::new();
    w.write_u8(v.mode);
    w.write_u8(0);
    w.write_u8(1);
    w.write_u32(15); // bitmask: all 4 channels enabled

    for ch in &v.channels {
        if v.mode == 0 {
            let points = match ch {
                CurvesChannel::Points(p) => p.clone(),
                CurvesChannel::Mapping(_) => vec![0, 0, 255, 255],
            };
            w.write_u16((points.len() / 2) as u16);
            for i in (0..points.len()).step_by(2) {
                w.write_u16(points[i + 1]); // y
                w.write_u16(points[i]); // x
            }
        } else {
            let mapping: Vec<u8> = match ch {
                CurvesChannel::Mapping(m) => m.clone(),
                CurvesChannel::Points(_) => (0u8..=255).collect(),
            };
            for b in &mapping {
                w.write_u8(*b);
            }
        }
    }
    Ok(w.into_vec())
}

fn write_gradient_definition(w: &mut Writer, g: &GradientDefinition) {
    w.write_u16(g.color_stops.len() as u16);
    for stop in &g.color_stops {
        w.write_u32(stop.location);
        w.write_u32(stop.midpoint);
        w.write_u16(0); // colour space tag
        w.write_u16(((stop.color[0] as f64 / 255.0) * 65535.0).round() as u16);
        w.write_u16(((stop.color[1] as f64 / 255.0) * 65535.0).round() as u16);
        w.write_u16(((stop.color[2] as f64 / 255.0) * 65535.0).round() as u16);
        w.write_u16(0); // padding
    }
    w.write_u16(g.transparency_stops.len() as u16);
    for stop in &g.transparency_stops {
        w.write_u32(stop.location);
        w.write_u32(stop.midpoint);
        w.write_u16(((stop.opacity as u32 * 255 + 50) / 100) as u16);
    }
    w.write_u16(2);
    w.write_u16(g.interpolation);
    w.write_u16(32);
}

fn write_gradient_map(v: &GradientMap) -> Result<Vec<u8>> {
    let mut w = Writer::new();
    w.write_u16(3);
    w.write_u8(if v.reverse { 1 } else { 0 });
    w.write_u8(if v.dither { 1 } else { 0 });

    let method = match v.interpolation_method_type.as_str() {
        "Lnr" => "Lnr ".to_string(),
        "stripes" => "\x00\x00\x0cm".to_string(),
        s => format!("{:<4}", s),
    };
    w.write_ascii(&method[..4]);

    w.write_unicode_string(&v.gradient.name);
    write_gradient_definition(&mut w, &v.gradient);

    // Trailing padding matching TS writer
    w.write_u16(1);
    w.write_u32(2048);
    w.write_u16(0);
    w.write_u16(0);
    w.write_u32(0);
    w.write_u16(3);
    w.write_zeros(8);
    for _ in 0..4 {
        w.write_u16(32768);
    }
    w.write_u16(0);

    Ok(w.into_vec())
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn roundtrip(adj: AdjustmentLayer) {
        let bytes = adj.to_bytes().expect("to_bytes failed");
        let key = adj.key();
        let recovered =
            AdjustmentLayer::from_key_and_bytes(key, &bytes).expect("from_key_and_bytes failed");
        assert_eq!(adj, recovered, "roundtrip failed for key={}", key);
    }

    #[test]
    fn test_invert_roundtrip() {
        roundtrip(AdjustmentLayer::Invert);
    }

    #[test]
    fn test_posterize_roundtrip() {
        roundtrip(AdjustmentLayer::Posterize(Posterize { levels: 4 }));
    }

    #[test]
    fn test_threshold_roundtrip() {
        roundtrip(AdjustmentLayer::Threshold(Threshold { level: 128 }));
    }

    #[test]
    fn test_exposure_roundtrip() {
        roundtrip(AdjustmentLayer::Exposure(Exposure {
            exposure: 0.5,
            offset: -0.01,
            gamma_correction: 1.0,
        }));
    }

    #[test]
    fn test_color_balance_roundtrip() {
        roundtrip(AdjustmentLayer::ColorBalance(ColorBalance {
            shadows: [10, -5, 3],
            midtones: [0, 0, 0],
            highlights: [-3, 7, -1],
            preserve_luminosity: true,
        }));
    }

    #[test]
    fn test_photo_filter_roundtrip() {
        roundtrip(AdjustmentLayer::PhotoFilter(PhotoFilter {
            color: LabColor {
                lmnc: 50.0,
                a: 10.0,
                b: -5.0,
            },
            density: 25,
            preserve_luminosity: true,
        }));
    }

    #[test]
    fn test_levels_roundtrip() {
        // Format always stores 29 channels and reader truncates to 4; supply 4 for a clean roundtrip
        let ch = |ib: u16, iw: u16| LevelsChannel {
            input_black: ib,
            input_white: iw,
            output_black: 0,
            output_white: 255,
            gamma: 100,
        };
        roundtrip(AdjustmentLayer::Levels(Levels {
            channels: vec![ch(10, 240), ch(0, 255), ch(0, 255), ch(0, 255)],
        }));
    }

    #[test]
    fn test_hue_saturation_roundtrip() {
        roundtrip(AdjustmentLayer::HueSaturation(HueSaturation {
            colorize: false,
            colorized_master: [0, 0, 0],
            master: [10, -5, 0],
            ranges: (0..6)
                .map(|_| HueSatRange {
                    range: [0, 0, 0, 0],
                    adjust: [0, 0, 0],
                })
                .collect(),
        }));
    }

    #[test]
    fn test_selective_color_roundtrip() {
        roundtrip(AdjustmentLayer::SelectiveColor(SelectiveColor {
            absolute: false,
            adjustments: (0..9).map(|_| [0i16; 4]).collect(),
        }));
    }

    #[test]
    fn test_channel_mixer_roundtrip() {
        roundtrip(AdjustmentLayer::ChannelMixer(ChannelMixer {
            monochrome: false,
            values: vec![
                100, 0, 0, 0, 100, 0, 0, 0, 100, 0, 0, 0, 100, 0, 0, 0, 100, 0, 0, 0,
            ],
        }));
    }

    #[test]
    fn test_curves_points_roundtrip() {
        roundtrip(AdjustmentLayer::Curves(Curves {
            mode: 0,
            channels: vec![
                CurvesChannel::Points(vec![0, 0, 128, 128, 255, 255]),
                CurvesChannel::Points(vec![0, 0, 255, 255]),
                CurvesChannel::Points(vec![0, 0, 255, 255]),
                CurvesChannel::Points(vec![0, 0, 255, 255]),
            ],
        }));
    }

    #[test]
    fn test_gradient_map_roundtrip() {
        roundtrip(AdjustmentLayer::GradientMap(GradientMap {
            reverse: false,
            dither: false,
            interpolation_method_type: "Gcls".to_string(),
            gradient: GradientDefinition {
                name: "Test".to_string(),
                interpolation: 0,
                color_stops: vec![
                    GradientColorStop {
                        location: 0,
                        midpoint: 50,
                        color: [0, 0, 0],
                    },
                    GradientColorStop {
                        location: 4096,
                        midpoint: 50,
                        color: [255, 255, 255],
                    },
                ],
                transparency_stops: vec![GradientTransparencyStop {
                    location: 0,
                    midpoint: 50,
                    opacity: 100,
                }],
            },
        }));
    }
}

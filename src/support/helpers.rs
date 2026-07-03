//! Helper utilities for PSD file operations
//!
//! Includes blend mode conversion, color space utilities, and image data handling.

use crate::support::compression;
use crate::support::error::{PsdError, Result};
use crate::api::types::{BlendMode, ChannelID, PixelData};
use bitflags::bitflags;
use std::collections::HashMap;

bitflags! {
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub(crate) struct LayerBlendFlags: u8 {
        const TRANSPARENCY_PROTECTED = 0x01;
        const HIDDEN = 0x02;
        const PHOTOSHOP_5 = 0x08;
    }
}

bitflags! {
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub(crate) struct LayerMaskStateBits: u8 {
        const POSITION_RELATIVE_TO_LAYER = 0x01;
        const DISABLED = 0x02;
        const FROM_VECTOR_DATA = 0x08;
        const HAS_PARAMETERS = 0x10;
    }
}

bitflags! {
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub(crate) struct LayerMaskParameterFlags: u8 {
        const USER_MASK_DENSITY = 0x01;
        const USER_MASK_FEATHER = 0x02;
        const VECTOR_MASK_DENSITY = 0x04;
        const VECTOR_MASK_FEATHER = 0x08;
    }
}

bitflags! {
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub(crate) struct ProtectedFlagsBits: u32 {
        const TRANSPARENCY = 0x01;
        const COMPOSITE = 0x02;
        const POSITION = 0x04;
        const ARTBOARDS = 0x08;
    }
}

bitflags! {
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub(crate) struct VectorMaskFlagsBits: u32 {
        const INVERT = 0x01;
        const NOT_LINK = 0x02;
        const DISABLE = 0x04;
    }
}

lazy_static::lazy_static! {
    /// Map from blend mode strings to BlendMode enum
    pub static ref TO_BLEND_MODE: HashMap<&'static str, BlendMode> = {
        let mut m = HashMap::new();
        m.insert("pass", BlendMode::PassThrough);
        m.insert("norm", BlendMode::Normal);
        m.insert("diss", BlendMode::Dissolve);
        m.insert("dark", BlendMode::Darken);
        m.insert("mul ", BlendMode::Multiply);
        m.insert("idiv", BlendMode::ColorBurn);
        m.insert("lbrn", BlendMode::LinearBurn);
        m.insert("dkCl", BlendMode::DarkerColor);
        m.insert("lite", BlendMode::Lighten);
        m.insert("scrn", BlendMode::Screen);
        m.insert("div ", BlendMode::ColorDodge);
        m.insert("lddg", BlendMode::LinearDodge);
        m.insert("lgCl", BlendMode::LighterColor);
        m.insert("over", BlendMode::Overlay);
        m.insert("sLit", BlendMode::SoftLight);
        m.insert("hLit", BlendMode::HardLight);
        m.insert("vLit", BlendMode::VividLight);
        m.insert("lLit", BlendMode::LinearLight);
        m.insert("pLit", BlendMode::PinLight);
        m.insert("hMix", BlendMode::HardMix);
        m.insert("diff", BlendMode::Difference);
        m.insert("smud", BlendMode::Exclusion);
        m.insert("fsub", BlendMode::Subtract);
        m.insert("fdiv", BlendMode::Divide);
        m.insert("hue ", BlendMode::Hue);
        m.insert("sat ", BlendMode::Saturation);
        m.insert("colr", BlendMode::Color);
        m.insert("lum ", BlendMode::Luminosity);
        m
    };

    /// Map from BlendMode enum to blend mode strings
    pub static ref FROM_BLEND_MODE: HashMap<BlendMode, &'static str> = {
        let mut m = HashMap::new();
        m.insert(BlendMode::PassThrough, "pass");
        m.insert(BlendMode::Normal, "norm");
        m.insert(BlendMode::Dissolve, "diss");
        m.insert(BlendMode::Darken, "dark");
        m.insert(BlendMode::Multiply, "mul ");
        m.insert(BlendMode::ColorBurn, "idiv");
        m.insert(BlendMode::LinearBurn, "lbrn");
        m.insert(BlendMode::DarkerColor, "dkCl");
        m.insert(BlendMode::Lighten, "lite");
        m.insert(BlendMode::Screen, "scrn");
        m.insert(BlendMode::ColorDodge, "div ");
        m.insert(BlendMode::LinearDodge, "lddg");
        m.insert(BlendMode::LighterColor, "lgCl");
        m.insert(BlendMode::Overlay, "over");
        m.insert(BlendMode::SoftLight, "sLit");
        m.insert(BlendMode::HardLight, "hLit");
        m.insert(BlendMode::VividLight, "vLit");
        m.insert(BlendMode::LinearLight, "lLit");
        m.insert(BlendMode::PinLight, "pLit");
        m.insert(BlendMode::HardMix, "hMix");
        m.insert(BlendMode::Difference, "diff");
        m.insert(BlendMode::Exclusion, "smud");
        m.insert(BlendMode::Subtract, "fsub");
        m.insert(BlendMode::Divide, "fdiv");
        m.insert(BlendMode::Hue, "hue ");
        m.insert(BlendMode::Saturation, "sat ");
        m.insert(BlendMode::Color, "colr");
        m.insert(BlendMode::Luminosity, "lum ");
        m
    };
}

/// Convert a 4-character blend mode signature to BlendMode enum
pub fn to_blend_mode(sig: &str) -> Result<BlendMode> {
    TO_BLEND_MODE
        .get(sig)
        .copied()
        .ok_or_else(|| PsdError::InvalidBlendMode(sig.to_string()))
}

/// Convert a BlendMode enum to a 4-character signature
pub fn from_blend_mode(mode: BlendMode) -> &'static str {
    FROM_BLEND_MODE.get(&mode).copied().unwrap_or("norm")
}

/// Get the byte offset for a channel ID in pixel data
pub fn offset_for_channel(channel_id: ChannelID, cmyk: bool) -> usize {
    match channel_id {
        ChannelID::Color0 => 0,
        ChannelID::Color1 => 1,
        ChannelID::Color2 => 2,
        ChannelID::Color3 => {
            if cmyk {
                3
            } else {
                4
            }
        }
        ChannelID::Transparency => {
            if cmyk {
                4
            } else {
                3
            }
        }
        ChannelID::UserMask => 0,
        ChannelID::RealUserMask => 0,
    }
}

/// Clamp a value between min and max
pub fn clamp(value: f64, min: f64, max: f64) -> f64 {
    if value < min {
        min
    } else if value > max {
        max
    } else {
        value
    }
}

/// Check if image data has alpha channel with transparency
pub fn has_alpha(data: &PixelData) -> bool {
    let size = data.width * data.height * 4;

    for i in (3..size).step_by(4) {
        if i < data.data.len() && data.data[i] != 255 {
            return true;
        }
    }

    false
}

/// Reset image data to transparent black
pub fn reset_image_data(data: &mut PixelData) {
    let size = data.width * data.height * 4;

    for i in 0..size {
        if i % 4 == 3 {
            data.data[i] = 255; // Alpha
        } else {
            data.data[i] = 0; // RGB
        }
    }
}

/// Decode bitmap (1-bit) image data
pub fn decode_bitmap(input: &[u8], output: &mut [u8], width: usize, height: usize) -> Result<()> {
    let mut output_pos = 0;
    let mut input_pos = 0;

    for _y in 0..height {
        for x in (0..width).step_by(8) {
            if input_pos >= input.len() {
                return Err(PsdError::InvalidFormat("Bitmap data truncated".to_string()));
            }

            let mut byte = input[input_pos];
            input_pos += 1;

            for i in 0..8 {
                if x + i >= width {
                    break;
                }

                let value = if byte & 0x80 != 0 { 0 } else { 255 };
                byte <<= 1;

                if output_pos + 3 < output.len() {
                    output[output_pos] = value;
                    output[output_pos + 1] = value;
                    output[output_pos + 2] = value;
                    output[output_pos + 3] = 255;
                    output_pos += 4;
                }
            }
        }
    }

    Ok(())
}

/// Setup grayscale image data (copy grayscale to RGB channels)
pub fn setup_grayscale(data: &mut PixelData) {
    let size = data.width * data.height * 4;

    for i in (0..size).step_by(4) {
        if i < data.data.len() {
            let c = data.data[i];
            if i + 1 < data.data.len() {
                data.data[i + 1] = c;
            }
            if i + 2 < data.data.len() {
                data.data[i + 2] = c;
            }
        }
    }
}

/// Write raw channel data
pub fn write_data_raw(data: &PixelData, offset: usize) -> Option<Vec<u8>> {
    let width = data.width;
    let height = data.height;

    if width == 0 || height == 0 {
        return None;
    }

    let mut result = Vec::with_capacity(width * height);

    for i in 0..(width * height) {
        let pos = i * 4 + offset;
        if pos < data.data.len() {
            result.push(data.data[pos]);
        } else {
            result.push(0);
        }
    }

    Some(result)
}

/// Write RLE-compressed channel data
pub fn write_data_rle(
    data: &PixelData,
    offsets: &[usize],
    _large: bool,
) -> Result<Option<Vec<u8>>> {
    let width = data.width;
    let height = data.height;

    if width == 0 || height == 0 {
        return Ok(None);
    }

    let mut all_data = Vec::new();

    for &offset in offsets {
        if let Some(channel_data) = write_data_raw(data, offset) {
            let compressed = compression::compress_rle(&channel_data, width, height, false)?;
            all_data.extend_from_slice(&compressed);
        }
    }

    Ok(Some(all_data))
}

/// Write ZIP-compressed channel data without prediction
pub fn write_data_zip_without_prediction(data: &PixelData, offsets: &[usize]) -> Result<Vec<u8>> {
    let width = data.width;
    let height = data.height;
    let size = width * height;

    let mut all_data = Vec::new();

    for &offset in offsets {
        let mut channel = Vec::with_capacity(size);

        for i in 0..size {
            let pos = i * 4 + offset;
            if pos < data.data.len() {
                channel.push(data.data[pos]);
            } else {
                channel.push(0);
            }
        }

        let compressed = compression::compress_zip(&channel)?;
        all_data.extend_from_slice(&compressed);
    }

    Ok(all_data)
}

/// Color space enumeration for color conversion
#[repr(u16)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ColorSpace {
    RGB = 0,
    HSB = 1,
    CMYK = 2,
    Lab = 7,
    Grayscale = 8,
}

/// Layer mask flags
#[repr(u8)]
#[derive(Debug, Clone, Copy)]
pub enum LayerMaskFlags {
    PositionRelativeToLayer = 1,
    LayerMaskDisabled = 2,
    InvertLayerMaskWhenBlending = 4,
    LayerMaskFromRenderingOtherData = 8,
    MaskHasParametersAppliedToIt = 16,
}

/// Mask parameters flags
#[repr(u8)]
#[derive(Debug, Clone, Copy)]
pub enum MaskParams {
    UserMaskDensity = 1,
    UserMaskFeather = 2,
    VectorMaskDensity = 4,
    VectorMaskFeather = 8,
}

/// Large additional info keys (for PSB format)
pub const LARGE_ADDITIONAL_INFO_KEYS: &[&str] = &[
    "LMsk", "Lr16", "Lr32", "Layr", "Mt16", "Mt32", "Mtrn", "Alph", "FMsk", "lnk2", "FEid", "FXid",
    "PxSD", "cinf",
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_blend_mode_conversion() {
        assert_eq!(to_blend_mode("norm").unwrap(), BlendMode::Normal);
        assert_eq!(from_blend_mode(BlendMode::Normal), "norm");
        assert_eq!(to_blend_mode("mul ").unwrap(), BlendMode::Multiply);
        assert_eq!(from_blend_mode(BlendMode::Multiply), "mul ");
    }

    #[test]
    fn test_clamp() {
        assert_eq!(clamp(0.5, 0.0, 1.0), 0.5);
        assert_eq!(clamp(-0.5, 0.0, 1.0), 0.0);
        assert_eq!(clamp(1.5, 0.0, 1.0), 1.0);
    }

    #[test]
    fn test_has_alpha() {
        let mut data = PixelData {
            width: 2,
            height: 1,
            data: vec![255, 0, 0, 255, 0, 255, 0, 255],
        };
        assert!(!has_alpha(&data));

        data.data[3] = 128;
        assert!(has_alpha(&data));
    }

    #[test]
    fn test_offset_for_channel() {
        assert_eq!(offset_for_channel(ChannelID::Color0, false), 0);
        assert_eq!(offset_for_channel(ChannelID::Color1, false), 1);
        assert_eq!(offset_for_channel(ChannelID::Color2, false), 2);
        assert_eq!(offset_for_channel(ChannelID::Transparency, false), 3);
    }
}

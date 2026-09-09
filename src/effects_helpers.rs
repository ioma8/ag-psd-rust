//! Helper functions for layer effects
//!
//! Provides utilities for reading and writing layer effects from PSD files.

use crate::api::effects::*;
use crate::api::types::{BevelDirection, BevelStyle, BlendMode, Color, Units, UnitsValue};
use crate::io::reader::PsdReader;
use crate::io::writer::PsdWriter;
use crate::support::binrw_support::{
    decode_be, encode_be, EffectBlockHeaderRecord, EffectsCommonStateRecord, EffectsHeaderRecord,
};
use crate::support::error::{PsdError, Result};
use crate::support::helpers::{clamp, from_blend_mode, to_blend_mode};
use std::io::{Read, Seek};

/// Default black color used when no color is specified.
const DEFAULT_COLOR: Color = Color::Rgb48 {
    red: 0,
    green: 0,
    blue: 0,
};

const BEVEL_STYLES_MAP: &[BevelStyle] = &[
    BevelStyle::InnerBevel, // placeholder for index 0
    BevelStyle::OuterBevel,
    BevelStyle::InnerBevel,
    BevelStyle::Emboss,
    BevelStyle::PillowEmboss,
    BevelStyle::StrokeEmboss,
];

/// Read blend mode from reader (8BIM signature + 4-byte mode)
fn read_blend_mode<R: Read + Seek>(reader: &mut PsdReader<R>) -> Result<BlendMode> {
    reader.check_signature("8BIM")?;
    let sig = reader.read_signature()?;
    to_blend_mode(&sig)
}

/// Write blend mode to writer (8BIM signature + 4-byte mode)
fn write_blend_mode(writer: &mut PsdWriter, mode: BlendMode) -> Result<()> {
    writer.write_signature("8BIM")?;
    let sig = from_blend_mode(mode);
    writer.write_signature(&sig)?;
    Ok(())
}

/// Read fixed point 8-bit value (0-255 -> 0.0-1.0)
fn read_fixed_point8<R: Read + Seek>(reader: &mut PsdReader<R>) -> Result<f64> {
    let value = reader.read_u8()?;
    Ok(value as f64 / 255.0)
}

/// Write fixed point 8-bit value (0.0-1.0 -> 0-255)
fn write_fixed_point8(writer: &mut PsdWriter, value: f64) -> Result<()> {
    let byte = (value * 255.0).round() as u8;
    writer.write_u8(byte)?;
    Ok(())
}

fn effect_record_color(color: Option<&Color>) -> Color {
    match color.copied() {
        Some(Color::RGBA(color)) => Color::Rgb48 {
            red: u16::from(color.r) * 257,
            green: u16::from(color.g) * 257,
            blue: u16::from(color.b) * 257,
        },
        Some(Color::RGB(color)) => Color::Rgb48 {
            red: u16::from(color.r) * 257,
            green: u16::from(color.g) * 257,
            blue: u16::from(color.b) * 257,
        },
        Some(Color::FRGB(color)) => Color::Rgb48 {
            red: (clamp(color.fr, 0.0, 1.0) * 65535.0).round() as u16,
            green: (clamp(color.fg, 0.0, 1.0) * 65535.0).round() as u16,
            blue: (clamp(color.fb, 0.0, 1.0) * 65535.0).round() as u16,
        },
        Some(color) => color,
        None => DEFAULT_COLOR,
    }
}

fn write_effect_color(writer: &mut PsdWriter, color: Option<&Color>) -> Result<()> {
    let color = effect_record_color(color);
    writer.write_color(Some(&color))
}

/// Read layer effects from a PSD reader
pub fn read_effects<R: Read + Seek>(reader: &mut PsdReader<R>) -> Result<LayerEffectsInfo> {
    let header: EffectsHeaderRecord = decode_be(&reader.read_bytes(4)?, "effects header")?;
    let version = header.version;
    if version != 0 {
        return Err(PsdError::InvalidFormat(format!(
            "Invalid effects layer version: {}",
            version
        )));
    }

    let effects_count = header.effects_count;
    let mut effects = LayerEffectsInfo {
        disabled: None,
        scale: None,
        drop_shadow: None,
        inner_shadow: None,
        outer_glow: None,
        inner_glow: None,
        bevel: None,
        satin: None,
        solid_fill: None,
        gradient_overlay: None,
        pattern_overlay: None,
        stroke: None,
    };

    for _ in 0..effects_count {
        reader.check_signature("8BIM")?;
        let effect_type = reader.read_signature()?;

        match effect_type.as_str() {
            "cmnS" => {
                // Common state
                let common: EffectsCommonStateRecord =
                    decode_be(&reader.read_bytes(11)?, "effects common state")?;

                if common.size != 7 || common.version != 0 {
                    return Err(PsdError::InvalidFormat(
                        "Invalid effects common state".to_string(),
                    ));
                }
                // visible == 0 means all effects are disabled — don't error, just mark
                effects.disabled = Some(common.visible == 0);
            }
            "dsdw" | "isdw" => {
                // Drop shadow / Inner shadow
                let block: EffectBlockHeaderRecord =
                    decode_be(&reader.read_bytes(8)?, "shadow header")?;
                let block_size = block.block_size;
                let version = block.version;

                if block_size != 41 && block_size != 51 {
                    return Err(PsdError::InvalidFormat(format!(
                        "Invalid shadow size: {}",
                        block_size
                    )));
                }
                if version != 0 && version != 2 {
                    return Err(PsdError::InvalidFormat(format!(
                        "Invalid shadow version: {}",
                        version
                    )));
                }

                let size = reader.read_fixed_point_32()?;
                let intensity = reader.read_fixed_point_32()?;
                let angle = reader.read_fixed_point_32()?;
                let distance = reader.read_fixed_point_32()?;
                let color = reader.read_color()?;
                let blend_mode = read_blend_mode(reader)?;
                let enabled = reader.read_u8()? != 0;
                let use_global_light = reader.read_u8()? != 0;
                let opacity = read_fixed_point8(reader)?;

                if block_size >= 51 {
                    let _native_color = reader.read_color()?;
                }

                let shadow = LayerEffectShadow {
                    present: None,
                    show_in_dialog: None,
                    enabled: Some(enabled),
                    intensity: Some(intensity),
                    blend_mode: Some(blend_mode),
                    color: Some(color),
                    opacity: Some(opacity),
                    angle: Some(angle),
                    distance: Some(UnitsValue {
                        units: Units::Pixels,
                        value: distance,
                    }),
                    size: Some(UnitsValue {
                        units: Units::Pixels,
                        value: size,
                    }),
                    contour: None,
                    antialiased: None,
                    choke: None,
                    layer_conceals: None,
                    use_global_light: Some(use_global_light),
                };

                if effect_type == "dsdw" {
                    effects.drop_shadow = Some(vec![shadow]);
                } else {
                    effects.inner_shadow = Some(vec![shadow]);
                }
            }
            "oglw" => {
                // Outer glow
                let block: EffectBlockHeaderRecord =
                    decode_be(&reader.read_bytes(8)?, "outer glow header")?;
                let block_size = block.block_size;
                let version = block.version;

                if block_size != 32 && block_size != 42 {
                    return Err(PsdError::InvalidFormat(format!(
                        "Invalid outer glow size: {}",
                        block_size
                    )));
                }
                if version != 0 && version != 2 {
                    return Err(PsdError::InvalidFormat(format!(
                        "Invalid outer glow version: {}",
                        version
                    )));
                }

                let size = reader.read_fixed_point_32()?;
                let intensity = reader.read_fixed_point_32()?;
                let color = reader.read_color()?;
                let blend_mode = read_blend_mode(reader)?;
                let enabled = reader.read_u8()? != 0;
                let opacity = read_fixed_point8(reader)?;

                if block_size >= 42 {
                    let _native_color = reader.read_color()?;
                }

                effects.outer_glow = Some(LayerEffectsOuterGlow {
                    present: None,
                    show_in_dialog: None,
                    enabled: Some(enabled),
                    blend_mode: Some(blend_mode),
                    intensity: Some(intensity),
                    opacity: Some(opacity),
                    noise: None,
                    color: Some(color),
                    size: Some(UnitsValue {
                        units: Units::Pixels,
                        value: size,
                    }),
                    contour: None,
                    antialiased: None,
                    range: None,
                    jitter: None,
                    source: None,
                    choke: None,
                });
            }
            "iglw" => {
                // Inner glow
                let block: EffectBlockHeaderRecord =
                    decode_be(&reader.read_bytes(8)?, "inner glow header")?;
                let block_size = block.block_size;
                let version = block.version;

                if block_size != 32 && block_size != 43 {
                    return Err(PsdError::InvalidFormat(format!(
                        "Invalid inner glow size: {}",
                        block_size
                    )));
                }
                if version != 0 && version != 2 {
                    return Err(PsdError::InvalidFormat(format!(
                        "Invalid inner glow version: {}",
                        version
                    )));
                }

                let size = reader.read_fixed_point_32()?;
                let intensity = reader.read_fixed_point_32()?;
                let color = reader.read_color()?;
                let blend_mode = read_blend_mode(reader)?;
                let enabled = reader.read_u8()? != 0;
                let opacity = read_fixed_point8(reader)?;

                if block_size >= 43 {
                    let _inverted = reader.read_u8()?;
                    let _native_color = reader.read_color()?;
                }

                effects.inner_glow = Some(LayerEffectInnerGlow {
                    present: None,
                    show_in_dialog: None,
                    enabled: Some(enabled),
                    blend_mode: Some(blend_mode),
                    intensity: Some(intensity),
                    opacity: Some(opacity),
                    noise: None,
                    color: Some(color),
                    technique: None,
                    source: None,
                    choke: None,
                    size: Some(UnitsValue {
                        units: Units::Pixels,
                        value: size,
                    }),
                    contour: None,
                    antialiased: None,
                    range: None,
                    jitter: None,
                });
            }
            "bevl" => {
                // Bevel
                let block_size = reader.read_u32()?;
                let version = reader.read_u32()?;

                if block_size != 58 && block_size != 78 {
                    return Err(PsdError::InvalidFormat(format!(
                        "Invalid bevel size: {}",
                        block_size
                    )));
                }
                if version != 0 && version != 2 {
                    return Err(PsdError::InvalidFormat(format!(
                        "Invalid bevel version: {}",
                        version
                    )));
                }

                let angle = reader.read_fixed_point_32()?;
                let strength = reader.read_fixed_point_32()?;
                let size = reader.read_fixed_point_32()?;
                let highlight_blend_mode = read_blend_mode(reader)?;
                let shadow_blend_mode = read_blend_mode(reader)?;
                let highlight_color = reader.read_color()?;
                let shadow_color = reader.read_color()?;
                let style_index = reader.read_u8()? as usize;
                let style = if style_index > 0 && style_index < BEVEL_STYLES_MAP.len() {
                    BEVEL_STYLES_MAP[style_index]
                } else {
                    BevelStyle::InnerBevel
                };
                let highlight_opacity = read_fixed_point8(reader)?;
                let shadow_opacity = read_fixed_point8(reader)?;
                let enabled = reader.read_u8()? != 0;
                let use_global_light = reader.read_u8()? != 0;
                let direction = if reader.read_u8()? != 0 {
                    BevelDirection::Down
                } else {
                    BevelDirection::Up
                };

                if block_size >= 78 {
                    let _real_highlight_color = reader.read_color()?;
                    let _real_shadow_color = reader.read_color()?;
                }

                effects.bevel = Some(LayerEffectBevel {
                    present: None,
                    show_in_dialog: None,
                    enabled: Some(enabled),
                    style: Some(style),
                    technique: None,
                    size: Some(UnitsValue {
                        units: Units::Pixels,
                        value: size,
                    }),
                    angle: Some(angle),
                    altitude: None,
                    highlight_blend_mode: Some(highlight_blend_mode),
                    shadow_blend_mode: Some(shadow_blend_mode),
                    highlight_color: Some(highlight_color),
                    shadow_color: Some(shadow_color),
                    highlight_opacity: Some(highlight_opacity),
                    shadow_opacity: Some(shadow_opacity),
                    contour: None,
                    antialias_gloss: None,
                    soften: None,
                    use_global_light: Some(use_global_light),
                    use_shape: None,
                    use_texture: None,
                    strength: Some(strength),
                    direction: Some(direction),
                });
            }
            "sofi" => {
                // Solid fill
                let size = reader.read_u32()?;
                let version = reader.read_u32()?;

                if size != 34 {
                    return Err(PsdError::InvalidFormat(format!(
                        "Invalid effects solid fill info size: {}",
                        size
                    )));
                }
                if version != 2 {
                    return Err(PsdError::InvalidFormat(format!(
                        "Invalid effects solid fill info version: {}",
                        version
                    )));
                }

                let blend_mode = read_blend_mode(reader)?;
                let color = reader.read_color()?;
                let opacity = read_fixed_point8(reader)?;
                let enabled = reader.read_u8()? != 0;
                let _native_color = reader.read_color()?;

                effects.solid_fill = Some(vec![LayerEffectSolidFill {
                    present: None,
                    show_in_dialog: None,
                    enabled: Some(enabled),
                    blend_mode: Some(blend_mode),
                    opacity: Some(opacity),
                    color: Some(color),
                }]);
            }
            _ => {
                // Unknown effect type (e.g. ChFX satin, stroke, gradient overlay added in PS 7+)
                // Break out of the loop gracefully rather than failing
                break;
            }
        }
    }

    Ok(effects)
}

/// Write shadow info to a PSD writer
fn write_shadow_info(writer: &mut PsdWriter, shadow: &LayerEffectShadow) -> Result<()> {
    writer.write_bytes(&encode_be(
        &EffectBlockHeaderRecord {
            block_size: 51,
            version: 2,
        },
        "shadow header",
    )?)?;
    writer.write_fixed_point_32(shadow.size.as_ref().map(|s| s.value).unwrap_or(0.0))?;
    writer.write_fixed_point_32(shadow.intensity.unwrap_or(75.0))?;
    writer.write_fixed_point_32(shadow.angle.unwrap_or(0.0))?;
    writer.write_fixed_point_32(shadow.distance.as_ref().map(|d| d.value).unwrap_or(0.0))?;
    write_effect_color(writer, shadow.color.as_ref())?;
    write_blend_mode(writer, shadow.blend_mode.unwrap_or(BlendMode::Normal))?;
    writer.write_u8(if shadow.enabled.unwrap_or(false) {
        1
    } else {
        0
    })?;
    writer.write_u8(if shadow.use_global_light.unwrap_or(false) {
        1
    } else {
        0
    })?;
    write_fixed_point8(writer, shadow.opacity.unwrap_or(1.0))?;
    write_effect_color(writer, shadow.color.as_ref())?; // native color
    Ok(())
}

/// Write layer effects to a PSD writer
pub fn write_effects(writer: &mut PsdWriter, effects: &LayerEffectsInfo) -> Result<()> {
    let drop_shadow = effects.drop_shadow.as_ref().and_then(|v| v.first());
    let inner_shadow = effects.inner_shadow.as_ref().and_then(|v| v.first());
    let outer_glow = effects.outer_glow.as_ref();
    let inner_glow = effects.inner_glow.as_ref();
    let bevel = effects.bevel.as_ref();
    let solid_fill = effects.solid_fill.as_ref().and_then(|v| v.first());

    let mut count = 1; // Always include common state
    if drop_shadow.is_some() {
        count += 1;
    }
    if inner_shadow.is_some() {
        count += 1;
    }
    if outer_glow.is_some() {
        count += 1;
    }
    if inner_glow.is_some() {
        count += 1;
    }
    if bevel.is_some() {
        count += 1;
    }
    if solid_fill.is_some() {
        count += 1;
    }

    writer.write_bytes(&encode_be(
        &EffectsHeaderRecord {
            version: 0,
            effects_count: count,
        },
        "effects header",
    )?)?;

    // Common state
    writer.write_signature("8BIM")?;
    writer.write_signature("cmnS")?;
    writer.write_bytes(&encode_be(
        &EffectsCommonStateRecord {
            size: 7,
            version: 0,
            visible: 1,
            padding: [0; 2],
        },
        "effects common state",
    )?)?;

    if let Some(shadow) = drop_shadow {
        writer.write_signature("8BIM")?;
        writer.write_signature("dsdw")?;
        write_shadow_info(writer, shadow)?;
    }

    if let Some(shadow) = inner_shadow {
        writer.write_signature("8BIM")?;
        writer.write_signature("isdw")?;
        write_shadow_info(writer, shadow)?;
    }

    if let Some(glow) = outer_glow {
        writer.write_signature("8BIM")?;
        writer.write_signature("oglw")?;
        writer.write_bytes(&encode_be(
            &EffectBlockHeaderRecord {
                block_size: 42,
                version: 2,
            },
            "outer glow header",
        )?)?;
        writer.write_fixed_point_32(glow.size.as_ref().map(|s| s.value).unwrap_or(0.0))?;
        writer.write_fixed_point_32(glow.intensity.unwrap_or(75.0))?;
        write_effect_color(writer, glow.color.as_ref())?;
        write_blend_mode(writer, glow.blend_mode.unwrap_or(BlendMode::Normal))?;
        writer.write_u8(if glow.enabled.unwrap_or(false) { 1 } else { 0 })?;
        write_fixed_point8(writer, glow.opacity.unwrap_or(0.0))?;
        write_effect_color(writer, glow.color.as_ref())?;
    }

    if let Some(glow) = inner_glow {
        writer.write_signature("8BIM")?;
        writer.write_signature("iglw")?;
        writer.write_bytes(&encode_be(
            &EffectBlockHeaderRecord {
                block_size: 43,
                version: 2,
            },
            "inner glow header",
        )?)?;
        writer.write_fixed_point_32(glow.size.as_ref().map(|s| s.value).unwrap_or(0.0))?;
        writer.write_fixed_point_32(glow.intensity.unwrap_or(75.0))?;
        write_effect_color(writer, glow.color.as_ref())?;
        write_blend_mode(writer, glow.blend_mode.unwrap_or(BlendMode::Normal))?;
        writer.write_u8(if glow.enabled.unwrap_or(false) { 1 } else { 0 })?;
        write_fixed_point8(writer, glow.opacity.unwrap_or(0.0))?;
        writer.write_u8(0)?; // inverted
        write_effect_color(writer, glow.color.as_ref())?;
    }

    if let Some(bevel_effect) = bevel {
        writer.write_signature("8BIM")?;
        writer.write_signature("bevl")?;
        writer.write_bytes(&encode_be(
            &EffectBlockHeaderRecord {
                block_size: 78,
                version: 2,
            },
            "bevel header",
        )?)?;
        writer.write_fixed_point_32(bevel_effect.angle.unwrap_or(0.0))?;
        writer.write_fixed_point_32(bevel_effect.strength.unwrap_or(0.0))?;
        writer.write_fixed_point_32(bevel_effect.size.as_ref().map(|s| s.value).unwrap_or(0.0))?;
        write_blend_mode(
            writer,
            bevel_effect
                .highlight_blend_mode
                .unwrap_or(BlendMode::Normal),
        )?;
        write_blend_mode(
            writer,
            bevel_effect.shadow_blend_mode.unwrap_or(BlendMode::Normal),
        )?;
        write_effect_color(writer, bevel_effect.highlight_color.as_ref())?;
        write_effect_color(writer, bevel_effect.shadow_color.as_ref())?;

        let style = bevel_effect.style.unwrap_or(BevelStyle::InnerBevel);
        let style_index = BEVEL_STYLES_MAP
            .iter()
            .position(|&s| s == style)
            .unwrap_or(2);
        writer.write_u8(style_index as u8)?;

        write_fixed_point8(writer, bevel_effect.highlight_opacity.unwrap_or(0.0))?;
        write_fixed_point8(writer, bevel_effect.shadow_opacity.unwrap_or(0.0))?;
        writer.write_u8(if bevel_effect.enabled.unwrap_or(false) {
            1
        } else {
            0
        })?;
        writer.write_u8(if bevel_effect.use_global_light.unwrap_or(false) {
            1
        } else {
            0
        })?;

        let direction = bevel_effect.direction.unwrap_or(BevelDirection::Up);
        writer.write_u8(if direction == BevelDirection::Down {
            1
        } else {
            0
        })?;

        write_effect_color(writer, bevel_effect.highlight_color.as_ref())?;
        write_effect_color(writer, bevel_effect.shadow_color.as_ref())?;
    }

    if let Some(fill) = solid_fill {
        writer.write_signature("8BIM")?;
        writer.write_signature("sofi")?;
        writer.write_bytes(&encode_be(
            &EffectBlockHeaderRecord {
                block_size: 34,
                version: 2,
            },
            "solid fill header",
        )?)?;
        write_blend_mode(writer, fill.blend_mode.unwrap_or(BlendMode::Normal))?;
        write_effect_color(writer, fill.color.as_ref())?;
        write_fixed_point8(writer, fill.opacity.unwrap_or(0.0))?;
        writer.write_u8(if fill.enabled.unwrap_or(false) { 1 } else { 0 })?;
        write_effect_color(writer, fill.color.as_ref())?;
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_fixed_point8_conversion() {
        let mut writer = PsdWriter::new(256);
        write_fixed_point8(&mut writer, 0.0).unwrap();
        write_fixed_point8(&mut writer, 0.5).unwrap();
        write_fixed_point8(&mut writer, 1.0).unwrap();

        let data = writer.get_buffer().to_vec();
        let cursor = std::io::Cursor::new(data);
        let mut reader = PsdReader::new(cursor, crate::api::psd::ReadOptions::default());

        assert!((read_fixed_point8(&mut reader).unwrap() - 0.0).abs() < 0.01);
        assert!((read_fixed_point8(&mut reader).unwrap() - 0.5).abs() < 0.01);
        assert!((read_fixed_point8(&mut reader).unwrap() - 1.0).abs() < 0.01);
    }
}

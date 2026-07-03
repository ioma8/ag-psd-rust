//! PSD file writer implementation
//!
//! Provides functionality to write PSD files.

use crate::support::binrw_support::{
    encode_be, ChannelInfoRecord, GlobalLayerMaskRecord, LayerBlendRecord, LayerMaskPrefixRecord,
    LayerRecordBounds, PsbChannelInfoRecord, PsdHeaderRecord,
};
use crate::support::compression;
use crate::support::error::{PsdError, Result};
use crate::support::helpers::{
    clamp, from_blend_mode, has_alpha, LayerBlendFlags, LayerMaskParameterFlags, LayerMaskStateBits,
};
use crate::api::layer::Layer;
use crate::api::psd::{GlobalLayerMaskInfo, Psd, WriteOptions};
use crate::api::types::{BlendMode, ChannelID, Color, ColorMode, Compression};
use crate::format::additional_info::SectionDivider;
use byteorder::{BigEndian, WriteBytesExt};
use std::io::Cursor;

/// PSD writer for binary data
pub struct PsdWriter {
    buffer: Vec<u8>,
    pub offset: usize,
}

impl PsdWriter {
    /// Create a new PSD writer with initial capacity
    pub fn new(capacity: usize) -> Self {
        Self {
            buffer: Vec::with_capacity(capacity),
            offset: 0,
        }
    }

    /// Create a new writer with default capacity (4KB)
    pub fn with_default_capacity() -> Self {
        Self::new(4096)
    }

    /// Get the written buffer
    pub fn get_buffer(&self) -> &[u8] {
        &self.buffer[..self.offset]
    }

    /// Get the written buffer as a Vec
    pub fn into_buffer(mut self) -> Vec<u8> {
        self.buffer.truncate(self.offset);
        self.buffer
    }

    /// Ensure buffer has enough capacity
    fn ensure_capacity(&mut self, additional: usize) {
        let required = self.offset + additional;
        if self.buffer.len() < required {
            let mut new_capacity = self.buffer.capacity();
            while new_capacity < required {
                new_capacity *= 2;
            }
            self.buffer.resize(new_capacity, 0);
        }
    }

    /// Write an unsigned 8-bit integer
    pub fn write_u8(&mut self, value: u8) -> Result<()> {
        self.ensure_capacity(1);
        self.buffer[self.offset] = value;
        self.offset += 1;
        Ok(())
    }

    /// Write a signed 16-bit integer (big-endian)
    pub fn write_i16(&mut self, value: i16) -> Result<()> {
        self.ensure_capacity(2);
        let mut cursor = Cursor::new(&mut self.buffer[self.offset..]);
        cursor.write_i16::<BigEndian>(value)?;
        self.offset += 2;
        Ok(())
    }

    /// Write an unsigned 16-bit integer (big-endian)
    pub fn write_u16(&mut self, value: u16) -> Result<()> {
        self.ensure_capacity(2);
        let mut cursor = Cursor::new(&mut self.buffer[self.offset..]);
        cursor.write_u16::<BigEndian>(value)?;
        self.offset += 2;
        Ok(())
    }

    /// Write a signed 32-bit integer (big-endian)
    pub fn write_i32(&mut self, value: i32) -> Result<()> {
        self.ensure_capacity(4);
        let mut cursor = Cursor::new(&mut self.buffer[self.offset..]);
        cursor.write_i32::<BigEndian>(value)?;
        self.offset += 4;
        Ok(())
    }

    /// Write an unsigned 32-bit integer (big-endian)
    pub fn write_u32(&mut self, value: u32) -> Result<()> {
        self.ensure_capacity(4);
        let mut cursor = Cursor::new(&mut self.buffer[self.offset..]);
        cursor.write_u32::<BigEndian>(value)?;
        self.offset += 4;
        Ok(())
    }

    /// Write a 32-bit float (big-endian)
    pub fn write_f32(&mut self, value: f32) -> Result<()> {
        self.ensure_capacity(4);
        let mut cursor = Cursor::new(&mut self.buffer[self.offset..]);
        cursor.write_f32::<BigEndian>(value)?;
        self.offset += 4;
        Ok(())
    }

    /// Write a 64-bit float (big-endian)
    pub fn write_f64(&mut self, value: f64) -> Result<()> {
        self.ensure_capacity(8);
        let mut cursor = Cursor::new(&mut self.buffer[self.offset..]);
        cursor.write_f64::<BigEndian>(value)?;
        self.offset += 8;
        Ok(())
    }

    /// Write raw bytes
    pub fn write_bytes(&mut self, data: &[u8]) -> Result<()> {
        self.ensure_capacity(data.len());
        self.buffer[self.offset..self.offset + data.len()].copy_from_slice(data);
        self.offset += data.len();
        Ok(())
    }

    /// Write zeros
    pub fn write_zeros(&mut self, count: usize) -> Result<()> {
        self.ensure_capacity(count);
        for _ in 0..count {
            self.buffer[self.offset] = 0;
            self.offset += 1;
        }
        Ok(())
    }

    /// Write a 4-character signature
    pub fn write_signature(&mut self, sig: &str) -> Result<()> {
        if sig.len() != 4 {
            return Err(PsdError::InvalidFormat(format!(
                "Invalid signature length: {}",
                sig
            )));
        }
        self.write_bytes(sig.as_bytes())
    }

    /// Write an ASCII string
    pub fn write_ascii_string(&mut self, text: &str) -> Result<()> {
        self.write_bytes(text.as_bytes())
    }

    /// Write a Pascal string (length-prefixed, padded)
    pub fn write_pascal_string(&mut self, text: &str, pad_to: usize) -> Result<()> {
        let mut length = text.len();
        if length > 255 {
            return Err(PsdError::InvalidFormat(
                "Pascal string too long".to_string(),
            ));
        }

        self.write_u8(length as u8)?;

        for byte in text.bytes() {
            let byte = if byte < 128 { byte } else { b'?' };
            self.write_u8(byte)?;
        }

        length += 1; // Include length byte
        while length % pad_to != 0 {
            self.write_u8(0)?;
            length += 1;
        }

        Ok(())
    }

    /// Write a Unicode string (UTF-16 BE)
    pub fn write_unicode_string(&mut self, text: &str) -> Result<()> {
        self.write_u32(text.len() as u32)?;

        for ch in text.chars() {
            self.write_u16(ch as u16)?;
        }

        Ok(())
    }

    /// Write a Unicode string with padding
    pub fn write_unicode_string_with_padding(&mut self, text: &str) -> Result<()> {
        self.write_u32((text.len() + 1) as u32)?;

        for ch in text.chars() {
            self.write_u16(ch as u16)?;
        }

        self.write_u16(0)?;

        Ok(())
    }

    /// Write a section with length prefix
    pub fn write_section<F>(&mut self, round: usize, large: bool, func: F) -> Result<()>
    where
        F: FnOnce(&mut Self) -> Result<()>,
    {
        self.write_section_with_length_mode(round, large, false, func)
    }

    pub fn write_section_with_length_mode<F>(
        &mut self,
        round: usize,
        large: bool,
        include_padding_in_length: bool,
        func: F,
    ) -> Result<()>
    where
        F: FnOnce(&mut Self) -> Result<()>,
    {
        if large {
            self.write_u32(0)?; // High 32 bits
        }

        let length_offset = self.offset;
        self.write_u32(0)?; // Placeholder for length

        let start_offset = self.offset;
        func(self)?;

        // Record content length BEFORE padding
        let content_length = (self.offset - start_offset) as u32;
        let mut padded_length = content_length;
        while padded_length % round as u32 != 0 {
            padded_length += 1;
        }

        // Pad to alignment (padding bytes are NOT counted in length)
        while (self.offset - start_offset) % round != 0 {
            self.write_u8(0)?;
        }

        let stored_length = if include_padding_in_length {
            padded_length
        } else {
            content_length
        };

        // Write content length
        let mut cursor = Cursor::new(&mut self.buffer[length_offset..]);
        cursor.write_u32::<BigEndian>(stored_length)?;

        Ok(())
    }

    /// Write a fixed-point number (16.16)
    pub fn write_fixed_point_32(&mut self, value: f64) -> Result<()> {
        let fixed = (value * 65536.0) as i32;
        self.write_i32(fixed)
    }

    /// Write a fixed-point path number (8.24)
    pub fn write_fixed_point_path_32(&mut self, value: f64) -> Result<()> {
        let fixed = (value * 16777216.0) as i32;
        self.write_i32(fixed)
    }

    /// Write a color value
    pub fn write_color(&mut self, color: Option<&Color>) -> Result<()> {
        match color {
            None => {
                self.write_u16(0)?; // RGB color space
                self.write_zeros(8)?;
            }
            Some(Color::RGBA(_)) | Some(Color::RGB(_)) | Some(Color::FRGB(_)) => {
                return Err(PsdError::UnsupportedFeature(
                    "Photoshop color structures require lossless raw color variants".to_string(),
                ));
            }
            Some(Color::Rgb48 { red, green, blue }) => {
                self.write_u16(0)?; // RGB
                self.write_u16(*red)?;
                self.write_u16(*green)?;
                self.write_u16(*blue)?;
                self.write_u16(0)?;
            }
            Some(Color::Hsb {
                hue,
                saturation,
                brightness,
            }) => {
                self.write_u16(1)?; // HSB
                self.write_u16(*hue)?;
                self.write_u16(*saturation)?;
                self.write_u16(*brightness)?;
                self.write_u16(0)?;
            }
            Some(Color::CMYK(c)) => {
                self.write_u16(2)?; // CMYK
                self.write_u16(c.c)?;
                self.write_u16(c.m)?;
                self.write_u16(c.y)?;
                self.write_u16(c.k)?;
            }
            Some(Color::Lab { lightness, a, b }) => {
                self.write_u16(7)?; // Lab
                self.write_u16(*lightness)?;
                self.write_i16(*a)?;
                self.write_i16(*b)?;
                self.write_u16(0)?;
            }
            Some(Color::Grayscale(c)) => {
                self.write_u16(8)?; // Grayscale
                self.write_u16(c.k)?;
                self.write_zeros(6)?;
            }
            Some(Color::OpaqueColorSpace {
                color_space,
                components,
            }) => {
                self.write_u16(*color_space)?;
                for component in components {
                    self.write_u16(*component)?;
                }
            }
        }

        Ok(())
    }
}

/// Write a PSD file
pub fn write_psd(psd: &Psd, options: &WriteOptions) -> Result<Vec<u8>> {
    if psd.width == 0 || psd.height == 0 {
        return Err(PsdError::InvalidFormat("Invalid document size".to_string()));
    }

    let max_size = if options.psb.unwrap_or(false) {
        300000
    } else {
        30000
    };
    if psd.width > max_size || psd.height > max_size {
        return Err(PsdError::InvalidFormat(format!(
            "Document size too large: {}x{} (max is {}x{})",
            psd.width, psd.height, max_size, max_size
        )));
    }

    let bits_per_channel = psd.bits_per_channel.unwrap_or(8);
    if !matches!(bits_per_channel, 8 | 16 | 32) {
        return Err(PsdError::UnsupportedFeature(format!(
            "Unsupported bits per channel for writing: {}",
            bits_per_channel
        )));
    }

    let mut writer = PsdWriter::new(1024 * 1024); // 1MB initial capacity

    let color_mode = psd.color_mode.unwrap_or(ColorMode::RGB);
    let global_alpha = if let Some(ref image_data) = psd.image_data {
        has_alpha(image_data)
    } else {
        false
    };
    let base_channels = match color_mode {
        ColorMode::Grayscale | ColorMode::Bitmap | ColorMode::Indexed => 1,
        ColorMode::CMYK => 4,
        _ => 3,
    };
    let channel_count = (base_channels + usize::from(global_alpha)) as u16;

    // Apply prewrite passes
    let mut psd = psd.clone();
    apply_resource_prewrite(&mut psd);
    crate::format::document_resource_postprocess::apply_document_prewrite(&mut psd)?;
    apply_text_prewrite(&mut psd)?;

    let header = PsdHeaderRecord {
        signature: *b"8BPS",
        version: if options.psb.unwrap_or(false) { 2 } else { 1 },
        reserved: [0; 6],
        channels: channel_count,
        height: psd.height,
        width: psd.width,
        depth: bits_per_channel as u16,
        color_mode: color_mode as u16,
    };
    writer.write_bytes(&encode_be(&header, "PSD header")?)?;

    // Write color mode data section
    write_color_mode_data(&mut writer, &psd)?;

    // Write image resources section
    write_image_resources(&mut writer, &psd, options)?;

    // Write layer and mask information section
    write_layer_and_mask_info(&mut writer, &psd, options, global_alpha)?;

    // Write image data section
    write_image_data(&mut writer, &psd, options, global_alpha)?;

    Ok(writer.into_buffer())
}

/// Write color mode data section
fn write_color_mode_data(writer: &mut PsdWriter, psd: &Psd) -> Result<()> {
    writer.write_section(1, false, |writer| {
        if psd.color_mode == Some(ColorMode::Indexed) {
            let palette = psd.palette.as_ref().ok_or_else(|| {
                PsdError::InvalidFormat("Indexed color mode requires palette".to_string())
            })?;
            if palette.len() != 256 {
                return Err(PsdError::InvalidFormat(
                    "Indexed color mode requires 256 palette entries".to_string(),
                ));
            }
            for entry in palette {
                writer.write_u8(entry.r)?;
            }
            for entry in palette {
                writer.write_u8(entry.g)?;
            }
            for entry in palette {
                writer.write_u8(entry.b)?;
            }
        } else if let Some(ref data) = psd.color_mode_data {
            writer.write_bytes(&data.bytes)?;
        }
        Ok(())
    })
}

/// Write image resources section
fn write_image_resources(writer: &mut PsdWriter, psd: &Psd, _options: &WriteOptions) -> Result<()> {
    writer.write_section(1, false, |writer| {
        if let Some(ref resources) = psd.image_resources {
            crate::format::image_resources::write_image_resources(writer, resources)?;
        }
        Ok(())
    })
}

/// Write layer and mask information section
fn write_layer_and_mask_info(
    writer: &mut PsdWriter,
    psd: &Psd,
    options: &WriteOptions,
    global_alpha: bool,
) -> Result<()> {
    let psb = options.psb.unwrap_or(false);
    writer.write_section(1, psb, |writer| {
        // Write layer info
        write_layer_info(writer, psd, options, global_alpha)?;

        // Write global layer mask info
        write_global_layer_mask_info(writer, psd.global_layer_mask_info.as_ref())?;

        // Write document-level tagged blocks
        crate::format::additional_info::write_document_additional_info_with_options(
            writer,
            &psd.additional_info,
            psb,
        )?;

        Ok(())
    })
}

/// Write layer info section
fn write_layer_info(
    writer: &mut PsdWriter,
    psd: &Psd,
    options: &WriteOptions,
    global_alpha: bool,
) -> Result<()> {
    let psb = options.psb.unwrap_or(false);
    let bits_per_channel = psd.bits_per_channel.unwrap_or(8);
    writer.write_section_with_length_mode(2, psb, true, |writer| {
        let layers = flatten_layers(psd.children.as_ref());
        let prepared_payloads: Vec<PreparedLayerChannels> = layers
            .iter()
            .map(|layer| prepare_layer_channels(layer, bits_per_channel, options))
            .collect::<Result<Vec<PreparedLayerChannels>>>()?;

        let layer_count = if global_alpha {
            -(layers.len() as i16)
        } else {
            layers.len() as i16
        };
        writer.write_i16(layer_count)?;

        // Write layer records
        for (layer, payloads) in layers.iter().zip(prepared_payloads.iter()) {
            write_layer_record(writer, layer, payloads, options)?;
        }

        // Write layer channel image data
        for payloads in &prepared_payloads {
            write_layer_channel_data(writer, payloads)?;
        }

        Ok(())
    })
}

/// Flatten layer hierarchy to a list
pub(crate) fn flatten_layers(children: Option<&Vec<Layer>>) -> Vec<Layer> {
    let mut result = Vec::new();

    if let Some(children) = children {
        for child in children {
            if let Some(ref child_children) = child.children {
                // Add synthetic closing marker first, matching Adobe flat layer order.
                let mut closing = Layer::default();
                closing.additional_info.name = Some("</Layer group>".to_string());
                closing.additional_info.section_divider = Some(SectionDivider {
                    divider_type: crate::api::types::SectionDividerType::BoundingSectionDivider,
                    blend_mode: None,
                    sub_type: None,
                });
                result.push(closing);

                // Add children.
                result.extend(flatten_layers(Some(child_children)));

                // Add the real folder marker last.
                let mut folder = child.clone();
                folder.children = None;
                let mut divider = folder.additional_info.section_divider.unwrap_or(SectionDivider {
                    divider_type: if child.opened.unwrap_or(true) {
                        crate::api::types::SectionDividerType::OpenFolder
                    } else {
                        crate::api::types::SectionDividerType::ClosedFolder
                    },
                    blend_mode: None,
                    sub_type: None,
                });
                divider.divider_type = if child.opened.unwrap_or(true) {
                    crate::api::types::SectionDividerType::OpenFolder
                } else {
                    crate::api::types::SectionDividerType::ClosedFolder
                };
                folder.additional_info.section_divider = Some(divider);
                result.push(folder);
            } else {
                result.push(child.clone());
            }
        }
    }

    result
}

#[derive(Debug, Clone)]
struct PreparedChannel {
    id: ChannelID,
    compression: Compression,
    payload: Vec<u8>,
}

#[derive(Debug, Clone)]
struct PreparedLayerChannels {
    entries: Vec<PreparedChannel>,
}

/// Write a single layer record
fn write_layer_record(
    writer: &mut PsdWriter,
    layer: &Layer,
    channel_payloads: &PreparedLayerChannels,
    options: &WriteOptions,
) -> Result<()> {
    let psb = options.psb.unwrap_or(false);
    writer.write_bytes(&encode_be(
        &LayerRecordBounds {
            top: layer.top.unwrap_or(0),
            left: layer.left.unwrap_or(0),
            bottom: layer.bottom.unwrap_or(0),
            right: layer.right.unwrap_or(0),
            channel_count: channel_payloads.entries.len() as u16,
        },
        "layer record bounds",
    )?)?;

    for entry in &channel_payloads.entries {
        let channel_payload_len = entry.payload.len() as u32;
        if psb {
            writer.write_bytes(&encode_be(
                &PsbChannelInfoRecord {
                    id: entry.id as i16,
                    high_length: 0,
                    low_length: channel_payload_len + 2,
                },
                "PSB channel info",
            )?)?;
        } else {
            writer.write_bytes(&encode_be(
                &ChannelInfoRecord {
                    id: entry.id as i16,
                    length: channel_payload_len + 2,
                },
                "channel info",
            )?)?;
        }
    }

    let blend_mode = layer.blend_mode.unwrap_or(BlendMode::Normal);
    let blend_mode_sig = from_blend_mode(blend_mode);
    let mut blend_mode_raw = [0u8; 4];
    blend_mode_raw.copy_from_slice(blend_mode_sig.as_bytes());
    let opacity = layer.opacity.unwrap_or(1.0);
    let mut flags = LayerBlendFlags::PHOTOSHOP_5;
    if layer.transparency_protected.unwrap_or(false) {
        flags |= LayerBlendFlags::TRANSPARENCY_PROTECTED;
    }
    if layer.hidden.unwrap_or(false) {
        flags |= LayerBlendFlags::HIDDEN;
    }
    writer.write_bytes(&encode_be(
        &LayerBlendRecord {
            signature: *b"8BIM",
            blend_mode: blend_mode_raw,
            opacity: (clamp(opacity, 0.0, 1.0) * 255.0).round() as u8,
            // TS always writes 0 here and uses image resource 1026 as the
            // semantic clipping source.
            clipping: 0,
            flags: flags.bits(),
            filler: 0,
        },
        "layer blend record",
    )?)?;

    // Write extra data
    writer.write_section(1, false, |writer| {
        // Write layer mask data
        writer.write_section(1, false, |writer| {
            if let Some(ref mask) = layer.additional_info.mask {
                let mut flags = LayerMaskStateBits::empty();
                if mask.position_relative_to_layer.unwrap_or(false) {
                    flags |= LayerMaskStateBits::POSITION_RELATIVE_TO_LAYER;
                }
                if mask.disabled.unwrap_or(false) {
                    flags |= LayerMaskStateBits::DISABLED;
                }
                if mask.from_vector_data.unwrap_or(false) {
                    flags |= LayerMaskStateBits::FROM_VECTOR_DATA;
                }
                let has_params = mask.user_mask_density.is_some()
                    || mask.user_mask_feather.is_some()
                    || mask.vector_mask_density.is_some()
                    || mask.vector_mask_feather.is_some();
                if has_params {
                    flags |= LayerMaskStateBits::HAS_PARAMETERS;
                }
                writer.write_bytes(&encode_be(
                    &LayerMaskPrefixRecord {
                        top: mask.top.unwrap_or(0),
                        left: mask.left.unwrap_or(0),
                        bottom: mask.bottom.unwrap_or(0),
                        right: mask.right.unwrap_or(0),
                        default_color: mask.default_color.unwrap_or(0),
                        flags: flags.bits(),
                    },
                    "layer mask prefix",
                )?)?;
                if has_params {
                    // Only write real-mask block when real mask data is present
                    if mask.real_flags_byte.is_some() {
                        writer.write_u8(mask.real_flags_byte.unwrap_or(0))?;
                        writer.write_u8(mask.real_default_color.unwrap_or(0))?;
                        writer.write_i32(mask.real_top.unwrap_or(0))?;
                        writer.write_i32(mask.real_left.unwrap_or(0))?;
                        writer.write_i32(mask.real_bottom.unwrap_or(0))?;
                        writer.write_i32(mask.real_right.unwrap_or(0))?;
                    }
                    let mut param_flags = LayerMaskParameterFlags::empty();
                    if mask.user_mask_density.is_some() {
                        param_flags |= LayerMaskParameterFlags::USER_MASK_DENSITY;
                    }
                    if mask.user_mask_feather.is_some() {
                        param_flags |= LayerMaskParameterFlags::USER_MASK_FEATHER;
                    }
                    if mask.vector_mask_density.is_some() {
                        param_flags |= LayerMaskParameterFlags::VECTOR_MASK_DENSITY;
                    }
                    if mask.vector_mask_feather.is_some() {
                        param_flags |= LayerMaskParameterFlags::VECTOR_MASK_FEATHER;
                    }
                    writer.write_u8(param_flags.bits())?;
                    if let Some(v) = mask.user_mask_density {
                        writer.write_u8(v as u8)?;
                    }
                    if let Some(v) = mask.user_mask_feather {
                        writer.write_f64(v)?;
                    }
                    if let Some(v) = mask.vector_mask_density {
                        writer.write_u8(v as u8)?;
                    }
                    if let Some(v) = mask.vector_mask_feather {
                        writer.write_f64(v)?;
                    }
                }
            }
            // Write uV filler block (required by some Photoshop versions)
            if layer.additional_info.mask.is_some() {
                writer.write_u16(0x0006)?;
                writer.write_zeros(38)?;
            }
            Ok(())
        })?;

        // Write blending ranges
        writer.write_section(1, false, |writer| {
            if let Some(ref ranges) = layer.blending_ranges_data {
                writer.write_bytes(&serialize_layer_blending_ranges(ranges))?;
            } else if should_emit_default_blending_ranges(layer, channel_payloads) {
                writer.write_bytes(&default_layer_blending_ranges_bytes(channel_payloads))?;
            }
            Ok(())
        })?;

        // Write layer name
        let name = layer.additional_info.name.as_deref().unwrap_or("");
        writer.write_pascal_string(name, 4)?;

        // Write tagged blocks (additional layer info)
        crate::format::additional_info::write_layer_additional_info_with_options(
            writer,
            &layer.additional_info,
            psb,
        )?;

        Ok(())
    })?;

    Ok(())
}

/// Write layer channel image data
fn write_layer_channel_data(
    writer: &mut PsdWriter,
    channel_payloads: &PreparedLayerChannels,
) -> Result<()> {
    for entry in &channel_payloads.entries {
        writer.write_u16(entry.compression as u16)?;
        writer.write_bytes(&entry.payload)?;
    }

    Ok(())
}

pub(crate) fn write_nested_layer_info_block(
    writer: &mut PsdWriter,
    layers: &[Layer],
    bits_per_channel: u8,
) -> Result<()> {
    let options = WriteOptions::default();
    let flattened = flatten_layers(Some(&layers.to_vec()));
    let prepared_payloads: Vec<PreparedLayerChannels> = flattened
        .iter()
        .map(|layer| prepare_layer_channels(layer, bits_per_channel, &options))
        .collect::<Result<Vec<PreparedLayerChannels>>>()?;

    writer.write_i16(flattened.len() as i16)?;
    for (layer, prepared) in flattened.iter().zip(prepared_payloads.iter()) {
        write_layer_record(writer, layer, prepared, &options)?;
    }
    for prepared in &prepared_payloads {
        write_layer_channel_data(writer, prepared)?;
    }
    Ok(())
}

/// Write global layer mask info
fn write_global_layer_mask_info(
    writer: &mut PsdWriter,
    info: Option<&GlobalLayerMaskInfo>,
) -> Result<()> {
    writer.write_section(1, false, |writer| {
        if let Some(info) = info {
            let record = GlobalLayerMaskRecord {
                overlay_color_space: info.overlay_color_space,
                color_space1: info.color_space1,
                color_space2: info.color_space2,
                color_space3: info.color_space3,
                color_space4: info.color_space4,
                opacity: info.opacity,
                kind: info.kind,
                reserved: [0; 3],
            };
            writer.write_bytes(&encode_be(&record, "global layer mask info")?)?;
        }
        Ok(())
    })
}

/// Write image data section
fn write_image_data(
    writer: &mut PsdWriter,
    psd: &Psd,
    options: &WriteOptions,
    global_alpha: bool,
) -> Result<()> {
    let bits_per_channel = psd.bits_per_channel.unwrap_or(8);
    let compression = preferred_channel_compression(bits_per_channel, options);
    writer.write_u16(compression as u16)?;

    let fallback_width = psd.width as usize;
    let fallback_height = psd.height as usize;
    let fallback_rgba = vec![0u8; fallback_width * fallback_height * 4];
    let (image_data, width, height) = if let Some(ref image_data) = psd.image_data {
        (
            image_data.data.as_slice(),
            image_data.width,
            image_data.height,
        )
    } else {
        (fallback_rgba.as_slice(), fallback_width, fallback_height)
    };

    let offsets: &[usize] = if global_alpha {
        &[0, 1, 2, 3]
    } else {
        &[0, 1, 2]
    };
    match compression {
        Compression::RawData => {
            for &offset in offsets {
                let raw = extract_channel_data_from_rgba(image_data, width, height, offset);
                writer.write_bytes(&expand_samples_for_depth(&raw, bits_per_channel))?;
            }
        }
        Compression::RleCompressed => {
            let mut compressed_channels = Vec::with_capacity(offsets.len());
            for &offset in offsets {
                let raw = extract_channel_data_from_rgba(image_data, width, height, offset);
                let expanded = expand_samples_for_depth(&raw, bits_per_channel);
                let row_bytes = width * bytes_per_sample(bits_per_channel);
                let compressed = compression::compress_rle(&expanded, row_bytes, height)?;
                compressed_channels.push(compressed);
            }

            // PSD composite RLE stores all row byte-counts first, then compressed row data.
            for channel in &compressed_channels {
                let table_len = height * 2;
                writer.write_bytes(&channel[..table_len])?;
            }
            for channel in &compressed_channels {
                let table_len = height * 2;
                writer.write_bytes(&channel[table_len..])?;
            }
        }
        Compression::ZipWithoutPrediction => {
            let mut planar = Vec::with_capacity(
                width * height * offsets.len() * bytes_per_sample(bits_per_channel),
            );
            for &offset in offsets {
                let raw = extract_channel_data_from_rgba(image_data, width, height, offset);
                planar.extend_from_slice(&expand_samples_for_depth(&raw, bits_per_channel));
            }
            let compressed = compression::compress_zip(&planar)?;
            writer.write_bytes(&compressed)?;
        }
        Compression::ZipWithPrediction => {
            let mut planar = Vec::with_capacity(
                width * height * offsets.len() * bytes_per_sample(bits_per_channel),
            );
            for &offset in offsets {
                let raw = extract_channel_data_from_rgba(image_data, width, height, offset);
                planar.extend_from_slice(&expand_samples_for_depth(&raw, bits_per_channel));
            }
            apply_prediction_planar(
                &mut planar,
                width,
                height,
                offsets.len(),
                bits_per_channel as u16,
            );
            let compressed = compression::compress_zip(&planar)?;
            writer.write_bytes(&compressed)?;
        }
    }

    Ok(())
}

fn layer_channel_payload(
    layer: &Layer,
    channel_id: ChannelID,
    bits_per_channel: u8,
    options: &WriteOptions,
) -> Result<Vec<u8>> {
    let (width, height) = layer_channel_dimensions(layer, channel_id);
    let raw = match channel_id {
        ChannelID::Color0 => {
            extract_layer_channel_data(layer.image_data.as_ref(), width, height, 0)
        }
        ChannelID::Color1 => {
            extract_layer_channel_data(layer.image_data.as_ref(), width, height, 1)
        }
        ChannelID::Color2 => {
            extract_layer_channel_data(layer.image_data.as_ref(), width, height, 2)
        }
        ChannelID::Transparency => {
            extract_layer_channel_data(layer.image_data.as_ref(), width, height, 3)
        }
        ChannelID::UserMask => extract_mask_channel_data(
            layer
                .additional_info
                .mask
                .as_ref()
                .and_then(|mask| mask.image_data.as_ref()),
            width,
            height,
        ),
        ChannelID::RealUserMask => extract_mask_channel_data(
            layer
                .additional_info
                .real_mask
                .as_ref()
                .and_then(|mask| mask.image_data.as_ref()),
            width,
            height,
        ),
        _ => vec![0; width * height],
    };
    let expanded = expand_samples_for_depth(&raw, bits_per_channel);
    match preferred_channel_compression(bits_per_channel, options) {
        Compression::RawData => Ok(expanded),
        Compression::RleCompressed => {
            let row_bytes = width * bytes_per_sample(bits_per_channel);
            compression::compress_rle(&expanded, row_bytes, height)
        }
        Compression::ZipWithoutPrediction => compression::compress_zip(&expanded),
        Compression::ZipWithPrediction => compression::compress_zip_with_prediction(
            &expanded,
            width,
            height,
            bits_per_channel as u16,
        ),
    }
}

fn prepare_layer_channels(
    layer: &Layer,
    bits_per_channel: u8,
    options: &WriteOptions,
) -> Result<PreparedLayerChannels> {
    if let Some(ref raw_data) = layer.raw_data {
        if raw_data.bits_per_channel == bits_per_channel {
            let mut entries = Vec::with_capacity(raw_data.channels.len());
            for channel in &raw_data.channels {
                let (width, height) = layer_channel_dimensions(layer, channel.id);
                let default_len = width * height * bytes_per_sample(bits_per_channel);
                let raw = channel.data.clone().unwrap_or_else(|| vec![0; default_len]);
                let payload = match channel.compression {
                    Compression::RawData => raw,
                    Compression::RleCompressed => {
                        let row_bytes = width * bytes_per_sample(bits_per_channel);
                        compression::compress_rle(&raw, row_bytes, height)?
                    }
                    Compression::ZipWithoutPrediction => compression::compress_zip(&raw)?,
                    Compression::ZipWithPrediction => compression::compress_zip_with_prediction(
                        &raw,
                        width,
                        height,
                        bits_per_channel as u16,
                    )?,
                };
                entries.push(PreparedChannel {
                    id: channel.id,
                    compression: channel.compression,
                    payload,
                });
            }
            return Ok(PreparedLayerChannels { entries });
        }
    }

    let mut channel_ids = vec![
        ChannelID::Transparency,
        ChannelID::Color0,
        ChannelID::Color1,
        ChannelID::Color2,
    ];
    if layer
        .additional_info
        .mask
        .as_ref()
        .and_then(|mask| mask.image_data.as_ref())
        .is_some()
    {
        channel_ids.push(ChannelID::UserMask);
    }
    if layer
        .additional_info
        .real_mask
        .as_ref()
        .and_then(|mask| mask.image_data.as_ref())
        .is_some()
    {
        channel_ids.push(ChannelID::RealUserMask);
    }
    let compression = preferred_channel_compression(bits_per_channel, options);
    let mut entries = Vec::with_capacity(channel_ids.len());
    for &channel_id in &channel_ids {
        entries.push(PreparedChannel {
            id: channel_id,
            compression,
            payload: layer_channel_payload(layer, channel_id, bits_per_channel, options)?,
        });
    }
    Ok(PreparedLayerChannels { entries })
}

fn layer_channel_dimensions(layer: &Layer, channel_id: ChannelID) -> (usize, usize) {
    if matches!(channel_id, ChannelID::UserMask | ChannelID::RealUserMask) {
        let (_, _, width, height) = layer_channel_bounds(layer, channel_id);
        return (width, height);
    }
    let (_, _, width, height) = layer_channel_bounds(layer, channel_id);
    (width, height)
}

fn layer_channel_bounds(layer: &Layer, channel_id: ChannelID) -> (i32, i32, usize, usize) {
    let layer_left = layer.left.unwrap_or(0);
    let layer_top = layer.top.unwrap_or(0);
    let layer_right = layer.right.unwrap_or(0);
    let layer_bottom = layer.bottom.unwrap_or(0);

    match channel_id {
        ChannelID::UserMask => {
            if let Some(mask) = layer.additional_info.mask.as_ref() {
                let left = mask.left.unwrap_or(layer_left);
                let top = mask.top.unwrap_or(layer_top);
                let right = mask.right.unwrap_or(left);
                let bottom = mask.bottom.unwrap_or(top);
                return (
                    left,
                    top,
                    (right - left).max(0) as usize,
                    (bottom - top).max(0) as usize,
                );
            }
        }
        ChannelID::RealUserMask => {
            if let Some(mask) = layer.additional_info.real_mask.as_ref() {
                if mask.left.is_some()
                    || mask.top.is_some()
                    || mask.right.is_some()
                    || mask.bottom.is_some()
                {
                    let left = mask.left.unwrap_or(layer_left);
                    let top = mask.top.unwrap_or(layer_top);
                    let right = mask.right.unwrap_or(left);
                    let bottom = mask.bottom.unwrap_or(top);
                    return (
                        left,
                        top,
                        (right - left).max(0) as usize,
                        (bottom - top).max(0) as usize,
                    );
                }
            }
            if let Some(mask) = layer.additional_info.mask.as_ref() {
                let left = mask.real_left.or(mask.left).unwrap_or(layer_left);
                let top = mask.real_top.or(mask.top).unwrap_or(layer_top);
                let right = mask.real_right.or(mask.right).unwrap_or(left);
                let bottom = mask.real_bottom.or(mask.bottom).unwrap_or(top);
                return (
                    left,
                    top,
                    (right - left).max(0) as usize,
                    (bottom - top).max(0) as usize,
                );
            }
        }
        _ => {}
    }

    (
        layer_left,
        layer_top,
        (layer_right - layer_left).max(0) as usize,
        (layer_bottom - layer_top).max(0) as usize,
    )
}

fn preferred_channel_compression(bits_per_channel: u8, options: &WriteOptions) -> Compression {
    if !options.compress.unwrap_or(true) {
        return Compression::RawData;
    }
    if bits_per_channel == 8 {
        Compression::RleCompressed
    } else {
        Compression::ZipWithPrediction
    }
}

fn bytes_per_sample(bits_per_channel: u8) -> usize {
    match bits_per_channel {
        8 => 1,
        16 => 2,
        32 => 4,
        _ => 1,
    }
}

fn apply_prediction_planar(
    data: &mut [u8],
    width: usize,
    height: usize,
    channels: usize,
    depth: u16,
) {
    let bytes_per_sample = match depth {
        8 => 1usize,
        16 => 2,
        32 => 4,
        _ => return,
    };
    let plane_len = width * height * bytes_per_sample;

    for channel in 0..channels {
        let plane_start = channel * plane_len;
        match depth {
            8 => {
                for row in 0..height {
                    let start = plane_start + row * width;
                    for x in (1..width).rev() {
                        data[start + x] = data[start + x].wrapping_sub(data[start + x - 1]);
                    }
                }
            }
            16 => {
                let row_bytes = width * 2;
                for row in 0..height {
                    let start = plane_start + row * row_bytes;
                    for i in (start + 1..start + row_bytes).rev() {
                        data[i] = data[i].wrapping_sub(data[i - 1]);
                    }
                }
            }
            32 => {
                let row_bytes = width * 4;
                let mut reordered = vec![0u8; row_bytes];
                for row in 0..height {
                    let row_off = plane_start + row * row_bytes;
                    for pixel in 0..width {
                        let src = row_off + pixel * 4;
                        reordered[pixel] = data[src];
                        reordered[width + pixel] = data[src + 1];
                        reordered[width * 2 + pixel] = data[src + 2];
                        reordered[width * 3 + pixel] = data[src + 3];
                    }
                    for plane in 0..4usize {
                        let base = plane * width;
                        for i in (1..width).rev() {
                            reordered[base + i] =
                                reordered[base + i].wrapping_sub(reordered[base + i - 1]);
                        }
                    }
                    data[row_off..row_off + row_bytes].copy_from_slice(&reordered);
                }
            }
            _ => {}
        }
    }
}

fn expand_samples_for_depth(samples: &[u8], bits_per_channel: u8) -> Vec<u8> {
    match bits_per_channel {
        8 => samples.to_vec(),
        16 => {
            let mut out = Vec::with_capacity(samples.len() * 2);
            for &sample in samples {
                out.push(sample);
                out.push(sample);
            }
            out
        }
        32 => {
            let mut out = Vec::with_capacity(samples.len() * 4);
            for &sample in samples {
                out.extend_from_slice(&(sample as f32 / 255.0).to_be_bytes());
            }
            out
        }
        _ => samples.to_vec(),
    }
}

fn extract_layer_channel_data(
    image_data: Option<&crate::api::types::PixelData>,
    width: usize,
    height: usize,
    offset: usize,
) -> Vec<u8> {
    let mut out = vec![0u8; width * height];
    if let Some(image_data) = image_data {
        for i in 0..(width * height) {
            let src = i * 4 + offset;
            if src < image_data.data.len() {
                out[i] = image_data.data[src];
            } else if offset == 3 {
                out[i] = 255;
            }
        }
    } else if offset == 3 {
        out.fill(255);
    }
    out
}

fn extract_mask_channel_data(
    image_data: Option<&crate::api::types::PixelData>,
    width: usize,
    height: usize,
) -> Vec<u8> {
    let expected_len = width * height;
    let mut out = vec![0u8; expected_len];
    if let Some(image_data) = image_data {
        let copy_len = expected_len.min(image_data.data.len());
        out[..copy_len].copy_from_slice(&image_data.data[..copy_len]);
    }
    out
}

fn extract_channel_data_from_rgba(
    image_data: &[u8],
    width: usize,
    height: usize,
    offset: usize,
) -> Vec<u8> {
    let mut out = vec![0u8; width * height];
    for i in 0..(width * height) {
        let src = i * 4 + offset;
        if src < image_data.len() {
            out[i] = image_data[src];
        } else if offset == 3 {
            out[i] = 255;
        }
    }
    out
}

fn serialize_layer_blending_ranges(ranges: &crate::api::layer::LayerBlendingRangesData) -> Vec<u8> {
    let mut out = Vec::new();
    let mut write_pair = |pair: &crate::api::layer::LayerBlendingRangePair| {
        out.push(pair.src_black);
        out.push(pair.src_white);
        out.push(pair.dst_black);
        out.push(pair.dst_white);
    };

    if let Some(ref pair) = ranges.composite_gray {
        write_pair(pair);
    }
    for pair in &ranges.channels {
        write_pair(pair);
    }
    out
}

fn should_emit_default_blending_ranges(
    layer: &Layer,
    channel_payloads: &PreparedLayerChannels,
) -> bool {
    layer.image_data.is_some()
        || layer.raw_data.is_some()
        || channel_payloads.entries.iter().any(|entry| {
            matches!(
                entry.id,
                ChannelID::Transparency | ChannelID::Color0 | ChannelID::Color1 | ChannelID::Color2 | ChannelID::Color3
            )
        })
}

fn default_layer_blending_ranges_bytes(channel_payloads: &PreparedLayerChannels) -> Vec<u8> {
    let channel_count = channel_payloads
        .entries
        .iter()
        .filter(|entry| {
            matches!(
                entry.id,
                ChannelID::Transparency | ChannelID::Color0 | ChannelID::Color1 | ChannelID::Color2 | ChannelID::Color3
            )
        })
        .count();

    let pair_count = channel_count + 1;
    let mut out = Vec::with_capacity(pair_count * 8);
    for _ in 0..pair_count {
        out.extend_from_slice(&[0x00, 0x00, 0xFF, 0xFF, 0x00, 0x00, 0xFF, 0xFF]);
    }
    out
}

/// Apply resource prewrite: map psd.path_selection_descriptor to resource 3000
fn apply_resource_prewrite(psd: &mut Psd) {
    if let Some(ref descriptor) = psd.path_selection_descriptor.clone() {
        let resources = psd.image_resources.get_or_insert_with(Default::default);
        resources
            .descriptor_resources
            .insert(3000, descriptor.clone());
    }
}

/// Apply text prewrite: synthesize Txt2 engine data from TySh layer text data
fn apply_text_prewrite(psd: &mut Psd) -> Result<()> {
    use crate::support::engine_data::EngineValue;
    use std::collections::HashMap;

    if psd.additional_info.text_engine.is_some() {
        return Ok(());
    }

    let mut text_objects = Vec::new();
    let mut document_resources: Option<EngineValue> = None;

    if let Some(ref mut layers) = psd.children {
        for layer in layers.iter_mut() {
            if let Some(ref mut text) = layer.additional_info.text {
                let mut style_run_array = Vec::new();
                let mut paragraph_run_array = Vec::new();

                if let Some(ref text_desc) = text.text_data {
                    if let Some(crate::support::descriptor::DescriptorValue::DataBytes(engine_bytes)) =
                        text_desc.items.get("EngineData")
                    {
                        if let Ok(EngineValue::Object(engine_map)) =
                            crate::support::engine_data::parse_engine_data(engine_bytes)
                        {
                            if let Some(EngineValue::Object(engine_dict)) =
                                engine_map.get("EngineDict")
                            {
                                if let Some(EngineValue::Object(style_run)) =
                                    engine_dict.get("StyleRun")
                                {
                                    if let Some(EngineValue::Array(run_array)) =
                                        style_run.get("RunArray")
                                    {
                                        style_run_array = run_array.clone();
                                    }
                                }
                                if let Some(EngineValue::Object(paragraph_run)) =
                                    engine_dict.get("ParagraphRun")
                                {
                                    if let Some(EngineValue::Array(run_array)) =
                                        paragraph_run.get("RunArray")
                                    {
                                        paragraph_run_array = run_array.clone();
                                    }
                                }
                            }

                            if document_resources.is_none() {
                                if let Some(value) = engine_map
                                    .get("DocumentResources")
                                    .cloned()
                                    .or_else(|| engine_map.get("ResourceDict").cloned())
                                {
                                    document_resources = Some(value);
                                }
                            }
                        }
                    }
                }

                // Build richer text object matching TS _Model structure
                let mut style_run = HashMap::new();
                style_run.insert("_RunArray".to_string(), EngineValue::Array(style_run_array));

                let mut paragraph_run = HashMap::new();
                paragraph_run.insert(
                    "_RunArray".to_string(),
                    EngineValue::Array(paragraph_run_array),
                );

                let mut model = HashMap::new();
                model.insert("_StyleRun".to_string(), EngineValue::Object(style_run));
                model.insert(
                    "_ParagraphRun".to_string(),
                    EngineValue::Object(paragraph_run),
                );

                let mut text_obj = HashMap::new();
                text_obj.insert("_Model".to_string(), EngineValue::Object(model));
                text_objects.push(EngineValue::Object(text_obj));
            }
        }
    }

    if !text_objects.is_empty() {
        let mut synthesized = HashMap::new();

        let mut doc_objects = HashMap::new();
        doc_objects.insert("_TextObjects".to_string(), EngineValue::Array(text_objects));
        synthesized.insert(
            "_DocumentObjects".to_string(),
            EngineValue::Object(doc_objects),
        );

        if let Some(doc_resources) = document_resources {
            synthesized
                .entry("_DocumentResources".to_string())
                .or_insert(doc_resources);
        }

        psd.additional_info.text_engine = Some(crate::format::additional_info::TextEngineBlock {
            data: EngineValue::Object(synthesized),
            raw: None,
        });
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::psd::ReadOptions;
    use crate::format::additional_info::LayerAdditionalInfo;
    use std::fs;
    use std::io::Cursor;
    use std::path::PathBuf;
    use std::process::Command;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn roundtrip_sample_path() -> PathBuf {
        PathBuf::from("/Users/jakubkolcar/Downloads/3D-preview-sample/3d-preview-mockup.psd")
    }

    fn find_layer_by_name_with_user_mask<'a>(layers: &'a [Layer], name: &str) -> Option<&'a Layer> {
        for layer in layers {
            if layer.additional_info.name.as_deref() == Some(name)
                && layer
                    .additional_info
                    .mask
                    .as_ref()
                    .and_then(|mask| mask.image_data.as_ref())
                    .is_some()
            {
                return Some(layer);
            }
            if let Some(children) = layer.children.as_ref() {
                if let Some(found) = find_layer_by_name_with_user_mask(children, name) {
                    return Some(found);
                }
            }
        }
        None
    }

    #[test]
    fn test_write_signature() {
        let mut writer = PsdWriter::with_default_capacity();
        writer.write_signature("8BPS").unwrap();
        assert_eq!(writer.get_buffer(), b"8BPS");
    }

    #[test]
    fn test_write_pascal_string() {
        let mut writer = PsdWriter::with_default_capacity();
        writer.write_pascal_string("Hi", 4).unwrap();
        assert_eq!(writer.get_buffer(), &[2, b'H', b'i', 0]);
    }

    #[test]
    fn test_write_integers() {
        let mut writer = PsdWriter::with_default_capacity();
        writer.write_u16(256).unwrap();
        writer.write_u32(512).unwrap();
        assert_eq!(writer.get_buffer(), &[0x01, 0x00, 0x00, 0x00, 0x02, 0x00]);
    }

    #[test]
    fn test_flatten_layers_emits_section_divider_markers_for_groups() {
        let group = Layer {
            opened: Some(false),
            additional_info: LayerAdditionalInfo {
                name: Some("Group".to_string()),
                ..Default::default()
            },
            children: Some(vec![Layer {
                additional_info: LayerAdditionalInfo {
                    name: Some("Leaf".to_string()),
                    ..Default::default()
                },
                ..Default::default()
            }]),
            ..Default::default()
        };

        let flattened = flatten_layers(Some(&vec![group]));
        assert_eq!(flattened.len(), 3);
        assert_eq!(
            flattened[0]
                .additional_info
                .section_divider
                .as_ref()
                .map(|divider| divider.divider_type),
            Some(crate::api::types::SectionDividerType::BoundingSectionDivider)
        );
        assert_eq!(
            flattened[2]
                .additional_info
                .section_divider
                .as_ref()
                .map(|divider| divider.divider_type),
            Some(crate::api::types::SectionDividerType::ClosedFolder)
        );
    }

    #[test]
    fn test_write_psd_uses_actual_composite_channel_count() {
        let psd = Psd {
            width: 1,
            height: 1,
            channels: Some(4),
            bits_per_channel: Some(8),
            color_mode: Some(ColorMode::RGB),
            image_data: Some(crate::api::types::PixelData {
                data: vec![12, 34, 56, 255],
                width: 1,
                height: 1,
            }),
            ..Default::default()
        };

        let bytes = write_psd(
            &psd,
            &WriteOptions {
                compress: Some(true),
                ..Default::default()
            },
        )
        .expect("write psd");

        assert_eq!(u16::from_be_bytes([bytes[12], bytes[13]]), 3);

        let reparsed = crate::read_psd(
            Cursor::new(bytes),
            ReadOptions {
                skip_composite_image_data: Some(false),
                ..Default::default()
            },
        )
        .expect("reparse written psd");
        assert_eq!(reparsed.channels, Some(3));
    }

    #[test]
    fn test_group_roundtrip_preserves_children_and_open_state() {
        let child = Layer {
            top: Some(0),
            left: Some(0),
            bottom: Some(1),
            right: Some(1),
            image_data: Some(crate::api::types::PixelData {
                data: vec![255, 0, 0, 255],
                width: 1,
                height: 1,
            }),
            additional_info: LayerAdditionalInfo {
                name: Some("Child".to_string()),
                ..Default::default()
            },
            ..Default::default()
        };
        let group = Layer {
            opened: Some(false),
            children: Some(vec![child]),
            additional_info: LayerAdditionalInfo {
                name: Some("Group".to_string()),
                ..Default::default()
            },
            ..Default::default()
        };
        let psd = Psd {
            width: 1,
            height: 1,
            color_mode: Some(ColorMode::RGB),
            bits_per_channel: Some(8),
            children: Some(vec![group]),
            ..Default::default()
        };

        let bytes = write_psd(
            &psd,
            &WriteOptions {
                compress: Some(false),
                ..Default::default()
            },
        )
        .expect("write grouped psd");
        let loaded = crate::read_psd(
            Cursor::new(bytes),
            ReadOptions {
                skip_layer_image_data: Some(false),
                skip_composite_image_data: Some(true),
                ..Default::default()
            },
        )
        .expect("read grouped psd");

        let roots = loaded.children.expect("root layers");
        assert_eq!(roots.len(), 1);
        assert_eq!(roots[0].additional_info.name.as_deref(), Some("Group"));
        assert_eq!(roots[0].opened, Some(false));
        assert_eq!(
            roots[0]
                .children
                .as_ref()
                .expect("group children")
                .first()
                .and_then(|child| child.additional_info.name.as_deref()),
            Some("Child")
        );
    }

    #[test]
    fn test_prepare_layer_channels_includes_mask_channels_without_raw_data() {
        let layer = Layer {
            left: Some(0),
            top: Some(0),
            right: Some(2),
            bottom: Some(1),
            image_data: Some(crate::api::types::PixelData {
                data: vec![10, 0, 0, 255, 20, 0, 0, 128],
                width: 2,
                height: 1,
            }),
            additional_info: LayerAdditionalInfo {
                mask: Some(crate::api::layer::LayerMaskData {
                    left: Some(0),
                    top: Some(0),
                    right: Some(1),
                    bottom: Some(1),
                    image_data: Some(crate::api::types::PixelData {
                        data: vec![77],
                        width: 1,
                        height: 1,
                    }),
                    ..Default::default()
                }),
                real_mask: Some(crate::api::layer::LayerMaskData {
                    left: Some(1),
                    top: Some(0),
                    right: Some(2),
                    bottom: Some(1),
                    image_data: Some(crate::api::types::PixelData {
                        data: vec![33],
                        width: 1,
                        height: 1,
                    }),
                    ..Default::default()
                }),
                ..Default::default()
            },
            ..Default::default()
        };

        let prepared = prepare_layer_channels(
            &layer,
            8,
            &WriteOptions {
                compress: Some(false),
                ..Default::default()
            },
        )
        .expect("prepare channels");

        let channel_ids: Vec<_> = prepared.entries.iter().map(|entry| entry.id).collect();
        assert_eq!(
            channel_ids,
            vec![
                ChannelID::Transparency,
                ChannelID::Color0,
                ChannelID::Color1,
                ChannelID::Color2,
                ChannelID::UserMask,
                ChannelID::RealUserMask,
            ]
        );
        assert_eq!(prepared.entries[4].payload, vec![77]);
        assert_eq!(prepared.entries[5].payload, vec![33]);
    }

    #[test]
    #[ignore = "external TS parser assumes 4-byte layer tagged-block padding; Adobe spec validation now uses even-byte layer block padding"]
    fn test_roundtrip_sample_opens_in_ts_parser_subprocess() {
        let original = fs::read(roundtrip_sample_path()).expect("read roundtrip sample");
        let psd = crate::read_psd(Cursor::new(original), ReadOptions::default())
            .expect("parse roundtrip sample");
        let output = write_psd(&psd, &WriteOptions::default()).expect("write roundtrip sample");

        let output_path = std::env::temp_dir().join(format!(
            "psd-great-roundtrip-{}-{}.psd",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("system time")
                .as_nanos()
        ));
        fs::write(&output_path, output).expect("write temporary roundtrip sample");

        let command_output = Command::new("npx")
            .args([
                "--yes",
                "tsx",
                "-e",
                "import { parsePsd } from './src/index.ts';\
                 import fs from 'node:fs';\
                 const bytes = fs.readFileSync(process.argv[1]);\
                 const psd = parsePsd(bytes);\
                 console.log(JSON.stringify({ width: psd.width, height: psd.height, layers: psd.layers.length }));",
                output_path.to_str().expect("utf-8 temp path"),
            ])
            .current_dir("../photoshop/psd")
            .output()
            .expect("run ts parser subprocess");

        let _ = fs::remove_file(&output_path);

        assert!(
            command_output.status.success(),
            "TS parser failed: {}",
            String::from_utf8_lossy(&command_output.stderr)
        );
    }

    #[test]
    fn test_prepare_layer_channels_for_sample_masked_layer_includes_user_mask() {
        let original = fs::read(roundtrip_sample_path()).expect("read roundtrip sample");
        let psd = crate::read_psd(Cursor::new(original), ReadOptions::default())
            .expect("parse original sample for synthesized write");
        let layer = find_layer_by_name_with_user_mask(
            psd.children.as_deref().unwrap_or(&[]),
            "4",
        )
        .expect("sample layer with user mask");

        let prepared = prepare_layer_channels(
            layer,
            psd.bits_per_channel.unwrap_or(8),
            &WriteOptions::default(),
        )
        .expect("prepare sample channels");

        let channel_ids: Vec<_> = prepared.entries.iter().map(|entry| entry.id).collect();
        assert!(
            channel_ids.contains(&ChannelID::UserMask),
            "prepared channels should include UserMask, got {channel_ids:?}"
        );
    }

}

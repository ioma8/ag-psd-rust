//! PSD file writer implementation
//!
//! Provides functionality to write PSD files.

use crate::api::layer::Layer;
use crate::api::psd::{GlobalLayerMaskInfo, Psd, WriteOptions};
use crate::api::types::{BlendMode, ChannelID, Color, ColorMode, Compression};
use crate::format::additional_info::SectionDivider;
use crate::support::binrw_support::{
    encode_be, ChannelInfoRecord, GlobalLayerMaskRecord, LayerBlendRecord, LayerMaskPrefixRecord,
    LayerRecordBounds, PsbChannelInfoRecord, PsdHeaderRecord,
};
use crate::support::compression;
use crate::support::error::{PsdError, Result};
use crate::support::helpers::{
    clamp, from_blend_mode, has_alpha, LayerBlendFlags, LayerMaskParameterFlags, LayerMaskStateBits,
};
use byteorder::{BigEndian, WriteBytesExt};
use std::io::Cursor;

/// PSD writer for binary data
pub struct PsdWriter {
    buffer: Vec<u8>,
    pub offset: usize,
    /// PSB (large) record layout in effect for structures this writer emits,
    /// so nested Lr16/Lr32 blocks can use document-consistent widths.
    pub large: bool,
    pub color_mode: ColorMode,
    pub global_alpha: bool,
}

impl PsdWriter {
    /// Create a new PSD writer with initial capacity
    pub fn new(capacity: usize) -> Self {
        Self {
            buffer: Vec::with_capacity(capacity),
            offset: 0,
            large: false,
            color_mode: ColorMode::RGB,
            global_alpha: false,
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
            let new_len = required.max(self.buffer.capacity().max(64) * 2);
            self.buffer.resize(new_len, 0);
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

    /// Write the legacy Pascal layer name.
    ///
    /// The PSD layer record limits this field to 255 bytes, but the full
    /// Unicode name lives in the `luni` tagged block, so the legacy field is
    /// truncated to fit instead of failing the whole write. Non-ASCII
    /// characters map to `?` one per character so multibyte UTF-8 never
    /// produces several question marks per character.
    pub fn write_legacy_pascal_layer_name(&mut self, text: &str, pad_to: usize) -> Result<()> {
        let mut length: usize = 0;
        let mut units: Vec<u8> = Vec::new();
        for ch in text.chars() {
            let mapped: u8 = if ch.is_ascii() { ch as u8 } else { b'?' };
            if length == 255 {
                break;
            }
            units.push(mapped);
            length += 1;
        }

        self.write_u8(length as u8)?;
        self.write_bytes(&units)?;

        let mut padded_length = length + 1; // Include length byte
        while padded_length % pad_to != 0 {
            self.write_u8(0)?;
            padded_length += 1;
        }

        Ok(())
    }

    /// Write a Unicode string (UTF-16 BE)
    pub fn write_unicode_string(&mut self, text: &str) -> Result<()> {
        let units: Vec<u16> = text.encode_utf16().collect();
        self.write_u32(units.len() as u32)?;
        for unit in units {
            self.write_u16(unit)?;
        }
        Ok(())
    }

    /// Write a Unicode string with padding
    pub fn write_unicode_string_with_padding(&mut self, text: &str) -> Result<()> {
        let units: Vec<u16> = text.encode_utf16().collect();
        self.write_u32((units.len() + 1) as u32)?;
        for unit in units {
            self.write_u16(unit)?;
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
        let content_length = (self.offset - start_offset) as u64;
        if content_length > u32::MAX as u64 {
            return Err(PsdError::UnsupportedFeature(format!(
                "Section of {} bytes exceeds the 4GiB wire limit",
                content_length
            )));
        }
        let content_length = content_length as u32;
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

/// Channel count to use when an unchanged native composite is available.
fn active_native_channel_count(
    psd: &Psd,
    bits_per_channel: u8,
    color_mode: ColorMode,
) -> Option<usize> {
    let native = psd.composite_native.as_ref()?;
    if native.bits_per_channel as u8 != bits_per_channel || native.color_mode != color_mode {
        return None;
    }
    let preview_unchanged = match psd.image_data.as_ref() {
        Some(image) => image.data == native.preview,
        None => native.preview.is_empty(),
    };
    if !preview_unchanged || native.channels.is_empty() {
        return None;
    }
    Some(native.channels.len())
}

/// Write a PSD file
pub fn write_psd(psd: &Psd, options: &WriteOptions) -> Result<Vec<u8>> {
    if psd.width == 0 || psd.height == 0 {
        return Err(PsdError::InvalidFormat("Invalid document size".to_string()));
    }
    // A document whose composite was skipped on read must not be silently
    // rewritten with synthesized black pixels unless explicitly requested.
    if psd.composite_skipped
        && psd.image_data.is_none()
        && psd.composite_native.is_none()
        && !options.overwrite_skipped_composite.unwrap_or(false)
    {
        return Err(PsdError::UnsupportedFeature(
            "document composite image data was skipped on read; \
             refusing to synthesize replacement pixels (set \
             WriteOptions::overwrite_skipped_composite to override)"
                .to_string(),
        ));
    }
    if psd.layer_image_data_skipped && !options.overwrite_skipped_composite.unwrap_or(false) {
        return Err(PsdError::UnsupportedFeature(
            "layer image data was skipped on read; refusing to synthesize replacement pixels (set WriteOptions::overwrite_skipped_composite to override)".to_string(),
        ));
    }
    if psd.linked_files_data_skipped && !options.overwrite_skipped_composite.unwrap_or(false) {
        return Err(PsdError::UnsupportedFeature(
            "linked-file payloads were skipped on read; refusing to synthesize empty payloads (set WriteOptions::overwrite_skipped_composite to override)".to_string(),
        ));
    }
    // Options that are part of the public surface but not implemented return
    // an explicit error when requested, instead of silently doing nothing.
    for (name, requested) in [
        ("generate_thumbnail", options.generate_thumbnail),
        ("trim_image_data", options.trim_image_data),
        ("no_background", options.no_background),
        ("log_missing_features", options.log_missing_features),
    ] {
        if requested == Some(true) {
            return Err(PsdError::UnsupportedFeature(format!(
                "WriteOptions::{} is not implemented",
                name
            )));
        }
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
    writer.large = options.psb.unwrap_or(false);

    let color_mode = psd.color_mode.unwrap_or(ColorMode::RGB);
    let global_alpha = if let Some(ref image_data) = psd.image_data {
        has_alpha(image_data)
    } else {
        false
    };
    // The header channel count and the planes emitted in the image-data
    // section agree: native planes when an unchanged save can reuse them,
    // otherwise one shared plan.
    let bits_per_channel = psd.bits_per_channel.unwrap_or(8);
    let declared_alpha = psd
        .channels
        .map(|channels| {
            let base = match color_mode {
                ColorMode::Grayscale | ColorMode::Bitmap | ColorMode::Indexed => 1,
                ColorMode::CMYK => 4,
                _ => 3,
            };
            channels as usize == base + 1
        })
        .unwrap_or(false);
    let global_alpha = global_alpha || declared_alpha;
    writer.color_mode = color_mode;
    writer.global_alpha = global_alpha;
    let channel_count = match active_native_channel_count(psd, bits_per_channel, color_mode) {
        Some(native_channels) => native_channels as u16,
        None => composite_plan(color_mode, global_alpha, psd.image_data.is_some())?.len() as u16,
    };
    if let Some(declared_channels) = psd.channels {
        if declared_channels > channel_count {
            return Err(PsdError::UnsupportedFeature(format!(
                "document declares {} composite channels but the available data provides {}; refusing to drop channels",
                declared_channels, channel_count
            )));
        }
    }
    if channel_count == 0 || channel_count > 56 {
        return Err(PsdError::InvalidFormat(format!(
            "Invalid composite channel count: {}",
            channel_count
        )));
    }

    // Apply prewrite passes
    let mut psd = psd.clone();
    if options.overwrite_skipped_thumbnail.unwrap_or(false) {
        if let Some(resources) = psd.image_resources.as_mut() {
            if resources.thumbnail_skipped {
                resources.thumbnail_skipped = false;
                resources.thumbnail_raw = None;
            }
        }
    }
    if options.invalidate_text_layers.unwrap_or(false) {
        crate::format::additional_info::invalidate_text_layer_caches(&mut psd);
    }
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
        let color_mode = psd.color_mode.unwrap_or(ColorMode::RGB);
        if !matches!(color_mode, ColorMode::RGB | ColorMode::Grayscale) {
            // CMYK/Indexed/Bitmap samples cannot be reconstructed from the
            // 8-bit RGBA preview; require the retained native channels.
            let has_synthesized = layers.iter().any(|layer| {
                !layer_raw_is_current(layer) && layer.image_data.is_some()
            });
            if has_synthesized {
                return Err(PsdError::UnsupportedFeature(format!(
                    "Writing layers without current raw channel data is only supported in RGB mode (document is {color_mode:?})"
                )));
            }
        }
        let prepared_payloads: Vec<PreparedLayerChannels> = layers
            .iter()
            .map(|layer| prepare_layer_channels(layer, color_mode, bits_per_channel, options))
            .collect::<Result<Vec<PreparedLayerChannels>>>()?;

        let layer_count = if global_alpha {
            -layer_count_i16(layers.len())?
        } else {
            layer_count_i16(layers.len())?
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
                closing.additional_info.id = child.additional_info.id;
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
                let mut divider =
                    folder
                        .additional_info
                        .section_divider
                        .unwrap_or(SectionDivider {
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

fn layer_count_i16(count: usize) -> Result<i16> {
    i16::try_from(count).map_err(|_| {
        PsdError::InvalidFormat(format!("Too many layers: {count} (max {})", i16::MAX))
    })
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
        // The wire length includes the 2-byte compression header, so the
        // payload itself must fit in u32::MAX - 2.
        let wire_len = u32::try_from(entry.payload.len())
            .ok()
            .and_then(|len| len.checked_add(2))
            .ok_or_else(|| {
                PsdError::UnsupportedFeature(format!(
                    "Channel payload of {} bytes exceeds the 4GiB wire limit",
                    entry.payload.len()
                ))
            })?;
        if psb {
            writer.write_bytes(&encode_be(
                &PsbChannelInfoRecord {
                    id: entry.id.as_i16(),
                    high_length: 0,
                    low_length: wire_len,
                },
                "PSB channel info",
            )?)?;
        } else {
            writer.write_bytes(&encode_be(
                &ChannelInfoRecord {
                    id: entry.id.as_i16(),
                    length: wire_len,
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
    // Rebuild the record flag byte: start from the original wire flags when
    // the layer was loaded (preserving unmodeled and pixel-data-irrelevant
    // bits), otherwise the standard Photoshop 5 defaults, then apply the typed
    // accessors that are explicitly present.
    let mut flags = match layer.raw_blend_flags {
        Some(original) => LayerBlendFlags::from_bits_retain(original),
        None => LayerBlendFlags::PHOTOSHOP_5,
    };
    match layer.transparency_protected {
        Some(true) => flags |= LayerBlendFlags::TRANSPARENCY_PROTECTED,
        Some(false) => flags.remove(LayerBlendFlags::TRANSPARENCY_PROTECTED),
        None => {}
    }
    match layer.hidden {
        Some(true) => flags |= LayerBlendFlags::HIDDEN,
        Some(false) => flags.remove(LayerBlendFlags::HIDDEN),
        None => {}
    }
    let clipping = layer.clipping.unwrap_or(0);
    let clipping_byte = u8::try_from(clipping).map_err(|_| {
        PsdError::UnsupportedFeature(format!(
            "Layer clipping value {} does not fit the wire byte",
            clipping
        ))
    })?;
    writer.write_bytes(&encode_be(
        &LayerBlendRecord {
            signature: *b"8BIM",
            blend_mode: blend_mode_raw,
            opacity: (clamp(opacity, 0.0, 1.0) * 255.0).round() as u8,
            // The layer record has its own clipping byte. Resource 1026 holds
            // dragging-group IDs, which is a separate concept.
            clipping: clipping_byte,
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
                let has_real = mask.real_flags_byte.is_some()
                    || mask.real_default_color.is_some()
                    || mask.real_top.is_some()
                    || mask.real_left.is_some()
                    || mask.real_bottom.is_some()
                    || mask.real_right.is_some();
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
                // Parameters precede the optional real-mask structure.
                if has_params {
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
                if has_real {
                    writer.write_u8(mask.real_flags_byte.unwrap_or(0))?;
                    writer.write_u8(mask.real_default_color.unwrap_or(0))?;
                    writer.write_i32(mask.real_top.unwrap_or(0))?;
                    writer.write_i32(mask.real_left.unwrap_or(0))?;
                    writer.write_i32(mask.real_bottom.unwrap_or(0))?;
                    writer.write_i32(mask.real_right.unwrap_or(0))?;
                }
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

        // Write layer name (legacy Pascal string; the full Unicode name is
        // carried by the luni tagged block, so the legacy field may be safely
        // truncated when it cannot fit 255 bytes).
        let name = layer.additional_info.name.as_deref().unwrap_or("");
        writer.write_legacy_pascal_layer_name(name, 4)?;

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
    // Nested high-depth records follow the enclosing document's layout: PSB
    // uses large row counts and channel records even inside an 8B64 block.
    let options = WriteOptions {
        psb: Some(writer.large),
        ..Default::default()
    };
    let flattened = flatten_layers(Some(&layers.to_vec()));
    let prepared_payloads: Vec<PreparedLayerChannels> = flattened
        .iter()
        .map(|layer| prepare_layer_channels(layer, writer.color_mode, bits_per_channel, &options))
        .collect::<Result<Vec<PreparedLayerChannels>>>()?;

    let layer_count = layer_count_i16(flattened.len())?;
    writer.write_i16(if writer.global_alpha {
        -layer_count
    } else {
        layer_count
    })?;
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

/// Source of one composite channel plane.
///
/// `Red`/`Green`/`Blue`/`Alpha` read the named component of an RGBA preview;
/// `Gray` reads the red component, which is where the reader stores the
/// gray sample of a Grayscale document. `Zero` fills with zeros (new blank
/// documents, or a native channel that has no preview counterpart).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CompositePlane {
    Red,
    Green,
    Blue,
    Gray,
    Alpha,
    Zero,
}

/// The validated composite channel plan for a document.
///
/// This is the single source of truth for the header channel count and the
/// planes emitted in the image-data section, so a successful write always
/// agrees with its own header. Modes whose native samples cannot be derived
/// from an RGBA preview are rejected instead of emitting silently mis-encoded
/// data.
fn composite_plan(
    color_mode: ColorMode,
    global_alpha: bool,
    has_preview: bool,
) -> Result<Vec<CompositePlane>> {
    let mut plan = match color_mode {
        ColorMode::Grayscale => vec![CompositePlane::Gray],
        ColorMode::RGB => vec![
            CompositePlane::Red,
            CompositePlane::Green,
            CompositePlane::Blue,
        ],
        ColorMode::Indexed => {
            if has_preview {
                return Err(PsdError::UnsupportedFeature(
                    "cannot write Indexed composite from an RGBA preview: \
                     palette indices cannot be inferred from RGB components"
                        .to_string(),
                ));
            }
            vec![CompositePlane::Zero]
        }
        ColorMode::Bitmap => {
            return Err(PsdError::UnsupportedFeature(
                "writing Bitmap composite images is not supported".to_string(),
            ))
        }
        ColorMode::CMYK => {
            if has_preview {
                return Err(PsdError::UnsupportedFeature(
                    "writing CMYK composite from an RGBA preview requires native \
                     CMYK samples or a managed conversion; refusing to fabricate \
                     channels"
                        .to_string(),
                ));
            }
            vec![CompositePlane::Zero; 4]
        }
        other => {
            if has_preview {
                return Err(PsdError::UnsupportedFeature(format!(
                    "composite image writing for {:?} documents is not supported",
                    other
                )));
            }
            // Blank document: emit zero planes so header and payload agree.
            vec![CompositePlane::Zero; 3]
        }
    };
    if global_alpha {
        plan.push(CompositePlane::Alpha);
    }
    Ok(plan)
}

/// Write the retained native composite planes.
fn write_native_composite(
    writer: &mut PsdWriter,
    native: &crate::api::psd::CompositeNativeData,
    width: usize,
    height: usize,
    compression: Compression,
    psb: bool,
) -> Result<()> {
    let bytes_per_sample = match native.bits_per_channel {
        8 => 1usize,
        16 => 2,
        32 => 4,
        other => {
            return Err(PsdError::UnsupportedFeature(format!(
                "Unsupported composite bits per channel: {}",
                other
            )))
        }
    };
    if native.channels.is_empty() {
        return Err(PsdError::InvalidFormat(
            "Native composite has no channel planes".to_string(),
        ));
    }
    let plane_len = width
        .checked_mul(height)
        .and_then(|v| v.checked_mul(bytes_per_sample))
        .ok_or_else(|| PsdError::InvalidFormat("Native composite size overflow".to_string()))?;
    for (i, plane) in native.channels.iter().enumerate() {
        if plane.len() != plane_len {
            return Err(PsdError::InvalidFormat(format!(
                "Native composite channel {} holds {} bytes; expected {} ({}x{} at {} bits)",
                i,
                plane.len(),
                plane_len,
                width,
                height,
                native.bits_per_channel
            )));
        }
    }

    match compression {
        Compression::RawData => {
            for plane in &native.channels {
                writer.write_bytes(plane)?;
            }
        }
        Compression::RleCompressed => {
            let row_bytes = width * bytes_per_sample;
            let mut compressed_channels = Vec::with_capacity(native.channels.len());
            for plane in &native.channels {
                compressed_channels.push(compression::compress_rle_rows(plane, row_bytes, height)?);
            }
            // All row byte-counts come first, then the row data.
            for (counts, _) in &compressed_channels {
                for count in counts {
                    if psb {
                        writer.write_u32(*count)?;
                    } else {
                        writer.write_u16(u16::try_from(*count).map_err(|_| {
                            PsdError::UnsupportedFeature(
                                "RLE row count exceeds the PSD 16-bit limit".to_string(),
                            )
                        })?)?;
                    }
                }
            }
            for (_, rows) in &compressed_channels {
                writer.write_bytes(rows)?;
            }
        }
        Compression::ZipWithoutPrediction => {
            let mut planar = Vec::with_capacity(plane_len * native.channels.len());
            for plane in &native.channels {
                planar.extend_from_slice(plane);
            }
            let compressed = compression::compress_zip(&planar)?;
            writer.write_bytes(&compressed)?;
        }
        Compression::ZipWithPrediction => {
            let mut planar = Vec::with_capacity(plane_len * native.channels.len());
            for plane in &native.channels {
                planar.extend_from_slice(plane);
            }
            compression::apply_prediction_planar(
                &mut planar,
                width,
                height,
                native.channels.len(),
                native.bits_per_channel,
            )?;
            let compressed = compression::compress_zip(&planar)?;
            writer.write_bytes(&compressed)?;
        }
    }
    Ok(())
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
    let psb = options.psb.unwrap_or(false);
    let color_mode = psd.color_mode.unwrap_or(ColorMode::RGB);
    writer.write_u16(compression as u16)?;

    // An unchanged save prefers the original native planes over the quantized
    // RGBA preview: 16/32-bit samples and non-RGB channels survive verbatim.
    // Any edit to the preview invalidates them.
    if let Some(ref native) = psd.composite_native {
        let preview_unchanged = match psd.image_data.as_ref() {
            Some(image) => image.data == native.preview,
            None => native.preview.is_empty(),
        };
        if preview_unchanged
            && native.bits_per_channel as u8 == bits_per_channel
            && native.color_mode == color_mode
        {
            return write_native_composite(
                writer,
                native,
                psd.width as usize,
                psd.height as usize,
                compression,
                psb,
            );
        }
    }

    let fallback_width = psd.width as usize;
    let fallback_height = psd.height as usize;
    let preview: Option<&[u8]> = psd.image_data.as_ref().map(|image| image.data.as_slice());
    let (width, height) = match psd.image_data.as_ref() {
        Some(image_data) => {
            // The payload dimensions must agree with the header, and the RGBA
            // buffer must actually cover the declared pixel area; mismatches
            // are errors, never silent padding or truncation.
            if image_data.width != psd.width as usize || image_data.height != psd.height as usize {
                return Err(PsdError::InvalidFormat(format!(
                    "Composite pixel data is {}x{} but the document header is {}x{}",
                    image_data.width, image_data.height, psd.width, psd.height
                )));
            }
            let pixel_len = image_data
                .width
                .checked_mul(image_data.height)
                .ok_or_else(|| {
                    PsdError::InvalidFormat("Composite dimensions overflow".to_string())
                })?;
            if image_data.data.len() < pixel_len * 4 {
                return Err(PsdError::InvalidFormat(format!(
                    "Composite pixel buffer has {} bytes but {}x{} RGBA needs {}",
                    image_data.data.len(),
                    image_data.width,
                    image_data.height,
                    pixel_len * 4
                )));
            }
            (image_data.width, image_data.height)
        }
        None => (fallback_width, fallback_height),
    };

    let has_preview = psd.image_data.is_some();
    let plan = composite_plan(color_mode, global_alpha, has_preview)?;
    let plane_byte_len = width * height;
    let mut planes: Vec<Vec<u8>> = Vec::with_capacity(plan.len());
    for plane in &plan {
        planes.push(match plane {
            CompositePlane::Red => extract_channel_data_from_rgba_opt(preview, width, height, 0),
            CompositePlane::Green => extract_channel_data_from_rgba_opt(preview, width, height, 1),
            CompositePlane::Blue => extract_channel_data_from_rgba_opt(preview, width, height, 2),
            CompositePlane::Alpha => extract_channel_data_from_rgba_opt(preview, width, height, 3),
            CompositePlane::Gray => extract_channel_data_from_rgba_opt(preview, width, height, 0),
            CompositePlane::Zero => vec![0u8; plane_byte_len],
        });
    }

    let emit_plane = |writer: &mut PsdWriter, raw: &[u8]| -> Result<()> {
        writer.write_bytes(&expand_samples_for_depth(raw, bits_per_channel))
    };

    match compression {
        Compression::RawData => {
            for raw in &planes {
                emit_plane(writer, raw)?;
            }
        }
        Compression::RleCompressed => {
            let mut compressed_channels = Vec::with_capacity(planes.len());
            for raw in &planes {
                let expanded = expand_samples_for_depth(raw, bits_per_channel);
                let row_bytes = width * bytes_per_sample(bits_per_channel);
                compressed_channels.push(compression::compress_rle_rows(
                    &expanded, row_bytes, height,
                )?);
            }

            // PSD composite RLE stores all row byte-counts first, then compressed row data.
            for (counts, _) in &compressed_channels {
                for count in counts {
                    if psb {
                        writer.write_u32(*count)?;
                    } else {
                        writer.write_u16(u16::try_from(*count).map_err(|_| {
                            PsdError::UnsupportedFeature(
                                "RLE row count exceeds the PSD 16-bit limit".to_string(),
                            )
                        })?)?;
                    }
                }
            }
            for (_, rows) in &compressed_channels {
                writer.write_bytes(rows)?;
            }
        }
        Compression::ZipWithoutPrediction => {
            let mut planar = Vec::with_capacity(plane_byte_len * planes.len());
            for raw in &planes {
                planar.extend_from_slice(&expand_samples_for_depth(raw, bits_per_channel));
            }
            let compressed = compression::compress_zip(&planar)?;
            writer.write_bytes(&compressed)?;
        }
        Compression::ZipWithPrediction => {
            let mut planar = Vec::with_capacity(plane_byte_len * planes.len());
            for raw in &planes {
                planar.extend_from_slice(&expand_samples_for_depth(raw, bits_per_channel));
            }
            compression::apply_prediction_planar(
                &mut planar,
                width,
                height,
                planes.len(),
                bits_per_channel as u16,
            )?;
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
            compression::compress_rle(&expanded, row_bytes, height, options.psb.unwrap_or(false))
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

/// Whether a layer's raw channel data is current enough to reuse: the raw
/// data must exist and its read-time preview must still match the current
/// `image_data` (or both be absent).
fn layer_raw_is_current(layer: &Layer) -> bool {
    match (layer.raw_data.as_ref(), layer.image_data.as_ref()) {
        (None, _) => false,
        (Some(raw), None) => raw.preview.is_none(),
        (Some(raw), Some(image)) => raw.preview.as_ref() == Some(image),
    }
}

fn prepare_layer_channels(
    layer: &Layer,
    color_mode: ColorMode,
    bits_per_channel: u8,
    options: &WriteOptions,
) -> Result<PreparedLayerChannels> {
    let psb = options.psb.unwrap_or(false);
    if let Some(ref raw_data) = layer.raw_data {
        if raw_data.color_mode != color_mode {
            return Err(PsdError::UnsupportedFeature(format!(
                "Layer raw channel data is {raw:?} but the document is {color_mode:?}; \
                 refusing to reinterpret color channels",
                raw = raw_data.color_mode
            )));
        }
        // An edited preview invalidates the raw cache: the caller's edits win.
        if raw_data.bits_per_channel == bits_per_channel && layer_raw_is_current(layer) {
            let mut entries = Vec::with_capacity(raw_data.channels.len());
            for channel in &raw_data.channels {
                let (width, height) = layer_channel_dimensions(layer, channel.id);
                let default_len = width * height * bytes_per_sample(bits_per_channel);
                let raw = channel.data.clone().unwrap_or_else(|| vec![0; default_len]);
                let payload = match channel.compression {
                    Compression::RawData => raw,
                    Compression::RleCompressed => {
                        let row_bytes = width * bytes_per_sample(bits_per_channel);
                        // Row-count width must match the destination version;
                        // the source version (raw_data.large) only applies when
                        // decoding the preserved source bytes.
                        compression::compress_rle(&raw, row_bytes, height, psb)?
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

    let mut channel_ids = match color_mode {
        ColorMode::Grayscale | ColorMode::Bitmap | ColorMode::Indexed => {
            vec![ChannelID::Transparency, ChannelID::Color0]
        }
        ColorMode::CMYK => vec![
            ChannelID::Transparency,
            ChannelID::Color0,
            ChannelID::Color1,
            ChannelID::Color2,
            ChannelID::Color3,
        ],
        _ => vec![
            ChannelID::Transparency,
            ChannelID::Color0,
            ChannelID::Color1,
            ChannelID::Color2,
        ],
    };
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
                return (left, top, extent(left, right), extent(top, bottom));
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
                    return (left, top, extent(left, right), extent(top, bottom));
                }
            }
            if let Some(mask) = layer.additional_info.mask.as_ref() {
                let left = mask.real_left.or(mask.left).unwrap_or(layer_left);
                let top = mask.real_top.or(mask.top).unwrap_or(layer_top);
                let right = mask.real_right.or(mask.right).unwrap_or(left);
                let bottom = mask.real_bottom.or(mask.bottom).unwrap_or(top);
                return (left, top, extent(left, right), extent(top, bottom));
            }
        }
        _ => {}
    }

    (
        layer_left,
        layer_top,
        extent(layer_left, layer_right),
        extent(layer_top, layer_bottom),
    )
}

fn extent(start: i32, end: i32) -> usize {
    end.checked_sub(start)
        .filter(|value| *value >= 0)
        .unwrap_or(0) as usize
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

fn extract_channel_data_from_rgba_opt(
    image_data: Option<&[u8]>,
    width: usize,
    height: usize,
    offset: usize,
) -> Vec<u8> {
    let mut out = vec![0u8; width * height];
    if let Some(image_data) = image_data {
        for i in 0..(width * height) {
            let src = i * 4 + offset;
            if src < image_data.len() {
                out[i] = image_data[src];
            } else if offset == 3 {
                out[i] = 255;
            }
        }
    } else if offset == 3 {
        out.fill(255);
    }
    out
}

fn serialize_layer_blending_ranges(ranges: &crate::api::layer::LayerBlendingRangesData) -> Vec<u8> {
    let mut out = Vec::new();
    let mut write_pair = |pair: &crate::api::layer::LayerBlendingRangePair| {
        out.extend_from_slice(&pair.src_black.to_be_bytes());
        out.extend_from_slice(&pair.src_white.to_be_bytes());
        out.extend_from_slice(&pair.dst_black.to_be_bytes());
        out.extend_from_slice(&pair.dst_white.to_be_bytes());
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
                ChannelID::Transparency
                    | ChannelID::Color0
                    | ChannelID::Color1
                    | ChannelID::Color2
                    | ChannelID::Color3
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
                ChannelID::Transparency
                    | ChannelID::Color0
                    | ChannelID::Color1
                    | ChannelID::Color2
                    | ChannelID::Color3
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

/// Clear the cached TySh raw bytes of every text layer (recursively) whose
/// typed `text` diverges from the displayed text stored in its wire
/// descriptor (`Txt ` item). Returns whether any layer was edited.
fn invalidate_edited_text_raws(psd: &mut Psd) -> bool {
    fn clear_layer(layer: &mut crate::api::layer::Layer) -> bool {
        let mut edited = false;
        if let Some(ref mut text) = layer.additional_info.text {
            if let Some(raw) = text.raw_bytes.as_ref() {
                if !text_raw_matches(text, raw) {
                    text.raw_bytes = None;
                    edited = true;
                }
            }
        }
        if let Some(children) = layer.children.as_mut() {
            edited |= children.iter_mut().any(clear_layer);
        }
        edited
    }

    psd.children
        .as_mut()
        .map(|children| children.iter_mut().any(clear_layer))
        .unwrap_or(false)
}

fn text_raw_matches(text: &crate::format::additional_info::TextLayerData, raw: &[u8]) -> bool {
    let mut reader =
        crate::io::reader::PsdReader::new(Cursor::new(raw.to_vec()), Default::default());
    let mut parsed = crate::format::additional_info::LayerAdditionalInfo::default();
    if reader.read_text_layer(&mut parsed, raw.len()).is_err() {
        return false;
    }
    let Some(parsed) = parsed.text else {
        return false;
    };
    text.transform == parsed.transform
        && text.text == parsed.text
        && text.text_version == parsed.text_version
        && text.descriptor_version == parsed.descriptor_version
        && text.text_data == parsed.text_data
        && text.warp_version == parsed.warp_version
        && text.warp_descriptor_version == parsed.warp_descriptor_version
        && text.warp_data == parsed.warp_data
        && text.left == parsed.left
        && text.top == parsed.top
        && text.right == parsed.right
        && text.bottom == parsed.bottom
}

/// Apply text prewrite: synthesize Txt2 engine data from TySh layer text data
fn apply_text_prewrite(psd: &mut Psd) -> Result<()> {
    use crate::support::engine_data::EngineValue;
    use std::collections::HashMap;

    // If a document engine exists, keep it (and its raw bytes) verbatim -
    // but only while no typed text edit diverges from the stored wire text.
    // A changed text forces reconciliation: the affected layer's TySh cache
    // is dropped (so the synced descriptor is written) and the stale
    // document engine is regenerated from the current layer tree.
    let edited_any = invalidate_edited_text_raws(psd);
    if psd.additional_info.text_engine.is_some() {
        if !edited_any {
            return Ok(());
        }
        psd.additional_info.text_engine = None;
    }

    let mut text_objects = Vec::new();
    let mut document_resources: Option<EngineValue> = None;

    fn collect_text(
        layers: &mut [crate::api::layer::Layer],
        text_objects: &mut Vec<EngineValue>,
        document_resources: &mut Option<EngineValue>,
    ) {
        for layer in layers.iter_mut() {
            if let Some(ref mut text) = layer.additional_info.text {
                let mut style_run_array = Vec::new();
                let mut paragraph_run_array = Vec::new();

                if let Some(ref text_desc) = text.text_data {
                    if let Some(crate::support::descriptor::DescriptorValue::DataBytes(
                        engine_bytes,
                    )) = text_desc.items.get("EngineData")
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
                                    *document_resources = Some(value);
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
            if let Some(children) = layer.children.as_mut() {
                collect_text(children, text_objects, document_resources);
            }
        }
    }

    if let Some(ref mut layers) = psd.children {
        collect_text(layers, &mut text_objects, &mut document_resources);
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
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/samples/3d-preview-mockup.psd")
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
    fn zero_capacity_writer_does_not_hang() {
        let mut writer = PsdWriter::new(0);
        writer.write_u8(42).unwrap();
        assert_eq!(writer.get_buffer(), &[42]);
    }

    #[test]
    fn layer_count_over_i16_max_errors() {
        assert!(layer_count_i16(40_000).is_err());
        assert_eq!(layer_count_i16(3).unwrap(), 3);
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

        assert_eq!(u16::from_be_bytes([bytes[12], bytes[13]]), 4);

        let reparsed = crate::read_psd(
            Cursor::new(bytes),
            ReadOptions {
                skip_composite_image_data: Some(false),
                ..Default::default()
            },
        )
        .expect("reparse written psd");
        assert_eq!(reparsed.channels, Some(4));
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
            ColorMode::RGB,
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
        let layer = find_layer_by_name_with_user_mask(psd.children.as_deref().unwrap_or(&[]), "4")
            .expect("sample layer with user mask");

        let prepared = prepare_layer_channels(
            layer,
            psd.color_mode.unwrap_or(ColorMode::RGB),
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

    #[test]
    fn editing_nested_text_in_loaded_document_reconciles_with_engine() {
        let original =
            fs::read("tests/fixtures/samples/multi-value-items.psd").expect("read text fixture");
        let mut psd = crate::read_psd(Cursor::new(original), ReadOptions::default())
            .expect("parse text fixture");

        fn count_texts(layers: &[Layer]) -> usize {
            layers.iter().fold(0, |acc, layer| {
                acc + usize::from(layer.additional_info.text.is_some())
                    + layer
                        .children
                        .as_deref()
                        .map(|children| count_texts(children))
                        .unwrap_or(0)
            })
        }

        fn find_deep_text_mut<'a>(
            layers: &'a mut [Layer],
            depth: usize,
            name: &str,
        ) -> Option<&'a mut Layer> {
            for layer in layers.iter_mut() {
                if depth > 0
                    && layer.additional_info.name.as_deref() == Some(name)
                    && layer.additional_info.text.is_some()
                {
                    return Some(layer);
                }
                if let Some(children) = layer.children.as_mut() {
                    if let Some(found) = find_deep_text_mut(children, depth + 1, name) {
                        return Some(found);
                    }
                }
            }
            None
        }

        fn find_text(layers: &[Layer], content: &str) -> bool {
            layers.iter().any(|layer| {
                (layer.additional_info.text.as_ref().map(|t| t.text.as_str()) == Some(content))
                    || layer
                        .children
                        .as_deref()
                        .map(|children| find_text(children, content))
                        .unwrap_or(false)
            })
        }

        let text_count_before = count_texts(psd.children.as_ref().expect("children"));
        assert!(
            text_count_before >= 4,
            "fixture should carry several text layers"
        );

        // Edit a text layer nested inside a group. No manual invalidation is
        // required: the prewrite detects the divergence and reconciles.
        let nested = find_deep_text_mut(
            psd.children.as_mut().expect("children"),
            0,
            "The third text item",
        )
        .expect("nested text layer");
        nested.additional_info.text.as_mut().unwrap().text = "edited-third-item".to_string();

        let bytes =
            crate::write_psd(&psd, &crate::api::psd::WriteOptions::default()).expect("write");
        let phrase_utf16be: Vec<u8> = "edited-third-item"
            .encode_utf16()
            .flat_map(|unit| unit.to_be_bytes())
            .collect();
        assert!(
            bytes
                .windows(phrase_utf16be.len())
                .any(|w| w == phrase_utf16be.as_slice()),
            "edited text missing from raw wire output"
        );
        let reparsed = crate::read_psd(Cursor::new(bytes), ReadOptions::default())
            .expect("reparse edited document");
        let children = reparsed.children.as_ref().expect("children");

        // The nested edit survived and is visible at the same nesting level.
        assert!(
            find_text(children, "edited-third-item"),
            "nested edit must persist"
        );
        assert_eq!(
            count_texts(children),
            text_count_before,
            "text object count unchanged"
        );

        // Every unedited text layer kept its exact original content.
        fn text_by_name<'a>(
            layers: &'a [Layer],
            out: &mut std::collections::HashMap<String, String>,
        ) {
            for layer in layers {
                if let (Some(name), Some(text)) = (
                    layer.additional_info.name.as_ref(),
                    layer.additional_info.text.as_ref(),
                ) {
                    out.entry(name.clone()).or_insert_with(|| text.text.clone());
                }
                if let Some(children) = layer.children.as_ref() {
                    text_by_name(children, out);
                }
            }
        }
        let mut before: std::collections::HashMap<String, String> =
            std::collections::HashMap::new();
        text_by_name(psd.children.as_ref().expect("children"), &mut before);
        let mut after: std::collections::HashMap<String, String> = std::collections::HashMap::new();
        text_by_name(children, &mut after);
        for (name, original_text) in &before {
            if name == "The third text item" {
                continue;
            }
            assert_eq!(
                after.get(name),
                Some(original_text),
                "unedited text layer {:?} changed",
                name
            );
        }
        assert_eq!(
            after.get("The third text item"),
            Some(&"edited-third-item".to_string())
        );

        // The document engine was regenerated instead of silently dropped.
        let engine = reparsed
            .additional_info
            .text_engine
            .as_ref()
            .expect("document engine regenerated");
        assert!(engine.raw.is_some(), "engine serialized");
    }
}

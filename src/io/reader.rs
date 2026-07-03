//! PSD file reader implementation
//!
//! Provides functionality to read PSD files and parse their structure.

use crate::support::binrw_support::{
    decode_be, ChannelInfoRecord, GlobalLayerMaskRecord, LayerBlendRecord, LayerMaskPrefixRecord,
    LayerRecordBounds, PsbChannelInfoRecord, PsdHeaderRecord,
};
use crate::support::compression;
use crate::support::error::{PsdError, Result};
use crate::support::helpers::{
    setup_grayscale, to_blend_mode, LayerBlendFlags, LayerMaskParameterFlags, LayerMaskStateBits,
};
use crate::api::layer::{Layer, LayerMaskData, LayerRawData, LayerRawDataChannel};
use crate::api::psd::{GlobalLayerMaskInfo, Psd, ReadOptions};
use crate::api::types::{ChannelID, ColorMode, Compression, PixelData, SectionDividerType};
use byteorder::{BigEndian, ReadBytesExt};
use std::io::{Read, Seek, SeekFrom};

/// PSD reader for binary data
pub struct PsdReader<R: Read + Seek> {
    reader: R,
    pub offset: u64,
    pub large: bool,
    pub global_alpha: bool,
    pub options: ReadOptions,
}

impl<R: Read + Seek> PsdReader<R> {
    /// Create a new PSD reader
    pub fn new(reader: R, options: ReadOptions) -> Self {
        Self {
            reader,
            offset: 0,
            large: false,
            global_alpha: false,
            options,
        }
    }

    /// Read an unsigned 8-bit integer
    pub fn read_u8(&mut self) -> Result<u8> {
        let val = self.reader.read_u8()?;
        self.offset += 1;
        Ok(val)
    }

    /// Peek at an unsigned 8-bit integer without advancing
    pub fn peek_u8(&mut self) -> Result<u8> {
        let pos = self.reader.stream_position()?;
        let val = self.reader.read_u8()?;
        self.reader.seek(SeekFrom::Start(pos))?;
        Ok(val)
    }

    /// Peek a 4-character signature without advancing.
    pub fn peek_signature(&mut self) -> Result<String> {
        let pos = self.reader.stream_position()?;
        let bytes = self.read_bytes(4)?;
        self.reader.seek(SeekFrom::Start(pos))?;
        self.offset = pos;
        Ok(String::from_utf8_lossy(&bytes).to_string())
    }

    /// Read a signed 16-bit integer (big-endian)
    pub fn read_i16(&mut self) -> Result<i16> {
        let val = self.reader.read_i16::<BigEndian>()?;
        self.offset += 2;
        Ok(val)
    }

    /// Read an unsigned 16-bit integer (big-endian)
    pub fn read_u16(&mut self) -> Result<u16> {
        let val = self.reader.read_u16::<BigEndian>()?;
        self.offset += 2;
        Ok(val)
    }

    /// Read a signed 32-bit integer (big-endian)
    pub fn read_i32(&mut self) -> Result<i32> {
        let val = self.reader.read_i32::<BigEndian>()?;
        self.offset += 4;
        Ok(val)
    }

    /// Read an unsigned 32-bit integer (big-endian)
    pub fn read_u32(&mut self) -> Result<u32> {
        let val = self.reader.read_u32::<BigEndian>()?;
        self.offset += 4;
        Ok(val)
    }

    /// Read a 32-bit float (big-endian)
    pub fn read_f32(&mut self) -> Result<f32> {
        let val = self.reader.read_f32::<BigEndian>()?;
        self.offset += 4;
        Ok(val)
    }

    /// Read a 64-bit float (big-endian)
    pub fn read_f64(&mut self) -> Result<f64> {
        let val = self.reader.read_f64::<BigEndian>()?;
        self.offset += 8;
        Ok(val)
    }

    /// Read raw bytes
    pub fn read_bytes(&mut self, length: usize) -> Result<Vec<u8>> {
        let mut buffer = vec![0u8; length];
        self.reader.read_exact(&mut buffer)?;
        self.offset += length as u64;
        Ok(buffer)
    }

    /// Skip bytes
    pub fn skip_bytes(&mut self, count: usize) -> Result<()> {
        self.reader.seek(SeekFrom::Current(count as i64))?;
        self.offset += count as u64;
        Ok(())
    }

    pub(crate) fn seek_to(&mut self, position: u64) -> Result<()> {
        self.reader.seek(SeekFrom::Start(position))?;
        self.offset = position;
        Ok(())
    }

    /// Read all remaining bytes to EOF from current offset.
    pub fn read_remaining_bytes(&mut self) -> Result<Vec<u8>> {
        let cur = self.reader.stream_position()?;
        let end = self.reader.seek(SeekFrom::End(0))?;
        self.reader.seek(SeekFrom::Start(cur))?;
        let remaining = (end - cur) as usize;
        self.read_bytes(remaining)
    }

    /// Read a 4-character signature
    pub fn read_signature(&mut self) -> Result<String> {
        let bytes = self.read_bytes(4)?;
        Ok(String::from_utf8_lossy(&bytes).to_string())
    }

    /// Check signature matches expected value
    pub fn check_signature(&mut self, expected: &str) -> Result<()> {
        let sig = self.read_signature()?;
        if sig != expected {
            return Err(PsdError::InvalidFormat(format!(
                "Invalid signature: expected '{}', got '{}'",
                expected, sig
            )));
        }
        Ok(())
    }

    /// Read a Pascal string (length-prefixed, padded)
    pub fn read_pascal_string(&mut self, pad_to: usize) -> Result<String> {
        let mut length = self.read_u8()? as usize;
        let text = if length > 0 {
            let bytes = self.read_bytes(length)?;
            String::from_utf8_lossy(&bytes).to_string()
        } else {
            String::new()
        };

        length += 1; // Include the length byte
        while length % pad_to != 0 {
            self.skip_bytes(1)?;
            length += 1;
        }

        Ok(text)
    }

    /// Read a Unicode string (UTF-16 BE)
    pub fn read_unicode_string(&mut self) -> Result<String> {
        let length = self.read_u32()? as usize;
        self.read_unicode_string_with_length(length)
    }

    /// Read a Unicode string with known length
    pub fn read_unicode_string_with_length(&mut self, length: usize) -> Result<String> {
        let mut chars = Vec::with_capacity(length);

        for _ in 0..length {
            let value = self.read_u16()?;
            // Skip null bytes (padding/termination)
            if value != 0 {
                if let Some(c) = char::from_u32(value as u32) {
                    chars.push(c);
                }
            }
        }

        Ok(chars.into_iter().collect())
    }

    /// Read an ASCII string
    pub fn read_ascii_string(&mut self, length: usize) -> Result<String> {
        let bytes = self.read_bytes(length)?;
        Ok(String::from_utf8_lossy(&bytes).to_string())
    }

    /// Read a section with length prefix
    pub fn read_section<F, T>(&mut self, round: usize, func: F) -> Result<T>
    where
        F: FnOnce(&mut Self, u64) -> Result<T>,
    {
        let length = if self.large {
            let high = self.read_u32()? as usize;
            if high != 0 {
                return Err(PsdError::UnsupportedFeature(
                    "Sizes larger than 4GB are not supported".to_string(),
                ));
            }
            self.read_u32()? as usize
        } else {
            self.read_u32()? as usize
        };

        let start_offset = self.offset;
        let end_offset = start_offset + length as u64;

        let result = func(self, end_offset)?;

        // Skip to end of section
        if self.offset < end_offset {
            let remaining = (end_offset - self.offset) as usize;
            self.skip_bytes(remaining)?;
        }

        // Section payload is padded to alignment outside the length field.
        if round > 1 {
            let mut padded_length = length;
            while padded_length % round != 0 {
                self.skip_bytes(1)?;
                padded_length += 1;
            }
        }

        Ok(result)
    }

    /// Get bytes left in current section
    pub fn bytes_left(&self, end_offset: u64) -> usize {
        if self.offset >= end_offset {
            0
        } else {
            (end_offset - self.offset) as usize
        }
    }

    /// Read a fixed-point number (16.16)
    pub fn read_fixed_point_32(&mut self) -> Result<f64> {
        let val = self.read_i32()?;
        Ok(val as f64 / 65536.0)
    }

    /// Read a fixed-point path number (8.24)
    pub fn read_fixed_point_path_32(&mut self) -> Result<f64> {
        let val = self.read_i32()?;
        Ok(val as f64 / 16777216.0)
    }

    /// Read a color value
    pub fn read_color(&mut self) -> Result<crate::api::types::Color> {
        use crate::api::types::{Color, Grayscale, CMYK};
        let color_space = self.read_u16()?;
        let c1 = self.read_u16()?;
        let c2 = self.read_u16()?;
        let c3 = self.read_u16()?;
        let c4 = self.read_u16()?;

        match color_space {
            0 => Ok(Color::Rgb48 {
                red: c1,
                green: c2,
                blue: c3,
            }),
            1 => Ok(Color::Hsb {
                hue: c1,
                saturation: c2,
                brightness: c3,
            }),
            2 => Ok(Color::CMYK(CMYK {
                c: c1,
                m: c2,
                y: c3,
                k: c4,
            })),
            7 => Ok(Color::Lab {
                lightness: c1,
                a: i16::from_be_bytes(c2.to_be_bytes()),
                b: i16::from_be_bytes(c3.to_be_bytes()),
            }),
            8 => Ok(Color::Grayscale(Grayscale { k: c1 })),
            _ => Ok(Color::OpaqueColorSpace {
                color_space,
                components: [c1, c2, c3, c4],
            }),
        }
    }
}

/// Read a PSD file from a reader
pub fn read_psd<R: Read + Seek>(mut reader: R, options: ReadOptions) -> Result<Psd> {
    let mut psd_reader = PsdReader::new(&mut reader, options);

    let header: PsdHeaderRecord = decode_be(&psd_reader.read_bytes(26)?, "PSD header")?;
    if &header.signature != b"8BPS" {
        return Err(PsdError::InvalidFormat(format!(
            "Invalid signature: expected '8BPS', got '{}'",
            String::from_utf8_lossy(&header.signature),
        )));
    }

    let version = header.version;
    if version != 1 && version != 2 {
        return Err(PsdError::InvalidFormat(format!(
            "Invalid PSD file version: {}",
            version
        )));
    }

    if header.reserved.iter().any(|byte| *byte != 0) {
        return Err(PsdError::InvalidFormat(
            "Header reserved bytes must be zero".to_string(),
        ));
    }

    psd_reader.large = version == 2;

    let channels = header.channels;
    let height = header.height;
    let width = header.width;
    let bits_per_channel = header.depth;
    let color_mode = header.color_mode;

    // Validate dimensions
    let max_size = if version == 1 { 30000 } else { 300000 };
    if width > max_size || height > max_size {
        return Err(PsdError::InvalidFormat(format!(
            "Invalid size: {}x{}",
            width, height
        )));
    }

    if width == 0 || height == 0 {
        return Err(PsdError::InvalidFormat(format!(
            "Invalid size: {}x{}",
            width, height
        )));
    }

    if channels == 0 || channels > 56 {
        return Err(PsdError::InvalidFormat(format!(
            "Invalid channel count: {}",
            channels
        )));
    }

    if ![1, 8, 16, 32].contains(&bits_per_channel) {
        return Err(PsdError::InvalidFormat(format!(
            "Invalid bits per channel: {}",
            bits_per_channel
        )));
    }

    let color_mode = ColorMode::from_u16(color_mode)?;

    let mut psd = Psd {
        width,
        height,
        channels: Some(channels),
        bits_per_channel: Some(bits_per_channel as u8),
        color_mode: Some(color_mode),
        palette: None,
        image_data: None,
        children: None,
        image_resources: None,
        linked_files: None,
        artboards: None,
        global_layer_mask_info: None,
        annotations: None,
        additional_info: Default::default(),
        color_mode_data: None,
        resolution: None,
        guides: None,
        alpha_channel_names: None,
        selected_layer_ids: None,
        icc_profile: None,
        path_selection_descriptor: None,
        slices: None,
        variable_sets: None,
        data_sets: None,
        descriptor_1065: None,
        descriptor_1074: None,
        descriptor_1075: None,
        layer_group_ids: None,
        color_samplers: None,
        display_info: None,
        clipping_path_name: None,
    };

    // Read color mode data section
    read_color_mode_data(&mut psd_reader, &mut psd)?;

    // Read image resources section
    read_image_resources(&mut psd_reader, &mut psd)?;

    // Read layer and mask information section
    read_layer_and_mask_info(&mut psd_reader, &mut psd)?;

    // Apply document resource postprocess (after layers are available)
    crate::format::document_resource_postprocess::apply_document_postprocess(&mut psd)?;

    // Read image data section
    if !psd_reader
        .options
        .skip_composite_image_data
        .unwrap_or(false)
    {
        read_image_data(&mut psd_reader, &mut psd)?;
    }

    Ok(psd)
}

/// Read color mode data section
fn read_color_mode_data<R: Read + Seek>(reader: &mut PsdReader<R>, psd: &mut Psd) -> Result<()> {
    reader.read_section(1, |reader, end_offset| {
        if reader.bytes_left(end_offset) == 0 {
            return Ok(());
        }

        if psd.color_mode == Some(ColorMode::Indexed) {
            if reader.bytes_left(end_offset) != 768 {
                return Err(PsdError::InvalidFormat(
                    "Invalid color palette size".to_string(),
                ));
            }

            let mut palette = Vec::with_capacity(256);

            // Read red values
            for _ in 0..256 {
                let r = reader.read_u8()?;
                palette.push(crate::api::types::RGB { r, g: 0, b: 0 });
            }

            // Read green values
            for i in 0..256 {
                palette[i].g = reader.read_u8()?;
            }

            // Read blue values
            for i in 0..256 {
                palette[i].b = reader.read_u8()?;
            }

            psd.palette = Some(palette);
            psd.color_mode_data = Some(crate::api::psd::ColorModeSectionData { bytes: Vec::new() });
        } else {
            // Preserve generic color mode data
            let remaining = reader.bytes_left(end_offset);
            psd.color_mode_data = Some(crate::api::psd::ColorModeSectionData {
                bytes: reader.read_bytes(remaining as usize)?,
            });
        }

        Ok(())
    })
}

/// Read image resources section
fn read_image_resources<R: Read + Seek>(reader: &mut PsdReader<R>, psd: &mut Psd) -> Result<()> {
    reader.read_section(1, |reader, end_offset| {
        let remaining = reader.bytes_left(end_offset) as usize;
        if remaining > 0 {
            let resources = crate::format::image_resources::read_image_resources(reader, remaining)?;
            // Map descriptor resource 3000 to psd.path_selection_descriptor
            if let Some(descriptor) = resources.descriptor_resources.get(&3000) {
                psd.path_selection_descriptor = Some(descriptor.clone());
            }
            psd.image_resources = Some(resources);
        }
        Ok(())
    })
}

/// Read layer and mask information section
fn read_layer_and_mask_info<R: Read + Seek>(
    reader: &mut PsdReader<R>,
    psd: &mut Psd,
) -> Result<()> {
    reader.read_section(1, |reader, end_offset| {
        // Read layer info
        if reader.bytes_left(end_offset) > 0 {
            reader.read_section(2, |reader, end_offset| {
                read_layer_info(reader, psd)?;
                reader.skip_bytes(reader.bytes_left(end_offset))?;
                Ok(())
            })?;
        }

        // Read global layer mask info
        if reader.bytes_left(end_offset) > 0 {
            let global_mask = read_global_layer_mask_info(reader)?;
            if let Some(mask) = global_mask {
                psd.global_layer_mask_info = Some(mask);
            }
        }

        if reader.bytes_left(end_offset) > 0 {
            psd.additional_info = crate::format::additional_info::read_layer_additional_info(
                reader,
                reader.bytes_left(end_offset),
            )?;
        }

        Ok(())
    })
}

/// Read layer info
fn read_layer_info<R: Read + Seek>(reader: &mut PsdReader<R>, psd: &mut Psd) -> Result<()> {
    let mut layer_count = reader.read_i16()? as i32;

    if layer_count < 0 {
        reader.global_alpha = true;
        layer_count = -layer_count;
    }

    let mut layers = Vec::new();
    let mut layer_channels = Vec::new();

    // Read layer records
    for _ in 0..layer_count {
        let (layer, channels) = read_layer_record(reader)?;
        layers.push(layer);
        layer_channels.push(channels);
    }

    // Read layer channel image data
    for (i, channels) in layer_channels.iter().enumerate() {
        read_layer_channel_image_data(reader, psd, &mut layers[i], channels)?;
    }

    // Build layer hierarchy
    build_layer_hierarchy(psd, layers)?;

    Ok(())
}

/// Read a single layer record
fn read_layer_record<R: Read + Seek>(
    reader: &mut PsdReader<R>,
) -> Result<(Layer, Vec<ChannelInfo>)> {
    let mut layer = Layer::default();

    let bounds: LayerRecordBounds = decode_be(&reader.read_bytes(18)?, "layer record bounds")?;
    layer.top = Some(bounds.top);
    layer.left = Some(bounds.left);
    layer.bottom = Some(bounds.bottom);
    layer.right = Some(bounds.right);

    let channel_count = bounds.channel_count as usize;
    let mut channels = Vec::with_capacity(channel_count);

    for _ in 0..channel_count {
        if reader.large {
            let record: PsbChannelInfoRecord =
                decode_be(&reader.read_bytes(10)?, "PSB channel info")?;
            if record.high_length != 0 {
                return Err(PsdError::UnsupportedFeature(
                    "Sizes larger than 4GB are not supported".to_string(),
                ));
            }
            channels.push(ChannelInfo {
                id: ChannelID::from_i16(record.id),
                length: record.low_length as u64,
            });
        } else {
            let record: ChannelInfoRecord = decode_be(&reader.read_bytes(6)?, "channel info")?;
            channels.push(ChannelInfo {
                id: ChannelID::from_i16(record.id),
                length: record.length as u64,
            });
        }
    }

    let blend: LayerBlendRecord = decode_be(&reader.read_bytes(12)?, "layer blend record")?;
    if &blend.signature != b"8BIM" {
        return Err(PsdError::InvalidFormat(format!(
            "Invalid signature: expected '8BIM', got '{}'",
            String::from_utf8_lossy(&blend.signature),
        )));
    }
    let blend_sig = String::from_utf8_lossy(&blend.blend_mode).to_string();
    layer.blend_mode = Some(to_blend_mode(&blend_sig)?);
    layer.opacity = Some(blend.opacity as f64 / 255.0);
    let blend_flags = LayerBlendFlags::from_bits_retain(blend.flags);
    layer.transparency_protected =
        Some(blend_flags.contains(LayerBlendFlags::TRANSPARENCY_PROTECTED));
    layer.hidden = Some(blend_flags.contains(LayerBlendFlags::HIDDEN));

    // Read extra data
    reader.read_section(1, |reader, end_offset| {
        // Read layer mask data
        let channel_ids: Vec<i16> = channels.iter().map(|c| c.id as i16).collect();
        read_layer_mask_data(reader, &mut layer, &channel_ids)?;

        // Read blending ranges
        let blending_len = reader.read_u32()? as usize;
        if blending_len > 0 && reader.bytes_left(end_offset) >= blending_len {
            let bytes = reader.read_bytes(blending_len)?;
            layer.blending_ranges_data = parse_layer_blending_ranges(&bytes);
        }

        // Read layer name
        let pascal_name = reader.read_pascal_string(4)?;

        // Read tagged blocks (additional layer info)
        let remaining = reader.bytes_left(end_offset) as usize;
        if remaining > 0 {
            let existing_mask = layer.additional_info.mask.take();
            let existing_real_mask = layer.additional_info.real_mask.take();
            let mut info = crate::format::additional_info::read_layer_additional_info(reader, remaining)?;
            if info.mask.is_none() {
                info.mask = existing_mask;
            }
            if info.real_mask.is_none() {
                info.real_mask = existing_real_mask;
            }
            layer.additional_info = info;
        }
        if layer.additional_info.name.is_none() {
            layer.additional_info.name = Some(pascal_name);
        }

        Ok(())
    })?;

    Ok((layer, channels))
}

/// Channel information
#[derive(Debug, Clone)]
struct ChannelInfo {
    id: ChannelID,
    length: u64,
}

pub(crate) fn read_nested_layer_info_block(
    bytes: &[u8],
    bits_per_channel: u8,
) -> Result<Vec<Layer>> {
    let cursor = std::io::Cursor::new(bytes.to_vec());
    let mut reader = PsdReader::new(cursor, Default::default());
    let mut layer_count = reader.read_i16()? as i32;
    if layer_count < 0 {
        layer_count = -layer_count;
    }

    let mut layers = Vec::new();
    let mut layer_channels = Vec::new();
    for _ in 0..layer_count {
        let (layer, channels) = read_layer_record(&mut reader)?;
        layers.push(layer);
        layer_channels.push(channels);
    }
    for (i, channels) in layer_channels.iter().enumerate() {
        read_layer_channel_raw_data(&mut reader, bits_per_channel, &mut layers[i], channels)?;
    }
    let mut temp_psd = Psd::default();
    build_layer_hierarchy(&mut temp_psd, layers)?;
    Ok(temp_psd.children.unwrap_or_default())
}

/// Read layer mask data
fn read_layer_mask_data<R: Read + Seek>(
    reader: &mut PsdReader<R>,
    layer: &mut Layer,
    channel_ids: &[i16],
) -> Result<()> {
    reader.read_section(1, |reader, end_offset| {
        if reader.bytes_left(end_offset) == 0 {
            return Ok(());
        }

        let prefix: LayerMaskPrefixRecord =
            decode_be(&reader.read_bytes(18)?, "layer mask prefix")?;
        let flags = LayerMaskStateBits::from_bits_retain(prefix.flags);

        let mut mask = LayerMaskData {
            top: Some(prefix.top),
            left: Some(prefix.left),
            bottom: Some(prefix.bottom),
            right: Some(prefix.right),
            default_color: Some(prefix.default_color),
            disabled: Some(flags.contains(LayerMaskStateBits::DISABLED)),
            position_relative_to_layer: Some(
                flags.contains(LayerMaskStateBits::POSITION_RELATIVE_TO_LAYER),
            ),
            from_vector_data: Some(flags.contains(LayerMaskStateBits::FROM_VECTOR_DATA)),
            ..Default::default()
        };

        // Read remaining mask parameters if present
        let remaining = reader.bytes_left(end_offset) as usize;
        if remaining >= 18 {
            // Check for mask parameters flag (bit 4 in flags byte)
            if flags.contains(LayerMaskStateBits::HAS_PARAMETERS) {
                // Real mask fields are only present when channel -3 (RealUserMask) exists
                let has_real_mask_channel = channel_ids.contains(&-3);
                if has_real_mask_channel && reader.bytes_left(end_offset) >= 18 {
                    mask.real_flags_byte = Some(reader.read_u8()?);
                    mask.real_default_color = Some(reader.read_u8()?);
                    mask.real_top = Some(reader.read_i32()?);
                    mask.real_left = Some(reader.read_i32()?);
                    mask.real_bottom = Some(reader.read_i32()?);
                    mask.real_right = Some(reader.read_i32()?);
                }
                let param_flags = LayerMaskParameterFlags::from_bits_retain(reader.read_u8()?);
                if param_flags.contains(LayerMaskParameterFlags::USER_MASK_DENSITY)
                    && reader.bytes_left(end_offset) > 0
                {
                    mask.user_mask_density = Some(reader.read_u8()? as f64);
                }
                if param_flags.contains(LayerMaskParameterFlags::USER_MASK_FEATHER)
                    && reader.bytes_left(end_offset) >= 8
                {
                    mask.user_mask_feather = Some(reader.read_f64()?);
                }
                if param_flags.contains(LayerMaskParameterFlags::VECTOR_MASK_DENSITY)
                    && reader.bytes_left(end_offset) > 0
                {
                    mask.vector_mask_density = Some(reader.read_u8()? as f64);
                }
                if param_flags.contains(LayerMaskParameterFlags::VECTOR_MASK_FEATHER)
                    && reader.bytes_left(end_offset) >= 8
                {
                    mask.vector_mask_feather = Some(reader.read_f64()?);
                }
            }
        }

        // For old format (pre-HAS_PARAMETERS), check if remaining bytes are the real mask
        let remaining_after_params = reader.bytes_left(end_offset) as usize;
        if !flags.contains(LayerMaskStateBits::HAS_PARAMETERS)
            && remaining_after_params >= 18
            && !channel_ids.contains(&-3)
        {
            // Old format: extra bytes are the real mask (only if no -3 channel to avoid over-read)
            mask.real_flags_byte = Some(reader.read_u8()?);
            mask.real_default_color = Some(reader.read_u8()?);
            mask.real_top = Some(reader.read_i32()?);
            mask.real_left = Some(reader.read_i32()?);
            mask.real_bottom = Some(reader.read_i32()?);
            mask.real_right = Some(reader.read_i32()?);
        }

        // Skip any remaining mask data
        reader.skip_bytes(reader.bytes_left(end_offset))?;

        layer.additional_info.mask = Some(mask);
        Ok(())
    })
}

/// Read layer channel image data
fn read_layer_channel_image_data<R: Read + Seek>(
    reader: &mut PsdReader<R>,
    psd: &Psd,
    layer: &mut Layer,
    channels: &[ChannelInfo],
) -> Result<()> {
    if reader.options.skip_layer_image_data.unwrap_or(false) {
        for channel in channels {
            reader.skip_bytes(channel.length as usize)?;
        }
        return Ok(());
    }

    let width = (layer.right.unwrap_or(0) - layer.left.unwrap_or(0)).max(0) as usize;
    let height = (layer.bottom.unwrap_or(0) - layer.top.unwrap_or(0)).max(0) as usize;
    if width == 0 || height == 0 {
        for channel in channels {
            reader.skip_bytes(channel.length as usize)?;
        }
        return Ok(());
    }

    let expected_len = width * height;
    let color_mode = psd.color_mode.unwrap_or(ColorMode::RGB);
    let cmyk = color_mode == ColorMode::CMYK;
    let is_grayscale = color_mode == ColorMode::Grayscale;
    let mut red: Option<Vec<u8>> = None;
    let mut green: Option<Vec<u8>> = None;
    let mut blue: Option<Vec<u8>> = None;
    let mut alpha: Option<Vec<u8>> = None;
    let mut user_mask_channel: Option<(Vec<u8>, usize, usize, i32, i32)> = None;
    let mut real_user_mask_channel: Option<(Vec<u8>, usize, usize, i32, i32)> = None;

    for channel in channels {
        let compression = reader.read_u16()?;
        let compression = Compression::from_u16(compression)?;
        let data_length = channel
            .length
            .checked_sub(2)
            .ok_or_else(|| PsdError::InvalidFormat("Invalid channel length".to_string()))?
            as usize;
        let (_, _, channel_width, channel_height) = layer_channel_bounds(layer, channel.id);
        let channel_expected_len = channel_width * channel_height;

        let decoded = match compression {
            Compression::RawData => {
                let data = reader.read_bytes(data_length)?;
                normalize_channel_data(data, channel_expected_len)
            }
            Compression::RleCompressed => {
                let row_count = channel_height;
                let byte_count_width = if reader.large { 4 } else { 2 };
                let byte_counts_len = row_count * byte_count_width;
                if data_length < byte_counts_len {
                    return Err(PsdError::InvalidFormat(
                        "Invalid RLE channel data length".to_string(),
                    ));
                }
                let mut byte_counts = Vec::with_capacity(row_count);
                for _ in 0..row_count {
                    let v = if reader.large {
                        reader.read_u32()? as u16
                    } else {
                        reader.read_u16()?
                    };
                    byte_counts.push(v);
                }
                let compressed_len = data_length - byte_counts_len;
                let compressed = reader.read_bytes(compressed_len)?;
                let mut out = vec![0u8; channel_expected_len];
                compression::decompress_rle(
                    &compressed,
                    &mut out,
                    channel_width,
                    channel_height,
                    &byte_counts,
                )?;
                out
            }
            Compression::ZipWithoutPrediction => {
                let compressed = reader.read_bytes(data_length)?;
                let out = compression::decompress_zip(&compressed, channel_expected_len)?;
                normalize_channel_data(out, channel_expected_len)
            }
            Compression::ZipWithPrediction => {
                let compressed = reader.read_bytes(data_length)?;
                let depth = psd.bits_per_channel.unwrap_or(8) as u16;
                let out = compression::decompress_zip_with_prediction(
                    &compressed,
                    channel_width,
                    channel_height,
                    depth,
                )?;
                normalize_channel_data(out, channel_expected_len)
            }
        };
        let offset = channel_offset(channel.id, cmyk);
        match offset {
            0 => red = Some(decoded),
            1 => green = Some(decoded),
            2 => blue = Some(decoded),
            3 => alpha = Some(decoded),
            _ => {
                let (mask_left, mask_top, mask_width, mask_height) =
                    layer_channel_bounds(layer, channel.id);
                match channel.id {
                    ChannelID::UserMask => {
                        user_mask_channel =
                            Some((decoded, mask_width, mask_height, mask_left, mask_top));
                    }
                    ChannelID::RealUserMask => {
                        real_user_mask_channel =
                            Some((decoded, mask_width, mask_height, mask_left, mask_top));
                    }
                    _ => {}
                }
            }
        }
    }

    let mut rgba = vec![0u8; expected_len * 4];
    for i in 0..expected_len {
        rgba[i * 4] = red.as_ref().and_then(|d| d.get(i)).copied().unwrap_or(0);
        rgba[i * 4 + 1] = green.as_ref().and_then(|d| d.get(i)).copied().unwrap_or(0);
        rgba[i * 4 + 2] = blue.as_ref().and_then(|d| d.get(i)).copied().unwrap_or(0);
        rgba[i * 4 + 3] = alpha
            .as_ref()
            .and_then(|d| d.get(i))
            .copied()
            .unwrap_or(255);
    }

    let mut pixel_data = PixelData {
        data: rgba,
        width,
        height,
    };
    if is_grayscale {
        setup_grayscale(&mut pixel_data);
    }

    if let Some((mask_data, mask_width, mask_height, mask_left, mask_top)) =
        user_mask_channel.as_ref()
    {
        let layer_left = layer.left.unwrap_or(0);
        let layer_top = layer.top.unwrap_or(0);
        let rel_left = *mask_left - layer_left;
        let rel_top = *mask_top - layer_top;

        for mask_y in 0..*mask_height {
            for mask_x in 0..*mask_width {
                let target_x = rel_left + mask_x as i32;
                let target_y = rel_top + mask_y as i32;
                if target_x < 0
                    || target_y < 0
                    || target_x >= width as i32
                    || target_y >= height as i32
                {
                    continue;
                }
                let mask_index = mask_y * *mask_width + mask_x;
                let target_index = ((target_y as usize) * width + target_x as usize) * 4 + 3;
                pixel_data.data[target_index] = pixel_data.data[target_index].min(mask_data[mask_index]);
            }
        }
    } else if let Some((mask_data, mask_width, mask_height, mask_left, mask_top)) =
        real_user_mask_channel.as_ref()
    {
        let layer_left = layer.left.unwrap_or(0);
        let layer_top = layer.top.unwrap_or(0);
        let rel_left = *mask_left - layer_left;
        let rel_top = *mask_top - layer_top;

        for mask_y in 0..*mask_height {
            for mask_x in 0..*mask_width {
                let target_x = rel_left + mask_x as i32;
                let target_y = rel_top + mask_y as i32;
                if target_x < 0
                    || target_y < 0
                    || target_x >= width as i32
                    || target_y >= height as i32
                {
                    continue;
                }
                let mask_index = mask_y * *mask_width + mask_x;
                let target_index = ((target_y as usize) * width + target_x as usize) * 4 + 3;
                pixel_data.data[target_index] = pixel_data.data[target_index].min(mask_data[mask_index]);
            }
        }
    }

    if let Some((mask_data, mask_width, mask_height, _, _)) = user_mask_channel {
        let mask_pixel_data = PixelData {
            data: mask_data,
            width: mask_width,
            height: mask_height,
        };
        if let Some(mask) = layer.additional_info.mask.as_mut() {
            mask.image_data = Some(mask_pixel_data);
        }
    }
    if let Some((mask_data, mask_width, mask_height, _, _)) = real_user_mask_channel {
        let mask_pixel_data = PixelData {
            data: mask_data,
            width: mask_width,
            height: mask_height,
        };
        let real_mask = layer
            .additional_info
            .real_mask
            .get_or_insert_with(Default::default);
        real_mask.image_data = Some(mask_pixel_data);
    }

    layer.image_data = Some(pixel_data);
    Ok(())
}

fn read_layer_channel_raw_data<R: Read + Seek>(
    reader: &mut PsdReader<R>,
    bits_per_channel: u8,
    layer: &mut Layer,
    channels: &[ChannelInfo],
) -> Result<()> {
    let mut raw_channels = Vec::with_capacity(channels.len());
    for channel in channels {
        let compression = reader.read_u16()?;
        let compression = Compression::from_u16(compression)?;
        let data_length = channel
            .length
            .checked_sub(2)
            .ok_or_else(|| PsdError::InvalidFormat("Invalid channel length".to_string()))?
            as usize;
        let (_, _, channel_width, channel_height) = layer_channel_bounds(layer, channel.id);
        let bytes_per_sample = match bits_per_channel {
            8 => 1,
            16 => 2,
            32 => 4,
            _ => 1,
        };
        let expected_len = channel_width * channel_height * bytes_per_sample;
        let decoded = match compression {
            Compression::RawData => reader.read_bytes(data_length)?,
            Compression::RleCompressed => {
                let row_count = channel_height;
                let byte_count_width = if reader.large { 4 } else { 2 };
                let byte_counts_len = row_count * byte_count_width;
                if data_length < byte_counts_len {
                    return Err(PsdError::InvalidFormat(
                        "Invalid RLE channel data length".to_string(),
                    ));
                }
                let mut byte_counts = Vec::with_capacity(row_count);
                for _ in 0..row_count {
                    let v = if reader.large {
                        reader.read_u32()? as u16
                    } else {
                        reader.read_u16()?
                    };
                    byte_counts.push(v);
                }
                let compressed_len = data_length - byte_counts_len;
                let compressed = reader.read_bytes(compressed_len)?;
                let mut out = vec![0u8; expected_len];
                compression::decompress_rle(
                    &compressed,
                    &mut out,
                    channel_width * bytes_per_sample,
                    channel_height,
                    &byte_counts,
                )?;
                out
            }
            Compression::ZipWithoutPrediction => {
                let compressed = reader.read_bytes(data_length)?;
                compression::decompress_zip(&compressed, expected_len)?
            }
            Compression::ZipWithPrediction => {
                let compressed = reader.read_bytes(data_length)?;
                compression::decompress_zip_with_prediction(
                    &compressed,
                    channel_width,
                    channel_height,
                    bits_per_channel as u16,
                )?
            }
        };
        raw_channels.push(LayerRawDataChannel {
            id: channel.id,
            compression,
            data: Some(decoded),
        });
    }
    layer.raw_data = Some(LayerRawData {
        color_mode: ColorMode::RGB,
        bits_per_channel,
        channels: raw_channels,
        large: reader.large,
    });
    Ok(())
}

/// Build layer hierarchy from flat layer list
fn build_layer_hierarchy(psd: &mut Psd, layers: Vec<Layer>) -> Result<()> {
    let mut stack: Vec<(Vec<Layer>, Option<bool>, Option<String>)> =
        vec![(Vec::new(), None, None)];

    for mut layer in layers.into_iter().rev() {
        let section_type = layer
            .additional_info
            .section_divider
            .as_ref()
            .map(|sd| sd.divider_type)
            .unwrap_or(SectionDividerType::Other);

        match section_type {
            SectionDividerType::BoundingSectionDivider => {
                let (children, opened, close_name) = if stack.len() > 1 {
                    stack.pop().unwrap()
                } else {
                    (Vec::new(), None, None)
                };
                if !children.is_empty() {
                    layer.children = Some(children);
                }
                layer.opened = opened;
                if let Some(ref name) = layer.additional_info.name {
                    if name.starts_with("</") {
                        if let Some(close_name) = close_name {
                            layer.additional_info.name = Some(close_name);
                        }
                    }
                }
                stack.last_mut().unwrap().0.insert(0, layer);
            }
            SectionDividerType::OpenFolder | SectionDividerType::ClosedFolder => {
                stack.push((
                    Vec::new(),
                    Some(matches!(section_type, SectionDividerType::OpenFolder)),
                    layer.additional_info.name.clone(),
                ));
            }
            SectionDividerType::Other => {
                stack.last_mut().unwrap().0.insert(0, layer);
            }
        }
    }

    psd.children = Some(stack.pop().map(|(layers, _, _)| layers).unwrap_or_default());
    Ok(())
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

/// Read global layer mask info
fn read_global_layer_mask_info<R: Read + Seek>(
    reader: &mut PsdReader<R>,
) -> Result<Option<GlobalLayerMaskInfo>> {
    reader.read_section(1, |reader, end_offset| {
        if reader.bytes_left(end_offset) == 0 {
            return Ok(None);
        }

        let record: GlobalLayerMaskRecord =
            decode_be(&reader.read_bytes(16)?, "global layer mask info")?;

        reader.skip_bytes(reader.bytes_left(end_offset))?;

        Ok(Some(GlobalLayerMaskInfo {
            overlay_color_space: record.overlay_color_space,
            color_space1: record.color_space1,
            color_space2: record.color_space2,
            color_space3: record.color_space3,
            color_space4: record.color_space4,
            opacity: record.opacity,
            kind: record.kind,
        }))
    })
}

/// Read image data section
fn read_image_data<R: Read + Seek>(reader: &mut PsdReader<R>, psd: &mut Psd) -> Result<()> {
    let compression = reader.read_u16()?;
    let compression = Compression::from_u16(compression)?;
    let width = psd.width as usize;
    let height = psd.height as usize;
    if width == 0 || height == 0 {
        return Ok(());
    }

    let color_mode = psd.color_mode.unwrap_or(ColorMode::RGB);
    if !matches!(
        color_mode,
        ColorMode::RGB
            | ColorMode::Grayscale
            | ColorMode::Bitmap
            | ColorMode::Indexed
            | ColorMode::CMYK
    ) {
        return Err(PsdError::UnsupportedFeature(format!(
            "Color mode not supported for composite image: {:?}",
            color_mode
        )));
    }
    let channel_len = width * height;
    let base_channels = match color_mode {
        ColorMode::Grayscale => 1usize,
        ColorMode::CMYK => 4usize,
        _ => 3usize,
    };
    let mut total_channels = psd.channels.unwrap_or(base_channels as u16) as usize;
    if total_channels == 0 {
        total_channels = base_channels;
    }
    if reader.global_alpha && total_channels == base_channels {
        total_channels += 1;
    }
    let bits_per_channel = psd.bits_per_channel.unwrap_or(8) as u16;
    let bytes_per_sample = match bits_per_channel {
        8 => 1usize,
        16 => 2,
        32 => 4,
        _ => {
            return Err(PsdError::UnsupportedFeature(format!(
                "Unsupported bits per channel for composite image: {}",
                bits_per_channel
            )))
        }
    };
    let channel_len_bytes = channel_len * bytes_per_sample;

    let mut planes: Vec<Vec<u8>> = vec![Vec::new(); total_channels];
    match compression {
        Compression::RawData => {
            for i in 0..total_channels {
                let plane = reader.read_bytes(channel_len_bytes)?;
                planes[i] = normalize_channel_data(plane, channel_len_bytes);
            }
        }
        Compression::RleCompressed => {
            let row_count = total_channels * height;
            let mut byte_counts = Vec::with_capacity(row_count);
            for _ in 0..row_count {
                let v = if reader.large {
                    reader.read_u32()? as u16
                } else {
                    reader.read_u16()?
                };
                byte_counts.push(v);
            }
            for channel_index in 0..total_channels {
                let start = channel_index * height;
                let end = start + height;
                let channel_counts = &byte_counts[start..end];
                let compressed_len = channel_counts.iter().map(|v| *v as usize).sum();
                let compressed = reader.read_bytes(compressed_len)?;
                let mut out = vec![0u8; channel_len_bytes];
                compression::decompress_rle(
                    &compressed,
                    &mut out,
                    width * bytes_per_sample,
                    height,
                    channel_counts,
                )?;
                planes[channel_index] = out;
            }
        }
        Compression::ZipWithoutPrediction | Compression::ZipWithPrediction => {
            let compressed = reader.read_remaining_bytes()?;
            let expected_total = channel_len_bytes * total_channels;
            let mut data = compression::decompress_zip(&compressed, expected_total)?;
            data = normalize_channel_data(data, expected_total);
            if compression == Compression::ZipWithPrediction {
                reverse_prediction_planar(
                    &mut data,
                    width,
                    height,
                    total_channels,
                    bits_per_channel,
                );
            }
            for (idx, plane) in planes.iter_mut().enumerate() {
                let start = idx * channel_len_bytes;
                let end = start + channel_len_bytes;
                *plane = data[start..end].to_vec();
            }
        }
    }

    let mut rgba = vec![0u8; channel_len * 4];
    for i in 0..channel_len {
        for (channel_idx, channel) in planes.iter().enumerate() {
            let value = sample_to_u8(channel, i, bits_per_channel);
            match color_mode {
                ColorMode::CMYK => match channel_idx {
                    0 | 1 | 2 | 3 => {}
                    4 => rgba[i * 4 + 3] = 255u8.saturating_sub(value),
                    _ => {}
                },
                ColorMode::Grayscale => match channel_idx {
                    0 => rgba[i * 4] = value,
                    1 => rgba[i * 4 + 3] = value,
                    _ => {}
                },
                _ => match channel_idx {
                    0 => rgba[i * 4] = value,
                    1 => rgba[i * 4 + 1] = value,
                    2 => rgba[i * 4 + 2] = value,
                    3 => rgba[i * 4 + 3] = value,
                    _ => {}
                },
            }
        }
        if color_mode == ColorMode::CMYK {
            let c = planes.get(0).and_then(|p| p.get(i)).copied().unwrap_or(0) as u32;
            let m = planes.get(1).and_then(|p| p.get(i)).copied().unwrap_or(0) as u32;
            let y = planes.get(2).and_then(|p| p.get(i)).copied().unwrap_or(0) as u32;
            let k = planes.get(3).and_then(|p| p.get(i)).copied().unwrap_or(0) as u32;
            rgba[i * 4] = ((255 * (255 - c) * (255 - k)) / (255 * 255)) as u8;
            rgba[i * 4 + 1] = ((255 * (255 - m) * (255 - k)) / (255 * 255)) as u8;
            rgba[i * 4 + 2] = ((255 * (255 - y) * (255 - k)) / (255 * 255)) as u8;
            if total_channels <= 4 {
                rgba[i * 4 + 3] = 255;
            }
        } else if total_channels <= 3 {
            rgba[i * 4 + 3] = 255;
        }
    }

    let mut pixel_data = PixelData {
        data: rgba,
        width,
        height,
    };
    if color_mode == ColorMode::Grayscale {
        setup_grayscale(&mut pixel_data);
    }
    // Note: TS source of truth stores raw channel data without white-matte removal.
    // Removing white matte is intentionally omitted to match TS behavior.
    psd.image_data = Some(pixel_data);

    Ok(())
}

fn reverse_prediction_planar(
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
                    let row_start = plane_start + row * width;
                    for x in 1..width {
                        let pos = row_start + x;
                        data[pos] = data[pos].wrapping_add(data[pos - 1]);
                    }
                }
            }
            16 => {
                let row_bytes = width * 2;
                for row in 0..height {
                    let start = plane_start + row * row_bytes;
                    for i in start + 1..start + row_bytes {
                        data[i] = data[i].wrapping_add(data[i - 1]);
                    }
                }
            }
            32 => {
                let row_bytes = width * 4;
                let mut reordered = vec![0u8; row_bytes];
                for row in 0..height {
                    let row_off = plane_start + row * row_bytes;
                    reordered.copy_from_slice(&data[row_off..row_off + row_bytes]);
                    for plane in 0..4usize {
                        let base = plane * width;
                        for i in 1..width {
                            reordered[base + i] =
                                reordered[base + i].wrapping_add(reordered[base + i - 1]);
                        }
                    }
                    for pixel in 0..width {
                        let dst = row_off + pixel * 4;
                        data[dst] = reordered[pixel];
                        data[dst + 1] = reordered[width + pixel];
                        data[dst + 2] = reordered[width * 2 + pixel];
                        data[dst + 3] = reordered[width * 3 + pixel];
                    }
                }
            }
            _ => {}
        }
    }
}

fn sample_to_u8(channel: &[u8], index: usize, depth: u16) -> u8 {
    match depth {
        8 => channel.get(index).copied().unwrap_or(0),
        16 => {
            let start = index * 2;
            channel.get(start).copied().unwrap_or(0)
        }
        32 => {
            let start = index * 4;
            if start + 4 > channel.len() {
                0
            } else {
                let value = f32::from_be_bytes([
                    channel[start],
                    channel[start + 1],
                    channel[start + 2],
                    channel[start + 3],
                ]);
                (value.clamp(0.0, 1.0) * 255.0).round() as u8
            }
        }
        _ => 0,
    }
}

fn channel_offset(id: ChannelID, cmyk: bool) -> i32 {
    match id {
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
        ChannelID::UserMask | ChannelID::RealUserMask => -1,
    }
}

fn normalize_channel_data(mut data: Vec<u8>, expected_len: usize) -> Vec<u8> {
    if data.len() < expected_len {
        data.resize(expected_len, 0);
        return data;
    }
    if data.len() > expected_len {
        data.truncate(expected_len);
    }
    data
}

fn parse_layer_blending_ranges(bytes: &[u8]) -> Option<crate::api::layer::LayerBlendingRangesData> {
    if bytes.is_empty() {
        return None;
    }
    let mut offset = 0;
    let read_pair =
        |buf: &[u8], offset: &mut usize| -> Option<crate::api::layer::LayerBlendingRangePair> {
            if *offset + 4 > buf.len() {
                return None;
            }
            let pair = crate::api::layer::LayerBlendingRangePair {
                src_black: buf[*offset],
                src_white: buf[*offset + 1],
                dst_black: buf[*offset + 2],
                dst_white: buf[*offset + 3],
            };
            *offset += 4;
            Some(pair)
        };

    let composite_gray = read_pair(bytes, &mut offset);
    let mut channels = Vec::new();
    while let Some(pair) = read_pair(bytes, &mut offset) {
        channels.push(pair);
    }

    Some(crate::api::layer::LayerBlendingRangesData {
        composite_gray,
        channels,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::io::writer::flatten_layers;
    use crate::{write_psd, Layer, PixelData, Psd, WriteOptions};
    use std::io::Cursor;
    use std::path::PathBuf;

    fn minimal_valid_psd() -> Vec<u8> {
        vec![
            b'8', b'B', b'P', b'S', // signature
            0x00, 0x01, // version
            0, 0, 0, 0, 0, 0, // reserved
            0x00, 0x03, // channels
            0x00, 0x00, 0x00, 0x01, // height
            0x00, 0x00, 0x00, 0x01, // width
            0x00, 0x08, // depth
            0x00, 0x03, // RGB
            0x00, 0x00, 0x00, 0x00, // color mode data length
            0x00, 0x00, 0x00, 0x00, // image resources length
            0x00, 0x00, 0x00, 0x00, // layer and mask length
            0x00, 0x00, // image compression = raw
            0x00, 0x00, 0x00, // one byte per channel
        ]
    }

    fn samples_dir() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .parent()
            .unwrap()
            .join("photoshop/psd/samples")
            .canonicalize()
            .unwrap_or_else(|_| {
                std::env::current_dir()
                    .unwrap()
                    .join("../photoshop/psd/samples")
                    .canonicalize()
                    .unwrap()
            })
    }

    fn read_flat_layers_for_sample(bytes: &[u8]) -> Result<Vec<Layer>> {
        let mut reader = PsdReader::new(Cursor::new(bytes.to_vec()), ReadOptions::default());
        let header: PsdHeaderRecord = decode_be(&reader.read_bytes(26)?, "PSD header")?;
        assert_eq!(&header.signature, b"8BPS");
        reader.large = header.version == 2;
        let mut psd = Psd {
            width: header.width,
            height: header.height,
            channels: Some(header.channels),
            bits_per_channel: Some(header.depth as u8),
            color_mode: Some(ColorMode::from_u16(header.color_mode)?),
            ..Default::default()
        };
        read_color_mode_data(&mut reader, &mut psd)?;
        read_image_resources(&mut reader, &mut psd)?;

        reader.read_section(1, |reader, end_offset| {
            let mut flat_layers = Vec::new();
            if reader.bytes_left(end_offset) > 0 {
                reader.read_section(2, |reader, end_offset| {
                    let mut layer_count = reader.read_i16()? as i32;
                    if layer_count < 0 {
                        layer_count = -layer_count;
                    }

                    let mut layer_channels = Vec::new();
                    for _ in 0..layer_count {
                        let (layer, channels) = read_layer_record(reader)?;
                        flat_layers.push(layer);
                        layer_channels.push(channels);
                    }
                    for (i, channels) in layer_channels.iter().enumerate() {
                        read_layer_channel_image_data(reader, &psd, &mut flat_layers[i], channels)?;
                    }
                    reader.skip_bytes(reader.bytes_left(end_offset))?;
                    Ok(())
                })?;
            }
            Ok(flat_layers)
        })
    }

    fn layer_signature(layer: &Layer) -> (Option<SectionDividerType>, Option<String>) {
        let divider = layer
            .additional_info
            .section_divider
            .as_ref()
            .map(|item| item.divider_type);
        let name = if divider.is_some() {
            None
        } else {
            layer.additional_info.name.clone()
        };
        (divider, name)
    }

    #[test]
    fn test_read_signature() {
        let data = b"8BPS";
        let mut reader = PsdReader::new(Cursor::new(data), ReadOptions::default());
        let sig = reader.read_signature().unwrap();
        assert_eq!(sig, "8BPS");
    }

    #[test]
    fn test_read_pascal_string() {
        let data = vec![5, b'H', b'e', b'l', b'l', b'o', 0, 0]; // "Hello" padded to 8 bytes
        let mut reader = PsdReader::new(Cursor::new(data), ReadOptions::default());
        let s = reader.read_pascal_string(4).unwrap();
        assert_eq!(s, "Hello");
    }

    #[test]
    fn test_read_integers() {
        let data = vec![
            0x00, 0x01, // u16: 1
            0x00, 0x00, 0x00, 0x02, // u32: 2
        ];
        let mut reader = PsdReader::new(Cursor::new(data), ReadOptions::default());
        assert_eq!(reader.read_u16().unwrap(), 1);
        assert_eq!(reader.read_u32().unwrap(), 2);
    }

    #[test]
    fn test_read_section_applies_padding_for_round() {
        let data = vec![
            0x00, 0x00, 0x00, 0x01, // section length = 1
            0xAA, // payload
            0x00, // pad to 2-byte boundary
            0xBB, // next byte after section
        ];
        let mut reader = PsdReader::new(Cursor::new(data), ReadOptions::default());
        let payload = reader
            .read_section(2, |r, _| r.read_u8())
            .expect("read section");
        assert_eq!(payload, 0xAA);
        let next = reader.read_u8().expect("next byte");
        assert_eq!(next, 0xBB);
    }

    #[test]
    fn test_rejects_non_zero_reserved_header_bytes() {
        let mut bytes = minimal_valid_psd();
        bytes[6] = 1;
        let err = read_psd(Cursor::new(bytes), ReadOptions::default()).unwrap_err();
        assert!(err.to_string().contains("reserved"));
    }

    #[test]
    fn test_rejects_zero_width_in_header() {
        let mut bytes = minimal_valid_psd();
        bytes[18..22].copy_from_slice(&0u32.to_be_bytes());
        let err = read_psd(Cursor::new(bytes), ReadOptions::default()).unwrap_err();
        assert!(err.to_string().contains("Invalid size"));
    }

    #[test]
    fn test_read_color_cmyk() {
        let bytes = [
            0x00, 0x02, // CMYK
            0xFF, 0xFF, // C
            0x80, 0x80, // M
            0x40, 0x40, // Y
            0x00, 0x00, // K
        ];
        let mut reader = PsdReader::new(Cursor::new(bytes), ReadOptions::default());
        let color = reader.read_color().unwrap();
        assert_eq!(
            color,
            crate::api::types::Color::CMYK(crate::api::types::CMYK {
                c: 65535,
                m: 32896,
                y: 16448,
                k: 0,
            })
        );
    }

    #[test]
    fn test_read_color_preserves_grayscale_0_to_10000() {
        let bytes = [
            0x00, 0x08, // grayscale
            0x27, 0x10, // 10000
            0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        ];
        let mut reader = PsdReader::new(Cursor::new(bytes), ReadOptions::default());
        let color = reader.read_color().unwrap();
        assert_eq!(
            color,
            crate::api::types::Color::Grayscale(crate::api::types::Grayscale { k: 10000 })
        );
    }

    #[test]
    fn test_read_color_preserves_opaque_custom_space() {
        let bytes = [
            0x00, 0x03, // custom space
            0x00, 0x01, 0x00, 0x02, 0x00, 0x03, 0x00, 0x04,
        ];
        let mut reader = PsdReader::new(Cursor::new(bytes), ReadOptions::default());
        let color = reader.read_color().unwrap();
        assert_eq!(
            color,
            crate::api::types::Color::OpaqueColorSpace {
                color_space: 3,
                components: [1, 2, 3, 4],
            }
        );
    }

    #[test]
    fn test_roundtrip_reads_layer_and_composite_pixels() {
        let layer = Layer {
            top: Some(0),
            left: Some(0),
            bottom: Some(1),
            right: Some(1),
            image_data: Some(PixelData {
                data: vec![255, 0, 0, 255],
                width: 1,
                height: 1,
            }),
            ..Default::default()
        };
        let psd = Psd {
            width: 1,
            height: 1,
            children: Some(vec![layer]),
            image_data: Some(PixelData {
                data: vec![0, 255, 0, 255],
                width: 1,
                height: 1,
            }),
            ..Default::default()
        };
        let bytes = write_psd(
            &psd,
            &WriteOptions {
                compress: Some(false),
                ..Default::default()
            },
        )
        .expect("write psd");
        let loaded = read_psd(
            Cursor::new(bytes),
            ReadOptions {
                skip_layer_image_data: Some(false),
                skip_composite_image_data: Some(false),
                ..Default::default()
            },
        )
        .expect("read psd");

        let top = loaded
            .children
            .as_ref()
            .and_then(|c| c.first())
            .and_then(|l| l.image_data.as_ref())
            .expect("layer image");
        assert_eq!(top.data, vec![255, 0, 0, 255]);

        let composite = loaded.image_data.expect("composite image");
        assert_eq!(composite.data, vec![0, 255, 0, 255]);
    }

    #[test]
    fn test_build_layer_hierarchy_uses_bounding_as_group_layer() {
        let mut psd = Psd::default();
        let group = Layer {
            additional_info: crate::format::additional_info::LayerAdditionalInfo {
                name: Some("Group".to_string()),
                section_divider: Some(crate::format::additional_info::SectionDivider {
                    divider_type: SectionDividerType::BoundingSectionDivider,
                    blend_mode: None,
                    sub_type: None,
                }),
                ..Default::default()
            },
            ..Default::default()
        };
        let leaf = Layer {
            additional_info: crate::format::additional_info::LayerAdditionalInfo {
                name: Some("Leaf".to_string()),
                ..Default::default()
            },
            ..Default::default()
        };
        let close = Layer {
            additional_info: crate::format::additional_info::LayerAdditionalInfo {
                section_divider: Some(crate::format::additional_info::SectionDivider {
                    divider_type: SectionDividerType::ClosedFolder,
                    blend_mode: None,
                    sub_type: None,
                }),
                ..Default::default()
            },
            ..Default::default()
        };

        build_layer_hierarchy(&mut psd, vec![group, leaf, close]).expect("build hierarchy");

        let roots = psd.children.expect("root layers");
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
            Some("Leaf")
        );
    }

    #[test]
    fn test_build_layer_hierarchy_uses_close_marker_name_for_generic_bounding_name() {
        let mut psd = Psd::default();
        let group = Layer {
            additional_info: crate::format::additional_info::LayerAdditionalInfo {
                name: Some("</Layer group>".to_string()),
                section_divider: Some(crate::format::additional_info::SectionDivider {
                    divider_type: SectionDividerType::BoundingSectionDivider,
                    blend_mode: None,
                    sub_type: None,
                }),
                ..Default::default()
            },
            ..Default::default()
        };
        let leaf = Layer {
            additional_info: crate::format::additional_info::LayerAdditionalInfo {
                name: Some("Leaf".to_string()),
                ..Default::default()
            },
            ..Default::default()
        };
        let close = Layer {
            additional_info: crate::format::additional_info::LayerAdditionalInfo {
                name: Some("top".to_string()),
                section_divider: Some(crate::format::additional_info::SectionDivider {
                    divider_type: SectionDividerType::ClosedFolder,
                    blend_mode: None,
                    sub_type: None,
                }),
                ..Default::default()
            },
            ..Default::default()
        };

        build_layer_hierarchy(&mut psd, vec![group, leaf, close]).expect("build hierarchy");

        let roots = psd.children.expect("root layers");
        assert_eq!(roots.len(), 1);
        assert_eq!(roots[0].additional_info.name.as_deref(), Some("top"));
        assert_eq!(
            roots[0]
                .children
                .as_ref()
                .expect("group children")
                .first()
                .and_then(|child| child.additional_info.name.as_deref()),
            Some("Leaf")
        );
    }

    #[test]
    fn test_layer_user_mask_modulates_alpha_and_is_preserved() {
        let mut bytes = Vec::new();
        for data in [&[10u8, 20][..], &[0u8, 0][..], &[0u8, 0][..], &[255u8, 255][..]] {
            bytes.extend_from_slice(&0u16.to_be_bytes());
            bytes.extend_from_slice(data);
        }
        bytes.extend_from_slice(&0u16.to_be_bytes());
        bytes.push(0);

        let mut reader = PsdReader::new(Cursor::new(bytes), ReadOptions::default());
        let psd = Psd {
            color_mode: Some(ColorMode::RGB),
            bits_per_channel: Some(8),
            ..Default::default()
        };
        let mut layer = Layer {
            left: Some(0),
            top: Some(0),
            right: Some(2),
            bottom: Some(1),
            additional_info: crate::format::additional_info::LayerAdditionalInfo {
                mask: Some(LayerMaskData {
                    left: Some(1),
                    top: Some(0),
                    right: Some(2),
                    bottom: Some(1),
                    ..Default::default()
                }),
                ..Default::default()
            },
            ..Default::default()
        };
        let channels = vec![
            ChannelInfo {
                id: ChannelID::Color0,
                length: 4,
            },
            ChannelInfo {
                id: ChannelID::Color1,
                length: 4,
            },
            ChannelInfo {
                id: ChannelID::Color2,
                length: 4,
            },
            ChannelInfo {
                id: ChannelID::Transparency,
                length: 4,
            },
            ChannelInfo {
                id: ChannelID::UserMask,
                length: 3,
            },
        ];

        read_layer_channel_image_data(&mut reader, &psd, &mut layer, &channels).expect("read channels");

        let pixels = layer.image_data.expect("layer image").data;
        assert_eq!(pixels, vec![10, 0, 0, 255, 20, 0, 0, 0]);

        let mask = layer
            .additional_info
            .mask
            .and_then(|mask| mask.image_data)
            .expect("mask image");
        assert_eq!(mask.width, 1);
        assert_eq!(mask.height, 1);
        assert_eq!(mask.data, vec![0]);
    }

    #[test]
    fn test_layer_preserves_user_and_real_mask_channels_separately() {
        let mut bytes = Vec::new();
        for data in [
            &[10u8, 20][..],
            &[0u8, 0][..],
            &[0u8, 0][..],
            &[255u8, 255][..],
            &[100u8][..],
            &[50u8][..],
        ] {
            bytes.extend_from_slice(&0u16.to_be_bytes());
            bytes.extend_from_slice(data);
        }

        let mut reader = PsdReader::new(Cursor::new(bytes), ReadOptions::default());
        let psd = Psd {
            color_mode: Some(ColorMode::RGB),
            bits_per_channel: Some(8),
            ..Default::default()
        };
        let mut layer = Layer {
            left: Some(0),
            top: Some(0),
            right: Some(2),
            bottom: Some(1),
            additional_info: crate::format::additional_info::LayerAdditionalInfo {
                mask: Some(LayerMaskData {
                    left: Some(0),
                    top: Some(0),
                    right: Some(1),
                    bottom: Some(1),
                    real_left: Some(1),
                    real_top: Some(0),
                    real_right: Some(2),
                    real_bottom: Some(1),
                    ..Default::default()
                }),
                ..Default::default()
            },
            ..Default::default()
        };
        let channels = vec![
            ChannelInfo {
                id: ChannelID::Color0,
                length: 4,
            },
            ChannelInfo {
                id: ChannelID::Color1,
                length: 4,
            },
            ChannelInfo {
                id: ChannelID::Color2,
                length: 4,
            },
            ChannelInfo {
                id: ChannelID::Transparency,
                length: 4,
            },
            ChannelInfo {
                id: ChannelID::UserMask,
                length: 3,
            },
            ChannelInfo {
                id: ChannelID::RealUserMask,
                length: 3,
            },
        ];

        read_layer_channel_image_data(&mut reader, &psd, &mut layer, &channels).expect("read channels");

        let pixels = layer.image_data.expect("layer image").data;
        assert_eq!(pixels, vec![10, 0, 0, 100, 20, 0, 0, 255]);

        let user_mask = layer
            .additional_info
            .mask
            .as_ref()
            .and_then(|mask| mask.image_data.as_ref())
            .expect("user mask image");
        assert_eq!(user_mask.width, 1);
        assert_eq!(user_mask.height, 1);
        assert_eq!(user_mask.data, vec![100]);

        let real_mask = layer
            .additional_info
            .real_mask
            .as_ref()
            .and_then(|mask| mask.image_data.as_ref())
            .expect("real mask image");
        assert_eq!(real_mask.width, 1);
        assert_eq!(real_mask.height, 1);
        assert_eq!(real_mask.data, vec![50]);
    }

    #[test]
    fn sample_files_preserve_layer_group_separator_order() {
        let samples = [
            "3d-preview-mockup.psd",
            "4901393.psd",
            "images.psd",
            "multi-value-items.psd",
            "placeholders-with-frames.psd",
            "rich-text.psd",
            "sample_1920×1280.psd",
            "text.psd",
        ];

        for sample in samples {
            let path = samples_dir().join(sample);
            let bytes = std::fs::read(&path).expect("read sample");
            let flat = read_flat_layers_for_sample(&bytes).expect("read flat layers");

            let mut rebuilt_psd = Psd::default();
            build_layer_hierarchy(&mut rebuilt_psd, flat.clone()).expect("build hierarchy");
            let reflattened = flatten_layers(rebuilt_psd.children.as_ref());

            let flat_sig: Vec<_> = flat.iter().map(layer_signature).collect();
            let reflattened_sig: Vec<_> = reflattened.iter().map(layer_signature).collect();

            assert_eq!(
                reflattened_sig, flat_sig,
                "layer/group order mismatch for {}",
                sample
            );
        }
    }
}

//! PSD file reader implementation
//!
//! Provides functionality to read PSD files and parse their structure.

use crate::api::layer::{Layer, LayerMaskData, LayerRawData, LayerRawDataChannel};
use crate::api::psd::{GlobalLayerMaskInfo, Psd, ReadOptions};
use crate::api::types::{ChannelID, ColorMode, Compression, PixelData, SectionDividerType};
use crate::support::binrw_support::{
    decode_be, ChannelInfoRecord, GlobalLayerMaskRecord, LayerBlendRecord, LayerMaskPrefixRecord,
    LayerRecordBounds, PsbChannelInfoRecord, PsdHeaderRecord,
};
use crate::support::compression;
use crate::support::error::{PsdError, Result};
use crate::support::helpers::{
    setup_grayscale, to_blend_mode, LayerBlendFlags, LayerMaskParameterFlags, LayerMaskStateBits,
};
use byteorder::{BigEndian, ReadBytesExt};
use std::io::{Read, Seek, SeekFrom};

/// PSD reader for binary data
pub struct PsdReader<R: Read + Seek> {
    reader: R,
    pub offset: u64,
    pub large: bool,
    pub global_alpha: bool,
    pub color_mode: Option<ColorMode>,
    pub options: ReadOptions,
    /// End offset of the innermost `read_section` payload, if any. Primitive
    /// reads refuse to consume bytes at or past this offset so a handler can
    /// never overread into sibling data.
    section_end: Option<u64>,
}

impl<R: Read + Seek> PsdReader<R> {
    /// Create a new PSD reader
    pub fn new(reader: R, options: ReadOptions) -> Self {
        Self {
            reader,
            offset: 0,
            large: false,
            global_alpha: false,
            color_mode: None,
            options,
            section_end: None,
        }
    }

    /// Validate that reading `extra` more bytes from the current offset stays
    /// inside the enclosing section payload.
    fn ensure_within_section(&self, extra: u64) -> Result<()> {
        if let Some(end) = self.section_end {
            let new_offset = self
                .offset
                .checked_add(extra)
                .ok_or_else(|| PsdError::InvalidFormat("Section read overflow".to_string()))?;
            if new_offset > end {
                return Err(PsdError::InvalidFormat(format!(
                    "Section overread: offset {} + {} exceeds section end {}",
                    self.offset, extra, end
                )));
            }
        }
        Ok(())
    }

    /// Whether strict parsing is in effect (`strict` or
    /// `throw_for_missing_features`).
    pub(crate) fn strict_enabled(&self) -> bool {
        self.options.strict.unwrap_or(false)
            || self.options.throw_for_missing_features.unwrap_or(false)
    }

    /// Temporarily constrain reads to end at `end`; returns the previous bound
    /// for later restoration with [`pop_section_bound`].
    pub(crate) fn push_section_bound(&mut self, end: u64) -> Option<u64> {
        let previous = self.section_end;
        self.section_end = Some(end);
        previous
    }

    /// Restore a bound previously saved by [`push_section_bound`].
    pub(crate) fn pop_section_bound(&mut self, previous: Option<u64>) {
        self.section_end = previous;
    }

    /// Read an unsigned 8-bit integer
    pub fn read_u8(&mut self) -> Result<u8> {
        self.ensure_within_section(1)?;
        let val = self.reader.read_u8()?;
        self.offset += 1;
        Ok(val)
    }

    /// Peek at an unsigned 8-bit integer without advancing
    pub fn peek_u8(&mut self) -> Result<u8> {
        self.ensure_within_section(1)?;
        let pos = self.reader.stream_position()?;
        let val = self.reader.read_u8()?;
        self.reader.seek(SeekFrom::Start(pos))?;
        Ok(val)
    }

    /// Peek a 4-character signature without advancing.
    pub fn peek_signature(&mut self) -> Result<String> {
        self.ensure_within_section(4)?;
        let pos = self.reader.stream_position()?;
        let bytes = self.read_bytes(4)?;
        self.reader.seek(SeekFrom::Start(pos))?;
        self.offset = pos;
        Ok(String::from_utf8_lossy(&bytes).to_string())
    }

    /// Read a signed 16-bit integer (big-endian)
    pub fn read_i16(&mut self) -> Result<i16> {
        self.ensure_within_section(2)?;
        let val = self.reader.read_i16::<BigEndian>()?;
        self.offset += 2;
        Ok(val)
    }

    /// Read an unsigned 16-bit integer (big-endian)
    pub fn read_u16(&mut self) -> Result<u16> {
        self.ensure_within_section(2)?;
        let val = self.reader.read_u16::<BigEndian>()?;
        self.offset += 2;
        Ok(val)
    }

    /// Read a signed 32-bit integer (big-endian)
    pub fn read_i32(&mut self) -> Result<i32> {
        self.ensure_within_section(4)?;
        let val = self.reader.read_i32::<BigEndian>()?;
        self.offset += 4;
        Ok(val)
    }

    /// Read an unsigned 32-bit integer (big-endian)
    pub fn read_u32(&mut self) -> Result<u32> {
        self.ensure_within_section(4)?;
        let val = self.reader.read_u32::<BigEndian>()?;
        self.offset += 4;
        Ok(val)
    }

    /// Read a 32-bit float (big-endian)
    pub fn read_f32(&mut self) -> Result<f32> {
        self.ensure_within_section(4)?;
        let val = self.reader.read_f32::<BigEndian>()?;
        self.offset += 4;
        Ok(val)
    }

    /// Read a 64-bit float (big-endian)
    pub fn read_f64(&mut self) -> Result<f64> {
        self.ensure_within_section(8)?;
        let val = self.reader.read_f64::<BigEndian>()?;
        self.offset += 8;
        Ok(val)
    }

    /// Read raw bytes.
    ///
    /// The buffer is grown incrementally as data actually arrives rather than
    /// pre-allocated from the declared length, so a hostile length can never
    /// trigger a large allocation on a small input. An early EOF is an error.
    pub fn read_bytes(&mut self, length: usize) -> Result<Vec<u8>> {
        self.ensure_within_section(length as u64)?;
        crate::support::limits::check_decoded_buffer(length, "raw read buffer")?;
        let mut buffer = Vec::new();
        let mut remaining = length;
        let mut chunk = [0u8; 65536];
        while remaining > 0 {
            let want = remaining.min(chunk.len());
            let n = self.reader.read(&mut chunk[..want])?;
            if n == 0 {
                return Err(PsdError::Io(std::io::Error::new(
                    std::io::ErrorKind::UnexpectedEof,
                    format!(
                        "unexpected EOF while reading {} more byte(s) of data",
                        remaining
                    ),
                )));
            }
            buffer.extend_from_slice(&chunk[..n]);
            remaining -= n;
        }
        self.offset += length as u64;
        Ok(buffer)
    }

    /// Skip bytes
    pub fn skip_bytes(&mut self, count: usize) -> Result<()> {
        self.ensure_within_section(count as u64)?;
        self.reader.seek(SeekFrom::Current(count as i64))?;
        self.offset += count as u64;
        Ok(())
    }

    pub(crate) fn seek_to(&mut self, position: u64) -> Result<()> {
        if let Some(end) = self.section_end {
            if position > end {
                return Err(PsdError::InvalidFormat(format!(
                    "Section seek past end: {} > {}",
                    position, end
                )));
            }
        }
        self.reader.seek(SeekFrom::Start(position))?;
        self.offset = position;
        Ok(())
    }

    /// Read all remaining bytes of the enclosing section (or to EOF when not
    /// inside a section) from the current offset.
    pub fn read_remaining_bytes(&mut self) -> Result<Vec<u8>> {
        let cur = self.reader.stream_position()?;
        let end = match self.section_end {
            Some(section_end) => section_end,
            None => self.reader.seek(SeekFrom::End(0))?,
        };
        if end < cur {
            return Err(PsdError::InvalidFormat(
                "Section end precedes current offset".to_string(),
            ));
        }
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
        let mut units = Vec::with_capacity(length);
        for _ in 0..length {
            units.push(self.read_u16()?);
        }
        if units.last() == Some(&0) {
            units.pop();
        }
        Ok(String::from_utf16_lossy(&units))
    }

    /// Read an ASCII string
    pub fn read_ascii_string(&mut self, length: usize) -> Result<String> {
        let bytes = self.read_bytes(length)?;
        Ok(String::from_utf8_lossy(&bytes).to_string())
    }

    /// Read a section with length prefix.
    ///
    /// The declared payload length is validated against the enclosing section
    /// and the physical input length, and the handler runs with a hard read
    /// bound at the payload end, so it can never overread into sibling data.
    pub fn read_section<F, T>(&mut self, round: usize, eight_byte: bool, func: F) -> Result<T>
    where
        F: FnOnce(&mut Self, u64) -> Result<T>,
    {
        if round == 0 {
            return Err(PsdError::InvalidFormat(
                "Section alignment must be non-zero".to_string(),
            ));
        }
        let length = if eight_byte {
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
        let end_offset = start_offset
            .checked_add(length as u64)
            .ok_or_else(|| PsdError::InvalidFormat("Section length overflow".to_string()))?;

        // The declared payload must not exceed the enclosing section or the
        // physical input length (a seek past EOF would otherwise only be
        // noticed later, after sibling data is consumed or lost).
        self.ensure_within_section(length as u64)?;
        let physical_end = self.reader.seek(SeekFrom::End(0))?;
        self.reader.seek(SeekFrom::Start(self.offset))?;
        if end_offset > physical_end {
            return Err(PsdError::InvalidFormat(format!(
                "Section length {} exceeds available input (payload end {} > file end {})",
                length, end_offset, physical_end
            )));
        }

        let previous_bound = self.section_end;
        self.section_end = Some(end_offset);
        let result = func(self, end_offset);
        self.section_end = previous_bound;
        let result = result?;

        // Skip to end of section. The declared payload is bounded, but any
        // alignment padding after it is not part of the payload.
        if self.offset < end_offset {
            let remaining = (end_offset - self.offset) as usize;
            self.offset += remaining as u64;
            self.reader.seek(SeekFrom::Current(remaining as i64))?;
        }

        // Section payload is padded to alignment outside the length field.
        let padding = (round - (length % round)) % round;
        if padding != 0 {
            // Read the padding instead of seeking over it: seek permits a
            // Cursor/file to move past EOF, while read_bytes turns truncated
            // padding into the required UnexpectedEof error.
            self.read_bytes(padding)?;
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
    for (name, requested) in [
        ("log_missing_features", options.log_missing_features),
        ("log_dev_features", options.log_dev_features),
        ("debug", options.debug),
    ] {
        if requested == Some(true) {
            return Err(PsdError::UnsupportedFeature(format!(
                "ReadOptions::{} is not implemented",
                name
            )));
        }
    }
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
    psd_reader.color_mode = Some(color_mode);

    let mut psd = Psd {
        width,
        height,
        channels: Some(channels),
        bits_per_channel: Some(bits_per_channel as u8),
        color_mode: Some(color_mode),
        palette: None,
        image_data: None,
        composite_skipped: false,
        layer_image_data_skipped: false,
        linked_files_data_skipped: false,
        composite_native: None,
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
    if psd_reader.options.skip_layer_image_data.unwrap_or(false) {
        psd.layer_image_data_skipped = psd
            .children
            .as_ref()
            .map(|layers| !layers.is_empty())
            .unwrap_or(false);
    }
    if psd_reader.options.skip_linked_files_data.unwrap_or(false) {
        psd.linked_files_data_skipped = psd
            .additional_info
            .linked_files
            .as_ref()
            .map(|block| block.items.iter().any(|item| item.data.is_none()))
            .unwrap_or(false);
    }

    // Apply document resource postprocess (after layers are available)
    crate::format::document_resource_postprocess::apply_document_postprocess(&mut psd)?;

    // Read image data section
    let skip_composite = psd_reader
        .options
        .skip_composite_image_data
        .unwrap_or(false)
        || psd_reader.options.use_image_data == Some(false);
    if !skip_composite {
        read_image_data(&mut psd_reader, &mut psd)?;
    } else {
        psd.composite_skipped = true;
    }

    Ok(psd)
}

/// Read color mode data section
fn read_color_mode_data<R: Read + Seek>(reader: &mut PsdReader<R>, psd: &mut Psd) -> Result<()> {
    reader.read_section(1, false, |reader, end_offset| {
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
    reader.read_section(1, false, |reader, end_offset| {
        let remaining = reader.bytes_left(end_offset) as usize;
        if remaining > 0 {
            let resources =
                crate::format::image_resources::read_image_resources(reader, remaining)?;
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
    reader.read_section(1, reader.large, |reader, end_offset| {
        // Read layer info
        if reader.bytes_left(end_offset) > 0 {
            reader.read_section(2, reader.large, |reader, end_offset| {
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
    if blend.clipping != 0 {
        // The layer record carries its own clipping byte; it is not derived
        // from resource 1026 (which holds dragging-group IDs).
        layer.clipping = Some(blend.clipping as u16);
    }
    let blend_flags = LayerBlendFlags::from_bits_retain(blend.flags);
    layer.raw_blend_flags = Some(blend.flags);
    layer.transparency_protected =
        Some(blend_flags.contains(LayerBlendFlags::TRANSPARENCY_PROTECTED));
    layer.hidden = Some(blend_flags.contains(LayerBlendFlags::HIDDEN));

    // Read extra data
    reader.read_section(1, false, |reader, end_offset| {
        // Read layer mask data
        let channel_ids: Vec<i16> = channels.iter().map(|c| c.id.as_i16()).collect();
        read_layer_mask_data(reader, &mut layer, &channel_ids)?;

        // Read blending ranges
        let blending_len = reader.read_u32()? as usize;
        if blending_len > reader.bytes_left(end_offset) {
            return Err(PsdError::InvalidFormat(format!(
                "Layer blending ranges length {} exceeds remaining extra data {}",
                blending_len,
                reader.bytes_left(end_offset)
            )));
        }
        if blending_len > 0 {
            let bytes = reader.read_bytes(blending_len)?;
            layer.blending_ranges_data =
                Some(parse_layer_blending_ranges(&bytes).ok_or_else(|| {
                    PsdError::InvalidFormat(format!(
                        "Layer blending ranges length {} is not a multiple of 8",
                        blending_len
                    ))
                })?);
        }

        // Read layer name
        let pascal_name = reader.read_pascal_string(4)?;

        // Read tagged blocks (additional layer info)
        let remaining = reader.bytes_left(end_offset) as usize;
        if remaining > 0 {
            let existing_mask = layer.additional_info.mask.take();
            let existing_real_mask = layer.additional_info.real_mask.take();
            let mut info =
                crate::format::additional_info::read_layer_additional_info(reader, remaining)?;
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
    large: bool,
    color_mode: ColorMode,
) -> Result<Vec<Layer>> {
    let cursor = std::io::Cursor::new(bytes.to_vec());
    let mut reader = PsdReader::new(cursor, Default::default());
    reader.large = large;
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
        read_layer_channel_raw_data(
            &mut reader,
            bits_per_channel,
            color_mode,
            &mut layers[i],
            channels,
        )?;
    }
    let mut temp_psd = Psd::default();
    build_layer_hierarchy(&mut temp_psd, layers)?;
    Ok(temp_psd.children.unwrap_or_default())
}

/// Read layer mask data
fn read_layer_mask_data<R: Read + Seek>(
    reader: &mut PsdReader<R>,
    layer: &mut Layer,
    _channel_ids: &[i16],
) -> Result<()> {
    reader.read_section(1, false, |reader, end_offset| {
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

        // Parameters (if flagged) come first and must be parsed even when the
        // whole payload is compact (well under the 18 bytes of a real mask);
        // otherwise valid density/feather-only parameter sets are lost.
        if flags.contains(LayerMaskStateBits::HAS_PARAMETERS) {
            if reader.bytes_left(end_offset) < 1 {
                return Err(PsdError::InvalidFormat(
                    "Mask parameters flag set but no parameter flags present".to_string(),
                ));
            }
            let param_flags = LayerMaskParameterFlags::from_bits_retain(reader.read_u8()?);
            if param_flags.contains(LayerMaskParameterFlags::USER_MASK_DENSITY) {
                if reader.bytes_left(end_offset) < 1 {
                    return Err(PsdError::InvalidFormat(
                        "Mask user density declared but missing".to_string(),
                    ));
                }
                mask.user_mask_density = Some(reader.read_u8()? as f64);
            }
            if param_flags.contains(LayerMaskParameterFlags::USER_MASK_FEATHER) {
                if reader.bytes_left(end_offset) < 8 {
                    return Err(PsdError::InvalidFormat(
                        "Mask user feather declared but missing".to_string(),
                    ));
                }
                mask.user_mask_feather = Some(reader.read_f64()?);
            }
            if param_flags.contains(LayerMaskParameterFlags::VECTOR_MASK_DENSITY) {
                if reader.bytes_left(end_offset) < 1 {
                    return Err(PsdError::InvalidFormat(
                        "Mask vector density declared but missing".to_string(),
                    ));
                }
                mask.vector_mask_density = Some(reader.read_u8()? as f64);
            }
            if param_flags.contains(LayerMaskParameterFlags::VECTOR_MASK_FEATHER) {
                if reader.bytes_left(end_offset) < 8 {
                    return Err(PsdError::InvalidFormat(
                        "Mask vector feather declared but missing".to_string(),
                    ));
                }
                mask.vector_mask_feather = Some(reader.read_f64()?);
            }
        }

        // The optional real-mask structure follows the parameters; within
        // this bounded mask section, 18 trailing bytes are its documented
        // size, independent of how parameters were laid out.
        let remaining_after_params = reader.bytes_left(end_offset) as usize;
        if remaining_after_params >= 18 {
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

    let width = layer
        .right
        .unwrap_or(0)
        .checked_sub(layer.left.unwrap_or(0))
        .ok_or_else(|| PsdError::InvalidFormat("Layer horizontal bounds overflow".to_string()))?;
    let height = layer
        .bottom
        .unwrap_or(0)
        .checked_sub(layer.top.unwrap_or(0))
        .ok_or_else(|| PsdError::InvalidFormat("Layer vertical bounds overflow".to_string()))?;
    if width < 0 || height < 0 {
        return Err(PsdError::InvalidFormat(
            "Layer bounds are reversed".to_string(),
        ));
    }
    let width = width as usize;
    let height = height as usize;

    let expected_len = width
        .checked_mul(height)
        .ok_or_else(|| PsdError::InvalidFormat("Layer image dimensions overflow".to_string()))?;
    crate::support::limits::check_decoded_buffer(
        expected_len.checked_mul(4).unwrap_or(usize::MAX),
        "layer image data",
    )?;
    let color_mode = psd.color_mode.unwrap_or(ColorMode::RGB);
    let cmyk = color_mode == ColorMode::CMYK;
    let is_grayscale = color_mode == ColorMode::Grayscale;
    let doc_depth = psd.bits_per_channel.unwrap_or(8);
    let bytes_per_sample = match doc_depth {
        8 => 1usize,
        16 => 2,
        32 => 4,
        other => {
            return Err(PsdError::UnsupportedFeature(format!(
                "Unsupported layer bits per channel: {}",
                other
            )))
        }
    };
    // Native samples are kept when the RGBA preview cannot represent them
    // losslessly: high-bit-depth layers, non-RGB layers, or explicit request.
    let capture_native = doc_depth != 8
        || reader.options.use_raw_data.unwrap_or(false)
        || !matches!(
            color_mode,
            ColorMode::RGB | ColorMode::Grayscale | ColorMode::Bitmap | ColorMode::Indexed
        );
    let mut native_channels: Vec<LayerRawDataChannel> = Vec::new();
    let mut red: Option<Vec<u8>> = None;
    let mut green: Option<Vec<u8>> = None;
    let mut blue: Option<Vec<u8>> = None;
    let mut black: Option<Vec<u8>> = None;
    let mut alpha: Option<Vec<u8>> = None;
    let mut transparency: Option<Vec<u8>> = None;
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
        let channel_samples = channel_width.checked_mul(channel_height).ok_or_else(|| {
            PsdError::InvalidFormat("Layer channel dimensions overflow".to_string())
        })?;
        let channel_expected_len =
            channel_samples
                .checked_mul(bytes_per_sample)
                .ok_or_else(|| {
                    PsdError::InvalidFormat("Layer channel byte size overflow".to_string())
                })?;
        crate::support::limits::check_decoded_buffer(channel_expected_len, "layer channel data")?;

        let decoded = match compression {
            Compression::RawData => {
                let data = reader.read_bytes(data_length)?;
                if reader.strict_enabled() && data.len() != channel_expected_len {
                    return Err(PsdError::InvalidFormat(format!(
                        "Strict parse: raw channel has {} bytes, expected {}",
                        data.len(),
                        channel_expected_len
                    )));
                }
                Ok(normalize_channel_data(data, channel_expected_len))
            }
            Compression::RleCompressed => {
                let row_count = channel_height;
                let byte_count_width = if reader.large { 4 } else { 2 };
                let byte_counts_len = row_count.checked_mul(byte_count_width).ok_or_else(|| {
                    PsdError::InvalidFormat("Layer RLE count table overflow".to_string())
                })?;
                if data_length < byte_counts_len {
                    return Err(PsdError::InvalidFormat(
                        "Invalid RLE channel data length".to_string(),
                    ));
                }
                let mut byte_counts = Vec::new();
                for _ in 0..row_count {
                    let v = if reader.large {
                        reader.read_u32()?
                    } else {
                        reader.read_u16()? as u32
                    };
                    byte_counts.push(v);
                }
                let compressed_len = data_length - byte_counts_len;
                let compressed = reader.read_bytes(compressed_len)?;
                let mut out = vec![0u8; channel_expected_len];
                compression::decompress_rle(
                    &compressed,
                    &mut out,
                    channel_width * bytes_per_sample,
                    channel_height,
                    &byte_counts,
                )?;
                Ok(out)
            }
            Compression::ZipWithoutPrediction => {
                let compressed = reader.read_bytes(data_length)?;
                compression::decompress_zip(&compressed, channel_expected_len)
            }
            Compression::ZipWithPrediction => {
                let compressed = reader.read_bytes(data_length)?;
                compression::decompress_zip_with_prediction(
                    &compressed,
                    channel_width,
                    channel_height,
                    doc_depth as u16,
                )
            }
        };
        let decoded = decoded?;
        if capture_native {
            native_channels.push(LayerRawDataChannel {
                id: channel.id,
                compression,
                data: Some(decoded.clone()),
            });
        }
        let offset = channel_offset(channel.id, cmyk);
        match offset {
            0 => red = Some(decoded),
            1 => green = Some(decoded),
            2 => blue = Some(decoded),
            3 if cmyk => black = Some(decoded),
            3 => alpha = Some(decoded),
            4 if cmyk => transparency = Some(decoded),
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

    if capture_native {
        layer.raw_data = Some(LayerRawData {
            color_mode,
            bits_per_channel: doc_depth,
            channels: native_channels,
            large: reader.large,
            preview: None,
        });
    }

    let mut rgba = vec![0u8; expected_len * 4];
    for i in 0..expected_len {
        if cmyk {
            let c = red
                .as_ref()
                .map(|d| sample_to_u8(d, i, doc_depth as u16))
                .unwrap_or(0) as u16;
            let m = green
                .as_ref()
                .map(|d| sample_to_u8(d, i, doc_depth as u16))
                .unwrap_or(0) as u16;
            let y = blue
                .as_ref()
                .map(|d| sample_to_u8(d, i, doc_depth as u16))
                .unwrap_or(0) as u16;
            let k = black
                .as_ref()
                .map(|d| sample_to_u8(d, i, doc_depth as u16))
                .unwrap_or(0) as u16;
            rgba[i * 4] = ((255 * (255 - c) * (255 - k)) / (255 * 255)) as u8;
            rgba[i * 4 + 1] = ((255 * (255 - m) * (255 - k)) / (255 * 255)) as u8;
            rgba[i * 4 + 2] = ((255 * (255 - y) * (255 - k)) / (255 * 255)) as u8;
            rgba[i * 4 + 3] = transparency
                .as_ref()
                .map(|d| sample_to_u8(d, i, doc_depth as u16))
                .unwrap_or(255);
        } else {
            rgba[i * 4] = red
                .as_ref()
                .map(|d| sample_to_u8(d, i, doc_depth as u16))
                .unwrap_or(0);
            rgba[i * 4 + 1] = green
                .as_ref()
                .map(|d| sample_to_u8(d, i, doc_depth as u16))
                .unwrap_or(0);
            rgba[i * 4 + 2] = blue
                .as_ref()
                .map(|d| sample_to_u8(d, i, doc_depth as u16))
                .unwrap_or(0);
            rgba[i * 4 + 3] = alpha
                .as_ref()
                .map(|d| sample_to_u8(d, i, doc_depth as u16))
                .unwrap_or(255);
        }
    }

    let mut pixel_data = PixelData {
        data: rgba,
        width,
        height,
    };
    if is_grayscale {
        setup_grayscale(&mut pixel_data);
    }
    // Masks are preserved as independent channels (attached below to
    // additional_info.mask/real_mask.image_data). Parsing never bakes them
    // into the layer's alpha, and a disabled or partially covering mask must
    // not alter the underlying pixel samples.

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

    if expected_len > 0 {
        layer.image_data = Some(pixel_data);
    }
    // The raw channels captured above are only reusable while this preview is
    // unchanged; snapshot it so the writer can honor later edits.
    if let Some(raw) = layer.raw_data.as_mut() {
        raw.preview = layer.image_data.clone();
    }
    Ok(())
}

fn read_layer_channel_raw_data<R: Read + Seek>(
    reader: &mut PsdReader<R>,
    bits_per_channel: u8,
    color_mode: ColorMode,
    layer: &mut Layer,
    channels: &[ChannelInfo],
) -> Result<()> {
    let bytes_per_sample = match bits_per_channel {
        8 => 1usize,
        16 => 2,
        32 => 4,
        _ => {
            return Err(PsdError::UnsupportedFeature(format!(
                "Unsupported layer bits per channel: {}",
                bits_per_channel
            )))
        }
    };
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
        let expected_len = channel_width
            .checked_mul(channel_height)
            .and_then(|v| v.checked_mul(bytes_per_sample))
            .ok_or_else(|| {
                PsdError::InvalidFormat("Layer channel dimensions overflow".to_string())
            })?;
        crate::support::limits::check_decoded_buffer(expected_len, "layer raw channel data")?;
        let decoded = match compression {
            Compression::RawData => reader.read_bytes(data_length)?,
            Compression::RleCompressed => {
                let row_count = channel_height;
                let byte_count_width = if reader.large { 4 } else { 2 };
                let byte_counts_len = row_count.checked_mul(byte_count_width).ok_or_else(|| {
                    PsdError::InvalidFormat("Layer RLE count table overflow".to_string())
                })?;
                if data_length < byte_counts_len {
                    return Err(PsdError::InvalidFormat(
                        "Invalid RLE channel data length".to_string(),
                    ));
                }
                let mut byte_counts = Vec::new();
                for _ in 0..row_count {
                    let v = if reader.large {
                        reader.read_u32()?
                    } else {
                        reader.read_u16()? as u32
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
        color_mode,
        bits_per_channel,
        channels: raw_channels,
        large: reader.large,
        preview: None,
    });
    Ok(())
}

/// Build layer hierarchy from flat layer list.
///
/// The folder start marker (open/closed divider) is the group's public node:
/// it carries the group name, opacity, visibility, blend mode, masks and
/// effects. Children are attached to it when its bounding closing marker is
/// reached, and empty groups keep `Some(vec![])`.
fn build_layer_hierarchy(psd: &mut Psd, layers: Vec<Layer>) -> Result<()> {
    // Bottom of stack is the root; each folder frame holds the marker record
    // that will become the public group node plus its accumulated children.
    let mut stack: Vec<(Option<Layer>, Vec<Layer>)> = vec![(None, Vec::new())];

    for mut layer in layers.into_iter().rev() {
        let section_type = layer
            .additional_info
            .section_divider
            .as_ref()
            .map(|sd| sd.divider_type)
            .unwrap_or(SectionDividerType::Other);

        match section_type {
            SectionDividerType::BoundingSectionDivider => {
                if stack.len() <= 1 {
                    return Err(PsdError::InvalidFormat(
                        "Bounding section divider without an open folder".to_string(),
                    ));
                }
                let (folder, mut children) = stack.pop().unwrap();
                // Records arrived reversed; restore their original order once.
                children.reverse();
                let mut group = folder.ok_or_else(|| {
                    PsdError::InvalidFormat("Empty folder frame in hierarchy".to_string())
                })?;
                group.children = Some(children);
                group.opened = Some(matches!(
                    group
                        .additional_info
                        .section_divider
                        .as_ref()
                        .map(|sd| sd.divider_type)
                        .unwrap_or(SectionDividerType::Other),
                    SectionDividerType::OpenFolder
                ));
                stack.last_mut().unwrap().1.push(group);
            }
            SectionDividerType::OpenFolder | SectionDividerType::ClosedFolder => {
                // The marker record is the group; children accumulate on top.
                layer.opened = Some(matches!(section_type, SectionDividerType::OpenFolder));
                stack.push((Some(layer), Vec::new()));
            }
            SectionDividerType::Other => {
                stack.last_mut().unwrap().1.push(layer);
            }
        }
    }

    if stack.len() != 1 {
        return Err(PsdError::InvalidFormat(format!(
            "Unbalanced folder markers: {} folder(s) never closed",
            stack.len() - 1
        )));
    }
    let mut root = stack.pop().map(|(_, layers)| layers).unwrap_or_default();
    root.reverse();
    psd.children = Some(root);
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
                return (left, top, extent(left, right), extent(top, bottom));
            }
        }
        ChannelID::RealUserMask => {
            if let Some(mask) = layer.additional_info.real_mask.as_ref() {
                let left = mask.left.unwrap_or(layer_left);
                let top = mask.top.unwrap_or(layer_top);
                let right = mask.right.unwrap_or(left);
                let bottom = mask.bottom.unwrap_or(top);
                return (left, top, extent(left, right), extent(top, bottom));
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
        layer_right
            .checked_sub(layer_left)
            .filter(|value| *value >= 0)
            .unwrap_or(0) as usize,
        layer_bottom
            .checked_sub(layer_top)
            .filter(|value| *value >= 0)
            .unwrap_or(0) as usize,
    )
}

fn extent(start: i32, end: i32) -> usize {
    end.checked_sub(start)
        .filter(|value| *value >= 0)
        .unwrap_or(0) as usize
}

/// Read global layer mask info
fn read_global_layer_mask_info<R: Read + Seek>(
    reader: &mut PsdReader<R>,
) -> Result<Option<GlobalLayerMaskInfo>> {
    reader.read_section(1, false, |reader, end_offset| {
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
    crate::support::limits::check_decoded_buffer(
        channel_len.checked_mul(4).unwrap_or(usize::MAX),
        "composite image data",
    )?;
    let base_channels = match color_mode {
        ColorMode::Grayscale => 1usize,
        ColorMode::CMYK => 4usize,
        _ => 3usize,
    };
    let mut total_channels = psd.channels.unwrap_or(base_channels as u16) as usize;
    if total_channels == 0 {
        total_channels = base_channels;
    }
    // The header channel count is authoritative. The negative layer-count
    // transparency marker describes layer data; it does not add a composite
    // plane to the image-data section.
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
    let channel_len_bytes = channel_len
        .checked_mul(bytes_per_sample)
        .ok_or_else(|| PsdError::InvalidFormat("Composite channel size overflow".to_string()))?;
    crate::support::limits::check_decoded_buffer(channel_len_bytes, "composite channel")?;

    let mut planes: Vec<Vec<u8>> = vec![Vec::new(); total_channels];
    match compression {
        Compression::RawData => {
            for i in 0..total_channels {
                let plane = reader.read_bytes(channel_len_bytes)?;
                planes[i] = normalize_channel_data(plane, channel_len_bytes);
            }
        }
        Compression::RleCompressed => {
            let row_count = total_channels.checked_mul(height).ok_or_else(|| {
                PsdError::InvalidFormat("Composite RLE row count overflow".to_string())
            })?;
            let mut byte_counts = Vec::new();
            for _ in 0..row_count {
                let v = if reader.large {
                    reader.read_u32()?
                } else {
                    reader.read_u16()? as u32
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
            if compression == Compression::ZipWithPrediction {
                compression::reverse_prediction_planar(
                    &mut data,
                    width,
                    height,
                    total_channels,
                    bits_per_channel,
                )?;
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
                    4 => rgba[i * 4 + 3] = value,
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
            let c = sample_to_u8(
                planes.get(0).map(Vec::as_slice).unwrap_or(&[]),
                i,
                bits_per_channel,
            ) as u32;
            let m = sample_to_u8(
                planes.get(1).map(Vec::as_slice).unwrap_or(&[]),
                i,
                bits_per_channel,
            ) as u32;
            let y = sample_to_u8(
                planes.get(2).map(Vec::as_slice).unwrap_or(&[]),
                i,
                bits_per_channel,
            ) as u32;
            let k = sample_to_u8(
                planes.get(3).map(Vec::as_slice).unwrap_or(&[]),
                i,
                bits_per_channel,
            ) as u32;
            rgba[i * 4] = ((255 * (255 - c) * (255 - k)) / (255 * 255)) as u8;
            rgba[i * 4 + 1] = ((255 * (255 - m) * (255 - k)) / (255 * 255)) as u8;
            rgba[i * 4 + 2] = ((255 * (255 - y) * (255 - k)) / (255 * 255)) as u8;
            if total_channels <= 4 {
                rgba[i * 4 + 3] = 255;
            }
        } else if match color_mode {
            // Force-opaque only when no real transparency plane was decoded:
            // Grayscale has an alpha plane from channel 2 onward, CMYK from
            // channel 5 onward, other modes from channel 4 onward.
            ColorMode::Grayscale => total_channels <= 1,
            ColorMode::CMYK => total_channels <= 4,
            _ => total_channels <= 3,
        } {
            rgba[i * 4 + 3] = 255;
        }
    }
    if color_mode == ColorMode::Indexed {
        // Composite indices resolve through the color palette instead of
        // being treated as an RGB component.
        let indices = planes.first().map(|p| p.as_slice()).unwrap_or(&[]);
        let palette = psd.palette.as_deref().ok_or_else(|| {
            PsdError::InvalidFormat(
                "Indexed composite is missing its 256-entry palette".to_string(),
            )
        })?;
        if palette.is_empty() {
            return Err(PsdError::InvalidFormat(
                "Indexed composite has an empty palette".to_string(),
            ));
        }
        for i in 0..channel_len {
            let idx = indices
                .get(i)
                .copied()
                .unwrap_or(0)
                .min(palette.len().saturating_sub(1) as u8);
            let entry = &palette[idx as usize];
            rgba[i * 4] = entry.r;
            rgba[i * 4 + 1] = entry.g;
            rgba[i * 4 + 2] = entry.b;
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

    // Keep the original native planes for byte-identical unchanged resaves.
    let preview = psd
        .image_data
        .as_ref()
        .map(|p| p.data.clone())
        .unwrap_or_default();
    psd.composite_native = Some(crate::api::psd::CompositeNativeData {
        bits_per_channel,
        color_mode,
        channels: planes,
        preview,
    });

    Ok(())
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
        // Extra/saved channels are not color planes; keep them off the RGBA
        // offsets so they cannot overwrite a valid channel.
        ChannelID::Other(_) => -2,
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
    // Each range pair is 8 bytes: four u16 (big-endian) endpoints covering
    // source black/white and destination black/white.
    if bytes.len() % 8 != 0 {
        return None;
    }
    let mut offset = 0;
    let mut read_pair = || -> Option<crate::api::layer::LayerBlendingRangePair> {
        if offset + 8 > bytes.len() {
            return None;
        }
        let u16_at = |i: usize| u16::from_be_bytes([bytes[i], bytes[i + 1]]);
        let pair = crate::api::layer::LayerBlendingRangePair {
            src_black: u16_at(offset),
            src_white: u16_at(offset + 2),
            dst_black: u16_at(offset + 4),
            dst_white: u16_at(offset + 6),
        };
        offset += 8;
        Some(pair)
    };

    let composite_gray = read_pair();
    let mut channels = Vec::new();
    while let Some(pair) = read_pair() {
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
                PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                    .join("tests/fixtures/samples")
                    .canonicalize()
                    .unwrap_or_else(|_| {
                        std::env::current_dir()
                            .unwrap()
                            .join("../photoshop/psd/samples")
                            .canonicalize()
                            .unwrap_or_else(|_| {
                                std::env::current_dir()
                                    .unwrap()
                                    .join("tests/fixtures/samples")
                                    .canonicalize()
                                    .unwrap()
                            })
                    })
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

        reader.read_section(1, reader.large, |reader, end_offset| {
            let mut flat_layers = Vec::new();
            if reader.bytes_left(end_offset) > 0 {
                reader.read_section(2, reader.large, |reader, end_offset| {
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
            .read_section(2, false, |r, _| r.read_u8())
            .expect("read section");
        assert_eq!(payload, 0xAA);
        let next = reader.read_u8().expect("next byte");
        assert_eq!(next, 0xBB);
    }

    #[test]
    fn truncated_section_padding_is_rejected() {
        let mut reader =
            PsdReader::new(Cursor::new(vec![0, 0, 0, 1, 0xAA]), ReadOptions::default());
        let result = reader.read_section(2, false, |reader, _| reader.read_u8());
        assert!(result.is_err(), "missing alignment padding must fail");
    }

    #[test]
    fn test_section_handler_overread_is_rejected() {
        // One-byte section whose handler reads a u16 must error instead of
        // silently consuming the following byte.
        let data = vec![
            0x00, 0x00, 0x00, 0x01, // section length = 1
            0x07, // payload byte
            0x08, // sibling byte that must not be consumed
        ];
        let mut reader = PsdReader::new(Cursor::new(data), ReadOptions::default());
        let err = reader
            .read_section(1, false, |r, _| r.read_u16())
            .unwrap_err();
        assert!(
            err.to_string().contains("overread"),
            "unexpected error: {}",
            err
        );
        // The failed read consumed nothing: the payload byte is still there.
        assert_eq!(reader.read_u8().unwrap(), 0x07);
    }

    #[test]
    fn test_section_declared_payload_past_eof_is_rejected() {
        // Declared 100-byte payload with no payload bytes present must error
        // even when the callback reads nothing.
        let data = vec![
            0x00, 0x00, 0x00, 0x64, // section length = 100
        ];
        let mut reader = PsdReader::new(Cursor::new(data), ReadOptions::default());
        let err = reader.read_section(1, false, |_, _| Ok(())).unwrap_err();
        assert!(
            err.to_string().contains("exceeds available input"),
            "unexpected error: {}",
            err
        );
    }

    #[test]
    fn test_nested_section_bounds_apply_to_inner_handlers() {
        // Outer section length 6; inner section claims 4 but only 2 bytes are
        // left within the outer payload. The inner declared payload is valid
        // against the physical input, so reading it must fail inside the
        // bounded read instead of consuming outer-payload siblings.
        let data = vec![
            0x00, 0x00, 0x00, 0x06, // outer length = 6
            0x00, 0x00, 0x00, 0x05, // inner length = 5 (exceeds outer payload)
            0x01, 0x02, // only two payload bytes follow
        ];
        let mut reader = PsdReader::new(Cursor::new(data), ReadOptions::default());
        let err = reader
            .read_section(1, false, |reader, end_offset| {
                if reader.bytes_left(end_offset) > 0 {
                    reader.read_section(1, false, |r, _| r.read_u8())?;
                }
                Ok(())
            })
            .unwrap_err();
        assert!(
            err.to_string().contains("overread")
                || err.to_string().contains("exceeds available input"),
            "unexpected error: {}",
            err
        );
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
    fn test_build_layer_hierarchy_keeps_folder_marker_as_group() {
        let mut psd = Psd::default();
        let marker = Layer {
            opacity: Some(0.25),
            hidden: Some(true),
            additional_info: crate::format::additional_info::LayerAdditionalInfo {
                name: Some("Group".to_string()),
                section_divider: Some(crate::format::additional_info::SectionDivider {
                    divider_type: SectionDividerType::ClosedFolder,
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
        let bounding = Layer {
            additional_info: crate::format::additional_info::LayerAdditionalInfo {
                section_divider: Some(crate::format::additional_info::SectionDivider {
                    divider_type: SectionDividerType::BoundingSectionDivider,
                    blend_mode: None,
                    sub_type: None,
                }),
                ..Default::default()
            },
            ..Default::default()
        };

        build_layer_hierarchy(&mut psd, vec![bounding, leaf, marker]).expect("build hierarchy");

        let roots = psd.children.expect("root layers");
        assert_eq!(roots.len(), 1);
        // The folder marker's own properties survive as the public group.
        assert_eq!(roots[0].additional_info.name.as_deref(), Some("Group"));
        assert_eq!(roots[0].opacity, Some(0.25));
        assert_eq!(roots[0].hidden, Some(true));
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
    fn test_build_layer_hierarchy_preserves_empty_group() {
        let mut psd = Psd::default();
        let marker = Layer {
            additional_info: crate::format::additional_info::LayerAdditionalInfo {
                name: Some("Empty".to_string()),
                section_divider: Some(crate::format::additional_info::SectionDivider {
                    divider_type: SectionDividerType::OpenFolder,
                    blend_mode: None,
                    sub_type: None,
                }),
                ..Default::default()
            },
            ..Default::default()
        };
        let bounding = Layer {
            additional_info: crate::format::additional_info::LayerAdditionalInfo {
                section_divider: Some(crate::format::additional_info::SectionDivider {
                    divider_type: SectionDividerType::BoundingSectionDivider,
                    blend_mode: None,
                    sub_type: None,
                }),
                ..Default::default()
            },
            ..Default::default()
        };

        build_layer_hierarchy(&mut psd, vec![bounding, marker]).expect("build hierarchy");

        let roots = psd.children.expect("root layers");
        assert_eq!(roots.len(), 1);
        // Empty groups stay groups (Some(vec![])), so writes recognize them.
        assert_eq!(roots[0].children.as_ref(), Some(&Vec::new()));
        assert_eq!(roots[0].opened, Some(true));
    }

    #[test]
    fn test_build_layer_hierarchy_rejects_unbalanced_markers() {
        let mut psd = Psd::default();
        let marker = Layer {
            additional_info: crate::format::additional_info::LayerAdditionalInfo {
                name: Some("Unclosed".to_string()),
                section_divider: Some(crate::format::additional_info::SectionDivider {
                    divider_type: SectionDividerType::OpenFolder,
                    blend_mode: None,
                    sub_type: None,
                }),
                ..Default::default()
            },
            ..Default::default()
        };
        let err = build_layer_hierarchy(&mut psd, vec![marker]).unwrap_err();
        assert!(
            err.to_string().contains("never closed"),
            "unexpected error: {}",
            err
        );
    }

    #[test]
    fn strict_mode_rejects_short_raw_channel() {
        // One 1x1 color channel, compression = raw (0), zero payload bytes.
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&0u16.to_be_bytes()); // compression: raw
                                                      // No payload: the channel claims one pixel but carries no bytes.

        let mut lenient_reader = PsdReader::new(Cursor::new(bytes.clone()), ReadOptions::default());
        let lenient_psd = Psd {
            color_mode: Some(ColorMode::RGB),
            bits_per_channel: Some(8),
            ..Default::default()
        };
        let mut lenient_layer = Layer {
            left: Some(0),
            top: Some(0),
            right: Some(1),
            bottom: Some(1),
            ..Default::default()
        };
        read_layer_channel_image_data(
            &mut lenient_reader,
            &lenient_psd,
            &mut lenient_layer,
            &[ChannelInfo {
                id: ChannelID::Color0,
                length: 2,
            }],
        )
        .expect("lenient read pads the missing byte");

        let mut strict_reader = PsdReader::new(
            Cursor::new(bytes),
            ReadOptions {
                strict: Some(true),
                ..Default::default()
            },
        );
        let strict_psd = Psd {
            color_mode: Some(ColorMode::RGB),
            bits_per_channel: Some(8),
            ..Default::default()
        };
        let mut strict_layer = Layer {
            left: Some(0),
            top: Some(0),
            right: Some(1),
            bottom: Some(1),
            ..Default::default()
        };
        let err = read_layer_channel_image_data(
            &mut strict_reader,
            &strict_psd,
            &mut strict_layer,
            &[ChannelInfo {
                id: ChannelID::Color0,
                length: 2,
            }],
        )
        .unwrap_err();
        assert!(
            err.to_string().contains("Strict parse"),
            "unexpected error: {}",
            err
        );
    }

    #[test]
    fn test_layer_user_mask_preserved_independently_of_alpha() {
        let mut bytes = Vec::new();
        for data in [
            &[10u8, 20][..],
            &[0u8, 0][..],
            &[0u8, 0][..],
            &[255u8, 255][..],
        ] {
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

        read_layer_channel_image_data(&mut reader, &psd, &mut layer, &channels)
            .expect("read channels");

        let pixels = layer.image_data.expect("layer image").data;
        // Underlying alpha survives unmasked; the mask is stored separately.
        assert_eq!(pixels, vec![10, 0, 0, 255, 20, 0, 0, 255]);

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

        read_layer_channel_image_data(&mut reader, &psd, &mut layer, &channels)
            .expect("read channels");

        let pixels = layer.image_data.expect("layer image").data;
        // Neither mask modulates preview alpha during parsing.
        assert_eq!(pixels, vec![10, 0, 0, 255, 20, 0, 0, 255]);

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

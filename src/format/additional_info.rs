//! Additional layer information handlers
//!
//! Handles layer-specific additional information sections like text layers,
//! vector masks, layer effects, smart objects, and other layer properties.

use crate::api::adjustments::AdjustmentLayer;
use crate::support::binrw_support::{
    decode_be, encode_be, AnnotationHeaderRecord, LayerColorRecord, NameSourceRecord,
    ProtectedFlagsRecord, SectionDividerBaseRecord, SectionDividerExtendedRecord,
    U32ValueRecord, U8BoolRecord, UsingAlignedRenderingRecord,
};
use crate::support::compression;
use crate::support::descriptor::{Descriptor, DescriptorValue};
use crate::support::error::{PsdError, Result};
/// Read a pascal-style string with 4-byte length prefix (matching TS readPascalStringWithPadding).
fn read_pascal_string_with_padding<R: Read + std::io::Seek>(
    reader: &mut crate::io::reader::PsdReader<R>,
) -> crate::support::error::Result<String> {
    let length_plus_one = reader.read_u32()? as usize;
    if length_plus_one == 0 {
        return Ok(String::new());
    }
    let length = length_plus_one - 1;
    let bytes = reader.read_bytes(length)?;
    let _null = reader.read_u8()?;
    Ok(String::from_utf8_lossy(&bytes).to_string())
}

/// Write a pascal-style string with 4-byte length prefix (matching TS writePascalStringWithPadding).
fn write_pascal_string_with_padding(
    writer: &mut crate::io::writer::PsdWriter,
    text: &str,
) -> crate::support::error::Result<()> {
    let bytes = text.as_bytes();
    writer.write_u32((bytes.len() + 1) as u32)?;
    writer.write_bytes(bytes)?;
    writer.write_u8(0)?;
    Ok(())
}

fn read_u64_parts<R: Read + Seek>(reader: &mut PsdReader<R>) -> Result<u64> {
    let high = reader.read_u32()? as u64;
    let low = reader.read_u32()? as u64;
    Ok((high << 32) | low)
}

fn write_u64_parts(writer: &mut PsdWriter, value: u64) -> Result<()> {
    writer.write_u32((value >> 32) as u32)?;
    writer.write_u32(value as u32)?;
    Ok(())
}

fn format_linked_file_time(
    year: u32,
    month: u8,
    day: u8,
    hour: u8,
    minute: u8,
    second: f64,
) -> String {
    let second_text = if (second.fract()).abs() < f64::EPSILON {
        format!("{:02}", second as u32)
    } else {
        let mut text = format!("{second:.6}");
        while text.contains('.') && text.ends_with('0') {
            text.pop();
        }
        if text.ends_with('.') {
            text.pop();
        }
        if second < 10.0 && !text.starts_with('0') {
            text.insert(0, '0');
        }
        text
    };
    format!("{year:04}-{month:02}-{day:02}T{hour:02}:{minute:02}:{second_text}Z")
}

fn parse_linked_file_time(text: &str) -> Option<(u32, u8, u8, u8, u8, f64)> {
    let trimmed = text.strip_suffix('Z').unwrap_or(text);
    let (date, time) = trimmed.split_once('T')?;
    let mut date_parts = date.split('-');
    let year = date_parts.next()?.parse().ok()?;
    let month = date_parts.next()?.parse().ok()?;
    let day = date_parts.next()?.parse().ok()?;
    let mut time_parts = time.split(':');
    let hour = time_parts.next()?.parse().ok()?;
    let minute = time_parts.next()?.parse().ok()?;
    let second = time_parts.next()?.parse().ok()?;
    Some((year, month, day, hour, minute, second))
}

fn linked_file_uses_versioned_paths(info: &LinkedFileInfo) -> bool {
    !info.name.is_empty()
        || !info.full_path.is_empty()
        || !info.original_path.is_empty()
        || !info.relative_path.is_empty()
}

fn validate_linked_file_item(item: &LinkedFile) -> Result<()> {
    let version = item.item_version.unwrap_or(7);
    let kind = item
        .data_kind
        .unwrap_or(LinkedFileDataKind::Data);

    if kind == LinkedFileDataKind::External {
        if item.descriptor.is_none() {
            return Err(PsdError::InvalidFormat(
                "liFE items require a descriptor".to_string(),
            ));
        }
        if version > 3 && item.time.is_none() {
            return Err(PsdError::InvalidFormat(
                "liFE items with item_version > 3 require time metadata".to_string(),
            ));
        }
        if item.linked_file.is_none() {
            return Err(PsdError::InvalidFormat(
                "liFE items require linked file metadata".to_string(),
            ));
        }
    }

    if item.child_document_id.is_some() && version < 5 {
        return Err(PsdError::InvalidFormat(
            "linked file child_document_id requires item_version >= 5".to_string(),
        ));
    }
    if item.asset_mod_time.is_some() && version < 6 {
        return Err(PsdError::InvalidFormat(
            "linked file asset_mod_time requires item_version >= 6".to_string(),
        ));
    }
    if item.asset_locked_state.is_some() && version < 7 {
        return Err(PsdError::InvalidFormat(
            "linked file asset_locked_state requires item_version >= 7".to_string(),
        ));
    }
    if item.descriptor.is_some() && kind != LinkedFileDataKind::External {
        return Err(PsdError::InvalidFormat(
            "linked file descriptor is only valid for liFE items".to_string(),
        ));
    }
    if item.time.is_some() && !(kind == LinkedFileDataKind::External && version > 3) {
        return Err(PsdError::InvalidFormat(
            "linked file time requires liFE item_version > 3".to_string(),
        ));
    }
    if let Some(linked_file) = item.linked_file.as_ref() {
        if kind != LinkedFileDataKind::External {
            return Err(PsdError::InvalidFormat(
                "linked file info is only valid for liFE items".to_string(),
            ));
        }
        if version < 7 && linked_file_uses_versioned_paths(linked_file) {
            return Err(PsdError::InvalidFormat(
                "linked file paths require liFE item_version >= 7".to_string(),
            ));
        }
    }
    if kind == LinkedFileDataKind::Alias
        && (item.descriptor.is_some() || item.time.is_some() || item.linked_file.is_some())
    {
        return Err(PsdError::InvalidFormat(
            "linked file alias items only support alias padding, payload, and versioned tail fields".to_string(),
        ));
    }
    if let Some(time) = item.time.as_deref() {
        if parse_linked_file_time(time).is_none() {
            return Err(PsdError::InvalidFormat(
                "linked file time must be ISO-like UTC text".to_string(),
            ));
        }
    }

    Ok(())
}
use crate::support::helpers::{from_blend_mode, to_blend_mode, ProtectedFlagsBits, VectorMaskFlagsBits};
use crate::api::layer::{
    KeyDescriptorItem, Layer, LinkedFile, LinkedFileInfo, RRectRadii, VectorOrigination,
};
use crate::io::reader::PsdReader;
use crate::api::text::UnitsBounds;
use crate::api::types::Color;
use crate::api::types::{
    BlendMode, LinkedFileDataKind, PixelData, Point, PsdIntCode, PsdStringCode, PsdU32Code,
    SectionDividerType, Units, UnitsValue, RGB,
};
use crate::io::writer::PsdWriter;
use std::collections::HashMap;
use std::io::{Read, Seek};

fn palette_colors_to_bytes(colors: &[RGB]) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(colors.len() * 3);
    for color in colors {
        bytes.push(color.r);
        bytes.push(color.g);
        bytes.push(color.b);
    }
    bytes
}

/// Layer additional information
#[derive(Debug, Clone, PartialEq, Default)]
pub struct LayerAdditionalInfo {
    /// Layer name
    pub name: Option<String>,
    /// Layer ID
    pub id: Option<i32>,
    /// Layer mask data
    pub mask: Option<crate::api::layer::LayerMaskData>,
    /// Real layer mask data
    pub real_mask: Option<crate::api::layer::LayerMaskData>,
    /// Layer color
    pub layer_color: Option<crate::api::types::LayerColor>,
    /// Section divider (layer group)
    pub section_divider: Option<SectionDivider>,
    /// Blend clipped elements
    pub blend_clipped_elements: Option<bool>,
    /// Blend interior elements
    pub blend_interior_elements: Option<bool>,
    /// Knockout mode
    pub knockout: Option<bool>,
    /// Protected flags
    pub protected: Option<ProtectedFlags>,
    /// Layer name source
    pub name_source: Option<String>,
    /// Text layer data
    pub text: Option<TextLayerData>,
    /// Vector fill data
    pub vector_fill: Option<VectorFill>,
    /// Vector stroke data
    pub vector_stroke: Option<VectorStroke>,
    /// Vector mask data
    pub vector_mask: Option<VectorMask>,
    /// Layer effects
    pub effects: Option<LayerEffects>,
    /// Placed layer (smart object)
    pub placed_layer: Option<PlacedLayer>,
    /// Artboard data
    pub artboard: Option<ArtboardData>,
    /// Using aligned rendering
    pub using_aligned_rendering: Option<bool>,
    /// Metadata
    pub metadata: Option<Metadata>,
    /// Adjustment layer data (brit/levl/curv/expA/blnc/phfl/hue2/selc/mixr/post/thrs/nvrt/grdm/blwh)
    pub adjustment: Option<AdjustmentLayer>,
    /// Fill opacity (iOpa)
    pub fill_opacity: Option<u8>,
    /// Layer mask as global mask (lmgm)
    pub lmgm: Option<u8>,
    /// Vector mask as global mask (vmgm)
    pub vmgm: Option<u8>,
    /// Filter CMYK flag (fcmy)
    pub fcmy: Option<u8>,
    /// Layer version (lyvr)
    pub layer_version: Option<u32>,
    /// Channel blending restrictions (brst)
    pub blending_restrictions: Option<[u8; 3]>,
    /// Reference point (fxrp)
    pub reference_point: Option<Point>,
    /// Filter mask tagged-block payload (FMsk)
    pub filter_mask: Option<FilterMaskPayload>,
    /// Shape pattern status (shpa)
    pub shape_pattern: Option<ShapePatternStatus>,
    /// Typed Lr16 or Lr32 nested high-bit-depth layer section
    pub high_depth_layer_data: Option<HighDepthLayerInfo>,
    /// Linked smart object file block
    pub linked_files: Option<LinkedFilesBlock>,
    /// Vector origination data
    pub vector_origination: Option<VectorOrigination>,
    /// Layer effects descriptor (lmfx/lfxs blocks)
    pub layer_effects_descriptor: Option<Descriptor>,
    /// Additional descriptor-backed blocks keyed by tagged-block name.
    pub descriptor_blocks: HashMap<String, Descriptor>,
    /// Pattern block data (Patt/Pat2/Pat3)
    pub pattern_data: Option<PatternBlock>,
    /// Text engine data (Txt2) - engine_data payload
    pub text_engine: Option<TextEngineBlock>,
    /// Annotation items (Anno)
    pub annotations: Option<Vec<AnnotationItem>>,
    /// Filter effects (FEid)
    pub filter_effects: Option<FilterEffectsBlock>,
    /// Pixel source data (PxSD)
    pub pixel_source_data: Option<PixelSourceDataBlock>,
    /// Original tagged-block order from the source PSD, including duplicates.
    pub tagged_block_order: Vec<String>,
    /// Opaque tagged blocks that are preserved verbatim because they are not semantically modeled.
    pub raw_blocks: Vec<RawTaggedBlock>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct RawTaggedBlock {
    pub key: String,
    pub data: Vec<u8>,
}

/// Text engine block (Txt2) containing engine data
#[derive(Debug, Clone, PartialEq)]
pub struct TextEngineBlock {
    pub data: crate::support::engine_data::EngineValue,
    pub raw: Option<Vec<u8>>,
}

/// Annotation item (Anno)
#[derive(Debug, Clone, PartialEq)]
pub struct AnnotationItem {
    pub x: i32,
    pub y: i32,
    pub color_l: u16,
    pub color_o: u16,
    pub color_c: u16,
    pub author: String,
    pub text: String,
}

/// Filter effects block (FEid)
#[derive(Debug, Clone, PartialEq)]
pub struct FilterEffectsBlock {
    pub version: u32,
    pub items: Vec<FilterEffectsItem>,
}

/// Filter effects item
#[derive(Debug, Clone, PartialEq)]
pub struct FilterEffectsItem {
    pub id: String,
    pub version: Option<u32>,
    pub rect: Option<FilterEffectsRect>,
    pub depth: Option<u32>,
    pub channel_count: Option<u32>,
    pub slots: Option<Vec<FilterEffectsSlot>>,
    pub preview: Option<FilterEffectsPreview>,
    pub rgba: Option<PixelData>,
}

/// Filter effects rectangle
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FilterEffectsRect {
    pub left: i32,
    pub top: i32,
    pub right: i32,
    pub bottom: i32,
}

/// Filter effects slot
#[derive(Debug, Clone, PartialEq)]
pub struct FilterEffectsSlot {
    pub slot: u32,
    pub channel_data: ChannelImageData,
}

/// Filter effects preview
#[derive(Debug, Clone, PartialEq)]
pub struct FilterEffectsPreview {
    pub rect: FilterEffectsRect,
    pub channel_data: ChannelImageData,
    pub rgba: Option<PixelData>,
}

/// Pixel source data block (PxSD)
#[derive(Debug, Clone, PartialEq)]
pub struct PixelSourceDataBlock {
    pub items: Vec<PixelSourceDataItem>,
}

/// Pixel source data item
#[derive(Debug, Clone, PartialEq)]
pub struct PixelSourceDataItem {
    pub key: u32,
    pub images: Option<Vec<PixelSourceDataImage>>,
}

/// Pixel source data image
#[derive(Debug, Clone, PartialEq)]
pub struct PixelSourceDataImage {
    pub index: u32,
    pub rect: Option<FilterEffectsRect>,
    pub rgba: Option<PixelData>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct FilterMaskPayload {
    pub bytes: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ChannelImageData {
    pub width: usize,
    pub height: usize,
    pub data: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ShapePatternStatus {
    pub version: u32,
    pub present_count: u32,
}

#[derive(Debug, Clone, PartialEq)]
pub struct PatternBlock {
    pub key: PsdStringCode,
    pub patterns: Vec<PatternBlockEntry>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct PatternBlockEntry {
    pub name: String,
    pub id: String,
    pub version: u32,
    pub mode: u32,
    pub width: u16,
    pub height: u16,
    pub indexed_palette: Option<Vec<RGB>>,
    pub rgba: Option<PixelData>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct HighDepthLayerInfo {
    pub key: PsdStringCode,
    pub layers: Vec<Layer>,
}

/// Section divider (layer group info)
#[derive(Debug, Clone, PartialEq)]
pub struct SectionDivider {
    pub divider_type: SectionDividerType,
    pub blend_mode: Option<BlendMode>,
    pub sub_type: Option<PsdU32Code>,
}

/// Protected flags
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ProtectedFlags {
    pub transparency: bool,
    pub composite: bool,
    pub position: bool,
    pub artboards: Option<bool>,
}

/// Text layer data
#[derive(Debug, Clone, PartialEq)]
pub struct TextLayerData {
    pub raw_bytes: Option<Vec<u8>>,
    pub transform: Vec<f64>,
    pub text: String,
    pub text_version: u16,
    pub descriptor_version: u32,
    pub text_data: Option<Descriptor>,
    pub warp_version: u16,
    pub warp_descriptor_version: u32,
    pub warp_data: Option<Descriptor>,
    pub left: f32,
    pub top: f32,
    pub right: f32,
    pub bottom: f32,
}

/// Vector fill
#[derive(Debug, Clone, PartialEq)]
pub struct VectorFill {
    pub fill_type: PsdStringCode,
    pub data: Descriptor,
}

/// Vector stroke
#[derive(Debug, Clone, PartialEq)]
pub struct VectorStroke {
    pub version: u32,
    pub descriptor: Descriptor,
}

/// Vector mask
#[derive(Debug, Clone, PartialEq)]
pub struct VectorMask {
    pub version: u32,
    pub invert: bool,
    pub not_link: bool,
    pub disable: bool,
    pub paths: Vec<VectorPath>,
}

/// Vector path
#[derive(Debug, Clone, PartialEq)]
pub struct VectorPath {
    pub path_type: PathType,
    pub initial_fill_rule: Option<u16>,
    pub clipboard_bounds: Option<Bounds>,
    pub points: Vec<PathPoint>,
}

/// Path type
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PathType {
    Closed = 0,
    Open = 1,
}

/// Path point
#[derive(Debug, Clone, PartialEq)]
pub struct PathPoint {
    pub anchor: Point,
    pub forward: Point,
    pub backward: Point,
    pub linked: bool,
}

/// Bounds rectangle
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Bounds {
    pub top: f64,
    pub left: f64,
    pub bottom: f64,
    pub right: f64,
}

/// Layer effects
#[derive(Debug, Clone, PartialEq)]
pub struct LayerEffects {
    pub version: u32,
    pub descriptor: Option<Descriptor>,
}

/// Placed layer (smart object)
#[derive(Debug, Clone, PartialEq)]
pub struct PlacedLayer {
    pub id: String,
    pub page: Option<i32>,
    pub total_pages: Option<i32>,
    pub anti_alias_policy: Option<PsdIntCode>,
    pub placed_layer_type: Option<PsdIntCode>,
    pub transform: Vec<f64>,
    pub legacy_warp: Option<Descriptor>,
    pub sold_descriptor: Option<Descriptor>,
    pub placed: Option<PsdStringCode>,
}

/// Artboard data
#[derive(Debug, Clone, PartialEq)]
pub struct ArtboardData {
    pub rect: Bounds,
    pub preset_name: Option<String>,
    pub color: Option<Color>,
    pub background_type: Option<PsdIntCode>,
}

/// Single entry inside a shmd block
#[derive(Debug, Clone, PartialEq)]
pub struct MetadataEntry {
    pub key: String,
    pub copy_on_sheet_change: bool,
    pub descriptor: Option<crate::support::descriptor::Descriptor>,
    pub raw_data: Vec<u8>,
}

/// Metadata (shmd block - zero or more tagged entries)
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Metadata {
    pub entries: Vec<MetadataEntry>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct LinkedFilesBlock {
    pub key: PsdStringCode,
    pub items: Vec<LinkedFile>,
}

impl<R: Read + Seek> PsdReader<R> {
    fn descriptor_bool(desc: &Descriptor, key: &str) -> Option<bool> {
        match desc.items.get(key) {
            Some(DescriptorValue::Boolean(v)) => Some(*v),
            _ => None,
        }
    }

    fn descriptor_int(desc: &Descriptor, key: &str) -> Option<i32> {
        match desc.items.get(key) {
            Some(DescriptorValue::Integer(v)) => Some(*v),
            _ => None,
        }
    }

    fn descriptor_number(desc: &Descriptor, key: &str) -> Option<f64> {
        match desc.items.get(key) {
            Some(DescriptorValue::Double(v)) => Some(*v),
            Some(DescriptorValue::Float(v)) => Some(*v as f64),
            Some(DescriptorValue::Integer(v)) => Some(*v as f64),
            Some(DescriptorValue::UnitDouble { value, .. }) => Some(*value),
            Some(DescriptorValue::UnitFloat { value, .. }) => Some(*value),
            _ => None,
        }
    }

    fn descriptor_units_value(desc: &Descriptor, key: &str) -> Option<UnitsValue> {
        match desc.items.get(key) {
            Some(DescriptorValue::UnitDouble { units, value })
            | Some(DescriptorValue::UnitFloat { units, value }) => Some(UnitsValue {
                units: match units.as_str() {
                    "Pixels" => Units::Pixels,
                    "Points" => Units::Points,
                    "Millimeters" => Units::Millimeters,
                    "Centimeters" => Units::Centimeters,
                    "Inches" => Units::Inches,
                    "Density" => Units::Density,
                    _ => Units::None,
                },
                value: *value,
            }),
            _ => None,
        }
    }

    fn descriptor_points(desc: &Descriptor, key: &str) -> Option<Vec<crate::api::types::Point>> {
        match desc.items.get(key) {
            Some(DescriptorValue::Descriptor(inner)) => match inner.items.get("points") {
                Some(DescriptorValue::List(items)) => {
                    let mut points = Vec::new();
                    let mut iter = items.iter();
                    while let (Some(x), Some(y)) = (iter.next(), iter.next()) {
                        let x = match x {
                            DescriptorValue::Double(v) => *v,
                            DescriptorValue::Float(v) => *v as f64,
                            DescriptorValue::Integer(v) => *v as f64,
                            _ => 0.0,
                        };
                        let y = match y {
                            DescriptorValue::Double(v) => *v,
                            DescriptorValue::Float(v) => *v as f64,
                            DescriptorValue::Integer(v) => *v as f64,
                            _ => 0.0,
                        };
                        points.push(crate::api::types::Point { x, y });
                    }
                    Some(points)
                }
                _ => None,
            },
            _ => None,
        }
    }

    fn descriptor_bounds(desc: &Descriptor, key: &str) -> Option<UnitsBounds> {
        let inner = match desc.items.get(key) {
            Some(DescriptorValue::Descriptor(inner)) => inner,
            _ => return None,
        };
        Some(UnitsBounds {
            top: Self::descriptor_units_value(inner, "Top")?,
            left: Self::descriptor_units_value(inner, "Left")?,
            right: Self::descriptor_units_value(inner, "Rght")?,
            bottom: Self::descriptor_units_value(inner, "Btom")?,
        })
    }

    fn descriptor_rrect(desc: &Descriptor, key: &str) -> Option<RRectRadii> {
        let inner = match desc.items.get(key) {
            Some(DescriptorValue::Descriptor(inner)) => inner,
            _ => return None,
        };
        Some(RRectRadii {
            top_right: Self::descriptor_units_value(inner, "topRight")?,
            top_left: Self::descriptor_units_value(inner, "topLeft")?,
            bottom_left: Self::descriptor_units_value(inner, "bottomLeft")?,
            bottom_right: Self::descriptor_units_value(inner, "bottomRight")?,
        })
    }

    fn read_blending_restrictions(
        &mut self,
        info: &mut LayerAdditionalInfo,
        length: usize,
    ) -> Result<()> {
        let mut restrictions = [1u8, 1u8, 1u8];
        let count = length / 4;
        for _ in 0..count {
            let index = self.read_u32()? as usize;
            if index < 3 {
                restrictions[index] = 0;
            }
        }
        info.blending_restrictions = Some(restrictions);
        Ok(())
    }

    fn read_reference_point(&mut self, info: &mut LayerAdditionalInfo) -> Result<()> {
        info.reference_point = Some(Point {
            x: self.read_f64()?,
            y: self.read_f64()?,
        });
        Ok(())
    }

    fn decode_pattern_channel(
        &mut self,
        width: usize,
        height: usize,
        length: usize,
    ) -> Result<Vec<u8>> {
        if length == 0 {
            return Ok(vec![0; width * height]);
        }

        let depth = self.read_u32()? as usize;
        self.skip_bytes(16)?;
        let compression = self.read_u16()?;
        self.skip_bytes(1)?;
        let remaining = length.saturating_sub(23);
        let bytes_per_sample = depth / 8;
        let row_width = width * bytes_per_sample;

        if compression == 1 {
            let mut row_lengths = Vec::with_capacity(height);
            for _ in 0..height {
                row_lengths.push(self.read_u16()? as u32);
            }
            let encoded_length = remaining.saturating_sub(height * 2);
            let encoded = self.read_bytes(encoded_length)?;
            let mut out = vec![0u8; row_width * height];
            compression::decompress_rle(&encoded, &mut out, row_width, height, &row_lengths)?;
            Ok(out)
        } else {
            self.read_bytes((row_width * height).min(remaining))
        }
    }

    fn interleave_pattern_buffer(
        mode: u32,
        width: usize,
        height: usize,
        palette: Option<&[RGB]>,
        channels: &[Vec<u8>],
        alpha: Option<&[u8]>,
    ) -> Option<Vec<u8>> {
        let pixel_count = width * height;
        let mut rgba = vec![255u8; pixel_count * 4];
        match mode {
            3 => {
                let red = channels.first()?;
                let green = channels.get(1).unwrap_or(red);
                let blue = channels.get(2).unwrap_or(red);
                for index in 0..pixel_count {
                    rgba[index * 4] = *red.get(index).unwrap_or(&0);
                    rgba[index * 4 + 1] = *green.get(index).unwrap_or(&0);
                    rgba[index * 4 + 2] = *blue.get(index).unwrap_or(&0);
                    rgba[index * 4 + 3] = alpha.and_then(|a| a.get(index)).copied().unwrap_or(255);
                }
                Some(rgba)
            }
            1 => {
                let gray = channels.first()?;
                for index in 0..pixel_count {
                    let value = *gray.get(index).unwrap_or(&0);
                    rgba[index * 4] = value;
                    rgba[index * 4 + 1] = value;
                    rgba[index * 4 + 2] = value;
                    rgba[index * 4 + 3] = alpha.and_then(|a| a.get(index)).copied().unwrap_or(255);
                }
                Some(rgba)
            }
            2 => {
                let palette = palette?;
                let indexed = channels.first()?;
                for index in 0..pixel_count {
                    let color_index = *indexed.get(index).unwrap_or(&0) as usize;
                    let color =
                        palette
                            .get(color_index)
                            .cloned()
                            .unwrap_or(RGB { r: 0, g: 0, b: 0 });
                    rgba[index * 4] = color.r;
                    rgba[index * 4 + 1] = color.g;
                    rgba[index * 4 + 2] = color.b;
                    rgba[index * 4 + 3] = alpha.and_then(|a| a.get(index)).copied().unwrap_or(255);
                }
                Some(rgba)
            }
            _ => None,
        }
    }

    fn palette_bytes_to_colors(bytes: &[u8]) -> Vec<RGB> {
        (0..256)
            .map(|index| {
                let offset = index * 3;
                RGB {
                    r: *bytes.get(offset).unwrap_or(&0),
                    g: *bytes.get(offset + 1).unwrap_or(&0),
                    b: *bytes.get(offset + 2).unwrap_or(&0),
                }
            })
            .collect()
    }

    fn sample_to_byte(data: &[u8], sample_index: usize, depth: usize) -> u8 {
        match depth {
            8 => *data.get(sample_index).unwrap_or(&0),
            16 => {
                let offset = sample_index * 2;
                let high = *data.get(offset).unwrap_or(&0) as u16;
                let low = *data.get(offset + 1).unwrap_or(&0) as u16;
                ((high << 8) | low) as u8
            }
            32 => {
                let offset = sample_index * 4;
                if offset + 3 >= data.len() {
                    return 0;
                }
                let float = f32::from_be_bytes([
                    data[offset],
                    data[offset + 1],
                    data[offset + 2],
                    data[offset + 3],
                ]);
                (float * 255.0).round().clamp(0.0, 255.0) as u8
            }
            _ => 0,
        }
    }

    fn interleave_feid_buffer(
        rect: FilterEffectsRect,
        slots: &[FilterEffectsSlot],
        depth: usize,
    ) -> PixelData {
        let width = (rect.right - rect.left).max(0) as usize;
        let height = (rect.bottom - rect.top).max(0) as usize;
        let pixel_count = width * height;
        let mut rgba = vec![255u8; pixel_count * 4];

        let red = slots.iter().find(|slot| slot.slot == 0);
        let green = slots.iter().find(|slot| slot.slot == 1);
        let blue = slots.iter().find(|slot| slot.slot == 2);
        let alpha = slots.iter().find(|slot| slot.slot == 25);

        if let Some(slot) = red {
            let count = pixel_count.min(slot.channel_data.data.len() / depth.max(1).div_ceil(8));
            for index in 0..count {
                rgba[index * 4] = Self::sample_to_byte(&slot.channel_data.data, index, depth);
            }
        }
        if let Some(slot) = green {
            let count = pixel_count.min(slot.channel_data.data.len() / depth.max(1).div_ceil(8));
            for index in 0..count {
                rgba[index * 4 + 1] = Self::sample_to_byte(&slot.channel_data.data, index, depth);
            }
        }
        if let Some(slot) = blue {
            let count = pixel_count.min(slot.channel_data.data.len() / depth.max(1).div_ceil(8));
            for index in 0..count {
                rgba[index * 4 + 2] = Self::sample_to_byte(&slot.channel_data.data, index, depth);
            }
        }
        if let Some(slot) = alpha {
            let count = pixel_count.min(slot.channel_data.data.len() / depth.max(1).div_ceil(8));
            for index in 0..count {
                rgba[index * 4 + 3] = Self::sample_to_byte(&slot.channel_data.data, index, depth);
            }
        }

        PixelData {
            data: rgba,
            width,
            height,
        }
    }

    fn read_pattern_entry(&mut self, length: usize) -> Result<PatternBlockEntry> {
        let start = self.offset;
        let version = self.read_u32()?;
        let mode = self.read_u32()?;
        let height = self.read_u16()?;
        let width = self.read_u16()?;
        let name = self.read_unicode_string()?;
        let id = self.read_pascal_string(1)?;
        let indexed_palette = if mode == 2 {
            let palette = self.read_bytes(3 * 256)?;
            self.read_u32()?;
            Some(Self::palette_bytes_to_colors(&palette))
        } else {
            None
        };
        self.read_u32()?;
        self.read_u32()?;
        self.skip_bytes(16)?;
        let channel_count = self.read_u32()? as usize;
        let width_usize = width as usize;
        let height_usize = height as usize;
        let mut channels = Vec::new();
        let mut alpha = None;
        for channel_index in 0..(channel_count + 2) {
            let present = self.read_u32()?;
            if present == 0 {
                continue;
            }
            let channel_length = self.read_u32()? as usize;
            if channel_length == 0 {
                continue;
            }
            let channel = self.decode_pattern_channel(width_usize, height_usize, channel_length)?;
            if channel_index < channel_count {
                channels.push(channel);
            } else if channel_index == channel_count + 1 {
                alpha = Some(channel);
            }
        }
        let consumed = (self.offset - start) as usize;
        if consumed < length {
            self.skip_bytes(length - consumed)?;
        }
        Ok(PatternBlockEntry {
            name,
            id,
            version,
            mode,
            width,
            height,
            indexed_palette: indexed_palette.clone(),
            rgba: Self::interleave_pattern_buffer(
                mode,
                width_usize,
                height_usize,
                indexed_palette.as_deref(),
                &channels,
                alpha.as_deref(),
            )
            .map(|data| PixelData {
                data,
                width: width_usize,
                height: height_usize,
            }),
        })
    }

    fn read_pattern_block(&mut self, key: &str, length: usize) -> Result<PatternBlock> {
        let start = self.offset;
        let mut patterns = Vec::new();
        while (self.offset - start) < length as u64 {
            let entry_length = self.read_u32()? as usize;
            let entry_start = self.offset;
            patterns.push(self.read_pattern_entry(entry_length)?);
            let consumed = (self.offset - entry_start) as usize;
            if consumed < entry_length {
                self.skip_bytes(entry_length - consumed)?;
            }
            let padding = entry_length % 4;
            if padding != 0 && (self.offset - start) < length as u64 {
                self.skip_bytes(4 - padding)?;
            }
        }
        Ok(PatternBlock {
            key: PsdStringCode::from(key),
            patterns,
        })
    }

    /// Read layer additional info section
    pub fn read_additional_info(
        &mut self,
        key: &str,
        length: usize,
        info: &mut LayerAdditionalInfo,
    ) -> Result<()> {
        let start_offset = self.offset;

        match key {
            "luni" => self.read_unicode_layer_name(info)?,
            "lyid" => self.read_layer_id(info)?,
            "lclr" => self.read_layer_color(info)?,
            "iOpa" => info.fill_opacity = Some(self.read_u8()?),
            "lsct" | "lsdk" => self.read_section_divider(info, length)?,
            "clbl" => self.read_blend_clipped(info)?,
            "infx" => self.read_blend_interior(info)?,
            "knko" => self.read_knockout(info)?,
            "lspf" => self.read_protected_flags(info, length)?,
            "lnsr" => self.read_name_source(info)?,
            "lyvr" => info.layer_version = Some(self.read_u32()?),
            "lmgm" => info.lmgm = Some(self.read_u8()?),
            "vmgm" => info.vmgm = Some(self.read_u8()?),
            "fcmy" => info.fcmy = Some(self.read_u8()?),
            "brst" => self.read_blending_restrictions(info, length)?,
            "fxrp" => self.read_reference_point(info)?,
            "TySh" => {
                let bytes = self.read_bytes(length)?;
                let mut sub = PsdReader::new(std::io::Cursor::new(bytes.clone()), Default::default());
                sub.read_text_layer(info, length)?;
                if let Some(text) = info.text.as_mut() {
                    text.raw_bytes = Some(bytes);
                }
            }
            "SoCo" | "GdFl" | "PtFl" => self.read_vector_fill(info, key)?,
            "vstk" => self.read_vector_stroke(info, length)?,
            "vscg" => self.read_vscg(info, length)?,
            "vmsk" | "vsms" => self.read_vector_mask(info, length)?,
            "vogk" => self.read_vector_origination(info, length)?,
            "lrFX" | "lfx2" => self.read_layer_effects(info, key, length)?,
            "PlLd" => self.read_placed_layer(info, key, length)?,
            "SoLd" => self.read_sold_layer(info, length)?,
            "artb" | "artd" => self.read_artboard(info, key, length)?,
            "sn2P" => self.read_using_aligned_rendering(info)?,
            "shmd" => self.read_metadata(info, length)?,
            "brit" | "levl" | "curv" | "expA" | "blnc" | "phfl" | "hue2" | "selc" | "mixr"
            | "post" | "thrs" | "nvrt" | "grdm" | "blwh" => {
                let data = self.read_bytes(length)?;
                info.adjustment = Some(AdjustmentLayer::from_key_and_bytes(key, &data)?);
            }
            "Lr16" | "Lr32" => {
                let data = self.read_bytes(length)?;
                let bits = if key == "Lr16" { 16 } else { 32 };
                let layers = crate::io::reader::read_nested_layer_info_block(&data, bits)?;
                info.high_depth_layer_data = Some(HighDepthLayerInfo {
                    key: PsdStringCode::from(key),
                    layers,
                });
            }
            "lnk2" | "lnkD" | "lnkD__" | "lnk3" => {
                let block_end = start_offset + length as u64;
                let mut items = Vec::new();
                while self.offset < block_end {
                    let chunk_length = read_u64_parts(self)?;
                    let chunk_start = self.offset;
                    let kind = self.read_signature()?;
                    let item_version = self.read_u32()?;
                    let id = self.read_pascal_string(1)?;
                    let name = self.read_unicode_string()?;
                    let file_type = self.read_signature()?;
                    let creator = self.read_signature()?;
                    let payload_length = read_u64_parts(self)?;
                    let open = self.read_u8()?;
                    let mut open_descriptor = None;
                    if open != 0 {
                        let _descriptor_version = self.read_u32()?;
                        let descriptor_start = self.offset;
                        open_descriptor = Some(self.read_descriptor_structure()?);
                        if (self.offset - descriptor_start) % 2 != 0 {
                            self.skip_bytes(1)?;
                        }
                    }
                    let descriptor = if kind == "liFE" {
                        Some(self.read_descriptor_structure()?)
                    } else {
                        None
                    };
                    let time = if kind == "liFE" && item_version > 3 {
                        Some(format_linked_file_time(
                            self.read_u32()?,
                            self.read_u8()?,
                            self.read_u8()?,
                            self.read_u8()?,
                            self.read_u8()?,
                            self.read_f64()?,
                        ))
                    } else {
                        None
                    };
                    let external_file_size = if kind == "liFE" {
                        Some(read_u64_parts(self)?)
                    } else {
                        None
                    };
                    if kind == "liFA" {
                        let _alias_padding = read_u64_parts(self)?;
                    }
                    let data = self.read_bytes(payload_length as usize)?;
                    let child_document_id =
                        if item_version >= 5 && self.offset < chunk_start + chunk_length {
                            Some(self.read_unicode_string()?)
                        } else {
                            None
                        };
                    let asset_mod_time =
                        if item_version >= 6 && self.offset < chunk_start + chunk_length {
                            Some(self.read_f64()?)
                        } else {
                            None
                        };
                    let asset_locked_state =
                        if item_version >= 7 && self.offset < chunk_start + chunk_length {
                            Some(self.read_u8()?)
                        } else {
                            None
                        };
                    let linked_file = if kind == "liFE" {
                        if item_version >= 7 && self.offset < chunk_start + chunk_length {
                            Some(LinkedFileInfo {
                                file_size: external_file_size.unwrap_or_default(),
                                name: self.read_unicode_string()?,
                                full_path: self.read_unicode_string()?,
                                original_path: self.read_unicode_string()?,
                                relative_path: self.read_unicode_string()?,
                            })
                        } else {
                            external_file_size.map(|file_size| LinkedFileInfo {
                                file_size,
                                name: String::new(),
                                full_path: String::new(),
                                original_path: String::new(),
                                relative_path: String::new(),
                            })
                        }
                    } else {
                        None
                    };
                    let remaining =
                        (chunk_start + chunk_length).saturating_sub(self.offset) as usize;
                    if remaining > 0 {
                        self.skip_bytes(remaining)?;
                    }
                    if chunk_length % 4 != 0 {
                        self.skip_bytes((4 - (chunk_length % 4)) as usize)?;
                    }
                    items.push(LinkedFile {
                        id,
                        name,
                        item_version: Some(item_version),
                        data_kind: Some(LinkedFileDataKind::from_code(&kind)),
                        file_type: Some(PsdStringCode(file_type)),
                        creator: Some(PsdStringCode(creator)),
                        data: Some(data),
                        time,
                        descriptor,
                        child_document_id,
                        asset_mod_time,
                        asset_locked_state,
                        linked_file,
                        open_descriptor,
                    });
                }
                info.linked_files = Some(LinkedFilesBlock {
                    key: PsdStringCode::from(key),
                    items,
                });
            }
            "lmfx" | "lfxs" => {
                // u32 version + u32 (descriptor version) + descriptor
                let _ver = self.read_u32()?;
                let descriptor = self.read_version_and_descriptor()?;
                info.layer_effects_descriptor = Some(descriptor);
            }
            "FMsk" => {
                info.filter_mask = Some(FilterMaskPayload {
                    bytes: self.read_bytes(length)?,
                });
            }
            "shpa" => {
                info.shape_pattern = Some(ShapePatternStatus {
                    version: self.read_u32()?,
                    present_count: self.read_u32()?,
                });
            }
            "Patt" | "Pat2" | "Pat3" => {
                info.pattern_data = Some(self.read_pattern_block(key, length)?);
            }
            "clrL" | "rplc" => {
                // u16 + u32 + descriptor
                let _ = self.read_u16()?;
                let _ = self.read_u32()?;
                let descriptor = self.read_descriptor_structure()?;
                info.descriptor_blocks.insert(key.to_string(), descriptor);
            }
            "pths" | "CgEd" | "vibA" | "PxSc" | "phry" => {
                let _version = self.read_u32()?;
                let descriptor = self.read_descriptor_structure()?;
                info.descriptor_blocks.insert(key.to_string(), descriptor);
            }
            "Txt2" => {
                let raw = self.read_bytes(length)?;
                let parsed = crate::support::engine_data::parse_engine_data(&raw).or_else(|_| {
                    if raw.len() < 4 {
                        return Err(PsdError::InvalidFormat(
                            "invalid Txt2 payload".to_string(),
                        ));
                    }
                    let inner_length =
                        u32::from_be_bytes(raw[..4].try_into().expect("Txt2 inner length")) as usize;
                    let available = raw.len().saturating_sub(4);
                    let bounded = inner_length.min(available);
                    crate::support::engine_data::parse_engine_data(&raw[4..4 + bounded])
                        .map_err(|e| PsdError::InvalidFormat(e.to_string()))
                })?;
                info.text_engine = Some(TextEngineBlock {
                    data: parsed,
                    raw: Some(raw),
                });
            }
            "LMsk" | "Mtrn" | "Mt16" | "Mt32" | "FXid" => {
                info.raw_blocks.push(RawTaggedBlock {
                    key: key.to_string(),
                    data: self.read_bytes(length)?,
                });
            }
            "abdd" | "anFX" | "cinf" | "SoLE" => {
                info.raw_blocks.push(RawTaggedBlock {
                    key: key.to_string(),
                    data: self.read_bytes(length)?,
                });
            }
            "Anno" => {
                let header: AnnotationHeaderRecord =
                    decode_be(&self.read_bytes(8)?, "annotation header")?;
                let count = header.count as usize;
                let mut items = Vec::with_capacity(count);
                for _ in 0..count {
                    let item_length = self.read_u32()? as usize;
                    let item_start = self.offset;
                    let signature = self.read_signature()?;
                    if signature != "txtA" {
                        return Err(PsdError::InvalidFormat(format!(
                            "Unexpected annotation signature: {}",
                            signature
                        )));
                    }
                    self.skip_bytes(4)?;
                    let x = self.read_i32()?;
                    let y = self.read_i32()?;
                    self.skip_bytes(24)?;
                    // Read color: 4×u16 (colorSpace + 3 scaled channels + padding)
                    let _color_space = self.read_u16()?;
                    let color_r = ((self.read_u16()? as u32 * 255 + 32767) / 65535) as u8;
                    let color_g = ((self.read_u16()? as u32 * 255 + 32767) / 65535) as u8;
                    let color_b = ((self.read_u16()? as u32 * 255 + 32767) / 65535) as u8;
                    let _color_pad = self.read_u16()?;
                    // Read author/date strings with 4-byte length prefix format
                    let author = read_pascal_string_with_padding(self)?;
                    let _empty = read_pascal_string_with_padding(self)?;
                    let _date = read_pascal_string_with_padding(self)?;
                    let _text_len = self.read_u32()? as usize;
                    let block = self.read_signature()?;
                    if block != "txtC" {
                        return Err(PsdError::InvalidFormat(format!(
                            "Unexpected annotation text block: {}",
                            block
                        )));
                    }
                    let chars_len = self.read_u32()? as usize;
                    self.skip_bytes(2)?;
                    let text = self.read_unicode_string_with_length(chars_len.saturating_sub(2) / 2)?;
                    let consumed = (self.offset - item_start) as usize;
                    if consumed < item_length {
                        self.skip_bytes(item_length - consumed)?;
                    }
                    items.push(AnnotationItem {
                        x,
                        y,
                        color_l: color_r as u16,
                        color_o: color_g as u16,
                        color_c: color_b as u16,
                        author,
                        text,
                    });
                }
                info.annotations = Some(items);
            }
            "FEid" => {
                let block_end = start_offset + length as u64;
                let version = self.read_u32()?;
                let mut items = Vec::new();
                while self.offset < block_end {
                    let high = self.read_u32()? as u64;
                    let low = self.read_u32()? as u64;
                    let chunk_length = (high << 32) | low;
                    let chunk_start = self.offset;
                    let id = self.read_pascal_string(1)?;
                    let item_version = if self.offset + 4 <= chunk_start + chunk_length {
                        Some(self.read_u32()?)
                    } else {
                        None
                    };
                    if self.offset + 4 <= chunk_start + chunk_length {
                        let _constant_8 = self.read_u32()?;
                    }
                    let mut rect = None;
                    let mut depth = None;
                    let mut channel_count = None;
                    let mut slots = Vec::new();
                    let mut preview = None;
                    if self.offset + 24 <= chunk_start + chunk_length {
                        rect = Some(FilterEffectsRect {
                            left: self.read_i32()?,
                            top: self.read_i32()?,
                            right: self.read_i32()?,
                            bottom: self.read_i32()?,
                        });
                        depth = Some(self.read_u32()?);
                        let ch_count = self.read_u32()?;
                        channel_count = Some(ch_count);
                        for slot_index in 0..ch_count + 2 {
                            if self.offset + 4 > chunk_start + chunk_length {
                                break;
                            }
                            let present = self.read_u32()?;
                            if present != 0 && self.offset + 8 <= chunk_start + chunk_length {
                                let slot_high = self.read_u32()? as u64;
                                let slot_low = self.read_u32()? as u64;
                                let slot_length = (slot_high << 32) | slot_low;
                                if slot_length
                                    <= (chunk_start + chunk_length).saturating_sub(self.offset)
                                {
                                    let slot_bytes = self.read_bytes(slot_length as usize)?;
                                    let width =
                                        (rect.unwrap().right - rect.unwrap().left).max(0) as usize;
                                    let height =
                                        (rect.unwrap().bottom - rect.unwrap().top).max(0) as usize;
                                    // Slot data: first 2 bytes are compression type
                                    let comp = if slot_bytes.len() >= 2 {
                                        u16::from_be_bytes([slot_bytes[0], slot_bytes[1]])
                                    } else {
                                        0
                                    };
                                    let slot_depth = depth.unwrap_or(8) as usize;
                                    let bytes_per_sample = slot_depth.max(8) / 8;
                                    let row_width = width * bytes_per_sample;
                                    let output_size = row_width * height;
                                    let pixel_data = match comp {
                                        0 => slot_bytes.get(2..).unwrap_or(&[]).to_vec(),
                                        1 => {
                                            // PackBits: row-length table (u16 per row) then data
                                            let row_table_bytes = height * 2;
                                            if slot_bytes.len() >= 2 + row_table_bytes {
                                                let row_lengths: Vec<u32> = slot_bytes
                                                    [2..2 + row_table_bytes]
                                                    .chunks_exact(2)
                                                    .map(|b| u16::from_be_bytes([b[0], b[1]]) as u32)
                                                    .collect();
                                                let encoded = &slot_bytes[2 + row_table_bytes..];
                                                let mut out = vec![0u8; output_size];
                                                let _ = compression::decompress_rle(
                                                    encoded,
                                                    &mut out,
                                                    row_width,
                                                    height,
                                                    &row_lengths,
                                                );
                                                out
                                            } else {
                                                slot_bytes.get(2..).unwrap_or(&[]).to_vec()
                                            }
                                        }
                                        2 => compression::decompress_zip(
                                            &slot_bytes[2..],
                                            output_size,
                                        )
                                        .unwrap_or_else(|_| {
                                            slot_bytes.get(2..).unwrap_or(&[]).to_vec()
                                        }),
                                        3 => compression::decompress_zip_with_prediction(
                                            &slot_bytes[2..],
                                            width,
                                            height,
                                            slot_depth as u16,
                                        )
                                        .unwrap_or_else(|_| {
                                            slot_bytes.get(2..).unwrap_or(&[]).to_vec()
                                        }),
                                        _ => slot_bytes.get(2..).unwrap_or(&[]).to_vec(),
                                    };
                                    slots.push(FilterEffectsSlot {
                                        slot: slot_index,
                                        channel_data: ChannelImageData {
                                            width,
                                            height,
                                            data: pixel_data,
                                        },
                                    });
                                }
                            }
                        }
                        if self.offset < chunk_start + chunk_length {
                            let has_preview = self.read_u8()?;
                            if has_preview != 0 && self.offset + 24 <= chunk_start + chunk_length {
                                let preview_rect = FilterEffectsRect {
                                    left: self.read_i32()?,
                                    top: self.read_i32()?,
                                    right: self.read_i32()?,
                                    bottom: self.read_i32()?,
                                };
                                let preview_high = self.read_u32()? as u64;
                                let preview_low = self.read_u32()? as u64;
                                let preview_len = (preview_high << 32) | preview_low;
                                let remaining =
                                    (chunk_start + chunk_length).saturating_sub(self.offset);
                                let preview_raw = if preview_len > 0 && preview_len <= remaining {
                                    self.read_bytes(preview_len as usize)?
                                } else {
                                    Vec::new()
                                };
                                let width =
                                    (preview_rect.right - preview_rect.left).max(0) as usize;
                                let height =
                                    (preview_rect.bottom - preview_rect.top).max(0) as usize;
                                preview = Some(FilterEffectsPreview {
                                    rect: preview_rect,
                                    channel_data: ChannelImageData {
                                        width,
                                        height,
                                        data: preview_raw.clone(),
                                    },
                                    rgba: Some(PixelData {
                                        data: preview_raw,
                                        width,
                                        height,
                                    }),
                                });
                            }
                        }
                    }
                    self.skip_bytes(
                        (chunk_start + chunk_length).saturating_sub(self.offset) as usize
                    )?;
                    if chunk_length % 4 != 0 {
                        self.skip_bytes((4 - (chunk_length % 4)) as usize)?;
                    }
                    let slots_option = if slots.is_empty() {
                        None
                    } else {
                        Some(slots.clone())
                    };
                    let rgba = rect.zip(depth).map(|(rect, depth)| {
                        Self::interleave_feid_buffer(rect, slots.as_slice(), depth as usize)
                    });
                    items.push(FilterEffectsItem {
                        id,
                        version: item_version,
                        rect,
                        depth,
                        channel_count,
                        slots: slots_option,
                        preview,
                        rgba,
                    });
                }
                info.filter_effects = Some(FilterEffectsBlock { version, items });
            }
            "PxSD" => {
                let block_end = start_offset + length as u64;
                let mut items = Vec::new();
                while self.offset + 8 <= block_end {
                    let high = self.read_u32()? as u64;
                    let low = self.read_u32()? as u64;
                    let chunk_length = (high << 32) | low;
                    let chunk_start = self.offset;
                    let key_value = self.read_u32()?;
                    let kind = self.read_u32()?;
                    let mut images = Vec::new();
                    if kind == 2 && self.offset + 12 <= chunk_start + chunk_length {
                        let _ = self.read_u32()?;
                        let _high = self.read_u32()?;
                        let _low = self.read_u32()?;
                        let image_count = self.read_u32()?;
                        for _image_index in 0..image_count {
                            if self.offset + 8 > chunk_start + chunk_length {
                                break;
                            }
                            let img_high = self.read_u32()? as u64;
                            let img_low = self.read_u32()? as u64;
                            let img_length = (img_high << 32) | img_low;
                            if img_length == 0 {
                                break;
                            }
                            let img_start = self.offset;
                            let parsed_index = self.read_u32()?;
                            let _top = self.read_i32()?;
                            let _left = self.read_i32()?;
                            let _ = self.read_u32()?;
                            let _ = self.read_u32()?;
                            let mut rect = None;
                            for _channel_index in 0..6 {
                                if self.offset + 28 > img_start + img_length {
                                    break;
                                }
                                let _ = self.read_u32()?;
                                rect = Some(FilterEffectsRect {
                                    left: self.read_i32()?,
                                    top: self.read_i32()?,
                                    right: self.read_i32()?,
                                    bottom: self.read_i32()?,
                                });
                                let _ = self.read_u32()?;
                                let payload_len = self.read_u32()? as usize;
                                if payload_len > 0
                                    && self.offset + payload_len as u64 <= img_start + img_length
                                {
                                    self.skip_bytes(payload_len)?;
                                }
                                break;
                            }
                            self.skip_bytes(
                                (img_start + img_length).saturating_sub(self.offset) as usize
                            )?;
                            images.push(PixelSourceDataImage {
                                index: parsed_index,
                                rect,
                                rgba: None,
                            });
                        }
                    }
                    self.skip_bytes(
                        (chunk_start + chunk_length).saturating_sub(self.offset) as usize
                    )?;
                    items.push(PixelSourceDataItem {
                        key: key_value,
                        images: if images.is_empty() {
                            None
                        } else {
                            Some(images)
                        },
                    });
                }
                info.pixel_source_data = Some(PixelSourceDataBlock { items });
            }
            _ => {
                info.raw_blocks.push(RawTaggedBlock {
                    key: key.to_string(),
                    data: self.read_bytes(length)?,
                });
            }
        }

        // Ensure we consumed exactly the right amount
        let consumed = (self.offset - start_offset) as usize;
        if consumed < length {
            self.skip_bytes(length - consumed)?;
        }

        Ok(())
    }

    /// Read layer ID (lyid)
    fn read_layer_id(&mut self, info: &mut LayerAdditionalInfo) -> Result<()> {
        info.id = Some(self.read_u32()? as i32);
        Ok(())
    }

    /// Read layer color (lclr)
    fn read_layer_color(&mut self, info: &mut LayerAdditionalInfo) -> Result<()> {
        let record: LayerColorRecord = decode_be(&self.read_bytes(8)?, "layer color")?;
        info.layer_color = Some(match record.color_value {
            0 => crate::api::types::LayerColor::None,
            1 => crate::api::types::LayerColor::Red,
            2 => crate::api::types::LayerColor::Orange,
            3 => crate::api::types::LayerColor::Yellow,
            4 => crate::api::types::LayerColor::Green,
            5 => crate::api::types::LayerColor::Blue,
            6 => crate::api::types::LayerColor::Violet,
            7 => crate::api::types::LayerColor::Gray,
            value => {
                return Err(PsdError::InvalidFormat(format!(
                    "Invalid layer color: {}",
                    value
                )))
            }
        });
        Ok(())
    }

    /// Read section divider (lsct/lsdk)
    fn read_section_divider(
        &mut self,
        info: &mut LayerAdditionalInfo,
        length: usize,
    ) -> Result<()> {
        let base: SectionDividerBaseRecord = decode_be(&self.read_bytes(4)?, "section divider")?;

        let mut blend_mode = None;
        let mut sub_type = None;

        if length >= 12 {
            let ext: SectionDividerExtendedRecord =
                decode_be(&self.read_bytes(8)?, "section divider extended")?;
            if &ext.signature != b"8BIM" {
                return Err(PsdError::InvalidFormat(format!(
                    "Invalid section divider signature: {}",
                    String::from_utf8_lossy(&ext.signature),
                )));
            }
            blend_mode = Some(to_blend_mode(&String::from_utf8_lossy(&ext.blend_mode))?);
        }

        if length >= 16 {
            sub_type = Some(PsdU32Code(
                decode_be::<U32ValueRecord>(&self.read_bytes(4)?, "section divider subtype")?.value,
            ));
        }

        info.section_divider = Some(SectionDivider {
            divider_type: match base.divider_type {
                0 => SectionDividerType::Other,
                1 => SectionDividerType::OpenFolder,
                2 => SectionDividerType::ClosedFolder,
                3 => SectionDividerType::BoundingSectionDivider,
                value => {
                    return Err(PsdError::InvalidFormat(format!(
                        "Invalid section divider type: {}",
                        value
                    )))
                }
            },
            blend_mode,
            sub_type,
        });

        Ok(())
    }

    /// Read blend clipped elements (clbl)
    fn read_blend_clipped(&mut self, info: &mut LayerAdditionalInfo) -> Result<()> {
        info.blend_clipped_elements =
            Some(decode_be::<U8BoolRecord>(&self.read_bytes(1)?, "blend clipped")?.value != 0);
        Ok(())
    }

    /// Read blend interior elements (infx)
    fn read_blend_interior(&mut self, info: &mut LayerAdditionalInfo) -> Result<()> {
        info.blend_interior_elements =
            Some(decode_be::<U8BoolRecord>(&self.read_bytes(1)?, "blend interior")?.value != 0);
        Ok(())
    }

    /// Read knockout mode (knko)
    fn read_knockout(&mut self, info: &mut LayerAdditionalInfo) -> Result<()> {
        info.knockout =
            Some(decode_be::<U8BoolRecord>(&self.read_bytes(1)?, "knockout")?.value != 0);
        Ok(())
    }

    /// Read protected flags (lspf)
    fn read_protected_flags(
        &mut self,
        info: &mut LayerAdditionalInfo,
        length: usize,
    ) -> Result<()> {
        let flags = ProtectedFlagsBits::from_bits_retain(
            decode_be::<ProtectedFlagsRecord>(&self.read_bytes(4)?, "protected flags")?.flags,
        );

        let protected = ProtectedFlags {
            transparency: flags.contains(ProtectedFlagsBits::TRANSPARENCY),
            composite: flags.contains(ProtectedFlagsBits::COMPOSITE),
            position: flags.contains(ProtectedFlagsBits::POSITION),
            artboards: if length >= 8 {
                Some(flags.contains(ProtectedFlagsBits::ARTBOARDS))
            } else {
                None
            },
        };

        info.protected = Some(protected);
        Ok(())
    }

    /// Read layer name source (lnsr)
    fn read_name_source(&mut self, info: &mut LayerAdditionalInfo) -> Result<()> {
        let record: NameSourceRecord = decode_be(&self.read_bytes(4)?, "name source")?;
        info.name_source = Some(String::from_utf8_lossy(&record.signature).to_string());
        Ok(())
    }

    fn read_unicode_layer_name(&mut self, info: &mut LayerAdditionalInfo) -> Result<()> {
        info.name = Some(self.read_unicode_string()?);
        Ok(())
    }

    /// Read text layer data (TySh)
    fn read_text_layer(&mut self, info: &mut LayerAdditionalInfo, _length: usize) -> Result<()> {
        let version = self.read_i16()?;
        if version != 1 {
            return Err(PsdError::InvalidFormat(format!(
                "Invalid TySh version: {}",
                version
            )));
        }

        // Read transform matrix (6 doubles)
        let mut transform = Vec::with_capacity(6);
        for _ in 0..6 {
            transform.push(self.read_f64()?);
        }

        // Read text version
        let text_version = self.read_i16()? as u16;
        if text_version != 50 {
            return Err(PsdError::InvalidFormat(format!(
                "Invalid text version: {}",
                text_version
            )));
        }

        let descriptor_version = self.read_u32()?;
        let text_descriptor = self.read_descriptor_structure()?;

        // Read warp version
        let warp_version = self.read_i16()? as u16;
        if warp_version != 1 {
            return Err(PsdError::InvalidFormat(format!(
                "Invalid warp version: {}",
                warp_version
            )));
        }

        let warp_descriptor_version = self.read_u32()?;
        let warp_descriptor = self.read_descriptor_structure()?;

        // Read bounds
        let left = self.read_f32()?;
        let top = self.read_f32()?;
        let right = self.read_f32()?;
        let bottom = self.read_f32()?;

        // Extract text from descriptor
        let text = text_descriptor
            .items
            .get("Txt ")
            .and_then(|v| {
                if let DescriptorValue::Text(s) = v {
                    Some(s.clone())
                } else {
                    None
                }
            })
            .unwrap_or_default();

        info.text = Some(TextLayerData {
            raw_bytes: None,
            transform,
            text: text.replace('\r', "\n"),
            text_version,
            descriptor_version,
            text_data: Some(text_descriptor),
            warp_version,
            warp_descriptor_version,
            warp_data: Some(warp_descriptor),
            left,
            top,
            right,
            bottom,
        });

        Ok(())
    }

    /// Read vector fill (SoCo/GdFl/PtFl)
    fn read_vector_fill(&mut self, info: &mut LayerAdditionalInfo, key: &str) -> Result<()> {
        let _version = self.read_u32()?;
        let descriptor = self.read_descriptor_structure()?;

        let fill_type = match key {
            "SoCo" => "color",
            "GdFl" => "gradient",
            "PtFl" => "pattern",
            _ => "unknown",
        };

        info.vector_fill = Some(VectorFill {
            fill_type: PsdStringCode::from(fill_type),
            data: descriptor,
        });

        Ok(())
    }

    /// Read vector stroke (vstk): u32 version + raw descriptor (no inner version)
    fn read_vector_stroke(&mut self, info: &mut LayerAdditionalInfo, _length: usize) -> Result<()> {
        let version = self.read_u32()?;
        let descriptor = self.read_descriptor_structure()?;
        info.vector_stroke = Some(VectorStroke {
            version,
            descriptor,
        });
        Ok(())
    }

    /// Read vscg: 4-byte wrapped key + u32 version + raw descriptor
    fn read_vscg(&mut self, info: &mut LayerAdditionalInfo, _length: usize) -> Result<()> {
        let _wrapped_key = self.read_signature()?; // always "vstk" in practice
        let version = self.read_u32()?;
        let descriptor = self.read_descriptor_structure()?;
        info.vector_stroke = Some(VectorStroke {
            version,
            descriptor,
        });
        Ok(())
    }

    /// Read vector mask (vmsk/vsms)
    fn read_vector_mask(&mut self, info: &mut LayerAdditionalInfo, length: usize) -> Result<()> {
        let data = self.read_bytes(length)?;
        let mut reader = PsdReader::new(std::io::Cursor::new(data), Default::default());
        let version = reader.read_u32()?;
        let flags = VectorMaskFlagsBits::from_bits_retain(reader.read_u32()?);

        let invert = flags.contains(VectorMaskFlagsBits::INVERT);
        let not_link = flags.contains(VectorMaskFlagsBits::NOT_LINK);
        let disable = flags.contains(VectorMaskFlagsBits::DISABLE);

        let mut paths = Vec::new();
        let mut pending_initial_fill_rule = None;
        let mut pending_clipboard_bounds = None;

        while reader.offset + 26 <= length as u64 {
            let selector = reader.read_u16()?;

            match selector {
                0 | 3 => {
                    let num_points = reader.read_u16()? as usize;
                    reader.skip_bytes(2)?; // GH
                    reader.skip_bytes(2)?; // Operation
                    reader.skip_bytes(4)?; // Index
                    reader.skip_bytes(4)?; // Reserved
                    reader.skip_bytes(10)?; // Padding

                    let path_type = if selector == 0 {
                        PathType::Closed
                    } else {
                        PathType::Open
                    };
                    let mut points = Vec::new();

                    for _ in 0..num_points {
                        if reader.offset + 26 > length as u64 {
                            break;
                        }

                        let knot_selector = reader.read_u16()?;
                        // TS accepts any selector value; treat non-2 as "linked"
                        let linked = knot_selector != 2;

                        // Read points (vertical, horizontal order)
                        let vert_y = reader.read_fixed_point_path_32()?;
                        let hor_y = reader.read_fixed_point_path_32()?;
                        let vert_anchor = reader.read_fixed_point_path_32()?;
                        let hor_anchor = reader.read_fixed_point_path_32()?;
                        let vert_forward = reader.read_fixed_point_path_32()?;
                        let hor_forward = reader.read_fixed_point_path_32()?;

                        points.push(PathPoint {
                            anchor: Point {
                                x: hor_anchor,
                                y: vert_anchor,
                            },
                            forward: Point {
                                x: hor_forward,
                                y: vert_forward,
                            },
                            backward: Point {
                                x: hor_y,
                                y: vert_y,
                            },
                            linked,
                        });
                    }

                    paths.push(VectorPath {
                        path_type,
                        initial_fill_rule: pending_initial_fill_rule.take(),
                        clipboard_bounds: pending_clipboard_bounds.take(),
                        points,
                    });
                }
                6 => {
                    reader.skip_bytes(24)?;
                }
                7 => {
                    let top = reader.read_fixed_point_path_32()?;
                    let left = reader.read_fixed_point_path_32()?;
                    let bottom = reader.read_fixed_point_path_32()?;
                    let right = reader.read_fixed_point_path_32()?;
                    let _resolution = reader.read_fixed_point_path_32()?;
                    pending_clipboard_bounds = Some(Bounds {
                        top,
                        left,
                        bottom,
                        right,
                    });
                }
                8 => {
                    pending_initial_fill_rule = Some(reader.read_u16()?);
                    reader.skip_bytes(22)?;
                }
                _ => {
                    reader.skip_bytes(24)?;
                }
            }
        }

        info.vector_mask = Some(VectorMask {
            version,
            invert,
            not_link,
            disable,
            paths,
        });

        Ok(())
    }

    /// Read vector origination data (vogk)
    fn read_vector_origination(
        &mut self,
        info: &mut LayerAdditionalInfo,
        _length: usize,
    ) -> Result<()> {
        let _version = self.read_u32()?;
        let _descriptor_version = self.read_u32()?;
        let descriptor = self.read_descriptor_structure()?;
        let list = match descriptor.items.get("keyDescriptorList") {
            Some(DescriptorValue::List(items)) => items,
            _ => {
                info.vector_origination = Some(VectorOrigination {
                    key_descriptor_list: Vec::new(),
                });
                return Ok(());
            }
        };
        let mut key_descriptor_list = Vec::new();
        for item in list {
            let inner = match item {
                DescriptorValue::Descriptor(desc) => desc,
                _ => continue,
            };
            key_descriptor_list.push(KeyDescriptorItem {
                key_shape_invalidated: Self::descriptor_bool(inner, "keyShapeInvalidated"),
                key_origin_index: Self::descriptor_int(inner, "keyOriginIndex").map(PsdIntCode),
                key_origin_type: Self::descriptor_int(inner, "keyOriginType").map(PsdIntCode),
                key_origin_resolution: Self::descriptor_number(inner, "keyOriginResolution"),
                key_origin_rrect_radii: Self::descriptor_rrect(inner, "keyOriginRRectRadii"),
                key_origin_shape_bounding_box: Self::descriptor_bounds(inner, "keyOriginShapeBBox")
                    .or_else(|| Self::descriptor_bounds(inner, "keyOriginShapeBoundingBox")),
                key_origin_box_corners: Self::descriptor_points(inner, "keyOriginBoxCorners"),
                transform: match inner.items.get("transform") {
                    Some(DescriptorValue::List(items)) => Some(
                        items
                            .iter()
                            .filter_map(|v| match v {
                                DescriptorValue::Double(v) => Some(*v),
                                DescriptorValue::Float(v) => Some(*v as f64),
                                DescriptorValue::Integer(v) => Some(*v as f64),
                                _ => None,
                            })
                            .collect(),
                    ),
                    _ => None,
                },
            });
        }
        info.vector_origination = Some(VectorOrigination {
            key_descriptor_list,
        });
        Ok(())
    }

    /// Read layer effects (lrFX/lfx2)
    fn read_layer_effects(
        &mut self,
        info: &mut LayerAdditionalInfo,
        key: &str,
        _length: usize,
    ) -> Result<()> {
        let version = self.read_u32()?;

        let descriptor = if key == "lfx2" {
            Some(self.read_version_and_descriptor()?)
        } else {
            None
        };

        info.effects = Some(LayerEffects {
            version,
            descriptor,
        });

        Ok(())
    }

    /// Read placed layer (PlLd legacy binary format)
    fn read_placed_layer(
        &mut self,
        info: &mut LayerAdditionalInfo,
        _key: &str,
        _length: usize,
    ) -> Result<()> {
        // Read type and version
        let _placed_type = self.read_signature()?;
        let _version = self.read_u32()?;

        // Read UUID as pascal string (u8 length + bytes)
        let id_length = self.read_u8()? as usize;
        let id_bytes = self.read_bytes(id_length)?;
        let id = String::from_utf8_lossy(&id_bytes).to_string();

        // Skip 4×u32 (matching TS: reader.readUint32() × 4)
        self.skip_bytes(16)?;

        // Read transform (8 doubles)
        let mut transform = Vec::with_capacity(8);
        for _ in 0..8 {
            transform.push(self.read_f64()?);
        }

        // Skip 2×u32 (matching TS)
        self.skip_bytes(8)?;

        // Read warp descriptor (legacy PlLd uses a raw descriptor here, no nested version)
        let warp = Some(self.read_descriptor_structure()?);

        info.placed_layer = Some(PlacedLayer {
            id,
            page: None,
            total_pages: None,
            anti_alias_policy: None,
            placed_layer_type: None,
            transform,
            legacy_warp: warp,
            sold_descriptor: None,
            placed: None,
        });

        Ok(())
    }

    /// Read SoLd (descriptor-based format): type + version + u32 + descriptor
    fn read_sold_layer(&mut self, info: &mut LayerAdditionalInfo, _length: usize) -> Result<()> {
        let _sold_type = self.read_signature()?;
        let _version = self.read_u32()?;
        let _skip = self.read_u32()?;
        let descriptor = self.read_descriptor_structure()?;

        // Extract ID from descriptor if possible
        let id = descriptor
            .items
            .get("Idnt")
            .and_then(|v| {
                if let DescriptorValue::Text(s) = v {
                    Some(s.clone())
                } else {
                    None
                }
            })
            .unwrap_or_default();

        if let Some(existing) = info.placed_layer.as_mut() {
            if existing.id.is_empty() {
                existing.id = id;
            }
            existing.sold_descriptor = Some(descriptor);
        } else {
            info.placed_layer = Some(PlacedLayer {
                id,
                page: None,
                total_pages: None,
                anti_alias_policy: None,
                placed_layer_type: None,
                transform: Vec::new(),
                legacy_warp: None,
                sold_descriptor: Some(descriptor),
                placed: None,
            });
        }

        Ok(())
    }

    /// Read artboard data (artb/artd)
    fn read_artboard(
        &mut self,
        info: &mut LayerAdditionalInfo,
        _key: &str,
        _length: usize,
    ) -> Result<()> {
        let _version = self.read_u32()?;
        let descriptor = self.read_descriptor_structure()?;

        // Extract artboard rectangle
        let rect = if let Some(DescriptorValue::Descriptor(rect_desc)) =
            descriptor.items.get("artboardRect")
        {
            let top = rect_desc
                .items
                .get("Top ")
                .and_then(|v| {
                    if let DescriptorValue::Double(d) = v {
                        Some(*d)
                    } else {
                        None
                    }
                })
                .unwrap_or(0.0);
            let left = rect_desc
                .items
                .get("Left")
                .and_then(|v| {
                    if let DescriptorValue::Double(d) = v {
                        Some(*d)
                    } else {
                        None
                    }
                })
                .unwrap_or(0.0);
            let bottom = rect_desc
                .items
                .get("Btom")
                .and_then(|v| {
                    if let DescriptorValue::Double(d) = v {
                        Some(*d)
                    } else {
                        None
                    }
                })
                .unwrap_or(0.0);
            let right = rect_desc
                .items
                .get("Rght")
                .and_then(|v| {
                    if let DescriptorValue::Double(d) = v {
                        Some(*d)
                    } else {
                        None
                    }
                })
                .unwrap_or(0.0);

            Bounds {
                top,
                left,
                bottom,
                right,
            }
        } else {
            Bounds {
                top: 0.0,
                left: 0.0,
                bottom: 0.0,
                right: 0.0,
            }
        };

        info.artboard = Some(ArtboardData {
            rect,
            preset_name: None,
            color: None,
            background_type: None,
        });

        Ok(())
    }

    /// Read using aligned rendering (sn2P)
    fn read_using_aligned_rendering(&mut self, info: &mut LayerAdditionalInfo) -> Result<()> {
        let record: UsingAlignedRenderingRecord =
            decode_be(&self.read_bytes(4)?, "using aligned rendering")?;
        info.using_aligned_rendering = Some(record.value != 0);
        Ok(())
    }

    /// Read metadata (shmd)
    fn read_metadata(&mut self, info: &mut LayerAdditionalInfo, _length: usize) -> Result<()> {
        let count = self.read_u32()?;
        let mut entries = Vec::with_capacity(count as usize);
        for _ in 0..count {
            let _sig = self.read_signature()?; // "8BIM"
            let key = self.read_signature()?;
            let copy_on_sheet_change = self.read_u8()? != 0;
            self.skip_bytes(3)?;
            let data_length = self.read_u32()? as usize;
            let raw = self.read_bytes(data_length)?;
            // Each entry is a versioned descriptor when the first u32 == 16
            let descriptor = if raw.len() >= 4 {
                let ver = u32::from_be_bytes([raw[0], raw[1], raw[2], raw[3]]);
                if ver == 16 {
                    let mut sub =
                        PsdReader::new(std::io::Cursor::new(&raw[4..]), Default::default());
                    sub.read_descriptor_structure().ok()
                } else {
                    None
                }
            } else {
                None
            };
            let has_descriptor = descriptor.is_some();
            entries.push(MetadataEntry {
                key,
                copy_on_sheet_change,
                descriptor,
                raw_data: if has_descriptor { Vec::new() } else { raw },
            });
        }
        info.metadata = Some(Metadata { entries });
        Ok(())
    }
}

impl PsdWriter {
    fn units_to_descriptor(value: &UnitsValue) -> DescriptorValue {
        let units = match value.units {
            Units::Pixels => "Pixels",
            Units::Points => "Points",
            Units::Millimeters => "Millimeters",
            Units::Centimeters => "Centimeters",
            Units::Inches => "Inches",
            Units::Density => "Density",
            _ => "None",
        };
        DescriptorValue::UnitDouble {
            units: units.to_string(),
            value: value.value,
        }
    }

    fn descriptor_from_bounds(bounds: &UnitsBounds) -> Descriptor {
        let mut items = HashMap::new();
        items.insert("Top".to_string(), Self::units_to_descriptor(&bounds.top));
        items.insert("Left".to_string(), Self::units_to_descriptor(&bounds.left));
        items.insert("Rght".to_string(), Self::units_to_descriptor(&bounds.right));
        items.insert(
            "Btom".to_string(),
            Self::units_to_descriptor(&bounds.bottom),
        );
        Descriptor {
            name: String::new(),
            class_id: "null".to_string(),
            items,
        }
    }

    fn descriptor_from_rrect(rrect: &RRectRadii) -> Descriptor {
        let mut items = HashMap::new();
        items.insert(
            "topRight".to_string(),
            Self::units_to_descriptor(&rrect.top_right),
        );
        items.insert(
            "topLeft".to_string(),
            Self::units_to_descriptor(&rrect.top_left),
        );
        items.insert(
            "bottomLeft".to_string(),
            Self::units_to_descriptor(&rrect.bottom_left),
        );
        items.insert(
            "bottomRight".to_string(),
            Self::units_to_descriptor(&rrect.bottom_right),
        );
        Descriptor {
            name: String::new(),
            class_id: "null".to_string(),
            items,
        }
    }

    fn split_pattern_buffer(
        mode: u32,
        buffer: &[u8],
        width: usize,
        height: usize,
    ) -> (Vec<Vec<u8>>, Vec<u8>) {
        let pixel_count = width * height;
        let mut alpha = vec![0u8; pixel_count];
        match mode {
            1 => {
                let mut gray = vec![0u8; pixel_count];
                for index in 0..pixel_count {
                    gray[index] = *buffer.get(index * 4).unwrap_or(&0);
                    alpha[index] = *buffer.get(index * 4 + 3).unwrap_or(&255);
                }
                (vec![gray], alpha)
            }
            2 => {
                let mut indexed = vec![0u8; pixel_count];
                for index in 0..pixel_count {
                    indexed[index] = *buffer.get(index * 4).unwrap_or(&0);
                    alpha[index] = *buffer.get(index * 4 + 3).unwrap_or(&255);
                }
                (vec![indexed], alpha)
            }
            _ => {
                let mut red = vec![0u8; pixel_count];
                let mut green = vec![0u8; pixel_count];
                let mut blue = vec![0u8; pixel_count];
                for index in 0..pixel_count {
                    red[index] = *buffer.get(index * 4).unwrap_or(&0);
                    green[index] = *buffer.get(index * 4 + 1).unwrap_or(&0);
                    blue[index] = *buffer.get(index * 4 + 2).unwrap_or(&0);
                    alpha[index] = *buffer.get(index * 4 + 3).unwrap_or(&255);
                }
                (vec![red, green, blue], alpha)
            }
        }
    }

    fn write_pattern_entry(&mut self, entry: &PatternBlockEntry) -> Result<()> {
        let width = entry.width as usize;
        let height = entry.height as usize;
        self.write_u32(entry.version)?;
        self.write_u32(entry.mode)?;
        self.write_u16(entry.height)?;
        self.write_u16(entry.width)?;
        self.write_unicode_string_with_padding(&entry.name)?;
        self.write_pascal_string(&entry.id, 1)?;
        if entry.mode == 2 {
            let default_palette = vec![RGB { r: 0, g: 0, b: 0 }; 256];
            let palette_bytes = palette_colors_to_bytes(
                entry
                    .indexed_palette
                    .as_deref()
                    .unwrap_or(default_palette.as_slice()),
            );
            self.write_bytes(&palette_bytes)?;
            self.write_u32(0)?;
        }
        let buffer = entry
            .rgba
            .clone()
            .map(|pixel_data| pixel_data.data)
            .unwrap_or_else(|| vec![0; width * height * 4]);
        let (channels, alpha) = Self::split_pattern_buffer(entry.mode, &buffer, width, height);
        self.write_u32(3)?;
        self.write_u32(0)?;
        self.write_i32(0)?;
        self.write_i32(0)?;
        self.write_i32(width as i32)?;
        self.write_i32(height as i32)?;
        self.write_u32(24)?;
        for channel_index in 0..26 {
            let include = channel_index < channels.len() || channel_index == 25;
            self.write_u32(u32::from(include))?;
            if !include {
                continue;
            }
            let channel_data = if channel_index < channels.len() {
                &channels[channel_index]
            } else {
                &alpha
            };
            let mut channel_writer = PsdWriter::new(channel_data.len() + 128);
            channel_writer.write_u32(8)?;
            channel_writer.write_i32(0)?;
            channel_writer.write_i32(0)?;
            channel_writer.write_i32(width as i32)?;
            channel_writer.write_i32(height as i32)?;
            channel_writer.write_u16(1)?;
            channel_writer.write_u8(1)?;
            let encoded = compression::compress_rle(channel_data, width, height, false)?;
            channel_writer.write_bytes(&encoded)?;
            let bytes = channel_writer.into_buffer();
            self.write_u32(bytes.len() as u32)?;
            self.write_bytes(&bytes)?;
        }
        Ok(())
    }

    /// Write layer additional info section
    pub fn write_additional_info(
        &mut self,
        key: &str,
        info: &LayerAdditionalInfo,
    ) -> Result<usize> {
        let mut temp_writer = PsdWriter::new(1024);

        match key {
            "luni" => {
                if let Some(ref name) = info.name {
                    temp_writer.write_unicode_string(name)?;
                }
            }
            "lyid" => {
                if let Some(id) = info.id {
                    temp_writer.write_u32(id as u32)?;
                }
            }
            "lclr" => {
                if let Some(color) = info.layer_color {
                    let color_value = match color {
                        crate::api::types::LayerColor::None => 0,
                        crate::api::types::LayerColor::Red => 1,
                        crate::api::types::LayerColor::Orange => 2,
                        crate::api::types::LayerColor::Yellow => 3,
                        crate::api::types::LayerColor::Green => 4,
                        crate::api::types::LayerColor::Blue => 5,
                        crate::api::types::LayerColor::Violet => 6,
                        crate::api::types::LayerColor::Gray => 7,
                    };
                    temp_writer.write_bytes(&encode_be(
                        &LayerColorRecord {
                            color_value,
                            padding: [0; 6],
                        },
                        "layer color",
                    )?)?;
                }
            }
            "iOpa" => {
                if let Some(value) = info.fill_opacity {
                    temp_writer.write_u8(value)?;
                    temp_writer.write_zeros(3)?; // pad to 4 bytes
                }
            }
            "lsct" => {
                if let Some(ref divider) = info.section_divider {
                    temp_writer.write_bytes(&encode_be(
                        &SectionDividerBaseRecord {
                            divider_type: divider.divider_type as u32,
                        },
                        "section divider",
                    )?)?;
                    if let Some(ref blend_mode) = divider.blend_mode {
                        let blend_mode_code = from_blend_mode(*blend_mode);
                        let blend_mode_bytes = blend_mode_code.as_bytes();
                        if blend_mode_bytes.len() != 4 {
                            return Err(PsdError::InvalidFormat(
                                "Invalid section divider blend mode".to_string(),
                            ));
                        }
                        let mut raw = [0u8; 4];
                        raw.copy_from_slice(blend_mode_bytes);
                        temp_writer.write_bytes(&encode_be(
                            &SectionDividerExtendedRecord {
                                signature: *b"8BIM",
                                blend_mode: raw,
                            },
                            "section divider extended",
                        )?)?;
                    }
                    if let Some(sub_type) = divider.sub_type {
                        temp_writer.write_bytes(&encode_be(
                            &U32ValueRecord { value: sub_type.0 },
                            "section divider subtype",
                        )?)?;
                    }
                }
            }
            "clbl" => {
                if let Some(blend_clipped) = info.blend_clipped_elements {
                    temp_writer.write_bytes(&encode_be(
                        &U8BoolRecord {
                            value: u8::from(blend_clipped),
                        },
                        "blend clipped",
                    )?)?;
                    temp_writer.write_zeros(3)?; // pad to 4 bytes
                }
            }
            "infx" => {
                if let Some(blend_interior) = info.blend_interior_elements {
                    temp_writer.write_bytes(&encode_be(
                        &U8BoolRecord {
                            value: u8::from(blend_interior),
                        },
                        "blend interior",
                    )?)?;
                    temp_writer.write_zeros(3)?; // pad to 4 bytes
                }
            }
            "knko" => {
                if let Some(knockout) = info.knockout {
                    temp_writer.write_bytes(&encode_be(
                        &U8BoolRecord {
                            value: u8::from(knockout),
                        },
                        "knockout",
                    )?)?;
                    temp_writer.write_zeros(3)?; // pad to 4 bytes
                }
            }
            "lspf" => {
                if let Some(ref protected) = info.protected {
                    let mut flags = ProtectedFlagsBits::empty();
                    if protected.transparency {
                        flags |= ProtectedFlagsBits::TRANSPARENCY;
                    }
                    if protected.composite {
                        flags |= ProtectedFlagsBits::COMPOSITE;
                    }
                    if protected.position {
                        flags |= ProtectedFlagsBits::POSITION;
                    }
                    if protected.artboards.unwrap_or(false) {
                        flags |= ProtectedFlagsBits::ARTBOARDS;
                    }
                    temp_writer.write_bytes(&encode_be(
                        &ProtectedFlagsRecord {
                            flags: flags.bits(),
                        },
                        "protected flags",
                    )?)?;
                }
            }
            "lnsr" => {
                if let Some(ref source) = info.name_source {
                    let source_bytes = source.as_bytes();
                    if source_bytes.len() != 4 {
                        return Err(PsdError::InvalidFormat(
                            "Invalid name source signature".to_string(),
                        ));
                    }
                    let mut raw = [0u8; 4];
                    raw.copy_from_slice(source_bytes);
                    temp_writer.write_bytes(&encode_be(
                        &NameSourceRecord { signature: raw },
                        "name source",
                    )?)?;
                }
            }
            "lyvr" => {
                if let Some(value) = info.layer_version {
                    temp_writer.write_u32(value)?;
                }
            }
            "lmgm" => {
                if let Some(value) = info.lmgm {
                    temp_writer.write_u8(value)?;
                    temp_writer.write_zeros(3)?; // pad to 4 bytes
                }
            }
            "vmgm" => {
                if let Some(value) = info.vmgm {
                    temp_writer.write_u8(value)?;
                    temp_writer.write_zeros(3)?; // pad to 4 bytes
                }
            }
            "fcmy" => {
                if let Some(value) = info.fcmy {
                    temp_writer.write_u8(value)?;
                    temp_writer.write_zeros(3)?; // pad to 4 bytes
                }
            }
            "brst" => {
                if let Some(restrictions) = info.blending_restrictions {
                    for (index, value) in restrictions.into_iter().enumerate() {
                        if value == 0 {
                            temp_writer.write_u32(index as u32)?;
                        }
                    }
                }
            }
            "fxrp" => {
                if let Some(point) = info.reference_point {
                    temp_writer.write_f64(point.x)?;
                    temp_writer.write_f64(point.y)?;
                }
            }
            "shmd" => {
                if let Some(ref metadata) = info.metadata {
                    temp_writer.write_u32(metadata.entries.len() as u32)?;
                    for entry in &metadata.entries {
                        temp_writer.write_signature("8BIM")?;
                        temp_writer.write_signature(&entry.key)?;
                        temp_writer.write_u8(if entry.copy_on_sheet_change { 1 } else { 0 })?;
                        temp_writer.write_zeros(3)?;
                        // Write descriptor data or raw data
                        if let Some(ref desc) = entry.descriptor {
                            let mut desc_writer = crate::io::writer::PsdWriter::new(256);
                            desc_writer.write_u32(16)?; // version
                            desc_writer.write_descriptor_structure(desc)?;
                            let desc_bytes = desc_writer.into_buffer();
                            temp_writer.write_u32(desc_bytes.len() as u32)?;
                            temp_writer.write_bytes(&desc_bytes)?;
                        } else {
                            temp_writer.write_u32(entry.raw_data.len() as u32)?;
                            temp_writer.write_bytes(&entry.raw_data)?;
                        }
                    }
                }
            }
            "TySh" => {
                if let Some(ref text) = info.text {
                    if let Some(raw) = text.raw_bytes.as_ref() {
                        temp_writer.write_bytes(raw)?;
                    } else {
                        temp_writer.write_i16(1)?; // version
                        for &v in &text.transform {
                            temp_writer.write_f64(v)?;
                        }
                        temp_writer.write_i16(50)?; // text version
                        if let Some(ref td) = text.text_data {
                            temp_writer.write_version_and_descriptor(text.descriptor_version, td)?;
                        }
                        temp_writer.write_i16(text.warp_version as i16)?;
                        if let Some(ref wd) = text.warp_data {
                            temp_writer.write_version_and_descriptor(
                                text.warp_descriptor_version,
                                wd,
                            )?;
                        }
                        temp_writer.write_f32(text.left)?;
                        temp_writer.write_f32(text.top)?;
                        temp_writer.write_f32(text.right)?;
                        temp_writer.write_f32(text.bottom)?;
                    }
                }
            }
            "SoCo" | "GdFl" | "PtFl" => {
                if let Some(ref vf) = info.vector_fill {
                    temp_writer.write_u32(16)?;
                    temp_writer.write_descriptor_structure(&vf.data)?;
                }
            }
            "vscg" => {
                if let Some(ref vs) = info.vector_stroke {
                    temp_writer.write_signature("vstk")?; // wrapped key prefix
                    temp_writer.write_u32(vs.version)?;
                    temp_writer.write_descriptor_structure(&vs.descriptor)?;
                }
            }
            "vstk" => {
                if let Some(ref vs) = info.vector_stroke {
                    temp_writer.write_u32(vs.version)?;
                    temp_writer.write_descriptor_structure(&vs.descriptor)?;
                }
            }
            "vmsk" | "vsms" => {
                if let Some(ref vm) = info.vector_mask {
                    temp_writer.write_u32(vm.version)?;
                    let mut flags = VectorMaskFlagsBits::empty();
                    if vm.invert {
                        flags |= VectorMaskFlagsBits::INVERT;
                    }
                    if vm.not_link {
                        flags |= VectorMaskFlagsBits::NOT_LINK;
                    }
                    if vm.disable {
                        flags |= VectorMaskFlagsBits::DISABLE;
                    }
                    temp_writer.write_u32(flags.bits())?;
                    temp_writer.write_u16(6)?;
                    temp_writer.write_zeros(24)?;
                    if let Some(initial_fill_rule) =
                        vm.paths.first().and_then(|path| path.initial_fill_rule)
                    {
                        temp_writer.write_u16(8)?;
                        temp_writer.write_u16(initial_fill_rule)?;
                        temp_writer.write_zeros(22)?;
                    }
                    if let Some(bounds) = vm.paths.first().and_then(|path| path.clipboard_bounds) {
                        temp_writer.write_u16(7)?;
                        temp_writer.write_fixed_point_path_32(bounds.top)?;
                        temp_writer.write_fixed_point_path_32(bounds.left)?;
                        temp_writer.write_fixed_point_path_32(bounds.bottom)?;
                        temp_writer.write_fixed_point_path_32(bounds.right)?;
                        temp_writer.write_fixed_point_path_32(0.0)?;
                    }
                    for path in &vm.paths {
                        // Subpath length record
                        let selector = match path.path_type {
                            PathType::Closed => 0u16,
                            PathType::Open => 3u16,
                        };
                        temp_writer.write_u16(selector)?;
                        temp_writer.write_u16(path.points.len() as u16)?;
                        temp_writer.write_u16(1)?;
                        temp_writer.write_u16(1)?;
                        temp_writer.write_zeros(18)?;
                        // Bezier knot records
                        for point in &path.points {
                            temp_writer.write_u16(if point.linked { 1 } else { 2 })?;
                            temp_writer.write_fixed_point_path_32(point.backward.y)?;
                            temp_writer.write_fixed_point_path_32(point.backward.x)?;
                            temp_writer.write_fixed_point_path_32(point.anchor.y)?;
                            temp_writer.write_fixed_point_path_32(point.anchor.x)?;
                            temp_writer.write_fixed_point_path_32(point.forward.y)?;
                            temp_writer.write_fixed_point_path_32(point.forward.x)?;
                        }
                    }
                    while temp_writer.offset % 4 != 0 {
                        temp_writer.write_u8(0)?;
                    }
                }
            }
            "vogk" => {
                if let Some(ref data) = info.vector_origination {
                    fn write_descriptor_entry(
                        writer: &mut PsdWriter,
                        key: &str,
                        value: &DescriptorValue,
                    ) -> Result<()> {
                        let sig = match value {
                            DescriptorValue::Boolean(_) => "bool",
                            DescriptorValue::Integer(_) => "long",
                            DescriptorValue::Double(_) => "doub",
                            DescriptorValue::Float(_) => "DBL ",
                            DescriptorValue::Descriptor(_) => "Objc",
                            DescriptorValue::GlobalObject(_) => "GlbO",
                            DescriptorValue::List(_) => "VlLs",
                            DescriptorValue::UnitDouble { .. } => "UntF",
                            DescriptorValue::UnitFloat { .. } => "UnFl",
                            DescriptorValue::Text(_) => "TEXT",
                            DescriptorValue::Enum { .. } => "Enmr",
                            DescriptorValue::Class { .. } => "type",
                            DescriptorValue::Reference(_) => "obj ",
                            DescriptorValue::DataBytes(_) => "tdta",
                            DescriptorValue::Property(_) => "prop",
                            DescriptorValue::LargeInteger { .. } => "comp",
                            DescriptorValue::Alias(_) => "alis",
                            DescriptorValue::FilePath { .. } => "Pth ",
                            DescriptorValue::ObjectArray { .. } => "ObAr",
                        };
                        writer.write_ascii_string_or_class_id(key)?;
                        writer.write_signature(sig)?;
                        writer.write_ostype(value)
                    }

                    temp_writer.write_u32(1)?;
                    temp_writer.write_u32(16)?;
                    temp_writer.write_class_structure("", "null")?;
                    temp_writer.write_u32(1)?;
                    temp_writer.write_ascii_string_or_class_id("keyDescriptorList")?;
                    temp_writer.write_signature("VlLs")?;
                    temp_writer.write_u32(data.key_descriptor_list.len() as u32)?;
                    for item in &data.key_descriptor_list {
                        temp_writer.write_signature("Objc")?;
                        temp_writer.write_class_structure("", "null")?;
                        let mut entries: Vec<(&str, DescriptorValue)> = Vec::new();
                        if let Some(v) = item.key_shape_invalidated {
                            entries.push(("keyShapeInvalidated", DescriptorValue::Boolean(v)));
                        }
                        if let Some(v) = item.key_origin_index {
                            entries.push(("keyOriginIndex", DescriptorValue::Integer(v.0)));
                        }
                        if let Some(v) = item.key_origin_type {
                            entries.push(("keyOriginType", DescriptorValue::Integer(v.0)));
                        }
                        if let Some(v) = item.key_origin_resolution {
                            entries.push(("keyOriginResolution", DescriptorValue::Double(v)));
                        }
                        if let Some(ref v) = item.key_origin_rrect_radii {
                            entries.push((
                                "keyOriginRRectRadii",
                                DescriptorValue::Descriptor(Self::descriptor_from_rrect(v)),
                            ));
                        }
                        if let Some(ref v) = item.key_origin_shape_bounding_box {
                            entries.push((
                                "keyOriginShapeBBox",
                                DescriptorValue::Descriptor(Self::descriptor_from_bounds(v)),
                            ));
                        }
                        if let Some(ref points) = item.key_origin_box_corners {
                            let mut point_values = Vec::new();
                            for point in points {
                                point_values.push(DescriptorValue::Double(point.x));
                                point_values.push(DescriptorValue::Double(point.y));
                            }
                            let mut point_desc_items = HashMap::new();
                            point_desc_items
                                .insert("points".to_string(), DescriptorValue::List(point_values));
                            entries.push((
                                "keyOriginBoxCorners",
                                DescriptorValue::Descriptor(Descriptor {
                                    name: String::new(),
                                    class_id: "null".to_string(),
                                    items: point_desc_items,
                                }),
                            ));
                        }
                        if let Some(ref transform) = item.transform {
                            entries.push((
                                "transform",
                                DescriptorValue::List(
                                    transform
                                        .iter()
                                        .copied()
                                        .map(DescriptorValue::Double)
                                        .collect(),
                                ),
                            ));
                        }
                        temp_writer.write_u32(entries.len() as u32)?;
                        for (key, value) in &entries {
                            write_descriptor_entry(&mut temp_writer, key, value)?;
                        }
                    }
                }
            }
            "lrFX" | "lfx2" => {
                if let Some(ref le) = info.effects {
                    if key == "lfx2" && le.descriptor.is_none() {
                        return Ok(0);
                    }
                    temp_writer.write_u32(le.version)?;
                    if key == "lfx2" {
                        if let Some(ref desc) = le.descriptor {
                            temp_writer.write_version_and_descriptor(16, desc)?;
                        }
                    }
                }
            }
            "PlLd" => {
                if let Some(ref pl) = info.placed_layer {
                    fn write_legacy_plld_entry(
                        writer: &mut PsdWriter,
                        key: &str,
                        value: &DescriptorValue,
                    ) -> Result<()> {
                        writer.write_ascii_string_or_class_id(key)?;
                        match value {
                            DescriptorValue::Enum { enum_type, value } => {
                                writer.write_signature("enum")?;
                                writer.write_ascii_string_or_class_id(enum_type)?;
                                writer.write_ascii_string_or_class_id(value)
                            }
                            _ => {
                                let sig = match value {
                                    DescriptorValue::Boolean(_) => "bool",
                                    DescriptorValue::Integer(_) => "long",
                                    DescriptorValue::Double(_) => "doub",
                                    DescriptorValue::Float(_) => "DBL ",
                                    DescriptorValue::Descriptor(_) => "Objc",
                                    DescriptorValue::GlobalObject(_) => "GlbO",
                                    DescriptorValue::List(_) => "VlLs",
                                    DescriptorValue::UnitDouble { .. } => "UntF",
                                    DescriptorValue::UnitFloat { .. } => "UnFl",
                                    DescriptorValue::Text(_) => "TEXT",
                                    DescriptorValue::Enum { .. } => unreachable!(),
                                    DescriptorValue::Class { .. } => "type",
                                    DescriptorValue::Reference(_) => "obj ",
                                    DescriptorValue::DataBytes(_) => "tdta",
                                    DescriptorValue::Property(_) => "prop",
                                    DescriptorValue::LargeInteger { .. } => "comp",
                                    DescriptorValue::Alias(_) => "alis",
                                    DescriptorValue::FilePath { .. } => "Pth ",
                                    DescriptorValue::ObjectArray { .. } => "ObAr",
                                };
                                writer.write_signature(sig)?;
                                writer.write_ostype(value)
                            }
                        }
                    }

                    fn write_plld_warp_descriptor(
                        writer: &mut PsdWriter,
                        desc: &Descriptor,
                    ) -> Result<()> {
                        use std::collections::HashSet;

                        writer.write_class_structure(&desc.name, &desc.class_id)?;
                        let ordered_keys = [
                            "warpStyle",
                            "warpValue",
                            "warpPerspective",
                            "warpPerspectiveOther",
                            "warpRotate",
                            "bounds",
                            "uOrder",
                            "vOrder",
                            "customEnvelopeWarp",
                        ];
                        writer.write_u32(desc.items.len() as u32)?;
                        let mut written = HashSet::new();
                        for key in ordered_keys {
                            if let Some(value) = desc.items.get(key) {
                                written.insert(key);
                                if key == "bounds" {
                                    if let DescriptorValue::Descriptor(bounds) = value {
                                        writer.write_ascii_string_or_class_id(key)?;
                                        writer.write_signature("Objc")?;
                                        writer.write_class_structure(&bounds.name, &bounds.class_id)?;
                                        writer.write_u32(bounds.items.len() as u32)?;
                                        for bounds_key in ["Top ", "Left", "Btom", "Rght"] {
                                            if let Some(bounds_value) = bounds.items.get(bounds_key) {
                                                write_legacy_plld_entry(
                                                    writer,
                                                    bounds_key,
                                                    bounds_value,
                                                )?;
                                            }
                                        }
                                        continue;
                                    }
                                }
                                write_legacy_plld_entry(writer, key, value)?;
                            }
                        }
                        let mut remaining_keys: Vec<_> = desc
                            .items
                            .keys()
                            .filter(|key| !written.contains(key.as_str()))
                            .collect();
                        remaining_keys.sort();
                        for key in remaining_keys {
                            let value = &desc.items[key];
                            write_legacy_plld_entry(writer, key, value)?;
                        }
                        Ok(())
                    }

                    if pl.transform.is_empty() {
                        return Ok(0);
                    }
                    temp_writer.write_signature("plcL")?;
                    temp_writer.write_u32(3)?; // version
                                               // UUID as pascal string (u8 length + bytes)
                    let id_bytes = pl.id.as_bytes();
                    temp_writer.write_u8(id_bytes.len() as u8)?;
                    temp_writer.write_bytes(id_bytes)?;
                    // Match the TS writer's legacy constants.
                    temp_writer.write_u32(1)?;
                    temp_writer.write_u32(1)?;
                    temp_writer.write_u32(16)?;
                    temp_writer.write_u32(2)?;
                    // transform (8×f64)
                    for &v in &pl.transform {
                        temp_writer.write_f64(v)?;
                    }
                    temp_writer.write_u32(0)?;
                    temp_writer.write_u32(16)?;
                    // Legacy PlLd stores a raw descriptor here, not version+descriptor.
                    if let Some(ref warp) = pl.legacy_warp {
                        write_plld_warp_descriptor(&mut temp_writer, warp)?;
                    }
                    while temp_writer.offset % 4 != 0 {
                        temp_writer.write_u8(0)?;
                    }
                }
            }
            "SoLd" => {
                if let Some(ref pl) = info.placed_layer {
                    temp_writer.write_signature("soLD")?;
                    temp_writer.write_u32(4)?;
                    temp_writer.write_u32(16)?;
                    if let Some(ref descriptor) =
                        pl.sold_descriptor.as_ref().or(pl.legacy_warp.as_ref())
                    {
                        temp_writer.write_descriptor_structure(descriptor)?;
                    }
                }
            }
            "artb" | "artd" => {
                if let Some(ref ab) = info.artboard {
                    use crate::support::descriptor::{Descriptor, DescriptorValue};
                    let mut rect_desc = Descriptor {
                        name: String::new(),
                        class_id: "Rct1".to_string(),
                        items: std::collections::HashMap::new(),
                    };
                    rect_desc
                        .items
                        .insert("Top ".to_string(), DescriptorValue::Double(ab.rect.top));
                    rect_desc
                        .items
                        .insert("Left".to_string(), DescriptorValue::Double(ab.rect.left));
                    rect_desc
                        .items
                        .insert("Btom".to_string(), DescriptorValue::Double(ab.rect.bottom));
                    rect_desc
                        .items
                        .insert("Rght".to_string(), DescriptorValue::Double(ab.rect.right));
                    let mut desc = Descriptor {
                        name: String::new(),
                        class_id: "artd".to_string(),
                        items: std::collections::HashMap::new(),
                    };
                    desc.items.insert(
                        "artboardRect".to_string(),
                        DescriptorValue::Descriptor(rect_desc),
                    );
                    temp_writer.write_u32(16)?;
                    temp_writer.write_descriptor_structure(&desc)?;
                }
            }
            "sn2P" => {
                if let Some(using) = info.using_aligned_rendering {
                    temp_writer.write_bytes(&encode_be(
                        &UsingAlignedRenderingRecord {
                            value: if using { 1 } else { 0 },
                        },
                        "using aligned rendering",
                    )?)?;
                }
            }
            "brit" | "levl" | "curv" | "expA" | "blnc" | "phfl" | "hue2" | "selc" | "mixr"
            | "post" | "thrs" | "nvrt" | "grdm" | "blwh" => {
                if let Some(ref adj) = info.adjustment {
                    if adj.key() == key {
                        let data = adj
                            .to_bytes()
                            .map_err(|e| PsdError::InvalidFormat(e.to_string()))?;
                        temp_writer.write_bytes(&data)?;
                    }
                }
            }
            "Lr16" | "Lr32" => {
                if let Some(ref block) = info.high_depth_layer_data {
                    if block.key.as_ref() == key {
                        let bits = if key == "Lr16" { 16 } else { 32 };
                        crate::io::writer::write_nested_layer_info_block(
                            &mut temp_writer,
                            &block.layers,
                            bits,
                        )?;
                    }
                }
            }
            "lnk2" | "lnkD" | "lnkD__" | "lnk3" => {
                if let Some(ref block) = info.linked_files {
                    if block.key.as_ref() != key {
                        return Ok(0);
                    }
                    for item in &block.items {
                        validate_linked_file_item(item)?;
                        let mut item_writer = PsdWriter::new(256);
                        let kind = item.data_kind.unwrap_or(LinkedFileDataKind::Data);
                        let version = item.item_version.unwrap_or(7);
                        item_writer.write_signature(std::str::from_utf8(&kind.to_code()).unwrap())?;
                        item_writer.write_u32(version)?;
                        item_writer.write_pascal_string(&item.id, 1)?;
                        item_writer.write_unicode_string_with_padding(&item.name)?;
                        item_writer.write_signature(
                            item.file_type
                                .as_ref()
                                .map(|v| v.as_ref())
                                .unwrap_or("    "),
                        )?;
                        item_writer.write_signature(
                            item.creator.as_ref().map(|v| v.as_ref()).unwrap_or("    "),
                        )?;
                        let data = item.data.as_deref().unwrap_or(&[]);
                        write_u64_parts(&mut item_writer, data.len() as u64)?;
                        if let Some(ref open_descriptor) = item.open_descriptor {
                            item_writer.write_u8(1)?;
                            item_writer.write_u32(16)?;
                            let descriptor_start = item_writer.offset;
                            item_writer.write_descriptor_structure(open_descriptor)?;
                            if (item_writer.offset - descriptor_start) % 2 != 0 {
                                item_writer.write_u8(0)?;
                            }
                        } else {
                            item_writer.write_u8(0)?;
                        }
                        if kind == LinkedFileDataKind::External {
                            if let Some(ref descriptor) = item.descriptor {
                                item_writer.write_descriptor_structure(descriptor)?;
                            }
                            if version > 3 {
                                if let Some((year, month, day, hour, minute, second)) =
                                    item.time.as_deref().and_then(parse_linked_file_time)
                                {
                                    item_writer.write_u32(year)?;
                                    item_writer.write_u8(month)?;
                                    item_writer.write_u8(day)?;
                                    item_writer.write_u8(hour)?;
                                    item_writer.write_u8(minute)?;
                                    item_writer.write_f64(second)?;
                                }
                            }
                            write_u64_parts(
                                &mut item_writer,
                                item.linked_file
                                    .as_ref()
                                    .map(|linked_file| linked_file.file_size)
                                    .unwrap_or_default(),
                            )?;
                        } else if kind == LinkedFileDataKind::Alias {
                            write_u64_parts(&mut item_writer, 0)?;
                        }
                        item_writer.write_bytes(data)?;
                        if version >= 5 {
                            if let Some(child_document_id) = item.child_document_id.as_ref() {
                                item_writer.write_unicode_string_with_padding(child_document_id)?;
                            }
                        }
                        if version >= 6 {
                            if let Some(asset_mod_time) = item.asset_mod_time {
                                item_writer.write_f64(asset_mod_time)?;
                            }
                        }
                        if version >= 7 {
                            if let Some(asset_locked_state) = item.asset_locked_state {
                                item_writer.write_u8(asset_locked_state)?;
                            }
                        }
                        if version >= 7 && kind == LinkedFileDataKind::External {
                            if let Some(linked_file) = item.linked_file.as_ref() {
                                if linked_file_uses_versioned_paths(linked_file) {
                                    item_writer
                                        .write_unicode_string_with_padding(&linked_file.name)?;
                                    item_writer.write_unicode_string_with_padding(
                                        &linked_file.full_path,
                                    )?;
                                    item_writer.write_unicode_string_with_padding(
                                        &linked_file.original_path,
                                    )?;
                                    item_writer.write_unicode_string_with_padding(
                                        &linked_file.relative_path,
                                    )?;
                                }
                            }
                        }
                        let item_bytes = item_writer.into_buffer();
                        write_u64_parts(&mut temp_writer, item_bytes.len() as u64)?;
                        temp_writer.write_bytes(&item_bytes)?;
                        let padding = item_bytes.len() % 4;
                        if padding != 0 {
                            temp_writer.write_zeros(4 - padding)?;
                        }
                    }
                }
            }
            "lmfx" | "lfxs" => {
                if let Some(ref desc) = info.layer_effects_descriptor {
                    temp_writer.write_u32(0)?; // version
                    temp_writer.write_version_and_descriptor(16, desc)?;
                }
            }
            "FMsk" => {
                if let Some(ref data) = info.filter_mask {
                    temp_writer.write_bytes(&data.bytes)?;
                }
            }
            "shpa" => {
                if let Some(ref shape) = info.shape_pattern {
                    temp_writer.write_u32(shape.version)?;
                    temp_writer.write_u32(shape.present_count)?;
                }
            }
            "clrL" | "rplc" => {
                if let Some(desc) = info.descriptor_blocks.get(key) {
                    temp_writer.write_u16(1)?;
                    temp_writer.write_u32(16)?;
                    temp_writer.write_descriptor_structure(desc)?;
                }
            }
            "pths" | "CgEd" | "vibA" | "PxSc" | "phry" => {
                if let Some(desc) = info.descriptor_blocks.get(key) {
                    temp_writer.write_u32(16)?;
                    temp_writer.write_descriptor_structure(desc)?;
                }
            }
            "Patt" | "Pat2" | "Pat3" => {
                if let Some(ref block) = info.pattern_data {
                    if block.key.as_ref() == key {
                        for pattern in &block.patterns {
                            let mut pattern_writer = PsdWriter::new(1024);
                            pattern_writer.write_pattern_entry(pattern)?;
                            let bytes = pattern_writer.into_buffer();
                            temp_writer.write_u32(bytes.len() as u32)?;
                            temp_writer.write_bytes(&bytes)?;
                            let remainder = bytes.len() % 4;
                            if remainder != 0 {
                                temp_writer.write_zeros(4 - remainder)?;
                            }
                        }
                    }
                }
            }
            "Txt2" => {
                if let Some(ref text_engine) = info.text_engine {
                    if let Some(ref raw) = text_engine.raw {
                        temp_writer.write_bytes(raw)?;
                    } else {
                        let bytes =
                            crate::support::engine_data::serialize_engine_data(&text_engine.data, true)
                                .map_err(|e| PsdError::InvalidFormat(e.to_string()))?;
                        temp_writer.write_bytes(&bytes)?;
                    }
                }
            }
            "LMsk" | "Mtrn" | "Mt16" | "Mt32" | "FXid" | "abdd" | "anFX" | "cinf" | "SoLE" => {
                if let Some(raw) = first_raw_block(info, key) {
                    temp_writer.write_bytes(raw)?;
                }
            }
            "Anno" => {
                if let Some(ref annotations) = info.annotations {
                    temp_writer.write_bytes(&encode_be(
                        &AnnotationHeaderRecord {
                            major: 2,
                            minor: 1,
                            count: annotations.len() as u32,
                        },
                        "annotation header",
                    )?)?;
                    for item in annotations {
                        let mut item_writer = PsdWriter::new(256);
                        item_writer.write_signature("txtA")?;
                        item_writer.write_u8(1)?;
                        item_writer.write_u8(28)?;
                        item_writer.write_u16(1)?;
                        item_writer.write_i32(item.x)?;
                        item_writer.write_i32(item.y)?;
                        item_writer.write_i32(17)?;
                        item_writer.write_i32(21)?;
                        item_writer.write_i32(item.x + 8)?;
                        item_writer.write_i32(item.y + 10)?;
                        item_writer.write_i32(241)?;
                        item_writer.write_i32(141)?;
                        // Write color: 8 bytes (colorSpace + 3 scaled channels + padding)
                        item_writer.write_u16(0)?;
                        item_writer
                            .write_u16(((item.color_l as u32 * 65535 + 127) / 255) as u16)?;
                        item_writer
                            .write_u16(((item.color_o as u32 * 65535 + 127) / 255) as u16)?;
                        item_writer
                            .write_u16(((item.color_c as u32 * 65535 + 127) / 255) as u16)?;
                        item_writer.write_u16(0)?;
                        // Strings use 4-byte length prefix format
                        write_pascal_string_with_padding(&mut item_writer, &item.author)?;
                        write_pascal_string_with_padding(&mut item_writer, "")?;
                        write_pascal_string_with_padding(
                            &mut item_writer,
                            "D:20211012120233+01'00'",
                        )?;
                        let text_units: Vec<u16> = item.text.encode_utf16().collect();
                        item_writer.write_u32((12 + 2 + text_units.len() * 2) as u32)?;
                        item_writer.write_signature("txtC")?;
                        item_writer.write_u32((2 + text_units.len() * 2) as u32)?;
                        item_writer.write_u8(254)?;
                        item_writer.write_u8(255)?;
                        for unit in text_units {
                            item_writer.write_u16(unit)?;
                        }
                        let bytes = item_writer.into_buffer();
                        temp_writer.write_u32(bytes.len() as u32)?;
                        temp_writer.write_bytes(&bytes)?;
                    }
                }
            }
            "FEid" => {
                if let Some(ref block) = info.filter_effects {
                    temp_writer.write_u32(block.version)?;
                    for item in &block.items {
                        let mut item_writer = PsdWriter::new(512);
                        item_writer.write_pascal_string(&item.id, 1)?;
                        item_writer.write_u32(item.version.unwrap_or(1))?;
                        // Write number of items in the extra block (always 8 for full items)
                        item_writer.write_u32(if item.rect.is_some() { 8 } else { 0 })?;
                        if let Some(ref rect) = item.rect {
                            item_writer.write_i32(rect.left)?;
                            item_writer.write_i32(rect.top)?;
                            item_writer.write_i32(rect.right)?;
                            item_writer.write_i32(rect.bottom)?;
                        }
                        if let Some(d) = item.depth {
                            item_writer.write_u32(d)?;
                        } else if item.rect.is_some() {
                            item_writer.write_u32(8)?;
                        }
                        let ch_count = item.channel_count.unwrap_or(24);
                        if item.rect.is_some() {
                            item_writer.write_u32(ch_count)?;
                        }
                        // Write slot presence flags and payloads
                        if let Some(ref slots) = item.slots {
                            let slot_map: std::collections::HashMap<u32, &ChannelImageData> =
                                slots.iter().map(|s| (s.slot, &s.channel_data)).collect();
                            for slot_index in 0..ch_count + 2 {
                                if let Some(slot_raw) = slot_map.get(&slot_index) {
                                    item_writer.write_u32(1)?;
                                    item_writer.write_u32(0)?;
                                    // 2-byte compression header (0x00 0x00 = raw/uncompressed) + data
                                    item_writer.write_u32(2 + slot_raw.data.len() as u32)?;
                                    item_writer.write_u16(0)?; // compression type: raw
                                    item_writer.write_bytes(&slot_raw.data)?;
                                } else {
                                    item_writer.write_u32(0)?;
                                }
                            }
                        }
                        // Write preview
                        if let Some(ref preview) = item.preview {
                            item_writer.write_u8(1)?;
                            item_writer.write_i32(preview.rect.left)?;
                            item_writer.write_i32(preview.rect.top)?;
                            item_writer.write_i32(preview.rect.right)?;
                            item_writer.write_i32(preview.rect.bottom)?;
                            item_writer.write_u32(0)?;
                            item_writer.write_u32(preview.channel_data.data.len() as u32)?;
                            item_writer.write_bytes(&preview.channel_data.data)?;
                        } else if item.rect.is_some() {
                            item_writer.write_u8(0)?;
                        }
                        let bytes = item_writer.into_buffer();
                        temp_writer.write_u32(0)?;
                        temp_writer.write_u32(bytes.len() as u32)?;
                        temp_writer.write_bytes(&bytes)?;
                        let remainder = bytes.len() % 4;
                        if remainder != 0 {
                            temp_writer.write_zeros(4 - remainder)?;
                        }
                    }
                }
            }
            "PxSD" => {
                if let Some(ref block) = info.pixel_source_data {
                    for item in &block.items {
                        let mut item_writer = PsdWriter::new(512);
                        item_writer.write_u32(item.key)?;
                        item_writer.write_u32(2)?;
                        // Reserved u32 + BigUint64 for nested length
                        item_writer.write_u32(2)?;
                        let _nested_len_offset = item_writer.get_buffer().len();
                        item_writer.write_u32(0)?; // BigUint64 high
                        item_writer.write_u32(0)?; // BigUint64 low
                        if let Some(ref images) = item.images {
                            item_writer.write_u32(images.len() as u32)?;
                            for image in images {
                                let mut img_writer = PsdWriter::new(256);
                                img_writer.write_u32(image.index)?;
                                if let Some(ref rect) = image.rect {
                                    img_writer.write_i32(rect.top)?;
                                    img_writer.write_i32(rect.left)?;
                                } else {
                                    img_writer.write_i32(0)?;
                                    img_writer.write_i32(0)?;
                                }
                                img_writer.write_u32(3)?;
                                img_writer.write_u32(3)?;
                                // Write 6 channels (RGBA + extra)
                                let rect = image.rect.unwrap_or(FilterEffectsRect {
                                    left: 0,
                                    top: 0,
                                    right: 0,
                                    bottom: 0,
                                });
                                for _channel_index in 0..6 {
                                    img_writer.write_u32(8)?;
                                    img_writer.write_i32(rect.left)?;
                                    img_writer.write_i32(rect.top)?;
                                    img_writer.write_i32(rect.right)?;
                                    img_writer.write_i32(rect.bottom)?;
                                    img_writer.write_u32(0)?;
                                    img_writer.write_u32(0)?;
                                }
                                let img_bytes = img_writer.into_buffer();
                                item_writer.write_u32(0)?;
                                item_writer.write_u32(img_bytes.len() as u32)?;
                                item_writer.write_bytes(&img_bytes)?;
                            }
                        } else {
                            item_writer.write_u32(0)?;
                        }
                        let bytes = item_writer.into_buffer();
                        temp_writer.write_u32(0)?;
                        temp_writer.write_u32(bytes.len() as u32)?;
                        temp_writer.write_bytes(&bytes)?;
                    }
                }
            }
            _ => {}
        }

        let data = temp_writer.get_buffer();
        let length = data.len();

        if length > 0 {
            self.write_bytes(data)?;
        }

        Ok(length)
    }
}

/// Read all additional info sections for a layer
pub fn read_layer_additional_info<R: Read + Seek>(
    reader: &mut PsdReader<R>,
    length: usize,
) -> Result<LayerAdditionalInfo> {
    let mut info = LayerAdditionalInfo::default();
    let start_offset = reader.offset;

    while (reader.offset - start_offset) < length as u64 {
        let signature = reader.read_signature()?;
        if signature != "8BIM" && signature != "8B64" {
            return Err(PsdError::InvalidFormat(format!(
                "Invalid additional info signature: {}",
                signature
            )));
        }

        let key = reader.read_signature()?;
        info.tagged_block_order.push(key.clone());
        let data_length = if signature == "8B64" {
            // Read 64-bit length
            let high = reader.read_u32()?;
            if high != 0 {
                return Err(PsdError::InvalidFormat(
                    "Resource size above 4GB".to_string(),
                ));
            }
            reader.read_u32()? as usize
        } else {
            reader.read_u32()? as usize
        };
        let block_start = reader.offset;
        reader.read_additional_info(&key, data_length, &mut info)?;

        // Handlers may under-read; the block length is authoritative.
        reader.seek_to(block_start + data_length as u64)?;

        let consumed = reader.offset.saturating_sub(start_offset);
        if consumed >= length as u64 {
            break;
        }
        let pad = (4 - (data_length % 4)) % 4;
        let remaining = (length as u64 - consumed) as usize;
        reader.skip_bytes(pad.min(remaining))?;
    }

    Ok(info)
}

fn tagged_block_uses_u64_length(key: &str, large: bool) -> bool {
    large
        && matches!(
            key,
            "LMsk"
                | "Lr16"
                | "Lr32"
                | "Layr"
                | "Mt16"
                | "Mt32"
                | "Mtrn"
                | "Alph"
                | "FMsk"
                | "lnk2"
                | "FEid"
                | "FXid"
                | "PxSD"
        )
}

fn write_tagged_block(
    writer: &mut PsdWriter,
    key: &str,
    data: &[u8],
    large: bool,
) -> Result<()> {
    let signature = if tagged_block_uses_u64_length(key, large) {
        "8B64"
    } else {
        "8BIM"
    };
    writer.write_signature(signature)?;
    writer.write_signature(key)?;
    if signature == "8B64" {
        writer.write_u32(0)?;
    }
    writer.write_u32(data.len() as u32)?;
    writer.write_bytes(data)?;
    let pad = (4 - (data.len() % 4)) % 4;
    if pad != 0 {
        writer.write_zeros(pad)?;
    }
    Ok(())
}

fn write_additional_info_subset_with_options(
    writer: &mut PsdWriter,
    info: &LayerAdditionalInfo,
    large: bool,
    sections: &[&str],
    raw_only_sections: &[&str],
) -> Result<()> {
    use std::collections::{HashMap, HashSet, VecDeque};

    fn disk_key(key: &str) -> &str {
        match key {
            // TS exposes this alias in the type surface, but PSD tagged-block
            // signatures are always 4 bytes on disk.
            "lnkD__" => "lnkD",
            _ => key,
        }
    }

    let raw_only_sections: HashSet<&str> = raw_only_sections.iter().copied().collect();

    let mut modeled_blocks = Vec::new();
    for key in sections {
        if raw_only_sections.contains(disk_key(key)) {
            continue;
        }
        let mut temp_writer = PsdWriter::new(1024);
        let length = temp_writer.write_additional_info(key, info)?;
        if length > 0 {
            modeled_blocks.push((disk_key(key).to_string(), temp_writer.into_buffer()));
        }
    }

    if sections
        .iter()
        .any(|key| matches!(disk_key(key), "Patt" | "Pat2" | "Pat3"))
    {
        if let Some(ref block) = info.pattern_data {
            let mut pattern_writer = PsdWriter::new(1024);
            for pattern in &block.patterns {
                let mut entry_writer = PsdWriter::new(1024);
                entry_writer.write_pattern_entry(pattern)?;
                let bytes = entry_writer.into_buffer();
                pattern_writer.write_u32(bytes.len() as u32)?;
                pattern_writer.write_bytes(&bytes)?;
                let remainder = bytes.len() % 4;
                if remainder != 0 {
                    pattern_writer.write_zeros(4 - remainder)?;
                }
            }
            modeled_blocks.push((block.key.as_ref().to_string(), pattern_writer.into_buffer()));
        }
    }

    let emitted_keys: HashSet<&str> = modeled_blocks
        .iter()
        .map(|(key, _)| key.as_str())
        .collect();

    let mut modeled_by_key: HashMap<String, VecDeque<Vec<u8>>> = HashMap::new();
    for (key, data) in &modeled_blocks {
        modeled_by_key
            .entry(key.clone())
            .or_default()
            .push_back(data.clone());
    }

    let mut raw_by_key: HashMap<String, VecDeque<Vec<u8>>> = HashMap::new();
    for raw in &info.raw_blocks {
        raw_by_key
            .entry(disk_key(&raw.key).to_string())
            .or_default()
            .push_back(raw.data.clone());
    }

    if !info.tagged_block_order.is_empty() {
        for key in &info.tagged_block_order {
            let key = disk_key(key).to_string();
            if let Some(queue) = raw_by_key.get_mut(&key) {
                if let Some(raw) = queue.pop_front() {
                    write_tagged_block(writer, &key, &raw, large)?;
                    continue;
                }
            }
            if let Some(queue) = modeled_by_key.get_mut(&key) {
                if let Some(data) = queue.pop_front() {
                    write_tagged_block(writer, &key, &data, large)?;
                }
            }
        }
    }

    for (key, _) in &modeled_blocks {
        if let Some(queue) = modeled_by_key.get_mut(key) {
            while let Some(data) = queue.pop_front() {
                write_tagged_block(writer, key, &data, large)?;
            }
        }
    }

    for raw in &info.raw_blocks {
        let key = disk_key(&raw.key).to_string();
        if !emitted_keys.contains(key.as_str()) {
            if let Some(queue) = raw_by_key.get_mut(&key) {
                if let Some(data) = queue.pop_front() {
                    write_tagged_block(writer, &key, &data, large)?;
                }
            }
        }
    }

    Ok(())
}

pub(crate) fn write_layer_additional_info_with_options(
    writer: &mut PsdWriter,
    info: &LayerAdditionalInfo,
    large: bool,
) -> Result<()> {
    let sections = vec![
        "luni", "lyid", "lclr", "iOpa", "lsct", "clbl", "infx", "knko", "lspf", "lnsr", "lyvr",
        "lmgm", "vmgm", "fcmy", "brst", "fxrp", "TySh", "Txt2", "SoCo", "GdFl", "PtFl", "vstk",
        "vscg", "vmsk", "vogk", "lfx2", "lrFX", "clrL", "rplc", "PlLd", "SoLd", "artb", "sn2P",
        "shmd", "FMsk", "shpa", "pths", "CgEd", "vibA", "PxSc", "phry", "Lr16", "Lr32", "lnk2",
        "lnkD", "lnkD__", "lnk3", "FEid", "PxSD", "Anno", "LMsk", "Mtrn", "Mt16", "Mt32", "FXid",
        "abdd", "anFX", "cinf", "SoLE",
    ];
    let raw_only_sections = [
        "LMsk", "Mtrn", "Mt16", "Mt32", "FXid", "abdd", "anFX", "cinf", "SoLE",
    ];

    // Write adjustment layer block
    write_additional_info_subset_with_options(writer, info, large, &sections, &raw_only_sections)?;

    if let Some(ref adj) = info.adjustment {
        let adj_key = adj.key().to_string();
        let data = adj
            .to_bytes()
            .map_err(|e| PsdError::InvalidFormat(e.to_string()))?;
        write_tagged_block(writer, &adj_key, &data, large)?;
    }

    if let Some(ref desc) = info.layer_effects_descriptor {
        let mut lmfx_writer = PsdWriter::new(256);
        lmfx_writer.write_u32(0)?;
        lmfx_writer.write_version_and_descriptor(16, desc)?;
        write_tagged_block(writer, "lmfx", &lmfx_writer.into_buffer(), large)?;
    }

    Ok(())
}

pub(crate) fn write_document_additional_info_with_options(
    writer: &mut PsdWriter,
    info: &LayerAdditionalInfo,
    large: bool,
) -> Result<()> {
    let sections = [
        "Txt2", "shmd", "Patt", "Pat2", "Pat3", "Anno", "lnk2", "lnkD", "lnkD__", "lnk3",
        "FEid", "PxSD", "FMsk", "Mtrn", "Mt16", "Mt32", "cinf", "abdd", "anFX", "SoLE",
    ];
    let raw_only_sections = ["FMsk", "Mtrn", "Mt16", "Mt32", "cinf", "abdd", "anFX", "SoLE"];
    use std::collections::{HashMap, HashSet, VecDeque};

    fn disk_key(key: &str) -> &str {
        match key {
            "lnkD__" => "lnkD",
            _ => key,
        }
    }

    let raw_only_sections: HashSet<&str> = raw_only_sections.iter().copied().collect();
    let mut modeled_blocks = Vec::new();
    for key in &sections {
        if raw_only_sections.contains(disk_key(key)) {
            continue;
        }
        let mut temp_writer = PsdWriter::new(1024);
        let length = temp_writer.write_additional_info(key, info)?;
        if length > 0 {
            modeled_blocks.push((disk_key(key).to_string(), temp_writer.into_buffer()));
        }
    }
    if let Some(ref block) = info.pattern_data {
        let mut pattern_writer = PsdWriter::new(1024);
        for pattern in &block.patterns {
            let mut entry_writer = PsdWriter::new(1024);
            entry_writer.write_pattern_entry(pattern)?;
            let bytes = entry_writer.into_buffer();
            pattern_writer.write_u32(bytes.len() as u32)?;
            pattern_writer.write_bytes(&bytes)?;
            let remainder = bytes.len() % 4;
            if remainder != 0 {
                pattern_writer.write_zeros(4 - remainder)?;
            }
        }
        modeled_blocks.push((block.key.as_ref().to_string(), pattern_writer.into_buffer()));
    }

    let emitted_keys: HashSet<&str> = modeled_blocks.iter().map(|(key, _)| key.as_str()).collect();
    let mut modeled_by_key: HashMap<String, VecDeque<Vec<u8>>> = HashMap::new();
    for (key, data) in &modeled_blocks {
        modeled_by_key.entry(key.clone()).or_default().push_back(data.clone());
    }
    let mut raw_by_key: HashMap<String, VecDeque<Vec<u8>>> = HashMap::new();
    for raw in &info.raw_blocks {
        raw_by_key
            .entry(disk_key(&raw.key).to_string())
            .or_default()
            .push_back(raw.data.clone());
    }
    if !info.tagged_block_order.is_empty() {
        for key in &info.tagged_block_order {
            let key = disk_key(key).to_string();
            if let Some(queue) = raw_by_key.get_mut(&key) {
                if let Some(raw) = queue.pop_front() {
                    write_tagged_block(writer, &key, &raw, large)?;
                    continue;
                }
            }
            if let Some(queue) = modeled_by_key.get_mut(&key) {
                if let Some(data) = queue.pop_front() {
                    write_tagged_block(writer, &key, &data, large)?;
                }
            }
        }
    }
    for (key, _) in &modeled_blocks {
        if let Some(queue) = modeled_by_key.get_mut(key) {
            while let Some(data) = queue.pop_front() {
                write_tagged_block(writer, key, &data, large)?;
            }
        }
    }
    for raw in &info.raw_blocks {
        let key = disk_key(&raw.key).to_string();
        if !emitted_keys.contains(key.as_str()) {
            if let Some(queue) = raw_by_key.get_mut(&key) {
                if let Some(data) = queue.pop_front() {
                    write_tagged_block(writer, &key, &data, large)?;
                }
            }
        }
    }
    Ok(())
}

fn first_raw_block<'a>(info: &'a LayerAdditionalInfo, key: &str) -> Option<&'a [u8]> {
    info.raw_blocks
        .iter()
        .find(|raw| raw.key == key)
        .map(|raw| raw.data.as_slice())
}

/// Write all additional info sections for a layer
pub fn write_layer_additional_info(
    writer: &mut PsdWriter,
    info: &LayerAdditionalInfo,
) -> Result<()> {
    write_layer_additional_info_with_options(writer, info, false)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn vstk_descriptor_roundtrip() {
        use crate::support::descriptor::{Descriptor, DescriptorValue};
        let mut desc = Descriptor {
            name: String::new(),
            class_id: "vstk".to_string(),
            items: std::collections::HashMap::new(),
        };
        desc.items.insert(
            "strokeStyleVersion".to_string(),
            DescriptorValue::Integer(2),
        );

        let mut info = LayerAdditionalInfo::default();
        info.vector_stroke = Some(VectorStroke {
            version: 1,
            descriptor: desc.clone(),
        });

        let mut w = PsdWriter::new(256);
        let len = w.write_additional_info("vstk", &info).unwrap();
        let buf = w.into_buffer();

        let cursor = std::io::Cursor::new(buf);
        let mut reader = PsdReader::new(cursor, Default::default());
        let mut read_info = LayerAdditionalInfo::default();
        reader
            .read_additional_info("vstk", len, &mut read_info)
            .unwrap();

        let vs = read_info.vector_stroke.unwrap();
        assert!(vs.descriptor.items.contains_key("strokeStyleVersion"));
    }

    #[test]
    fn test_layer_id_roundtrip() {
        let mut info = LayerAdditionalInfo::default();
        info.id = Some(12345);

        let mut writer = PsdWriter::new(128);
        let length = writer.write_additional_info("lyid", &info).unwrap();

        assert_eq!(length, 4);

        let buffer = writer.into_buffer();
        let cursor = std::io::Cursor::new(buffer);
        let mut reader = PsdReader::new(cursor, Default::default());

        let mut read_info = LayerAdditionalInfo::default();
        reader
            .read_additional_info("lyid", length, &mut read_info)
            .unwrap();

        assert_eq!(read_info.id, Some(12345));
    }

    #[test]
    fn test_layer_color_roundtrip() {
        let mut info = LayerAdditionalInfo::default();
        info.layer_color = Some(crate::api::types::LayerColor::Blue);

        let mut writer = PsdWriter::new(128);
        let length = writer.write_additional_info("lclr", &info).unwrap();

        assert_eq!(length, 8);

        let buffer = writer.into_buffer();
        let cursor = std::io::Cursor::new(buffer);
        let mut reader = PsdReader::new(cursor, Default::default());

        let mut read_info = LayerAdditionalInfo::default();
        reader
            .read_additional_info("lclr", length, &mut read_info)
            .unwrap();

        assert_eq!(read_info.layer_color, Some(crate::api::types::LayerColor::Blue));
    }

    #[test]
    fn test_section_divider_roundtrip() {
        let mut info = LayerAdditionalInfo::default();
        info.section_divider = Some(SectionDivider {
            divider_type: SectionDividerType::OpenFolder,
            blend_mode: Some(BlendMode::Normal),
            sub_type: Some(PsdU32Code(2)),
        });

        let mut writer = PsdWriter::new(128);
        let length = writer.write_additional_info("lsct", &info).unwrap();

        let buffer = writer.into_buffer();
        let cursor = std::io::Cursor::new(buffer);
        let mut reader = PsdReader::new(cursor, Default::default());

        let mut read_info = LayerAdditionalInfo::default();
        reader
            .read_additional_info("lsct", length, &mut read_info)
            .unwrap();

        assert_eq!(read_info.section_divider, info.section_divider);
    }

    #[test]
    fn test_small_bool_and_name_source_roundtrip() {
        let mut info = LayerAdditionalInfo::default();
        info.blend_clipped_elements = Some(true);
        info.blend_interior_elements = Some(false);
        info.knockout = Some(true);
        info.name_source = Some("abcd".to_string());

        let mut writer = PsdWriter::new(128);
        let clbl_len = writer.write_additional_info("clbl", &info).unwrap();
        let infx_len = writer.write_additional_info("infx", &info).unwrap();
        let knko_len = writer.write_additional_info("knko", &info).unwrap();
        let lnsr_len = writer.write_additional_info("lnsr", &info).unwrap();

        let buffer = writer.into_buffer();
        let cursor = std::io::Cursor::new(buffer);
        let mut reader = PsdReader::new(cursor, Default::default());

        let mut read_info = LayerAdditionalInfo::default();
        reader
            .read_additional_info("clbl", clbl_len, &mut read_info)
            .unwrap();
        reader
            .read_additional_info("infx", infx_len, &mut read_info)
            .unwrap();
        reader
            .read_additional_info("knko", knko_len, &mut read_info)
            .unwrap();
        reader
            .read_additional_info("lnsr", lnsr_len, &mut read_info)
            .unwrap();

        assert_eq!(read_info.blend_clipped_elements, Some(true));
        assert_eq!(read_info.blend_interior_elements, Some(false));
        assert_eq!(read_info.knockout, Some(true));
        assert_eq!(read_info.name_source.as_deref(), Some("abcd"));
    }

    #[test]
    fn test_layer_tagged_blocks_are_padded_to_four_bytes() {
        let mut info = LayerAdditionalInfo::default();
        info.name = Some("A".to_string());
        info.id = Some(7);

        let mut writer = PsdWriter::new(128);
        write_layer_additional_info_with_options(&mut writer, &info, false).unwrap();
        let buffer = writer.into_buffer();

        let luni_offset = buffer
            .windows(4)
            .position(|window| window == b"luni")
            .expect("luni block present");
        let luni_len = u32::from_be_bytes(
            buffer[luni_offset + 4..luni_offset + 8]
                .try_into()
                .expect("luni length bytes"),
        ) as usize;
        let lyid_offset = buffer
            .windows(8)
            .position(|window| window == b"8BIMlyid")
            .expect("lyid block present");

        assert_eq!(luni_len, 6);
        assert_eq!(lyid_offset, luni_offset + 4 + 4 + luni_len + 2);
        assert_eq!(&buffer[luni_offset + 8 + luni_len..lyid_offset], &[0, 0]);
    }

    #[test]
    fn test_vector_origination_roundtrip() {
        let mut info = LayerAdditionalInfo::default();
        info.vector_origination = Some(VectorOrigination {
            key_descriptor_list: vec![KeyDescriptorItem {
                key_shape_invalidated: Some(true),
                key_origin_index: None,
                key_origin_type: Some(PsdIntCode(4)),
                key_origin_resolution: Some(72.0),
                key_origin_rrect_radii: None,
                key_origin_shape_bounding_box: None,
                key_origin_box_corners: Some(vec![
                    crate::api::types::Point { x: 1.0, y: 2.0 },
                    crate::api::types::Point { x: 3.0, y: 4.0 },
                ]),
                transform: Some(vec![1.0, 0.0, 0.0, 1.0, 10.0, 20.0]),
            }],
        });

        let mut writer = PsdWriter::new(256);
        let length = writer.write_additional_info("vogk", &info).unwrap();
        let buffer = writer.into_buffer();
        let cursor = std::io::Cursor::new(buffer);
        let mut reader = PsdReader::new(cursor, Default::default());
        let mut read_info = LayerAdditionalInfo::default();
        reader
            .read_additional_info("vogk", length, &mut read_info)
            .unwrap();

        assert_eq!(read_info.vector_origination, info.vector_origination);
    }

    #[test]
    fn lnk2_roundtrip() {
        let mut info = LayerAdditionalInfo::default();
        info.linked_files = Some(LinkedFilesBlock {
            key: PsdStringCode::from("lnk2"),
            items: vec![LinkedFile {
                id: "id".to_string(),
                name: "name".to_string(),
                item_version: Some(9),
                data_kind: Some(LinkedFileDataKind::Data),
                file_type: Some(PsdStringCode::from("JPEG")),
                creator: Some(PsdStringCode::from("8BIM")),
                data: Some(vec![0xAA, 0xBB]),
                time: None,
                descriptor: None,
                child_document_id: Some("chid".to_string()),
                asset_mod_time: None,
                asset_locked_state: None,
                linked_file: None,
                open_descriptor: Some(Descriptor {
                    name: String::new(),
                    class_id: "null".to_string(),
                    items: HashMap::new(),
                }),
            }],
        });
        let mut w = PsdWriter::new(128);
        let len = w.write_additional_info("lnk2", &info).unwrap();
        let buf = w.into_buffer();
        let mut reader = PsdReader::new(std::io::Cursor::new(buf), Default::default());
        let mut read_info = LayerAdditionalInfo::default();
        reader
            .read_additional_info("lnk2", len, &mut read_info)
            .unwrap();
        assert_eq!(read_info.linked_files, info.linked_files);
    }

    #[test]
    fn reader_consumes_spec_correct_life_layout() {
        let descriptor = Descriptor {
            name: "linked".to_string(),
            class_id: "lnkF".to_string(),
            items: HashMap::from([("size".to_string(), DescriptorValue::Integer(42))]),
        };
        let open_descriptor = Descriptor {
            name: String::new(),
            class_id: "null".to_string(),
            items: HashMap::new(),
        };
        let linked = LinkedFile {
            id: "external-id".to_string(),
            name: "external.psb".to_string(),
            item_version: Some(7),
            data_kind: Some(LinkedFileDataKind::External),
            file_type: Some(PsdStringCode::from("8BPS")),
            creator: Some(PsdStringCode::from("8BIM")),
            data: Some(vec![0xAA, 0xBB, 0xCC]),
            time: Some("2026-05-28T11:22:33.5Z".to_string()),
            descriptor: Some(descriptor.clone()),
            child_document_id: Some("child-doc".to_string()),
            asset_mod_time: Some(88.5),
            asset_locked_state: Some(1),
            linked_file: Some(LinkedFileInfo {
                file_size: 123_456,
                name: "external.psb".to_string(),
                full_path: "/tmp/external.psb".to_string(),
                original_path: "/orig/external.psb".to_string(),
                relative_path: "external.psb".to_string(),
            }),
            open_descriptor: Some(open_descriptor.clone()),
        };
        let mut item_writer = PsdWriter::new(512);
        item_writer.write_signature("liFE").unwrap();
        item_writer.write_u32(7).unwrap();
        item_writer.write_pascal_string("external-id", 1).unwrap();
        item_writer
            .write_unicode_string_with_padding("external.psb")
            .unwrap();
        item_writer.write_signature("8BPS").unwrap();
        item_writer.write_signature("8BIM").unwrap();
        write_u64_parts(&mut item_writer, 3).unwrap();
        item_writer.write_u8(1).unwrap();
        item_writer.write_u32(16).unwrap();
        item_writer
            .write_descriptor_structure(&open_descriptor)
            .unwrap();
        item_writer.write_descriptor_structure(&descriptor).unwrap();
        item_writer.write_u32(2026).unwrap();
        item_writer.write_u8(5).unwrap();
        item_writer.write_u8(28).unwrap();
        item_writer.write_u8(11).unwrap();
        item_writer.write_u8(22).unwrap();
        item_writer.write_f64(33.5).unwrap();
        write_u64_parts(&mut item_writer, 123_456).unwrap();
        item_writer.write_bytes(&[0xAA, 0xBB, 0xCC]).unwrap();
        item_writer
            .write_unicode_string_with_padding("child-doc")
            .unwrap();
        item_writer.write_f64(88.5).unwrap();
        item_writer.write_u8(1).unwrap();
        item_writer
            .write_unicode_string_with_padding("external.psb")
            .unwrap();
        item_writer
            .write_unicode_string_with_padding("/tmp/external.psb")
            .unwrap();
        item_writer
            .write_unicode_string_with_padding("/orig/external.psb")
            .unwrap();
        item_writer
            .write_unicode_string_with_padding("external.psb")
            .unwrap();
        let item_bytes = item_writer.into_buffer();

        let mut block_writer = PsdWriter::new(1024);
        write_u64_parts(&mut block_writer, item_bytes.len() as u64).unwrap();
        block_writer.write_bytes(&item_bytes).unwrap();
        if item_bytes.len() % 4 != 0 {
            block_writer
                .write_zeros(4 - (item_bytes.len() % 4))
                .unwrap();
        }
        let buf = block_writer.into_buffer();
        let len = buf.len();
        let mut reader = PsdReader::new(std::io::Cursor::new(buf), Default::default());
        let mut read_info = LayerAdditionalInfo::default();
        reader
            .read_additional_info("lnkD", len, &mut read_info)
            .unwrap();

        assert_eq!(read_info.linked_files.unwrap().items[0], linked);
    }

    #[test]
    fn life_writer_places_file_size_before_payload_bytes() {
        let descriptor = Descriptor {
            name: "linked".to_string(),
            class_id: "lnkF".to_string(),
            items: HashMap::from([("size".to_string(), DescriptorValue::Integer(42))]),
        };
        let open_descriptor = Descriptor {
            name: String::new(),
            class_id: "null".to_string(),
            items: HashMap::new(),
        };
        let mut info = LayerAdditionalInfo::default();
        info.linked_files = Some(LinkedFilesBlock {
            key: PsdStringCode::from("lnkD"),
            items: vec![LinkedFile {
                id: "external-id".to_string(),
                name: "external.psb".to_string(),
                item_version: Some(7),
                data_kind: Some(LinkedFileDataKind::External),
                file_type: Some(PsdStringCode::from("8BPS")),
                creator: Some(PsdStringCode::from("8BIM")),
                data: Some(vec![0xAA, 0xBB, 0xCC]),
                time: Some("2026-05-28T11:22:33.5Z".to_string()),
                descriptor: Some(descriptor),
                child_document_id: Some("child-doc".to_string()),
                asset_mod_time: Some(88.5),
                asset_locked_state: Some(1),
                linked_file: Some(LinkedFileInfo {
                    file_size: 123_456,
                    name: "external.psb".to_string(),
                    full_path: "/tmp/external.psb".to_string(),
                    original_path: "/orig/external.psb".to_string(),
                    relative_path: "external.psb".to_string(),
                }),
                open_descriptor: Some(open_descriptor),
            }],
        });

        let mut w = PsdWriter::new(1024);
        let _len = w.write_additional_info("lnkD", &info).unwrap();
        let buf = w.into_buffer();
        let mut reader = PsdReader::new(std::io::Cursor::new(buf), Default::default());

        let _chunk_length = read_u64_parts(&mut reader).unwrap();
        assert_eq!(reader.read_signature().unwrap(), "liFE");
        assert_eq!(reader.read_u32().unwrap(), 7);
        assert_eq!(reader.read_pascal_string(1).unwrap(), "external-id");
        assert_eq!(reader.read_unicode_string().unwrap(), "external.psb");
        assert_eq!(reader.read_signature().unwrap(), "8BPS");
        assert_eq!(reader.read_signature().unwrap(), "8BIM");
        assert_eq!(read_u64_parts(&mut reader).unwrap(), 3);
        assert_eq!(reader.read_u8().unwrap(), 1);
        assert_eq!(reader.read_u32().unwrap(), 16);
        let descriptor_start = reader.offset;
        let _open_descriptor = reader.read_descriptor_structure().unwrap();
        if (reader.offset - descriptor_start) % 2 != 0 {
            reader.skip_bytes(1).unwrap();
        }
        let _descriptor = reader.read_descriptor_structure().unwrap();
        assert_eq!(reader.read_u32().unwrap(), 2026);
        assert_eq!(reader.read_u8().unwrap(), 5);
        assert_eq!(reader.read_u8().unwrap(), 28);
        assert_eq!(reader.read_u8().unwrap(), 11);
        assert_eq!(reader.read_u8().unwrap(), 22);
        assert_eq!(reader.read_f64().unwrap(), 33.5);
        assert_eq!(read_u64_parts(&mut reader).unwrap(), 123_456);
        assert_eq!(reader.read_bytes(3).unwrap(), vec![0xAA, 0xBB, 0xCC]);
    }

    #[test]
    fn lifa_roundtrip_preserves_alias_payload() {
        let linked = LinkedFile {
            id: "alias-id".to_string(),
            name: "alias".to_string(),
            item_version: Some(2),
            data_kind: Some(LinkedFileDataKind::Alias),
            file_type: Some(PsdStringCode::from("TEXT")),
            creator: Some(PsdStringCode::from("8BIM")),
            data: Some(vec![1, 2, 3, 4]),
            time: None,
            descriptor: None,
            child_document_id: None,
            asset_mod_time: None,
            asset_locked_state: None,
            linked_file: None,
            open_descriptor: None,
        };
        let mut info = LayerAdditionalInfo::default();
        info.linked_files = Some(LinkedFilesBlock {
            key: PsdStringCode::from("lnk3"),
            items: vec![linked.clone()],
        });

        let mut w = PsdWriter::new(256);
        let len = w.write_additional_info("lnk3", &info).unwrap();
        let buf = w.into_buffer();
        let mut reader = PsdReader::new(std::io::Cursor::new(buf), Default::default());
        let mut read_info = LayerAdditionalInfo::default();
        reader
            .read_additional_info("lnk3", len, &mut read_info)
            .unwrap();

        assert_eq!(read_info.linked_files.unwrap().items[0], linked);
    }

    #[test]
    fn linked_file_writer_rejects_non_roundtrippable_versioned_fields() {
        let mut info = LayerAdditionalInfo::default();
        info.linked_files = Some(LinkedFilesBlock {
            key: PsdStringCode::from("lnkD"),
            items: vec![LinkedFile {
                id: "bad".to_string(),
                name: "bad".to_string(),
                item_version: Some(4),
                data_kind: Some(LinkedFileDataKind::Data),
                file_type: Some(PsdStringCode::from("TEXT")),
                creator: Some(PsdStringCode::from("8BIM")),
                data: Some(vec![9]),
                time: None,
                descriptor: None,
                child_document_id: Some("too-new".to_string()),
                asset_mod_time: None,
                asset_locked_state: None,
                linked_file: Some(LinkedFileInfo {
                    file_size: 7,
                    name: "bad".to_string(),
                    full_path: "/tmp/bad".to_string(),
                    original_path: "/tmp/bad".to_string(),
                    relative_path: "bad".to_string(),
                }),
                open_descriptor: None,
            }],
        });

        let mut w = PsdWriter::new(256);
        let err = w.write_additional_info("lnkD", &info).unwrap_err();
        assert!(
            err.to_string().contains("linked file"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn fcmy_is_typed_u8() {
        let payload = vec![7u8];
        let cursor = std::io::Cursor::new(payload);
        let mut reader = PsdReader::new(cursor, Default::default());
        let mut info = LayerAdditionalInfo::default();
        reader.read_additional_info("fcmy", 1, &mut info).unwrap();
        assert_eq!(info.fcmy, Some(7));
    }

    #[test]
    fn pattern_block_roundtrip() {
        let mut info = LayerAdditionalInfo::default();
        info.pattern_data = Some(PatternBlock {
            key: PsdStringCode::from("Patt"),
            patterns: vec![PatternBlockEntry {
                name: "pattern".to_string(),
                id: "pat".to_string(),
                version: 1,
                mode: 3,
                width: 2,
                height: 1,
                indexed_palette: None,
                rgba: Some(PixelData {
                    data: vec![10, 20, 30, 255, 40, 50, 60, 128],
                    width: 2,
                    height: 1,
                }),
            }],
        });

        let mut writer = PsdWriter::new(256);
        let len = writer.write_additional_info("Patt", &info).unwrap();
        let buf = writer.into_buffer();
        let mut reader = PsdReader::new(std::io::Cursor::new(buf), Default::default());
        let mut read_info = LayerAdditionalInfo::default();
        reader
            .read_additional_info("Patt", len, &mut read_info)
            .unwrap();

        assert_eq!(read_info.pattern_data, info.pattern_data);
    }

    #[test]
    fn lr16_typed_roundtrip() {
        let layer = Layer {
            top: Some(0),
            left: Some(0),
            bottom: Some(1),
            right: Some(1),
            blend_mode: Some(crate::api::types::BlendMode::Normal),
            opacity: Some(1.0),
            raw_data: Some(crate::api::layer::LayerRawData {
                color_mode: crate::api::types::ColorMode::RGB,
                bits_per_channel: 16,
                channels: vec![crate::api::layer::LayerRawDataChannel {
                    id: crate::api::types::ChannelID::Color0,
                    compression: crate::api::types::Compression::RawData,
                    data: Some(vec![0x12, 0x34]),
                }],
                large: false,
            }),
            ..Layer::default()
        };
        let mut info = LayerAdditionalInfo::default();
        info.high_depth_layer_data = Some(HighDepthLayerInfo {
            key: PsdStringCode::from("Lr16"),
            layers: vec![layer.clone()],
        });

        let mut w = PsdWriter::new(64);
        write_layer_additional_info(&mut w, &info).unwrap();
        let buf = w.into_buffer();
        let total_len = buf.len();
        let mut reader = PsdReader::new(std::io::Cursor::new(buf), Default::default());
        let parsed = read_layer_additional_info(&mut reader, total_len).unwrap();
        let block = parsed.high_depth_layer_data.unwrap();
        assert_eq!(block.key, PsdStringCode::from("Lr16"));
        assert_eq!(block.layers.len(), 1);
        let raw = block.layers[0].raw_data.as_ref().unwrap();
        assert_eq!(raw.bits_per_channel, 16);
        assert_eq!(raw.channels[0].data.as_deref(), Some(&[0x12, 0x34][..]));
    }

    #[test]
    fn adjustment_brit_roundtrip() {
        use crate::api::adjustments::{AdjustmentLayer, BrightnessContrast};

        let mut info = LayerAdditionalInfo::default();
        // brit writer emits 8 zero bytes (legacy block)
        info.adjustment = Some(AdjustmentLayer::BrightnessContrast(BrightnessContrast {
            brightness: 50,
            contrast: -42,
            use_legacy: true,
        }));

        let mut writer = PsdWriter::new(128);
        let length = writer.write_additional_info("brit", &info).unwrap();

        assert_eq!(length, 8); // legacy zero-fill

        let buffer = writer.into_buffer();
        let cursor = std::io::Cursor::new(buffer);
        let mut reader = PsdReader::new(cursor, Default::default());

        let mut read_info = LayerAdditionalInfo::default();
        reader
            .read_additional_info("brit", length, &mut read_info)
            .unwrap();

        assert!(matches!(
            read_info.adjustment,
            Some(AdjustmentLayer::BrightnessContrast(_))
        ));
    }

    #[test]
    fn test_shmd_roundtrip() {
        let mut info = LayerAdditionalInfo::default();
        info.metadata = Some(Metadata {
            entries: vec![
                MetadataEntry {
                    key: "mlst".to_string(),
                    copy_on_sheet_change: false,
                    descriptor: None,
                    raw_data: vec![0x00, 0x00, 0x00, 0x04, 0x01, 0x02, 0x03, 0x04],
                },
                MetadataEntry {
                    key: "cust".to_string(),
                    copy_on_sheet_change: true,
                    descriptor: None,
                    raw_data: vec![0x00, 0x00, 0x00, 0x02, 0xAB, 0xCD],
                },
            ],
        });

        let mut writer = PsdWriter::new(256);
        let length = writer.write_additional_info("shmd", &info).unwrap();

        let buffer = writer.into_buffer();
        let cursor = std::io::Cursor::new(buffer);
        let mut reader = PsdReader::new(cursor, Default::default());

        let mut read_info = LayerAdditionalInfo::default();
        reader
            .read_additional_info("shmd", length, &mut read_info)
            .unwrap();

        let meta = read_info.metadata.unwrap();
        assert_eq!(meta.entries.len(), 2);
        assert_eq!(meta.entries[0].key, "mlst");
        assert_eq!(meta.entries[0].copy_on_sheet_change, false);
        assert_eq!(
            meta.entries[0].raw_data,
            vec![0x00, 0x00, 0x00, 0x04, 0x01, 0x02, 0x03, 0x04]
        );
        assert_eq!(meta.entries[1].key, "cust");
        assert_eq!(meta.entries[1].copy_on_sheet_change, true);
        assert_eq!(
            meta.entries[1].raw_data,
            vec![0x00, 0x00, 0x00, 0x02, 0xAB, 0xCD]
        );
    }

    #[test]
    fn test_protected_flags_roundtrip() {
        let mut info = LayerAdditionalInfo::default();
        info.protected = Some(ProtectedFlags {
            transparency: true,
            composite: false,
            position: true,
            artboards: Some(false),
        });

        let mut writer = PsdWriter::new(128);
        let length = writer.write_additional_info("lspf", &info).unwrap();

        let buffer = writer.into_buffer();
        let cursor = std::io::Cursor::new(buffer);
        let mut reader = PsdReader::new(cursor, Default::default());

        let mut read_info = LayerAdditionalInfo::default();
        reader
            .read_additional_info("lspf", length, &mut read_info)
            .unwrap();

        let protected = read_info.protected.unwrap();
        assert_eq!(protected.transparency, true);
        assert_eq!(protected.composite, false);
        assert_eq!(protected.position, true);
    }

    #[test]
    fn feid_roundtrip_minimal_item() {
        let mut info = LayerAdditionalInfo::default();
        info.filter_effects = Some(FilterEffectsBlock {
            version: 1,
            items: vec![FilterEffectsItem {
                id: "test".to_string(),
                version: Some(1),
                rect: None,
                depth: None,
                channel_count: None,
                slots: None,
                preview: None,
                rgba: None,
            }],
        });

        let mut w = PsdWriter::new(256);
        let len = w.write_additional_info("FEid", &info).unwrap();
        let buf = w.into_buffer();
        let mut reader = PsdReader::new(std::io::Cursor::new(buf), Default::default());
        let mut reparsed = LayerAdditionalInfo::default();
        reader
            .read_additional_info("FEid", len, &mut reparsed)
            .unwrap();
        assert_eq!(reparsed.filter_effects, info.filter_effects);
    }

    #[test]
    fn pxsd_roundtrip_minimal_item() {
        let mut info = LayerAdditionalInfo::default();
        info.pixel_source_data = Some(PixelSourceDataBlock {
            items: vec![PixelSourceDataItem {
                key: 7,
                images: None,
            }],
        });

        let mut w = PsdWriter::new(256);
        let len = w.write_additional_info("PxSD", &info).unwrap();
        let buf = w.into_buffer();
        let mut reader = PsdReader::new(std::io::Cursor::new(buf), Default::default());
        let mut reparsed = LayerAdditionalInfo::default();
        reader
            .read_additional_info("PxSD", len, &mut reparsed)
            .unwrap();
        assert_eq!(reparsed.pixel_source_data, info.pixel_source_data);
    }

    #[test]
    fn txt2_roundtrip_engine_data() {
        use std::collections::HashMap;
        let engine = crate::support::engine_data::EngineValue::Object(HashMap::from([(
            "_DocumentObjects".to_string(),
            crate::support::engine_data::EngineValue::Object(HashMap::new()),
        )]));
        let mut info = LayerAdditionalInfo::default();
        info.text_engine = Some(TextEngineBlock {
            data: engine.clone(),
            raw: None,
        });

        let mut w = PsdWriter::new(256);
        let len = w.write_additional_info("Txt2", &info).unwrap();
        let buf = w.into_buffer();
        let mut reader = PsdReader::new(std::io::Cursor::new(buf), Default::default());
        let mut reparsed = LayerAdditionalInfo::default();
        reader
            .read_additional_info("Txt2", len, &mut reparsed)
            .unwrap();
        assert_eq!(
            reparsed.text_engine.as_ref().map(|block| &block.data),
            info.text_engine.as_ref().map(|block| &block.data)
        );
    }

    #[test]
    fn unknown_tagged_block_roundtrips_via_raw_blocks() {
        let mut writer = PsdWriter::new(64);
        write_tagged_block(&mut writer, "ZZZ1", &[1, 2, 3, 4, 5], false).unwrap();
        let bytes = writer.into_buffer();
        assert_eq!(bytes.len(), 20);
        assert_eq!(u32::from_be_bytes(bytes[8..12].try_into().unwrap()), 5);
        assert_eq!(&bytes[17..20], &[0, 0, 0]);

        let mut reader = PsdReader::new(std::io::Cursor::new(bytes.clone()), Default::default());
        let parsed = read_layer_additional_info(&mut reader, bytes.len()).unwrap();
        assert_eq!(parsed.raw_blocks.len(), 1);
        assert_eq!(parsed.raw_blocks[0].key, "ZZZ1");
        assert_eq!(parsed.raw_blocks[0].data, vec![1, 2, 3, 4, 5]);

        let mut rewritten = PsdWriter::new(64);
        write_layer_additional_info(&mut rewritten, &parsed).unwrap();
        assert_eq!(rewritten.into_buffer(), bytes);
    }

    #[test]
    fn duplicate_unknown_tagged_blocks_preserve_multiplicity_and_order() {
        let mut writer = PsdWriter::new(128);
        write_tagged_block(&mut writer, "ZZZ1", &[1], false).unwrap();
        write_tagged_block(&mut writer, "ZZZ2", &[2], false).unwrap();
        write_tagged_block(&mut writer, "ZZZ1", &[3], false).unwrap();
        let bytes = writer.into_buffer();

        let mut reader = PsdReader::new(std::io::Cursor::new(bytes.clone()), Default::default());
        let parsed = read_layer_additional_info(&mut reader, bytes.len()).unwrap();

        let mut rewritten = PsdWriter::new(128);
        write_layer_additional_info(&mut rewritten, &parsed).unwrap();
        let rewritten = rewritten.into_buffer();

        assert_eq!(rewritten, bytes);
    }

    #[test]
    fn mixed_modeled_and_unknown_tagged_blocks_preserve_original_order() {
        let mut writer = PsdWriter::new(128);
        write_tagged_block(&mut writer, "ZZZ1", &[9], false).unwrap();
        write_tagged_block(&mut writer, "lyid", &123u32.to_be_bytes(), false).unwrap();
        write_tagged_block(&mut writer, "ZZZ2", &[8], false).unwrap();
        let bytes = writer.into_buffer();

        let mut reader = PsdReader::new(std::io::Cursor::new(bytes.clone()), Default::default());
        let parsed = read_layer_additional_info(&mut reader, bytes.len()).unwrap();

        let mut rewritten = PsdWriter::new(128);
        write_layer_additional_info(&mut rewritten, &parsed).unwrap();
        let rewritten = rewritten.into_buffer();

        assert_eq!(rewritten, bytes);
    }

    #[test]
    fn layer_tagged_blocks_are_four_byte_padded_on_write() {
        let mut info = LayerAdditionalInfo::default();
        info.raw_blocks.push(RawTaggedBlock {
            key: "ZZZ1".to_string(),
            data: vec![1],
        });
        info.raw_blocks.push(RawTaggedBlock {
            key: "ZZZ2".to_string(),
            data: vec![2],
        });
        info.tagged_block_order = vec!["ZZZ1".to_string(), "ZZZ2".to_string()];

        let mut writer = PsdWriter::new(64);
        write_layer_additional_info(&mut writer, &info).unwrap();
        let bytes = writer.into_buffer();

        let mut expected = PsdWriter::new(64);
        write_tagged_block(&mut expected, "ZZZ1", &[1], false).unwrap();
        write_tagged_block(&mut expected, "ZZZ2", &[2], false).unwrap();

        assert_eq!(bytes, expected.into_buffer());
    }

    #[test]
    fn txt2_preserves_raw_bytes_verbatim() {
        let raw = b" << /0 1 /1 [ 1.0 ] >> \0\0".to_vec();
        let mut info = LayerAdditionalInfo::default();
        let parsed = crate::support::engine_data::parse_engine_data(&raw).unwrap();
        info.text_engine = Some(TextEngineBlock {
            data: parsed,
            raw: Some(raw.clone()),
        });

        let mut w = PsdWriter::new(256);
        let len = w.write_additional_info("Txt2", &info).unwrap();
        assert_eq!(w.into_buffer()[..len], raw[..]);
    }

    #[test]
    fn txt2_synthesized_has_no_inner_length_prefix() {
        use std::collections::HashMap;
        let engine = crate::support::engine_data::EngineValue::Object(HashMap::from([(
            "_DocumentObjects".to_string(),
            crate::support::engine_data::EngineValue::Object(HashMap::new()),
        )]));
        let mut info = LayerAdditionalInfo::default();
        info.text_engine = Some(TextEngineBlock {
            data: engine,
            raw: None,
        });

        let mut w = PsdWriter::new(256);
        let len = w.write_additional_info("Txt2", &info).unwrap();
        let buf = w.into_buffer();
        assert!(len > 0);
        assert_eq!(buf[0], b' ');
    }

    #[test]
    fn sn2p_is_u32() {
        let mut info = LayerAdditionalInfo::default();
        info.using_aligned_rendering = Some(true);

        let mut w = PsdWriter::new(16);
        let len = w.write_additional_info("sn2P", &info).unwrap();
        let buf = w.into_buffer();

        assert_eq!(len, 4);
        assert_eq!(buf, vec![0, 0, 0, 1]);
    }

    #[test]
    fn sold_read_preserves_existing_plld_transform() {
        let original = std::fs::read(
            PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("tests")
                .join("fixtures")
                .join("samples")
                .join("3d-preview-mockup.psd"),
        )
        .expect("read sample");
        let psd = crate::read_psd(std::io::Cursor::new(original), Default::default())
            .expect("parse sample");
        let layer = psd
            .children
            .as_deref()
            .unwrap_or(&[])
            .iter()
            .find_map(|layer| {
                fn find<'a>(layer: &'a crate::Layer, name: &str) -> Option<&'a crate::Layer> {
                    if layer.additional_info.name.as_deref() == Some(name) {
                        return Some(layer);
                    }
                    layer.children
                        .as_deref()
                        .and_then(|children| children.iter().find_map(|child| find(child, name)))
                }
                find(layer, "4")
            })
            .expect("sample layer 4");
        let placed = layer.additional_info.placed_layer.as_ref().expect("placed layer");
        assert!(
            !placed.transform.is_empty(),
            "PlLd transform should survive SoLd parsing on placed layers"
        );
    }

    #[test]
    fn sample_vmsk_preserves_fill_rule_and_initial_fill_record() {
        fn extract_named_block(path: &PathBuf, layer_name: &str, key: &str) -> Vec<u8> {
            let data = std::fs::read(path).expect("read sample");
            let mut offset = 26usize;
            let read_u32 = |bytes: &[u8], o: usize| {
                u32::from_be_bytes(bytes[o..o + 4].try_into().expect("u32"))
            };
            let color_len = read_u32(&data, offset) as usize;
            offset += 4 + color_len;
            let image_resources_len = read_u32(&data, offset) as usize;
            offset += 4 + image_resources_len;
            let _layer_and_mask_len = read_u32(&data, offset) as usize;
            offset += 4;
            let _layer_info_len = read_u32(&data, offset) as usize;
            offset += 4;
            let layer_count =
                i16::from_be_bytes(data[offset..offset + 2].try_into().expect("i16")).abs() as usize;
            offset += 2;

            for _ in 0..layer_count {
                offset += 16;
                let channel_count =
                    u16::from_be_bytes(data[offset..offset + 2].try_into().expect("u16")) as usize;
                offset += 2 + channel_count * 6 + 12;
                let extra_len = read_u32(&data, offset) as usize;
                let extra_end = offset + 4 + extra_len;
                offset += 4;
                let mask_len = read_u32(&data, offset) as usize;
                offset += 4 + mask_len;
                let blend_len = read_u32(&data, offset) as usize;
                offset += 4 + blend_len;
                let name_len = data[offset] as usize;
                let name = String::from_utf8_lossy(&data[offset + 1..offset + 1 + name_len]).to_string();
                let padded_name_len = ((name_len + 1) + 3) & !3;
                offset += padded_name_len;

                let mut tagged_offset = offset;
                while tagged_offset + 12 <= extra_end {
                    let signature = &data[tagged_offset..tagged_offset + 4];
                    if signature != b"8BIM" && signature != b"8B64" {
                        break;
                    }
                    let block_key =
                        String::from_utf8_lossy(&data[tagged_offset + 4..tagged_offset + 8]).to_string();
                    let length = read_u32(&data, tagged_offset + 8) as usize;
                    let data_start = tagged_offset + 12;
                    let data_end = data_start + length;
                    if name == layer_name && block_key == key {
                        return data[data_start..data_end].to_vec();
                    }
                    tagged_offset = data_end;
                    while tagged_offset < extra_end
                        && tagged_offset + 4 <= extra_end
                        && &data[tagged_offset..tagged_offset + 4] != b"8BIM"
                        && &data[tagged_offset..tagged_offset + 4] != b"8B64"
                    {
                        tagged_offset += 1;
                    }
                }
                offset = extra_end;
            }

            panic!("block {key} for layer {layer_name} not found");
        }

        let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("tests")
            .join("fixtures")
            .join("samples")
            .join("3d-preview-mockup.psd");
        let original_block = extract_named_block(&path, "4", "vmsk");
        let psd = crate::read_psd(std::io::Cursor::new(std::fs::read(&path).expect("read sample")), Default::default())
            .expect("parse sample");
        fn find_layer<'a>(layers: &'a [crate::Layer], name: &str) -> Option<&'a crate::Layer> {
            for layer in layers {
                if layer.additional_info.name.as_deref() == Some(name) {
                    return Some(layer);
                }
                if let Some(found) = layer
                    .children
                    .as_deref()
                    .and_then(|children| find_layer(children, name))
                {
                    return Some(found);
                }
            }
            None
        }
        let layer = find_layer(psd.children.as_deref().unwrap_or(&[]), "4").expect("sample layer 4");

        let mut writer = PsdWriter::new(512);
        let length = writer
            .write_additional_info("vmsk", &layer.additional_info)
            .expect("write vmsk");
        let rewritten = writer.into_buffer();

        assert_eq!(length, original_block.len(), "vmsk block length changed");
        assert_eq!(rewritten, original_block, "vmsk bytes changed after semantic roundtrip");
    }

    #[test]
    fn sample_plld_preserves_legacy_payload_for_layer_4() {
        fn extract_named_block(path: &PathBuf, layer_name: &str, key: &str) -> Vec<u8> {
            let data = std::fs::read(path).expect("read sample");
            let mut offset = 26usize;
            let read_u32 = |bytes: &[u8], o: usize| {
                u32::from_be_bytes(bytes[o..o + 4].try_into().expect("u32"))
            };
            let color_len = read_u32(&data, offset) as usize;
            offset += 4 + color_len;
            let image_resources_len = read_u32(&data, offset) as usize;
            offset += 4 + image_resources_len;
            let _layer_and_mask_len = read_u32(&data, offset) as usize;
            offset += 4;
            let _layer_info_len = read_u32(&data, offset) as usize;
            offset += 4;
            let layer_count =
                i16::from_be_bytes(data[offset..offset + 2].try_into().expect("i16")).abs() as usize;
            offset += 2;

            for _ in 0..layer_count {
                offset += 16;
                let channel_count =
                    u16::from_be_bytes(data[offset..offset + 2].try_into().expect("u16")) as usize;
                offset += 2 + channel_count * 6 + 12;
                let extra_len = read_u32(&data, offset) as usize;
                let extra_end = offset + 4 + extra_len;
                offset += 4;
                let mask_len = read_u32(&data, offset) as usize;
                offset += 4 + mask_len;
                let blend_len = read_u32(&data, offset) as usize;
                offset += 4 + blend_len;
                let name_len = data[offset] as usize;
                let name = String::from_utf8_lossy(&data[offset + 1..offset + 1 + name_len]).to_string();
                let padded_name_len = ((name_len + 1) + 3) & !3;
                offset += padded_name_len;

                let mut tagged_offset = offset;
                while tagged_offset + 12 <= extra_end {
                    let signature = &data[tagged_offset..tagged_offset + 4];
                    if signature != b"8BIM" && signature != b"8B64" {
                        break;
                    }
                    let block_key =
                        String::from_utf8_lossy(&data[tagged_offset + 4..tagged_offset + 8]).to_string();
                    let length = read_u32(&data, tagged_offset + 8) as usize;
                    let data_start = tagged_offset + 12;
                    let data_end = data_start + length;
                    if name == layer_name && block_key == key {
                        return data[data_start..data_end].to_vec();
                    }
                    tagged_offset = data_end;
                    while tagged_offset < extra_end
                        && tagged_offset + 4 <= extra_end
                        && &data[tagged_offset..tagged_offset + 4] != b"8BIM"
                        && &data[tagged_offset..tagged_offset + 4] != b"8B64"
                    {
                        tagged_offset += 1;
                    }
                }
                offset = extra_end;
            }

            panic!("block {key} for layer {layer_name} not found");
        }

        let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("tests")
            .join("fixtures")
            .join("samples")
            .join("3d-preview-mockup.psd");
        let original_block = extract_named_block(&path, "4", "PlLd");
        let psd =
            crate::read_psd(std::io::Cursor::new(std::fs::read(&path).expect("read sample")), Default::default())
                .expect("parse sample");
        fn find_layer<'a>(layers: &'a [crate::Layer], name: &str) -> Option<&'a crate::Layer> {
            for layer in layers {
                if layer.additional_info.name.as_deref() == Some(name) {
                    return Some(layer);
                }
                if let Some(found) = layer
                    .children
                    .as_deref()
                    .and_then(|children| find_layer(children, name))
                {
                    return Some(found);
                }
            }
            None
        }
        let layer = find_layer(psd.children.as_deref().unwrap_or(&[]), "4").expect("sample layer 4");

        let mut writer = PsdWriter::new(2048);
        let length = writer
            .write_additional_info("PlLd", &layer.additional_info)
            .expect("write PlLd");
        let rewritten = writer.into_buffer();

        assert_eq!(length, original_block.len(), "PlLd block length changed");
        assert_eq!(rewritten, original_block, "PlLd bytes changed after semantic roundtrip");
    }

    #[test]
    fn sample_plld_preserves_custom_envelope_warp_for_plants_layer() {
        fn extract_named_block(path: &PathBuf, layer_name: &str, key: &str) -> Vec<u8> {
            let data = std::fs::read(path).expect("read sample");
            let mut offset = 26usize;
            let read_u32 = |bytes: &[u8], o: usize| {
                u32::from_be_bytes(bytes[o..o + 4].try_into().expect("u32"))
            };
            let color_len = read_u32(&data, offset) as usize;
            offset += 4 + color_len;
            let image_resources_len = read_u32(&data, offset) as usize;
            offset += 4 + image_resources_len;
            let _layer_and_mask_len = read_u32(&data, offset) as usize;
            offset += 4;
            let _layer_info_len = read_u32(&data, offset) as usize;
            offset += 4;
            let layer_count =
                i16::from_be_bytes(data[offset..offset + 2].try_into().expect("i16")).abs() as usize;
            offset += 2;

            for _ in 0..layer_count {
                offset += 16;
                let channel_count =
                    u16::from_be_bytes(data[offset..offset + 2].try_into().expect("u16")) as usize;
                offset += 2 + channel_count * 6 + 12;
                let extra_len = read_u32(&data, offset) as usize;
                let extra_end = offset + 4 + extra_len;
                offset += 4;
                let mask_len = read_u32(&data, offset) as usize;
                offset += 4 + mask_len;
                let blend_len = read_u32(&data, offset) as usize;
                offset += 4 + blend_len;
                let name_len = data[offset] as usize;
                let name = String::from_utf8_lossy(&data[offset + 1..offset + 1 + name_len]).to_string();
                let padded_name_len = ((name_len + 1) + 3) & !3;
                offset += padded_name_len;

                let mut tagged_offset = offset;
                while tagged_offset + 12 <= extra_end {
                    let signature = &data[tagged_offset..tagged_offset + 4];
                    if signature != b"8BIM" && signature != b"8B64" {
                        break;
                    }
                    let block_key =
                        String::from_utf8_lossy(&data[tagged_offset + 4..tagged_offset + 8]).to_string();
                    let length = read_u32(&data, tagged_offset + 8) as usize;
                    let data_start = tagged_offset + 12;
                    let data_end = data_start + length;
                    if name == layer_name && block_key == key {
                        return data[data_start..data_end].to_vec();
                    }
                    tagged_offset = data_end;
                    while tagged_offset < extra_end
                        && tagged_offset + 4 <= extra_end
                        && &data[tagged_offset..tagged_offset + 4] != b"8BIM"
                        && &data[tagged_offset..tagged_offset + 4] != b"8B64"
                    {
                        tagged_offset += 1;
                    }
                }
                offset = extra_end;
            }

            panic!("block {key} for layer {layer_name} not found");
        }

        let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("tests")
            .join("fixtures")
            .join("samples")
            .join("4901393.psd");
        let original_block = extract_named_block(&path, "Plants", "PlLd");
        let psd =
            crate::read_psd(std::io::Cursor::new(std::fs::read(&path).expect("read sample")), Default::default())
                .expect("parse sample");
        fn find_layer<'a>(layers: &'a [crate::Layer], name: &str) -> Option<&'a crate::Layer> {
            for layer in layers {
                if layer.additional_info.name.as_deref() == Some(name) {
                    return Some(layer);
                }
                if let Some(found) = layer
                    .children
                    .as_deref()
                    .and_then(|children| find_layer(children, name))
                {
                    return Some(found);
                }
            }
            None
        }
        let layer = find_layer(psd.children.as_deref().unwrap_or(&[]), "Plants").expect("plants layer");

        let mut writer = PsdWriter::new(2048);
        let length = writer
            .write_additional_info("PlLd", &layer.additional_info)
            .expect("write PlLd");
        let rewritten = writer.into_buffer();

        assert_eq!(length, original_block.len(), "PlLd block length changed");
        assert_eq!(rewritten, original_block, "PlLd bytes changed after semantic roundtrip");
    }

    #[test]
    fn sample_luni_preserves_adobe_padded_unicode_name_for_layer_4() {
        fn extract_named_block(path: &PathBuf, layer_name: &str, key: &str) -> Vec<u8> {
            let data = std::fs::read(path).expect("read sample");
            let mut offset = 26usize;
            let read_u32 = |bytes: &[u8], o: usize| {
                u32::from_be_bytes(bytes[o..o + 4].try_into().expect("u32"))
            };
            let color_len = read_u32(&data, offset) as usize;
            offset += 4 + color_len;
            let image_resources_len = read_u32(&data, offset) as usize;
            offset += 4 + image_resources_len;
            let _layer_and_mask_len = read_u32(&data, offset) as usize;
            offset += 4;
            let _layer_info_len = read_u32(&data, offset) as usize;
            offset += 4;
            let layer_count =
                i16::from_be_bytes(data[offset..offset + 2].try_into().expect("i16")).abs() as usize;
            offset += 2;

            for _ in 0..layer_count {
                offset += 16;
                let channel_count =
                    u16::from_be_bytes(data[offset..offset + 2].try_into().expect("u16")) as usize;
                offset += 2 + channel_count * 6 + 12;
                let extra_len = read_u32(&data, offset) as usize;
                let extra_end = offset + 4 + extra_len;
                offset += 4;
                let mask_len = read_u32(&data, offset) as usize;
                offset += 4 + mask_len;
                let blend_len = read_u32(&data, offset) as usize;
                offset += 4 + blend_len;
                let name_len = data[offset] as usize;
                let name = String::from_utf8_lossy(&data[offset + 1..offset + 1 + name_len]).to_string();
                let padded_name_len = ((name_len + 1) + 3) & !3;
                offset += padded_name_len;

                let mut tagged_offset = offset;
                while tagged_offset + 12 <= extra_end {
                    let signature = &data[tagged_offset..tagged_offset + 4];
                    if signature != b"8BIM" && signature != b"8B64" {
                        break;
                    }
                    let block_key =
                        String::from_utf8_lossy(&data[tagged_offset + 4..tagged_offset + 8]).to_string();
                    let length = read_u32(&data, tagged_offset + 8) as usize;
                    let data_start = tagged_offset + 12;
                    let data_end = data_start + length;
                    if name == layer_name && block_key == key {
                        return data[data_start..data_end].to_vec();
                    }
                    tagged_offset = data_end;
                    while tagged_offset < extra_end
                        && tagged_offset + 4 <= extra_end
                        && &data[tagged_offset..tagged_offset + 4] != b"8BIM"
                        && &data[tagged_offset..tagged_offset + 4] != b"8B64"
                    {
                        tagged_offset += 1;
                    }
                }
                offset = extra_end;
            }

            panic!("block {key} for layer {layer_name} not found");
        }

        let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("tests")
            .join("fixtures")
            .join("samples")
            .join("3d-preview-mockup.psd");
        let original_block = extract_named_block(&path, "4", "luni");
        let psd =
            crate::read_psd(std::io::Cursor::new(std::fs::read(&path).expect("read sample")), Default::default())
                .expect("parse sample");
        fn find_layer<'a>(layers: &'a [crate::Layer], name: &str) -> Option<&'a crate::Layer> {
            for layer in layers {
                if layer.additional_info.name.as_deref() == Some(name) {
                    return Some(layer);
                }
                if let Some(found) = layer
                    .children
                    .as_deref()
                    .and_then(|children| find_layer(children, name))
                {
                    return Some(found);
                }
            }
            None
        }
        let layer = find_layer(psd.children.as_deref().unwrap_or(&[]), "4").expect("sample layer 4");

        let mut writer = PsdWriter::new(64);
        let length = writer
            .write_additional_info("luni", &layer.additional_info)
            .expect("write luni");
        let rewritten = writer.into_buffer();

        assert_eq!(length, 6, "canonical luni payload length changed");
        assert_eq!(rewritten, original_block[..length], "luni content changed after semantic roundtrip");
        assert_eq!(&original_block[length..], &[0, 0], "sample should only differ by legacy terminator");
    }
}

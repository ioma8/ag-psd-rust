//! Image resource handlers for PSD files
//!
//! Image resources contain document-level information like resolution,
//! guides, grids, color profiles, and thumbnails.

use crate::support::binrw_support::{
    decode_be, encode_be, DisplayInfoRecord, GridAndGuidesHeaderRecord, GuideRecord,
    ImageResourceHeaderRecord, ImageResourceLengthRecord, LayerStateRecord, PrintFlagsRecord,
    PrintScaleRecord, ResolutionInfoRecord, SignedI32Record, ThumbnailHeaderRecord,
    U16ListCountRecord, U32ValueRecord, U8BoolRecord,
};
use crate::support::descriptor::Descriptor;
use crate::support::error::{PsdError, Result};
use crate::io::reader::PsdReader;
use crate::api::types::{
    BlendMode, Color, DisplayUnit, Fraction, LayerCompCapturedInfo, Point, RenderingIntent,
    SliceAlignment, SliceOrigin, SliceSourceType, SliceType,
};
use crate::io::writer::PsdWriter;
use std::collections::HashMap;
use std::io::{Cursor, Read, Seek};

/// Image resources structure
#[derive(Debug, Clone, PartialEq, Default)]
pub struct ImageResources {
    /// Original image resource order from source PSD, including unknown entries.
    pub resource_order: Vec<u16>,
    /// Resolution information (DPI)
    pub resolution_info: Option<ResolutionInfo>,
    /// XMP metadata
    pub xmp_metadata: Option<String>,
    /// Caption digest (MD5 hash)
    pub caption_digest: Option<String>,
    /// Print information
    pub print_information: Option<PrintInformation>,
    /// Print flags
    pub print_flags: Option<PrintFlags>,
    /// Background color
    pub background_color: Option<Color>,
    /// Copyright flag
    pub copyrighted: Option<bool>,
    /// URL
    pub url: Option<String>,
    /// Thumbnail
    pub thumbnail: Option<Thumbnail>,
    /// Grid and guides
    pub grid_and_guides: Option<GridAndGuides>,
    /// Global lighting angle
    pub global_angle: Option<i32>,
    /// Global altitude
    pub global_altitude: Option<i32>,
    /// Layer state
    pub layer_state: Option<u16>,
    /// Layer group IDs (resource 1026)
    pub layer_group_ids: Option<Vec<u16>>,
    /// Resource visibility (resource 1072)
    pub resource_visibility_typed: Option<ResourceVisibility>,
    /// Color samplers (resource 1073)
    pub color_samplers_typed: Option<ColorSamplersResource>,
    /// Display info (resource 1077)
    pub display_info_typed: Option<DisplayInfoResource>,
    /// Name of clipping path (resource 2999)
    pub clipping_path_name: Option<String>,
    /// Layer selection IDs
    pub layer_selection_ids: Option<Vec<u32>>,
    /// Alpha channel names
    pub alpha_names: Option<Vec<String>>,
    /// Alpha channel unicode names
    pub alpha_unicode_names: Option<Vec<String>>,
    /// Alpha identifiers
    pub alpha_identifiers: Option<Vec<u32>>,
    /// ICC profile data
    pub icc_profile: Option<Vec<u8>>,
    /// Print scale
    pub print_scale: Option<PrintScale>,
    /// Slices
    pub slices: Option<Slices>,
    /// Path resources (2000..=2998)
    pub path_resources: HashMap<u16, Vec<PathResourceRecord>>,
    /// Layer comps
    pub layer_comps: Option<LayerComps>,
    /// Timeline information
    pub timeline: Option<Timeline>,
    /// Sheet disclosure
    pub sheet_disclosure: Option<Vec<u8>>,
    /// Onion skins
    pub onion_skins: Option<OnionSkins>,
    /// Count information
    pub count_information: Option<Vec<CountGroup>>,
    /// URL list
    pub url_list: Option<Vec<UrlEntry>>,
    /// Variables XML (resource 7000)
    pub variables: Option<String>,
    /// Data sets XML (resource 7001)
    pub data_sets: Option<String>,
    /// Generic descriptor resources (1065, 1074, 1075)
    pub descriptor_resources: HashMap<u16, Descriptor>,
    /// Opaque resource blocks preserved verbatim for unknown/unmodeled resources.
    pub raw_resources: Vec<RawImageResource>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct RawImageResource {
    pub resource_id: u16,
    pub bytes: Vec<u8>,
}

/// Simplified path resource record (26-byte document path record)
#[derive(Debug, Clone, PartialEq)]
pub struct PathResourceRecord {
    pub record_type: u16,
    pub points: Vec<Point>,
    pub closed: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ResourceVisibility {
    pub values: Vec<bool>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ColorSamplersResource {
    pub version: u32,
    pub samplers: Vec<crate::api::psd::ColorSampler>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct DisplayInfoResource {
    pub version: u16,
    pub h_res_unit: DisplayUnit,
    pub v_res_unit: DisplayUnit,
    pub width_unit: DisplayUnit,
    pub height_unit: DisplayUnit,
}

fn parse_thumbnail_resource(bytes: &[u8]) -> Option<Thumbnail> {
    let header: ThumbnailHeaderRecord = decode_be(bytes.get(..28)?, "thumbnail header").ok()?;
    let data_end = 28 + header.compressed_size as usize;
    let data = bytes.get(28..data_end)?.to_vec();
    Some(Thumbnail {
        width: header.width,
        height: header.height,
        format: if header.format == 1 {
            ThumbnailFormat::JpegRgb
        } else {
            ThumbnailFormat::RawRgb
        },
        data,
    })
}

fn build_thumbnail_resource(thumbnail: &Thumbnail) -> Vec<u8> {
    let format = match thumbnail.format {
        ThumbnailFormat::RawRgb => 0u32,
        ThumbnailFormat::JpegRgb => 1u32,
    };
    let width_bytes = ((thumbnail.width * 24 + 31) / 32) * 4;
    let total_size = width_bytes * thumbnail.height;
    let compressed_size = thumbnail.data.len() as u32;
    let header = ThumbnailHeaderRecord {
        format,
        width: thumbnail.width,
        height: thumbnail.height,
        width_bytes,
        total_size,
        compressed_size,
        bits_per_pixel: 24,
        planes: 1,
    };
    let mut bytes = encode_be(&header, "thumbnail header").unwrap_or_else(|_| vec![0u8; 28]);
    bytes.extend_from_slice(&thumbnail.data);
    bytes
}

/// Resolution information
#[derive(Debug, Clone, PartialEq)]
pub struct ResolutionInfo {
    pub horizontal_res: f64,
    pub horizontal_res_unit: ResolutionUnit,
    pub width_unit: MeasurementUnit,
    pub vertical_res: f64,
    pub vertical_res_unit: ResolutionUnit,
    pub height_unit: MeasurementUnit,
}

/// Resolution units
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResolutionUnit {
    PixelsPerInch = 1,
    PixelsPerCentimeter = 2,
}

/// Measurement units
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MeasurementUnit {
    Inches = 1,
    Centimeters = 2,
    Points = 3,
    Picas = 4,
    Columns = 5,
}

fn parse_display_info_resource(bytes: &[u8]) -> Option<DisplayInfoResource> {
    if bytes.len() < 28 {
        return None;
    }
    let record: DisplayInfoRecord = decode_be(bytes, "display info resource").ok()?;
    Some(DisplayInfoResource {
        version: record.version,
        h_res_unit: DisplayUnit::from_u16(record.h_res_unit()),
        v_res_unit: DisplayUnit::from_u16(record.v_res_unit()),
        width_unit: DisplayUnit::from_u16(record.width_unit()),
        height_unit: DisplayUnit::from_u16(record.height_unit()),
    })
}

fn build_display_info_resource(info: &DisplayInfoResource) -> Vec<u8> {
    let record = DisplayInfoRecord::from(info.clone());
    encode_be(&record, "display info resource").unwrap_or_else(|_| vec![0u8; 28])
}

fn parse_color_samplers_resource(bytes: &[u8]) -> ColorSamplersResource {
    if bytes.len() < 8 {
        return ColorSamplersResource {
            version: 0,
            samplers: Vec::new(),
        };
    }
    let version = u32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]);
    let count = u32::from_be_bytes([bytes[4], bytes[5], bytes[6], bytes[7]]) as usize;
    let mut samplers = Vec::new();
    let mut offset = 8;
    for _ in 0..count {
        let record_len = if version == 1 { 10 } else { 12 };
        if offset + record_len > bytes.len() {
            break;
        }
        let horizontal = i32::from_be_bytes([
            bytes[offset],
            bytes[offset + 1],
            bytes[offset + 2],
            bytes[offset + 3],
        ]);
        let vertical = i32::from_be_bytes([
            bytes[offset + 4],
            bytes[offset + 5],
            bytes[offset + 6],
            bytes[offset + 7],
        ]);
        let position = match version {
            1 => crate::api::psd::ColorSamplerPosition::V1 {
                horizontal,
                vertical,
            },
            2 => crate::api::psd::ColorSamplerPosition::V2 {
                horizontal,
                vertical,
                depth: u16::from_be_bytes([bytes[offset + 10], bytes[offset + 11]]),
            },
            _ => crate::api::psd::ColorSamplerPosition::Unsupported {
                version,
                horizontal,
                vertical,
                depth: if version >= 2 {
                    Some(u16::from_be_bytes([bytes[offset + 10], bytes[offset + 11]]))
                } else {
                    None
                },
            },
        };
        samplers.push(crate::api::psd::ColorSampler {
            position,
            color_space: i16::from_be_bytes([bytes[offset + 8], bytes[offset + 9]]),
        });
        offset += record_len;
    }
    ColorSamplersResource { version, samplers }
}

pub(crate) fn infer_color_sampler_version(
    samplers: &[crate::api::psd::ColorSampler],
) -> Result<Option<u32>> {
    let Some(first) = samplers.first() else {
        return Ok(None);
    };

    let version = first.position.version();
    for sampler in samplers.iter().skip(1) {
        let sampler_version = sampler.position.version();
        if sampler_version != version {
            return Err(PsdError::InvalidFormat(
                "Color samplers resource must contain a single color sampler version".to_string(),
            ));
        }
    }

    Ok(Some(version))
}

fn build_color_samplers_resource(samplers: &ColorSamplersResource) -> Result<Vec<u8>> {
    if let Some(version) = infer_color_sampler_version(&samplers.samplers)? {
        if samplers.version != version {
            return Err(PsdError::InvalidFormat(format!(
                "Color sampler position version {version} does not match resource version {}",
                samplers.version
            )));
        }
    }

    let mut bytes = Vec::with_capacity(8 + samplers.samplers.len() * 12);
    bytes.extend_from_slice(&samplers.version.to_be_bytes());
    bytes.extend_from_slice(&(samplers.samplers.len() as u32).to_be_bytes());
    for sampler in &samplers.samplers {
        let (horizontal, vertical) = sampler.position.coordinates();
        bytes.extend_from_slice(&horizontal.to_be_bytes());
        bytes.extend_from_slice(&vertical.to_be_bytes());
        bytes.extend_from_slice(&(sampler.color_space as u16).to_be_bytes());
        match &sampler.position {
            crate::api::psd::ColorSamplerPosition::V1 { .. } => {}
            crate::api::psd::ColorSamplerPosition::V2 { depth, .. } => {
                bytes.extend_from_slice(&depth.to_be_bytes());
            }
            crate::api::psd::ColorSamplerPosition::Unsupported { version, depth, .. } => {
                if *version >= 2 {
                    let depth = depth.ok_or_else(|| {
                        PsdError::InvalidFormat(format!(
                            "Color sampler version {version} requires depth to serialize losslessly"
                        ))
                    })?;
                    bytes.extend_from_slice(&depth.to_be_bytes());
                }
            }
        }
    }
    Ok(bytes)
}

#[cfg(test)]
pub(crate) fn parse_color_samplers_resource_for_test(bytes: &[u8]) -> ColorSamplersResource {
    parse_color_samplers_resource(bytes)
}

#[cfg(test)]
pub(crate) fn build_color_samplers_resource_for_test(
    samplers: &ColorSamplersResource,
) -> Result<Vec<u8>> {
    build_color_samplers_resource(samplers)
}

/// Print information
#[derive(Debug, Clone, PartialEq)]
pub struct PrintInformation {
    pub printer_name: String,
    pub rendering_intent: RenderingIntent,
    pub black_point_compensation: Option<bool>,
    pub printer_manages_colors: Option<bool>,
    pub printer_profile: Option<String>,
    pub print_sixteen_bit: Option<bool>,
    pub hard_proof: Option<bool>,
    pub proof_setup: Option<ProofSetup>,
}

/// Proof setup
#[derive(Debug, Clone, PartialEq)]
pub enum ProofSetup {
    Builtin {
        name: String,
    },
    Custom {
        profile: String,
        rendering_intent: RenderingIntent,
        black_point_compensation: bool,
        paper_white: bool,
    },
}

/// Print flags
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct PrintFlags {
    pub labels: bool,
    pub crop_marks: bool,
    pub color_bars: bool,
    pub registration_marks: bool,
    pub negative: bool,
    pub flip: bool,
    pub interpolate: bool,
    pub caption: bool,
    pub print_flags: bool,
}

/// Thumbnail
#[derive(Debug, Clone, PartialEq)]
pub struct Thumbnail {
    pub width: u32,
    pub height: u32,
    pub format: ThumbnailFormat,
    pub data: Vec<u8>,
}

/// Thumbnail format
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ThumbnailFormat {
    RawRgb = 0,
    JpegRgb = 1,
}

/// Grid and guides
#[derive(Debug, Clone, PartialEq)]
pub struct GridAndGuides {
    pub grid: Grid,
    pub guides: Vec<Guide>,
}

/// Grid spacing
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Grid {
    pub horizontal: u32,
    pub vertical: u32,
}

/// Guide line
#[derive(Debug, Clone, PartialEq)]
pub struct Guide {
    pub location: f64,
    pub direction: GuideDirection,
}

/// Guide direction
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GuideDirection {
    Vertical,
    Horizontal,
}

/// Print scale
#[derive(Debug, Clone, PartialEq)]
pub struct PrintScale {
    pub style: PrintScaleStyle,
    pub x: f32,
    pub y: f32,
    pub scale: f32,
}

/// Print scale style
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PrintScaleStyle {
    Centered = 0,
    SizeToFit = 1,
    UserDefined = 2,
}

/// Slices
#[derive(Debug, Clone, PartialEq)]
pub struct Slices {
    pub version: u32,
    pub bounds: Option<SliceBounds>,
    pub group_name: Option<String>,
    pub slices: Vec<Slice>,
    pub descriptor: Option<Descriptor>,
}

/// Slice
#[derive(Debug, Clone, PartialEq)]
pub struct Slice {
    pub id: u32,
    pub group_id: u32,
    pub origin: SliceOrigin,
    pub associated_layer_id: u32,
    pub name: String,
    pub slice_type: SliceType,
    pub bounds: SliceBounds,
    pub url: String,
    pub target: String,
    pub message: String,
    pub alt_tag: String,
    pub cell_text: String,
    pub horizontal_align: SliceAlignment,
    pub vertical_align: SliceAlignment,
    pub alpha: u8,
    pub bg_color: [u8; 4],
    pub cell_is_html: bool,
    pub source_id: Option<u32>,
    pub source_type: Option<SliceSourceType>,
    pub descriptor: Option<Descriptor>,
}

/// Rectangle bounds for slices (integer pixel coordinates)
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SliceBounds {
    pub top: i32,
    pub left: i32,
    pub bottom: i32,
    pub right: i32,
}

fn next_bytes_start_legacy_slice_descriptor(bytes: &[u8], offset: usize, end: usize) -> bool {
    let remaining = end.saturating_sub(offset);
    if remaining < 20 {
        return false;
    }

    let descriptor_version = u32::from_be_bytes(
        bytes[offset..offset + 4]
            .try_into()
            .expect("slice descriptor version"),
    );
    if descriptor_version != 16 {
        return false;
    }

    let mut cursor = offset + 4;
    let name_len =
        u32::from_be_bytes(bytes[cursor..cursor + 4].try_into().expect("name length")) as usize;
    cursor += 4;
    if name_len != 1 {
        return false;
    }

    let Some(name_bytes) = name_len.checked_mul(2) else {
        return false;
    };
    if cursor + name_bytes + 8 > end {
        return false;
    }
    if bytes[cursor..cursor + 2] != [0, 0] {
        return false;
    }
    cursor += name_bytes;

    let class_id_len = u32::from_be_bytes(
        bytes[cursor..cursor + 4]
            .try_into()
            .expect("class id length"),
    ) as usize;
    cursor += 4;

    let class_id_bytes = if class_id_len == 0 { 4 } else { class_id_len };
    if cursor + class_id_bytes + 4 > end {
        return false;
    }

    true
}

fn parse_legacy_slice<R: Read + Seek>(
    reader: &mut PsdReader<R>,
    bytes: &[u8],
    end: u64,
) -> Result<Slice> {
    let id = reader.read_u32()?;
    let group_id = reader.read_u32()?;
    let origin = reader.read_u32()?;
    let associated_layer_id = if origin == 1 { reader.read_u32()? } else { 0 };
    let name = reader.read_unicode_string()?;
    let slice_type = reader.read_u32()?;
    let left = reader.read_i32()?;
    let top = reader.read_i32()?;
    let right = reader.read_i32()?;
    let bottom = reader.read_i32()?;
    let url = reader.read_unicode_string()?;
    let target = reader.read_unicode_string()?;
    let message = reader.read_unicode_string()?;
    let alt_tag = reader.read_unicode_string()?;
    let cell_is_html = reader.read_u8()? != 0;
    let cell_text = reader.read_unicode_string()?;
    let horizontal_align = reader.read_i32()?;
    let vertical_align = reader.read_i32()?;
    let alpha = reader.read_u8()?;
    let bg_color = [
        alpha,
        reader.read_u8()?,
        reader.read_u8()?,
        reader.read_u8()?,
    ];
    let descriptor =
        if next_bytes_start_legacy_slice_descriptor(bytes, reader.offset as usize, end as usize) {
            reader.read_u32()?;
            Some(reader.read_descriptor_structure()?)
        } else {
            None
        };

    Ok(Slice {
        id,
        group_id,
        origin: SliceOrigin::from_u32(origin),
        associated_layer_id,
        name,
        slice_type: SliceType::from_u32(slice_type),
        bounds: SliceBounds {
            top,
            left,
            bottom,
            right,
        },
        url,
        target,
        message,
        alt_tag,
        cell_text,
        horizontal_align: SliceAlignment::from_i32(horizontal_align),
        vertical_align: SliceAlignment::from_i32(vertical_align),
        alpha,
        bg_color,
        cell_is_html,
        source_id: None,
        source_type: None,
        descriptor,
    })
}

/// Layer comps
#[derive(Debug, Clone, PartialEq)]
pub struct LayerComps {
    pub list: Vec<LayerComp>,
    pub last_applied: Option<i32>,
}

/// Layer comp
#[derive(Debug, Clone, PartialEq)]
pub struct LayerComp {
    pub id: i32,
    pub name: String,
    pub comment: Option<String>,
    pub captured_info: LayerCompCapturedInfo,
}

/// Timeline
#[derive(Debug, Clone, PartialEq)]
pub struct Timeline {
    pub enabled: bool,
    pub frame_step: Fraction,
    pub frame_rate: f64,
    pub time: Fraction,
    pub duration: Fraction,
    pub work_in_time: Fraction,
    pub work_out_time: Fraction,
    pub layer_count: i32,
    pub has_motion: bool,
}

/// Onion skins
#[derive(Debug, Clone, PartialEq)]
pub struct OnionSkins {
    pub enabled: bool,
    pub frames_before: i32,
    pub frames_after: i32,
    pub frame_spacing: i32,
    pub min_opacity: f64,
    pub max_opacity: f64,
    pub blend_mode: BlendMode,
}

/// Count information group
#[derive(Debug, Clone, PartialEq)]
pub struct CountGroup {
    pub color: Color,
    pub name: String,
    pub size: i32,
    pub font_size: i32,
    pub visible: bool,
    pub points: Vec<Point>,
}

/// URL entry
#[derive(Debug, Clone, PartialEq)]
pub struct UrlEntry {
    pub id: u32,
    pub url: String,
}

/// Resource handler function type
// Note: Function pointers can't use impl Trait, so handlers are called directly

impl<R: Read + Seek> PsdReader<R> {
    fn read_slices(&mut self, resources: &mut ImageResources, length: usize) -> Result<()> {
        if length < 4 {
            self.skip_bytes(length)?;
            return Ok(());
        }
        let version = decode_be::<U32ValueRecord>(&self.read_bytes(4)?, "slices version")?.value;
        let payload = self.read_bytes(length.saturating_sub(4))?;
        let mut reader = PsdReader::new(Cursor::new(payload.as_slice()), self.options.clone());
        let payload_end = payload.len() as u64;
        if version == 7 || version == 8 {
            let _descriptor_version =
                decode_be::<U32ValueRecord>(&reader.read_bytes(4)?, "slices descriptor version")?
                    .value;
            let descriptor = reader.read_descriptor_structure()?;
            resources.slices = Some(Slices {
                version,
                bounds: None,
                group_name: None,
                slices: Vec::new(),
                descriptor: Some(descriptor),
            });
            return Ok(());
        }
        if payload.len() < 20 {
            return Ok(());
        }
        let top = reader.read_i32()?;
        let left = reader.read_i32()?;
        let bottom = reader.read_i32()?;
        let right = reader.read_i32()?;
        let group_name = reader.read_unicode_string()?;
        let count =
            decode_be::<U32ValueRecord>(&reader.read_bytes(4)?, "slices count")?.value as usize;
        let mut slices = Vec::with_capacity(count);
        for _ in 0..count {
            if reader.bytes_left(payload_end) == 0 {
                break;
            }
            slices.push(parse_legacy_slice(
                &mut reader,
                payload.as_slice(),
                payload_end,
            )?);
        }
        resources.slices = Some(Slices {
            version,
            bounds: Some(SliceBounds {
                top,
                left,
                bottom,
                right,
            }),
            group_name: Some(group_name),
            slices,
            descriptor: None,
        });
        Ok(())
    }

    fn read_path_resource_records(
        &mut self,
        resources: &mut ImageResources,
        resource_id: u16,
        length: usize,
    ) -> Result<()> {
        let bytes = self.read_bytes(length)?;
        let mut records = Vec::new();
        let mut offset = 0usize;
        while offset + 26 <= bytes.len() {
            let record_type = u16::from_be_bytes([bytes[offset], bytes[offset + 1]]);
            offset += 2;
            let mut points = Vec::with_capacity(4);
            for _ in 0..4 {
                let y = i32::from_be_bytes(bytes[offset..offset + 4].try_into().unwrap()) as f64
                    / 16777216.0;
                offset += 4;
                let x = i32::from_be_bytes(bytes[offset..offset + 4].try_into().unwrap()) as f64
                    / 16777216.0;
                offset += 4;
                points.push(Point { x, y });
            }
            offset += 2;
            records.push(PathResourceRecord {
                record_type,
                closed: matches!(record_type, 1 | 2 | 3),
                points,
            });
        }
        if !records.is_empty() {
            resources.path_resources.insert(resource_id, records);
        }
        Ok(())
    }

    /// Read resolution info (resource 1005)
    pub fn read_resolution_info(&mut self, resources: &mut ImageResources) -> Result<()> {
        let record: ResolutionInfoRecord = decode_be(&self.read_bytes(16)?, "resolution info")?;
        let horizontal_res_unit = match record.horizontal_res_unit {
            1 => ResolutionUnit::PixelsPerInch,
            2 => ResolutionUnit::PixelsPerCentimeter,
            v => {
                return Err(PsdError::InvalidFormat(format!(
                    "Invalid resolution unit: {}",
                    v
                )))
            }
        };
        let width_unit = match record.width_unit {
            1 => MeasurementUnit::Inches,
            2 => MeasurementUnit::Centimeters,
            3 => MeasurementUnit::Points,
            4 => MeasurementUnit::Picas,
            5 => MeasurementUnit::Columns,
            v => {
                return Err(PsdError::InvalidFormat(format!(
                    "Invalid measurement unit: {}",
                    v
                )))
            }
        };

        let vertical_res_unit = match record.vertical_res_unit {
            1 => ResolutionUnit::PixelsPerInch,
            2 => ResolutionUnit::PixelsPerCentimeter,
            v => {
                return Err(PsdError::InvalidFormat(format!(
                    "Invalid resolution unit: {}",
                    v
                )))
            }
        };
        let height_unit = match record.height_unit {
            1 => MeasurementUnit::Inches,
            2 => MeasurementUnit::Centimeters,
            3 => MeasurementUnit::Points,
            4 => MeasurementUnit::Picas,
            5 => MeasurementUnit::Columns,
            v => {
                return Err(PsdError::InvalidFormat(format!(
                    "Invalid measurement unit: {}",
                    v
                )))
            }
        };

        resources.resolution_info = Some(ResolutionInfo {
            horizontal_res: record.horizontal_res_fixed as f64 / 65536.0,
            horizontal_res_unit,
            width_unit,
            vertical_res: record.vertical_res_fixed as f64 / 65536.0,
            vertical_res_unit,
            height_unit,
        });

        Ok(())
    }

    /// Read XMP metadata (resource 1060)
    pub fn read_xmp_metadata(
        &mut self,
        resources: &mut ImageResources,
        length: usize,
    ) -> Result<()> {
        let data = self.read_bytes(length)?;
        resources.xmp_metadata = Some(String::from_utf8_lossy(&data).to_string());
        Ok(())
    }

    /// Read caption digest (resource 1061)
    pub fn read_caption_digest(&mut self, resources: &mut ImageResources) -> Result<()> {
        let mut hex = String::with_capacity(32);
        for _ in 0..16 {
            hex.push_str(&format!("{:02x}", self.read_u8()?));
        }
        resources.caption_digest = Some(hex);
        Ok(())
    }

    /// Read background color (resource 1010)
    pub fn read_background_color(&mut self, resources: &mut ImageResources) -> Result<()> {
        resources.background_color = Some(self.read_color()?);
        Ok(())
    }

    /// Read print flags (resource 1011)
    pub fn read_print_flags(&mut self, resources: &mut ImageResources) -> Result<()> {
        let record: PrintFlagsRecord = decode_be(&self.read_bytes(9)?, "print flags")?;
        resources.print_flags = Some(PrintFlags {
            labels: record.labels != 0,
            crop_marks: record.crop_marks != 0,
            color_bars: record.color_bars != 0,
            registration_marks: record.registration_marks != 0,
            negative: record.negative != 0,
            flip: record.flip != 0,
            interpolate: record.interpolate != 0,
            caption: record.caption != 0,
            print_flags: record.print_flags != 0,
        });
        Ok(())
    }

    /// Read copyright flag (resource 1034)
    pub fn read_copyright_flag(&mut self, resources: &mut ImageResources) -> Result<()> {
        let record: U8BoolRecord = decode_be(&self.read_bytes(1)?, "copyright flag")?;
        resources.copyrighted = Some(record.value != 0);
        Ok(())
    }

    /// Read URL (resource 1035)
    pub fn read_url(&mut self, resources: &mut ImageResources, length: usize) -> Result<()> {
        resources.url = Some(self.read_ascii_string(length)?);
        Ok(())
    }

    /// Read grid and guides (resource 1032)
    pub fn read_grid_and_guides(&mut self, resources: &mut ImageResources) -> Result<()> {
        let header: GridAndGuidesHeaderRecord =
            decode_be(&self.read_bytes(16)?, "grid and guides header")?;
        if header.version != 1 {
            return Err(PsdError::InvalidFormat(format!(
                "Invalid grid/guides version: {}",
                header.version
            )));
        }

        let mut guides = Vec::with_capacity(header.guide_count as usize);
        for _ in 0..header.guide_count {
            let guide: GuideRecord = decode_be(&self.read_bytes(5)?, "guide record")?;
            let direction = if guide.direction == 1 {
                GuideDirection::Horizontal
            } else {
                GuideDirection::Vertical
            };
            guides.push(Guide {
                location: guide.location_times_32 as f64 / 32.0,
                direction,
            });
        }

        resources.grid_and_guides = Some(GridAndGuides {
            grid: Grid {
                horizontal: header.grid_horizontal,
                vertical: header.grid_vertical,
            },
            guides,
        });

        Ok(())
    }

    /// Read global angle (resource 1037)
    pub fn read_global_angle(&mut self, resources: &mut ImageResources) -> Result<()> {
        let record: SignedI32Record = decode_be(&self.read_bytes(4)?, "global angle")?;
        resources.global_angle = Some(record.value);
        Ok(())
    }

    /// Read global altitude (resource 1049)
    pub fn read_global_altitude(&mut self, resources: &mut ImageResources) -> Result<()> {
        let record: SignedI32Record = decode_be(&self.read_bytes(4)?, "global altitude")?;
        resources.global_altitude = Some(record.value);
        Ok(())
    }

    /// Read layer state (resource 1024)
    pub fn read_layer_state(&mut self, resources: &mut ImageResources) -> Result<()> {
        let record: LayerStateRecord = decode_be(&self.read_bytes(2)?, "layer state")?;
        resources.layer_state = Some(record.state);
        Ok(())
    }

    /// Read layer group IDs (resource 1026)
    pub fn read_layer_group_ids(
        &mut self,
        resources: &mut ImageResources,
        length: usize,
    ) -> Result<()> {
        let mut groups = Vec::new();
        let count = length / 2;
        for _ in 0..count {
            groups.push(self.read_u16()?);
        }
        resources.layer_group_ids = Some(groups);
        Ok(())
    }

    /// Read layer selection IDs (resource 1069)
    pub fn read_layer_selection_ids(&mut self, resources: &mut ImageResources) -> Result<()> {
        let count = decode_be::<U16ListCountRecord>(&self.read_bytes(2)?, "layer selection count")?
            .count as usize;
        let mut ids = Vec::new();
        for _ in 0..count {
            ids.push(
                decode_be::<U32ValueRecord>(&self.read_bytes(4)?, "layer selection id")?.value,
            );
        }
        resources.layer_selection_ids = Some(ids);
        Ok(())
    }

    /// Read alpha names (resource 1006)
    pub fn read_alpha_names(
        &mut self,
        resources: &mut ImageResources,
        length: usize,
    ) -> Result<()> {
        let mut names = Vec::new();
        let mut remaining = length;

        while remaining > 0 {
            let name_len = self.read_u8()? as usize;
            remaining -= 1;
            let name = self.read_bytes(name_len)?;
            remaining -= name_len;
            names.push(String::from_utf8_lossy(&name).to_string());
        }

        resources.alpha_names = Some(names);
        Ok(())
    }

    /// Read alpha unicode names (resource 1045)
    pub fn read_alpha_unicode_names(
        &mut self,
        resources: &mut ImageResources,
        length: usize,
    ) -> Result<()> {
        let mut names = Vec::new();
        let mut remaining = length;

        while remaining >= 6 {
            let name_length = self.read_u32()? as usize;
            if name_length == 0 {
                break;
            }
            // name_length includes null terminator: read (name_length - 1) chars, then skip null
            let char_count = name_length - 1;
            let name = self.read_unicode_string_with_length(char_count)?;
            let _null = self.read_u16()?; // consume null terminator
            names.push(name);
            remaining = remaining.saturating_sub(4 + char_count * 2 + 2);
        }

        resources.alpha_unicode_names = Some(names);
        Ok(())
    }

    /// Read alpha identifiers (resource 1053)
    pub fn read_alpha_identifiers(
        &mut self,
        resources: &mut ImageResources,
        length: usize,
    ) -> Result<()> {
        let count = length / 4;
        let mut ids = Vec::new();
        for _ in 0..count {
            ids.push(decode_be::<U32ValueRecord>(&self.read_bytes(4)?, "alpha identifier")?.value);
        }
        resources.alpha_identifiers = Some(ids);
        Ok(())
    }

    /// Read ICC profile (resource 1039)
    pub fn read_icc_profile(
        &mut self,
        resources: &mut ImageResources,
        length: usize,
    ) -> Result<()> {
        resources.icc_profile = Some(self.read_bytes(length)?);
        Ok(())
    }

    /// Read print scale (resource 1062)
    pub fn read_print_scale(&mut self, resources: &mut ImageResources) -> Result<()> {
        let record: PrintScaleRecord = decode_be(&self.read_bytes(14)?, "print scale")?;
        let style = match record.style {
            0 => PrintScaleStyle::Centered,
            1 => PrintScaleStyle::SizeToFit,
            2 => PrintScaleStyle::UserDefined,
            v => {
                return Err(PsdError::InvalidFormat(format!(
                    "Invalid print scale style: {}",
                    v
                )))
            }
        };

        resources.print_scale = Some(PrintScale {
            style,
            x: record.x,
            y: record.y,
            scale: record.scale,
        });
        Ok(())
    }
}

impl PsdWriter {
    /// Write resolution info
    pub fn write_resolution_info(&mut self, info: &ResolutionInfo) -> Result<()> {
        let record = ResolutionInfoRecord {
            horizontal_res_fixed: (info.horizontal_res * 65536.0) as i32,
            horizontal_res_unit: info.horizontal_res_unit as u16,
            width_unit: info.width_unit as u16,
            vertical_res_fixed: (info.vertical_res * 65536.0) as i32,
            vertical_res_unit: info.vertical_res_unit as u16,
            height_unit: info.height_unit as u16,
        };
        self.write_bytes(&encode_be(&record, "resolution info")?)
    }

    /// Write XMP metadata
    pub fn write_xmp_metadata(&mut self, xmp: &str) -> Result<()> {
        self.write_bytes(xmp.as_bytes())?;
        Ok(())
    }

    /// Write caption digest
    pub fn write_caption_digest(&mut self, digest: &str) -> Result<()> {
        for i in 0..16 {
            let byte = u8::from_str_radix(&digest[i * 2..i * 2 + 2], 16)
                .map_err(|_| PsdError::InvalidFormat("Invalid hex digest".to_string()))?;
            self.write_u8(byte)?;
        }
        Ok(())
    }

    /// Write background color
    pub fn write_background_color(&mut self, color: &Color) -> Result<()> {
        self.write_color(Some(color))?;
        Ok(())
    }

    /// Write print flags
    pub fn write_print_flags(&mut self, flags: &PrintFlags) -> Result<()> {
        let record = PrintFlagsRecord {
            labels: u8::from(flags.labels),
            crop_marks: u8::from(flags.crop_marks),
            color_bars: u8::from(flags.color_bars),
            registration_marks: u8::from(flags.registration_marks),
            negative: u8::from(flags.negative),
            flip: u8::from(flags.flip),
            interpolate: u8::from(flags.interpolate),
            caption: u8::from(flags.caption),
            print_flags: u8::from(flags.print_flags),
        };
        self.write_bytes(&encode_be(&record, "print flags")?)
    }

    /// Write copyright flag
    pub fn write_copyright_flag(&mut self, copyrighted: bool) -> Result<()> {
        self.write_bytes(&encode_be(
            &U8BoolRecord {
                value: u8::from(copyrighted),
            },
            "copyright flag",
        )?)
    }

    /// Write URL
    pub fn write_url(&mut self, url: &str) -> Result<()> {
        self.write_ascii_string(url)?;
        Ok(())
    }

    /// Write grid and guides
    pub fn write_grid_and_guides(&mut self, grid_guides: &GridAndGuides) -> Result<()> {
        let header = GridAndGuidesHeaderRecord {
            version: 1,
            grid_horizontal: grid_guides.grid.horizontal,
            grid_vertical: grid_guides.grid.vertical,
            guide_count: grid_guides.guides.len() as u32,
        };
        self.write_bytes(&encode_be(&header, "grid and guides header")?)?;

        for guide in &grid_guides.guides {
            let record = GuideRecord {
                location_times_32: (guide.location * 32.0) as u32,
                direction: if guide.direction == GuideDirection::Horizontal {
                    1
                } else {
                    0
                },
            };
            self.write_bytes(&encode_be(&record, "guide record")?)?;
        }

        Ok(())
    }

    /// Write global angle
    pub fn write_global_angle(&mut self, angle: i32) -> Result<()> {
        self.write_bytes(&encode_be(
            &SignedI32Record { value: angle },
            "global angle",
        )?)
    }

    /// Write global altitude
    pub fn write_global_altitude(&mut self, altitude: i32) -> Result<()> {
        self.write_bytes(&encode_be(
            &SignedI32Record { value: altitude },
            "global altitude",
        )?)
    }

    /// Write layer state
    pub fn write_layer_state(&mut self, state: u16) -> Result<()> {
        self.write_bytes(&encode_be(&LayerStateRecord { state }, "layer state")?)
    }

    /// Write layer group IDs
    pub fn write_layer_group_ids(&mut self, group_ids: &[u16]) -> Result<()> {
        for value in group_ids {
            self.write_u16(*value)?;
        }
        Ok(())
    }

    /// Write layer selection IDs
    pub fn write_layer_selection_ids(&mut self, ids: &[u32]) -> Result<()> {
        self.write_bytes(&encode_be(
            &U16ListCountRecord {
                count: ids.len() as u16,
            },
            "layer selection count",
        )?)?;
        for id in ids {
            self.write_bytes(&encode_be(
                &U32ValueRecord { value: *id },
                "layer selection id",
            )?)?;
        }
        Ok(())
    }

    /// Write print scale
    pub fn write_print_scale(&mut self, scale: &PrintScale) -> Result<()> {
        self.write_bytes(&encode_be(
            &PrintScaleRecord {
                style: scale.style as i16,
                x: scale.x,
                y: scale.y,
                scale: scale.scale,
            },
            "print scale",
        )?)
    }
}

/// Process all image resources from reader
pub fn read_image_resources<R: Read + Seek>(
    reader: &mut PsdReader<R>,
    length: usize,
) -> Result<ImageResources> {
    let mut resources = ImageResources::default();
    let start_offset = reader.offset;

    while (reader.offset - start_offset) < length as u64 {
        let header: ImageResourceHeaderRecord =
            decode_be(&reader.read_bytes(7)?, "image resource header")?;
        if &header.signature != b"8BIM" && &header.signature != b"MeSa" {
            return Err(PsdError::InvalidFormat(format!(
                "Invalid resource signature: {}",
                String::from_utf8_lossy(&header.signature),
            )));
        }
        let resource_id = header.resource_id;
        resources.resource_order.push(resource_id);

        // Read pascal string (name)
        let name_length = header.name_length as usize;
        let name = reader.read_bytes(name_length)?;
        if (name_length + 1) % 2 != 0 {
            reader.skip_bytes(1)?; // Padding
        }

        let data_length = decode_be::<ImageResourceLengthRecord>(
            &reader.read_bytes(4)?,
            "image resource length",
        )?
        .data_length as usize;
        let resource_start = reader.offset;

        // Dispatch to appropriate handler
        match resource_id {
            1005 => reader.read_resolution_info(&mut resources)?,
            1010 => reader.read_background_color(&mut resources)?,
            1011 => reader.read_print_flags(&mut resources)?,
            1024 => reader.read_layer_state(&mut resources)?,
            1026 => reader.read_layer_group_ids(&mut resources, data_length)?,
            1032 => reader.read_grid_and_guides(&mut resources)?,
            1036 => {
                let bytes = reader.read_bytes(data_length)?;
                resources.thumbnail = parse_thumbnail_resource(&bytes);
            }
            1077 => {
                let bytes = reader.read_bytes(data_length)?;
                resources.display_info_typed = parse_display_info_resource(&bytes);
            }
            1034 => reader.read_copyright_flag(&mut resources)?,
            1035 => reader.read_url(&mut resources, data_length)?,
            1037 => reader.read_global_angle(&mut resources)?,
            1049 => reader.read_global_altitude(&mut resources)?,
            1060 => reader.read_xmp_metadata(&mut resources, data_length)?,
            1061 => reader.read_caption_digest(&mut resources)?,
            1062 => reader.read_print_scale(&mut resources)?,
            1069 => reader.read_layer_selection_ids(&mut resources)?,
            1006 => reader.read_alpha_names(&mut resources, data_length)?,
            1045 => reader.read_alpha_unicode_names(&mut resources, data_length)?,
            1053 => reader.read_alpha_identifiers(&mut resources, data_length)?,
            1039 => reader.read_icc_profile(&mut resources, data_length)?,
            1050 => reader.read_slices(&mut resources, data_length)?,
            1072 => {
                let bytes = reader.read_bytes(data_length)?;
                resources.resource_visibility_typed = Some(ResourceVisibility {
                    values: bytes.into_iter().map(|b| b == 1).collect(),
                });
            }
            1073 => {
                let bytes = reader.read_bytes(data_length)?;
                resources.color_samplers_typed = Some(parse_color_samplers_resource(&bytes));
            }
            2999 => {
                resources.clipping_path_name = Some(reader.read_pascal_string(2)?);
            }
            1065 | 1074 | 1075 | 3000 => {
                let _version = decode_be::<U32ValueRecord>(
                    &reader.read_bytes(4)?,
                    "descriptor resource version",
                )?
                .value;
                let desc = reader.read_descriptor_structure()?;
                resources.descriptor_resources.insert(resource_id, desc);
            }
            2000..=2998 => {
                reader.read_path_resource_records(&mut resources, resource_id, data_length)?
            }
            7000 => {
                let bytes = reader.read_bytes(data_length)?;
                resources.variables = Some(String::from_utf8_lossy(&bytes).to_string());
            }
            7001 => {
                let bytes = reader.read_bytes(data_length)?;
                resources.data_sets = Some(String::from_utf8_lossy(&bytes).to_string());
            }
            _ => {
                let data = reader.read_bytes(data_length)?;
                let mut raw = Vec::new();
                raw.extend_from_slice(&header.signature);
                raw.extend_from_slice(&resource_id.to_be_bytes());
                raw.push(header.name_length);
                raw.extend_from_slice(&name);
                if (name_length + 1) % 2 != 0 {
                    raw.push(0);
                }
                raw.extend_from_slice(&(data_length as u32).to_be_bytes());
                raw.extend_from_slice(&data);
                if data_length % 2 != 0 {
                    raw.push(0);
                }
                resources.raw_resources.push(RawImageResource {
                    resource_id,
                    bytes: raw,
                });
            }
        }

        reader.seek_to(resource_start + data_length as u64)?;
        if data_length % 2 != 0 {
            reader.skip_bytes(1)?;
        }
    }

    Ok(resources)
}

/// Write all image resources
pub fn write_image_resources(writer: &mut PsdWriter, resources: &ImageResources) -> Result<()> {
    use std::collections::{HashMap, HashSet, VecDeque};

    // Helper to write a resource block
    let build_resource = |id: u16, write_fn: &dyn Fn(&mut PsdWriter) -> Result<()>| -> Result<Vec<u8>> {
        let mut resource = PsdWriter::new(1024);
        resource.write_bytes(&encode_be(
            &ImageResourceHeaderRecord {
                signature: *b"8BIM",
                resource_id: id,
                name_length: 0,
            },
            "image resource header",
        )?)?;
        resource.write_u8(0)?; // Name padding

        // Write to temp buffer to get length
        let mut temp_writer = PsdWriter::new(1024);
        write_fn(&mut temp_writer)?;
        let data = temp_writer.get_buffer();

        resource.write_bytes(&encode_be(
            &ImageResourceLengthRecord {
                data_length: data.len() as u32,
            },
            "image resource length",
        )?)?;
        resource.write_bytes(data)?;

        // Pad to even boundary
        if data.len() % 2 != 0 {
            resource.write_u8(0)?;
        }

        Ok(resource.into_buffer())
    };

    let mut typed_resources = Vec::new();

    // Write each resource type
    if let Some(ref info) = resources.resolution_info {
        typed_resources.push((1005, build_resource(1005, &|w| w.write_resolution_info(info))?));
    }

    if let Some(ref color) = resources.background_color {
        typed_resources.push((1010, build_resource(1010, &|w| w.write_background_color(color))?));
    }

    if let Some(ref flags) = resources.print_flags {
        typed_resources.push((1011, build_resource(1011, &|w| w.write_print_flags(flags))?));
    }

    if let Some(state) = resources.layer_state {
        typed_resources.push((1024, build_resource(1024, &|w| w.write_layer_state(state))?));
    }

    if let Some(ref group_ids) = resources.layer_group_ids {
        typed_resources.push((1026, build_resource(1026, &|w| w.write_layer_group_ids(group_ids))?));
    }

    if let Some(ref grid_guides) = resources.grid_and_guides {
        typed_resources.push((1032, build_resource(1032, &|w| w.write_grid_and_guides(grid_guides))?));
    }

    if let Some(copyrighted) = resources.copyrighted {
        typed_resources.push((1034, build_resource(1034, &|w| w.write_copyright_flag(copyrighted))?));
    }

    if let Some(ref url) = resources.url {
        typed_resources.push((1035, build_resource(1035, &|w| w.write_url(url))?));
    }

    if let Some(angle) = resources.global_angle {
        typed_resources.push((1037, build_resource(1037, &|w| w.write_global_angle(angle))?));
    }

    if let Some(altitude) = resources.global_altitude {
        typed_resources.push((1049, build_resource(1049, &|w| w.write_global_altitude(altitude))?));
    }

    if let Some(ref xmp) = resources.xmp_metadata {
        typed_resources.push((1060, build_resource(1060, &|w| w.write_xmp_metadata(xmp))?));
    }

    if let Some(ref digest) = resources.caption_digest {
        typed_resources.push((1061, build_resource(1061, &|w| w.write_caption_digest(digest))?));
    }

    if let Some(ref scale) = resources.print_scale {
        typed_resources.push((1062, build_resource(1062, &|w| w.write_print_scale(scale))?));
    }

    if let Some(ref ids) = resources.layer_selection_ids {
        typed_resources.push((1069, build_resource(1069, &|w| w.write_layer_selection_ids(ids))?));
    }

    if let Some(ref thumbnail) = resources.thumbnail {
        typed_resources.push((1036, build_resource(1036, &|w| {
            w.write_bytes(&build_thumbnail_resource(thumbnail))
        })?));
    }

    if let Some(ref info) = resources.display_info_typed {
        typed_resources.push((1077, build_resource(1077, &|w| {
            w.write_bytes(&build_display_info_resource(info))
        })?));
    }

    if let Some(ref visibility) = resources.resource_visibility_typed {
        typed_resources.push((1072, build_resource(1072, &|w| {
            w.write_bytes(
                &visibility
                    .values
                    .iter()
                    .map(|v| if *v { 1 } else { 0 })
                    .collect::<Vec<u8>>(),
            )
        })?));
    }

    if let Some(ref points) = resources.color_samplers_typed {
        typed_resources.push((1073, build_resource(1073, &|w| {
            w.write_bytes(&build_color_samplers_resource(points)?)
        })?));
    }

    if let Some(ref clipping_path_name) = resources.clipping_path_name {
        typed_resources.push((2999, build_resource(2999, &|w| {
            w.write_pascal_string(clipping_path_name, 2)
        })?));
    }

    if let Some(ref slices) = resources.slices {
        typed_resources.push((1050, build_resource(1050, &|w| {
            if let Some(ref descriptor) = slices.descriptor {
                w.write_u32(slices.version)?;
                w.write_u32(16)?;
                w.write_descriptor_structure(descriptor)?;
                return Ok(());
            }

            w.write_u32(6)?;
            let bounds = slices.bounds.unwrap_or(SliceBounds {
                top: 0,
                left: 0,
                bottom: 0,
                right: 0,
            });
            w.write_i32(bounds.top)?;
            w.write_i32(bounds.left)?;
            w.write_i32(bounds.bottom)?;
            w.write_i32(bounds.right)?;
            w.write_unicode_string(slices.group_name.as_deref().unwrap_or(""))?;
            w.write_u32(slices.slices.len() as u32)?;
            for slice in &slices.slices {
                if slice.source_id.is_some() || slice.source_type.is_some() {
                    return Err(PsdError::UnsupportedFeature(
                        "v6 slice source_id/source_type are unsupported by tightened descriptor-tail framing"
                            .to_string(),
                    ));
                }
                w.write_i32(slice.id as i32)?;
                w.write_i32(slice.group_id as i32)?;
                w.write_i32(slice.origin.to_u32() as i32)?;
                if slice.origin == SliceOrigin::LayerBased {
                    w.write_i32(slice.associated_layer_id as i32)?;
                }
                w.write_unicode_string(&slice.name)?;
                w.write_i32(slice.slice_type.to_u32() as i32)?;
                w.write_i32(slice.bounds.left)?;
                w.write_i32(slice.bounds.top)?;
                w.write_i32(slice.bounds.right)?;
                w.write_i32(slice.bounds.bottom)?;
                w.write_unicode_string(&slice.url)?;
                w.write_unicode_string(&slice.target)?;
                w.write_unicode_string(&slice.message)?;
                w.write_unicode_string(&slice.alt_tag)?;
                w.write_u8(u8::from(slice.cell_is_html))?;
                w.write_unicode_string(&slice.cell_text)?;
                w.write_i32(slice.horizontal_align.to_i32())?;
                w.write_i32(slice.vertical_align.to_i32())?;
                w.write_u8(slice.alpha)?;
                w.write_u8(slice.bg_color[1])?;
                w.write_u8(slice.bg_color[2])?;
                w.write_u8(slice.bg_color[3])?;
                if let Some(ref descriptor) = slice.descriptor {
                    w.write_u32(16)?;
                    w.write_descriptor_structure(descriptor)?;
                }
            }
            Ok(())
        })?));
    }

    // Write alpha names (1006)
    if let Some(ref names) = resources.alpha_names {
        typed_resources.push((1006, build_resource(1006, &|w| {
            for name in names {
                w.write_u8(name.len() as u8)?;
                w.write_bytes(name.as_bytes())?;
            }
            Ok(())
        })?));
    }

    // Write alpha unicode names (1045)
    if let Some(ref names) = resources.alpha_unicode_names {
        typed_resources.push((1045, build_resource(1045, &|w| {
            for name in names {
                w.write_unicode_string(name)?;
            }
            Ok(())
        })?));
    }

    // Write alpha identifiers (1053)
    if let Some(ref ids) = resources.alpha_identifiers {
        typed_resources.push((1053, build_resource(1053, &|w| {
            for &id in ids {
                w.write_u32(id)?;
            }
            Ok(())
        })?));
    }

    // Write ICC profile (1039)
    if let Some(ref profile) = resources.icc_profile {
        typed_resources.push((1039, build_resource(1039, &|w| w.write_bytes(profile))?));
    }

    // Write descriptor resources (1065, 1074, 1075)
    for (&id, desc) in &resources.descriptor_resources {
        typed_resources.push((id, build_resource(id, &|w| {
            w.write_bytes(&encode_be(
                &U32ValueRecord { value: 16 },
                "descriptor resource version",
            )?)?;
            w.write_descriptor_structure(desc)
        })?));
    }

    for (&id, records) in &resources.path_resources {
        typed_resources.push((id, build_resource(id, &|w| {
            for record in records {
                w.write_u16(record.record_type)?;
                for point in record.points.iter().take(4) {
                    w.write_i32((point.y * 16777216.0).round() as i32)?;
                    w.write_i32((point.x * 16777216.0).round() as i32)?;
                }
                for _ in record.points.len()..4 {
                    w.write_i32(0)?;
                    w.write_i32(0)?;
                }
                w.write_u16(0)?;
            }
            Ok(())
        })?));
    }

    if let Some(ref xml) = resources.variables {
        typed_resources.push((7000, build_resource(7000, &|w| w.write_bytes(xml.as_bytes()))?));
    }
    if let Some(ref xml) = resources.data_sets {
        typed_resources.push((7001, build_resource(7001, &|w| w.write_bytes(xml.as_bytes()))?));
    }

    let typed_ids: HashSet<u16> = typed_resources.iter().map(|(id, _)| *id).collect();
    let mut typed_by_id: HashMap<u16, VecDeque<Vec<u8>>> = HashMap::new();
    for (id, bytes) in typed_resources {
        typed_by_id.entry(id).or_default().push_back(bytes);
    }
    let mut raw_by_id: HashMap<u16, VecDeque<Vec<u8>>> = HashMap::new();
    for raw in &resources.raw_resources {
        raw_by_id
            .entry(raw.resource_id)
            .or_default()
            .push_back(raw.bytes.clone());
    }

    if !resources.resource_order.is_empty() {
        for resource_id in &resources.resource_order {
            if let Some(queue) = raw_by_id.get_mut(resource_id) {
                if let Some(bytes) = queue.pop_front() {
                    writer.write_bytes(&bytes)?;
                    continue;
                }
            }
            if let Some(queue) = typed_by_id.get_mut(resource_id) {
                if let Some(bytes) = queue.pop_front() {
                    writer.write_bytes(&bytes)?;
                }
            }
        }
    }

    let mut remaining_typed_ids = typed_by_id.keys().copied().collect::<Vec<_>>();
    remaining_typed_ids.sort_unstable();
    for resource_id in remaining_typed_ids {
        if let Some(queue) = typed_by_id.get_mut(&resource_id) {
            while let Some(bytes) = queue.pop_front() {
                writer.write_bytes(&bytes)?;
            }
        }
    }

    for raw in &resources.raw_resources {
        if !typed_ids.contains(&raw.resource_id) {
            if let Some(queue) = raw_by_id.get_mut(&raw.resource_id) {
                if let Some(bytes) = queue.pop_front() {
                    writer.write_bytes(&bytes)?;
                }
            }
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn parse_resource_ids(bytes: &[u8]) -> Vec<u16> {
        let mut ids = Vec::new();
        let mut pos = 0usize;
        while pos + 7 <= bytes.len() {
            let header: ImageResourceHeaderRecord =
                decode_be(&bytes[pos..pos + 7], "image resource header").expect("header");
            pos += 7;
            let name_len = header.name_length as usize;
            pos += name_len;
            if (name_len + 1) % 2 != 0 {
                pos += 1;
            }
            let length: ImageResourceLengthRecord = decode_be(
                &bytes[pos..pos + 4],
                "image resource length",
            )
            .expect("length");
            pos += 4;
            ids.push(header.resource_id);
            pos += length.data_length as usize;
            if length.data_length % 2 != 0 {
                pos += 1;
            }
        }
        ids
    }

    fn samples_dir() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("tests")
            .join("fixtures")
            .join("samples")
    }

    #[test]
    fn test_resolution_info_roundtrip() {
        let info = ResolutionInfo {
            horizontal_res: 72.0,
            horizontal_res_unit: ResolutionUnit::PixelsPerInch,
            width_unit: MeasurementUnit::Inches,
            vertical_res: 72.0,
            vertical_res_unit: ResolutionUnit::PixelsPerInch,
            height_unit: MeasurementUnit::Inches,
        };

        let mut writer = PsdWriter::new(128);
        writer.write_resolution_info(&info).unwrap();

        let buffer = writer.into_buffer();
        let cursor = std::io::Cursor::new(buffer);
        let mut reader = PsdReader::new(cursor, Default::default());

        let mut resources = ImageResources::default();
        reader.read_resolution_info(&mut resources).unwrap();

        let read_info = resources.resolution_info.unwrap();
        assert_eq!(read_info.horizontal_res, info.horizontal_res);
        assert_eq!(read_info.vertical_res, info.vertical_res);
    }

    #[test]
    fn test_grid_and_guides_roundtrip() {
        let grid_guides = GridAndGuides {
            grid: Grid {
                horizontal: 576,
                vertical: 576,
            },
            guides: vec![
                Guide {
                    location: 100.0,
                    direction: GuideDirection::Vertical,
                },
                Guide {
                    location: 200.0,
                    direction: GuideDirection::Horizontal,
                },
            ],
        };

        let mut writer = PsdWriter::new(256);
        writer.write_grid_and_guides(&grid_guides).unwrap();

        let buffer = writer.into_buffer();
        let cursor = std::io::Cursor::new(buffer);
        let mut reader = PsdReader::new(cursor, Default::default());

        let mut resources = ImageResources::default();
        reader.read_grid_and_guides(&mut resources).unwrap();

        let read_grid_guides = resources.grid_and_guides.unwrap();
        assert_eq!(read_grid_guides, grid_guides);
    }

    #[test]
    fn test_print_flags_roundtrip() {
        let flags = PrintFlags {
            labels: true,
            crop_marks: false,
            color_bars: true,
            registration_marks: false,
            negative: true,
            flip: false,
            interpolate: true,
            caption: false,
            print_flags: true,
        };

        let mut writer = PsdWriter::new(64);
        writer.write_print_flags(&flags).unwrap();

        let buffer = writer.into_buffer();
        let cursor = std::io::Cursor::new(buffer);
        let mut reader = PsdReader::new(cursor, Default::default());

        let mut resources = ImageResources::default();
        reader.read_print_flags(&mut resources).unwrap();

        assert_eq!(resources.print_flags, Some(flags));
    }

    #[test]
    fn test_print_scale_roundtrip() {
        let scale = PrintScale {
            style: PrintScaleStyle::UserDefined,
            x: 12.5,
            y: 42.0,
            scale: 66.0,
        };

        let mut writer = PsdWriter::new(64);
        writer.write_print_scale(&scale).unwrap();

        let buffer = writer.into_buffer();
        let cursor = std::io::Cursor::new(buffer);
        let mut reader = PsdReader::new(cursor, Default::default());

        let mut resources = ImageResources::default();
        reader.read_print_scale(&mut resources).unwrap();

        assert_eq!(resources.print_scale, Some(scale));
    }

    #[test]
    fn test_layer_selection_ids_roundtrip() {
        let ids = vec![11, 22, 33];

        let mut writer = PsdWriter::new(64);
        writer.write_layer_selection_ids(&ids).unwrap();

        let buffer = writer.into_buffer();
        let cursor = std::io::Cursor::new(buffer);
        let mut reader = PsdReader::new(cursor, Default::default());

        let mut resources = ImageResources::default();
        reader.read_layer_selection_ids(&mut resources).unwrap();

        assert_eq!(resources.layer_selection_ids, Some(ids));
    }

    #[test]
    fn path_selection_descriptor_roundtrip() {
        let mut resources = ImageResources::default();
        let mut desc = Descriptor {
            name: String::new(),
            class_id: "null".to_string(),
            items: HashMap::new(),
        };
        desc.items.insert(
            "path".to_string(),
            crate::support::descriptor::DescriptorValue::Text("selection".to_string()),
        );
        resources.descriptor_resources.insert(3000, desc.clone());

        let mut w = PsdWriter::new(256);
        write_image_resources(&mut w, &resources).unwrap();
        let buf = w.into_buffer();
        let len = buf.len();
        let mut reader = PsdReader::new(std::io::Cursor::new(buf), Default::default());
        let reparsed = read_image_resources(&mut reader, len).unwrap();

        assert_eq!(reparsed.descriptor_resources.get(&3000), Some(&desc));
    }

    #[test]
    fn duplicate_unknown_image_resources_preserve_multiplicity_and_order() {
        let mut bytes = Vec::new();
        let mut writer = PsdWriter::new(64);
        writer
            .write_bytes(&encode_be(
                &ImageResourceHeaderRecord {
                    signature: *b"8BIM",
                    resource_id: 8000,
                    name_length: 0,
                },
                "image resource header",
            ).unwrap())
            .unwrap();
        writer.write_u8(0).unwrap();
        writer
            .write_bytes(&encode_be(&ImageResourceLengthRecord { data_length: 1 }, "len").unwrap())
            .unwrap();
        writer.write_u8(1).unwrap();
        writer.write_u8(0).unwrap();
        writer
            .write_bytes(&encode_be(
                &ImageResourceHeaderRecord {
                    signature: *b"8BIM",
                    resource_id: 8001,
                    name_length: 0,
                },
                "image resource header",
            ).unwrap())
            .unwrap();
        writer.write_u8(0).unwrap();
        writer
            .write_bytes(&encode_be(&ImageResourceLengthRecord { data_length: 1 }, "len").unwrap())
            .unwrap();
        writer.write_u8(2).unwrap();
        writer.write_u8(0).unwrap();
        writer
            .write_bytes(&encode_be(
                &ImageResourceHeaderRecord {
                    signature: *b"8BIM",
                    resource_id: 8000,
                    name_length: 0,
                },
                "image resource header",
            ).unwrap())
            .unwrap();
        writer.write_u8(0).unwrap();
        writer
            .write_bytes(&encode_be(&ImageResourceLengthRecord { data_length: 1 }, "len").unwrap())
            .unwrap();
        writer.write_u8(3).unwrap();
        writer.write_u8(0).unwrap();
        bytes.extend_from_slice(writer.get_buffer());

        let mut reader = PsdReader::new(std::io::Cursor::new(bytes.clone()), Default::default());
        let resources = read_image_resources(&mut reader, bytes.len()).unwrap();
        assert_eq!(resources.resource_order, vec![8000, 8001, 8000]);
        assert_eq!(resources.raw_resources.len(), 3);

        let mut out = PsdWriter::new(64);
        write_image_resources(&mut out, &resources).unwrap();
        assert_eq!(out.into_buffer(), bytes);
    }

    #[test]
    fn modeled_image_resource_underread_does_not_desync_next_resource() {
        let mut writer = PsdWriter::new(64);
        writer.write_signature("8BIM").unwrap();
        writer.write_u16(1037).unwrap();
        writer.write_u8(0).unwrap();
        writer.write_u8(0).unwrap();
        writer.write_u32(6).unwrap();
        writer.write_u32(90).unwrap();
        writer.write_u16(0xBEEF).unwrap();
        writer.write_signature("8BIM").unwrap();
        writer.write_u16(8001).unwrap();
        writer.write_u8(0).unwrap();
        writer.write_u8(0).unwrap();
        writer.write_u32(1).unwrap();
        writer.write_u8(2).unwrap();
        writer.write_u8(0).unwrap();

        let bytes = writer.into_buffer();
        let len = bytes.len();
        let mut reader = PsdReader::new(std::io::Cursor::new(bytes), Default::default());
        let resources = read_image_resources(&mut reader, len).unwrap();

        assert_eq!(resources.global_angle, Some(90));
        assert_eq!(resources.resource_order, vec![1037, 8001]);
        assert_eq!(resources.raw_resources.len(), 1);
        assert_eq!(resources.raw_resources[0].resource_id, 8001);
    }

    #[test]
    fn mixed_modeled_and_unknown_image_resources_preserve_original_order() {
        let mut original = PsdWriter::new(128);
        original
            .write_bytes(&encode_be(
                &ImageResourceHeaderRecord {
                    signature: *b"8BIM",
                    resource_id: 8000,
                    name_length: 0,
                },
                "image resource header",
            ).unwrap())
            .unwrap();
        original.write_u8(0).unwrap();
        original
            .write_bytes(&encode_be(&ImageResourceLengthRecord { data_length: 1 }, "len").unwrap())
            .unwrap();
        original.write_u8(9).unwrap();
        original.write_u8(0).unwrap();
        let mut typed = ImageResources::default();
        typed.resolution_info = Some(ResolutionInfo {
            horizontal_res: 72.0,
            horizontal_res_unit: ResolutionUnit::PixelsPerInch,
            width_unit: MeasurementUnit::Inches,
            vertical_res: 72.0,
            vertical_res_unit: ResolutionUnit::PixelsPerInch,
            height_unit: MeasurementUnit::Inches,
        });
        let mut typed_writer = PsdWriter::new(64);
        write_image_resources(&mut typed_writer, &typed).unwrap();
        original.write_bytes(&typed_writer.into_buffer()).unwrap();
        original
            .write_bytes(&encode_be(
                &ImageResourceHeaderRecord {
                    signature: *b"8BIM",
                    resource_id: 8001,
                    name_length: 0,
                },
                "image resource header",
            ).unwrap())
            .unwrap();
        original.write_u8(0).unwrap();
        original
            .write_bytes(&encode_be(&ImageResourceLengthRecord { data_length: 1 }, "len").unwrap())
            .unwrap();
        original.write_u8(8).unwrap();
        original.write_u8(0).unwrap();
        let bytes = original.into_buffer();

        let mut reader = PsdReader::new(std::io::Cursor::new(bytes.clone()), Default::default());
        let resources = read_image_resources(&mut reader, bytes.len()).unwrap();
        let mut out = PsdWriter::new(128);
        write_image_resources(&mut out, &resources).unwrap();
        assert_eq!(out.into_buffer(), bytes);
    }

    #[test]
    fn sample_image_resource_id_sequence_is_stable_after_roundtrip() {
        let path = samples_dir().join("4901393.psd");
        let original = std::fs::read(path).unwrap();
        let orig_len = u32::from_be_bytes(original[30..34].try_into().unwrap()) as usize;
        let orig_resources = &original[34..34 + orig_len];
        let original_ids = parse_resource_ids(orig_resources);

        let mut reader = PsdReader::new(std::io::Cursor::new(orig_resources.to_vec()), Default::default());
        let resources = read_image_resources(&mut reader, orig_resources.len()).unwrap();
        let mut out = PsdWriter::new(orig_resources.len() + 1024);
        write_image_resources(&mut out, &resources).unwrap();
        let rewritten = out.into_buffer();
        let rewritten_ids = parse_resource_ids(&rewritten);

        assert_eq!(rewritten_ids, original_ids);
    }

    #[test]
    fn clipping_path_name_roundtrip() {
        let mut resources = ImageResources::default();
        resources.clipping_path_name = Some("Path 1".to_string());

        let mut w = PsdWriter::new(128);
        write_image_resources(&mut w, &resources).unwrap();
        let buf = w.into_buffer();
        let len = buf.len();
        let mut reader = PsdReader::new(std::io::Cursor::new(buf), Default::default());
        let reparsed = read_image_resources(&mut reader, len).unwrap();

        assert_eq!(reparsed.clipping_path_name.as_deref(), Some("Path 1"));
    }

    #[test]
    fn thumbnail_format_matches_spec_values() {
        let thumbnail = Thumbnail {
            width: 1,
            height: 1,
            format: ThumbnailFormat::JpegRgb,
            data: vec![0xFF, 0xD8, 0xFF],
        };
        let bytes = build_thumbnail_resource(&thumbnail);
        assert_eq!(u32::from_be_bytes(bytes[0..4].try_into().unwrap()), 1);
    }

    #[test]
    fn thumbnail_header_roundtrips_via_binrw() {
        let record = crate::support::binrw_support::ThumbnailHeaderRecord {
            format: 1,
            width: 5,
            height: 7,
            width_bytes: 16,
            total_size: 112,
            compressed_size: 13,
            bits_per_pixel: 24,
            planes: 1,
        };

        let bytes = encode_be(&record, "thumbnail header").expect("encode");
        let reparsed: crate::support::binrw_support::ThumbnailHeaderRecord =
            decode_be(&bytes, "thumbnail header").expect("decode");

        assert_eq!(reparsed, record);
    }

    #[test]
    fn color_sampler_roundtrips_color_space_and_depth() {
        let resource = ColorSamplersResource {
            version: 2,
            samplers: vec![crate::api::psd::ColorSampler {
                position: crate::api::psd::ColorSamplerPosition::V2 {
                    horizontal: 100,
                    vertical: 200,
                    depth: 16,
                },
                color_space: 8,
            }],
        };

        let bytes = build_color_samplers_resource_for_test(&resource).unwrap();
        let reparsed = parse_color_samplers_resource_for_test(&bytes);
        assert_eq!(reparsed, resource);
    }

    #[test]
    fn color_sampler_resource_v1_roundtrips_versioned_position() {
        let resource = ColorSamplersResource {
            version: 1,
            samplers: vec![crate::api::psd::ColorSampler {
                position: crate::api::psd::ColorSamplerPosition::V1 {
                    horizontal: 123,
                    vertical: 456,
                },
                color_space: 2,
            }],
        };

        let bytes = build_color_samplers_resource_for_test(&resource).unwrap();
        let reparsed = parse_color_samplers_resource_for_test(&bytes);

        assert_eq!(reparsed, resource);
        assert_eq!(bytes.len(), 18);
    }

    #[test]
    fn color_sampler_resource_v2_roundtrips_depth_and_position() {
        let resource = ColorSamplersResource {
            version: 2,
            samplers: vec![crate::api::psd::ColorSampler {
                position: crate::api::psd::ColorSamplerPosition::V2 {
                    horizontal: -32,
                    vertical: 4096,
                    depth: 16,
                },
                color_space: 8,
            }],
        };

        let bytes = build_color_samplers_resource_for_test(&resource).unwrap();
        let reparsed = parse_color_samplers_resource_for_test(&bytes);

        assert_eq!(reparsed, resource);
        assert_eq!(bytes.len(), 20);
    }

    #[test]
    fn color_sampler_resource_preserves_unsupported_version() {
        let resource = ColorSamplersResource {
            version: 3,
            samplers: vec![crate::api::psd::ColorSampler {
                position: crate::api::psd::ColorSamplerPosition::Unsupported {
                    version: 3,
                    horizontal: 7,
                    vertical: 9,
                    depth: Some(12),
                },
                color_space: 8,
            }],
        };

        let bytes = build_color_samplers_resource_for_test(&resource).unwrap();
        let reparsed = parse_color_samplers_resource_for_test(&bytes);

        assert_eq!(reparsed, resource);
        assert_eq!(bytes.len(), 20);
    }

    #[test]
    fn color_sampler_resource_rejects_mixed_versions() {
        let resource = ColorSamplersResource {
            version: 2,
            samplers: vec![
                crate::api::psd::ColorSampler {
                    position: crate::api::psd::ColorSamplerPosition::V2 {
                        horizontal: 1,
                        vertical: 2,
                        depth: 16,
                    },
                    color_space: 8,
                },
                crate::api::psd::ColorSampler {
                    position: crate::api::psd::ColorSamplerPosition::V1 {
                        horizontal: 3,
                        vertical: 4,
                    },
                    color_space: 0,
                },
            ],
        };

        let err = build_color_samplers_resource_for_test(&resource).unwrap_err();
        assert!(
            err.to_string().contains("single color sampler version"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn color_sampler_resource_rejects_version_shape_mismatch() {
        let resource = ColorSamplersResource {
            version: 1,
            samplers: vec![crate::api::psd::ColorSampler {
                position: crate::api::psd::ColorSamplerPosition::V2 {
                    horizontal: 1,
                    vertical: 2,
                    depth: 16,
                },
                color_space: 8,
            }],
        };

        let err = build_color_samplers_resource_for_test(&resource).unwrap_err();
        assert!(
            err.to_string().contains("does not match resource version"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn color_sampler_resource_rejects_missing_depth_for_unsupported_version_two_or_higher() {
        let resource = ColorSamplersResource {
            version: 9,
            samplers: vec![crate::api::psd::ColorSampler {
                position: crate::api::psd::ColorSamplerPosition::Unsupported {
                    version: 9,
                    horizontal: 1,
                    vertical: 2,
                    depth: None,
                },
                color_space: 8,
            }],
        };

        let err = build_color_samplers_resource_for_test(&resource).unwrap_err();
        assert!(
            err.to_string().contains("requires depth"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn color_sampler_public_model_carries_depth_in_position() {
        let sampler = crate::api::psd::ColorSampler {
            position: crate::api::psd::ColorSamplerPosition::V2 {
                horizontal: 1,
                vertical: 2,
                depth: 16,
            },
            color_space: 8,
        };

        match sampler.position {
            crate::api::psd::ColorSamplerPosition::V2 { depth, .. } => assert_eq!(depth, 16),
            _ => panic!("expected version 2 sampler"),
        }
    }

    #[test]
    fn path_resource_roundtrip() {
        let mut resources = ImageResources::default();
        resources.path_resources.insert(
            2000,
            vec![PathResourceRecord {
                record_type: 1,
                closed: true,
                points: vec![
                    Point { x: 1.0, y: 2.0 },
                    Point { x: 3.0, y: 4.0 },
                    Point { x: 5.0, y: 6.0 },
                    Point { x: 7.0, y: 8.0 },
                ],
            }],
        );

        let mut w = PsdWriter::new(256);
        write_image_resources(&mut w, &resources).unwrap();
        let buf = w.into_buffer();
        let len = buf.len();
        let mut reader = PsdReader::new(std::io::Cursor::new(buf), Default::default());
        let reparsed = read_image_resources(&mut reader, len).unwrap();

        assert_eq!(
            reparsed.path_resources.get(&2000),
            resources.path_resources.get(&2000)
        );
    }

    #[test]
    fn slices_roundtrip() {
        let mut resources = ImageResources::default();
        resources.slices = Some(Slices {
            version: 6,
            bounds: Some(SliceBounds {
                top: 0,
                left: 0,
                bottom: 100,
                right: 100,
            }),
            group_name: Some("group".to_string()),
            slices: vec![Slice {
                id: 1,
                group_id: 2,
                origin: crate::api::types::SliceOrigin::LayerBased,
                associated_layer_id: 3,
                name: "slice".to_string(),
                slice_type: crate::api::types::SliceType::NoImage,
                bounds: SliceBounds {
                    top: 10,
                    left: 20,
                    bottom: 30,
                    right: 40,
                },
                url: "https://example.com".to_string(),
                target: "_blank".to_string(),
                message: "msg".to_string(),
                alt_tag: "alt".to_string(),
                cell_text: "cell".to_string(),
                horizontal_align: crate::api::types::SliceAlignment::RightOrBottom,
                vertical_align: crate::api::types::SliceAlignment::Other(5),
                alpha: 255,
                bg_color: [255, 2, 3, 4],
                cell_is_html: true,
                source_id: None,
                source_type: None,
                descriptor: None,
            }],
            descriptor: None,
        });

        let mut w = PsdWriter::new(512);
        write_image_resources(&mut w, &resources).unwrap();
        let buf = w.into_buffer();
        let len = buf.len();
        let mut reader = PsdReader::new(std::io::Cursor::new(buf), Default::default());
        let reparsed = read_image_resources(&mut reader, len).unwrap();

        assert_eq!(reparsed.slices, resources.slices);
    }

    #[test]
    fn legacy_slice_descriptor_probe_rejects_header_like_trailing_bytes_without_empty_name_prefix()
    {
        let bytes = [
            0, 0, 0, 16, // descriptor version marker
            0, 0, 0,
            0, // invalid for tightened framing; empty descriptor names encode as len=1
            0, 0, 0, 0, // pretend class id len
            b'n', b'u', b'l', b'l', // pretend class id bytes
            0, 0, 0, 0, // pretend item count
        ];

        assert!(!next_bytes_start_legacy_slice_descriptor(
            &bytes,
            0,
            bytes.len()
        ));
    }

    #[test]
    fn icc_profile_roundtrip() {
        let mut res = ImageResources::default();
        res.icc_profile = Some(vec![0x01, 0x02, 0x03, 0x04]);
        let mut w = PsdWriter::new(256);
        write_image_resources(&mut w, &res).unwrap();
        let buf = w.into_buffer();
        let buf_len = buf.len();
        let cursor = std::io::Cursor::new(buf);
        let mut reader = PsdReader::new(cursor, Default::default());
        let read_res = read_image_resources(&mut reader, buf_len).unwrap();
        assert_eq!(read_res.icc_profile, Some(vec![0x01, 0x02, 0x03, 0x04]));
    }

    #[test]
    fn variables_xml_roundtrip() {
        let mut res = ImageResources::default();
        res.variables = Some("<variables/>".to_string());
        let mut w = PsdWriter::new(256);
        write_image_resources(&mut w, &res).unwrap();
        let buf = w.into_buffer();
        let buf_len = buf.len();
        let cursor = std::io::Cursor::new(buf);
        let mut reader = PsdReader::new(cursor, Default::default());
        let read_res = read_image_resources(&mut reader, buf_len).unwrap();
        assert_eq!(read_res.variables.as_deref(), Some("<variables/>"));
    }

    #[test]
    fn accepts_mesa_resource_signature() {
        let mut writer = PsdWriter::new(64);
        writer.write_bytes(b"MeSa").unwrap();
        writer.write_u16(1039).unwrap();
        writer.write_u8(0).unwrap();
        writer.write_u8(0).unwrap();
        writer.write_u32(4).unwrap();
        writer.write_bytes(&[1, 2, 3, 4]).unwrap();
        let buf = writer.into_buffer();
        let len = buf.len();
        let mut reader = PsdReader::new(std::io::Cursor::new(buf), Default::default());
        let resources = read_image_resources(&mut reader, len).unwrap();
        assert_eq!(resources.icc_profile, Some(vec![1, 2, 3, 4]));
    }
}

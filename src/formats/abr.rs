//! Adobe Brush (ABR) file format support
//!
//! Provides reading of Adobe Photoshop brush preset files.
//!
//! Version 6+ files are parsed metadata-only: `desc` sections populate
//! `brushes`, while sampled brush pixels (`samp`), patterns (`patt`) and
//! hierarchies (`phry`) are not decoded. `Abr::incomplete` is set whenever
//! any section or record content was skipped or left unmodeled, so callers
//! never mistake a partial parse for a complete brush collection.

use crate::api::layer::PatternInfo;
use crate::api::psd::ReadOptions;
use crate::io::reader::PsdReader;
use crate::support::descriptor::Descriptor;
use crate::support::error::{PsdError, Result};
use byteorder::{BigEndian, ReadBytesExt};
use std::io::{Cursor, Read};

/// Brush file structure
#[derive(Debug, Clone)]
pub struct Abr {
    pub brushes: Vec<Brush>,
    pub samples: Vec<SampleInfo>,
    pub patterns: Vec<PatternInfo>,
    /// True when the file contained records or sections this parser does not
    /// fully model (legacy record tails, unknown brush types, or v6+ `samp`,
    /// `patt`, `phry` and unknown sections). Such results are explicitly
    /// metadata-only and must not be treated as a complete brush set.
    pub incomplete: bool,
}

/// Sample information
#[derive(Debug, Clone)]
pub struct SampleInfo {
    pub id: String,
    pub bounds: BrushBounds,
    pub alpha: Vec<u8>,
}

/// Brush bounds
#[derive(Debug, Clone, PartialEq)]
pub struct BrushBounds {
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
}

/// Brush dynamics control
#[derive(Debug, Clone, PartialEq)]
pub enum DynamicsControl {
    Off,
    Fade,
    PenPressure,
    PenTilt,
    StylusWheel,
    InitialDirection,
    Direction,
    InitialRotation,
    Rotation,
}

/// Brush dynamics settings
#[derive(Debug, Clone)]
pub struct BrushDynamics {
    pub control: DynamicsControl,
    pub steps: i32,
    pub jitter: f64,
    pub minimum: f64,
}

/// Brush shape types
#[derive(Debug, Clone)]
pub enum BrushShape {
    Computed {
        size: f64,
        angle: f64,
        roundness: f64,
        hardness: f64,
        spacing_on: bool,
        spacing: f64,
        flip_x: bool,
        flip_y: bool,
    },
    Sampled {
        name: String,
        size: f64,
        angle: f64,
        roundness: f64,
        spacing_on: bool,
        spacing: f64,
        flip_x: bool,
        flip_y: bool,
        sampled_data: String,
    },
}

/// Brush definition
#[derive(Debug, Clone)]
pub struct Brush {
    pub name: String,
    pub shape: Option<BrushShape>,
    pub spacing: Option<f64>,
    pub diameter: Option<f64>,
    pub roundness: Option<f64>,
    pub angle: Option<f64>,
    pub hardness: Option<f64>,
    pub size_dynamics: Option<BrushDynamics>,
    pub angle_dynamics: Option<BrushDynamics>,
    pub roundness_dynamics: Option<BrushDynamics>,
}

/// Read an ABR file from a reader
pub fn read_abr<R: Read>(mut reader: R) -> Result<Abr> {
    let mut buffer = Vec::new();
    reader
        .read_to_end(&mut buffer)
        .map_err(|e| PsdError::InvalidAbr(format!("Failed to read ABR: {}", e)))?;

    let mut cursor = Cursor::new(&buffer);
    let version = cursor
        .read_u16::<BigEndian>()
        .map_err(|e| PsdError::InvalidAbr(format!("Failed to read version: {}", e)))?;

    match version {
        1 | 2 => read_abr_v1_v2(&buffer[2..], version),
        6 | 7 | 9 | 10 => read_abr_v6_plus(&buffer),
        _ => Err(PsdError::InvalidAbr(format!(
            "Unsupported ABR version: {}",
            version
        ))),
    }
}

/// Read ABR version 1 or 2.
///
/// `data` is everything after the version word. Every brush record is parsed
/// through a cursor bounded to its declared `size`, so truncated records fail
/// loudly instead of silently consuming bytes from the next record. Unmodeled
/// bytes at the tail of a record (or an unknown brush type) are skipped and
/// surfaced through `Abr::incomplete`.
fn read_abr_v1_v2(data: &[u8], version: u16) -> Result<Abr> {
    let mut cur = Cursor::new(data);

    fn read_u16_c(cur: &mut Cursor<&[u8]>, what: &str) -> Result<u16> {
        cur.read_u16::<BigEndian>()
            .map_err(|_| PsdError::InvalidAbr(format!("Truncated ABR data while reading {}", what)))
    }
    fn read_u32_c(cur: &mut Cursor<&[u8]>, what: &str) -> Result<u32> {
        cur.read_u32::<BigEndian>()
            .map_err(|_| PsdError::InvalidAbr(format!("Truncated ABR data while reading {}", what)))
    }
    fn read_u8_c(cur: &mut Cursor<&[u8]>, what: &str) -> Result<u8> {
        cur.read_u8()
            .map_err(|_| PsdError::InvalidAbr(format!("Truncated ABR data while reading {}", what)))
    }
    fn read_i16_c(cur: &mut Cursor<&[u8]>, what: &str) -> Result<i16> {
        cur.read_i16::<BigEndian>()
            .map_err(|_| PsdError::InvalidAbr(format!("Truncated ABR data while reading {}", what)))
    }

    let count = read_u16_c(&mut cur, "brush count")?;
    let mut brushes = Vec::new();
    let mut samples = Vec::new();
    let mut incomplete = false;

    for _ in 0..count {
        let brush_type = read_u16_c(&mut cur, "brush type")?;
        let size = read_u32_c(&mut cur, "brush record size")? as usize;

        // Bound the whole record body to the declared size.
        let body_start = cur.position() as usize;
        let body_end = body_start
            .checked_add(size)
            .ok_or_else(|| PsdError::InvalidAbr("Brush record size overflow".to_string()))?;
        if body_end > data.len() {
            return Err(PsdError::InvalidAbr(format!(
                "Brush record declares {} bytes but only {} remain (version {})",
                size,
                data.len() - body_start,
                version
            )));
        }
        let mut body = Cursor::new(&data[body_start..body_end]);

        match brush_type {
            1 => {
                // Computed brush
                read_u32_c(&mut body, "computed brush misc")?;
                let spacing = read_u16_c(&mut body, "computed brush spacing")? as f64;
                let diameter = read_u16_c(&mut body, "computed brush diameter")? as f64;
                let roundness = read_u16_c(&mut body, "computed brush roundness")? as f64;
                let angle = read_u16_c(&mut body, "computed brush angle")? as f64;
                let hardness = read_u16_c(&mut body, "computed brush hardness")? as f64;

                brushes.push(Brush {
                    name: format!("Brush {}", brushes.len() + 1),
                    shape: Some(BrushShape::Computed {
                        size: diameter,
                        angle,
                        roundness: roundness / 100.0,
                        hardness: hardness / 100.0,
                        spacing_on: true,
                        spacing: spacing / 100.0,
                        flip_x: false,
                        flip_y: false,
                    }),
                    spacing: Some(spacing / 100.0),
                    diameter: Some(diameter),
                    roundness: Some(roundness / 100.0),
                    angle: Some(angle),
                    hardness: Some(hardness / 100.0),
                    size_dynamics: None,
                    angle_dynamics: None,
                    roundness_dynamics: None,
                });
            }
            2 => {
                // Sampled brush
                let misc = read_u32_c(&mut body, "sampled brush misc")?;
                let spacing = read_u16_c(&mut body, "sampled brush spacing")? as f64;

                // Name presence is signaled by the misc flag. The exact name
                // layout is version-dependent and not independently verified;
                // if the declared record is too short for it we fail loudly.
                let name_length = if misc & 1 != 0 { 256 } else { 0 };
                let mut name = vec![0u8; name_length];
                if name_length > 0 {
                    body.read_exact(&mut name).map_err(|_| {
                        PsdError::InvalidAbr("Truncated sampled brush name".to_string())
                    })?;
                }

                read_u8_c(&mut body, "sampled brush anti-alias")?;
                let y = read_i16_c(&mut body, "sampled brush top")?;
                let x = read_i16_c(&mut body, "sampled brush left")?;
                let h = read_i16_c(&mut body, "sampled brush height")?;
                let w = read_i16_c(&mut body, "sampled brush width")?;
                let depth = read_u16_c(&mut body, "sampled brush depth")?;
                let compression = read_u8_c(&mut body, "sampled brush compression")?;

                // Dimensions and depth drive the sample allocation; negative
                // or zero values (or unsupported depths) must not be cast
                // into a huge allocation.
                if w <= 0 || h <= 0 {
                    return Err(PsdError::InvalidAbr(format!(
                        "Sampled brush has invalid bounds {}x{}",
                        w, h
                    )));
                }
                if !matches!(depth, 1 | 8 | 16 | 32) {
                    return Err(PsdError::InvalidAbr(format!(
                        "Sampled brush has unsupported depth {}",
                        depth
                    )));
                }
                let width = w as usize;
                let height = h as usize;

                let bounds = BrushBounds {
                    x: x as i32,
                    y: y as i32,
                    w: w as i32,
                    h: h as i32,
                };

                // Compression 0 is raw; other legacy encodings are not
                // implemented, so reject them explicitly instead of parsing
                // their bytes as raw samples.
                if compression != 0 {
                    return Err(PsdError::UnsupportedFeature(format!(
                        "Sampled brush compression {} is not supported",
                        compression
                    )));
                }
                let row_bytes = (width * depth as usize + 7) / 8;
                let data_size = height.checked_mul(row_bytes).ok_or_else(|| {
                    PsdError::InvalidAbr("Sampled brush size overflow".to_string())
                })?;
                // Refuse to allocate more than the record actually carries.
                let body_remaining = (body.get_ref().len() - body.position() as usize) as usize;
                if data_size > body_remaining {
                    return Err(PsdError::InvalidAbr(format!(
                        "Sampled brush declares {} sample bytes but record has only {}",
                        data_size, body_remaining
                    )));
                }
                let mut alpha = vec![0u8; data_size];
                body.read_exact(&mut alpha).map_err(|e| {
                    PsdError::InvalidAbr(format!("Failed to read sampled brush data: {}", e))
                })?;

                samples.push(SampleInfo {
                    id: format!("sample_{}", samples.len()),
                    bounds,
                    alpha,
                });

                brushes.push(Brush {
                    name: String::from_utf8_lossy(&name).to_string(),
                    shape: None,
                    spacing: Some(spacing / 100.0),
                    diameter: Some(w as f64),
                    roundness: None,
                    angle: None,
                    hardness: None,
                    size_dynamics: None,
                    angle_dynamics: None,
                    roundness_dynamics: None,
                });
            }
            _ => {
                // Unknown brush type: the whole declared record is skipped,
                // which the flag makes visible to callers.
                incomplete = true;
            }
        }

        // Bytes past the modeled fields inside a record are unmodeled content.
        if body.position() as usize != body.get_ref().len() {
            incomplete = true;
        }

        cur.set_position(body_end as u64);
    }

    Ok(Abr {
        brushes,
        samples,
        patterns: Vec::new(),
        incomplete,
    })
}

/// Read ABR version 6+
fn read_abr_v6_plus(data: &[u8]) -> Result<Abr> {
    let mut brushes = Vec::new();
    let samples = Vec::new();
    let patterns = Vec::new();
    let mut incomplete = false;

    let mut offset = 2; // Skip version

    // Read subversion
    if offset + 2 > data.len() {
        return Err(PsdError::InvalidAbr("Unexpected end of data".to_string()));
    }
    let _subversion = u16::from_be_bytes([data[offset], data[offset + 1]]);
    offset += 2;

    while offset < data.len() {
        if offset + 8 > data.len() {
            return Err(PsdError::InvalidAbr(
                "Truncated ABR section header".to_string(),
            ));
        }

        // Read section signature
        let mut sig = [0u8; 4];
        sig.copy_from_slice(&data[offset..offset + 4]);
        offset += 4;

        if &sig != b"8BIM" {
            return Err(PsdError::InvalidAbr(format!(
                "Unexpected ABR section signature {:?} at offset {}",
                String::from_utf8_lossy(&sig),
                offset
            )));
        }

        // Read section type
        let mut section_type = [0u8; 4];
        section_type.copy_from_slice(&data[offset..offset + 4]);
        offset += 4;

        // Read section size
        if offset + 4 > data.len() {
            return Err(PsdError::InvalidAbr(
                "Truncated ABR section size".to_string(),
            ));
        }
        let size = u32::from_be_bytes([
            data[offset],
            data[offset + 1],
            data[offset + 2],
            data[offset + 3],
        ]);
        offset += 4;

        let section_end = offset
            .checked_add(size as usize)
            .ok_or_else(|| PsdError::InvalidAbr("ABR section size overflow".to_string()))?;
        if section_end > data.len() {
            return Err(PsdError::InvalidAbr(format!(
                "ABR section of {} bytes exceeds remaining input",
                size
            )));
        }

        match &section_type {
            b"samp" => {
                // Sampled brush pixels are not decoded; surface the gap.
                incomplete = true;
                offset = section_end;
            }
            b"desc" => {
                // Descriptor section - contains brush presets
                let section_data = &data[offset..section_end];
                let descriptor = parse_brush_descriptor(section_data).map_err(|e| {
                    PsdError::InvalidAbr(format!("Brush descriptor parse failed: {}", e))
                })?;
                if let Some(brush) = descriptor_to_brush(&descriptor) {
                    brushes.push(brush);
                } else {
                    incomplete = true;
                }
                offset = section_end;
            }
            b"patt" => {
                // Pattern section: not decoded.
                incomplete = true;
                offset = section_end;
            }
            b"phry" => {
                // Hierarchy section: not decoded.
                incomplete = true;
                offset = section_end;
            }
            _ => {
                // Unknown section: skipped, surfaced through the flag.
                incomplete = true;
                offset = section_end;
            }
        }
    }

    Ok(Abr {
        brushes,
        samples,
        patterns,
        incomplete,
    })
}

/// Parse a brush descriptor
fn parse_brush_descriptor(data: &[u8]) -> Result<Descriptor> {
    let cursor = Cursor::new(data);
    let mut psd_reader = PsdReader::new(cursor, ReadOptions::default());
    psd_reader.read_version_and_descriptor()
}

/// Convert descriptor to brush
fn descriptor_to_brush(descriptor: &Descriptor) -> Option<Brush> {
    let name = descriptor
        .items
        .get("Nm  ")
        .and_then(|v| {
            if let crate::support::descriptor::DescriptorValue::Text(s) = v {
                Some(s.clone())
            } else {
                None
            }
        })
        .unwrap_or_else(|| "Unnamed".to_string());

    Some(Brush {
        name,
        shape: None,
        spacing: None,
        diameter: None,
        roundness: None,
        angle: None,
        hardness: None,
        size_dynamics: None,
        angle_dynamics: None,
        roundness_dynamics: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_abr_bounds() {
        let bounds = BrushBounds {
            x: 10,
            y: 20,
            w: 100,
            h: 200,
        };

        assert_eq!(bounds.x, 10);
        assert_eq!(bounds.w, 100);
    }

    #[test]
    fn test_dynamics_control() {
        let dynamics = BrushDynamics {
            control: DynamicsControl::PenPressure,
            steps: 10,
            jitter: 0.5,
            minimum: 0.0,
        };

        assert_eq!(dynamics.steps, 10);
    }

    /// Build a complete v2 legacy file carrying one computed brush whose
    /// declared record size matches its modeled payload exactly.
    fn legacy_computed_file(record_tail: usize) -> Vec<u8> {
        let mut data: Vec<u8> = Vec::new();
        data.extend_from_slice(&2u16.to_be_bytes()); // version
        data.extend_from_slice(&1u16.to_be_bytes()); // brush count
        data.extend_from_slice(&1u16.to_be_bytes()); // type 1: computed
                                                     // body: misc 4 + spacing 2 + diameter 2 + roundness 2 + angle 2 +
                                                     // hardness 2 + tail
        let body_len = 4 + 2 * 5 + record_tail;
        data.extend_from_slice(&(body_len as u32).to_be_bytes());
        data.extend_from_slice(&0u32.to_be_bytes()); // misc
        data.extend_from_slice(&25u16.to_be_bytes()); // spacing
        data.extend_from_slice(&40u16.to_be_bytes()); // diameter
        data.extend_from_slice(&50u16.to_be_bytes()); // roundness 50% -> 0.5
        data.extend_from_slice(&45u16.to_be_bytes()); // angle degrees
        data.extend_from_slice(&100u16.to_be_bytes()); // hardness 100% -> 1.0
        data.extend_from_slice(&vec![0u8; record_tail]);
        data
    }

    #[test]
    fn test_abr_v2_computed_brush_exact_record() {
        let data = legacy_computed_file(0);
        let abr = read_abr(Cursor::new(data)).unwrap();
        assert!(!abr.incomplete);
        assert_eq!(abr.brushes.len(), 1);
        assert_eq!(abr.samples.len(), 0);
        let brush = &abr.brushes[0];
        assert_eq!(brush.diameter, Some(40.0));
        assert_eq!(brush.roundness, Some(0.5));
        assert_eq!(brush.angle, Some(45.0));
        assert_eq!(brush.hardness, Some(1.0));
        assert_eq!(brush.spacing, Some(0.25));
    }

    #[test]
    fn test_abr_record_tail_marks_incomplete() {
        let data = legacy_computed_file(6); // 6 unmodeled trailing bytes
        let abr = read_abr(Cursor::new(data)).unwrap();
        assert!(abr.incomplete);
        assert_eq!(abr.brushes.len(), 1);
    }

    #[test]
    fn test_abr_truncated_record_rejected() {
        // Declared record extends past the end of the file.
        let mut data = legacy_computed_file(0);
        data.truncate(data.len() - 2);
        let err = read_abr(Cursor::new(data)).unwrap_err();
        assert!(
            err.to_string().contains("declares") && err.to_string().contains("remain"),
            "unexpected error: {}",
            err
        );
    }

    #[test]
    fn test_abr_unknown_brush_type_skipped_and_flagged() {
        let mut data: Vec<u8> = Vec::new();
        data.extend_from_slice(&2u16.to_be_bytes()); // version
        data.extend_from_slice(&1u16.to_be_bytes()); // brush count
        data.extend_from_slice(&9u16.to_be_bytes()); // unknown type
        data.extend_from_slice(&4u32.to_be_bytes()); // 4 payload bytes
        data.extend_from_slice(&[1, 2, 3, 4]);
        let abr = read_abr(Cursor::new(data)).unwrap();
        assert!(abr.incomplete);
        assert!(abr.brushes.is_empty());
    }

    #[test]
    fn test_abr_sampled_brush_raw_round_trip() {
        let mut data: Vec<u8> = Vec::new();
        data.extend_from_slice(&2u16.to_be_bytes()); // version
        data.extend_from_slice(&1u16.to_be_bytes()); // brush count
        data.extend_from_slice(&2u16.to_be_bytes()); // type 2: sampled
        let pixel = [0xAAu8, 0xBB, 0xCC, 0xDD, 0xEE, 0xFF, 0x11, 0x22];
        // body: misc 4 + spacing 2 + anti 1 + bounds 8 + depth 2 +
        // compression 1 + pixels
        let body_len = 4 + 2 + 1 + 8 + 2 + 1 + pixel.len();
        data.extend_from_slice(&(body_len as u32).to_be_bytes());
        data.extend_from_slice(&0u32.to_be_bytes()); // misc: no name
        data.extend_from_slice(&25u16.to_be_bytes()); // spacing
        data.push(1); // anti-alias
        data.extend_from_slice(&0i16.to_be_bytes()); // top
        data.extend_from_slice(&(-3i16).to_be_bytes()); // left
        data.extend_from_slice(&2i16.to_be_bytes()); // height
        data.extend_from_slice(&4i16.to_be_bytes()); // width
        data.extend_from_slice(&8u16.to_be_bytes()); // depth
        data.push(0); // compression: raw
        data.extend_from_slice(&pixel);
        let abr = read_abr(Cursor::new(data)).unwrap();
        assert!(!abr.incomplete);
        assert_eq!(abr.samples.len(), 1);
        let sample = &abr.samples[0];
        assert_eq!(sample.bounds.x, -3);
        assert_eq!(sample.bounds.w, 4);
        assert_eq!(sample.bounds.h, 2);
        assert_eq!(sample.alpha, pixel.to_vec());
        assert_eq!(abr.brushes[0].diameter, Some(4.0));
    }

    #[test]
    fn test_abr_sampled_declares_more_than_record_carries() {
        let mut data: Vec<u8> = Vec::new();
        data.extend_from_slice(&2u16.to_be_bytes()); // version
        data.extend_from_slice(&1u16.to_be_bytes()); // brush count
        data.extend_from_slice(&2u16.to_be_bytes()); // type 2: sampled
                                                     // Body covers the full fixed field prefix but carries no pixels,
                                                     // although geometry (2 rows x 4 bytes) demands 8.
        data.extend_from_slice(&18u32.to_be_bytes()); // body length
        data.extend_from_slice(&0u32.to_be_bytes()); // misc
        data.extend_from_slice(&25u16.to_be_bytes()); // spacing
        data.push(1); // anti-alias
        data.extend_from_slice(&0i16.to_be_bytes()); // top
        data.extend_from_slice(&0i16.to_be_bytes()); // left
        data.extend_from_slice(&2i16.to_be_bytes()); // height
        data.extend_from_slice(&4i16.to_be_bytes()); // width
        data.extend_from_slice(&8u16.to_be_bytes()); // depth
        data.push(0); // compression raw -> 8 pixels wanted, 0 left
        let err = read_abr(Cursor::new(data)).unwrap_err();
        assert!(
            err.to_string().contains("declares 8 sample bytes"),
            "unexpected error: {}",
            err
        );
    }

    #[test]
    fn test_abr_v6_sampled_section_flags_incomplete() {
        let mut data: Vec<u8> = Vec::new();
        data.extend_from_slice(&6u16.to_be_bytes()); // version
        data.extend_from_slice(&1u16.to_be_bytes()); // subversion
        data.extend_from_slice(b"8BIM");
        data.extend_from_slice(b"samp");
        data.extend_from_slice(&0u32.to_be_bytes()); // empty payload
        let abr = read_abr(Cursor::new(data)).unwrap();
        assert!(abr.incomplete);
        assert!(abr.brushes.is_empty());
    }

    #[test]
    fn test_abr_v6_truncated_section_header_rejected() {
        let mut data: Vec<u8> = Vec::new();
        data.extend_from_slice(&6u16.to_be_bytes()); // version
        data.extend_from_slice(&1u16.to_be_bytes()); // subversion
        data.extend_from_slice(b"8BIM"); // partial: no type + size follow
        let err = read_abr(Cursor::new(data)).unwrap_err();
        assert!(
            err.to_string().contains("Truncated"),
            "unexpected error: {}",
            err
        );
    }
}

//! Custom Shape (CSH) file format support
//!
//! Provides reading of Adobe Photoshop Custom Shape files.

use crate::api::layer::{BezierKnot, BezierPath};
use crate::api::types::BooleanOperation;
use crate::support::error::{PsdError, Result};
use byteorder::{BigEndian, ReadBytesExt};
use std::io::{Cursor, Read};

/// Custom shape definition
#[derive(Debug, Clone)]
pub struct CustomShape {
    pub name: String,
    pub id: String,
    pub width: u32,
    pub height: u32,
    pub paths: Vec<BezierPath>,
}

/// CSH file structure
#[derive(Debug, Clone)]
pub struct Csh {
    pub shapes: Vec<CustomShape>,
}

fn remaining(cursor: &Cursor<Vec<u8>>) -> usize {
    cursor
        .get_ref()
        .len()
        .saturating_sub(cursor.position() as usize)
}

fn ensure(cursor: &Cursor<Vec<u8>>, needed: usize, what: &str) -> Result<()> {
    if remaining(cursor) < needed {
        return Err(PsdError::InvalidCsh(format!(
            "Truncated CSH data while reading {}",
            what
        )));
    }
    Ok(())
}

fn read_u8_c(cursor: &mut Cursor<Vec<u8>>) -> Result<u8> {
    ensure(cursor, 1, "a byte")?;
    cursor
        .read_u8()
        .map_err(|e| PsdError::InvalidCsh(format!("Failed to read byte: {}", e)))
}

fn read_u16_c(cursor: &mut Cursor<Vec<u8>>) -> Result<u16> {
    ensure(cursor, 2, "a u16")?;
    cursor
        .read_u16::<BigEndian>()
        .map_err(|e| PsdError::InvalidCsh(format!("Failed to read u16: {}", e)))
}

fn read_u32_c(cursor: &mut Cursor<Vec<u8>>) -> Result<u32> {
    ensure(cursor, 4, "a u32")?;
    cursor
        .read_u32::<BigEndian>()
        .map_err(|e| PsdError::InvalidCsh(format!("Failed to read u32: {}", e)))
}

fn read_i32_c(cursor: &mut Cursor<Vec<u8>>) -> Result<i32> {
    ensure(cursor, 4, "an i32")?;
    cursor
        .read_i32::<BigEndian>()
        .map_err(|e| PsdError::InvalidCsh(format!("Failed to read i32: {}", e)))
}

/// Read a CSH file from a reader
pub fn read_csh<R: Read>(mut reader: R) -> Result<Csh> {
    // Read entire file into memory and use a cursor for position tracking
    let mut buffer = Vec::new();
    reader
        .read_to_end(&mut buffer)
        .map_err(|e| PsdError::InvalidCsh(format!("Failed to read CSH: {}", e)))?;
    let file_len = buffer.len();
    let mut cursor = Cursor::new(buffer);

    // Read signature
    let mut sig = [0u8; 4];
    cursor
        .read_exact(&mut sig)
        .map_err(|e| PsdError::InvalidCsh(format!("Failed to read signature: {}", e)))?;

    if &sig != b"cush" {
        return Err(PsdError::InvalidCsh(format!(
            "Invalid signature: expected 'cush', got {:?}",
            String::from_utf8_lossy(&sig)
        )));
    }

    // Read version
    let version = read_u32_c(&mut cursor)?;
    if version != 2 {
        return Err(PsdError::InvalidCsh(format!(
            "Unsupported version: {} (expected 2)",
            version
        )));
    }

    // Read shape count
    let count = read_u32_c(&mut cursor)?;
    crate::support::limits::check_container_count(count as u64, "CSH shape count")
        .map_err(|e| PsdError::InvalidCsh(e.to_string()))?;

    let mut shapes = Vec::with_capacity(count as usize);

    for _ in 0..count {
        // Read Unicode name
        let name = read_unicode_string(&mut cursor)?;

        // Align to 4-byte boundary
        align_to_4bytes(&mut cursor)?;

        // Read shape version
        let shape_version = read_u32_c(&mut cursor)?;
        if shape_version != 1 {
            return Err(PsdError::InvalidCsh(format!(
                "Unsupported shape version: {}",
                shape_version
            )));
        }

        // Read size: the declared payload of this shape. Everything parsed
        // below must stay inside [shape_start, shape_end).
        let size = read_u32_c(&mut cursor)? as usize;
        let shape_start = cursor.position() as usize;
        let shape_end = shape_start
            .checked_add(size)
            .ok_or_else(|| PsdError::InvalidCsh("Shape size overflow".to_string()))?;
        if shape_end > file_len {
            return Err(PsdError::InvalidCsh(format!(
                "Shape payload of {} bytes exceeds remaining file",
                size
            )));
        }

        // Read ID (Pascal string)
        let id = read_pascal_string(&mut cursor, 1)?;

        // Read bounds. Reversed or degenerate bounds must not underflow.
        let y1 = read_u32_c(&mut cursor)?;
        let x1 = read_u32_c(&mut cursor)?;
        let y2 = read_u32_c(&mut cursor)?;
        let x2 = read_u32_c(&mut cursor)?;
        let width = x2.saturating_sub(x1);
        let height = y2.saturating_sub(y1);

        // Read vector mask data, bounded by the shape payload.
        let paths = read_vector_mask_paths(&mut cursor, width, height, shape_end)?;

        // Any trailing (unmodeled) payload belongs to this shape: advance to
        // its declared end so the next shape starts at the right offset.
        cursor.set_position(shape_end as u64);
        if (cursor.position() as usize) < file_len {
            align_to_4bytes(&mut cursor)?;
        }

        shapes.push(CustomShape {
            name,
            id,
            width,
            height,
            paths,
        });
    }

    Ok(Csh { shapes })
}

/// Read a Unicode string (length-prefixed UTF-16 big-endian).
///
/// Only the trailing terminator unit is dropped; interior NULs are preserved.
fn read_unicode_string(cursor: &mut Cursor<Vec<u8>>) -> Result<String> {
    let length = read_u32_c(cursor)?;

    if length == 0 {
        return Ok(String::new());
    }

    crate::support::limits::check_container_count(length as u64, "CSH Unicode length")
        .map_err(|e| PsdError::InvalidCsh(e.to_string()))?;
    let remaining = cursor
        .get_ref()
        .len()
        .saturating_sub(cursor.position() as usize);
    if (length as usize).checked_mul(2).unwrap_or(usize::MAX) > remaining {
        return Err(PsdError::InvalidCsh(
            "CSH Unicode string exceeds remaining input".to_string(),
        ));
    }

    let mut buffer = Vec::with_capacity(length as usize);
    for _ in 0..length {
        buffer.push(read_u16_c(cursor)?);
    }
    if buffer.last() == Some(&0) {
        buffer.pop();
    }

    String::from_utf16(&buffer).map_err(|e| PsdError::InvalidCsh(format!("Invalid UTF-16: {}", e)))
}

/// Read a Pascal string (length byte + characters)
fn read_pascal_string(cursor: &mut Cursor<Vec<u8>>, pad_to: usize) -> Result<String> {
    let length = read_u8_c(cursor)?;

    let mut buffer = vec![0u8; length as usize];
    ensure(cursor, length as usize, "a pascal string")?;
    cursor
        .read_exact(&mut buffer)
        .map_err(|e| PsdError::InvalidCsh(format!("Failed to read pascal string: {}", e)))?;

    // Read padding
    let total = 1 + length as usize;
    let padding = (pad_to - (total % pad_to)) % pad_to;
    for _ in 0..padding {
        read_u8_c(cursor)?;
    }

    String::from_utf8(buffer).map_err(|e| PsdError::InvalidCsh(format!("Invalid UTF-8: {}", e)))
}

/// Align cursor position to 4-byte boundary
fn align_to_4bytes(cursor: &mut Cursor<Vec<u8>>) -> Result<()> {
    let pos = cursor.position() as usize;
    let padding = (4 - (pos % 4)) % 4;
    for _ in 0..padding {
        read_u8_c(cursor)?;
    }
    Ok(())
}

/// Read vector mask paths
fn read_vector_mask_paths(
    cursor: &mut Cursor<Vec<u8>>,
    _width: u32,
    _height: u32,
    end: usize,
) -> Result<Vec<BezierPath>> {
    let mut paths = Vec::new();

    // Each knot is 24 bytes (6 x i32 fixed-point 8.24 coordinates).
    const KNOT_BYTES: usize = 24;

    // Read path count
    let path_count = read_u32_c(cursor)?;
    crate::support::limits::check_container_count(path_count as u64, "CSH path count")
        .map_err(|e| PsdError::InvalidCsh(e.to_string()))?;

    for _ in 0..path_count {
        let pos = cursor.position() as usize;
        if end < pos || end - pos < 4 {
            return Err(PsdError::InvalidCsh(
                "Truncated CSH path record".to_string(),
            ));
        }
        // Read path record type and count
        let record_type = read_u16_c(cursor)?;
        let num_points = read_u16_c(cursor)?;

        // Bound the whole run against the shape payload before allocating.
        let bytes_needed = (num_points as usize)
            .checked_mul(KNOT_BYTES)
            .ok_or_else(|| PsdError::InvalidCsh("Path size overflow".to_string()))?;
        if bytes_needed > end.saturating_sub(cursor.position() as usize) {
            return Err(PsdError::InvalidCsh(format!(
                "Path of {} points exceeds shape payload",
                num_points
            )));
        }

        match record_type {
            0 => {
                // Closed path length record
                let mut knots = Vec::with_capacity(num_points as usize);
                for _ in 0..num_points {
                    knots.push(read_bezier_knot(cursor)?);
                }
                paths.push(BezierPath {
                    open: false,
                    operation: Some(BooleanOperation::Combine),
                    knots,
                    fill_rule: crate::api::types::PsdStringCode::from("nonzero"),
                });
            }
            3 => {
                // Open path length record
                let mut knots = Vec::with_capacity(num_points as usize);
                for _ in 0..num_points {
                    knots.push(read_bezier_knot(cursor)?);
                }
                paths.push(BezierPath {
                    open: true,
                    operation: Some(BooleanOperation::Combine),
                    knots,
                    fill_rule: crate::api::types::PsdStringCode::from("nonzero"),
                });
            }
            _ => {
                // Unknown selector: consume its declared payload so subsequent
                // records stay aligned.
                let mut skip = vec![0u8; bytes_needed];
                cursor
                    .read_exact(&mut skip)
                    .map_err(|e| PsdError::InvalidCsh(format!("Failed to skip path: {}", e)))?;
            }
        }
    }

    Ok(paths)
}

/// Read a Bezier knot (6 x 32-bit 8.24 fixed-point coordinates)
fn read_bezier_knot(cursor: &mut Cursor<Vec<u8>>) -> Result<BezierKnot> {
    let mut points = Vec::with_capacity(6);

    for _ in 0..6 {
        let value = read_i32_c(cursor)?;

        // Convert from fixed point (8.24) to floating point
        let float_value = value as f64 / (1i64 << 24) as f64;
        points.push(float_value);
    }

    Ok(BezierKnot {
        linked: true,
        points,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_read_csh_invalid_signature() {
        let data = b"INVALID";
        let cursor = Cursor::new(data.to_vec());
        let result = read_csh(cursor);
        assert!(result.is_err());
    }

    #[test]
    fn test_read_pascal_string() {
        let data = vec![5, b'H', b'e', b'l', b'l', b'o', 0]; // length=5, "Hello", padding
        let mut cursor = Cursor::new(data);
        let result = read_pascal_string(&mut cursor, 2).unwrap();
        assert_eq!(result, "Hello");
    }

    #[test]
    fn test_read_csh_rejects_declared_size_past_eof() {
        // cush v2, one shape whose declared size overruns the file.
        let mut data: Vec<u8> = Vec::new();
        data.extend_from_slice(b"cush");
        data.extend_from_slice(&2u32.to_be_bytes()); // version
        data.extend_from_slice(&1u32.to_be_bytes()); // shape count
        data.extend_from_slice(&0u32.to_be_bytes()); // empty unicode name
        data.extend_from_slice(&1u32.to_be_bytes()); // shape version
        data.extend_from_slice(&0xFFFF_FF00u32.to_be_bytes()); // declared size
        let err = read_csh(Cursor::new(data)).unwrap_err();
        assert!(
            err.to_string().contains("exceeds remaining file"),
            "unexpected error: {}",
            err
        );
    }

    #[test]
    fn test_read_csh_reversed_bounds_do_not_panic() {
        // One shape with reversed bounds and an empty vector-mask run.
        let mut data: Vec<u8> = Vec::new();
        data.extend_from_slice(b"cush");
        data.extend_from_slice(&2u32.to_be_bytes());
        data.extend_from_slice(&1u32.to_be_bytes());
        data.extend_from_slice(&0u32.to_be_bytes()); // empty name
        data.extend_from_slice(&1u32.to_be_bytes()); // shape version
                                                     // size covers: pascal id (1 byte len 0) + bounds (16) + path count (4)
        data.extend_from_slice(&(1u32 + 16 + 4).to_be_bytes());
        data.push(0); // id: empty pascal
        data.extend_from_slice(&10u32.to_be_bytes()); // y1
        data.extend_from_slice(&20u32.to_be_bytes()); // x1 (after left)
        data.extend_from_slice(&5u32.to_be_bytes()); // y2 < y1
        data.extend_from_slice(&2u32.to_be_bytes()); // x2 < x1
        data.extend_from_slice(&0u32.to_be_bytes()); // zero paths
        let csh = read_csh(Cursor::new(data)).unwrap();
        assert_eq!(csh.shapes.len(), 1);
        assert_eq!(csh.shapes[0].width, 0); // saturating, no underflow
        assert_eq!(csh.shapes[0].height, 0);
    }
}

//! Compression and decompression utilities for PSD files
//!
//! Supports RLE and ZIP compression methods used in PSD files.

use crate::support::error::{PsdError, Result};
use flate2::read::DeflateDecoder;
use flate2::write::DeflateEncoder;
use flate2::Compression as FlateCompression;
use std::io::{Read, Write};

/// Decompress RLE-compressed data
///
/// RLE (Run-Length Encoding) is used for layer image data in PSD files.
/// Each scanline is compressed separately.
pub fn decompress_rle(
    input: &[u8],
    output: &mut [u8],
    _width: usize,
    height: usize,
    byte_counts: &[u32],
) -> Result<()> {
    let mut input_pos: usize = 0;
    let mut output_pos: usize = 0;

    if byte_counts.len() < height {
        return Err(PsdError::Compression(
            "RLE: missing row byte counts".to_string(),
        ));
    }

    for row in 0..height {
        let byte_count = byte_counts[row] as usize;
        let row_end = input_pos
            .checked_add(byte_count)
            .ok_or_else(|| PsdError::Compression("RLE: row size overflow".to_string()))?;
        if row_end > input.len() {
            return Err(PsdError::Compression(
                "RLE: row exceeds input length".to_string(),
            ));
        }

        while input_pos < row_end && output_pos < output.len() {
            let header = input[input_pos];
            input_pos += 1;

            if header == 128 {
                continue;
            } else if header > 128 {
                // Repeat next byte (257 - header) times.
                let count = 257usize - header as usize;
                if input_pos >= input.len() {
                    return Err(PsdError::Compression(
                        "RLE: unexpected end of input".to_string(),
                    ));
                }
                let value = input[input_pos];
                input_pos += 1;

                for _ in 0..count {
                    if output_pos >= output.len() {
                        return Err(PsdError::Compression("RLE: output overflow".to_string()));
                    }
                    output[output_pos] = value;
                    output_pos += 1;
                }
            } else {
                // Copy next (header + 1) bytes
                let count = header as usize + 1;
                for _ in 0..count {
                    if input_pos >= input.len() {
                        return Err(PsdError::Compression(
                            "RLE: unexpected end of input".to_string(),
                        ));
                    }
                    if output_pos >= output.len() {
                        return Err(PsdError::Compression("RLE: output overflow".to_string()));
                    }
                    output[output_pos] = input[input_pos];
                    input_pos += 1;
                    output_pos += 1;
                }
            }
        }

        // Skip any unread row bytes if malformed streams decode early.
        if input_pos < row_end {
            input_pos = row_end;
        }
    }

    Ok(())
}

/// Compress data using RLE
///
/// Returns the compressed data with byte counts for each scanline prepended.
pub fn compress_rle_rows(data: &[u8], width: usize, height: usize) -> Result<(Vec<u32>, Vec<u8>)> {
    let mut rows = Vec::new();
    let mut byte_counts = Vec::with_capacity(height);

    for y in 0..height {
        let row_start = y * width;
        let row_end = row_start + width;
        let row = &data[row_start..row_end];

        let compressed_row = compress_rle_row(row)?;
        byte_counts.push(compressed_row.len() as u32);
        rows.extend_from_slice(&compressed_row);
    }
    Ok((byte_counts, rows))
}

/// Compress data using RLE.
///
/// Returns the compressed data with byte counts for each scanline prepended.
pub fn compress_rle(data: &[u8], width: usize, height: usize, large: bool) -> Result<Vec<u8>> {
    let (byte_counts, rows) = compress_rle_rows(data, width, height)?;
    let mut output = Vec::with_capacity(byte_counts.len() * if large { 4 } else { 2 } + rows.len());
    for count in byte_counts {
        if large {
            output.extend_from_slice(&count.to_be_bytes());
        } else {
            output.extend_from_slice(&(count as u16).to_be_bytes());
        }
    }
    output.extend_from_slice(&rows);

    Ok(output)
}

/// Compress a single row using RLE
fn compress_rle_row(row: &[u8]) -> Result<Vec<u8>> {
    let mut result = Vec::new();
    let mut i = 0;

    while i < row.len() {
        // Try to encode a run first.
        let mut run_len = 1usize;
        while i + run_len < row.len() && row[i + run_len] == row[i] && run_len < 128 {
            run_len += 1;
        }
        if run_len >= 3 {
            result.push((1i16 - run_len as i16) as u8);
            result.push(row[i]);
            i += run_len;
            continue;
        }

        // Otherwise encode a literal block up to 128 bytes or until next run.
        let lit_start = i;
        let mut lit_len = 0usize;
        while i < row.len() && lit_len < 128 {
            if i + 2 < row.len() && row[i] == row[i + 1] && row[i] == row[i + 2] {
                break;
            }
            i += 1;
            lit_len += 1;
        }
        if lit_len == 0 {
            // Fallback safety; should never happen due run path above.
            lit_len = 1;
            i += 1;
        }
        result.push((lit_len - 1) as u8);
        result.extend_from_slice(&row[lit_start..lit_start + lit_len]);
    }

    Ok(result)
}

/// Decompress ZIP-compressed data (raw DEFLATE, no Zlib headers)
pub fn decompress_zip(input: &[u8], output_size: usize) -> Result<Vec<u8>> {
    let mut decoder = DeflateDecoder::new(input);
    let mut output = Vec::with_capacity(output_size);

    decoder
        .read_to_end(&mut output)
        .map_err(|e| PsdError::Compression(format!("ZIP decompression failed: {}", e)))?;

    Ok(output)
}

/// Compress data using ZIP (raw DEFLATE, no Zlib headers)
pub fn compress_zip(input: &[u8]) -> Result<Vec<u8>> {
    let mut encoder = DeflateEncoder::new(Vec::new(), FlateCompression::default());

    encoder
        .write_all(input)
        .map_err(|e| PsdError::Compression(format!("ZIP compression failed: {}", e)))?;

    encoder
        .finish()
        .map_err(|e| PsdError::Compression(format!("ZIP compression finish failed: {}", e)))
}

/// Decompress ZIP-with-prediction for a single PSD channel.
///
/// `depth` is bits per sample: 8, 16, or 32. Data arrives one channel at a time.
pub fn decompress_zip_with_prediction(
    input: &[u8],
    width: usize,
    height: usize,
    depth: u16,
) -> Result<Vec<u8>> {
    let bytes_per_sample = match depth {
        8 => 1usize,
        16 => 2,
        32 => 4,
        _ => {
            return Err(PsdError::Compression(format!(
                "Unsupported depth: {}",
                depth
            )))
        }
    };
    let expected = width * height * bytes_per_sample;
    let mut data = decompress_zip(input, expected)?;

    match depth {
        8 => {
            for row in 0..height {
                let start = row * width;
                for x in 1..width {
                    data[start + x] = data[start + x].wrapping_add(data[start + x - 1]);
                }
            }
        }
        16 => {
            // Byte-level delta (same as 8-bit path) applied to raw byte stream.
            // Each row is width * 2 bytes. Left-to-right prefix sum.
            let row_bytes = width * 2;
            for row in 0..height {
                let start = row * row_bytes;
                for i in start + 1..start + row_bytes {
                    data[i] = data[i].wrapping_add(data[i - 1]);
                }
            }
        }
        32 => {
            let row_bytes = width * 4;
            let mut reordered = vec![0u8; row_bytes];
            for row in 0..height {
                let row_off = row * row_bytes;
                reordered.copy_from_slice(&data[row_off..row_off + row_bytes]);
                // Undo 8-bit delta per plane
                for plane in 0..4usize {
                    let base = plane * width;
                    for i in 1..width {
                        reordered[base + i] =
                            reordered[base + i].wrapping_add(reordered[base + i - 1]);
                    }
                }
                // De-interleave: planes → pixels
                for pixel in 0..width {
                    let dst = row_off + pixel * 4;
                    data[dst] = reordered[pixel];
                    data[dst + 1] = reordered[width + pixel];
                    data[dst + 2] = reordered[width * 2 + pixel];
                    data[dst + 3] = reordered[width * 3 + pixel];
                }
            }
        }
        _ => unreachable!(),
    }
    Ok(data)
}

/// Compress a single PSD channel with ZIP-with-prediction.
///
/// `depth` is bits per sample: 8, 16, or 32.
pub fn compress_zip_with_prediction(
    input: &[u8],
    width: usize,
    height: usize,
    depth: u16,
) -> Result<Vec<u8>> {
    match depth {
        8 | 16 | 32 => {}
        _ => {
            return Err(PsdError::Compression(format!(
                "Unsupported depth: {}",
                depth
            )))
        }
    }
    let mut predicted = input.to_vec();

    match depth {
        8 => {
            // Right-to-left byte delta per row
            for row in 0..height {
                let start = row * width;
                for x in (1..width).rev() {
                    predicted[start + x] =
                        predicted[start + x].wrapping_sub(predicted[start + x - 1]);
                }
            }
        }
        16 => {
            // Byte-level delta (same as 8-bit path) applied to raw byte stream.
            // Each row is width * 2 bytes. Right-to-left byte delta.
            let row_bytes = width * 2;
            for row in 0..height {
                let start = row * row_bytes;
                for i in (start + 1..start + row_bytes).rev() {
                    predicted[i] = predicted[i].wrapping_sub(predicted[i - 1]);
                }
            }
        }
        32 => {
            let row_bytes = width * 4;
            let mut reordered = vec![0u8; row_bytes];
            for row in 0..height {
                let row_off = row * row_bytes;
                // Pixels → byte-planes
                for pixel in 0..width {
                    let src = row_off + pixel * 4;
                    reordered[pixel] = predicted[src];
                    reordered[width + pixel] = predicted[src + 1];
                    reordered[width * 2 + pixel] = predicted[src + 2];
                    reordered[width * 3 + pixel] = predicted[src + 3];
                }
                // Right-to-left 8-bit delta per plane
                for plane in 0..4usize {
                    let base = plane * width;
                    for i in (1..width).rev() {
                        reordered[base + i] =
                            reordered[base + i].wrapping_sub(reordered[base + i - 1]);
                    }
                }
                predicted[row_off..row_off + row_bytes].copy_from_slice(&reordered);
            }
        }
        _ => unreachable!(),
    }
    compress_zip(&predicted)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_compress_decompress_rle() {
        let data = vec![1, 1, 1, 2, 3, 4, 5, 5];
        let compressed = compress_rle(&data, 8, 1, false).unwrap();

        // Skip byte counts (2 bytes for 1 row)
        let byte_count = u16::from_be_bytes([compressed[0], compressed[1]]) as usize;
        let compressed_data = &compressed[2..];

        let mut output = vec![0u8; 8];
        decompress_rle(compressed_data, &mut output, 8, 1, &[byte_count as u32]).unwrap();

        assert_eq!(output, data);
    }

    #[test]
    fn compress_rle_large_emits_four_byte_counts() {
        let data = vec![7u8; 16];
        let psb = compress_rle(&data, 16, 1, true).unwrap();
        let count = u32::from_be_bytes(psb[0..4].try_into().unwrap()) as usize;
        let mut out = vec![0u8; 16];
        decompress_rle(&psb[4..], &mut out, 16, 1, &[count as u32]).unwrap();
        assert_eq!(out, data);

        let psd = compress_rle(&data, 16, 1, false).unwrap();
        assert_eq!(psd.len(), psb.len() - 2);
    }

    #[test]
    fn test_compress_decompress_zip() {
        let data = b"Hello, World! This is a test of ZIP compression.";
        let compressed = compress_zip(data).unwrap();
        let decompressed = decompress_zip(&compressed, data.len()).unwrap();

        assert_eq!(&decompressed[..], &data[..]);
    }

    #[test]
    fn zip_roundtrip_is_raw_deflate_not_zlib() {
        let data: Vec<u8> = (0..256u16).map(|v| v as u8).collect();
        let compressed = compress_zip(&data).unwrap();
        let recovered = decompress_zip(&compressed, data.len()).unwrap();
        assert_eq!(recovered, data);
        // Zlib best-compression header is 0x78 0x9C; raw deflate must NOT start with that.
        assert!(
            !(compressed.len() >= 2 && compressed[0] == 0x78 && compressed[1] == 0x9C),
            "output looks like a Zlib stream; should be raw deflate"
        );
    }

    #[test]
    fn zip_prediction_8bit_roundtrip() {
        let data: Vec<u8> = vec![10, 20, 30, 40, 50, 60, 70, 80];
        let compressed = compress_zip_with_prediction(&data, 4, 2, 8).unwrap();
        let recovered = decompress_zip_with_prediction(&compressed, 4, 2, 8).unwrap();
        assert_eq!(recovered, data);
    }

    #[test]
    fn zip_prediction_16bit_roundtrip() {
        // 3 big-endian u16 values: 256, 512, 768
        let data: Vec<u8> = vec![0x01, 0x00, 0x02, 0x00, 0x03, 0x00];
        let compressed = compress_zip_with_prediction(&data, 3, 1, 16).unwrap();
        let recovered = decompress_zip_with_prediction(&compressed, 3, 1, 16).unwrap();
        assert_eq!(recovered, data);
    }

    #[test]
    fn zip_prediction_32bit_roundtrip() {
        // 2 IEEE-754 floats: 1.0f32 and 2.0f32 (big-endian bytes)
        let data: Vec<u8> = vec![0x3f, 0x80, 0x00, 0x00, 0x40, 0x00, 0x00, 0x00];
        let compressed = compress_zip_with_prediction(&data, 2, 1, 32).unwrap();
        let recovered = decompress_zip_with_prediction(&compressed, 2, 1, 32).unwrap();
        assert_eq!(recovered, data);
    }
}

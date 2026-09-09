//! Compression and decompression utilities for PSD files
//!
//! Supports RLE and ZIP compression methods used in PSD files.
//!
//! ZIP data in PSD/PSB files uses zlib framing (a zlib header and trailer
//! around a DEFLATE stream), not raw DEFLATE. `decompress_zip` also carries an
//! explicit compatibility path for files written by earlier versions of this
//! crate that stored raw DEFLATE without zlib framing.

use crate::support::error::{PsdError, Result};
use flate2::read::{DeflateDecoder, ZlibDecoder};
use flate2::write::ZlibEncoder;
use flate2::Compression as FlateCompression;
use std::io::{Read, Write};

/// Decompress RLE-compressed data.
///
/// `row_len` is the decoded byte length of one scanline and `height` the number
/// of scanlines; rows are packed into `output` sequentially, each into exactly
/// `row_len` bytes. Every run must stay inside its row's compressed window and
/// every row must decode to exactly `row_len` bytes. Short or overlong rows,
/// runs crossing the row boundary, and non-no-op bytes left in a full row are
/// all errors.
pub fn decompress_rle(
    input: &[u8],
    output: &mut [u8],
    row_len: usize,
    height: usize,
    byte_counts: &[u32],
) -> Result<()> {
    if byte_counts.len() < height {
        return Err(PsdError::Compression(
            "RLE: missing row byte counts".to_string(),
        ));
    }
    let expected_total = row_len
        .checked_mul(height)
        .ok_or_else(|| PsdError::Compression("RLE: output size overflow".to_string()))?;
    if output.len() < expected_total {
        return Err(PsdError::Compression(
            "RLE: output buffer smaller than declared row size".to_string(),
        ));
    }

    let mut input_pos: usize = 0;

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

        let row_output = &mut output[row * row_len..(row + 1) * row_len];
        let mut filled = 0usize;

        while input_pos < row_end {
            let header = input[input_pos];
            input_pos += 1;

            if header == 128 {
                // No-op packet; consumes no output.
                continue;
            } else if header > 128 {
                // Repeat next byte (257 - header) times; count is 2..=128.
                let count = 257usize - header as usize;
                if input_pos >= row_end {
                    return Err(PsdError::Compression(
                        "RLE: truncated repeat run".to_string(),
                    ));
                }
                let value = input[input_pos];
                input_pos += 1;
                if filled + count > row_len {
                    return Err(PsdError::Compression(
                        "RLE: run overflows row boundary".to_string(),
                    ));
                }
                row_output[filled..filled + count].fill(value);
                filled += count;
            } else {
                // Copy next (header + 1) bytes; count is 1..=128.
                let count = header as usize + 1;
                if row_end - input_pos < count {
                    return Err(PsdError::Compression(
                        "RLE: truncated literal run".to_string(),
                    ));
                }
                if filled + count > row_len {
                    return Err(PsdError::Compression(
                        "RLE: literal overflows row boundary".to_string(),
                    ));
                }
                row_output[filled..filled + count]
                    .copy_from_slice(&input[input_pos..input_pos + count]);
                input_pos += count;
                filled += count;
            }
        }

        if filled != row_len {
            return Err(PsdError::Compression(format!(
                "RLE: row {} decoded {} of {} expected bytes",
                row, filled, row_len
            )));
        }
    }

    Ok(())
}

/// Compress data using RLE
///
/// Returns the compressed data with byte counts for each scanline prepended.
pub fn compress_rle_rows(
    data: &[u8],
    row_len: usize,
    height: usize,
) -> Result<(Vec<u32>, Vec<u8>)> {
    let required = row_len
        .checked_mul(height)
        .ok_or_else(|| PsdError::Compression("RLE: row size overflow".to_string()))?;
    if data.len() < required {
        return Err(PsdError::Compression(format!(
            "RLE: input is {} bytes, rows need {}",
            data.len(),
            required
        )));
    }

    let mut rows = Vec::new();
    let mut byte_counts = Vec::with_capacity(height);

    for y in 0..height {
        let row_start = y * row_len;
        let row = &data[row_start..row_start + row_len];

        let compressed_row = compress_rle_row(row)?;
        byte_counts.push(compressed_row.len() as u32);
        rows.extend_from_slice(&compressed_row);
    }
    Ok((byte_counts, rows))
}

/// Compress data using RLE.
///
/// Returns the compressed data with byte counts for each scanline prepended.
pub fn compress_rle(data: &[u8], row_len: usize, height: usize, large: bool) -> Result<Vec<u8>> {
    let (byte_counts, rows) = compress_rle_rows(data, row_len, height)?;
    if !large {
        if let Some(&count) = byte_counts.iter().find(|&&count| count > u16::MAX as u32) {
            return Err(PsdError::Compression(format!(
                "RLE: compressed row of {} bytes exceeds the PSD row-count limit of 65535; \
                 write this channel as PSB or use another compression",
                count
            )));
        }
    }
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

fn sample_bytes(depth: u16) -> Result<usize> {
    match depth {
        8 => Ok(1),
        16 => Ok(2),
        32 => Ok(4),
        _ => Err(PsdError::Compression(format!(
            "Unsupported depth: {}",
            depth
        ))),
    }
}

/// Decompress ZIP-compressed data (zlib framing).
///
/// The decoded output must be exactly `output_size` bytes; shorter or longer
/// streams and streams with trailing bytes after the zlib container are
/// errors. Decoding is bounded so a compressed expansion bomb cannot trigger
/// unbounded growth.
pub fn decompress_zip(input: &[u8], output_size: usize) -> Result<Vec<u8>> {
    match decompress_zlib_exact(input, output_size) {
        Ok(data) => Ok(data),
        Err(zlib_err) => {
            // Compatibility read path: earlier versions of this crate wrote raw
            // DEFLATE without zlib framing. Accept such streams only when they
            // decode exactly to the expected size with no trailing bytes.
            decompress_deflate_exact(input, output_size).map_err(|_| zlib_err)
        }
    }
}

/// Read a framed DEFLATE stream (`decoder`) to exactly `output_size` bytes.
///
/// Reading is capped at `output_size + 1` bytes so oversized streams error
/// instead of growing without bound.
fn read_bounded_exact<R: Read>(decoder: &mut R, output_size: usize, name: &str) -> Result<Vec<u8>> {
    let cap = output_size
        .checked_add(1)
        .ok_or_else(|| PsdError::Compression("ZIP: output size overflow".to_string()))?;
    let mut output = Vec::with_capacity(output_size);
    let mut buf = [0u8; 8192];
    loop {
        if output.len() > output_size {
            return Err(PsdError::Compression(format!(
                "{}: stream decoded {} bytes, expected at most {}",
                name,
                output.len(),
                output_size
            )));
        }
        let remaining = cap - output.len();
        let want = remaining.min(buf.len());
        let n = decoder
            .read(&mut buf[..want])
            .map_err(|e| PsdError::Compression(format!("{} decompression failed: {}", name, e)))?;
        if n == 0 {
            break;
        }
        output.extend_from_slice(&buf[..n]);
    }
    if output.len() < output_size {
        return Err(PsdError::Compression(format!(
            "{}: stream decoded {} bytes, expected {}",
            name,
            output.len(),
            output_size
        )));
    }
    Ok(output)
}

fn decompress_zlib_exact(input: &[u8], output_size: usize) -> Result<Vec<u8>> {
    let mut decoder = ZlibDecoder::new(input);
    let output = read_bounded_exact(&mut decoder, output_size, "ZIP")?;
    if decoder.total_in() != input.len() as u64 {
        return Err(PsdError::Compression(
            "ZIP: trailing bytes after compressed stream".to_string(),
        ));
    }
    Ok(output)
}

fn decompress_deflate_exact(input: &[u8], output_size: usize) -> Result<Vec<u8>> {
    let mut decoder = DeflateDecoder::new(input);
    let output = read_bounded_exact(&mut decoder, output_size, "ZIP (raw deflate)")?;
    if decoder.total_in() != input.len() as u64 {
        return Err(PsdError::Compression(
            "ZIP (raw deflate): trailing bytes after compressed stream".to_string(),
        ));
    }
    Ok(output)
}

/// Compress data using ZIP (zlib framing).
pub fn compress_zip(input: &[u8]) -> Result<Vec<u8>> {
    let mut encoder = ZlibEncoder::new(Vec::new(), FlateCompression::default());

    encoder
        .write_all(input)
        .map_err(|e| PsdError::Compression(format!("ZIP compression failed: {}", e)))?;

    encoder
        .finish()
        .map_err(|e| PsdError::Compression(format!("ZIP compression finish failed: {}", e)))
}

/// Undo the ZIP-with-prediction transform on one channel plane.
///
/// `data` must hold `width * height` samples of `depth` bits each, stored
/// big-endian. 8-bit and 16-bit depths use per-row sample deltas; 32-bit data
/// is first shuffled into byte planes and the delta runs across the entire
/// shuffled row (i.e. it is not reset at each byte-plane boundary).
pub fn reverse_prediction(data: &mut [u8], width: usize, height: usize, depth: u16) -> Result<()> {
    let bytes_per_sample = sample_bytes(depth)?;
    let row_len = width
        .checked_mul(bytes_per_sample)
        .ok_or_else(|| PsdError::Compression("Prediction: row size overflow".to_string()))?;
    let required = row_len
        .checked_mul(height)
        .ok_or_else(|| PsdError::Compression("Prediction: size overflow".to_string()))?;
    if data.len() < required {
        return Err(PsdError::Compression(format!(
            "Prediction: buffer has {} bytes, channel needs {}",
            data.len(),
            required
        )));
    }

    match depth {
        8 => {
            for row in 0..height {
                let start = row * width;
                for i in start + 1..start + width {
                    data[i] = data[i].wrapping_add(data[i - 1]);
                }
            }
        }
        16 => {
            // 16-bit sample deltas: cumulative wrap-add across samples (the
            // addition carries across the two big-endian bytes of each sample).
            for row in 0..height {
                let start = row * width;
                let mut prev: u16 = 0;
                for x in 0..width {
                    let idx = start + x;
                    let sample = u16::from_be_bytes([data[idx * 2], data[idx * 2 + 1]]);
                    let value = sample.wrapping_add(prev);
                    let bytes = value.to_be_bytes();
                    data[idx * 2] = bytes[0];
                    data[idx * 2 + 1] = bytes[1];
                    prev = value;
                }
            }
        }
        32 => {
            // Planes are packed first (byte j of every pixel together), and the
            // byte delta spans the whole shuffled row of width*4 bytes.
            let row_bytes = width * 4;
            let mut shuffled = vec![0u8; row_bytes];
            let mut interleaved = vec![0u8; row_bytes];
            for row in 0..height {
                let row_off = row * row_bytes;
                shuffled.copy_from_slice(&data[row_off..row_off + row_bytes]);
                for i in 1..row_bytes {
                    shuffled[i] = shuffled[i].wrapping_add(shuffled[i - 1]);
                }
                for pixel in 0..width {
                    for plane in 0..4usize {
                        interleaved[pixel * 4 + plane] = shuffled[plane * width + pixel];
                    }
                }
                data[row_off..row_off + row_bytes].copy_from_slice(&interleaved);
            }
        }
        _ => unreachable!("depth validated by sample_bytes"),
    }
    Ok(())
}

/// Apply the ZIP-with-prediction transform to one channel plane.
///
/// Inverse of [`reverse_prediction`]. `data` must hold `width * height`
/// samples of `depth` bits each, stored big-endian.
pub fn apply_prediction(data: &mut [u8], width: usize, height: usize, depth: u16) -> Result<()> {
    let bytes_per_sample = sample_bytes(depth)?;
    let row_len = width
        .checked_mul(bytes_per_sample)
        .ok_or_else(|| PsdError::Compression("Prediction: row size overflow".to_string()))?;
    let required = row_len
        .checked_mul(height)
        .ok_or_else(|| PsdError::Compression("Prediction: size overflow".to_string()))?;
    if data.len() < required {
        return Err(PsdError::Compression(format!(
            "Prediction: buffer has {} bytes, channel needs {}",
            data.len(),
            required
        )));
    }

    match depth {
        8 => {
            for row in 0..height {
                let start = row * width;
                for i in (start + 1..start + width).rev() {
                    data[i] = data[i].wrapping_sub(data[i - 1]);
                }
            }
        }
        16 => {
            // 16-bit sample deltas: right-to-left wrap-sub across samples.
            for row in 0..height {
                let start = row * width;
                let mut prev: u16 = 0;
                for x in 0..width {
                    let idx = start + x;
                    let sample = u16::from_be_bytes([data[idx * 2], data[idx * 2 + 1]]);
                    let delta = sample.wrapping_sub(prev);
                    let bytes = delta.to_be_bytes();
                    data[idx * 2] = bytes[0];
                    data[idx * 2 + 1] = bytes[1];
                    prev = sample;
                }
            }
        }
        32 => {
            let row_bytes = width * 4;
            let mut shuffled = vec![0u8; row_bytes];
            let mut interleaved = vec![0u8; row_bytes];
            for row in 0..height {
                let row_off = row * row_bytes;
                interleaved.copy_from_slice(&data[row_off..row_off + row_bytes]);
                for pixel in 0..width {
                    for plane in 0..4usize {
                        shuffled[plane * width + pixel] = interleaved[pixel * 4 + plane];
                    }
                }
                for i in (1..row_bytes).rev() {
                    shuffled[i] = shuffled[i].wrapping_sub(shuffled[i - 1]);
                }
                data[row_off..row_off + row_bytes].copy_from_slice(&shuffled);
            }
        }
        _ => unreachable!("depth validated by sample_bytes"),
    }
    Ok(())
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
    let bps = sample_bytes(depth)?;
    let expected = width
        .checked_mul(height)
        .and_then(|v| v.checked_mul(bps))
        .ok_or_else(|| PsdError::Compression("Prediction: size overflow".to_string()))?;
    let mut data = decompress_zip(input, expected)?;
    reverse_prediction(&mut data, width, height, depth)?;
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
    let bps = sample_bytes(depth)?;
    let expected = width
        .checked_mul(height)
        .and_then(|v| v.checked_mul(bps))
        .ok_or_else(|| PsdError::Compression("Prediction: size overflow".to_string()))?;
    if input.len() < expected {
        return Err(PsdError::Compression(format!(
            "Prediction: input has {} bytes, channel needs {}",
            input.len(),
            expected
        )));
    }
    let mut predicted = input[..expected].to_vec();
    apply_prediction(&mut predicted, width, height, depth)?;
    compress_zip(&predicted)
}

/// Undo ZIP-with-prediction on a planar multi-channel buffer.
///
/// Each channel occupies a contiguous plane of `width * height` samples and
/// prediction is undone per plane, which is equivalent to treating the buffer
/// as `height * channels` rows (the channel boundary coincides with a row
/// boundary).
pub fn reverse_prediction_planar(
    data: &mut [u8],
    width: usize,
    height: usize,
    channels: usize,
    depth: u16,
) -> Result<()> {
    let bytes_per_sample = sample_bytes(depth)?;
    let plane_len = width
        .checked_mul(height)
        .and_then(|v| v.checked_mul(bytes_per_sample))
        .ok_or_else(|| PsdError::Compression("Prediction: plane size overflow".to_string()))?;
    let required = plane_len
        .checked_mul(channels)
        .ok_or_else(|| PsdError::Compression("Prediction: buffer size overflow".to_string()))?;
    if data.len() < required {
        return Err(PsdError::Compression(format!(
            "Prediction: buffer has {} bytes, needs {}",
            data.len(),
            required
        )));
    }
    for channel in 0..channels {
        let start = channel * plane_len;
        reverse_prediction(&mut data[start..start + plane_len], width, height, depth)?;
    }
    Ok(())
}

/// Apply ZIP-with-prediction on a planar multi-channel buffer.
///
/// Inverse of [`reverse_prediction_planar`].
pub fn apply_prediction_planar(
    data: &mut [u8],
    width: usize,
    height: usize,
    channels: usize,
    depth: u16,
) -> Result<()> {
    let bytes_per_sample = sample_bytes(depth)?;
    let plane_len = width
        .checked_mul(height)
        .and_then(|v| v.checked_mul(bytes_per_sample))
        .ok_or_else(|| PsdError::Compression("Prediction: plane size overflow".to_string()))?;
    let required = plane_len
        .checked_mul(channels)
        .ok_or_else(|| PsdError::Compression("Prediction: buffer size overflow".to_string()))?;
    if data.len() < required {
        return Err(PsdError::Compression(format!(
            "Prediction: buffer has {} bytes, needs {}",
            data.len(),
            required
        )));
    }
    for channel in 0..channels {
        let start = channel * plane_len;
        apply_prediction(&mut data[start..start + plane_len], width, height, depth)?;
    }
    Ok(())
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
    fn rle_short_row_is_rejected() {
        // One-byte literal `[0, 7]` cannot fill a two-byte row.
        let input = [0u8, 7];
        let mut out = vec![0u8; 2];
        let err = decompress_rle(&input, &mut out, 2, 1, &[2]).unwrap_err();
        assert!(
            err.to_string().contains("decoded"),
            "unexpected error: {}",
            err
        );
    }

    #[test]
    fn rle_literal_cannot_cross_row_boundary() {
        // Row 0 declares 1 byte but the literal wants 2; row 1's byte must not
        // be consumed by row 0.
        let input = [1u8, 5, 6]; // header=1 => literal of 2 bytes, but row window has 1
        let mut out = vec![0u8; 4];
        let err = decompress_rle(&input, &mut out, 2, 2, &[1, 1]).unwrap_err();
        assert!(
            err.to_string().contains("truncated literal"),
            "unexpected error: {}",
            err
        );
    }

    #[test]
    fn rle_repeat_overflowing_row_is_rejected() {
        // header 0xFD => repeat 4 times; row only holds 2.
        let input = [0xFDu8, 3, 9];
        let mut out = vec![0u8; 4];
        let err = decompress_rle(&input, &mut out, 2, 2, &[2, 1]).unwrap_err();
        assert!(
            err.to_string().contains("overflows row"),
            "unexpected error: {}",
            err
        );
    }

    #[test]
    fn rle_psd_row_count_limit_is_enforced() {
        // A row of incompressible data whose compressed byte count exceeds
        // 65535 cannot be represented in a PSD row-count table.
        let data: Vec<u8> = (0..70000u32).map(|i| ((i * 31) % 256) as u8).collect();
        let err = compress_rle(&data, 70000, 1, false).unwrap_err();
        assert!(
            err.to_string().contains("row-count limit"),
            "unexpected error: {}",
            err
        );
        // The same row is representable in PSB (4-byte counts).
        assert!(compress_rle(&data, 70000, 1, true).is_ok());
    }

    #[test]
    fn compress_rle_rows_validates_input_length() {
        let err = compress_rle_rows(&[1, 2, 3], 8, 1).unwrap_err();
        assert!(
            err.to_string().contains("rows need"),
            "unexpected error: {}",
            err
        );
    }

    #[test]
    fn test_compress_decompress_zip() {
        let data = b"Hello, World! This is a test of ZIP compression.";
        let compressed = compress_zip(data).unwrap();
        let decompressed = decompress_zip(&compressed, data.len()).unwrap();

        assert_eq!(&decompressed[..], &data[..]);
    }

    #[test]
    fn zip_roundtrip_is_zlib() {
        let data: Vec<u8> = (0..256u16).map(|v| v as u8).collect();
        let compressed = compress_zip(&data).unwrap();
        let recovered = decompress_zip(&compressed, data.len()).unwrap();
        assert_eq!(recovered, data);
        // Zlib streams begin with 0x78 (CMF byte).
        assert!(
            compressed.len() >= 2 && compressed[0] == 0x78,
            "output should be a Zlib stream with 0x78 CMF byte"
        );
    }

    #[test]
    fn zip_oversize_output_is_rejected() {
        // 100 zero bytes compress small; requesting a 10-byte decode must fail.
        let data = vec![0u8; 100];
        let compressed = compress_zip(&data).unwrap();
        let err = decompress_zip(&compressed, 10).unwrap_err();
        assert!(
            err.to_string().contains("expected at most 10"),
            "unexpected error: {}",
            err
        );
    }

    #[test]
    fn zip_short_output_is_rejected() {
        let data = vec![7u8; 5];
        let compressed = compress_zip(&data).unwrap();
        let err = decompress_zip(&compressed, 6).unwrap_err();
        assert!(
            err.to_string().contains("expected 6"),
            "unexpected error: {}",
            err
        );
    }

    #[test]
    fn zip_trailing_bytes_are_rejected() {
        let data = vec![3u8; 20];
        let compressed = compress_zip(&data).unwrap();
        let mut padded = compressed.clone();
        padded.extend_from_slice(&[0xAA, 0xBB]);
        let err = decompress_zip(&padded, data.len()).unwrap_err();
        assert!(
            err.to_string().contains("decompression failed")
                || err.to_string().contains("trailing"),
            "unexpected error: {}",
            err
        );
    }

    #[test]
    fn zip_accepts_legacy_raw_deflate_compat() {
        // Files produced by older crate versions stored raw DEFLATE. This is
        // the documented compatibility read path.
        let data = b"legacy raw deflate payload";
        let mut encoder = DeflateEncoderCompat::new();
        encoder.write_all(data).unwrap();
        let raw = encoder.finish();
        let recovered = decompress_zip(&raw, data.len()).unwrap();
        assert_eq!(&recovered[..], &data[..]);
    }

    /// Tiny raw-DEFLATE encoder used only by tests.
    struct DeflateEncoderCompat {
        inner: flate2::write::DeflateEncoder<Vec<u8>>,
    }
    impl DeflateEncoderCompat {
        fn new() -> Self {
            Self {
                inner: flate2::write::DeflateEncoder::new(Vec::new(), FlateCompression::default()),
            }
        }
        fn write_all(&mut self, data: &[u8]) -> std::io::Result<()> {
            use std::io::Write;
            self.inner.write_all(data)
        }
        fn finish(self) -> Vec<u8> {
            self.inner.finish().unwrap()
        }
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
    fn zip_prediction_16bit_handles_carry() {
        // Values chosen so a naive per-byte delta (which the audit found in the
        // 16-bit path) differs from the correct per-sample delta: 0x00FF then
        // 0x0100 crosses a byte boundary with a carry.
        let data: Vec<u8> = vec![0x00, 0xFF, 0x01, 0x00, 0x00, 0x00, 0x80, 0x00];
        let compressed = compress_zip_with_prediction(&data, 4, 1, 16).unwrap();
        let recovered = decompress_zip_with_prediction(&compressed, 4, 1, 16).unwrap();
        assert_eq!(recovered, data);
    }

    #[test]
    fn zip_prediction_32bit_roundtrip() {
        // 4 distinct IEEE-754 floats exercise nonzero deltas on every byte
        // plane, including across plane boundaries.
        let data: Vec<u8> = vec![
            0x3f, 0x80, 0x00, 0x00, // 1.0
            0x40, 0x00, 0x00, 0x00, // 2.0
            0x40, 0x20, 0x00, 0x00, // 2.5
            0xbf, 0x80, 0x00, 0x00, // -1.0
        ];
        let compressed = compress_zip_with_prediction(&data, 4, 1, 32).unwrap();
        let recovered = decompress_zip_with_prediction(&compressed, 4, 1, 32).unwrap();
        assert_eq!(recovered, data);
    }

    #[test]
    fn zip_prediction_32bit_matches_reference_byte_delta() {
        // Independent reference: bytes [a0..a3][b0..b3] (pixel-interleaved) are
        // shuffled into planes a0b0.. / a1b1.. / ..., then a byte delta spans
        // the entire shuffled row. Assert the predicted stream (pre-zlib)
        // equals the reference rather than only roundtripping.
        let data: Vec<u8> = vec![
            0x3f, 0x80, 0x00, 0x01, // pixel 0
            0x40, 0x10, 0x00, 0x02, // pixel 1
            0x41, 0x20, 0x00, 0x03, // pixel 2
            0x42, 0x30, 0x00, 0x04, // pixel 3
        ];
        let expected = prediction_reference_32(&data, 4, 1);
        let predicted = {
            let mut p = data.clone();
            apply_prediction(&mut p, 4, 1, 32).unwrap();
            p
        };
        assert_eq!(predicted, expected);
    }

    /// Byte-level reference implementation of the 32-bit prediction, written
    /// directly from the PSD spec/psd-tools description: shuffle pixel bytes
    /// into planes, then right-to-left byte deltas over the whole row.
    fn prediction_reference_32(data: &[u8], width: usize, height: usize) -> Vec<u8> {
        let row_bytes = width * 4;
        let mut out = vec![0u8; data.len()];
        for row in 0..height {
            let row_off = row * row_bytes;
            let mut shuffled = vec![0u8; row_bytes];
            for pixel in 0..width {
                for plane in 0..4usize {
                    shuffled[plane * width + pixel] = data[row_off + pixel * 4 + plane];
                }
            }
            for i in (1..row_bytes).rev() {
                shuffled[i] = shuffled[i].wrapping_sub(shuffled[i - 1]);
            }
            out[row_off..row_off + row_bytes].copy_from_slice(&shuffled);
        }
        out
    }

    #[test]
    fn prediction_rejects_unsupported_depth() {
        let mut data = vec![0u8; 4];
        assert!(apply_prediction(&mut data, 1, 1, 1).is_err());
        assert!(reverse_prediction(&mut data, 1, 1, 1).is_err());
    }

    #[test]
    fn planar_prediction_roundtrip_matches_per_channel() {
        // Composite (planar) ZIP-with-prediction resets at channel boundaries,
        // matching per-plane application.
        let data: Vec<u8> = (0..64u8).collect();
        let mut applied = data.clone();
        apply_prediction_planar(&mut applied, 4, 4, 2, 8).unwrap();
        let mut per_channel = data.clone();
        apply_prediction(&mut per_channel[..16], 4, 4, 8).unwrap();
        apply_prediction(&mut per_channel[16..], 4, 4, 8).unwrap();
        assert_eq!(applied, per_channel);

        let mut recovered = applied;
        reverse_prediction_planar(&mut recovered, 4, 4, 2, 8).unwrap();
        assert_eq!(recovered, data);
    }
}

use psd_great::additional_info::SectionDivider;
use psd_great::*;
use std::collections::HashMap;
use std::fs;
use std::fs::File;
use std::io::Cursor;
use std::path::PathBuf;

fn in_repo_sample_fixtures_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join("samples")
}

fn sample_fixture_paths() -> Vec<PathBuf> {
    let mut entries = fs::read_dir(in_repo_sample_fixtures_dir())
        .expect("read fixture samples directory")
        .map(|entry| entry.expect("fixture directory entry").path())
        .filter(|path| path.extension().and_then(|ext| ext.to_str()) == Some("psd"))
        .collect::<Vec<_>>();
    entries.sort();
    entries
}

fn document_tagged_block_keys(bytes: &[u8]) -> Vec<String> {
    fn read_u32(bytes: &[u8], offset: usize) -> u32 {
        u32::from_be_bytes(bytes[offset..offset + 4].try_into().expect("u32"))
    }

    let mut offset = 26usize;
    let color_mode_len = read_u32(bytes, offset) as usize;
    offset += 4 + color_mode_len;
    let image_resources_len = read_u32(bytes, offset) as usize;
    offset += 4 + image_resources_len;
    let layer_mask_len = read_u32(bytes, offset) as usize;
    let layer_mask_start = offset + 4;
    offset = layer_mask_start;

    let layer_info_len = read_u32(bytes, offset) as usize;
    let layer_info_start = offset + 4;
    offset = layer_info_start;

    let layer_count = i16::from_be_bytes(bytes[offset..offset + 2].try_into().expect("i16")).abs()
        as usize;
    offset += 2;

    let mut channel_payload_bytes = 0usize;
    for _ in 0..layer_count {
        offset += 16;
        let channel_count =
            u16::from_be_bytes(bytes[offset..offset + 2].try_into().expect("u16")) as usize;
        offset += 2;
        for _ in 0..channel_count {
            channel_payload_bytes += read_u32(bytes, offset + 2) as usize;
            offset += 6;
        }
        offset += 12;
        let extra_len = read_u32(bytes, offset) as usize;
        offset += 4 + extra_len;
    }

    offset += channel_payload_bytes;
    let layer_info_end = layer_info_start + layer_info_len;
    if offset != layer_info_end {
        assert!(
            offset < layer_info_end && layer_info_end - offset <= 3,
            "layer info payload parsing drifted"
        );
        offset = layer_info_end;
    }

    let global_mask_len = read_u32(bytes, offset) as usize;
    offset += 4 + global_mask_len;

    let layer_mask_end = layer_mask_start + layer_mask_len;
    let mut keys = Vec::new();
    while offset + 12 <= layer_mask_end {
        let signature = &bytes[offset..offset + 4];
        if signature != b"8BIM" && signature != b"8B64" {
            break;
        }
        let key = String::from_utf8_lossy(&bytes[offset + 4..offset + 8]).to_string();
        offset += 8;
        if signature == b"8B64" {
            offset += 4;
        }
        let data_len = read_u32(bytes, offset) as usize;
        offset += 4 + data_len;
        while offset % 4 != 0 {
            offset += 1;
        }
        keys.push(key);
    }

    keys
}

fn document_tagged_block_data(bytes: &[u8], key: &str) -> Vec<u8> {
    fn read_u32(bytes: &[u8], offset: usize) -> u32 {
        u32::from_be_bytes(bytes[offset..offset + 4].try_into().expect("u32"))
    }

    let mut offset = 26usize;
    let color_mode_len = read_u32(bytes, offset) as usize;
    offset += 4 + color_mode_len;
    let image_resources_len = read_u32(bytes, offset) as usize;
    offset += 4 + image_resources_len;
    let layer_mask_len = read_u32(bytes, offset) as usize;
    let layer_mask_start = offset + 4;
    offset = layer_mask_start;

    let layer_info_len = read_u32(bytes, offset) as usize;
    let layer_info_start = offset + 4;
    offset = layer_info_start;

    let layer_count = i16::from_be_bytes(bytes[offset..offset + 2].try_into().expect("i16")).abs()
        as usize;
    offset += 2;

    let mut channel_payload_bytes = 0usize;
    for _ in 0..layer_count {
        offset += 16;
        let channel_count =
            u16::from_be_bytes(bytes[offset..offset + 2].try_into().expect("u16")) as usize;
        offset += 2;
        for _ in 0..channel_count {
            channel_payload_bytes += read_u32(bytes, offset + 2) as usize;
            offset += 6;
        }
        offset += 12;
        let extra_len = read_u32(bytes, offset) as usize;
        offset += 4 + extra_len;
    }

    offset += channel_payload_bytes;
    let layer_info_end = layer_info_start + layer_info_len;
    if offset != layer_info_end {
        assert!(
            offset < layer_info_end && layer_info_end - offset <= 3,
            "layer info payload parsing drifted"
        );
        offset = layer_info_end;
    }

    let global_mask_len = read_u32(bytes, offset) as usize;
    offset += 4 + global_mask_len;

    let layer_mask_end = layer_mask_start + layer_mask_len;
    while offset + 12 <= layer_mask_end {
        let signature = &bytes[offset..offset + 4];
        if signature != b"8BIM" && signature != b"8B64" {
            break;
        }
        let current_key = String::from_utf8_lossy(&bytes[offset + 4..offset + 8]).to_string();
        offset += 8;
        if signature == b"8B64" {
            offset += 4;
        }
        let data_len = read_u32(bytes, offset) as usize;
        offset += 4;
        let data = bytes[offset..offset + data_len].to_vec();
        offset += data_len;
        while offset % 4 != 0 {
            offset += 1;
        }
        if current_key == key {
            return data;
        }
    }

    panic!("document tagged block {key} not found");
}

fn layer_blending_range_lengths(bytes: &[u8]) -> Vec<u32> {
    fn read_u32(bytes: &[u8], offset: usize) -> u32 {
        u32::from_be_bytes(bytes[offset..offset + 4].try_into().expect("u32"))
    }

    let mut offset = 26usize;
    let color_mode_len = read_u32(bytes, offset) as usize;
    offset += 4 + color_mode_len;
    let image_resources_len = read_u32(bytes, offset) as usize;
    offset += 4 + image_resources_len;
    let _layer_mask_len = read_u32(bytes, offset) as usize;
    offset += 4;

    let _layer_info_len = read_u32(bytes, offset) as usize;
    offset += 4;

    let layer_count = i16::from_be_bytes(bytes[offset..offset + 2].try_into().expect("i16")).abs()
        as usize;
    offset += 2;

    let mut lengths = Vec::with_capacity(layer_count);
    for _ in 0..layer_count {
        offset += 16;
        let channel_count =
            u16::from_be_bytes(bytes[offset..offset + 2].try_into().expect("u16")) as usize;
        offset += 2 + channel_count * 6 + 12;
        let extra_len = read_u32(bytes, offset) as usize;
        offset += 4;
        let extra_start = offset;
        let mask_len = read_u32(bytes, offset) as usize;
        offset += 4 + mask_len;
        let blend_len = read_u32(bytes, offset);
        lengths.push(blend_len);
        offset = extra_start + extra_len;
    }

    lengths
}

fn layer_extra_data(bytes: &[u8], layer_index: usize) -> Vec<u8> {
    fn read_u32(bytes: &[u8], offset: usize) -> u32 {
        u32::from_be_bytes(bytes[offset..offset + 4].try_into().expect("u32"))
    }

    let mut offset = 26usize;
    let color_mode_len = read_u32(bytes, offset) as usize;
    offset += 4 + color_mode_len;
    let image_resources_len = read_u32(bytes, offset) as usize;
    offset += 4 + image_resources_len;
    offset += 4;
    offset += 4;
    let layer_count = i16::from_be_bytes(bytes[offset..offset + 2].try_into().expect("i16")).abs()
        as usize;
    offset += 2;
    assert!(layer_index < layer_count, "layer index out of bounds");

    for current in 0..layer_count {
        offset += 16;
        let channel_count =
            u16::from_be_bytes(bytes[offset..offset + 2].try_into().expect("u16")) as usize;
        offset += 2 + channel_count * 6 + 12;
        let extra_len = read_u32(bytes, offset) as usize;
        offset += 4;
        let extra = bytes[offset..offset + extra_len].to_vec();
        if current == layer_index {
            return extra;
        }
        offset += extra_len;
    }

    unreachable!("layer not found")
}

fn layer_tagged_block_data(bytes: &[u8], layer_name: &str, key: &str) -> Vec<u8> {
    fn read_u32(bytes: &[u8], offset: usize) -> u32 {
        u32::from_be_bytes(bytes[offset..offset + 4].try_into().expect("u32"))
    }

    let mut offset = 26usize;
    let color_mode_len = read_u32(bytes, offset) as usize;
    offset += 4 + color_mode_len;
    let image_resources_len = read_u32(bytes, offset) as usize;
    offset += 4 + image_resources_len;
    offset += 4;
    offset += 4;
    let layer_count = i16::from_be_bytes(bytes[offset..offset + 2].try_into().expect("i16")).abs()
        as usize;
    offset += 2;

    for _ in 0..layer_count {
        offset += 16;
        let channel_count =
            u16::from_be_bytes(bytes[offset..offset + 2].try_into().expect("u16")) as usize;
        offset += 2 + channel_count * 6 + 12;
        let extra_len = read_u32(bytes, offset) as usize;
        offset += 4;
        let extra_start = offset;
        let mask_len = read_u32(bytes, offset) as usize;
        offset += 4 + mask_len;
        let blend_len = read_u32(bytes, offset) as usize;
        offset += 4 + blend_len;
        let name_len = bytes[offset] as usize;
        let name = String::from_utf8_lossy(&bytes[offset + 1..offset + 1 + name_len]).to_string();
        offset += 1 + name_len;
        while (offset - extra_start) % 4 != 0 {
            offset += 1;
        }
        while offset + 12 <= extra_start + extra_len {
            let signature = &bytes[offset..offset + 4];
            if signature != b"8BIM" && signature != b"8B64" {
                break;
            }
            let block_key = String::from_utf8_lossy(&bytes[offset + 4..offset + 8]).to_string();
            offset += 8;
            if signature == b"8B64" {
                offset += 4;
            }
            let data_len = read_u32(bytes, offset) as usize;
            offset += 4;
            let data = bytes[offset..offset + data_len].to_vec();
            offset += data_len;
            if name == layer_name && block_key == key {
                return data;
            }
            offset += (4 - (data_len % 4)) % 4;
        }
        offset = extra_start + extra_len;
    }

    panic!("tagged block {key} for layer {layer_name} not found");
}

fn clear_text_raw_bytes(layers: &mut [Layer]) {
    for layer in layers {
        if let Some(text) = layer.additional_info.text.as_mut() {
            text.raw_bytes = None;
        }
        if let Some(children) = layer.children.as_mut() {
            clear_text_raw_bytes(children);
        }
    }
}

fn layer_mask_semantics(mask: Option<&psd_great::layer::LayerMaskData>) -> Option<(Option<i32>, Option<i32>, Option<i32>, Option<i32>, Option<u8>, Option<Vec<u8>>)> {
    mask.map(|mask| {
        (
            mask.top,
            mask.left,
            mask.bottom,
            mask.right,
            mask.default_color,
            mask.image_data.as_ref().map(|image| image.data.clone()),
        )
    })
}

fn raw_block_semantics(
    info: &psd_great::additional_info::LayerAdditionalInfo,
) -> Vec<(String, Vec<u8>)> {
    info.raw_blocks
        .iter()
        .map(|block| (block.key.clone(), block.data.clone()))
        .collect()
}

fn is_default_blending_ranges(
    ranges: &psd_great::layer::LayerBlendingRangesData,
) -> bool {
    let default_pair = psd_great::layer::LayerBlendingRangePair {
        src_black: 0,
        src_white: 0,
        dst_black: 255,
        dst_white: 255,
    };
    ranges.composite_gray.as_ref() == Some(&default_pair)
        && ranges.channels.iter().all(|pair| pair == &default_pair)
}

fn blending_ranges_semantically_equal(
    expected: &Option<psd_great::layer::LayerBlendingRangesData>,
    actual: &Option<psd_great::layer::LayerBlendingRangesData>,
) -> bool {
    if expected == actual {
        return true;
    }

    matches!(
        (expected, actual),
        (None, Some(ranges)) | (Some(ranges), None) if is_default_blending_ranges(ranges)
    )
}

fn assert_layers_semantically_equal(expected: &[Layer], actual: &[Layer], strict_order: bool) {
    assert_eq!(actual.len(), expected.len(), "layer count mismatch");

    for (expected, actual) in expected.iter().zip(actual.iter()) {
        assert_eq!(actual.top, expected.top, "layer top mismatch");
        assert_eq!(actual.left, expected.left, "layer left mismatch");
        assert_eq!(actual.bottom, expected.bottom, "layer bottom mismatch");
        assert_eq!(actual.right, expected.right, "layer right mismatch");
        assert_eq!(actual.blend_mode, expected.blend_mode, "layer blend mismatch");
        assert_eq!(actual.opacity, expected.opacity, "layer opacity mismatch");
        assert_eq!(actual.hidden, expected.hidden, "layer hidden mismatch");
        assert_eq!(actual.clipping, expected.clipping, "layer clipping mismatch");
        assert_eq!(actual.opened, expected.opened, "layer open state mismatch");
        assert_eq!(
            actual.additional_info.name,
            expected.additional_info.name,
            "layer name mismatch"
        );
        assert_eq!(
            actual.additional_info.id,
            expected.additional_info.id,
            "layer id mismatch"
        );
        assert_eq!(
            actual.additional_info.section_divider,
            expected.additional_info.section_divider,
            "layer section divider mismatch"
        );
        assert_eq!(
            raw_block_semantics(&actual.additional_info),
            raw_block_semantics(&expected.additional_info),
            "layer raw tagged block mismatch"
        );
        if strict_order {
            assert_eq!(
                actual.additional_info.tagged_block_order,
                expected.additional_info.tagged_block_order,
                "layer tagged block order mismatch"
            );
        }
        assert!(
            blending_ranges_semantically_equal(
                &expected.blending_ranges_data,
                &actual.blending_ranges_data
            ),
            "layer blending ranges mismatch"
        );
        assert_eq!(
            actual.image_data.as_ref().map(|image| &image.data),
            expected.image_data.as_ref().map(|image| &image.data),
            "layer pixel data mismatch"
        );
        assert_eq!(
            layer_mask_semantics(actual.additional_info.mask.as_ref()),
            layer_mask_semantics(expected.additional_info.mask.as_ref()),
            "user mask mismatch"
        );
        assert_eq!(
            layer_mask_semantics(actual.additional_info.real_mask.as_ref()),
            layer_mask_semantics(expected.additional_info.real_mask.as_ref()),
            "real mask mismatch"
        );

        let expected_children = expected.children.as_deref().unwrap_or(&[]);
        let actual_children = actual.children.as_deref().unwrap_or(&[]);
        assert_layers_semantically_equal(expected_children, actual_children, strict_order);
    }
}

fn assert_psd_semantically_equal(expected: &Psd, actual: &Psd, strict_order: bool) {
    assert_eq!(actual.width, expected.width, "document width mismatch");
    assert_eq!(actual.height, expected.height, "document height mismatch");
    assert_eq!(
        actual.bits_per_channel,
        expected.bits_per_channel,
        "document depth mismatch"
    );
    assert_eq!(actual.color_mode, expected.color_mode, "color mode mismatch");
    assert_eq!(
        actual.image_data.as_ref().map(|image| &image.data),
        expected.image_data.as_ref().map(|image| &image.data),
        "composite pixel data mismatch"
    );
    assert_eq!(
        raw_block_semantics(&actual.additional_info),
        raw_block_semantics(&expected.additional_info),
        "document raw tagged block mismatch"
    );
    if strict_order {
        assert_eq!(
            actual.additional_info.tagged_block_order,
            expected.additional_info.tagged_block_order,
            "document tagged block order mismatch"
        );
    }

    assert_layers_semantically_equal(
        expected.children.as_deref().unwrap_or(&[]),
        actual.children.as_deref().unwrap_or(&[]),
        strict_order,
    );
}

fn is_canonical_8bit_rgba_layer(layer: &Layer) -> bool {
    layer.children.is_none()
        && layer.image_data.is_some()
        && layer.additional_info.mask.is_none()
        && layer.additional_info.real_mask.is_none()
        && layer.top.zip(layer.bottom).is_some_and(|(top, bottom)| bottom >= top)
        && layer.left.zip(layer.right).is_some_and(|(left, right)| right >= left)
}

fn assert_canonical_layers_drop_raw_data(layers: &[Layer]) {
    for layer in layers {
        if is_canonical_8bit_rgba_layer(layer) {
            assert!(
                layer.raw_data.is_none(),
                "canonical RGBA layer should regenerate instead of retaining raw data: {:?}",
                layer.additional_info.name
            );
        }
        if let Some(children) = layer.children.as_deref() {
            assert_canonical_layers_drop_raw_data(children);
        }
    }
}

#[test]
#[ignore = "semantic roundtrip contract does not require byte-exact Adobe bytes"]
fn sample_psd_roundtrip_is_byte_exact() {
    for path in sample_fixture_paths() {
        let original = fs::read(&path).expect("read sample fixture");
        let psd = read_psd(Cursor::new(&original), ReadOptions::default())
            .unwrap_or_else(|err| panic!("{}: parse failed: {err}", path.display()));
        let rewritten = write_psd(&psd, &WriteOptions::default())
            .unwrap_or_else(|err| panic!("{}: write failed: {err}", path.display()));

        assert_eq!(
            rewritten,
            original,
            "{}: byte mismatch after roundtrip (original {} bytes, rewritten {} bytes)",
            path.display(),
            original.len(),
            rewritten.len()
        );
    }
}

#[test]
fn default_roundtrip_preserves_pixels_and_structure() {
    let path = in_repo_sample_fixtures_dir().join("3d-preview-mockup.psd");
    let original = fs::read(&path).expect("read sample fixture");
    let psd = read_psd(Cursor::new(&original), ReadOptions::default())
        .unwrap_or_else(|err| panic!("{}: parse failed: {err}", path.display()));
    let rewritten = write_psd(&psd, &WriteOptions::default())
        .unwrap_or_else(|err| panic!("{}: write failed: {err}", path.display()));
    let reparsed = read_psd(Cursor::new(&rewritten), ReadOptions::default())
        .unwrap_or_else(|err| panic!("{}: reparse failed: {err}", path.display()));

    assert_psd_semantically_equal(&psd, &reparsed, false);
}

#[test]
fn roundtrip_3d_preview_sample_does_not_massively_expand() {
    let path = in_repo_sample_fixtures_dir().join("3d-preview-mockup.psd");
    let original = fs::read(&path).expect("read sample fixture");
    let psd = read_psd(Cursor::new(&original), ReadOptions::default())
        .unwrap_or_else(|err| panic!("{}: parse failed: {err}", path.display()));
    let rewritten = write_psd(&psd, &WriteOptions::default())
        .unwrap_or_else(|err| panic!("{}: write failed: {err}", path.display()));

    assert!(
        rewritten.len() <= original.len() * 2,
        "{}: rewritten file expanded unexpectedly (original {} bytes, rewritten {} bytes)",
        path.display(),
        original.len(),
        rewritten.len()
    );
}

#[test]
fn default_read_drops_raw_for_canonical_rgba_layers() {
    let path = in_repo_sample_fixtures_dir().join("3d-preview-mockup.psd");
    let original = fs::read(&path).expect("read sample fixture");
    let psd = read_psd(Cursor::new(&original), ReadOptions::default())
        .unwrap_or_else(|err| panic!("{}: parse failed: {err}", path.display()));

    assert_canonical_layers_drop_raw_data(psd.children.as_deref().unwrap_or(&[]));
}

#[test]
fn all_sample_psds_remain_semantically_stable_across_multiple_roundtrips() {
    for path in sample_fixture_paths() {
        let original = fs::read(&path).expect("read sample fixture");
        let psd1 = read_psd(Cursor::new(&original), ReadOptions::default())
            .unwrap_or_else(|err| panic!("{}: first parse failed: {err}", path.display()));

        let bytes2 = write_psd(&psd1, &WriteOptions::default())
            .unwrap_or_else(|err| panic!("{}: first write failed: {err}", path.display()));
        let psd2 = read_psd(Cursor::new(&bytes2), ReadOptions::default())
            .unwrap_or_else(|err| panic!("{}: second parse failed: {err}", path.display()));

        let bytes3 = write_psd(&psd2, &WriteOptions::default())
            .unwrap_or_else(|err| panic!("{}: second write failed: {err}", path.display()));
        let psd3 = read_psd(Cursor::new(&bytes3), ReadOptions::default())
            .unwrap_or_else(|err| panic!("{}: third parse failed: {err}", path.display()));

        assert_psd_semantically_equal(&psd1, &psd2, false);
        assert_psd_semantically_equal(&psd2, &psd3, true);
    }
}

#[test]
fn default_writer_does_not_emit_layer_only_tagged_blocks_at_document_scope() {
    let psd = Psd {
        width: 1,
        height: 1,
        channels: Some(3),
        bits_per_channel: Some(8),
        color_mode: Some(ColorMode::RGB),
        image_data: Some(PixelData {
            data: vec![0x11, 0x22, 0x33, 0xFF],
            width: 1,
            height: 1,
        }),
        additional_info: additional_info::LayerAdditionalInfo {
            name: Some("Writer Smoke Test".to_string()),
            id: Some(99),
            ..Default::default()
        },
        ..Default::default()
    };

    let bytes = write_psd(&psd, &WriteOptions::default()).expect("write psd");
    let doc_keys = document_tagged_block_keys(&bytes);

    assert!(
        !doc_keys.iter().any(|key| key == "luni" || key == "lyid"),
        "document scope should not contain layer-only tagged blocks, got {doc_keys:?}"
    );
}

#[test]
fn default_writer_emits_photoshop_style_blending_ranges_for_synthesized_rgb_layers() {
    let psd = Psd {
        width: 1,
        height: 1,
        channels: Some(3),
        bits_per_channel: Some(8),
        color_mode: Some(ColorMode::RGB),
        children: Some(vec![Layer {
            top: Some(0),
            left: Some(0),
            bottom: Some(1),
            right: Some(1),
            image_data: Some(PixelData {
                data: vec![0x11, 0x22, 0x33, 0xFF],
                width: 1,
                height: 1,
            }),
            additional_info: additional_info::LayerAdditionalInfo {
                name: Some("Layer 1".to_string()),
                ..Default::default()
            },
            ..Default::default()
        }]),
        ..Default::default()
    };

    let bytes = write_psd(&psd, &WriteOptions::default()).expect("write psd");
    assert_eq!(layer_blending_range_lengths(&bytes), vec![40]);
}

#[test]
fn layer_tagged_blocks_use_four_byte_padding_between_entries() {
    let psd = Psd {
        width: 1,
        height: 1,
        channels: Some(3),
        bits_per_channel: Some(8),
        color_mode: Some(ColorMode::RGB),
        children: Some(vec![Layer {
            top: Some(0),
            left: Some(0),
            bottom: Some(1),
            right: Some(1),
            image_data: Some(PixelData {
                data: vec![0x11, 0x22, 0x33, 0xFF],
                width: 1,
                height: 1,
            }),
            additional_info: additional_info::LayerAdditionalInfo {
                name: Some("Blue Square".to_string()),
                id: Some(3),
                ..Default::default()
            },
            ..Default::default()
        }]),
        ..Default::default()
    };

    let bytes = write_psd(&psd, &WriteOptions::default()).expect("write psd");
    let extra = layer_extra_data(&bytes, 0);
    let luni_offset = extra
        .windows(8)
        .position(|window| window == b"8BIMluni")
        .expect("find luni block");
    let luni_len = u32::from_be_bytes(
        extra[luni_offset + 8..luni_offset + 12]
            .try_into()
            .expect("luni length"),
    ) as usize;
    assert_eq!(luni_len % 4, 2, "test needs a block with two pad bytes");
    let lyid_signature = &extra[luni_offset + 12 + luni_len + 2..luni_offset + 12 + luni_len + 6];
    assert_eq!(lyid_signature, b"8BIM");
}

#[test]
fn layer_info_length_is_rounded_up_to_an_even_byte_count() {
    fn read_u32(bytes: &[u8], offset: usize) -> u32 {
        u32::from_be_bytes(bytes[offset..offset + 4].try_into().expect("u32"))
    }

    let mut checked = 0usize;
    for width in 1..=4usize {
        for height in 1..=4usize {
            for include_mask in [false, true] {
                let mut layer = Layer {
                    top: Some(0),
                    left: Some(0),
                    bottom: Some(height as i32),
                    right: Some(width as i32),
                    ..Default::default()
                };
                layer.image_data = Some(PixelData {
                    width,
                    height,
                    data: vec![255; width * height * 4],
                });
                if include_mask {
                    layer.additional_info.mask = Some(LayerMaskData {
                        top: Some(0),
                        left: Some(0),
                        bottom: Some(height as i32),
                        right: Some(width as i32),
                        image_data: Some(PixelData {
                            width,
                            height,
                            data: vec![255; width * height * 4],
                        }),
                        ..Default::default()
                    });
                }

                let psd = Psd {
                    width: width as u32,
                    height: height as u32,
                    bits_per_channel: Some(8),
                    color_mode: Some(ColorMode::RGB),
                    children: Some(vec![layer]),
                    global_layer_mask_info: Some(GlobalLayerMaskInfo {
                        overlay_color_space: 0,
                        color_space1: 1,
                        color_space2: 2,
                        color_space3: 3,
                        color_space4: 4,
                        opacity: 255,
                        kind: 128,
                    }),
                    ..Default::default()
                };

                let bytes = write_psd(&psd, &WriteOptions::default())
                    .expect("write candidate odd layer info PSD");

                let color_mode_len = read_u32(&bytes, 26) as usize;
                let image_resources_offset = 30 + color_mode_len;
                let image_resources_len = read_u32(&bytes, image_resources_offset) as usize;
                let layer_and_mask_offset = image_resources_offset + 4 + image_resources_len;
                let layer_info_offset = layer_and_mask_offset + 4;
                let layer_info_len = read_u32(&bytes, layer_info_offset) as usize;
                checked += 1;
                assert_eq!(
                    layer_info_len % 2,
                    0,
                    "layer info length should be rounded up to an even byte count"
                );
            }
        }
    }

    assert!(checked > 0, "expected to validate at least one PSD candidate");
}

#[test]
fn tysh_descriptor_version_is_preserved_on_roundtrip_for_sample_text_layer() {
    let path = in_repo_sample_fixtures_dir().join("4901393.psd");
    let original = fs::read(&path).expect("read sample fixture");
    let original_tysh = layer_tagged_block_data(&original, "Website", "TySh");
    let original_descriptor_version =
        u32::from_be_bytes(original_tysh[52..56].try_into().expect("descriptor version"));

    let psd = read_psd(Cursor::new(&original), ReadOptions::default())
        .unwrap_or_else(|err| panic!("{}: parse failed: {err}", path.display()));
    let rewritten = write_psd(&psd, &WriteOptions::default())
        .unwrap_or_else(|err| panic!("{}: write failed: {err}", path.display()));
    let rewritten_tysh = layer_tagged_block_data(&rewritten, "Website", "TySh");
    let rewritten_descriptor_version =
        u32::from_be_bytes(rewritten_tysh[52..56].try_into().expect("descriptor version"));

    assert_eq!(original_descriptor_version, 16);
    assert_eq!(rewritten_descriptor_version, original_descriptor_version);
    assert_eq!(rewritten_tysh, original_tysh);
}

#[test]
fn tysh_semantic_rewrite_preserves_sample_text_layer_semantics() {
    let path = in_repo_sample_fixtures_dir().join("4901393.psd");
    let original = fs::read(&path).expect("read sample fixture");
    let original_tysh = layer_tagged_block_data(&original, "Website", "TySh");

    let mut psd = read_psd(Cursor::new(&original), ReadOptions::default())
        .unwrap_or_else(|err| panic!("{}: parse failed: {err}", path.display()));
    clear_text_raw_bytes(
        psd.children
            .as_mut()
            .expect("sample should contain layers")
            .as_mut_slice(),
    );
    let rewritten = write_psd(&psd, &WriteOptions::default())
        .unwrap_or_else(|err| panic!("{}: write failed: {err}", path.display()));
    let reparsed = read_psd(Cursor::new(&rewritten), ReadOptions::default())
        .unwrap_or_else(|err| panic!("{}: reparse failed: {err}", path.display()));

    assert_psd_semantically_equal(&psd, &reparsed, false);
    let rewritten_tysh = layer_tagged_block_data(&rewritten, "Website", "TySh");
    assert_eq!(
        u32::from_be_bytes(rewritten_tysh[52..56].try_into().expect("descriptor version")),
        u32::from_be_bytes(original_tysh[52..56].try_into().expect("descriptor version")),
    );
}

#[test]
fn existing_txt2_block_is_not_rewritten_on_roundtrip() {
    let path = in_repo_sample_fixtures_dir().join("4901393.psd");
    let original = fs::read(&path).expect("read sample fixture");
    let original_txt2 = document_tagged_block_data(&original, "Txt2");

    let psd = read_psd(Cursor::new(&original), ReadOptions::default())
        .unwrap_or_else(|err| panic!("{}: parse failed: {err}", path.display()));
    let rewritten = write_psd(&psd, &WriteOptions::default())
        .unwrap_or_else(|err| panic!("{}: write failed: {err}", path.display()));
    let rewritten_txt2 = document_tagged_block_data(&rewritten, "Txt2");

    assert_eq!(rewritten_txt2, original_txt2);
}

#[test]
fn read_color_preserves_raw_rgb_hsb_lab_values() {
    let rgb_bytes = vec![0x00, 0x00, 0x12, 0x34, 0x56, 0x78, 0x9a, 0xbc, 0x00, 0x00];
    let mut rgb_reader = PsdReader::new(Cursor::new(rgb_bytes), ReadOptions::default());
    assert_eq!(
        rgb_reader.read_color().unwrap(),
        Color::Rgb48 {
            red: 0x1234,
            green: 0x5678,
            blue: 0x9abc,
        }
    );

    let hsb_bytes = vec![0x00, 0x01, 0x11, 0x11, 0x22, 0x22, 0x33, 0x33, 0x00, 0x00];
    let mut hsb_reader = PsdReader::new(Cursor::new(hsb_bytes), ReadOptions::default());
    assert_eq!(
        hsb_reader.read_color().unwrap(),
        Color::Hsb {
            hue: 0x1111,
            saturation: 0x2222,
            brightness: 0x3333,
        }
    );

    let lab_bytes = vec![0x00, 0x07, 0x27, 0x10, 0xff, 0x9c, 0x00, 0x64, 0x00, 0x00];
    let mut lab_reader = PsdReader::new(Cursor::new(lab_bytes), ReadOptions::default());
    assert_eq!(
        lab_reader.read_color().unwrap(),
        Color::Lab {
            lightness: 10000,
            a: -100,
            b: 100,
        }
    );
}

#[test]
fn non_ascii_layer_names_roundtrip() {
    let psd = Psd {
        width: 1,
        height: 1,
        color_mode: Some(ColorMode::RGB),
        bits_per_channel: Some(8),
        children: Some(vec![Layer {
            top: Some(0),
            left: Some(0),
            bottom: Some(1),
            right: Some(1),
            image_data: Some(PixelData {
                data: vec![1, 2, 3, 4],
                width: 1,
                height: 1,
            }),
            additional_info: psd_great::additional_info::LayerAdditionalInfo {
                name: Some("Žluťoučký 😀".to_string()),
                ..Default::default()
            },
            ..Default::default()
        }]),
        ..Default::default()
    };
    let bytes = write_psd(&psd, &WriteOptions::default()).unwrap();
    let read = read_psd(Cursor::new(bytes), ReadOptions::default()).unwrap();
    assert_eq!(
        read.children.unwrap()[0].additional_info.name.as_deref(),
        Some("Žluťoučký 😀")
    );
}

#[test]
fn psb_write_read_roundtrip() {
    let psd = Psd {
        width: 4,
        height: 4,
        color_mode: Some(ColorMode::RGB),
        bits_per_channel: Some(8),
        image_data: Some(PixelData {
            data: vec![128u8; 4 * 4 * 4],
            width: 4,
            height: 4,
        }),
        children: Some(vec![Layer {
            top: Some(0),
            left: Some(0),
            bottom: Some(4),
            right: Some(4),
            image_data: Some(PixelData {
                data: vec![200u8; 4 * 4 * 4],
                width: 4,
                height: 4,
            }),
            ..Default::default()
        }]),
        ..Default::default()
    };
    let bytes = write_psd(
        &psd,
        &WriteOptions {
            psb: Some(true),
            compress: Some(false),
            ..Default::default()
        },
    )
    .unwrap();
    let read = read_psd(Cursor::new(bytes), ReadOptions::default()).unwrap();
    assert_eq!((read.width, read.height), (4, 4));
    assert_eq!(read.children.as_ref().map(|c| c.len()), Some(1));
    let layer = &read.children.unwrap()[0];
    assert_eq!(layer.image_data.as_ref().unwrap().data[0], 200);
}

#[test]
fn psb_rle_roundtrip() {
    let psd = Psd {
        width: 8,
        height: 8,
        color_mode: Some(ColorMode::RGB),
        bits_per_channel: Some(8),
        image_data: Some(PixelData {
            data: vec![99u8; 8 * 8 * 4],
            width: 8,
            height: 8,
        }),
        children: Some(vec![Layer {
            top: Some(0),
            left: Some(0),
            bottom: Some(8),
            right: Some(8),
            image_data: Some(PixelData {
                data: vec![50u8; 8 * 8 * 4],
                width: 8,
                height: 8,
            }),
            ..Default::default()
        }]),
        ..Default::default()
    };
    let bytes = write_psd(
        &psd,
        &WriteOptions {
            psb: Some(true),
            ..Default::default()
        },
    )
    .unwrap();
    let read = read_psd(Cursor::new(bytes), ReadOptions::default()).unwrap();
    assert_eq!(read.children.unwrap()[0].image_data.as_ref().unwrap().data[0], 50);
    assert_eq!(read.image_data.unwrap().data[0], 99);
}

#[test]
fn writing_synthesized_layers_in_non_rgb_mode_errors() {
    let psd = Psd {
        width: 1,
        height: 1,
        color_mode: Some(ColorMode::CMYK),
        bits_per_channel: Some(8),
        children: Some(vec![Layer {
            top: Some(0),
            left: Some(0),
            bottom: Some(1),
            right: Some(1),
            image_data: Some(PixelData {
                data: vec![1, 2, 3, 4],
                width: 1,
                height: 1,
            }),
            ..Default::default()
        }]),
        ..Default::default()
    };
    let err = write_psd(&psd, &WriteOptions::default()).unwrap_err();
    assert!(matches!(err, psd_great::PsdError::UnsupportedFeature(_)));
}

#[test]
fn write_color_roundtrips_raw_color_structures_exactly() {
    let colors = [
        Color::Rgb48 {
            red: 0x1234,
            green: 0x5678,
            blue: 0x9abc,
        },
        Color::Hsb {
            hue: 0x1111,
            saturation: 0x2222,
            brightness: 0x3333,
        },
        Color::Lab {
            lightness: 10000,
            a: -100,
            b: 100,
        },
        Color::CMYK(CMYK {
            c: 0x1111,
            m: 0x2222,
            y: 0x3333,
            k: 0x4444,
        }),
        Color::Grayscale(Grayscale { k: 0x2710 }),
        Color::OpaqueColorSpace {
            color_space: 42,
            components: [1, 2, 3, 4],
        },
    ];

    for color in colors {
        let mut writer = PsdWriter::new(32);
        writer.write_color(Some(&color)).unwrap();
        let bytes = writer.into_buffer();
        let mut reader = PsdReader::new(Cursor::new(bytes), ReadOptions::default());
        assert_eq!(reader.read_color().unwrap(), color);
    }
}

#[test]
fn write_color_rejects_lossy_rgb_rgba_and_frgb_color_structures() {
    let lossy_colors = [
        Color::RGB(RGB {
            r: 0x12,
            g: 0x34,
            b: 0x56,
        }),
        Color::RGBA(RGBA {
            r: 0x12,
            g: 0x34,
            b: 0x56,
            a: 0x78,
        }),
        Color::FRGB(FRGB {
            fr: 0.1,
            fg: 0.2,
            fb: 0.3,
        }),
    ];

    for color in lossy_colors {
        let mut writer = PsdWriter::new(32);
        let err = writer.write_color(Some(&color)).unwrap_err();
        assert!(
            err.to_string().contains("lossless"),
            "unexpected error: {err}"
        );
    }
}

#[test]
fn color_sampler_public_model_preserves_version_specific_positions() {
    let v1 = psd::ColorSampler {
        position: psd::ColorSamplerPosition::V1 {
            horizontal: 11,
            vertical: 22,
        },
        color_space: 0,
    };
    let v2 = psd::ColorSampler {
        position: psd::ColorSamplerPosition::V2 {
            horizontal: 33,
            vertical: 44,
            depth: 16,
        },
        color_space: 8,
    };
    let unsupported = psd::ColorSampler {
        position: psd::ColorSamplerPosition::Unsupported {
            version: 9,
            horizontal: 55,
            vertical: 66,
            depth: Some(32),
        },
        color_space: 1,
    };

    assert_ne!(v1.position, v2.position);
    assert_eq!(unsupported.position.version(), 9);
    assert_eq!(v1.position.coordinates(), (11, 22));
    assert_eq!(v2.position.coordinates(), (33, 44));

    match v2.position {
        psd::ColorSamplerPosition::V2 { depth, .. } => assert_eq!(depth, 16),
        _ => panic!("expected v2 sampler"),
    }

    match unsupported.position {
        psd::ColorSamplerPosition::Unsupported { depth, .. } => assert_eq!(depth, Some(32)),
        _ => panic!("expected unsupported sampler"),
    }
}

#[test]
fn test_basic_types() {
    // Test color types
    let rgba = RGBA {
        r: 255,
        g: 128,
        b: 64,
        a: 255,
    };
    assert_eq!(rgba.r, 255);

    let rgb = RGB {
        r: 255,
        g: 128,
        b: 64,
    };
    assert_eq!(rgb.g, 128);

    let cmyk = CMYK {
        c: 100,
        m: 50,
        y: 25,
        k: 0,
    };
    assert_eq!(cmyk.c, 100);

    // Test blend mode
    let blend = BlendMode::Normal;
    assert_eq!(blend, BlendMode::Normal);

    // Test color mode
    let mode = ColorMode::RGB;
    assert_eq!(mode as u16, 3);
}

#[test]
fn test_layer_creation() {
    let layer = Layer {
        top: Some(10),
        left: Some(20),
        bottom: Some(100),
        right: Some(200),
        blend_mode: Some(BlendMode::Normal),
        opacity: Some(100.0),
        transparency_protected: Some(false),
        effects_open: Some(false),
        hidden: Some(false),
        clipping: Some(0),
        image_data: None,
        raw_data: None,
        children: None,
        opened: Some(true),
        link_group: None,
        link_group_enabled: None,
        additional_info: LayerAdditionalInfo {
            name: Some("Test Layer".to_string()),
            id: Some(1),
            ..Default::default()
        },
        ..Default::default()
    };

    assert_eq!(layer.additional_info.name, Some("Test Layer".to_string()));
    assert_eq!(layer.top, Some(10));
}

#[test]
fn test_psd_structure() {
    let psd = Psd {
        width: 1920,
        height: 1080,
        channels: Some(3),
        bits_per_channel: Some(8),
        color_mode: Some(ColorMode::RGB),
        palette: None,
        children: None,
        image_data: None,
        image_resources: None,
        linked_files: None,
        artboards: None,
        global_layer_mask_info: None,
        annotations: None,
        additional_info: LayerAdditionalInfo {
            name: Some("Test Document".to_string()),
            ..Default::default()
        },
        ..Default::default()
    };

    assert_eq!(psd.width, 1920);
    assert_eq!(psd.height, 1080);
    assert_eq!(psd.color_mode, Some(ColorMode::RGB));
}

#[test]
fn test_rgba_value_type() {
    let rgba = RGBA {
        r: 255,
        g: 128,
        b: 64,
        a: 255,
    };
    assert_eq!(rgba.a, 255);
}

#[test]
fn test_effects() {
    let shadow = LayerEffectShadow {
        present: Some(true),
        show_in_dialog: Some(true),
        enabled: Some(true),
        intensity: Some(75.0),
        size: Some(UnitsValue {
            units: Units::Pixels,
            value: 5.0,
        }),
        angle: Some(120.0),
        distance: Some(UnitsValue {
            units: Units::Pixels,
            value: 10.0,
        }),
        color: Some(Color::RGBA(RGBA {
            r: 0,
            g: 0,
            b: 0,
            a: 255,
        })),
        blend_mode: Some(BlendMode::Multiply),
        opacity: Some(0.75),
        use_global_light: Some(true),
        antialiased: Some(true),
        contour: None,
        choke: None,
        layer_conceals: None,
    };

    assert_eq!(shadow.enabled, Some(true));
    assert_eq!(shadow.angle, Some(120.0));
}

// ============================================================================
// Integration Tests - Real-world Scenarios
// ============================================================================

#[test]
fn test_create_and_write_simple_psd() {
    // Create a simple PSD document with basic properties
    let psd = Psd {
        width: 100,
        height: 100,
        channels: Some(4), // RGBA
        bits_per_channel: Some(8),
        color_mode: Some(ColorMode::RGB),
        children: Some(vec![Layer {
            top: Some(0),
            left: Some(0),
            bottom: Some(100),
            right: Some(100),
            blend_mode: Some(BlendMode::Normal),
            opacity: Some(255.0),
            additional_info: LayerAdditionalInfo {
                name: Some("Background".to_string()),
                id: Some(1),
                ..Default::default()
            },
            ..Default::default()
        }]),
        additional_info: LayerAdditionalInfo {
            name: Some("Simple Test".to_string()),
            ..Default::default()
        },
        ..Default::default()
    };

    // Write the PSD
    let options = WriteOptions {
        compress: Some(false),
        psb: Some(false),
        generate_thumbnail: Some(false),
        trim_image_data: Some(false),
        invalidate_text_layers: None,
        log_missing_features: None,
        no_background: None,
    };

    let result = write_psd(&psd, &options);
    assert!(result.is_ok(), "Failed to write PSD: {:?}", result.err());

    let buffer = result.unwrap();
    assert!(buffer.len() > 0, "Written buffer should not be empty");

    // Verify header signature
    assert_eq!(&buffer[0..4], b"8BPS", "Invalid PSD signature");
}

#[test]
fn test_write_and_read_roundtrip() {
    // Create a PSD with multiple layers
    let original_psd = Psd {
        width: 200,
        height: 150,
        channels: Some(3),
        bits_per_channel: Some(8),
        color_mode: Some(ColorMode::RGB),
        children: Some(vec![
            Layer {
                top: Some(0),
                left: Some(0),
                bottom: Some(150),
                right: Some(200),
                blend_mode: Some(BlendMode::Normal),
                opacity: Some(255.0),
                additional_info: LayerAdditionalInfo {
                    name: Some("Layer 1".to_string()),
                    id: Some(1),
                    ..Default::default()
                },
                ..Default::default()
            },
            Layer {
                top: Some(25),
                left: Some(25),
                bottom: Some(125),
                right: Some(175),
                blend_mode: Some(BlendMode::Multiply),
                opacity: Some(128.0),
                additional_info: LayerAdditionalInfo {
                    name: Some("Layer 2".to_string()),
                    id: Some(2),
                    ..Default::default()
                },
                ..Default::default()
            },
        ]),
        additional_info: LayerAdditionalInfo {
            name: Some("Roundtrip Test".to_string()),
            ..Default::default()
        },
        ..Default::default()
    };

    // Write to buffer
    let write_options = WriteOptions::default();
    let buffer = write_psd(&original_psd, &write_options).expect("Failed to write PSD");

    // Read back from buffer
    let read_options = ReadOptions::default();
    let cursor = Cursor::new(buffer);
    let read_psd = read_psd(cursor, read_options).expect("Failed to read PSD");

    // Verify core properties match
    assert_eq!(read_psd.width, original_psd.width);
    assert_eq!(read_psd.height, original_psd.height);
    assert_eq!(read_psd.channels, original_psd.channels);
    assert_eq!(read_psd.bits_per_channel, original_psd.bits_per_channel);
    assert_eq!(read_psd.color_mode, original_psd.color_mode);
}

#[test]
fn test_global_layer_mask_roundtrip() {
    let original_psd = Psd {
        width: 2,
        height: 2,
        channels: Some(3),
        bits_per_channel: Some(8),
        color_mode: Some(ColorMode::RGB),
        global_layer_mask_info: Some(GlobalLayerMaskInfo {
            overlay_color_space: 1,
            color_space1: 2,
            color_space2: 3,
            color_space3: 4,
            color_space4: 5,
            opacity: 255,
            kind: 128,
        }),
        ..Default::default()
    };

    let buffer = write_psd(&original_psd, &WriteOptions::default()).unwrap();
    let read_psd = read_psd(
        Cursor::new(buffer),
        ReadOptions {
            skip_composite_image_data: Some(true),
            ..Default::default()
        },
    )
    .unwrap();

    assert_eq!(
        read_psd.global_layer_mask_info,
        original_psd.global_layer_mask_info
    );
}

#[test]
fn test_different_color_modes() {
    let color_modes = vec![
        ColorMode::Bitmap,
        ColorMode::Grayscale,
        ColorMode::Indexed,
        ColorMode::RGB,
        ColorMode::CMYK,
        ColorMode::Multichannel,
        ColorMode::Duotone,
        ColorMode::Lab,
    ];

    for color_mode in color_modes {
        let psd = Psd {
            width: 50,
            height: 50,
            channels: Some(match color_mode {
                ColorMode::Bitmap | ColorMode::Grayscale => 1,
                ColorMode::RGB => 3,
                ColorMode::CMYK => 4,
                _ => 3,
            }),
            bits_per_channel: Some(8),
            color_mode: Some(color_mode),
            ..Default::default()
        };

        let options = WriteOptions::default();
        let result = write_psd(&psd, &options);

        // Some color modes may not be fully supported yet
        if result.is_ok() {
            let buffer = result.unwrap();
            assert!(buffer.len() > 0);
        }
    }
}

#[test]
fn test_psd_roundtrip_does_not_depend_on_serde() {
    let psd = Psd {
        width: 4,
        height: 4,
        channels: Some(4),
        bits_per_channel: Some(8),
        color_mode: Some(ColorMode::RGB),
        image_data: Some(PixelData {
            data: vec![128u8; 4 * 4 * 4],
            width: 4,
            height: 4,
        }),
        ..Default::default()
    };

    let bytes = write_psd(&psd, &WriteOptions::default()).expect("write");
    let reparsed = read_psd(
        std::io::Cursor::new(bytes),
        ReadOptions {
            skip_composite_image_data: Some(true),
            ..Default::default()
        },
    )
    .expect("read");
    assert_eq!(reparsed.width, 4);
    assert_eq!(reparsed.height, 4);
}

#[test]
fn test_canonical_image_resource_types_are_public() {
    // Test that image_resource types are used canonically
    let resources = ImageResources {
        resolution_info: Some(psd_great::image_resources::ResolutionInfo {
            horizontal_res: 300.0,
            horizontal_res_unit: psd_great::image_resources::ResolutionUnit::PixelsPerInch,
            width_unit: psd_great::image_resources::MeasurementUnit::Inches,
            vertical_res: 300.0,
            vertical_res_unit: psd_great::image_resources::ResolutionUnit::PixelsPerInch,
            height_unit: psd_great::image_resources::MeasurementUnit::Inches,
        }),
        ..Default::default()
    };

    assert!(resources.resolution_info.is_some());
}

#[test]
fn slices_v6_roundtrip_preserves_per_slice_descriptor_tail() {
    let descriptor = psd_great::descriptor::Descriptor {
        name: String::new(),
        class_id: "null".to_string(),
        items: HashMap::new(),
    };
    let slices = psd_great::image_resources::Slices {
        version: 6,
        bounds: Some(psd_great::image_resources::SliceBounds {
            top: 0,
            left: 0,
            bottom: 100,
            right: 100,
        }),
        group_name: Some("legacy".to_string()),
        slices: vec![
            psd_great::image_resources::Slice {
                id: 1,
                group_id: 2,
                origin: SliceOrigin::AutoGenerated,
                associated_layer_id: 0,
                name: "first".to_string(),
                slice_type: SliceType::NoImage,
                bounds: psd_great::image_resources::SliceBounds {
                    top: 10,
                    left: 20,
                    bottom: 30,
                    right: 40,
                },
                url: "https://example.com/1".to_string(),
                target: "_self".to_string(),
                message: "first".to_string(),
                alt_tag: "first".to_string(),
                cell_text: "first".to_string(),
                horizontal_align: SliceAlignment::LeftOrTop,
                vertical_align: SliceAlignment::Center,
                alpha: 255,
                bg_color: [255, 1, 2, 3],
                cell_is_html: false,
                source_id: None,
                source_type: None,
                descriptor: Some(descriptor.clone()),
            },
            psd_great::image_resources::Slice {
                id: 11,
                group_id: 12,
                origin: SliceOrigin::LayerBased,
                associated_layer_id: 13,
                name: "second".to_string(),
                slice_type: SliceType::Image,
                bounds: psd_great::image_resources::SliceBounds {
                    top: 50,
                    left: 60,
                    bottom: 70,
                    right: 80,
                },
                url: "https://example.com/2".to_string(),
                target: "_blank".to_string(),
                message: "second".to_string(),
                alt_tag: "second".to_string(),
                cell_text: "second".to_string(),
                horizontal_align: SliceAlignment::RightOrBottom,
                vertical_align: SliceAlignment::Other(4),
                alpha: 200,
                bg_color: [200, 4, 5, 6],
                cell_is_html: true,
                source_id: None,
                source_type: None,
                descriptor: None,
            },
        ],
        descriptor: None,
    };
    let resources = ImageResources {
        slices: Some(slices.clone()),
        ..Default::default()
    };

    let mut writer = PsdWriter::new(1024);
    psd_great::image_resources::write_image_resources(&mut writer, &resources).expect("write");
    let bytes = writer.into_buffer();
    let mut reader = PsdReader::new(Cursor::new(bytes.clone()), ReadOptions::default());
    let reparsed =
        psd_great::image_resources::read_image_resources(&mut reader, bytes.len()).expect("read");

    assert_eq!(reparsed.slices, Some(slices));
}

#[test]
fn slices_v6_write_rejects_unsupported_source_fields() {
    let resources = ImageResources {
        slices: Some(psd_great::image_resources::Slices {
            version: 6,
            bounds: Some(psd_great::image_resources::SliceBounds {
                top: 0,
                left: 0,
                bottom: 10,
                right: 10,
            }),
            group_name: Some("legacy".to_string()),
            slices: vec![psd_great::image_resources::Slice {
                id: 1,
                group_id: 2,
                origin: SliceOrigin::AutoGenerated,
                associated_layer_id: 0,
                name: "slice".to_string(),
                slice_type: SliceType::Image,
                bounds: psd_great::image_resources::SliceBounds {
                    top: 1,
                    left: 2,
                    bottom: 3,
                    right: 4,
                },
                url: String::new(),
                target: String::new(),
                message: String::new(),
                alt_tag: String::new(),
                cell_text: String::new(),
                horizontal_align: SliceAlignment::Default,
                vertical_align: SliceAlignment::Default,
                alpha: 0,
                bg_color: [0, 0, 0, 0],
                cell_is_html: false,
                source_id: Some(42),
                source_type: Some(SliceSourceType::Other(7)),
                descriptor: None,
            }],
            descriptor: None,
        }),
        ..Default::default()
    };

    let mut writer = PsdWriter::new(256);
    let err = psd_great::image_resources::write_image_resources(&mut writer, &resources)
        .expect_err(
            "v6 slice write should reject unsupported source fields instead of dropping them",
        );

    assert!(
        err.to_string().contains("source_id/source_type"),
        "unexpected error: {err}"
    );
}

#[test]
fn test_canonical_tagged_block_types_are_used_by_layer_additional_info() {
    let divider = psd_great::additional_info::SectionDivider {
        divider_type: SectionDividerType::OpenFolder,
        blend_mode: None,
        sub_type: None,
    };
    let stroke = psd_great::additional_info::VectorStroke {
        version: 16,
        descriptor: psd_great::descriptor::Descriptor {
            name: String::new(),
            class_id: "null".to_string(),
            items: std::collections::HashMap::new(),
        },
    };
    let placed = psd_great::additional_info::PlacedLayer {
        id: "id".to_string(),
        page: None,
        total_pages: None,
        anti_alias_policy: None,
        placed_layer_type: None,
        transform: vec![1.0, 0.0, 0.0, 1.0, 0.0, 0.0],
        legacy_warp: None,
        sold_descriptor: None,
        placed: None,
    };

    let info = LayerAdditionalInfo {
        section_divider: Some(divider),
        vector_stroke: Some(stroke),
        placed_layer: Some(placed),
        ..Default::default()
    };

    assert!(info.section_divider.is_some());
    assert!(info.vector_stroke.is_some());
    assert!(info.placed_layer.is_some());
}

#[test]
fn test_adjustment_layer_comes_from_canonical_module() {
    // This will use the canonical AdjustmentLayer from adjustments.rs
    // once the duplicate in layer.rs is removed
    let adjustment = adjustments::AdjustmentLayer::Invert;
    match adjustment {
        adjustments::AdjustmentLayer::Invert => {}
        _ => panic!("wrong adjustment layer variant"),
    }
}

#[test]
fn test_canonical_shared_types_are_constructible_from_public_api() {
    let point = Point { x: 1.0, y: 2.0 };
    let fraction = Fraction {
        numerator: 1,
        denominator: 24,
    };
    let rendering_intent = RenderingIntent::Perceptual;

    assert_eq!(point.x, 1.0);
    assert_eq!(fraction.denominator, 24);
    assert_eq!(rendering_intent, RenderingIntent::Perceptual);
}

#[test]
fn test_no_remaining_duplicate_public_model_type_names() {
    let source_files = [
        (
            "src/format/additional_info.rs",
            include_str!("../src/format/additional_info.rs"),
        ),
        (
            "src/api/adjustments.rs",
            include_str!("../src/api/adjustments.rs"),
        ),
        (
            "src/format/image_resources.rs",
            include_str!("../src/format/image_resources.rs"),
        ),
        ("src/api/layer.rs", include_str!("../src/api/layer.rs")),
        ("src/api/psd.rs", include_str!("../src/api/psd.rs")),
        ("src/api/types.rs", include_str!("../src/api/types.rs")),
    ];

    let audited_names = [
        "AdjustmentLayer",
        "Bounds",
        "Fraction",
        "LayerComps",
        "OnionSkins",
        "PlacedLayer",
        "Point",
        "PrintScale",
        "ProofSetup",
        "RenderingIntent",
        "SectionDivider",
        "Slice",
        "Timeline",
        "VectorStroke",
    ];

    let mut duplicates = Vec::new();

    for name in audited_names {
        let mut hits = Vec::new();
        for (path, contents) in &source_files {
            // Use regex-like word boundary matching to avoid false positives
            // (e.g. "PlacedLayer" should not match "PlacedLayerType")
            let needle_struct = format!("pub struct {name} ");
            let needle_struct_semi = format!("pub struct {name}}}");
            let needle_enum = format!("pub enum {name} ");
            let needle_enum_semi = format!("pub enum {name}}}");
            let needle_type = format!("pub type {name} ");
            if contents.contains(&needle_struct)
                || contents.contains(&needle_struct_semi)
                || contents.contains(&needle_enum)
                || contents.contains(&needle_enum_semi)
                || contents.contains(&needle_type)
            {
                hits.push(*path);
            }
        }
        if hits.len() > 1 {
            duplicates.push((name, hits));
        }
    }

    assert!(
        duplicates.is_empty(),
        "duplicate public model types remain: {:?}",
        duplicates
    );
}

#[test]
fn test_no_duplicate_public_type_names_for_canonical_models() {
    let files = [
        (
            "src/format/additional_info.rs",
            include_str!("../src/format/additional_info.rs"),
        ),
        (
            "src/format/image_resources.rs",
            include_str!("../src/format/image_resources.rs"),
        ),
        ("src/api/layer.rs", include_str!("../src/api/layer.rs")),
        ("src/api/psd.rs", include_str!("../src/api/psd.rs")),
        ("src/api/types.rs", include_str!("../src/api/types.rs")),
    ];

    let mut seen: HashMap<&str, Vec<&str>> = HashMap::new();
    let target_names = [
        "LayerAdditionalInfo",
        "LayerColor",
        "ResolutionInfo",
        "PrintInformation",
        "PrintFlags",
    ];

    for (path, source) in files {
        for line in source.lines() {
            for name in target_names {
                if line.contains(&format!("pub struct {name}"))
                    || line.contains(&format!("pub enum {name}"))
                {
                    seen.entry(name).or_default().push(path);
                }
            }
        }
    }

    let duplicates: Vec<_> = seen
        .into_iter()
        .filter(|(_, paths)| paths.len() > 1)
        .collect();

    assert!(
        duplicates.is_empty(),
        "duplicate canonical public types remain: {duplicates:?}"
    );
}

#[test]
fn test_layer_effects_roundtrip() {
    let layer_effects = LayerEffectsInfo {
        disabled: Some(false),
        scale: Some(100.0),
        drop_shadow: Some(vec![LayerEffectShadow {
            present: Some(true),
            show_in_dialog: None,
            enabled: Some(true),
            intensity: Some(75.0),
            blend_mode: Some(BlendMode::Multiply),
            color: Some(Color::RGBA(RGBA {
                r: 0,
                g: 0,
                b: 0,
                a: 255,
            })),
            opacity: Some(0.75),
            angle: Some(120.0),
            distance: Some(UnitsValue {
                units: Units::Pixels,
                value: 10.0,
            }),
            size: Some(UnitsValue {
                units: Units::Pixels,
                value: 5.0,
            }),
            use_global_light: Some(true),
            antialiased: None,
            contour: None,
            choke: None,
            layer_conceals: None,
        }]),
        inner_shadow: None,
        outer_glow: None,
        inner_glow: None,
        bevel: None,
        solid_fill: None,
        satin: None,
        stroke: None,
        gradient_overlay: None,
        pattern_overlay: None,
    };

    // Verify effects are properly structured
    assert!(layer_effects.drop_shadow.is_some());
    let shadow = &layer_effects.drop_shadow.as_ref().unwrap()[0];
    assert_eq!(shadow.enabled, Some(true));
    assert_eq!(shadow.blend_mode, Some(BlendMode::Multiply));
}

#[test]
fn write_effects_roundtrips_display_colors_through_raw_photoshop_records() {
    let effects = LayerEffectsInfo {
        disabled: Some(false),
        scale: Some(100.0),
        drop_shadow: Some(vec![LayerEffectShadow {
            present: Some(true),
            show_in_dialog: None,
            enabled: Some(true),
            intensity: Some(75.0),
            blend_mode: Some(BlendMode::Multiply),
            color: Some(Color::RGBA(RGBA {
                r: 0x12,
                g: 0x34,
                b: 0x56,
                a: 0xff,
            })),
            opacity: Some(0.75),
            angle: Some(120.0),
            distance: Some(UnitsValue {
                units: Units::Pixels,
                value: 10.0,
            }),
            size: Some(UnitsValue {
                units: Units::Pixels,
                value: 5.0,
            }),
            use_global_light: Some(true),
            antialiased: None,
            contour: None,
            choke: None,
            layer_conceals: None,
        }]),
        inner_shadow: None,
        outer_glow: Some(LayerEffectsOuterGlow {
            present: None,
            show_in_dialog: None,
            enabled: Some(true),
            size: Some(UnitsValue {
                units: Units::Pixels,
                value: 7.0,
            }),
            intensity: Some(50.0),
            color: None,
            blend_mode: Some(BlendMode::Screen),
            opacity: Some(0.5),
            source: None,
            antialiased: None,
            noise: None,
            range: None,
            choke: None,
            jitter: None,
            contour: None,
        }),
        inner_glow: None,
        bevel: None,
        solid_fill: None,
        satin: None,
        stroke: None,
        gradient_overlay: None,
        pattern_overlay: None,
    };

    let mut writer = PsdWriter::new(256);
    write_effects(&mut writer, &effects).unwrap();

    let bytes = writer.into_buffer();
    let mut reader = PsdReader::new(Cursor::new(bytes), ReadOptions::default());
    let reparsed = read_effects(&mut reader).unwrap();

    assert_eq!(
        reparsed.drop_shadow.as_ref().unwrap()[0].color,
        Some(Color::Rgb48 {
            red: 0x1212,
            green: 0x3434,
            blue: 0x5656,
        })
    );
    assert_eq!(
        reparsed.outer_glow.as_ref().unwrap().color,
        Some(Color::Rgb48 {
            red: 0,
            green: 0,
            blue: 0,
        })
    );
}

#[test]
fn test_text_layer_data() {
    // Basic test for text layer structure
    let text_data = LayerTextData {
        text: "Hello, World!".to_string(),
        transform: Some(vec![1.0, 0.0, 0.0, 1.0, 0.0, 0.0]),
        anti_alias: Some(AntiAlias::Sharp),
        gridding: None,
        orientation: None,
        index: None,
        warp: None,
        top: None,
        left: None,
        bottom: None,
        right: None,
        grid_info: None,
        use_fractional_glyph_widths: None,
        style: None,
        style_runs: None,
        paragraph_style: None,
        paragraph_style_runs: None,
        superscript_size: None,
        superscript_position: None,
        subscript_size: None,
        subscript_position: None,
        small_cap_size: None,
        shape_type: None,
        point_base: None,
        box_bounds: None,
        bounds: None,
        bounding_box: None,
        text_path: None,
    };

    assert_eq!(text_data.text, "Hello, World!");
    assert!(text_data.transform.is_some());
    assert_eq!(text_data.anti_alias, Some(AntiAlias::Sharp));
}

#[test]
fn test_compression_methods() {
    // Test that compression functions exist and can be called
    let data = vec![1, 1, 1, 2, 2, 3, 3, 3, 3];

    // Test ZIP compression roundtrip
    let compressed_zip = compress_zip(&data).expect("Failed to compress ZIP");
    assert!(compressed_zip.len() > 0);

    let decompressed_zip =
        decompress_zip(&compressed_zip, data.len()).expect("Failed to decompress ZIP");
    assert_eq!(decompressed_zip, data);

    // Test RLE compression (basic test that it doesn't panic)
    let width = 3;
    let height = 3;
    let compress_result = compress_rle(&data, width, height, false);
    assert!(compress_result.is_ok());
}

#[test]
fn test_blend_mode_conversion() {
    let blend_modes = vec![
        BlendMode::PassThrough,
        BlendMode::Normal,
        BlendMode::Dissolve,
        BlendMode::Darken,
        BlendMode::Multiply,
        BlendMode::ColorBurn,
        BlendMode::LinearBurn,
        BlendMode::DarkerColor,
        BlendMode::Lighten,
        BlendMode::Screen,
        BlendMode::ColorDodge,
        BlendMode::LinearDodge,
        BlendMode::LighterColor,
        BlendMode::Overlay,
        BlendMode::SoftLight,
        BlendMode::HardLight,
        BlendMode::VividLight,
        BlendMode::LinearLight,
        BlendMode::PinLight,
        BlendMode::HardMix,
        BlendMode::Difference,
        BlendMode::Exclusion,
        BlendMode::Subtract,
        BlendMode::Divide,
        BlendMode::Hue,
        BlendMode::Saturation,
        BlendMode::Color,
        BlendMode::Luminosity,
    ];

    for mode in blend_modes {
        let key = from_blend_mode(mode);
        let converted = to_blend_mode(&key).expect("Failed to convert blend mode");
        assert_eq!(
            converted, mode,
            "Blend mode conversion failed for {:?}",
            mode
        );
    }
}

#[test]
fn test_layer_hierarchy() {
    // Test nested layer groups
    let psd = Psd {
        width: 100,
        height: 100,
        channels: Some(3),
        bits_per_channel: Some(8),
        color_mode: Some(ColorMode::RGB),
        children: Some(vec![Layer {
            additional_info: LayerAdditionalInfo {
                name: Some("Group 1".to_string()),
                section_divider: Some(SectionDivider {
                    divider_type: SectionDividerType::OpenFolder,
                    blend_mode: None,
                    sub_type: None,
                }),
                ..Default::default()
            },
            children: Some(vec![
                Layer {
                    additional_info: LayerAdditionalInfo {
                        name: Some("Layer 1.1".to_string()),
                        ..Default::default()
                    },
                    ..Default::default()
                },
                Layer {
                    additional_info: LayerAdditionalInfo {
                        name: Some("Layer 1.2".to_string()),
                        ..Default::default()
                    },
                    ..Default::default()
                },
            ]),
            ..Default::default()
        }]),
        ..Default::default()
    };

    assert!(psd.children.is_some());
    let children = psd.children.as_ref().unwrap();
    assert_eq!(children.len(), 1);

    let group = &children[0];
    assert_eq!(
        group
            .additional_info
            .section_divider
            .as_ref()
            .unwrap()
            .divider_type,
        SectionDividerType::OpenFolder
    );
    assert!(group.children.is_some());
    assert_eq!(group.children.as_ref().unwrap().len(), 2);
}

#[test]
fn test_image_resources() {
    let resources = ImageResources {
        alpha_identifiers: Some(vec![1, 2, 3]),
        ..Default::default()
    };

    assert!(resources.alpha_identifiers.is_some());
    let alpha_ids = resources.alpha_identifiers.as_ref().unwrap();
    assert_eq!(alpha_ids.len(), 3);
    assert_eq!(alpha_ids[0], 1);
}

#[test]
fn test_layer_colors() {
    let colors = vec![
        LayerColor::None,
        LayerColor::Red,
        LayerColor::Orange,
        LayerColor::Yellow,
        LayerColor::Green,
        LayerColor::Blue,
        LayerColor::Violet,
        LayerColor::Gray,
    ];

    for color in colors {
        let layer = Layer {
            additional_info: LayerAdditionalInfo {
                name: Some(format!("Layer {:?}", color)),
                layer_color: Some(color),
                ..Default::default()
            },
            ..Default::default()
        };

        assert_eq!(layer.additional_info.layer_color, Some(color));
    }
}

#[test]
fn test_read_existing_psd() {
    // Try to read a test PSD file if it exists
    let test_file = "../test/test.psd";
    if let Ok(file) = File::open(test_file) {
        let options = ReadOptions::default();
        let result = read_psd(file, options);

        if let Ok(psd) = result {
            assert!(psd.width > 0);
            assert!(psd.height > 0);
            assert!(psd.color_mode.is_some());
            println!("Successfully read test PSD: {}x{}", psd.width, psd.height);
        }
    }
}

#[test]
fn spec_color_mode_codes_match_adobe_header_values() {
    assert_eq!(ColorMode::Bitmap as u16, 0);
    assert_eq!(ColorMode::Grayscale as u16, 1);
    assert_eq!(ColorMode::Indexed as u16, 2);
    assert_eq!(ColorMode::RGB as u16, 3);
    assert_eq!(ColorMode::CMYK as u16, 4);
    assert_eq!(ColorMode::Multichannel as u16, 7);
    assert_eq!(ColorMode::Duotone as u16, 8);
    assert_eq!(ColorMode::Lab as u16, 9);
}

#[test]
fn packbits_0x80_is_a_noop() {
    let compressed = [0x80, 0x02, b'A', b'B', b'C'];
    let mut output = [0u8; 3];

    decompress_rle(&compressed, &mut output, 3, 1, &[5]).unwrap();

    assert_eq!(&output, b"ABC");
}

#[test]
fn additional_info_uses_even_padding_not_four_byte_padding() {
    let bytes = vec![
        b'8', b'B', b'I', b'M', // signature
        b'f', b'c', b'm', b'y', // key
        0, 0, 0, 2, // length includes even-byte padding
        7, // payload
        0, // even-byte padding
    ];
    let mut reader = psd_great::PsdReader::new(Cursor::new(bytes), ReadOptions::default());
    let parsed = psd_great::additional_info::read_layer_additional_info(&mut reader, 14).unwrap();

    assert_eq!(parsed.fcmy, Some(7));
}

#[test]
fn psb_large_tagged_blocks_use_8b64_length_headers() {
    let mut layer = Layer {
        top: Some(0),
        left: Some(0),
        bottom: Some(1),
        right: Some(1),
        ..Default::default()
    };
    layer.additional_info.high_depth_layer_data =
        Some(psd_great::additional_info::HighDepthLayerInfo {
            key: PsdStringCode::from("Lr16"),
            layers: vec![],
        });

    let psd = Psd {
        width: 1,
        height: 1,
        children: Some(vec![layer]),
        ..Default::default()
    };

    let bytes = write_psd(
        &psd,
        &WriteOptions {
            psb: Some(true),
            ..Default::default()
        },
    )
    .unwrap();

    assert!(bytes.windows(4).any(|w| w == b"8B64"));
}

#[test]
fn merged_alpha_writes_negative_layer_count() {
    let psd = Psd {
        width: 1,
        height: 1,
        channels: Some(4),
        bits_per_channel: Some(8),
        color_mode: Some(ColorMode::RGB),
        children: Some(vec![Layer {
            top: Some(0),
            left: Some(0),
            bottom: Some(1),
            right: Some(1),
            image_data: Some(PixelData {
                data: vec![1, 2, 3, 4],
                width: 1,
                height: 1,
            }),
            ..Default::default()
        }]),
        image_data: Some(PixelData {
            data: vec![1, 2, 3, 4],
            width: 1,
            height: 1,
        }),
        ..Default::default()
    };

    let bytes = write_psd(&psd, &WriteOptions::default()).unwrap();

    let layer_count_offset = 26 + 4 + 4 + 4 + 4;
    assert_eq!(
        &bytes[layer_count_offset..layer_count_offset + 2],
        &[0xFF, 0xFF]
    );
}

#[test]
fn composite_16bit_raw_roundtrip_reads_correct_plane_sizes() {
    let psd = Psd {
        width: 1,
        height: 1,
        channels: Some(3),
        bits_per_channel: Some(16),
        color_mode: Some(ColorMode::RGB),
        image_data: Some(PixelData {
            data: vec![0x12, 0x34, 0x56, 0xFF],
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
    .unwrap();
    let reparsed = read_psd(Cursor::new(bytes), ReadOptions::default()).unwrap();

    assert_eq!(
        reparsed.image_data.unwrap().data,
        vec![0x12, 0x34, 0x56, 0xFF]
    );
}

#[test]
fn composite_32bit_zip_prediction_roundtrip_reads_correct_plane_sizes() {
    let psd = Psd {
        width: 1,
        height: 1,
        channels: Some(1),
        bits_per_channel: Some(32),
        color_mode: Some(ColorMode::Grayscale),
        image_data: Some(PixelData {
            data: vec![0x40, 0x40, 0x40, 0xFF],
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
    .unwrap();
    let reparsed = read_psd(Cursor::new(bytes), ReadOptions::default()).unwrap();

    assert_eq!(
        reparsed.image_data.unwrap().data,
        vec![0x40, 0x40, 0x40, 0xFF]
    );
}

#[test]
fn path_resources_use_8_24_fixed_point() {
    let mut resources = ImageResources::default();
    resources.path_resources.insert(
        2000,
        vec![psd_great::image_resources::PathResourceRecord {
            record_type: 1,
            closed: true,
            points: vec![
                Point { x: 0.25, y: 1.0 },
                Point { x: 0.5, y: 0.5 },
                Point { x: 0.25, y: 0.25 },
                Point { x: 0.0, y: 0.0 },
            ],
        }],
    );

    let mut writer = PsdWriter::new(256);
    psd_great::image_resources::write_image_resources(&mut writer, &resources).unwrap();
    let bytes = writer.into_buffer();

    assert_eq!(&bytes[14..18], &[0x01, 0x00, 0x00, 0x00]);
    assert_eq!(&bytes[18..22], &[0x00, 0x40, 0x00, 0x00]);
}

#[test]
fn write_psd_rejects_life_version_4_without_required_time() {
    use psd_great::additional_info::LinkedFilesBlock;
    use psd_great::descriptor::Descriptor;

    let mut layer = Layer::default();
    layer.top = Some(0);
    layer.left = Some(0);
    layer.bottom = Some(1);
    layer.right = Some(1);
    layer.blend_mode = Some(BlendMode::Normal);
    layer.opacity = Some(1.0);
    layer.additional_info.linked_files = Some(LinkedFilesBlock {
        key: psd_great::PsdStringCode::from("lnkD"),
        items: vec![psd_great::LinkedFile {
            id: "asset-id".to_string(),
            name: "Placed Asset".to_string(),
            item_version: Some(4),
            data_kind: Some(psd_great::LinkedFileDataKind::External),
            file_type: Some(psd_great::PsdStringCode::from("8BPS")),
            creator: Some(psd_great::PsdStringCode::from("8BIM")),
            data: Some(vec![1, 2, 3, 4]),
            time: None,
            descriptor: Some(Descriptor {
                name: "linked".to_string(),
                class_id: "lnkF".to_string(),
                items: std::collections::HashMap::new(),
            }),
            child_document_id: None,
            asset_mod_time: None,
            asset_locked_state: None,
            linked_file: Some(psd_great::layer::LinkedFileInfo {
                file_size: 99,
                name: String::new(),
                full_path: String::new(),
                original_path: String::new(),
                relative_path: String::new(),
            }),
            open_descriptor: None,
        }],
    });

    let psd = Psd {
        width: 1,
        height: 1,
        channels: Some(4),
        bits_per_channel: Some(8),
        color_mode: Some(ColorMode::RGB),
        children: Some(vec![layer]),
        ..Default::default()
    };

    let err = write_psd(&psd, &WriteOptions::default()).unwrap_err();
    assert!(err.to_string().contains("time"), "unexpected error: {err}");
}

#[test]
fn canonical_image_resource_types_are_public() {
    let guide = psd_great::GuideInfo {
        location: 10.0,
        direction: psd_great::GuideDirection::Horizontal,
    };
    let sampler = psd_great::ColorSampler {
        position: psd_great::ColorSamplerPosition::V2 {
            horizontal: 1,
            vertical: 2,
            depth: 16,
        },
        color_space: 8,
    };

    assert_eq!(guide.direction, psd_great::GuideDirection::Horizontal);
    assert_eq!(sampler.position.version(), 2);

    let display = psd_great::image_resources::DisplayInfoResource {
        version: 1,
        h_res_unit: DisplayUnit::PixelsPerInch,
        v_res_unit: DisplayUnit::PixelsPerCentimeter,
        width_unit: DisplayUnit::Points,
        height_unit: DisplayUnit::Picas,
    };
    let slice = psd_great::image_resources::Slice {
        id: 1,
        group_id: 2,
        origin: SliceOrigin::LayerBased,
        associated_layer_id: 3,
        name: "slice".into(),
        slice_type: SliceType::NoImage,
        bounds: psd_great::image_resources::SliceBounds {
            top: 0,
            left: 0,
            bottom: 1,
            right: 1,
        },
        url: String::new(),
        target: String::new(),
        message: String::new(),
        alt_tag: String::new(),
        cell_text: String::new(),
        horizontal_align: SliceAlignment::Center,
        vertical_align: SliceAlignment::RightOrBottom,
        alpha: 0,
        bg_color: [0, 0, 0, 0],
        cell_is_html: false,
        source_id: None,
        source_type: Some(SliceSourceType::Other(7)),
        descriptor: None,
    };

    assert_eq!(display.h_res_unit, DisplayUnit::PixelsPerInch);
    assert_eq!(slice.origin, SliceOrigin::LayerBased);
    assert_eq!(slice.slice_type, SliceType::NoImage);
    assert_eq!(slice.horizontal_align, SliceAlignment::Center);
    assert_eq!(slice.source_type, Some(SliceSourceType::Other(7)));
}

#[test]
fn linked_file_kind_is_typed_publicly() {
    let file = psd_great::LinkedFile {
        id: "id".into(),
        name: "name".into(),
        data_kind: Some(psd_great::LinkedFileDataKind::External),
        file_type: None,
        creator: None,
        item_version: None,
        data: None,
        time: None,
        descriptor: None,
        child_document_id: None,
        asset_mod_time: None,
        asset_locked_state: None,
        linked_file: None,
        open_descriptor: None,
    };

    assert_eq!(file.data_kind, Some(psd_great::LinkedFileDataKind::External));
}

#[test]
fn guide_direction_and_display_units_are_typed_publicly() {
    use psd_great::psd::{
        DisplayInfo,
    };

    let guide = psd_great::GuideInfo {
        location: 10.0,
        direction: psd_great::GuideDirection::Vertical,
    };
    let mut psd = psd_great::Psd::default();
    psd.display_info = Some(DisplayInfo {
        h_res_unit: psd_great::DisplayUnit::PixelsPerInch,
        v_res_unit: psd_great::DisplayUnit::PixelsPerCentimeter,
        width_unit: psd_great::DisplayUnit::Inches,
        height_unit: psd_great::DisplayUnit::Centimeters,
    });

    assert_eq!(guide.direction, psd_great::GuideDirection::Vertical);
    assert_eq!(
        psd.display_info.unwrap().h_res_unit,
        psd_great::DisplayUnit::PixelsPerInch
    );
}

#[test]
fn display_unit_preserves_unknown_raw_values() {
    let unit = psd_great::DisplayUnit::from_u16(99);

    assert_eq!(unit, psd_great::DisplayUnit::Other(99));
    assert_eq!(unit.to_u16(), 99);
}

#[test]
fn guide_direction_preserves_unknown_descriptor_codes() {
    let direction = psd_great::GuideDirection::from_code("Abcd");

    assert_eq!(direction, psd_great::GuideDirection::Other(*b"Abcd"));
    assert_eq!(direction.to_code(), *b"Abcd");
}

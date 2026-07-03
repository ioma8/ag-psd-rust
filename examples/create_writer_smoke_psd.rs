use psd_great::{
    write_psd, BlendMode, ColorMode, Layer, LayerAdditionalInfo, PixelData, Psd, Result,
    WriteOptions,
};
use std::env;
use std::fs;

fn main() -> Result<()> {
    let args: Vec<String> = env::args().collect();
    let output = args
        .get(1)
        .cloned()
        .unwrap_or_else(|| "writer-smoke.psd".to_string());

    let width = 160usize;
    let height = 120usize;

    let background = solid_rgba(width, height, [250, 244, 232, 255]);
    let red_box = solid_rgba(120, 80, [220, 48, 48, 255]);
    let blue_box = solid_rgba(70, 70, [40, 110, 230, 180]);

    let composite = compose_preview(width, height);

    let psd = Psd {
        width: width as u32,
        height: height as u32,
        channels: Some(4),
        bits_per_channel: Some(8),
        color_mode: Some(ColorMode::RGB),
        image_data: Some(PixelData {
            data: composite,
            width,
            height,
        }),
        children: Some(vec![
            Layer {
                top: Some(0),
                left: Some(0),
                bottom: Some(height as i32),
                right: Some(width as i32),
                blend_mode: Some(BlendMode::Normal),
                opacity: Some(255.0),
                image_data: Some(PixelData {
                    data: background,
                    width,
                    height,
                }),
                additional_info: LayerAdditionalInfo {
                    name: Some("Background".to_string()),
                    id: Some(1),
                    ..Default::default()
                },
                ..Default::default()
            },
            Layer {
                top: Some(20),
                left: Some(18),
                bottom: Some(100),
                right: Some(138),
                blend_mode: Some(BlendMode::Normal),
                opacity: Some(255.0),
                image_data: Some(PixelData {
                    data: red_box,
                    width: 120,
                    height: 80,
                }),
                additional_info: LayerAdditionalInfo {
                    name: Some("Red Card".to_string()),
                    id: Some(2),
                    ..Default::default()
                },
                ..Default::default()
            },
            Layer {
                top: Some(34),
                left: Some(72),
                bottom: Some(104),
                right: Some(142),
                blend_mode: Some(BlendMode::Normal),
                opacity: Some(180.0),
                image_data: Some(PixelData {
                    data: blue_box,
                    width: 70,
                    height: 70,
                }),
                additional_info: LayerAdditionalInfo {
                    name: Some("Blue Square".to_string()),
                    id: Some(3),
                    ..Default::default()
                },
                ..Default::default()
            },
        ]),
        additional_info: LayerAdditionalInfo {
            name: Some("Writer Smoke Test".to_string()),
            ..Default::default()
        },
        ..Default::default()
    };

    let bytes = write_psd(&psd, &WriteOptions::default())?;
    fs::write(&output, bytes)?;
    println!("{output}");
    Ok(())
}

fn solid_rgba(width: usize, height: usize, color: [u8; 4]) -> Vec<u8> {
    let mut data = Vec::with_capacity(width * height * 4);
    for _ in 0..width * height {
        data.extend_from_slice(&color);
    }
    data
}

fn compose_preview(width: usize, height: usize) -> Vec<u8> {
    let mut data = solid_rgba(width, height, [250, 244, 232, 255]);

    for y in 20..100usize {
        for x in 18..138usize {
            let idx = (y * width + x) * 4;
            data[idx..idx + 4].copy_from_slice(&[220, 48, 48, 255]);
        }
    }

    for y in 34..104usize {
        for x in 72..142usize {
            let idx = (y * width + x) * 4;
            let bg = [data[idx], data[idx + 1], data[idx + 2], data[idx + 3]];
            let fg = [40u8, 110u8, 230u8, 180u8];
            let alpha = fg[3] as u16;
            let inv = 255u16 - alpha;
            data[idx] = ((fg[0] as u16 * alpha + bg[0] as u16 * inv) / 255) as u8;
            data[idx + 1] = ((fg[1] as u16 * alpha + bg[1] as u16 * inv) / 255) as u8;
            data[idx + 2] = ((fg[2] as u16 * alpha + bg[2] as u16 * inv) / 255) as u8;
            data[idx + 3] = 255;
        }
    }

    data
}

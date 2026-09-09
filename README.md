# psd-great

A Rust library for reading and writing Adobe Photoshop PSD and PSB files.

`psd-great` aims to be the most feature-rich and complete PSD/PSB reading and writing crate in Rust. It began as a port of the TypeScript [ag-psd](https://github.com/Agamnentzar/ag-psd) parser/writer and has since grown into a broader typed Rust API for document structure, layers, tagged blocks, resources, text, effects, masks, paths, and smart-object metadata.

[![License: MIT](https://img.shields.io/badge/License-MIT-yellow.svg)](https://opensource.org/licenses/MIT)

## Installation

This crate is not currently published on crates.io.

Use it as a Git dependency:

```toml
[dependencies]
psd-great = { git = "git@github.com:ioma8/psd-great.git" }
```

## Quick Start

```rust
use psd_great::*;
use std::fs::File;

fn main() -> Result<()> {
    let file = File::open("input.psd")?;
    let mut psd = read_psd(file, ReadOptions::default())?;

    println!("{}x{}", psd.width, psd.height);

    if let Some(ref mut layers) = psd.children {
        for layer in layers {
            // Opacity is 0..1; Photoshop's 0..255 byte value is scaled.
            layer.opacity = Some(128.0 / 255.0);
        }
    }

    let bytes = write_psd(&psd, &WriteOptions::default())?;
    std::fs::write("output.psd", bytes)?;
    Ok(())
}
```

## Highlights

- PSD and PSB read/write
- Typed public API centered on `Psd`, `Layer`, `ReadOptions`, and `WriteOptions`
- Layer trees, groups, masks, blend modes, layer effects, and many tagged blocks
- Text structures, engine data, style runs, paragraph runs, and document resources
- Paths, slices, thumbnails, XMP, linked files, placed layers, and smart-object metadata
- ABR, ASE, and CSH parsing helpers

## Current Status

| Area | Status | Notes |
|---|---|---|
| PSD/PSB structure | Broad support | Reader and writer cover the main parser/writer surface |
| Layers, masks, effects | Broad support | Includes vector masks, many tagged blocks, and effect structures |
| Text layers | Partial | Rich text structures are supported, but not every Photoshop text workflow is exhaustively validated |
| Color modes | Partial | Reads RGB/Grayscale/CMYK/Indexed composites (Indexed via palette lookup). Bitmap composite decoding is read-only and not yet implemented; composite writing from an RGBA preview is supported for RGB and Grayscale only, and rejected for CMYK/Indexed/Bitmap rather than emitting mismatched channels. Synthesizing new non-RGB layers from RGBA image data returns an error |
| 16/32-bit depth | Partial | Structural support exists, but high-bit-depth layer and composite pixel roundtrips are not yet fully validated end to end |
| Smart objects / linked data | Partial | Typed structures exist, but not every Photoshop workflow is exhaustively covered |

## Limits

- This crate is a PSD/PSB parser and writer, not a Photoshop compositor or rasterizer.
- Editing text, vector data, masks, or effects does not automatically redraw layer pixels or the composite image.
- Some non-RGB modes such as `Multichannel`, `Duotone`, and `Lab` are preserved structurally, but composite-image decoding and write-side color handling are still incomplete in some paths.
- Higher bit depths are only partially supported.
- Newer Photoshop features, animation/timeline workflows, smart filters, and 3D-related data have significant typed coverage, but not every workflow has exhaustive interoperability validation.

## Examples

```bash
cargo run --example read_psd path/to/file.psd
cargo run --example modify_psd path/to/input.psd output.psd
cargo run --example extract_layers path/to/file.psd
cargo run --example create_psd
```

## Layout

- `api` - public document model types
- `format` - PSD/PSB wire-format sections
- `io` - high-level readers and writers
- `support` - internal helpers
- `formats` - ABR, ASE, and CSH support

Generate API docs locally with:

```bash
cargo doc --open
```

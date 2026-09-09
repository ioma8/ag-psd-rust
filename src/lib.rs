//! # psd-great
//!
//! Rust library for reading and writing Adobe Photoshop PSD files.
//!
//! This codebase began as a Rust port of the TypeScript ag-psd library and has
//! since grown into a broader PSD/PSB implementation with expanded feature
//! coverage and typed data structures for working with Photoshop files.
//!
//! ## Module Groups
//!
//! - `api` - Public PSD document model types
//! - `format` - PSD/PSB wire-format sections and transforms
//! - `io` - High-level readers and writers
//! - `support` - Shared low-level helpers
//! - `formats` - Additional Adobe/Photoshop-adjacent file formats

pub mod api;
pub mod format;
pub mod formats;
pub mod io;
pub mod support;

pub use api::{adjustments, effects, layer, psd, text, types};
pub use format::{additional_info, document_resource_postprocess, image_resources};
pub use formats::{abr, ase, csh};
pub use io::{reader, writer};
pub use support::{compression, descriptor, engine_data, error, helpers, jpeg, utf8};

pub mod effects_helpers;

// Re-export commonly used types at the crate root
pub use additional_info::LayerAdditionalInfo;
pub use additional_info::{PlacedLayer, SectionDivider, VectorStroke};
pub use adjustments::AdjustmentLayer;
pub use compression::{compress_rle, compress_zip, decompress_rle, decompress_zip};
pub use effects::{
    ColorStop, EffectContour, EffectPattern, LayerEffectBevel, LayerEffectGradientOverlay,
    LayerEffectInnerGlow, LayerEffectPatternOverlay, LayerEffectSatin, LayerEffectShadow,
    LayerEffectSolidFill, LayerEffectStroke, LayerEffectsInfo, LayerEffectsOuterGlow, OpacityStop,
};
pub use error::{PsdError, Result};
pub use helpers::{from_blend_mode, has_alpha, to_blend_mode};
pub use image_resources::ImageResources;
pub use layer::{
    BezierKnot, BezierPath, Layer, LayerMaskData, LayerVectorMask, LinkedFile, PatternInfo,
    VectorContent,
};
pub use psd::{
    Annotation, DocumentSlices, GlobalLayerMaskInfo, GuideInfo, Psd, ReadOptions, WriteOptions,
};
pub use psd::{ColorSampler, ColorSamplerPosition, DisplayInfo};
pub use reader::{read_psd, PsdReader};
pub use text::{
    Font, LayerTextData, ParagraphStyle, ParagraphStyleRun, TextStyle, TextStyleRun, Warp,
};
pub use types::{
    AntiAlias, BlendMode, BooleanOperation, ChannelID, Color, ColorMode, Compression, DisplayUnit,
    Fraction, GradientStyle, Grayscale, GuideDirection, InterpolationMethod, Justification,
    LayerColor, LayerCompCapturedInfo, LinkedFileDataKind, Orientation, PixelData, Point,
    PsdIntCode, PsdStringCode, PsdU16Code, PsdU32Code, RenderingIntent, SectionDividerType,
    SliceAlignment, SliceOrigin, SliceSourceType, SliceType, Units, UnitsValue, WarpStyle, CMYK,
    FRGB, RGB, RGBA,
};
pub use writer::{write_psd, PsdWriter};

// Re-export additional format types
pub use abr::{read_abr, Abr, Brush, BrushDynamics, BrushShape, SampleInfo};
pub use ase::{read_ase, write_ase, Ase, AseColor, AseColorOrGroup, AseColorType, AseGroup};
pub use csh::{read_csh, Csh, CustomShape};
pub use effects_helpers::{read_effects, write_effects};
pub use engine_data::{parse_engine_data, serialize_engine_data, EngineValue};
pub use jpeg::{decode_jpeg, decode_jpeg_raw};
pub use utf8::{decode_string, encode_string, string_length_in_bytes};

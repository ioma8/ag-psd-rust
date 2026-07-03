use crate::api::effects::LayerEffectsInfo;
use crate::api::types::*;

/// Layer mask data
#[derive(Debug, Clone, PartialEq, Default)]
pub struct LayerMaskData {
    pub top: Option<i32>,
    pub left: Option<i32>,
    pub bottom: Option<i32>,
    pub right: Option<i32>,
    pub default_color: Option<u8>,
    pub disabled: Option<bool>,
    pub position_relative_to_layer: Option<bool>,
    pub from_vector_data: Option<bool>,
    pub user_mask_density: Option<f64>,
    pub user_mask_feather: Option<f64>,
    pub vector_mask_density: Option<f64>,
    pub vector_mask_feather: Option<f64>,
    pub real_flags_byte: Option<u8>,
    pub real_default_color: Option<u8>,
    pub real_top: Option<i32>,
    pub real_left: Option<i32>,
    pub real_bottom: Option<i32>,
    pub real_right: Option<i32>,
    pub image_data: Option<PixelData>,
}

/// Pattern info
#[derive(Debug, Clone, PartialEq)]
pub struct PatternInfo {
    pub name: String,
    pub id: String,
    pub x: f64,
    pub y: f64,
    pub bounds: PatternBounds,
    pub data: Vec<u8>,
}

/// Pattern bounds
#[derive(Debug, Clone, PartialEq)]
pub struct PatternBounds {
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
}

/// Bezier knot
#[derive(Debug, Clone, PartialEq)]
pub struct BezierKnot {
    pub linked: bool,
    pub points: Vec<f64>,
}

/// Bezier path
#[derive(Debug, Clone, PartialEq)]
pub struct BezierPath {
    pub open: bool,
    pub operation: Option<BooleanOperation>,
    pub knots: Vec<BezierKnot>,
    pub fill_rule: PsdStringCode,
}

/// Extra pattern info
#[derive(Debug, Clone, PartialEq)]
pub struct ExtraPatternInfo {
    pub linked: Option<bool>,
    pub phase: Option<Point>,
}

/// Vector content types
#[derive(Debug, Clone, PartialEq)]
pub enum VectorContent {
    Color {
        content_type: String,
        color: Color,
    },
    SolidGradient {
        name: String,
        gradient_type: String,
        smoothness: Option<f64>,
        style: Option<GradientStyle>,
        scale: Option<f64>,
        angle: Option<f64>,
        dither: Option<bool>,
        reverse: Option<bool>,
        align: Option<bool>,
        offset: Option<Point>,
    },
    Pattern {
        name: String,
        id: String,
        content_type: String,
        linked: Option<bool>,
        phase: Option<Point>,
    },
}

/// Brightness adjustment
#[derive(Debug, Clone, PartialEq)]
pub struct BrightnessAdjustment {
    pub adjustment_type: String,
    pub brightness: Option<f64>,
    pub contrast: Option<f64>,
    pub mean_value: Option<f64>,
    pub use_legacy: Option<bool>,
    pub lab_color_only: Option<bool>,
    pub auto: Option<bool>,
}

/// Levels adjustment channel
#[derive(Debug, Clone, PartialEq)]
pub struct LevelsAdjustmentChannel {
    pub shadow_input: f64,
    pub highlight_input: f64,
    pub shadow_output: f64,
    pub highlight_output: f64,
    pub midtone_input: f64,
}

/// Preset info
#[derive(Debug, Clone, PartialEq)]
pub struct PresetInfo {
    pub preset_kind: Option<i32>,
    pub preset_file_name: Option<String>,
}

/// Levels adjustment
#[derive(Debug, Clone, PartialEq)]
pub struct LevelsAdjustment {
    pub adjustment_type: String,
    pub rgb: Option<LevelsAdjustmentChannel>,
    pub red: Option<LevelsAdjustmentChannel>,
    pub green: Option<LevelsAdjustmentChannel>,
    pub blue: Option<LevelsAdjustmentChannel>,
    pub preset: Option<PresetInfo>,
}

/// Curves adjustment channel
pub type CurvesAdjustmentChannel = Vec<CurvePoint>;

/// Curve point
#[derive(Debug, Clone, PartialEq)]
pub struct CurvePoint {
    pub input: f64,
    pub output: f64,
}

/// Curves adjustment
#[derive(Debug, Clone, PartialEq)]
pub struct CurvesAdjustment {
    pub adjustment_type: String,
    pub rgb: Option<CurvesAdjustmentChannel>,
    pub red: Option<CurvesAdjustmentChannel>,
    pub green: Option<CurvesAdjustmentChannel>,
    pub blue: Option<CurvesAdjustmentChannel>,
    pub preset: Option<PresetInfo>,
}

/// Exposure adjustment
#[derive(Debug, Clone, PartialEq)]
pub struct ExposureAdjustment {
    pub adjustment_type: String,
    pub exposure: Option<f64>,
    pub offset: Option<f64>,
    pub gamma: Option<f64>,
    pub preset: Option<PresetInfo>,
}

/// Vibrance adjustment
#[derive(Debug, Clone, PartialEq)]
pub struct VibranceAdjustment {
    pub adjustment_type: String,
    pub vibrance: Option<f64>,
    pub saturation: Option<f64>,
}

/// Hue saturation adjustment channel
#[derive(Debug, Clone, PartialEq)]
pub struct HueSaturationAdjustmentChannel {
    pub a: f64,
    pub b: f64,
    pub c: f64,
    pub d: f64,
    pub hue: f64,
    pub saturation: f64,
    pub lightness: f64,
}

/// Hue saturation adjustment
#[derive(Debug, Clone, PartialEq)]
pub struct HueSaturationAdjustment {
    pub adjustment_type: String,
    pub master: Option<HueSaturationAdjustmentChannel>,
    pub reds: Option<HueSaturationAdjustmentChannel>,
    pub yellows: Option<HueSaturationAdjustmentChannel>,
    pub greens: Option<HueSaturationAdjustmentChannel>,
    pub cyans: Option<HueSaturationAdjustmentChannel>,
    pub blues: Option<HueSaturationAdjustmentChannel>,
    pub magentas: Option<HueSaturationAdjustmentChannel>,
    pub preset: Option<PresetInfo>,
}

/// Color balance values
#[derive(Debug, Clone, PartialEq)]
pub struct ColorBalanceValues {
    pub cyan_red: f64,
    pub magenta_green: f64,
    pub yellow_blue: f64,
}

/// Color balance adjustment
#[derive(Debug, Clone, PartialEq)]
pub struct ColorBalanceAdjustment {
    pub adjustment_type: String,
    pub shadows: Option<ColorBalanceValues>,
    pub midtones: Option<ColorBalanceValues>,
    pub highlights: Option<ColorBalanceValues>,
    pub preserve_luminosity: Option<bool>,
}

/// Black and white adjustment
#[derive(Debug, Clone, PartialEq)]
pub struct BlackAndWhiteAdjustment {
    pub adjustment_type: String,
    pub reds: Option<f64>,
    pub yellows: Option<f64>,
    pub greens: Option<f64>,
    pub cyans: Option<f64>,
    pub blues: Option<f64>,
    pub magentas: Option<f64>,
    pub use_tint: Option<bool>,
    pub tint_color: Option<Color>,
    pub preset: Option<PresetInfo>,
}

/// Photo filter adjustment
#[derive(Debug, Clone, PartialEq)]
pub struct PhotoFilterAdjustment {
    pub adjustment_type: String,
    pub color: Option<Color>,
    pub density: Option<f64>,
    pub preserve_luminosity: Option<bool>,
}

/// Channel mixer channel
#[derive(Debug, Clone, PartialEq)]
pub struct ChannelMixerChannel {
    pub red: f64,
    pub green: f64,
    pub blue: f64,
    pub constant: f64,
}

/// Channel mixer adjustment
#[derive(Debug, Clone, PartialEq)]
pub struct ChannelMixerAdjustment {
    pub adjustment_type: String,
    pub monochrome: Option<bool>,
    pub red: Option<ChannelMixerChannel>,
    pub green: Option<ChannelMixerChannel>,
    pub blue: Option<ChannelMixerChannel>,
    pub gray: Option<ChannelMixerChannel>,
    pub preset: Option<PresetInfo>,
}

/// Color lookup adjustment
#[derive(Debug, Clone, PartialEq)]
pub struct ColorLookupAdjustment {
    pub adjustment_type: String,
    pub lookup_type: Option<String>,
    pub name: Option<String>,
    pub dither: Option<bool>,
    pub profile: Option<Vec<u8>>,
    pub lut_format: Option<String>,
    pub data_order: Option<String>,
    pub table_order: Option<String>,
    pub lut3d_file_data: Option<Vec<u8>>,
    pub lut3d_file_name: Option<String>,
}

/// Invert adjustment
#[derive(Debug, Clone, PartialEq)]
pub struct InvertAdjustment {
    pub adjustment_type: String,
}

/// Posterize adjustment
#[derive(Debug, Clone, PartialEq)]
pub struct PosterizeAdjustment {
    pub adjustment_type: String,
    pub levels: Option<i32>,
}

/// Threshold adjustment
#[derive(Debug, Clone, PartialEq)]
pub struct ThresholdAdjustment {
    pub adjustment_type: String,
    pub level: Option<f64>,
}

/// Gradient map adjustment
#[derive(Debug, Clone, PartialEq)]
pub struct GradientMapAdjustment {
    pub adjustment_type: String,
    pub name: Option<String>,
    pub gradient_type: String,
    pub dither: Option<bool>,
    pub reverse: Option<bool>,
    pub method: Option<InterpolationMethod>,
    pub smoothness: Option<f64>,
    pub color_stops: Option<Vec<crate::api::effects::ColorStop>>,
    pub opacity_stops: Option<Vec<crate::api::effects::OpacityStop>>,
    pub roughness: Option<f64>,
    pub color_model: Option<String>,
    pub random_seed: Option<i32>,
    pub restrict_colors: Option<bool>,
    pub add_transparency: Option<bool>,
    pub min: Option<Vec<f64>>,
    pub max: Option<Vec<f64>>,
}

/// Selective color adjustment
#[derive(Debug, Clone, PartialEq)]
pub struct SelectiveColorAdjustment {
    pub adjustment_type: String,
    pub mode: Option<String>,
    pub reds: Option<CMYK>,
    pub yellows: Option<CMYK>,
    pub greens: Option<CMYK>,
    pub cyans: Option<CMYK>,
    pub blues: Option<CMYK>,
    pub magentas: Option<CMYK>,
    pub whites: Option<CMYK>,
    pub neutrals: Option<CMYK>,
    pub blacks: Option<CMYK>,
}

/// Adjustment layer types

/// Linked file
#[derive(Debug, Clone, PartialEq)]
pub struct LinkedFile {
    pub id: String,
    pub name: String,
    pub item_version: Option<u32>,
    pub data_kind: Option<LinkedFileDataKind>,
    pub file_type: Option<PsdStringCode>,
    pub creator: Option<PsdStringCode>,
    pub data: Option<Vec<u8>>,
    pub time: Option<String>,
    pub descriptor: Option<crate::support::descriptor::Descriptor>,
    pub child_document_id: Option<String>,
    pub asset_mod_time: Option<f64>,
    pub asset_locked_state: Option<u8>,
    pub linked_file: Option<LinkedFileInfo>,
    pub open_descriptor: Option<crate::support::descriptor::Descriptor>,
}

/// Linked file info
#[derive(Debug, Clone, PartialEq)]
pub struct LinkedFileInfo {
    pub file_size: u64,
    pub name: String,
    pub full_path: String,
    pub original_path: String,
    pub relative_path: String,
}

/// Key descriptor item
#[derive(Debug, Clone, PartialEq)]
pub struct KeyDescriptorItem {
    pub key_shape_invalidated: Option<bool>,
    pub key_origin_index: Option<PsdIntCode>,
    pub key_origin_type: Option<PsdIntCode>,
    pub key_origin_resolution: Option<f64>,
    pub key_origin_rrect_radii: Option<RRectRadii>,
    pub key_origin_shape_bounding_box: Option<crate::api::text::UnitsBounds>,
    pub key_origin_box_corners: Option<Vec<Point>>,
    pub transform: Option<Vec<f64>>,
}

/// Rounded rectangle radii
#[derive(Debug, Clone, PartialEq)]
pub struct RRectRadii {
    pub top_right: UnitsValue,
    pub top_left: UnitsValue,
    pub bottom_left: UnitsValue,
    pub bottom_right: UnitsValue,
}

/// Layer vector mask
#[derive(Debug, Clone, PartialEq)]
pub struct LayerVectorMask {
    pub invert: Option<bool>,
    pub not_link: Option<bool>,
    pub disable: Option<bool>,
    pub fill_starts_with_all_pixels: Option<bool>,
    pub clipboard: Option<ClipboardData>,
    pub paths: Vec<BezierPath>,
}

/// Clipboard data
#[derive(Debug, Clone, PartialEq)]
pub struct ClipboardData {
    pub top: f64,
    pub left: f64,
    pub bottom: f64,
    pub right: f64,
    pub resolution: f64,
}

/// Animation frame
#[derive(Debug, Clone, PartialEq)]
pub struct AnimationFrame {
    pub frames: Vec<i32>,
    pub enable: Option<bool>,
    pub offset: Option<Point>,
    pub reference_point: Option<Point>,
    pub opacity: Option<f64>,
    pub effects: Option<LayerEffectsInfo>,
}

/// Timeline key types
#[derive(Debug, Clone, PartialEq)]
pub enum TimelineKey {
    Opacity {
        interpolation: TimelineKeyInterpolation,
        time: Fraction,
        selected: Option<bool>,
        value: f64,
    },
    Position {
        interpolation: TimelineKeyInterpolation,
        time: Fraction,
        selected: Option<bool>,
        x: f64,
        y: f64,
    },
    Transform {
        interpolation: TimelineKeyInterpolation,
        time: Fraction,
        selected: Option<bool>,
        scale: Point,
        skew: Point,
        rotation: f64,
        translation: Point,
    },
    Style {
        interpolation: TimelineKeyInterpolation,
        time: Fraction,
        selected: Option<bool>,
        style: Option<LayerEffectsInfo>,
    },
    GlobalLighting {
        interpolation: TimelineKeyInterpolation,
        time: Fraction,
        selected: Option<bool>,
        global_angle: f64,
        global_altitude: f64,
    },
}

/// Timeline track
#[derive(Debug, Clone, PartialEq)]
pub struct TimelineTrack {
    pub track_type: TimelineTrackType,
    pub enabled: Option<bool>,
    pub effect_params: Option<EffectParams>,
    pub keys: Vec<TimelineKey>,
}

/// Effect params
#[derive(Debug, Clone, PartialEq)]
pub struct EffectParams {
    pub keys: Vec<TimelineKey>,
    pub fill_canvas: bool,
    pub zoom_origin: f64,
}

/// Animation timeline data (animation model, distinct from the resource-level Timeline)
#[derive(Debug, Clone, PartialEq)]
pub struct AnimationTimeline {
    pub start: Fraction,
    pub duration: Fraction,
    pub in_time: Fraction,
    pub out_time: Fraction,
    pub auto_scope: bool,
    pub audio_level: f64,
    pub tracks: Option<Vec<TimelineTrack>>,
}

/// Protected flags
#[derive(Debug, Clone, PartialEq)]
pub struct Protected {
    pub transparency: Option<bool>,
    pub composite: Option<bool>,
    pub position: Option<bool>,
    pub artboards: Option<bool>,
}

/// Filter mask
#[derive(Debug, Clone, PartialEq)]
pub struct FilterMask {
    pub color_space: Color,
    pub opacity: f64,
}

/// Compositor used info
#[derive(Debug, Clone, PartialEq)]
pub struct CompositorUsed {
    pub version: Option<Version>,
    pub photoshop_version: Option<Version>,
    pub description: String,
    pub reason: String,
    pub engine: String,
    pub enable_comp_core: Option<String>,
    pub enable_comp_core_gpu: Option<String>,
    pub enable_comp_core_threads: Option<String>,
    pub comp_core_support: Option<String>,
    pub comp_core_gpu_support: Option<String>,
}

/// Version
#[derive(Debug, Clone, PartialEq)]
pub struct Version {
    pub major: i32,
    pub minor: i32,
    pub fix: i32,
}

/// Artboard info
#[derive(Debug, Clone, PartialEq)]
pub struct Artboard {
    pub rect: Rect,
    pub guide_indices: Option<Vec<i32>>,
    pub preset_name: Option<String>,
    pub color: Option<Color>,
    pub background_type: Option<PsdIntCode>,
}

/// Rectangle
#[derive(Debug, Clone, PartialEq)]
pub struct Rect {
    pub top: f64,
    pub left: f64,
    pub bottom: f64,
    pub right: f64,
}

/// Animation frame flags
#[derive(Debug, Clone, PartialEq)]
pub struct AnimationFrameFlags {
    pub propagate_frame_one: Option<bool>,
    pub unify_layer_position: Option<bool>,
    pub unify_layer_style: Option<bool>,
    pub unify_layer_visibility: Option<bool>,
}

/// Filter effects mask channel
#[derive(Debug, Clone, PartialEq)]
pub struct FilterEffectsMaskChannel {
    pub compression_mode: i32,
    pub data: Vec<u8>,
}

/// Filter effects mask extra
#[derive(Debug, Clone, PartialEq)]
pub struct FilterEffectsMaskExtra {
    pub top: f64,
    pub left: f64,
    pub bottom: f64,
    pub right: f64,
    pub compression_mode: i32,
    pub data: Vec<u8>,
}

/// Filter effects mask
#[derive(Debug, Clone, PartialEq)]
pub struct FilterEffectsMask {
    pub id: String,
    pub top: f64,
    pub left: f64,
    pub bottom: f64,
    pub right: f64,
    pub depth: i32,
    pub channels: Vec<Option<FilterEffectsMaskChannel>>,
    pub extra: Option<FilterEffectsMaskExtra>,
}

/// Comps settings
#[derive(Debug, Clone, PartialEq)]
pub struct CompsSettings {
    pub enabled: Option<bool>,
    pub comp_list: Vec<i32>,
    pub offset: Option<Point>,
    pub effects_reference_point: Option<Point>,
}

/// Comps
#[derive(Debug, Clone, PartialEq)]
pub struct Comps {
    pub original_effects_reference_point: Option<Point>,
    pub settings: Vec<CompsSettings>,
}

/// User mask
#[derive(Debug, Clone, PartialEq)]
pub struct UserMask {
    pub color_space: Color,
    pub opacity: f64,
}

/// Blending ranges
#[derive(Debug, Clone, PartialEq)]
pub struct BlendingRanges {
    pub composite_gray_blend_source: Vec<f64>,
    pub composite_graph_blend_destination_range: Vec<f64>,
    pub ranges: Vec<BlendRange>,
}

/// Blend range
#[derive(Debug, Clone, PartialEq)]
pub struct BlendRange {
    pub source_range: Vec<f64>,
    pub dest_range: Vec<f64>,
}

/// Pixel source
#[derive(Debug, Clone, PartialEq)]
pub struct PixelSource {
    pub source_type: PsdStringCode,
    pub origin: Point,
    pub interpretation: Interpretation,
    pub frame_reader: FrameReader,
    pub show_altered_video: bool,
}

/// Interpretation
#[derive(Debug, Clone, PartialEq)]
pub struct Interpretation {
    pub interpret_alpha: PsdStringCode,
    pub profile: Vec<u8>,
}

/// Frame reader
#[derive(Debug, Clone, PartialEq)]
pub struct FrameReader {
    pub reader_type: String,
    pub link: FrameReaderLink,
    pub media_descriptor: String,
}

/// Frame reader link
#[derive(Debug, Clone, PartialEq)]
pub struct FrameReaderLink {
    pub name: String,
    pub full_path: String,
    pub original_path: String,
    pub relative_path: String,
    pub alias: String,
}

/// Layer raw data channel
#[derive(Debug, Clone, PartialEq)]
pub struct LayerRawDataChannel {
    pub id: ChannelID,
    pub compression: Compression,
    pub data: Option<Vec<u8>>,
}

/// Layer raw data
#[derive(Debug, Clone, PartialEq)]
pub struct LayerRawData {
    pub color_mode: ColorMode,
    pub bits_per_channel: u8,
    pub channels: Vec<LayerRawDataChannel>,
    pub large: bool,
}

/// Vector origination
#[derive(Debug, Clone, PartialEq)]
pub struct VectorOrigination {
    pub key_descriptor_list: Vec<KeyDescriptorItem>,
}

/// Layer structure
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Layer {
    pub top: Option<i32>,
    pub left: Option<i32>,
    pub bottom: Option<i32>,
    pub right: Option<i32>,
    pub blend_mode: Option<BlendMode>,
    pub opacity: Option<f64>,
    pub transparency_protected: Option<bool>,
    pub effects_open: Option<bool>,
    pub hidden: Option<bool>,
    pub clipping: Option<u16>,
    pub resource_visible: Option<bool>,
    pub image_data: Option<PixelData>,
    pub raw_data: Option<LayerRawData>,
    pub children: Option<Vec<Layer>>,
    pub opened: Option<bool>,
    pub link_group: Option<i32>,
    pub link_group_enabled: Option<bool>,
    pub additional_info: crate::format::additional_info::LayerAdditionalInfo,
    pub blending_ranges_data: Option<LayerBlendingRangesData>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct LayerBlendingRangePair {
    pub src_black: u8,
    pub src_white: u8,
    pub dst_black: u8,
    pub dst_white: u8,
}

#[derive(Debug, Clone, PartialEq)]
pub struct LayerBlendingRangesData {
    pub composite_gray: Option<LayerBlendingRangePair>,
    pub channels: Vec<LayerBlendingRangePair>,
}

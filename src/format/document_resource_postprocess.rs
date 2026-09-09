//! Document resource postprocess/prewrite mirroring TS document-postprocess.ts and resource-postprocess.ts.
//!
//! Maps typed PSD document fields (e.g., variable_sets, data_sets, display_info,
//! color_samplers) to/from the low-level image resource storage (XML strings, typed resources,
//! descriptor resources).

use crate::api::layer::Layer;
use crate::api::psd::Psd;
use crate::support::error::{PsdError, Result};
use quick_xml::escape::unescape;
use quick_xml::events::{BytesEnd, BytesStart, BytesText, Event};
use quick_xml::XmlVersion;
use quick_xml::{Reader, Writer};
use std::io::Cursor;

/// Collect the canonical flat-layer slot paths of a tree in flatten_layers
/// order: a plain layer contributes one slot; a group contributes its two
/// section-divider marker slots (bounding first, folder last) surrounding the
/// slots of its descendants. Resource 1072 carries one value per slot.
fn collect_visibility_slots(children: &[Layer], out: &mut Vec<Vec<usize>>) {
    fn walk(children: &[Layer], prefix: &mut Vec<usize>, out: &mut Vec<Vec<usize>>) {
        for (index, child) in children.iter().enumerate() {
            prefix.push(index);
            if let Some(ref grand_children) = child.children {
                out.push(prefix.clone()); // bounding section-divider marker
                walk(grand_children, prefix, out);
                out.push(prefix.clone()); // folder marker
            } else {
                out.push(prefix.clone());
            }
            prefix.pop();
        }
    }
    let mut prefix = Vec::new();
    walk(children, &mut prefix, out);
}

fn layer_at_mut<'a>(children: &'a mut [Layer], path: &[usize]) -> Option<&'a mut Layer> {
    let index = *path.first()?;
    let child = children.get_mut(index)?;
    if path.len() == 1 {
        Some(child)
    } else {
        layer_at_mut(child.children.as_deref_mut()?, &path[1..])
    }
}

fn layer_at<'a>(children: &'a [Layer], path: &[usize]) -> Option<&'a Layer> {
    let index = *path.first()?;
    let child = children.get(index)?;
    if path.len() == 1 {
        Some(child)
    } else {
        layer_at(child.children.as_ref()?, &path[1..])
    }
}

fn slot_visibility_value(children: &[Layer], slot: &[usize]) -> bool {
    layer_at(children, slot)
        .map(|l| l.resource_visible != Some(false))
        .unwrap_or(true)
}

/// Apply read-side document resource postprocess: map low-level ImageResources fields
/// onto typed Psd fields.
pub fn apply_document_postprocess(psd: &mut Psd) -> Result<()> {
    if let Some(ref resources) = psd.image_resources {
        if let Some(group_ids) = resources.layer_group_ids.as_ref() {
            psd.layer_group_ids = Some(group_ids.clone());
        }

        // Map resource visibility from resource 1072. Entries correspond to
        // the canonical flat layer order (the same sequence produced by
        // flatten_layers), where a group occupies two section-divider marker
        // slots (bounding and folder) around its descendants.
        if let (Some(visibility), Some(layers)) = (
            resources.resource_visibility_typed.as_ref(),
            psd.children.as_mut(),
        ) {
            let mut slots = Vec::new();
            collect_visibility_slots(layers, &mut slots);
            for (slot, value) in slots.iter().zip(visibility.values.iter()) {
                if let Some(node) = layer_at_mut(layers, slot) {
                    node.resource_visible = Some(*value);
                }
            }
        }

        // Map variables XML → typed variable_sets
        if let Some(ref xml) = resources.variables {
            psd.variable_sets = Some(parse_variables_xml(xml));
        }

        // Map data sets XML → typed data_sets
        if let Some(ref xml) = resources.data_sets {
            psd.data_sets = Some(parse_data_sets_xml(xml));
        }

        // Map descriptor resources 1065, 1074, 1075
        if let Some(desc) = resources.descriptor_resources.get(&1065) {
            psd.descriptor_1065 = Some(desc.clone());
        }
        if let Some(desc) = resources.descriptor_resources.get(&1074) {
            psd.descriptor_1074 = Some(desc.clone());
        }
        if let Some(desc) = resources.descriptor_resources.get(&1075) {
            psd.descriptor_1075 = Some(desc.clone());
        }

        if let Some(samplers) = resources.color_samplers_typed.as_ref() {
            psd.color_samplers = Some(samplers.samplers.clone());
        }

        if let Some(info) = resources.display_info_typed.as_ref() {
            psd.display_info = Some(crate::api::psd::DisplayInfo {
                h_res_unit: info.h_res_unit,
                v_res_unit: info.v_res_unit,
                width_unit: info.width_unit,
                height_unit: info.height_unit,
            });
        }

        if let Some(name) = resources.clipping_path_name.as_ref() {
            psd.clipping_path_name = Some(name.clone());
        }

        // Map resolution (resource 1005) → psd.resolution
        if let Some(ref res_info) = resources.resolution_info {
            psd.resolution = Some(res_info.horizontal_res);
        }

        // Map guides (resource 1032) → psd.guides
        if let Some(ref grid_info) = resources.grid_and_guides {
            if !grid_info.guides.is_empty() {
                let guides: Vec<crate::api::psd::GuideInfo> = grid_info
                    .guides
                    .iter()
                    .map(|g| crate::api::psd::GuideInfo {
                        location: g.location,
                        direction: match g.direction {
                            crate::format::image_resources::GuideDirection::Vertical => {
                                crate::api::types::GuideDirection::Vertical
                            }
                            crate::format::image_resources::GuideDirection::Horizontal => {
                                crate::api::types::GuideDirection::Horizontal
                            }
                        },
                    })
                    .collect();
                psd.guides = Some(guides);
            }
        }

        // Map alpha channel names (resource 1045) → psd.alpha_channel_names
        if let Some(ref names) = resources.alpha_unicode_names {
            if !names.is_empty() {
                psd.alpha_channel_names = Some(names.clone());
            }
        }

        // Map selected layer IDs (resource 1069) → psd.selected_layer_ids
        if let Some(ref ids) = resources.layer_selection_ids {
            psd.selected_layer_ids = Some(ids.clone());
        }

        // Map ICC profile (resource 1039) → psd.icc_profile
        if let Some(ref profile) = resources.icc_profile {
            psd.icc_profile = Some(profile.clone());
        }

        // Map slices (resource 1050) → psd.slices
        if let Some(ref slices) = resources.slices {
            psd.slices = Some(match &slices.descriptor {
                Some(descriptor) => crate::api::psd::DocumentSlices::Descriptor {
                    version: slices.version,
                    descriptor: descriptor.clone(),
                },
                None => crate::api::psd::DocumentSlices::Legacy(slices.clone()),
            });
        }

        // Map path selection descriptor (resource 3000) → psd.path_selection_descriptor
        if let Some(desc) = resources.descriptor_resources.get(&3000) {
            psd.path_selection_descriptor = Some(desc.clone());
        }
    }
    Ok(())
}

/// Apply write-side document resource prewrite: map typed Psd fields back
/// onto low-level ImageResources fields before serialization.
pub fn apply_document_prewrite(psd: &mut Psd) -> Result<()> {
    let resources = psd.image_resources.get_or_insert_with(Default::default);

    if let Some(group_ids) = psd.layer_group_ids.as_ref() {
        resources.layer_group_ids = Some(group_ids.clone());
    }

    // Map resource visibility from layers → resource 1072, aligned with the
    // canonical flat layer order (flatten_layers): one entry per layer record
    // including the two section-divider marker slots each group occupies. The
    // resource is always rebuilt after an edit - even when every value became
    // true again - so a stale original payload cannot survive a change.
    if let Some(layers) = psd.children.as_ref() {
        let mut slots = Vec::new();
        collect_visibility_slots(layers, &mut slots);
        let values: Vec<bool> = slots
            .iter()
            .map(|slot| slot_visibility_value(layers, slot))
            .collect();
        let existed = resources.resource_visibility_typed.is_some()
            || resources
                .original_blocks
                .iter()
                .any(|raw| raw.resource_id == 1072);
        if existed || values.iter().any(|value| !*value) {
            resources.resource_visibility_typed =
                Some(crate::format::image_resources::ResourceVisibility { values });
        }
    }

    // Map typed variable_sets → variables XML
    if let Some(variable_sets) = psd.variable_sets.as_ref() {
        resources.variables = Some(build_variables_xml(variable_sets));
    }

    // Map typed data_sets → data sets XML
    if let Some(data_sets) = psd.data_sets.as_ref() {
        resources.data_sets = Some(build_data_sets_xml(data_sets));
    }

    // Map descriptor resources 1065, 1074, 1075
    if let Some(ref desc) = psd.descriptor_1065 {
        resources.descriptor_resources.insert(1065, desc.clone());
    }
    if let Some(ref desc) = psd.descriptor_1074 {
        resources.descriptor_resources.insert(1074, desc.clone());
    }
    if let Some(ref desc) = psd.descriptor_1075 {
        resources.descriptor_resources.insert(1075, desc.clone());
    }

    if let Some(points) = psd.color_samplers.as_ref() {
        let version =
            crate::format::image_resources::infer_color_sampler_version(points)?.unwrap_or(2);
        resources.color_samplers_typed =
            Some(crate::format::image_resources::ColorSamplersResource {
                version,
                samplers: points.clone(),
            });
    }

    // Map typed display info → resource 1077. When the resource was loaded
    // and its raw payload is retained, keep that payload byte-for-byte; the
    // typed mirror is only written for newly constructed resources.
    if resources.display_info_raw.is_none() {
        if let Some(info) = psd.display_info.as_ref() {
            resources.display_info_typed =
                Some(crate::format::image_resources::DisplayInfoResource {
                    version: 1,
                    h_res_unit: info.h_res_unit,
                    v_res_unit: info.v_res_unit,
                    width_unit: info.width_unit,
                    height_unit: info.height_unit,
                });
        }
    }

    if let Some(name) = psd.clipping_path_name.as_ref() {
        resources.clipping_path_name = Some(name.clone());
    }

    // Map resolution → resource 1005. The full resource (units, unequal
    // axes) is preserved when unedited: the mirror is only rewritten when the
    // typed resolution differs from the stored horizontal resolution, i.e.
    // when the caller explicitly changed it.
    if let Some(dpi) = psd.resolution {
        let unchanged = resources
            .resolution_info
            .as_ref()
            .map(|info| info.horizontal_res == dpi)
            .unwrap_or(false);
        if !unchanged {
            resources.resolution_info = Some(crate::format::image_resources::ResolutionInfo {
                horizontal_res: dpi,
                horizontal_res_unit: crate::format::image_resources::ResolutionUnit::PixelsPerInch,
                width_unit: crate::format::image_resources::MeasurementUnit::Inches,
                vertical_res: dpi,
                vertical_res_unit: crate::format::image_resources::ResolutionUnit::PixelsPerInch,
                height_unit: crate::format::image_resources::MeasurementUnit::Inches,
            });
        }
    }

    // Map guides → resource 1032
    if let Some(ref guides) = psd.guides {
        let grid_guides = resources.grid_and_guides.get_or_insert_with(|| {
            crate::format::image_resources::GridAndGuides {
                grid: crate::format::image_resources::Grid {
                    horizontal: 0,
                    vertical: 0,
                },
                guides: Vec::new(),
            }
        });
        grid_guides.guides = guides
            .iter()
            .map(|g| {
                let dir = match g.direction {
                    crate::api::types::GuideDirection::Vertical => {
                        Ok(crate::format::image_resources::GuideDirection::Vertical)
                    }
                    crate::api::types::GuideDirection::Horizontal => {
                        Ok(crate::format::image_resources::GuideDirection::Horizontal)
                    }
                    crate::api::types::GuideDirection::Other(code) => {
                        Err(PsdError::InvalidFormat(format!(
                            "Guide direction code {:?} cannot be serialized to resource 1032",
                            std::str::from_utf8(&code).unwrap_or("????")
                        )))
                    }
                }?;
                Ok(crate::format::image_resources::Guide {
                    location: g.location,
                    direction: dir,
                })
            })
            .collect::<Result<Vec<_>>>()?;
    }

    // Map alpha_channel_names → resource 1045
    if let Some(ref names) = psd.alpha_channel_names {
        resources.alpha_unicode_names = Some(names.clone());
    }

    // Map selected_layer_ids → resource 1069
    if let Some(ref ids) = psd.selected_layer_ids {
        resources.layer_selection_ids = Some(ids.clone());
    }

    // Map icc_profile → resource 1039
    if let Some(ref profile) = psd.icc_profile {
        resources.icc_profile = Some(profile.clone());
    }

    // Map slices → resource 1050
    if let Some(ref slices) = psd.slices {
        resources.slices = Some(match slices {
            crate::api::psd::DocumentSlices::Legacy(slices) => slices.clone(),
            crate::api::psd::DocumentSlices::Descriptor {
                version,
                descriptor,
            } => crate::format::image_resources::Slices {
                version: *version,
                bounds: None,
                group_name: None,
                slices: Vec::new(),
                descriptor: Some(descriptor.clone()),
            },
        });
    }

    // Map path_selection_descriptor → resource 3000
    if let Some(ref desc) = psd.path_selection_descriptor {
        resources.descriptor_resources.insert(3000, desc.clone());
    }

    Ok(())
}

// ── XML helpers (porting TS document-postprocess.ts behavior) ────────────────

/// Parse variables XML into typed VariableSet vectors.
fn parse_variables_xml(xml: &str) -> Vec<crate::api::psd::VariableSet> {
    let mut variables = Vec::new();
    let mut reader = Reader::from_str(xml);
    reader.config_mut().trim_text(true);

    loop {
        match reader.read_event() {
            Ok(Event::Empty(e)) | Ok(Event::Start(e)) if e.name().as_ref() == b"variable" => {
                let mut var_name = None;
                let mut trait_name = None;
                let mut doc_ref = None;
                let mut placement_method = None;
                let mut align = None;
                let mut valign = None;
                let mut clip = None;

                for attr in e.attributes().with_checks(false).flatten() {
                    let key = attr.key.as_ref();
                    let value = attr
                        .decoded_and_normalized_value(XmlVersion::Implicit1_0, reader.decoder())
                        .ok()
                        .map(|v| v.into_owned());
                    match key {
                        b"varName" => var_name = value,
                        b"trait" => trait_name = value,
                        b"docRef" => doc_ref = value,
                        b"placementMethod" => placement_method = value,
                        b"align" => align = value,
                        b"valign" => valign = value,
                        b"clip" => clip = value,
                        _ => {}
                    }
                }

                variables.push(crate::api::psd::VariableSet {
                    var_name,
                    trait_name,
                    doc_ref,
                    placement_method,
                    align,
                    valign,
                    clip,
                });
            }
            Ok(Event::Eof) | Err(_) => break,
            _ => {}
        }
    }
    variables
}

/// Build variables XML from typed VariableSet vectors.
fn build_variables_xml(variables: &[crate::api::psd::VariableSet]) -> String {
    let mut writer = Writer::new(Cursor::new(Vec::new()));
    writer
        .write_event(Event::Start(BytesStart::new("variableSets")))
        .expect("write variableSets start");
    writer
        .write_event(Event::Start(BytesStart::new("variableSet")))
        .expect("write variableSet start");
    writer
        .write_event(Event::Start(BytesStart::new("variables")))
        .expect("write variables start");

    for v in variables {
        let mut tag = BytesStart::new("variable");
        if let Some(ref name) = v.var_name {
            tag.push_attribute(("varName", name.as_str()));
        }
        if let Some(ref trait_name) = v.trait_name {
            tag.push_attribute(("trait", trait_name.as_str()));
        }
        if let Some(ref doc_ref) = v.doc_ref {
            tag.push_attribute(("docRef", doc_ref.as_str()));
        }
        if let Some(ref pm) = v.placement_method {
            tag.push_attribute(("placementMethod", pm.as_str()));
        }
        if let Some(ref a) = v.align {
            tag.push_attribute(("align", a.as_str()));
        }
        if let Some(ref v) = v.valign {
            tag.push_attribute(("valign", v.as_str()));
        }
        if let Some(ref c) = v.clip {
            tag.push_attribute(("clip", c.as_str()));
        }
        writer
            .write_event(Event::Empty(tag))
            .expect("write variable entry");
    }
    writer
        .write_event(Event::End(BytesEnd::new("variables")))
        .expect("write variables end");
    writer
        .write_event(Event::End(BytesEnd::new("variableSet")))
        .expect("write variableSet end");
    writer
        .write_event(Event::End(BytesEnd::new("variableSets")))
        .expect("write variableSets end");

    String::from_utf8(writer.into_inner().into_inner()).expect("utf8 xml")
}

/// Parse data sets XML into typed table: first row is header, subsequent rows are values.
fn parse_data_sets_xml(xml: &str) -> Vec<Vec<String>> {
    let mut reader = Reader::from_str(xml);
    reader.config_mut().trim_text(true);
    let mut table = vec![Vec::new()];
    let mut current_cells: Vec<(String, String)> = Vec::new();
    let mut in_sample = false;

    loop {
        match reader.read_event() {
            Ok(Event::Start(e)) if e.name().as_ref() == b"sampleDataSet" => {
                in_sample = true;
                current_cells.clear();
            }
            Ok(Event::End(e)) if e.name().as_ref() == b"sampleDataSet" => {
                if !current_cells.is_empty() {
                    if table[0].is_empty() {
                        table[0] = current_cells.iter().map(|(name, _)| name.clone()).collect();
                    }
                    table.push(
                        current_cells
                            .iter()
                            .map(|(_, value)| value.clone())
                            .collect(),
                    );
                }
                in_sample = false;
            }
            Ok(Event::Start(e)) if in_sample => {
                let end_name = e.name().as_ref().to_vec();
                let tag_name = String::from_utf8_lossy(&end_name).to_string();
                let value = read_element_text(&mut reader, &end_name);
                current_cells.push((tag_name, value));
            }
            Ok(Event::Eof) | Err(_) => break,
            _ => {}
        }
    }
    table
}

/// Build data sets XML from typed table.
fn build_data_sets_xml(table: &[Vec<String>]) -> String {
    let mut writer = Writer::new(Cursor::new(Vec::new()));
    writer
        .write_event(Event::Start(BytesStart::new("sampleDataSets")))
        .expect("write sampleDataSets start");
    if let Some(header) = table.first() {
        for record in table.iter().skip(1) {
            writer
                .write_event(Event::Start(BytesStart::new("sampleDataSet")))
                .expect("write sampleDataSet start");
            for (index, tag_name) in header.iter().enumerate() {
                let value = record.get(index).cloned().unwrap_or_default();
                writer
                    .write_event(Event::Start(BytesStart::new(tag_name.as_str())))
                    .expect("write cell start");
                writer
                    .write_event(Event::Text(BytesText::new(&value)))
                    .expect("write cell text");
                writer
                    .write_event(Event::End(BytesEnd::new(tag_name.as_str())))
                    .expect("write cell end");
            }
            writer
                .write_event(Event::End(BytesEnd::new("sampleDataSet")))
                .expect("write sampleDataSet end");
        }
    }
    writer
        .write_event(Event::End(BytesEnd::new("sampleDataSets")))
        .expect("write sampleDataSets end");
    String::from_utf8(writer.into_inner().into_inner()).expect("utf8 xml")
}

fn decode_xml_text(text: BytesText<'_>) -> String {
    let decoded = text
        .decode()
        .map(|value| value.into_owned())
        .unwrap_or_default();
    unescape(&decoded)
        .map(|value| value.into_owned())
        .unwrap_or(decoded)
}

fn read_element_text(reader: &mut Reader<&[u8]>, end_name: &[u8]) -> String {
    let mut value = String::new();

    loop {
        match reader.read_event() {
            Ok(Event::Text(text)) => value.push_str(&decode_xml_text(text)),
            Ok(Event::GeneralRef(reference)) => {
                value.push_str(match reference.as_ref() {
                    b"amp" => "&",
                    b"lt" => "<",
                    b"gt" => ">",
                    b"quot" => "\"",
                    b"apos" => "'",
                    _ => "",
                });
            }
            Ok(Event::End(end)) if end.name().as_ref() == end_name => break,
            Ok(Event::Eof) | Err(_) => break,
            _ => {}
        }
    }

    value
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_variables_xml_decodes_entities() {
        let xml = r#"<variableSets><variableSet><variables><variable varName="A&amp;B" trait="x&lt;y" docRef="&quot;doc&quot;" clip="c&gt;d"/></variables></variableSet></variableSets>"#;

        let parsed = parse_variables_xml(xml);

        assert_eq!(parsed.len(), 1);
        assert_eq!(parsed[0].var_name.as_deref(), Some("A&B"));
        assert_eq!(parsed[0].trait_name.as_deref(), Some("x<y"));
        assert_eq!(parsed[0].doc_ref.as_deref(), Some("\"doc\""));
        assert_eq!(parsed[0].clip.as_deref(), Some("c>d"));
    }

    #[test]
    fn parse_data_sets_xml_decodes_entities() {
        let xml = r#"<sampleDataSets><sampleDataSet><name>A&amp;B</name><value>&lt;ok&gt;</value></sampleDataSet></sampleDataSets>"#;

        let parsed = parse_data_sets_xml(xml);

        assert_eq!(
            parsed,
            vec![
                vec!["name".to_string(), "value".to_string()],
                vec!["A&B".to_string(), "<ok>".to_string()],
            ]
        );
    }

    fn layer_named(name: &str) -> Layer {
        let mut layer = Layer::default();
        layer.additional_info.name = Some(name.to_string());
        layer
    }

    #[test]
    fn visibility_slot_count_matches_flatten_layers() {
        let mut group = layer_named("group");
        group.children = Some(vec![layer_named("inner"), layer_named("deeper")]);
        let mut psd = Psd::default();
        psd.children = Some(vec![group, layer_named("tail")]);

        let mut slots = Vec::new();
        collect_visibility_slots(psd.children.as_ref().unwrap(), &mut slots);
        let flat = crate::io::writer::flatten_layers(psd.children.as_ref());
        assert_eq!(slots.len(), flat.len());
        assert_eq!(slots.len(), 5); // group bounding + inner + deeper + group folder + tail
    }

    #[test]
    fn nested_visibility_read_maps_marker_aware_flat_order() {
        let mut group = layer_named("group");
        group.children = Some(vec![layer_named("inner")]);
        let mut psd = Psd::default();
        psd.children = Some(vec![group, layer_named("tail")]);
        // Slots: [group(bounding), inner, group(folder), tail]
        let mut resources = crate::format::image_resources::ImageResources::default();
        resources.resource_visibility_typed =
            Some(crate::format::image_resources::ResourceVisibility {
                values: vec![true, false, true, true],
            });
        psd.image_resources = Some(resources);

        crate::format::document_resource_postprocess::apply_document_postprocess(&mut psd).unwrap();

        let children = psd.children.as_ref().unwrap();
        assert_eq!(children[0].resource_visible, Some(true));
        assert_eq!(
            children[0].children.as_ref().unwrap()[0].resource_visible,
            Some(false)
        );
        assert_eq!(children[1].resource_visible, Some(true));
    }

    #[test]
    fn nested_visibility_write_keeps_flat_order_after_edit() {
        let mut group = layer_named("group");
        group.children = Some(vec![layer_named("inner")]);
        group.resource_visible = Some(true);
        let mut psd = Psd::default();
        psd.children = Some(vec![group, layer_named("tail")]);

        // Resource existed with a disabled descendant; flip that value to true
        // and confirm the resource is rebuilt (not left stale).
        let mut resources = crate::format::image_resources::ImageResources::default();
        resources.resource_visibility_typed =
            Some(crate::format::image_resources::ResourceVisibility {
                values: vec![true, false, true, true],
            });
        psd.image_resources = Some(resources);

        psd.children.as_mut().unwrap()[0].children.as_mut().unwrap()[0].resource_visible =
            Some(true);

        crate::format::document_resource_postprocess::apply_document_prewrite(&mut psd).unwrap();

        let rebuilt = psd
            .image_resources
            .as_ref()
            .unwrap()
            .resource_visibility_typed
            .as_ref()
            .expect("1072 must be rebuilt after the edit");
        assert_eq!(rebuilt.values, vec![true, true, true, true]);
    }

    #[test]
    fn visibility_write_uses_nested_slot_values() {
        let mut group = layer_named("group");
        group.resource_visible = Some(false);
        group.children = Some(vec![layer_named("inner")]);
        let mut psd = Psd::default();
        psd.children = Some(vec![group, layer_named("tail")]);

        crate::format::document_resource_postprocess::apply_document_prewrite(&mut psd).unwrap();

        let rebuilt = psd
            .image_resources
            .as_ref()
            .unwrap()
            .resource_visibility_typed
            .as_ref()
            .expect("a disabled group must be recorded");
        // [group(bounding)=false, inner=true, group(folder)=false, tail=true]
        assert_eq!(rebuilt.values, vec![false, true, false, true]);
    }
}

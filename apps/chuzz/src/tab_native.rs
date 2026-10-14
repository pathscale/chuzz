use std::collections::BTreeSet;
use std::sync::Arc;

use blitz_dom::{BaseDocument, NodeId, local_name, qual_name};
use zeroize::Zeroize;

use crate::tab_stash::{Bytes, FieldKind, FormLocator, FormState, PaintSnapshot, RestoreState, SleepingBudget, Text, PREVIEW_TARGET};

fn field_kind(document: &BaseDocument, id: NodeId) -> Option<FieldKind> {
    let element = document.get_node(id)?.element_data()?;
    match element.name.local.as_ref() {
        "textarea" => Some(FieldKind::Textarea),
        "select" if element.attr(local_name!("multiple")).is_none() => Some(FieldKind::Select),
        "input" => match element.attr(local_name!("type")).unwrap_or("text").to_ascii_lowercase().as_str() {
            "text" => Some(FieldKind::Text), "checkbox" => Some(FieldKind::Checkbox), "radio" => Some(FieldKind::Radio), _ => None,
        },
        _ => None,
    }
}

fn identity(document: &BaseDocument, id: NodeId) -> Option<(&str, &str, &str)> {
    let node = document.get_node(id)?;
    let mut scope = node.parent;
    while let Some(id) = scope {
        let ancestor = document.get_node(id)?;
        if ancestor.is_shadow_root() { return None; }
        scope = ancestor.parent;
    }
    let element = node.element_data()?;
    let field = element.attr(local_name!("id"))?;
    if field.is_empty() || document.tree().iter().filter(|(_, node)| node.attr(local_name!("id")) == Some(field)).count() != 1 { return None; }
    let mut parent = node.parent;
    let mut owner = element.attr(local_name!("form")).and_then(|name| document.get_element_by_id(name));
    while owner.is_none() {
        let Some(id) = parent else { break; };
        let node = document.get_node(id)?;
        if node.element_data().is_some_and(|element| element.name.local == local_name!("form")) { owner = Some(id); }
        parent = node.parent;
    }
    if let Some(form) = owner {
        let form = document.get_node(form)?.element_data()?;
        let form_id = form.attr(local_name!("id"))?;
        if form_id.is_empty() || document.tree().iter().filter(|(_, node)| node.attr(local_name!("id")) == Some(form_id)).count() != 1 { return None; }
        Some((form_id, field, form.attr(local_name!("action")).unwrap_or("")))
    } else { Some(("", field, "")) }
}

fn protected(value: &str) -> Option<Text> { Text::capture(value.encode_utf16(), 4096) }
fn equals(text: &Text, value: &str) -> bool { text.units().iter().copied().eq(value.encode_utf16()) }

pub fn capture_forms(document: &mut BaseDocument) -> Vec<FormState> {
    document.classify_stash();
    let mut forms = Vec::new();
    let mut bytes = 0;
    for (id, node) in document.tree().iter() {
        if document.stash_sensitive(id) || !node.flags.is_in_document() { continue; }
        let Some(kind) = field_kind(document, id) else { continue; };
        let Some((form, field, action)) = identity(document, id) else { continue; };
        let Some(element) = node.element_data() else { continue; };
        let value = if kind == FieldKind::Select {
            let mut value = document.select_value(id);
            let text = protected(&value);
            value.zeroize();
            text
        } else if let Some(input) = element.text_input_data() {
            Text::capture(input.editor.text().into_iter().flat_map(str::encode_utf16), 4096)
        } else { protected(element.attr(local_name!("value")).unwrap_or("")) };
        let (Some(form), Some(field), Some(action), Some(value)) = (protected(form), protected(field), protected(action), value) else { continue; };
        bytes += form.allocated_bytes() + field.allocated_bytes() + action.allocated_bytes() + value.allocated_bytes() + size_of::<FormState>();
        if bytes > 24 * 1024 || forms.len() >= 256 { break; }
        forms.push(FormState { locator: FormLocator { form, field, action, kind }, value,
            checked: element.checkbox_input_checked().unwrap_or(false), selection: None });
    }
    forms
}

pub fn restore_forms(document: &mut BaseDocument, state: &RestoreState, applied: &mut BTreeSet<NodeId>) {
    document.classify_stash();
    let ids: Vec<_> = document.tree().iter().filter_map(|(id, node)| (node.flags.is_in_document() && field_kind(document, id).is_some()).then_some(id)).collect();
    for id in ids {
        if applied.contains(&id) { continue; }
        if document.stash_sensitive(id) { applied.insert(id); continue; }
        let Some(kind) = field_kind(document, id) else { continue; };
        let Some((form, field, action)) = identity(document, id) else { continue; };
        let mut matching = state.forms().iter().filter(|saved| saved.locator.kind == kind
            && equals(&saved.locator.form, form) && equals(&saved.locator.field, field) && equals(&saved.locator.action, action));
        let Some(saved) = matching.next() else { continue; };
        if matching.next().is_some() { continue; }
        let mut value = String::from_utf16_lossy(saved.value.units());
        if kind == FieldKind::Select {
            if let Some(index) = document.select_options(id).iter().position(|option| document.option_value(*option) == value) { document.set_select_selected_index(id, index); }
        } else if matches!(kind, FieldKind::Checkbox | FieldKind::Radio) {
            if let Some(checked) = document.get_node_mut(id).and_then(|node| node.element_data_mut()).and_then(|el| el.checkbox_input_checked_mut()) { *checked = saved.checked; }
            document.snapshot_node(id);
        } else { document.mutate().set_attribute(id, qual_name!("value"), &value); }
        value.zeroize();
        applied.insert(id);
    }
}

pub fn capture_paint(document: &mut BaseDocument, budget: &Arc<SleepingBudget>) -> Option<Arc<PaintSnapshot>> {
    if document.stash_disabled() { return None; }
    let (view_width, view_height) = document.viewport().window_size;
    if view_width == 0 || view_height == 0 { return None; }
    let mut fit = (512.0 / view_width as f64).min(288.0 / view_height as f64).min(1.0);
    for _ in 0..4 {
        let width = (view_width as f64 * fit).floor().max(1.0) as u16;
        let height = (view_height as f64 * fit).floor().max(1.0) as u16;
        let scale = document.viewport().scale_f64() * fit;
        document.stash_paint = true;
        let pixels = anyrender::render_to_buffer::<anyrender_vello_cpu::VelloCpuImageRenderer, _>(|scene| {
            blitz_paint::paint_scene(scene, document, scale, width as u32, height as u32, 0, 0);
        }, width as u32, height as u32);
        document.stash_paint = false;
        let pixels = Bytes::new(pixels);
        let mut output = Bytes::new(Vec::with_capacity(PREVIEW_TARGET));
        let mut index = 0;
        while index < pixels.bytes().len() {
            let pixel = &pixels.bytes()[index..index + 4];
            let mut run = 1usize;
            while run < u16::MAX as usize && index + (run + 1) * 4 <= pixels.bytes().len() && &pixels.bytes()[index + run * 4..index + (run + 1) * 4] == pixel { run += 1; }
            if output.bytes().len() + 6 > PREVIEW_TARGET { break; }
            output.extend(&(run as u16).to_le_bytes()); output.extend(pixel);
            index += run * 4;
        }
        if index == pixels.bytes().len() {
            return PaintSnapshot::seal(budget, width, height, nagoya::now_ns(), output).ok();
        }
        fit *= 0.5;
    }
    None
}

/// One selected host image only. The sleeping record retains encoded bytes.
pub fn show_preview(ui: &mut BaseDocument, id: NodeId, paint: &PaintSnapshot) -> bool {
    let length = paint.width as usize * paint.height as usize * 4;
    let mut decoded = Bytes::new(Vec::with_capacity(length));
    for run in paint.encoded().as_chunks::<6>().0 {
        let count = u16::from_le_bytes([run[0], run[1]]) as usize;
        if decoded.bytes().len().saturating_add(count * 4) > length { return false; }
        for _ in 0..count { decoded.extend(&run[2..]); }
    }
    if decoded.bytes().len() != length { return false; }
    let Some(element) = ui.get_node_mut(id).and_then(|node| node.element_data_mut()) else { return false; };
    element.special_data = blitz_dom::node::SpecialElementData::Image(Box::new(blitz_dom::node::ImageData::Raster(blitz_dom::node::RasterImageData {
        width: paint.width as u32, height: paint.height as u32, data: peniko::Blob::new(Arc::new(decoded)),
    })));
    true
}

//! Unified list markers without changing authored source offsets or inline slots.
use std::borrow::Cow;

use rebook_publication::{Inline, TextBlock, TextBlockKind, TextRun};

pub(super) fn normalize(block: &TextBlock, unified: bool) -> Cow<'_, TextBlock> {
    let TextBlockKind::ListItem { .. } = block.kind else {
        return Cow::Borrowed(block);
    };
    let Some(count) = block.style.authored_list_marker.filter(|_| unified) else {
        return Cow::Borrowed(block);
    };
    let mut result = block.clone();
    // Images have no source text scalars. Replace only the leading marker image,
    // keeping its inline index so supplied source-span boundaries remain valid.
    let image_marker = result
        .content
        .iter()
        .position(|inline| match inline {
            Inline::Text(run) => !run.text.trim().is_empty(),
            _ => true,
        })
        .filter(|&index| matches!(result.content[index], Inline::Image(_)));
    if let Some(index) = image_marker {
        result.content[index] = Inline::Text(TextRun {
            text: String::new(),
            style: Default::default(),
            link: None,
        });
    }
    // The generated marker sits outside the source text. Keep one zero-width
    // scalar per authored prefix scalar, as with collapsed footnote markers.
    let mut remaining = count as usize;
    for inline in &mut result.content {
        if remaining == 0 {
            break;
        }
        let Inline::Text(run) = inline else {
            break;
        };
        let consumed = run.text.chars().count().min(remaining);
        let end = run
            .text
            .char_indices()
            .nth(consumed)
            .map_or(run.text.len(), |(i, _)| i);
        run.text.replace_range(..end, &"\u{2060}".repeat(consumed));
        remaining -= consumed;
    }
    result.style.authored_list_marker = None;
    if let TextBlockKind::ListItem { marker_visible, .. } = &mut result.kind {
        *marker_visible = true;
    }
    Cow::Owned(result)
}

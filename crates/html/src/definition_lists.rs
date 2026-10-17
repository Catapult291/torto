//! Recover explicit glossary entries while retaining their original source node.
use super::*;

pub(super) fn recover_glossary(node: Node<'_, '_>, blocks: &mut Vec<Block>, start: usize) {
    let Some(scope) = node.ancestors().filter(Node::is_element).find(|n| {
        attribute_local(*n, "type").is_some_and(|s| s.split_whitespace().any(|s| s == "glossary"))
            || n.attribute("role") == Some("doc-glossary")
    }) else {
        return;
    };
    if blocks.len() != start + 1 {
        return;
    }
    let Some(Block::Text(block)) = blocks.get(start) else {
        return;
    };
    if block.content.iter().any(|i| !matches!(i, Inline::Text(_))) {
        return;
    }
    let value = block
        .content
        .iter()
        .filter_map(|i| {
            if let Inline::Text(t) = i {
                Some(t.text.as_str())
            } else {
                None
            }
        })
        .collect::<String>();
    let Some(colon) = value.find([':', '：']) else {
        return;
    };
    let split = value[..colon].chars().count() + 1;
    let after_colon = colon + value[colon..].chars().next().unwrap().len_utf8();
    if split > 80 || value[..colon].trim().is_empty() || value[after_colon..].trim().is_empty() {
        return;
    }
    let mut term = block.clone();
    let mut description = block.clone();
    term.content.clear();
    description.content.clear();
    let mut remaining = split;
    for inline in &block.content {
        let Inline::Text(run) = inline else {
            unreachable!()
        };
        let count = run.text.chars().count().min(remaining);
        let at = run
            .text
            .char_indices()
            .nth(count)
            .map_or(run.text.len(), |(b, _)| b);
        if at > 0 {
            let mut r = run.clone();
            r.text.truncate(at);
            term.content.push(Inline::Text(r));
        }
        if at < run.text.len() {
            let mut r = run.clone();
            r.text = run.text[at..].to_owned();
            description.content.push(Inline::Text(r));
        }
        remaining -= count;
    }
    term.kind = TextBlockKind::DefinitionTerm { depth: 0 };
    description.kind = TextBlockKind::DefinitionDescription { depth: 0 };
    for b in [&mut term, &mut description] {
        b.style.list_group = Some(scope.range().start as u64);
        b.style.paragraph_group = None;
        b.style.indent = 0.0;
    }
    if let Some(source) = &mut term.source {
        source.end.text_offset = source.start.text_offset + split as u64;
    }
    if let Some(source) = &mut description.source {
        source.start.text_offset += split as u64;
    }
    blocks[start] = Block::Text(term);
    blocks.push(Block::Text(description));
}

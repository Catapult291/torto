use super::*;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[ignore = "requires TORTO_REFLOW_TEST_PDF pointing to the local Designing Type PDF"]
    fn local_designing_type_visual_groups() {
        let path = std::env::var_os("TORTO_REFLOW_TEST_PDF").expect("local PDF path");
        let publication = crate::pdf::open(fs::read(path).unwrap(), "local-probe.pdf").unwrap();
        let tags = extract::structure_tags(&publication.pdf);
        for index in [12, 13, 14] {
            let page = extract::page(&publication.pdf, index, &tags);
            let (stored, crops) = build(
                &page,
                index,
                10.0,
                &HashSet::new(),
                &mut Statistics::default(),
            )
            .unwrap();
            assert_eq!(crops.len(), if index == 14 { 3 } else { 1 }, "{crops:?}");
            if index != 14 {
                let mut ids = crops[0].image_sources.clone();
                ids.sort_unstable();
                assert_eq!(
                    ids,
                    if index == 12 {
                        vec![0, 1]
                    } else {
                        vec![0, 1, 2]
                    }
                );
                let Block::Figure(figure) = &stored.section.blocks[0] else {
                    panic!("shared description must be editable outside composite");
                };
                assert_eq!(figure.captions.len(), 1);
                let caption = figure.captions[0]
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
                assert!(
                    caption.starts_with(if index == 12 {
                        "IBM Plex"
                    } else {
                        "Neue was commissioned"
                    }),
                    "{caption}"
                );
            }
            assert_eq!(stored.provenance.len(), page.glyphs.len());
            assert_eq!(
                stored
                    .provenance
                    .iter()
                    .map(|p| p.original_glyph)
                    .collect::<HashSet<_>>()
                    .len(),
                page.glyphs.len()
            );
            eprintln!(
                "VISUAL_GROUP_AUDIT page={} crops={} glyphs={} sources={:?}",
                index + 1,
                crops.len(),
                page.glyphs.len(),
                crops.iter().map(|c| &c.image_sources).collect::<Vec<_>>()
            );
        }
    }
}

fn subfigure_label(line: &Line) -> Option<char> {
    let text = line.text();
    let mut chars = text.trim().chars();
    let open = chars.next()?;
    let label = chars.next()?.to_ascii_lowercase();
    let close = chars.next()?;
    (matches!((open, close), ('(', ')') | ('（', '）'))
        && label.is_ascii_lowercase()
        && chars.next().is_none())
    .then_some(label)
}

/// Preserve a shared-caption figure's subimages and panel labels in one crop.
/// Work from page geometry before labels can become intervening prose blocks.
/// Uncertain groups stay separate; no text or unrelated region may be crossed.
pub(super) fn group_subfigures(regions: &mut Vec<Region>, prose: &[Line], body: f64) {
    let labels = prose
        .iter()
        .enumerate()
        .filter_map(|(index, line)| subfigure_label(line).map(|label| (index, label)))
        .collect::<Vec<_>>();
    if labels.len() < 2 || regions.is_empty() {
        return;
    }
    for (caption_index, caption) in prose
        .iter()
        .enumerate()
        .filter(|(_, line)| caption_prefix_len(&line.text()).is_some())
    {
        if caption.heading(body).is_some() || caption.size > body * 1.1 {
            continue;
        }
        let mut caption_lines = vec![caption.clone()];
        for next in &prose[caption_index + 1..] {
            if !figure_caption_continues(&caption_lines, next, body) {
                break;
            }
            caption_lines.push(next.clone());
        }
        let text = caption_lines
            .iter()
            .map(Line::text)
            .collect::<Vec<_>>()
            .join(" ")
            .to_ascii_lowercase();
        let mut members = Vec::new();
        let mut panels = Vec::new();
        let mut names = HashSet::new();
        for &(index, label) in &labels {
            let line = &prose[index];
            if line.rect.y1 >= caption.rect.y0
                || caption.baseline - line.baseline > body * 5.0
                || !(text.contains(&format!("({label})")) || text.contains(&format!("（{label}）")))
            {
                continue;
            }
            let image = regions
                .iter()
                .enumerate()
                .filter(|(_, region)| {
                    let rect = region.bounds;
                    let gap = line.rect.y0 - rect.y1;
                    gap >= -body * 0.2
                        && gap <= body * 2.5
                        && rect.x0 <= line.rect.center().x
                        && line.rect.center().x <= rect.x1
                        && rect.y1 <= caption.rect.y0
                        && caption.rect.y0 - rect.y1 <= body * 6.0
                })
                .min_by(|(_, a), (_, b)| {
                    (line.rect.y0 - a.bounds.y1)
                        .abs()
                        .total_cmp(&(line.rect.y0 - b.bounds.y1).abs())
                })
                .map(|(index, _)| index);
            if let Some(image) = image {
                if !names.insert(label) {
                    // Duplicate panel names indicate competing figure groups.
                    panels.clear();
                    break;
                }
                panels.push(index);
                if !members.contains(&image) {
                    members.push(image);
                }
            }
        }
        if panels.len() < 2 {
            continue;
        }
        let group = members
            .iter()
            .map(|&index| regions[index].bounds)
            .chain(panels.iter().map(|&index| prose[index].rect))
            .reduce(|a, b| a.union(b))
            .unwrap();
        // A short caption need not span every panel. Require overlap with the
        // complete figure instead of rejecting its rightmost subimage.
        if group.x0 >= caption.rect.x1 || caption.rect.x0 >= group.x1 {
            continue;
        }
        let crosses_region = regions.iter().enumerate().any(|(index, region)| {
            !members.contains(&index) && group.intersect(region.bounds).area() > 0.0
        });
        let crosses_text = prose.iter().enumerate().any(|(index, line)| {
            !panels.contains(&index)
                && line.glyphs.iter().any(|glyph| {
                    !glyph.text.chars().all(char::is_whitespace)
                        && (group.contains(glyph.bounds().center())
                            || (glyph.bounds().center().y > group.y1
                                && glyph.bounds().center().y < caption.rect.y0
                                && glyph.bounds().center().x >= group.x0
                                && glyph.bounds().center().x <= group.x1))
                        && !members
                            .iter()
                            .any(|&member| regions[member].bounds.contains(glyph.bounds().center()))
                })
        });
        if crosses_region || crosses_text {
            continue;
        }
        members.sort_unstable();
        let first = members[0];
        let mut combined = Region::graphic(group);
        for &index in members.iter().rev() {
            combined.merge(regions.remove(index));
        }
        regions.insert(first, combined);
    }
}

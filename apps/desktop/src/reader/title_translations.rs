//! TOC labels and their source headings share one accepted translation.
use super::*;

fn plain(text: &str) -> String {
    let mut output = String::new();
    let mut rest = text;
    while let Some(start) = rest.find('<') {
        output.push_str(&rest[..start]);
        let tail = &rest[start..];
        if let Some(end) = tail.find('>') {
            let tag = tail[1..end]
                .trim_start_matches('/')
                .split_whitespace()
                .next()
                .unwrap_or("");
            if tag.starts_with("t-")
                || ["strong", "em", "b", "i", "u", "sup", "sub", "small", "big"].contains(&tag)
            {
                rest = &tail[end + 1..];
                continue;
            }
        }
        output.push('<');
        rest = &tail[1..];
    }
    output.push_str(rest);
    output
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&amp;", "&")
        .trim()
        .to_owned()
}

pub(super) fn linked_rows(
    section: &Section,
    index: usize,
    rows: &[rebook_reader::TocViewItem],
) -> Vec<String> {
    let Some(Block::Text(heading)) = section.blocks.get(index) else {
        return Vec::new();
    };
    if heading
            .content
            .iter()
            .any(|i| !matches!(i, Inline::Text(r) if r.style.inline_role == InlineRole::Normal && r.style.link_role != LinkRole::FootnoteReference))
    {
        return Vec::new();
    }
    let Some(source) = &heading.source else {
        return Vec::new();
    };
    let candidates = rows
        .iter()
        .filter(|row| {
            row.target.as_ref().is_some_and(|target| {
                target.path() == section.href.path()
                    && target.fragment().is_none_or(|fragment| {
                        section.anchors.iter().any(|a| {
                            a.fragment == fragment
                                && a.source.spine == source.start.spine
                                && a.source.node == source.start.node
                        })
                    })
            })
        })
        .collect::<Vec<_>>();
    if candidates.is_empty() {
        return Vec::new();
    }
    if !matches!(heading.kind, TextBlockKind::Heading(_))
        && heading
            .content
            .iter()
            .filter_map(|i| {
                if let Inline::Text(t) = i {
                    Some(t.text.len())
                } else {
                    None
                }
            })
            .sum::<usize>()
            > 1024
    {
        return Vec::new();
    }
    let first_heading = matches!(heading.kind, TextBlockKind::Heading(_))
        .then(|| {
            section.blocks.iter().position(
                |b| matches!(b,Block::Text(t) if matches!(t.kind,TextBlockKind::Heading(_))),
            )
        })
        .flatten();
    let original = heading
        .content
        .iter()
        .filter_map(|i| {
            if let Inline::Text(r) = i {
                Some(r.text.as_str())
            } else {
                None
            }
        })
        .collect::<String>();
    let normalize = |s: &str| {
        s.trim_start_matches(|c: char| c.is_ascii_digit() || ".):- ".contains(c))
            .chars()
            .filter(|c| c.is_alphanumeric())
            .flat_map(char::to_lowercase)
            .collect::<String>()
    };
    let normalized = normalize(&original);
    candidates
        .into_iter()
        .filter(|row| {
            let text_match = !normalized.is_empty() && normalize(&row.label) == normalized;
            if !matches!(heading.kind, TextBlockKind::Heading(_)) && !text_match {
                return false;
            }
            row.target.as_ref().is_some_and(|target| {
                target.path() == section.href.path()
                    && match target.fragment() {
                        Some(fragment) => section.anchors.iter().any(|a| {
                            a.fragment == fragment
                                && a.source.spine == source.start.spine
                                && a.source.node == source.start.node
                        }),
                        None => first_heading == Some(index) || text_match,
                    }
            })
        })
        .map(|row| row.id.clone())
        .collect()
}

impl DesktopReader {
    fn title_rows(&self, section: usize, block: usize) -> Vec<String> {
        self.semantic_layout
            .title_links
            .get(&(section, block))
            .cloned()
            .unwrap_or_default()
    }

    pub(super) fn reuse_title_translation(
        &self,
        section: usize,
        input: &TranslationBlockInput,
    ) -> Option<BlockTranslation> {
        if input.segment_index.is_some() {
            return None;
        }
        let rows = self.title_rows(section, input.block_index);
        let text = rows
            .iter()
            .find_map(|id| self.translation.canonical_titles.get(id))?;
        Some(BlockTranslation {
            block_index: input.block_index,
            segment_index: None,
            text: text.clone(),
        })
    }

    pub(super) fn canonicalize_title_translation(
        &mut self,
        section: usize,
        translation: &mut BlockTranslation,
    ) {
        if translation.segment_index.is_some() {
            return;
        }
        let rows = self.title_rows(section, translation.block_index);
        if rows.is_empty() {
            return;
        }
        if let Some(accepted) = rows
            .iter()
            .find_map(|id| self.translation.canonical_titles.get(id))
        {
            translation.text.clone_from(accepted);
        }
        let label = plain(&translation.text);
        for id in rows {
            self.translation
                .canonical_titles
                .insert(id.clone(), translation.text.clone());
            self.translation.toc_labels.insert(id, label.clone());
        }
    }

    pub(super) fn accept_toc_titles(&mut self, ids: &[String], translations: &[BlockTranslation]) {
        for translation in translations {
            let Some(id) = ids.get(translation.block_index) else {
                continue;
            };
            if translation.text.trim().is_empty() {
                continue;
            }
            let accepted = self
                .translation
                .canonical_titles
                .entry(id.clone())
                .or_insert_with(|| translation.text.clone());
            self.translation
                .toc_labels
                .insert(id.clone(), plain(accepted));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn toc_target_links_to_its_heading_and_never_a_same_named_other_section() {
        let (mut reader, _, _) = super::super::semantic_layout::tests::fixture();
        let original = reader.semantic_source.original().parse_section(0).unwrap();
        let mut section = original.clone();
        let Block::Text(text) = &mut section.blocks[0] else {
            panic!()
        };
        text.kind = TextBlockKind::Heading(1);
        let row = rebook_reader::TocViewItem {
            id: "chapter".into(),
            label: "Heading".into(),
            target: Some(section.href.clone()),
            depth: 0,
            ancestors: vec![],
            has_children: false,
        };
        assert_eq!(
            linked_rows(&section, 0, std::slice::from_ref(&row)),
            vec!["chapter"]
        );
        reader
            .semantic_layout
            .originals
            .insert(0, Arc::new(section));
        reader.accept_toc_titles(
            &["chapter".into()],
            &[BlockTranslation {
                block_index: 0,
                segment_index: None,
                text: "统一标题".into(),
            }],
        );
        reader.accept_toc_titles(
            &["chapter".into()],
            &[BlockTranslation {
                block_index: 0,
                segment_index: None,
                text: "另一个翻译".into(),
            }],
        );
        assert_eq!(reader.translation.toc_labels["chapter"], "统一标题");
    }

    #[test]
    fn heading_and_toc_share_the_first_accepted_translation_in_both_completion_orders() {
        for toc_first in [false, true] {
            let (mut reader, mut section, _) =
                super::super::semantic_layout::tests::fixture_with_toc(vec![
                    rebook_publication::TocEntry {
                        label: "Heading".into(),
                        href: Some(PublicationUrl::parse("chapter.xhtml").unwrap()),
                        children: vec![],
                    },
                ]);
            let Block::Text(t) = &mut section.blocks[0] else {
                panic!()
            };
            t.kind = TextBlockKind::Heading(1);
            reader
                .semantic_layout
                .originals
                .insert(0, Arc::new(section));
            let id = reader.reader.toc_items()[0].id.clone();
            reader
                .semantic_layout
                .title_links
                .insert((0, 0), vec![id.clone()]);
            let toc = BlockTranslation {
                block_index: 0,
                segment_index: None,
                text: "目录译文".into(),
            };
            let mut heading = BlockTranslation {
                block_index: 0,
                segment_index: None,
                text: "<strong>标题译文</strong>".into(),
            };
            if toc_first {
                reader.accept_toc_titles(std::slice::from_ref(&id), std::slice::from_ref(&toc));
            }
            reader.canonicalize_title_translation(0, &mut heading);
            reader.accept_toc_titles(std::slice::from_ref(&id), std::slice::from_ref(&toc));
            let expected = if toc_first {
                "目录译文"
            } else {
                "标题译文"
            };
            assert_eq!(reader.translation.toc_labels[&id], expected);
            assert_eq!(plain(&heading.text), expected);
            let reused = reader
                .reuse_title_translation(
                    0,
                    &TranslationBlockInput {
                        block_index: 0,
                        segment_index: None,
                        text: "Heading".into(),
                    },
                )
                .unwrap();
            assert_eq!(reused.text, heading.text);
        }
    }
}

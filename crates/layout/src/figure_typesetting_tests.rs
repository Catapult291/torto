use super::*;

#[test]
fn unified_hyphenation_overrides_authored_prohibition_but_classic_keeps_it() {
    let mut text = block(TextBlockKind::Paragraph);
    let Inline::Text(run) = &mut text.content[0] else { unreachable!() };
    run.text = "Extraordinary typographical considerations improve international readability and representation.".into();
    run.style.language = rebook_publication::TextLanguage::EnglishGb;
    run.style.hyphenation = rebook_publication::HyphenationMode::None;
    let style = ReaderStyle { typesetting: ReaderTypesetting::unified(), ..Default::default() };
    let resolved = resolve_text_block(&text, &style, TextContext::Flow);
    let mut engine = LayoutEngine::new();
    let counts: Vec<_> = (100_u16..=240).step_by(10).map(|width| {
        engine.shape_text_with_min_width(&resolved, &style, f32::from(width), 40.0).hyphens.len()
    }).collect();
    assert!(counts.iter().sum::<usize>() > 0, "CSS must not disable unified dictionary breaks");
    let classic = resolve_text_block(&text, &ReaderStyle::default(), TextContext::Flow);
    assert_eq!(classic.content, text.content);
    // A link remains indivisible even though its CSS hyphenation policy resets.
    let mut linked = resolved.into_owned();
    for inline in &mut linked.content {
        if let Inline::Text(run) = inline { run.link = Some(rebook_publication::PublicationUrl::parse("chapter.xhtml#term").unwrap()); }
    }
    assert!((100_u16..=240).step_by(10).all(|width| engine.shape_text_with_min_width(&linked, &style, f32::from(width), 40.0).hyphens.is_empty()));
}

fn block(kind: TextBlockKind) -> TextBlock {
    TextBlock {
        kind,
        content: vec![Inline::Text(TextRun {
            text: "Text".into(),
            style: Default::default(),
            link: None,
        })],
        style: Default::default(),
        source: None,
    }
}

#[test]
fn unified_heading_ordinals_share_title_scale_but_keep_superscripts() {
    let style = ReaderStyle {
        typesetting: ReaderTypesetting::unified(),
        ..Default::default()
    };
    for kind in [TextBlockKind::Heading(1), TextBlockKind::HeadingOrdinal(2)] {
        let mut text = block(kind);
        text.content = [None, Some(0.62), Some(0.66)]
            .into_iter()
            .map(|inline_size_scale| {
                Inline::Text(TextRun {
                    text: "Title".into(),
                    style: TextStyle {
                        inline_size_scale,
                        size_scale: 3.5,
                        ..Default::default()
                    },
                    link: None,
                })
            })
            .collect();
        let resolved = resolve_text_block(&text, &style, TextContext::Flow);
        let sizes: Vec<_> = resolved
            .content
            .iter()
            .flat_map(Inline::text_runs)
            .map(|r| r.style.size_scale)
            .collect();
        assert!(sizes.windows(2).all(|p| p[0] == p[1]));
        let Inline::Text(run) = &mut text.content[2] else {
            unreachable!()
        };
        run.style.baseline = TextBaseline::Superscript;
        let resolved = resolve_text_block(&text, &style, TextContext::Flow);
        let sizes: Vec<_> = resolved
            .content
            .iter()
            .flat_map(Inline::text_runs)
            .map(|r| r.style.size_scale)
            .collect();
        assert_eq!(sizes[2], sizes[0] * 0.7);
        assert_eq!(
            resolve_text_block(&text, &ReaderStyle::default(), TextContext::Flow).content,
            text.content
        );
    }
}

#[test]
fn auto_indent_uses_display_language_and_custom_indent_keeps_user_value() {
    let mut style = ReaderStyle {
        writing_system: WritingSystem::Latin,
        typesetting: ReaderTypesetting::unified(),
        ..Default::default()
    };
    let mut paragraph = block(TextBlockKind::Paragraph);
    let Inline::Text(run) = &mut paragraph.content[0] else {
        unreachable!()
    };
    run.style.display_writing_system = Some(WritingSystem::Cjk);
    assert_eq!(
        resolve_text_block(&paragraph, &style, TextContext::Flow)
            .style
            .indent,
        style.typography.font_size * 2.0
    );
    style.writing_system = WritingSystem::Cjk;
    let Inline::Text(run) = &mut paragraph.content[0] else {
        unreachable!()
    };
    run.style.display_writing_system = Some(WritingSystem::Latin);
    assert_eq!(
        resolve_text_block(&paragraph, &style, TextContext::Flow)
            .style
            .indent,
        style.typography.font_size * 1.5
    );
    style.typesetting.paragraph_indent_mode = ParagraphIndentMode::Custom;
    style.typesetting.paragraph_indent_em = 0.75;
    assert_eq!(
        resolve_text_block(&paragraph, &style, TextContext::Flow)
            .style
            .indent,
        style.typography.font_size * 0.75
    );
    paragraph.kind = TextBlockKind::Caption;
    assert_eq!(
        resolve_text_block(&paragraph, &style, TextContext::Flow)
            .style
            .indent,
        0.0
    );
    paragraph.kind = TextBlockKind::Paragraph;
    paragraph.style.paragraph_group = Some(1);
    paragraph.style.authored_alignment = Some(TextAlignment::Center);
    assert_eq!(
        resolve_text_block(&paragraph, &style, TextContext::Flow)
            .style
            .indent,
        0.0
    );
}

#[test]
fn alphabetic_list_markers_preserve_authored_numbering() {
    let mut text = block(TextBlockKind::ListItem {
        ordered: true,
        ordinal: 27,
        depth: 1,
        marker_visible: true,
    });
    text.style.list_marker_format = Some(rebook_publication::ListMarkerFormat::LowerAlpha);
    assert_eq!(list_markers::prefix(&text).as_deref(), Some("aa.\u{00a0}"));
}

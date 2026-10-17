use super::*;
use rebook_layout::{LayoutEngine, LayoutViewport, PageItem, ReaderStyle, TypesettingMode};
use std::sync::atomic::AtomicUsize;

struct Fixture {
    book: Book,
    sections: Vec<Section>,
    parses: AtomicUsize,
    resources: AtomicUsize,
    revision: AtomicU64,
}
impl BookSource for Fixture {
    fn content_revision(&self) -> Option<u64> {
        Some(self.revision.load(Ordering::Acquire))
    }
    fn book(&self) -> &Book {
        &self.book
    }
    fn parse_section(&self, index: usize) -> Result<Section, PublicationError> {
        self.parses.fetch_add(1, Ordering::Relaxed);
        Ok(self.sections[index].clone())
    }
    fn resource(&self, _: &PublicationUrl) -> Result<Resource, PublicationError> {
        self.resources.fetch_add(1, Ordering::Relaxed);
        Err(PublicationError::InvalidPublication(
            "catalog must not read images".into(),
        ))
    }
}
fn fixture(html: &[&str]) -> Arc<Fixture> {
    let sections: Vec<_> = html
        .iter()
        .enumerate()
        .map(|(index, _)| SpineItem {
            id: SpineItemId::new(format!("s{index}")).unwrap(),
            href: PublicationUrl::parse(&format!("s{index}.xhtml")).unwrap(),
            media_type: "application/xhtml+xml".into(),
            linear: true,
            properties: vec![],
        })
        .collect();
    let parsed = html
        .iter()
        .zip(&sections)
        .map(|(html, section)| rebook_html::parse_section(html, section, |_| None).unwrap())
        .collect();
    Arc::new(Fixture {
        book: Book {
            id: PublicationId::new("media-links-tests").unwrap(),
            metadata: Metadata::default(),
            cover: None,
            sections,
            table_of_contents: vec![],
        },
        sections: parsed,
        parses: AtomicUsize::new(0),
        resources: AtomicUsize::new(0),
        revision: AtomicU64::new(0),
    })
}

#[test]
fn preview_candidates_use_validated_links_and_resolve_same_numbers_by_destination() {
    let fixture = fixture(&[
        "<html><body><p>See Table 2, <a href='s1.xhtml#remote'>Figure 1</a>, <a href='#local'>Figure 1</a>, Table 2 and Figure 99.</p><figure id='local'><div><p>Local diagram</p></div><figcaption>Figure 1 Local</figcaption></figure><table><caption>Table 2 Results</caption><tr><td>Data</td></tr></table></body></html>",
        "<html><body><figure id='remote'><div><p>Remote diagram</p></div><figcaption>Figure 1 Remote</figcaption></figure></body></html>",
    ]);
    let source = ready(&fixture);
    let text = fixture.sections[0].blocks[0].text_blocks().next().unwrap();
    let before = fixture.parses.load(Ordering::Relaxed);
    let candidates = source.preview_references(text.source.as_ref().unwrap(), &text.content);
    assert_eq!(candidates.iter().map(|(key,target,_)| (key.kind,key.number.as_str(),target.section)).collect::<Vec<_>>(),
        [(MediaKind::Table,"2",0),(MediaKind::Figure,"1",1),(MediaKind::Figure,"1",0),(MediaKind::Table,"2",0)]);
    assert!(candidates.iter().all(|(_,_,node)| node.is_some()));
    assert_eq!(fixture.parses.load(Ordering::Relaxed), before, "opening a group must not parse sections on the UI thread");
    assert_eq!(fixture.resources.load(Ordering::Relaxed), 0);
}

#[test]
fn changed_source_rejects_old_catalog_and_cancelled_build_does_no_work() {
    let fixture = fixture(&[
        "<html><body><p>See Figure 1.</p><figure><img src='a.png'/><figcaption>Figure 1 example</figcaption></figure></body></html>",
    ]);
    let source = ready(&fixture);
    let mut section = source.parse_section(0).unwrap();
    assert_eq!(
        linked(&mut section.blocks[0].text_blocks_mut().next().unwrap().content),
        "Figure 1"
    );
    fixture.revision.store(1, Ordering::Release);
    let mut section = source.parse_section(0).unwrap();
    assert_eq!(
        linked(&mut section.blocks[0].text_blocks_mut().next().unwrap().content),
        ""
    );
    let before = fixture.parses.load(Ordering::Relaxed);
    assert!(Catalog::build(fixture.as_ref(), || false).is_err());
    assert_eq!(fixture.parses.load(Ordering::Relaxed), before);
    assert_eq!(fixture.resources.load(Ordering::Relaxed), 0);
}

#[test]
fn bibliography_year_candidates_keep_their_original_citation_link() {
    let fixture = fixture(&[
        "<html><body><p>Smith <a href='#bib'>(2001)</a></p><p id='bib'>Smith, 2001. A book.</p></body></html>",
    ]);
    let source = ready(&fixture);
    let section = source.parse_section(0).unwrap();
    let mut content = section.blocks[0]
        .text_blocks()
        .next()
        .unwrap()
        .content
        .clone();
    assert_eq!(linked(&mut content), "");
    assert!(content.iter().flat_map(Inline::text_runs).any(|run| {
        run.text.contains("2001")
            && run
                .link
                .as_ref()
                .is_some_and(|href| href.fragment() == Some("bib"))
    }));
}
fn ready(fixture: &Arc<Fixture>) -> Source {
    let source = Source::new(fixture.clone(), fixture.clone());
    source.catalog.store(Some(Arc::new(
        Catalog::build(fixture.as_ref(), || true).unwrap(),
    )));
    source.ready.store(1, Ordering::Release);
    source
}
fn linked(content: &mut Vec<Inline>) -> String {
    media_references::style_links(
        content,
        Rgba {
            red: 40,
            green: 80,
            blue: 220,
            alpha: 255,
        },
    );
    content
        .iter()
        .flat_map(Inline::text_runs)
        .filter(|run| run.style.media_reference.is_some())
        .map(|run| run.text.as_str())
        .collect()
}

#[test]
fn catalog_validates_links_without_images_and_popup_does_no_parsing() {
    let fixture = fixture(&[
        "<html><body><p>See <a href='s1.xhtml#f'>Figure 1</a>, Figure 9 and Table 99.</p><figure><img src='a.png'/><figcaption>Figure 1 local</figcaption></figure></body></html>",
        "<html><body><figure id='f'><img src='b.png'/><figcaption>Figure 1 linked</figcaption></figure></body></html>",
    ]);
    let source = ready(&fixture);
    let original = fixture.sections[0].blocks[0].text_blocks().next().unwrap();
    let mut popup = vec![Inline::Text(TextRun {
        text: "See Figure 1, Figure 9 and Table 99.".into(),
        style: Default::default(),
        link: None,
    })];
    let before = fixture.parses.load(Ordering::Relaxed);
    source.prepare_media_references(&mut popup, original.source.as_ref());
    assert_eq!(linked(&mut popup), "Figure 1");
    assert_eq!(
        fixture.parses.load(Ordering::Relaxed),
        before,
        "popup validation is memory-only"
    );
    assert_eq!(fixture.resources.load(Ordering::Relaxed), 0);
    let reference = media_references::inline_references(&popup)[0].clone();
    assert_eq!(
        href_for(&popup, &reference).unwrap().unwrap().path(),
        "s1.xhtml",
        "popup and translated prose retain original link provenance"
    );
}

#[test]
fn broken_authored_targets_and_ambiguous_global_numbers_stay_plain() {
    let fixture = fixture(&[
        "<html><body><p><a href='s1.xhtml#missing'>Figure 1</a>; Figure 2; Figure 9</p><figure><img src='a.png'/><figcaption>Figure 1 local</figcaption></figure></body></html>",
        "<html><body><figure><img src='b.png'/><figcaption>Figure 2 first</figcaption></figure><figure><img src='c.png'/><figcaption>Figure 9 unique</figcaption></figure></body></html>",
        "<html><body><figure><img src='d.png'/><figcaption>Figure 2 second</figcaption></figure></body></html>",
    ]);
    let source = ready(&fixture);
    let section = source.parse_section(0).unwrap();
    let mut content = section.blocks[0]
        .text_blocks()
        .next()
        .unwrap()
        .content
        .clone();
    assert_eq!(linked(&mut content), "Figure 9");
    let catalog = source.catalog.load();
    assert_eq!(
        catalog
            .as_ref()
            .unwrap()
            .resolve(
                fixture.as_ref(),
                0,
                &media_references::references("Figure 9")[0].key,
                None
            )
            .unwrap()
            .section,
        1
    );
}

#[test]
fn only_media_citations_restore_original_when_unresolved_or_pending() {
    let fixture = fixture(&["<html><body><p>(see Figure 99), (Smith, 2020)</p></body></html>"]);
    let source = Source::new(fixture.clone(), fixture.clone());
    let origin = fixture.sections[0].blocks[0]
        .text_blocks()
        .next()
        .unwrap()
        .source
        .clone();
    let mut content = vec![
        Inline::Text(TextRun {
            text: "(see Figure 99)".into(),
            style: TextStyle {
                inline_citation: 1,
                ..Default::default()
            },
            link: None,
        }),
        Inline::Text(TextRun {
            text: ", ".into(),
            style: Default::default(),
            link: None,
        }),
        Inline::Text(TextRun {
            text: "(Smith, 2020)".into(),
            style: TextStyle {
                inline_citation: 2,
                ..Default::default()
            },
            link: None,
        }),
    ];
    source.prepare_media_references(&mut content, origin.as_ref());
    assert_eq!(linked(&mut content), "");
    assert!(
        content
            .iter()
            .flat_map(Inline::text_runs)
            .filter(|run| run.text.contains("Smith"))
            .all(|run| run.style.inline_citation == 2)
    );
    assert!(
        content
            .iter()
            .flat_map(Inline::text_runs)
            .filter(|run| !run.text.contains("Smith"))
            .all(|run| run.style.inline_citation == 0)
    );
    assert_eq!(
        content
            .iter()
            .flat_map(Inline::text_runs)
            .map(|run| run.text.as_str())
            .collect::<String>(),
        "(see Figure 99), (Smith, 2020)"
    );
}

#[test]
fn equations_use_document_scoped_anchors_and_explicit_click_brushes() {
    let fixture = fixture(&[
        "<html><body><p>See <a href='s1.xhtml#formula1'>Eq. (3.1)</a>, <a href='s1.xhtml#formula2'>(3.2)</a>; Eq. (8.1), Eq. (8.9), EQ3.</p><p id='formula1'><img src='x.gif'/><span>(8.1)</span></p></body></html>",
        "<html><body><p id='formula1'><img src='y.gif'/><span>(3.1)</span></p><p id='formula2'><img src='z.gif'/><span>(3.2)</span></p></body></html>",
    ]);
    let source = ready(&fixture);
    let section = source.parse_section(0).unwrap();
    let mut paragraph = section.blocks[0].text_blocks().next().unwrap().clone();
    let keys = media_references::inline_references(&paragraph.content);
    assert_eq!(
        keys.iter()
            .map(|r| r.key.number.as_str())
            .collect::<Vec<_>>(),
        ["3.1", "3.2", "8.1"]
    );
    let catalog = source.catalog.load();
    let href = href_for(&paragraph.content, &keys[0]).unwrap();
    assert_eq!(
        catalog
            .as_ref()
            .unwrap()
            .resolve(fixture.as_ref(), 0, &keys[0].key, href.as_ref())
            .unwrap()
            .section,
        1
    );
    let mut style = ReaderStyle::default();
    style.typesetting.mode = TypesettingMode::Unified;
    let layout = LayoutEngine::with_fonts(crate::fonts::embedded_reader_fonts().iter().cloned())
        .layout_blocks(
            &source,
            &[Block::Text(paragraph.clone())],
            LayoutViewport::new(900, 600).unwrap(),
            &style,
        )
        .unwrap();
    let hits: Vec<_> = layout
        .pages
        .iter()
        .flat_map(|page| &page.items)
        .filter_map(|item| {
            if let PageItem::Text(text) = item {
                Some(rebook_renderer::text_media_reference_rects(text))
            } else {
                None
            }
        })
        .flatten()
        .map(|(_, key)| key.number)
        .collect();
    assert!(
        hits.contains(&"3.1".into())
            && hits.contains(&"3.2".into())
            && hits.contains(&"8.1".into()),
        "{hits:?}"
    );
    assert!(!hits.contains(&"8.9".into()));
    // Book layout and arbitrary blue runs do not gain media interaction.
    style.typesetting.mode = TypesettingMode::Book;
    for run in paragraph.content.iter_mut().filter_map(|inline| {
        if let Inline::Text(run) = inline {
            Some(run)
        } else {
            None
        }
    }) {
        run.style.color = Rgba {
            red: 0,
            green: 0,
            blue: 255,
            alpha: 255,
        };
    }
    let layout = LayoutEngine::with_fonts(crate::fonts::embedded_reader_fonts().iter().cloned())
        .layout_blocks(
            &source,
            &[Block::Text(paragraph)],
            LayoutViewport::new(900, 600).unwrap(),
            &style,
        )
        .unwrap();
    assert!(layout.pages.iter().flat_map(|page| &page.items).all(|item| !matches!(item, PageItem::Text(text) if !rebook_renderer::text_media_reference_rects(text).is_empty())));
    assert_eq!(fixture.resources.load(Ordering::Relaxed), 0);
}

#[test]
#[ignore = "requires local Text Entry EPUB via TORTO_MEDIA_BOOK"]
fn local_text_entry_catalog_validates_all_authored_equations() {
    let publication =
        rebook_formats::open_file(std::env::var("TORTO_MEDIA_BOOK").unwrap()).unwrap();
    let original = publication.source();
    let catalog = Catalog::build(original.as_ref(), || true).unwrap();
    let equations = catalog
        .entries
        .iter()
        .filter(|entry| entry.target.key.kind == media_references::MediaKind::Equation)
        .count();
    assert_eq!(equations, 60);
    let mut links = 0;
    let mut rejected_years = 0;
    for ((section, _), references) in &catalog.links {
        for (key, href) in references
            .iter()
            .filter(|(key, _)| key.kind == media_references::MediaKind::Equation)
        {
            if catalog
                .resolve(original.as_ref(), *section, key, Some(href))
                .is_some()
            {
                links += 1;
            } else {
                // A linked parenthesized year is only a lexical candidate.
                // It must fail the structural equation/anchor validation.
                assert!(
                    key.number.trim_end_matches(|c: char| c.is_ascii_alphabetic())
                        .parse::<u16>()
                        .is_ok_and(|year| (1000..2100).contains(&year)),
                    "Unresolved equation {section}: {key:?} -> {href}"
                );
                rejected_years += 1;
            }
        }
    }
    assert_eq!(links, 28);
    println!(
        "Text Entry: {equations} structural equation targets; {links} authored equation links validated; {rejected_years} bibliography years rejected"
    );
}

use rebook_formats::pdf_reflow::{Manifest, ReflowSource};
use rebook_publication::{Block, BookSource, Inline};
use std::collections::HashSet;
use std::path::PathBuf;

#[test]
#[ignore = "requires TORTO_TEST_DESIGNING_TYPE_REFLOW pointing to a regenerated local cache"]
fn local_designing_type_composites_persist_captions_and_glyph_ownership() {
    let directory = PathBuf::from(std::env::var("TORTO_TEST_DESIGNING_TYPE_REFLOW").unwrap());
    let manifest: Manifest =
        serde_json::from_slice(&std::fs::read(directory.join("manifest.json")).unwrap()).unwrap();
    let source = ReflowSource::open(&directory, manifest.original.id.as_str()).unwrap();
    let mut identities = HashSet::new();
    let mut figure_pages = HashSet::new();
    for index in 0..source.book().sections.len() {
        let section = source.parse_section(index).unwrap();
        for slice in source.provenance(index).unwrap().iter() {
            assert!(
                identities.insert((slice.page, slice.original_glyph)),
                "glyph owned twice: page {} glyph {}",
                slice.page,
                slice.original_glyph
            );
        }
        for block in &section.blocks {
            let Block::Figure(figure) = block else {
                continue;
            };
            let Some(range) = &figure.source else {
                continue;
            };
            let page = if range.start.node.starts_with("p13-image") {
                13
            } else if range.start.node.starts_with("p14-image") {
                14
            } else {
                continue;
            };
            assert!(figure_pages.insert(page), "one composite per figure page");
            assert_eq!(figure.images.len(), 1);
            assert_eq!(figure.captions.len(), 1);
            let caption = figure.captions[0]
                .content
                .iter()
                .filter_map(|inline| {
                    if let Inline::Text(run) = inline {
                        Some(run.text.as_str())
                    } else {
                        None
                    }
                })
                .collect::<String>();
            assert!(
                caption.starts_with(if page == 13 {
                    "IBM Plex"
                } else {
                    "Neue was commissioned"
                }),
                "{caption}"
            );
            let pixels = image::load_from_memory(
                &std::fs::read(directory.join(figure.images[0].href.path())).unwrap(),
            )
            .unwrap();
            let ratio = pixels.width() as f64 / pixels.height() as f64;
            assert!(
                if page == 13 {
                    (2.9..3.2).contains(&ratio)
                } else {
                    (0.79..0.85).contains(&ratio)
                },
                "page {page}: {}x{}",
                pixels.width(),
                pixels.height()
            );
            println!(
                "PERSISTED_VISUAL_GROUP page={page} composite={}x{} caption_chars={}",
                pixels.width(),
                pixels.height(),
                caption.chars().count()
            );
        }
    }
    assert_eq!(figure_pages, HashSet::from([13, 14]));
    assert_eq!(
        identities.len(),
        manifest.stats.glyphs - manifest.stats.removed_chrome_glyphs
    );
    println!(
        "PERSISTED_GLYPH_AUDIT glyphs={} pages={} resources={}",
        identities.len(),
        manifest.stats.pages,
        manifest.resources.len()
    );
}

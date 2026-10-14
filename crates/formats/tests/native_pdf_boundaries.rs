use rebook_formats::pdf_reflow::{Manifest, ReflowSource};
use rebook_publication::{Block, BookSource, CONTINUATION_SECTION_PROPERTY, Inline};
use std::path::PathBuf;

fn text(block: &Block) -> String {
    match block {
        Block::Text(t) => t
            .content
            .iter()
            .filter_map(|i| match i {
                Inline::Text(r) => Some(r.text.as_str()),
                _ => None,
            })
            .collect(),
        _ => String::new(),
    }
}

#[test]
#[ignore = "requires TORTO_TEST_FOUR_THOUSAND_REFLOW pointing to a regenerated local cache"]
fn local_four_thousand_weeks_keeps_previous_chapter_tail_before_new_heading() {
    let directory = PathBuf::from(std::env::var("TORTO_TEST_FOUR_THOUSAND_REFLOW").unwrap());
    let manifest: Manifest =
        serde_json::from_slice(&std::fs::read(directory.join("manifest.json")).unwrap()).unwrap();
    let source = ReflowSource::open(&directory, manifest.original.id.as_str()).unwrap();
    let entry = manifest
        .book
        .table_of_contents
        .iter()
        .find(|entry| entry.label == "永恒之终结")
        .unwrap();
    let target = entry.href.as_ref().unwrap();
    let index = manifest
        .book
        .sections
        .iter()
        .position(|section| section.href.path() == target.path())
        .unwrap();
    let section = source.parse_section(index).unwrap();
    assert_eq!(text(&section.blocks[0]), "永恒之终结");
    assert!(matches!(&section.blocks[0], Block::Text(t) if t.kind.is_heading()));
    let anchor = section
        .anchors
        .iter()
        .find(|anchor| Some(anchor.fragment.as_str()) == target.fragment())
        .unwrap();
    let Block::Text(title) = &section.blocks[0] else {
        unreachable!()
    };
    assert_eq!(
        anchor.source.node,
        title.source.as_ref().unwrap().start.node
    );
    assert!(
        !section
            .blocks
            .iter()
            .any(|block| text(block).contains("直到已经看不见他们"))
    );
    let previous = source.parse_section(index - 1).unwrap();
    assert!(
        previous
            .blocks
            .iter()
            .any(|block| text(block).contains("直到已经看不见他们"))
    );
    let physical_page = &manifest.page_targets[15];
    assert_eq!(physical_page.path(), previous.href.path());
    assert!(
        !manifest.book.sections[index]
            .properties
            .iter()
            .any(|p| p == CONTINUATION_SECTION_PROPERTY)
    );
    let mapped = source.provenance(index - 1).unwrap();
    assert!(mapped.iter().any(|slice| slice.page == 16));
    println!(
        "CHAPTER_BOUNDARY_AUDIT section={index} heading_block=0 tail_owner={} physical_page=16",
        index - 1
    );
}

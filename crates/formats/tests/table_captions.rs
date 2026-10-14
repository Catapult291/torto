//! Local regression survey; copyrighted fixtures remain outside the repository.
use rebook_publication::{Block, TextBlockKind};

#[test]
#[ignore = "requires TORTO_TABLE_BOOK Handbook EPUB; source-only table regression"]
fn local_handbook_caption_and_source_belong_to_table_two() {
    let opened = rebook_formats::open_file(std::env::var_os("TORTO_TABLE_BOOK").unwrap()).unwrap();
    let source = opened.source();
    let index = source
        .book()
        .sections
        .iter()
        .position(|section| section.href.path().ends_with("c01.xhtml"))
        .unwrap();
    let section = source.parse_section(index).unwrap();
    let text = |block: &rebook_publication::TextBlock| {
        block
            .content
            .iter()
            .flat_map(|inline| inline.text_runs())
            .map(|run| run.text.as_str())
            .collect::<String>()
    };
    let table = section
        .blocks
        .iter()
        .find_map(|block| {
            let Block::Table(table) = block else {
                return None;
            };
            table
                .before
                .iter()
                .any(|caption| text(caption).contains("Objectives of HFE Discipline"))
                .then_some(table)
        })
        .expect("Table 2 title must be attached to its grid");
    assert_eq!(table.rows.len(), 18);
    assert_eq!(table.before.len(), 1);
    assert_eq!(table.before[0].kind, TextBlockKind::Caption);
    assert!(
        table
            .after
            .iter()
            .any(|note| text(note).contains("Chapanis (1995)"))
    );
    assert!(table.text_blocks().all(|block| block.source.is_some()));
    assert!(!section.blocks.iter().any(|block| matches!(block, Block::Text(t) if text(t).contains("Objectives of HFE Discipline") || text(t).contains("Source: Chapanis (1995)"))));
    println!(
        "TABLE_TWO_AUDIT before={} rows={} after={}",
        table.before.len(),
        table.rows.len(),
        table.after.len()
    );
}

#[test]
#[ignore = "requires TORTO_TABLE_BOOK pointing to a local EPUB"]
fn local_table_caption_survey() {
    let path = std::env::var_os("TORTO_TABLE_BOOK").expect("TORTO_TABLE_BOOK");
    let opened = rebook_formats::open_file(std::path::PathBuf::from(path)).unwrap();
    let source = opened.source();
    let mut grids = 0;
    let mut before = 0;
    let mut after = 0;
    let mut notes = 0;
    for index in 0..source.book().sections.len() {
        let section = source.parse_section(index).unwrap();
        for block in &section.blocks {
            if let Block::Table(table) = block {
                grids += 1;
                before += table.before.len();
                after += table.after.len();
                notes += table
                    .before
                    .iter()
                    .chain(&table.after)
                    .filter(|text| text.kind == TextBlockKind::Paragraph)
                    .count();
                assert!(table.text_blocks().all(|text| text.source.is_some()));
            }
        }
    }
    println!("grids={grids} before={before} after={after} notes={notes}");
    assert!(grids > 0);
    if std::env::var_os("TORTO_TABLE_EXPECT_NO_CAPTIONS").is_some() {
        assert_eq!(before + after, 0);
    } else {
        assert!(before + after > 0);
    }
}

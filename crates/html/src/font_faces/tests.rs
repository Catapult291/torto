use super::*;

fn parse(
    body: &str,
    css: &str,
    load: impl FnMut(&PublicationUrl) -> Option<FontTraits>,
) -> Vec<rebook_publication::TextRun> {
    let descriptor = SpineItem {
        id: SpineItemId::new("font-test").unwrap(),
        href: PublicationUrl::parse("Text/chapter.xhtml").unwrap(),
        media_type: "application/xhtml+xml".into(),
        linear: true,
        properties: vec![],
    };
    let section = parse_section_with_resource_metadata(&format!("<html><head><link rel='stylesheet' href='../Styles/book.css'/></head><body>{body}</body></html>"), &descriptor, |_| Some(css.into()), |_| false, |_| None, load, SectionParseHints::default()).unwrap();
    section
        .blocks
        .into_iter()
        .filter_map(|b| match b {
            Block::Text(t) => Some(t.content),
            _ => None,
        })
        .flatten()
        .filter_map(|i| match i {
            Inline::Text(t) => Some(t),
            _ => None,
        })
        .collect()
}

#[test]
fn dedicated_face_styles_work_with_normal_css_and_conservative_name_fallback() {
    let runs = parse(
        "<p style='font-family:Nexa-Bold; font-weight:normal'>Bold</p><p style='font-family:ClassicaPro-Demi'>Demi</p><p style='font-family:Nexa-BoldItalic; font-style:normal'>Both</p><p style='font-family:ClassicaPro-LightItalic'>Italic</p><p style='font-family:Blackberry'>Regular</p><p style='font-family:serif, Nexa-Bold'>Fallback</p>",
        "",
        |_| None,
    );
    assert!(runs[0].style.bold && !runs[0].style.italic);
    assert!(runs[1].style.bold);
    assert!(runs[2].style.bold && runs[2].style.italic);
    assert!(!runs[3].style.bold && runs[3].style.italic);
    assert!(!runs[4].style.bold && !runs[4].style.italic);
    assert!(!runs[5].style.bold);
    assert_eq!(
        FontTraits::from_style_name("Black Hole Sans"),
        FontTraits::default()
    );
}

#[test]
fn opaque_aliases_use_metadata_and_only_used_resources_are_read_once() {
    let css = "@font-face {font-family:opaque; src:url('../Fonts/Actual-Bold.otf');font-weight:normal} @font-face {font-family:unused;src:url('../Fonts/Unused.otf')} p {font-family:opaque;font-weight:normal}";
    let mut paths = Vec::new();
    let runs = parse(
        "<p>First</p><p>Second <span>Inherited</span></p>",
        css,
        |url| {
            paths.push(url.path().to_owned());
            Some(FontTraits {
                bold: true,
                italic: false,
            })
        },
    );
    assert_eq!(paths, vec!["Fonts/Actual-Bold.otf"]);
    assert!(runs.iter().all(|r| r.style.bold));
}

#[test]
fn shared_family_selects_regular_bold_and_italic_faces_separately() {
    let css = "@font-face {font-family:shared;src:url('../Fonts/Regular.otf')} @font-face {font-family:shared;src:url('../Fonts/Bold.otf');font-weight:700} @font-face {font-family:shared;src:url('../Fonts/Italic.otf');font-style:italic} body {font-family:shared}";
    let runs = parse(
        "<p>Regular <b>Bold</b> <i>Italic</i> <span style='font-weight:700;font-style:italic'>Both</span></p>",
        css,
        |url| {
            Some(FontTraits {
                bold: url.path().contains("Bold"),
                italic: url.path().contains("Italic"),
            })
        },
    );
    let find = |text| runs.iter().find(|r| r.text.trim() == text).unwrap().style;
    assert!(!find("Regular").bold && !find("Regular").italic);
    assert!(find("Bold").bold && !find("Bold").italic);
    assert!(!find("Italic").bold && find("Italic").italic);
    assert!(find("Both").bold && find("Both").italic);
}

#[test]
fn switching_back_to_regular_clears_face_traits_but_keeps_explicit_inherited_styles() {
    let css = "@font-face {font-family:face-bold;src:url('../Fonts/Bold.otf')} @font-face {font-family:face-regular;src:url('../Fonts/Regular.otf')} @font-face {font-family:face-italic;src:url('../Fonts/Italic.otf')}";
    let runs = parse(
        "<p style='font-family:face-bold'>Bold <span style='font-family:face-regular'>Regular</span> <span style='font-family:face-italic'>Italic</span> <span style='font-family:serif'>Generic</span></p><p style='font-family:face-bold;font-weight:bold'><span style='font-family:face-regular'>Inherited bold</span><span style='font-family:face-regular;font-weight:normal'>Reset</span></p>",
        css,
        |url| {
            Some(FontTraits {
                bold: url.path().contains("Bold"),
                italic: url.path().contains("Italic"),
            })
        },
    );
    let find = |text| runs.iter().find(|r| r.text.trim() == text).unwrap().style;
    assert!(find("Bold").bold);
    assert!(!find("Regular").bold);
    assert!(!find("Italic").bold && find("Italic").italic);
    assert!(!find("Generic").bold);
    assert!(find("Inherited bold").bold);
    assert!(!find("Reset").bold);
}

#[test]
fn readable_regular_metadata_overrides_misleading_resource_names() {
    let runs = parse(
        "<p>Actual regular</p>",
        "@font-face {font-family:alias;src:url('../Fonts/Fake-Bold.otf')} p {font-family:alias}",
        |_| Some(FontTraits::default()),
    );
    assert!(!runs[0].style.bold);
}

#[test]
fn missing_face_resources_keep_clear_family_style_hints() {
    let runs = parse(
        "<p>Unavailable</p>",
        "@font-face {font-family:Nexa-BoldItalic;src:local('Nexa Bold Italic')} p {font-family:Nexa-BoldItalic}",
        |_| panic!("local-only faces have no archive resource"),
    );
    assert!(runs[0].style.bold && runs[0].style.italic);
}

#[test]
fn font_shorthand_keeps_priority_resets_and_inline_family_information() {
    let runs = parse(
        "<p class='short'>Shorthand</p><p style='font:italic 16px / 1.4 serif'>Inline</p><p class='override'>Override</p>",
        ".short {font:16px/1.3 Nexa-Bold !important; font-family:serif} .override {font-weight:bold; font-style:italic; font:16px serif}",
        |_| None,
    );
    assert!(runs[0].style.bold && !runs[0].style.italic);
    assert!(!runs[1].style.bold && runs[1].style.italic);
    assert!(!runs[2].style.bold && !runs[2].style.italic);
}

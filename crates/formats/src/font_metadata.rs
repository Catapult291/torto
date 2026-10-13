//! Small publication-local summaries; decoded font data is immediately released.
use read_fonts::{FontRef, TableProvider};
use rebook_html::FontTraits;
use rebook_publication::PublicationUrl;
use std::{collections::HashMap, sync::Mutex};

pub(crate) const MAX_FONT_BYTES: usize = 8 * 1024 * 1024;
const MAX_CACHED_FONTS: usize = 128;

#[derive(Debug, Default)]
pub(crate) struct FontMetadataCache(Mutex<HashMap<PublicationUrl, Option<FontTraits>>>);

impl FontMetadataCache {
    pub(crate) fn get<B: AsRef<[u8]>>(
        &self,
        href: &PublicationUrl,
        load: impl FnOnce() -> Option<B>,
    ) -> Option<FontTraits> {
        let key = href.resource_url();
        if let Some(value) = self
            .0
            .lock()
            .ok()
            .and_then(|cache| cache.get(&key).copied())
        {
            return value;
        }
        let value = load().and_then(|bytes| traits(bytes.as_ref()));
        if let Ok(mut cache) = self.0.lock() {
            if cache.len() >= MAX_CACHED_FONTS
                && !cache.contains_key(&key)
                && let Some(old) = cache.keys().next().cloned()
            {
                cache.remove(&old);
            }
            cache.insert(key, value);
        }
        value
    }
}

fn traits(bytes: &[u8]) -> Option<FontTraits> {
    if bytes.len() > MAX_FONT_BYTES {
        return None;
    }
    let font = FontRef::from_index(bytes, 0).ok()?;
    let os2 = font.os2().ok();
    let head = font.head().ok();
    let mut result = FontTraits::default();
    if let Some(os2) = &os2 {
        use read_fonts::tables::os2::SelectionFlags;
        result.bold =
            os2.us_weight_class() >= 600 || os2.fs_selection().contains(SelectionFlags::BOLD);
        result.italic = os2
            .fs_selection()
            .intersects(SelectionFlags::ITALIC | SelectionFlags::OBLIQUE);
    }
    if let Some(head) = &head {
        use read_fonts::tables::head::MacStyle;
        result.bold |= head.mac_style().contains(MacStyle::BOLD);
        result.italic |= head.mac_style().contains(MacStyle::ITALIC);
    }
    // Some real fonts have a clear "Light Italic" subfamily but unset flags.
    let names = font.name().ok();
    if let Some(names) = &names {
        for record in names.name_record().iter().take(128) {
            if !matches!(record.name_id().to_u16(), 2 | 17 | 22) || record.length() > 256 {
                continue;
            }
            if let Ok(name) = record.string(names.string_data()) {
                let hint = FontTraits::from_style_name(&name.to_string());
                result.bold |= hint.bold;
                result.italic |= hint.italic;
            }
        }
    }
    (os2.is_some() || head.is_some() || names.is_some()).then_some(result)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn font(weight: u16, flags: u16) -> Vec<u8> {
        let mut bytes = vec![0; 106];
        bytes[..4].copy_from_slice(&0x00010000u32.to_be_bytes());
        bytes[4..6].copy_from_slice(&1u16.to_be_bytes());
        bytes[12..16].copy_from_slice(b"OS/2");
        bytes[20..24].copy_from_slice(&28u32.to_be_bytes());
        bytes[24..28].copy_from_slice(&78u32.to_be_bytes());
        bytes[32..34].copy_from_slice(&weight.to_be_bytes());
        bytes[90..92].copy_from_slice(&flags.to_be_bytes());
        bytes
    }

    #[test]
    fn intrinsic_weight_flags_and_regular_faces_are_distinct() {
        assert_eq!(
            traits(&font(600, 0)),
            Some(FontTraits {
                bold: true,
                italic: false
            })
        );
        assert_eq!(
            traits(&font(400, 1)),
            Some(FontTraits {
                bold: false,
                italic: true
            })
        );
        assert_eq!(traits(&font(400, 0)), Some(FontTraits::default()));
        assert_eq!(traits(b"invalid or obfuscated font"), None);
    }

    #[test]
    fn successes_and_failures_are_cached_without_retaining_font_bytes() {
        let cache = FontMetadataCache::default();
        let url = PublicationUrl::parse("font.otf").unwrap();
        let value = cache.get(&url, || Some(font(700, 0))).unwrap();
        assert!(value.bold);
        assert_eq!(
            cache.get(&url, || -> Option<Vec<u8>> { panic!("must use summary") }),
            Some(value)
        );
        let missing = PublicationUrl::parse("missing.otf").unwrap();
        assert_eq!(cache.get(&missing, || None::<Vec<u8>>), None);
        assert_eq!(
            cache.get(&missing, || -> Option<Vec<u8>> {
                panic!("must cache failure")
            }),
            None
        );
        for i in 0..140 {
            cache.get(&PublicationUrl::parse(&format!("{i}.otf")).unwrap(), || {
                None::<Vec<u8>>
            });
        }
        assert_eq!(cache.0.lock().unwrap().len(), MAX_CACHED_FONTS);
    }

    #[test]
    #[ignore = "requires TORTO_FONT_BOOK; reads the actual local Beauty EPUB"]
    fn local_beauty_dedicated_fonts_preserve_bold_and_italic_source_styles() {
        use rebook_publication::{Block, Inline, TextBlock};
        fn collect<'a>(blocks: &'a [Block], out: &mut Vec<&'a rebook_publication::TextRun>) {
            fn text<'a>(text: &'a TextBlock, out: &mut Vec<&'a rebook_publication::TextRun>) {
                for inline in &text.content {
                    if let Inline::Text(run) = inline {
                        out.push(run)
                    }
                }
            }
            for block in blocks {
                match block {
                    Block::Text(t) => text(t, out),
                    Block::Figure(f) => f.captions.iter().for_each(|t| text(t, out)),
                    Block::Table(t) => t.text_blocks().for_each(|t| text(t, out)),
                    Block::Quote(q) => q
                        .body
                        .iter()
                        .chain(&q.attribution)
                        .for_each(|t| text(t, out)),
                    Block::Note(n) => collect(&n.blocks, out),
                    _ => {}
                }
            }
        }
        let publication = crate::open_file(std::env::var("TORTO_FONT_BOOK").unwrap()).unwrap();
        let source = publication.source();
        let mut checked = Vec::new();
        for (suffix, text, bold, italic) in [
            (
                "042-045_Sustainable_Products.xhtml",
                "End-of-life",
                true,
                false,
            ),
            (
                "154-157_choose_shampoo_conditioner.xhtml",
                "Amount of oil:",
                true,
                false,
            ),
            (
                "034-037_Trust_Marketing.xhtml",
                "THERAPEUTIC (DRUG) CLAIMS",
                true,
                false,
            ),
            (
                "132-135_Supplements_Look-After_Skin.xhtml",
                "Polypodium",
                true,
                true,
            ),
            (
                "132-135_Supplements_Look-After_Skin.xhtml",
                "Polypodium",
                false,
                true,
            ),
            ("104-105_Botox.xhtml", "Clostridium botulinum", false, true),
            (
                "026-027_Active_Ingredients.xhtml",
                "Cutibacterium acnes",
                false,
                true,
            ),
            ("copyright.xhtml", "DK LONDON", true, false),
            ("Acknowledgments.xhtml", "Dr. Michelle Wong", true, false),
            ("Acknowledgments.xhtml", "Disclaimer", true, false),
        ] {
            let i = source
                .book()
                .sections
                .iter()
                .position(|s| s.href.path().ends_with(suffix))
                .unwrap();
            let section = source.parse_section(i).unwrap();
            let mut runs = Vec::new();
            collect(&section.blocks, &mut runs);
            let styles: Vec<_> = runs
                .iter()
                .filter(|r| r.text.trim() == text)
                .map(|r| (r.style.bold, r.style.italic))
                .collect();
            // Polypodium occurs in prose as light italic and in the table as
            // bold italic. Verify both authored uses rather than the first hit.
            assert!(
                styles.contains(&(bold, italic)),
                "{text} in {suffix}: {styles:?}"
            );
            println!("{text}: bold={bold}, italic={italic}");
            // The same flags survive persisted structure-cache reloads.
            assert_eq!(source.parse_section(i).unwrap(), section);
            checked.push((i, section));
        }
        let reopened = crate::open_file(std::env::var("TORTO_FONT_BOOK").unwrap()).unwrap();
        for (i, section) in checked {
            assert_eq!(reopened.source().parse_section(i).unwrap(), section);
        }
    }
}

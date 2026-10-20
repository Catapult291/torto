use serde::{Deserialize, Serialize};
use std::sync::OnceLock;
use unicode_segmentation::UnicodeSegmentation;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct Payload {
    pub request_id: String,
    #[serde(rename = "type")]
    pub kind: Kind,
    pub description: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub screenshots: Vec<Screenshot>,
    pub context: Context,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Screenshot {
    pub data: String,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum Kind {
    #[default]
    Bug,
    Feature,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Context {
    pub source: String,
    pub app: App,
    pub os: Os,
    pub display: Display,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub book: Option<Book>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub location: Option<Location>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reader: Option<Reader>,
}

#[derive(Clone)]
pub(crate) struct OpenContext {
    pub context: Context,
    // Used only by the preparation worker to determine PDF text source. Never uploaded.
    pub pdf_id: Option<String>,
}

impl OpenContext {
    pub fn shelf(ctx: &egui::Context) -> Self {
        let rect = ctx.content_rect();
        let scale = ctx.pixels_per_point();
        Self {
            context: Context {
                source: "shelf".into(),
                app: App {
                    version: env!("CARGO_PKG_VERSION").into(),
                },
                os: Os {
                    name: std::env::consts::OS.into(),
                    version: "unknown".into(),
                    arch: std::env::consts::ARCH.into(),
                },
                display: Display {
                    width: (rect.width() * scale).clamp(1.0, 100_000.0) as u32,
                    height: (rect.height() * scale).clamp(1.0, 100_000.0) as u32,
                    scale: scale.clamp(0.1, 20.0),
                },
                book: None,
                location: None,
                reader: None,
            },
            pdf_id: None,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct App {
    pub version: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Os {
    pub name: String,
    pub version: String,
    pub arch: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Display {
    pub width: u32,
    pub height: u32,
    pub scale: f32,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Book {
    pub title: String,
    pub authors: Vec<String>,
    pub languages: Vec<String>,
    pub format: String,
    pub layout: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Location {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub chapter: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub document: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub anchor: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub page: Option<usize>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct Reader {
    pub mode: String,
    pub typesetting: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pdf_reflow: Option<String>,
    pub translation: Translation,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct Translation {
    pub enabled: bool,
    pub target_language: String,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Draft {
    pub version: u8,
    pub endpoint: String,
    pub payload: Payload,
    pub retry_until: Option<u64>,
}

pub(super) fn character_count(value: &str) -> usize {
    static VISIBLE: OnceLock<regex::Regex> = OnceLock::new();
    let visible = VISIBLE.get_or_init(|| {
        regex::Regex::new(r"[^\s\p{Default_Ignorable_Code_Point}\p{Mark}\p{Control}]")
            .expect("feedback character regex")
    });
    value
        .graphemes(true)
        .filter(|part| visible.is_match(part))
        .count()
}

pub(super) fn valid_description(value: &str) -> bool {
    (10..=5000).contains(&character_count(value)) && value.encode_utf16().count() <= 10_000
}

pub(crate) fn clipped(value: &str, max: usize) -> String {
    let mut used = 0;
    value
        .trim()
        .chars()
        .take_while(|c| {
            used += c.len_utf16();
            used <= max
        })
        .collect()
}

pub(crate) fn book_document(path: &str) -> Option<String> {
    (!path.is_empty()
        && !path.starts_with(['/', '\\'])
        && !path.contains(':')
        && !path.split(['/', '\\']).any(|part| part == ".."))
    .then(|| clipped(path, 1000))
}

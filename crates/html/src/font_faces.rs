//! Intrinsic face styles survive replacing a publisher's font with reader fonts.
use super::*;
#[cfg(test)]
mod tests;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct FontTraits {
    pub bold: bool,
    pub italic: bool,
}

impl FontTraits {
    /// Delimited style names, never arbitrary substrings of a family name.
    pub fn from_style_name(name: &str) -> Self {
        let tokens: Vec<_> = name
            .split(|c: char| !c.is_alphanumeric())
            .filter(|s| !s.is_empty())
            .map(str::to_ascii_lowercase)
            .collect();
        let suffix: Vec<_> = tokens
            .iter()
            .rev()
            .take_while(|s| {
                matches!(
                    s.as_str(),
                    "regular"
                        | "light"
                        | "medium"
                        | "book"
                        | "bold"
                        | "semibold"
                        | "demibold"
                        | "demi"
                        | "black"
                        | "heavy"
                        | "extrabold"
                        | "ultrabold"
                        | "italic"
                        | "oblique"
                        | "bolditalic"
                        | "boldoblique"
                        | "lightitalic"
                        | "semibolditalic"
                        | "demibolditalic"
                )
            })
            .collect();
        let bold = suffix.iter().any(|s| {
            matches!(
                s.as_str(),
                "bold"
                    | "semibold"
                    | "demibold"
                    | "demi"
                    | "black"
                    | "heavy"
                    | "extrabold"
                    | "ultrabold"
                    | "bolditalic"
                    | "boldoblique"
                    | "semibolditalic"
                    | "demibolditalic"
            )
        });
        let italic = suffix.iter().any(|s| {
            matches!(
                s.as_str(),
                "italic"
                    | "oblique"
                    | "bolditalic"
                    | "boldoblique"
                    | "lightitalic"
                    | "semibolditalic"
                    | "demibolditalic"
            )
        });
        Self { bold, italic }
    }
}

#[derive(Clone)]
struct Face {
    weight: (u16, u16),
    italic: bool,
    urls: Vec<PublicationUrl>,
    hints: FontTraits,
    intrinsic: Option<FontTraits>,
}

#[derive(Default)]
pub(super) struct FontFaces {
    families: HashMap<String, Vec<Face>>,
}

pub(super) fn primary_family(value: &str) -> Option<String> {
    let mut quote = None;
    let end = value
        .char_indices()
        .find_map(|(i, c)| {
            if matches!(c, '\'' | '"') {
                if quote == Some(c) {
                    quote = None;
                } else if quote.is_none() {
                    quote = Some(c);
                }
            }
            (c == ',' && quote.is_none()).then_some(i)
        })
        .unwrap_or(value.len());
    let family = value[..end].trim().trim_matches(['\'', '"']).trim();
    (!family.is_empty()).then(|| family.to_ascii_lowercase())
}

fn urls(src: &str, base: &PublicationUrl) -> Vec<PublicationUrl> {
    let mut result = Vec::new();
    let mut tail = src;
    while let Some(start) = tail.to_ascii_lowercase().find("url(") {
        tail = &tail[start + 4..];
        let Some(end) = tail.find(')') else { break };
        let value = tail[..end].trim().trim_matches(['\'', '"']);
        if let Ok(url) = base.resolve(value) {
            let url = url.resource_url();
            if !result.contains(&url) {
                result.push(url);
            }
        }
        tail = &tail[end + 1..];
    }
    result
}

fn weight(value: &str, inherited: u16) -> Option<u16> {
    match value.trim() {
        "normal" | "initial" => Some(400),
        "bold" => Some(700),
        "bolder" => Some(if inherited < 350 {
            400
        } else if inherited < 550 {
            700
        } else {
            900
        }),
        "lighter" => Some(if inherited < 550 {
            100
        } else if inherited < 750 {
            400
        } else {
            700
        }),
        "inherit" | "unset" => Some(inherited),
        v => v.parse::<u16>().ok().filter(|w| (1..=1000).contains(w)),
    }
}

impl FontFaces {
    pub(super) fn is_empty(&self) -> bool {
        self.families.is_empty()
    }

    pub(super) fn add(&mut self, css: &str, base: &PublicationUrl) {
        // Resource URLs are case-sensitive. The ordinary property parser
        // lowercases values, so retain original @font-face source spelling.
        let props: HashMap<_, _> = css
            .split(';')
            .filter_map(|d| {
                let (k, v) = d.split_once(':')?;
                Some((k.trim().to_ascii_lowercase(), v.trim().to_owned()))
            })
            .collect();
        let Some(family) = props.get("font-family").and_then(|v| primary_family(v)) else {
            return;
        };
        let weights: Vec<_> = props
            .get("font-weight")
            .map_or("normal", String::as_str)
            .split_whitespace()
            .filter_map(|v| weight(&v.to_ascii_lowercase(), 400))
            .collect();
        let lo = weights.first().copied().unwrap_or(400);
        let hi = weights.get(1).copied().unwrap_or(lo).max(lo);
        let src = props.get("src").map_or("", String::as_str);
        let hints = urls(src, base)
            .iter()
            .fold(FontTraits::default(), |mut hints, url| {
                let name = url
                    .path()
                    .rsplit('/')
                    .next()
                    .unwrap_or_default()
                    .rsplit_once('.')
                    .map_or(url.path(), |(stem, _)| stem);
                let inferred = FontTraits::from_style_name(name);
                hints.bold |= inferred.bold;
                hints.italic |= inferred.italic;
                hints
            });
        self.families.entry(family).or_default().push(Face {
            weight: (lo, hi),
            italic: props.get("font-style").is_some_and(|s| {
                let s = s.to_ascii_lowercase();
                s.starts_with("italic") || s.starts_with("oblique")
            }),
            urls: urls(src, base),
            hints,
            intrinsic: None,
        });
    }

    pub(super) fn prepare(
        &mut self,
        used: &HashSet<String>,
        load: &mut impl FnMut(&PublicationUrl) -> Option<FontTraits>,
    ) {
        let mut metadata = HashMap::new();
        for (family, faces) in &mut self.families {
            if !used.contains(family) {
                continue;
            }
            for face in faces {
                for url in &face.urls {
                    if metadata.len() >= 64 && !metadata.contains_key(url) {
                        break;
                    }
                    let value = *metadata.entry(url.clone()).or_insert_with(|| load(url));
                    if value.is_some() {
                        face.intrinsic = value;
                        break;
                    }
                }
            }
        }
    }

    pub(super) fn resolve(&self, family: &str, requested: u16, italic: bool) -> Option<FontTraits> {
        if let Some(faces) = self.families.get(family) {
            // CSS weight search: 400..500 prefers the remaining part of that
            // interval, then lighter faces, then faces above 500.
            let rank = |face: &Face| {
                let w = requested.clamp(face.weight.0, face.weight.1);
                let rank = if w == requested {
                    (0, 0)
                } else if (400..=500).contains(&requested) && w >= requested && w <= 500 {
                    (1, w - requested)
                } else if requested <= 500 && w < requested {
                    (2, requested - w)
                } else if requested <= 500 {
                    (3, w - requested)
                } else if w > requested {
                    (1, w - requested)
                } else {
                    (2, requested - w)
                };
                (face.italic != italic, rank)
            };
            let face = faces.iter().rev().min_by_key(|face| rank(face))?;
            let family_hint = FontTraits::from_style_name(family);
            let mut traits = face.intrinsic.unwrap_or(FontTraits {
                bold: face.hints.bold || family_hint.bold || face.weight.0 >= 600,
                italic: face.hints.italic || family_hint.italic || face.italic,
            });
            if face.weight.0 != face.weight.1 {
                traits.bold = false;
            }
            return Some(traits);
        }
        let hint = FontTraits::from_style_name(family);
        (hint.bold || hint.italic).then_some(hint)
    }
}

#[derive(Clone, Default)]
struct FontState {
    family: Option<Rc<str>>,
    weight: u16,
    italic: bool,
    managed: bool,
    traits: FontTraits,
}

#[derive(Default)]
pub(super) struct FontStateCache(HashMap<roxmltree::NodeId, FontState>);

impl StyleSheet {
    pub(super) fn font_traits(&self, node: Node<'_, '_>) -> Option<FontTraits> {
        if !self.has_font_families {
            return None;
        }
        let state = self.font_state(node);
        state.managed.then_some(state.traits)
    }

    fn font_state(&self, node: Node<'_, '_>) -> FontState {
        if let Some(state) = self.font_cache.borrow().0.get(&node.id()) {
            return state.clone();
        }
        let mut state = node
            .parent_element()
            .map(|p| self.font_state(p))
            .unwrap_or(FontState {
                weight: 400,
                ..Default::default()
            });
        match node.tag_name().name() {
            "b" | "strong" | "h1" | "h2" | "h3" | "h4" | "h5" | "h6" => state.weight = 700,
            "i" | "em" | "cite" => state.italic = true,
            _ => {}
        }
        let props = self.cascaded_properties(node);
        if let Some(family) = props.get("font-family") {
            match family.as_str() {
                "inherit" | "unset" => {}
                "initial" => state.family = None,
                v => state.family = primary_family(v).map(Rc::from),
            }
        }
        if let Some(w) = props
            .get("font-weight")
            .and_then(|v| weight(v, state.weight))
        {
            state.weight = w;
        }
        if let Some(s) = props.get("font-style") {
            match s.as_str() {
                "normal" | "initial" => state.italic = false,
                "italic" | "oblique" => state.italic = true,
                _ => {}
            }
        }
        let intrinsic = state
            .family
            .as_deref()
            .and_then(|family| self.font_faces.resolve(family, state.weight, state.italic));
        state.managed |= intrinsic.is_some();
        state.traits = FontTraits {
            bold: state.weight >= 600 || intrinsic.is_some_and(|v| v.bold),
            italic: state.italic || intrinsic.is_some_and(|v| v.italic),
        };
        let mut cache = self.font_cache.borrow_mut();
        // Document-local summaries only. Family strings are shared with children.
        if cache.0.len() < 16_384 {
            cache.0.insert(node.id(), state.clone());
        }
        state
    }
}

impl FontStateCache {
    pub(super) fn clear(&mut self) {
        self.0.clear();
    }
}

/// Expand the supported longhands of the font shorthand before the cascade,
/// preserving its resets and declaration priority relative to other properties.
pub(super) fn expand_shorthand(value: &str) -> Option<Vec<(String, String)>> {
    let mut cursor = 0;
    let tokens: Vec<_> = value
        .split_whitespace()
        .map(|token| {
            let start = cursor + value[cursor..].find(token).unwrap();
            cursor = start + token.len();
            (start, cursor, token)
        })
        .collect();
    let (size_index, (_, _, size)) = tokens.iter().enumerate().find(|(_, (_, _, t))| {
        FontSize::parse(t.split('/').next().unwrap_or_default()).is_some()
    })?;
    let (size, inline_height) = size
        .split_once('/')
        .map_or((*size, None), |(s, h)| (s, Some(h)));
    let mut family_index = size_index + 1;
    let mut height = inline_height.filter(|h| !h.is_empty());
    if inline_height == Some("") {
        height = Some(tokens.get(family_index)?.2);
        family_index += 1;
    } else if let Some((_, _, token)) = tokens.get(family_index) {
        if let Some(h) = token.strip_prefix('/') {
            family_index += 1;
            if h.is_empty() {
                height = Some(tokens.get(family_index)?.2);
                family_index += 1;
            } else {
                height = Some(h);
            }
        }
    }
    let family = &value[tokens.get(family_index)?.0..];
    let prefix = &tokens[..size_index];
    let font_weight = prefix
        .iter()
        .find_map(|(_, _, t)| {
            (*t != "normal")
                .then(|| weight(t, 400))
                .flatten()
                .map(|_| *t)
        })
        .unwrap_or("normal");
    let font_style = prefix
        .iter()
        .find_map(|(_, _, t)| matches!(*t, "italic" | "oblique").then_some(*t))
        .unwrap_or("normal");
    Some(vec![
        ("font-family".into(), family.into()),
        ("font-weight".into(), font_weight.into()),
        ("font-style".into(), font_style.into()),
        ("font-size".into(), size.into()),
        ("line-height".into(), height.unwrap_or("normal").into()),
    ])
}

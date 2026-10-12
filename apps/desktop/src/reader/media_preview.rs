//! Media lookup and preview rasterization are dispatched only after a click.
//! The worker retains bounded identifiers, never sections, layouts or pixels.
use anyrender::ImageRenderer;
use rebook_layout::{LayoutEngine, LayoutViewport, ReaderStyle, SpreadMode};
use rebook_publication::{
    Block, BookSource, Inline, PublicationUrl, Section, SourceRange, TableBlock, TextBlock,
    media_references::{self, MediaKey, MediaKind},
};
use std::{
    collections::HashMap,
    ops::Range,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
        mpsc,
    },
};

const INDEX_BUDGET: usize = 2 * 1024 * 1024;
const PIXEL_BUDGET: usize = 32 * 1024 * 1024;

#[derive(Clone, Debug, PartialEq, Eq)]
struct Target {
    section: usize,
    block: usize,
    images: Vec<usize>,
    captions: Vec<usize>,
    key: MediaKey,
}

fn plain(text: &TextBlock) -> String {
    text.content
        .iter()
        .flat_map(Inline::text_runs)
        .map(|run| run.text.as_str())
        .collect()
}

fn caption_text(text: &TextBlock) -> String {
    text.content
        .iter()
        .map(|inline| match inline {
            Inline::Math(math) => math
                .original_text()
                .unwrap_or_else(|| format!("\\({}\\)", math.latex)),
            Inline::Image(image) => image
                .image
                .formula
                .as_ref()
                .map(|formula| format!("\\({}\\)", formula.latex))
                .unwrap_or_else(|| {
                    let alt = &image.image.alt;
                    if [".png", ".jpg", ".jpeg", ".svg", ".gif", ".webp"]
                        .iter()
                        .any(|suffix| alt.to_ascii_lowercase().ends_with(suffix))
                    {
                        "□".into()
                    } else {
                        alt.clone()
                    }
                }),
            _ => inline
                .text_runs()
                .iter()
                .map(|run| run.text.as_str())
                .collect(),
        })
        .collect()
}

#[cfg(test)]
#[path = "media_preview_tests.rs"]
mod tests;

fn targets(section: &Section, index: usize) -> Vec<Target> {
    let mut result = Vec::new();
    for (at, block) in section.blocks.iter().enumerate() {
        let mut captions = Vec::new();
        let mut images = Vec::new();
        let (kind, texts): (Option<MediaKind>, Vec<&TextBlock>) = match block {
            // Some books publish their numbered tables as images.
            Block::Figure(figure) => (None, figure.captions.iter().collect()),
            Block::Table(table) => {
                let mut texts: Vec<_> = table.before.iter().chain(&table.after).collect();
                if let Some(Block::Text(text)) =
                    at.checked_sub(1).and_then(|i| section.blocks.get(i))
                    && media_references::caption_label(&plain(text))
                        .is_some_and(|label| label.key.kind == MediaKind::Table)
                {
                    captions.push(at - 1);
                    texts.push(text);
                }
                for (next, adjacent) in section.blocks.iter().enumerate().skip(at + 1) {
                    let Block::Text(text) = adjacent else { break };
                    let value = plain(text);
                    let lower = value.trim_start().to_ascii_lowercase();
                    if [
                        "note:",
                        "notes:",
                        "source:",
                        "sources:",
                        "注：",
                        "注:",
                        "来源：",
                        "来源:",
                    ]
                    .iter()
                    .any(|prefix| lower.starts_with(prefix))
                    {
                        captions.push(next);
                    } else {
                        break;
                    }
                }
                (Some(MediaKind::Table), texts)
            }
            Block::Text(text) if media_references::caption_label(&plain(text)).is_some() => {
                // A label alone (e.g. a list of illustrations) is never a target.
                captions.push(at);
                let mut start = at;
                while start > 0 && matches!(section.blocks[start - 1], Block::Image(_)) {
                    start -= 1;
                }
                let mut end = at + 1;
                while end < section.blocks.len() && matches!(section.blocks[end], Block::Image(_)) {
                    end += 1;
                }
                images.extend(start..at);
                if images.is_empty() {
                    images.extend(at + 1..end);
                }
                if images.is_empty() {
                    continue;
                }
                (None, vec![text])
            }
            _ => continue,
        };
        for text in texts {
            let Some(label) = media_references::caption_label(&plain(text)) else {
                continue;
            };
            if kind.is_some_and(|kind| label.key.kind != kind) {
                continue;
            }
            let target = Target {
                section: index,
                block: at,
                images: images.clone(),
                captions: captions.clone(),
                key: label.key,
            };
            if !result.contains(&target) {
                result.push(target);
            }
        }
    }
    result
}

fn text_blocks(block: &Block) -> Vec<&TextBlock> {
    match block {
        Block::Text(t) => vec![t],
        Block::Figure(f) => f.captions.iter().collect(),
        Block::Table(t) => t.text_blocks().collect(),
        Block::Quote(q) => q.body.iter().chain(&q.attribution).collect(),
        Block::Note(n) => n.blocks.iter().flat_map(text_blocks).collect(),
        _ => vec![],
    }
}

fn authored_link(
    section: &Section,
    origin: &SourceRange,
    key: &MediaKey,
) -> Result<Option<PublicationUrl>, String> {
    let mut found = None;
    for text in section.blocks.iter().flat_map(text_blocks) {
        if !text
            .source
            .as_ref()
            .is_some_and(|s| s.start.node == origin.start.node)
        {
            continue;
        }
        let value = plain(text);
        for reference in media_references::references(&value)
            .into_iter()
            .filter(|r| &r.key == key)
        {
            let base = text.source.as_ref().unwrap().start.text_offset;
            let start = base + value[..reference.range.start].chars().count() as u64;
            let end = base + value[..reference.range.end].chars().count() as u64;
            if end <= origin.start.text_offset || start >= origin.end.text_offset {
                continue;
            }
            let mut offset = 0;
            for run in text.content.iter().flat_map(Inline::text_runs) {
                let end = offset + run.text.len();
                if offset < reference.range.end
                    && end > reference.range.start
                    && let Some(href) = run
                        .link
                        .as_ref()
                        .filter(|href| href.website_url().is_none())
                {
                    if found.as_ref().is_some_and(|previous| previous != href) {
                        return Err("This reference has several different authored targets.".into());
                    }
                    found = Some(href.clone());
                }
                offset = end;
            }
        }
    }
    Ok(found)
}

fn target_has_anchor(section: &Section, target: &Target, href: &PublicationUrl) -> bool {
    let Some(fragment) = href.fragment() else {
        return false;
    };
    let Some(anchor) = section.anchors.iter().find(|a| a.fragment == fragment) else {
        return false;
    };
    let mut indices = vec![target.block];
    indices.extend(&target.images);
    indices.extend(&target.captions);
    indices.into_iter().any(|index| {
        let block = &section.blocks[index];
        let mut sources: Vec<_> = text_blocks(block)
            .into_iter()
            .filter_map(|t| t.source.as_ref())
            .collect();
        match block {
            Block::Image(i) => sources.extend(i.source.as_ref()),
            Block::Figure(f) => {
                sources.extend(f.source.as_ref());
                sources.extend(f.images.iter().filter_map(|i| i.source.as_ref()));
            }
            Block::Table(t) => sources.extend(t.source.as_ref()),
            _ => {}
        }
        sources.iter().any(|s| s.start.node == anchor.source.node)
    })
}

fn matching(target: &Target, key: &MediaKey) -> bool {
    target.key == *key
        || (target.key.kind == key.kind
            && key.number.strip_suffix(|c: char| c.is_ascii_alphabetic())
                == Some(target.key.number.as_str()))
}

fn unique(candidates: impl Iterator<Item = Target>) -> Result<Option<Target>, String> {
    let mut candidates = candidates.collect::<Vec<_>>();
    // Multiple subfigure labels can describe the same complete figure.
    candidates.dedup_by(|a, b| a.section == b.section && a.block == b.block);
    match candidates.len() {
        0 => Ok(None),
        1 => Ok(candidates.pop()),
        _ => Err("This figure/table number has several possible targets in this book.".into()),
    }
}

fn select(
    candidates: impl Iterator<Item = Target>,
    key: &MediaKey,
) -> Result<Option<Target>, String> {
    let matches = candidates
        .filter(|target| matching(target, key))
        .collect::<Vec<_>>();
    if matches.iter().any(|t| t.key == *key) {
        unique(matches.into_iter().filter(|t| t.key == *key))
    } else {
        unique(matches.into_iter())
    }
}

fn chapter_scope(source: &dyn BookSource, origin: usize) -> Range<usize> {
    fn collect(
        entries: &[rebook_publication::TocEntry],
        source: &dyn BookSource,
        starts: &mut Vec<usize>,
    ) {
        for entry in entries {
            let label = entry.label.to_ascii_lowercase();
            if (label.contains("chapter") || (label.contains('第') && label.contains('章')))
                && let Some(href) = &entry.href
                && let Some(index) = source
                    .book()
                    .sections
                    .iter()
                    .position(|s| s.href.path() == href.path())
            {
                starts.push(index);
            }
            collect(&entry.children, source, starts);
        }
    }
    let mut starts = vec![];
    collect(&source.book().table_of_contents, source, &mut starts);
    starts.sort_unstable();
    starts.dedup();
    let Some(start) = starts.iter().copied().take_while(|s| *s <= origin).last() else {
        return origin..origin + 1;
    };
    start
        ..starts
            .iter()
            .copied()
            .find(|s| *s > origin)
            .unwrap_or(source.book().sections.len())
}

#[derive(Default)]
struct Index {
    entries: HashMap<usize, Vec<Target>>,
    bytes: usize,
}
impl Index {
    fn add(&mut self, section: &Section, at: usize) -> Result<(), String> {
        if self.entries.contains_key(&at) {
            return Ok(());
        }
        let entries = targets(section, at);
        let bytes = entries
            .iter()
            .map(|t| {
                std::mem::size_of::<Target>()
                    + t.key.number.len()
                    + (t.images.len() + t.captions.len()) * 8
            })
            .sum::<usize>()
            + 64;
        if self.bytes + bytes > INDEX_BUDGET {
            return Err("The figure/table index exceeds the preview limit.".into());
        }
        self.bytes += bytes;
        self.entries.insert(at, entries);
        Ok(())
    }
    fn resolve(&mut self, job: &Job, needed: &impl Fn() -> bool) -> Result<Target, String> {
        let source = job.source.as_ref();
        let origin_index = source
            .book()
            .sections
            .iter()
            .position(|s| s.id == job.origin.start.spine)
            .ok_or("The reference section is unavailable.")?;
        let origin = source
            .parse_section(origin_index)
            .map_err(|e| e.to_string())?;
        self.add(&origin, origin_index)?;
        if let Some(href) = authored_link(&origin, &job.origin, &job.key)?
            && let Some(index) = source
                .book()
                .sections
                .iter()
                .position(|s| s.href.path() == href.path())
        {
            let section = if index == origin_index {
                origin.clone()
            } else {
                source.parse_section(index).map_err(|e| e.to_string())?
            };
            self.add(&section, index)?;
            if let Some(target) = select(
                self.entries[&index]
                    .iter()
                    .filter(|t| target_has_anchor(&section, t, &href))
                    .cloned(),
                &job.key,
            )? {
                return Ok(target);
            }
        }
        if let Some(target) = select(self.entries[&origin_index].iter().cloned(), &job.key)? {
            return Ok(target);
        }
        let scope = chapter_scope(source, origin_index);
        for index in scope.clone() {
            if !needed() {
                return Err("Cancelled".into());
            }
            if !self.entries.contains_key(&index) {
                self.add(
                    &source.parse_section(index).map_err(|e| e.to_string())?,
                    index,
                )?;
            }
        }
        if let Some(target) = select(
            scope.flat_map(|index| &self.entries[&index]).cloned(),
            &job.key,
        )? {
            return Ok(target);
        }
        // Build remaining metadata incrementally on demand. Every iteration is cancellable;
        // parsed sections are dropped immediately and no image is decoded here.
        for index in 0..source.book().sections.len() {
            if !needed() {
                return Err("Cancelled".into());
            }
            if self.entries.contains_key(&index) {
                continue;
            }
            let section = source.parse_section(index).map_err(|e| e.to_string())?;
            self.add(&section, index)?;
        }
        select(self.entries.values().flatten().cloned(), &job.key)?
            .ok_or_else(|| "No matching figure/table was found in this book.".into())
    }
}

/// Bound each table shaping job before layout sees the rows. Never cut a rowspan.
fn table_chunks(table: &TableBlock) -> Result<Vec<Range<usize>>, String> {
    let mut chunks = Vec::new();
    let mut start = 0;
    while start < table.rows.len() {
        let mut end = (start + 24).min(table.rows.len());
        let mut row = start;
        while row < end {
            for cell in &table.rows[row].cells {
                end = end.max(
                    row.saturating_add(usize::from(cell.row_span.max(1)))
                        .min(table.rows.len()),
                );
            }
            if end - start > 128 {
                return Err("A table row group is too large to preview safely.".into());
            }
            row += 1;
        }
        chunks.push(start..end);
        start = end;
    }
    Ok(chunks)
}

#[derive(Clone, Copy, Default)]
struct Cursor {
    chunk: usize,
    page: usize,
}
struct Job {
    generation: u64,
    source: Arc<dyn BookSource>,
    origin: SourceRange,
    key: MediaKey,
    cursor: Cursor,
    style: ReaderStyle,
    ctx: egui::Context,
}
struct Ready {
    image: egui::ColorImage,
    size: [usize; 2],
    captions: Vec<String>,
    caption_images: Vec<egui::ColorImage>,
    previous: Option<Cursor>,
    next: Option<Cursor>,
    #[cfg(test)]
    position: String,
}
struct Completion {
    generation: u64,
    revision: Option<u64>,
    result: Result<Ready, String>,
}
struct Worker {
    tx: mpsc::SyncSender<()>,
    pending: Arc<arc_swap::ArcSwapOption<Job>>,
    result: Arc<arc_swap::ArcSwapOption<Completion>>,
    generation: Arc<AtomicU64>,
}
impl Drop for Worker {
    fn drop(&mut self) {
        self.generation.fetch_add(1, Ordering::Release);
        self.pending.store(None);
    }
}

fn render(job: &Job, target: Target, needed: &impl Fn() -> bool) -> Result<Ready, String> {
    let section = job
        .source
        .parse_section(target.section)
        .map_err(|e| e.to_string())?;
    if !needed() {
        return Err("Cancelled".into());
    }
    // Sources without a revision can still receive semantic/translation updates
    // between lookup and rendering. Never reuse a shifted block ordinal.
    if !targets(&section, target.section).contains(&target) {
        return Err("The figure/table changed. Please reopen the preview.".into());
    }
    let block = section
        .blocks
        .get(target.block)
        .ok_or("The figure/table changed. Please reopen the preview.")?;
    let mut caption_blocks: Vec<TextBlock> = Vec::new();
    let (chunk, chunk_count, blocks) = match block {
        Block::Figure(figure) => {
            caption_blocks.extend(figure.captions.iter().cloned());
            let at = job.cursor.chunk.min(figure.images.len().saturating_sub(1));
            (
                at,
                figure.images.len(),
                vec![Block::Image(
                    figure
                        .images
                        .get(at)
                        .ok_or("The figure has no image.")?
                        .clone(),
                )],
            )
        }
        Block::Table(table) => {
            caption_blocks.extend(table.before.iter().chain(&table.after).cloned());
            caption_blocks.extend(target.captions.iter().filter_map(|i| {
                match &section.blocks[*i] {
                    Block::Text(t) => Some(t.clone()),
                    _ => None,
                }
            }));
            let ranges = table_chunks(table)?;
            let at = job.cursor.chunk.min(ranges.len().saturating_sub(1));
            let range = ranges.get(at).ok_or("The table is empty.")?.clone();
            let mut part = TableBlock {
                before: vec![],
                after: vec![],
                rows: table.rows[range].to_vec(),
                source: table.source.clone(),
            };
            if at > 0 {
                let headers: Vec<_> = table
                    .rows
                    .iter()
                    .take(8)
                    .take_while(|row| {
                        !row.cells.is_empty()
                            && row
                                .cells
                                .iter()
                                .all(|cell| cell.header && cell.row_span == 1)
                    })
                    .cloned()
                    .collect();
                part.rows.splice(0..0, headers);
            }
            if part
                .rows
                .iter()
                .flat_map(|r| &r.cells)
                .map(|c| plain(&c.text).len())
                .sum::<usize>()
                > 512 * 1024
            {
                return Err("This table segment is too large to preview safely.".into());
            }
            (at, ranges.len(), vec![Block::Table(part)])
        }
        Block::Text(_) => {
            caption_blocks.extend(target.captions.iter().filter_map(|i| {
                match &section.blocks[*i] {
                    Block::Text(t) => Some(t.clone()),
                    _ => None,
                }
            }));
            let at = job.cursor.chunk.min(target.images.len().saturating_sub(1));
            (
                at,
                target.images.len(),
                vec![
                    section.blocks[*target.images.get(at).ok_or("The figure has no image.")?]
                        .clone(),
                ],
            )
        }
        _ => return Err("The figure/table changed. Please reopen the preview.".into()),
    };
    let mut style = job.style.clone();
    style.typesetting.mode = rebook_layout::TypesettingMode::Unified;
    style.spread = SpreadMode::Single;
    style.focus_footnote_icons = false;
    style.website_icons = false;
    style.horizontal_margin = 24.0;
    style.minimum_horizontal_margin = 0.0;
    style.minimum_content_width = 0.0;
    style.top_margin = 24.0;
    style.bottom_margin = 24.0;
    style.foreground = rebook_publication::Rgba::BLACK;
    style.background = rebook_publication::Rgba {
        red: 255,
        green: 255,
        blue: 255,
        alpha: 255,
    };
    style.typography.font_size = 22.0;
    let height = if target.key.kind == MediaKind::Table {
        2048
    } else {
        1400
    };
    let mut engine =
        LayoutEngine::with_fonts(crate::fonts::embedded_reader_fonts().iter().cloned());
    engine.set_deferred_images(true);
    let layout = engine
        .layout_blocks(
            job.source.as_ref(),
            &blocks,
            LayoutViewport::new(1200, height).map_err(|e| e.to_string())?,
            &style,
        )
        .map_err(|e| e.to_string())?;
    if !needed() {
        return Err("Cancelled".into());
    }
    let at = job.cursor.page.min(layout.pages.len().saturating_sub(1));
    let page = layout.pages.get(at).ok_or("The preview is empty.")?;
    let display = rebook_renderer::DisplayListCompiler.compile(page);
    for (request, _) in display.deferred_rasters() {
        request
            .load_guarded(job.source.as_ref(), needed)
            .map_err(|e| e.to_string())?;
    }
    if !needed() {
        return Err("Cancelled".into());
    }
    let height = (display.content_bottom().unwrap_or(100.0) + 8.0)
        .ceil()
        .clamp(1.0, height as f32) as usize;
    let width = rebook_layout::reading_content_width(1200.0, &style).ceil() as usize + 32;
    let left = rebook_layout::reading_content_left(1200.0, &style) - 16.0;
    if width * height * 4 > PIXEL_BUDGET {
        return Err("The preview exceeds the image memory limit.".into());
    }
    let mut renderer = anyrender_vello_cpu::VelloCpuImageRenderer::new(width as u32, height as u32);
    let mut pixels = Vec::new();
    renderer.render_to_vec(
        |scene| display.paint_scaled_at(scene, 1.0, -left, 0.0),
        &mut pixels,
    );
    let image = egui::ColorImage::from_rgba_premultiplied([width, height], &pixels);
    drop(pixels);
    let captions = caption_blocks.iter().map(caption_text).collect();
    let mut caption_style = style.clone();
    caption_style.typography.font_size = 20.0;
    caption_style.horizontal_margin = 12.0;
    caption_style.top_margin = 8.0;
    caption_style.bottom_margin = 8.0;
    for caption in &mut caption_blocks {
        caption.kind = rebook_publication::TextBlockKind::Caption;
    }
    let caption_blocks: Vec<_> = caption_blocks.into_iter().map(Block::Text).collect();
    let caption_layout = engine
        .layout_blocks(
            job.source.as_ref(),
            &caption_blocks,
            LayoutViewport::new(800, 2048).map_err(|e| e.to_string())?,
            &caption_style,
        )
        .map_err(|e| e.to_string())?;
    let mut caption_images = Vec::new();
    let mut retained_bytes = image.pixels.len() * 4;
    for page in &caption_layout.pages {
        if !needed() {
            return Err("Cancelled".into());
        }
        let display = rebook_renderer::DisplayListCompiler.compile(page);
        let Some(bottom) = display.content_bottom() else {
            continue;
        };
        let height = (bottom + 8.0).ceil().clamp(1.0, 2048.0) as usize;
        retained_bytes += 800 * height * 4;
        if retained_bytes > PIXEL_BUDGET {
            return Err("The caption exceeds the preview memory limit.".into());
        }
        for (request, _) in display.deferred_rasters() {
            request
                .load_guarded(job.source.as_ref(), needed)
                .map_err(|e| e.to_string())?;
        }
        let mut renderer = anyrender_vello_cpu::VelloCpuImageRenderer::new(800, height as u32);
        let mut pixels = Vec::new();
        renderer.render_to_vec(|scene| display.paint(scene), &mut pixels);
        caption_images.push(egui::ColorImage::from_rgba_premultiplied(
            [800, height],
            &pixels,
        ));
    }
    let previous = if at > 0 {
        Some(Cursor {
            chunk,
            page: at - 1,
        })
    } else if chunk > 0 {
        Some(Cursor {
            chunk: chunk - 1,
            page: usize::MAX,
        })
    } else {
        None
    };
    let next = if at + 1 < layout.pages.len() {
        Some(Cursor {
            chunk,
            page: at + 1,
        })
    } else if chunk + 1 < chunk_count {
        Some(Cursor {
            chunk: chunk + 1,
            page: 0,
        })
    } else {
        None
    };
    Ok(Ready {
        image,
        size: [width, height],
        captions,
        caption_images,
        previous,
        next,
        #[cfg(test)]
        position: format!(
            "{} / {} · {} / {}",
            chunk + 1,
            chunk_count,
            at + 1,
            layout.pages.len()
        ),
    })
}

impl Worker {
    fn new() -> Self {
        let (tx, wake) = mpsc::sync_channel::<()>(1);
        let pending = Arc::new(arc_swap::ArcSwapOption::empty());
        let jobs: Arc<arc_swap::ArcSwapOption<Job>> = pending.clone();
        let complete = Arc::new(arc_swap::ArcSwapOption::empty());
        let result = complete.clone();
        let generation = Arc::new(AtomicU64::new(0));
        let epoch = generation.clone();
        std::thread::Builder::new()
            .name("media-preview".into())
            .spawn(move || {
                let mut index = Index::default();
                let mut identity = None;
                while wake.recv().is_ok() {
                    let Some(job) = jobs.swap(None) else { continue };
                    let Ok(job) = Arc::try_unwrap(job) else {
                        continue;
                    };
                    let key = (
                        job.source.book().id.clone(),
                        job.source.content_revision(),
                        Arc::as_ptr(&job.source) as *const () as usize,
                    );
                    if identity.as_ref() != Some(&key) || key.1.is_none() {
                        index = Index::default();
                        identity = Some(key);
                    }
                    let needed = || epoch.load(Ordering::Acquire) == job.generation;
                    if !needed() {
                        continue;
                    }
                    let revision = job.source.content_revision();
                    let result = index
                        .resolve(&job, &needed)
                        .and_then(|target| render(&job, target, &needed));
                    if needed() {
                        complete.store(Some(Arc::new(Completion {
                            generation: job.generation,
                            revision,
                            result,
                        })));
                        job.ctx.request_repaint();
                    }
                }
            })
            .expect("media preview worker");
        Self {
            tx,
            pending,
            result,
            generation,
        }
    }
}

struct Preview {
    origin: SourceRange,
    key: MediaKey,
    texture: Option<egui::TextureHandle>,
    captions: Vec<String>,
    caption_textures: Vec<egui::TextureHandle>,
    size: egui::Vec2,
    zoom: f32,
    pan: egui::Vec2,
    previous: Option<Cursor>,
    next: Option<Cursor>,
    error: Option<String>,
    loading: bool,
}
#[derive(Default)]
pub(super) struct MediaPreviewController {
    worker: Option<Worker>,
    active: Option<Preview>,
    restore_parent: bool,
}

impl super::DesktopReader {
    pub(super) fn preview_is_open(&self) -> bool {
        self.image_preview.is_some() || self.media_preview.active.is_some()
    }
    pub(super) fn media_preview_is_open(&self) -> bool {
        self.media_preview.active.is_some()
    }
    pub(super) fn preserve_media_parent_popup(&mut self) -> bool {
        if !self.ui.focus_footnotes_visible {
            self.media_preview.restore_parent = false;
        }
        self.media_preview.active.is_some()
            || (self.media_preview.restore_parent && self.ui.focus_footnotes_visible)
    }
    pub(super) fn media_preview_copy(&self, ctx: &egui::Context) -> bool {
        let Some(active) = &self.media_preview.active else {
            return false;
        };
        if ctx.input_mut(|i| i.consume_shortcut(&self.shortcuts.copy)) {
            ctx.copy_text(active.captions.join("\n"));
        }
        true
    }
    pub(super) fn open_media_preview(
        &mut self,
        ctx: &egui::Context,
        origin: SourceRange,
        key: MediaKey,
    ) {
        self.ui.focus_navigation_repeat = Default::default();
        self.ui.focus_wheel.reset();
        self.ui.wheel_accumulator = 0.0;
        self.media_preview.restore_parent = self.ui.focus_footnotes_visible;
        self.media_preview.active = Some(Preview {
            origin,
            key,
            texture: None,
            captions: vec![],
            caption_textures: vec![],
            size: egui::Vec2::ZERO,
            zoom: 1.0,
            pan: egui::Vec2::ZERO,
            previous: None,
            next: None,
            error: None,
            loading: true,
        });
        self.request_media_page(ctx, Cursor::default());
    }
    fn request_media_page(&mut self, ctx: &egui::Context, cursor: Cursor) {
        let worker = self.media_preview.worker.get_or_insert_with(Worker::new);
        // One pending request and one result slot bound rapid close/reopen cycles.
        worker.result.store(None);
        let Some(active) = &mut self.media_preview.active else {
            return;
        };
        active.loading = true;
        active.texture = None;
        active.caption_textures.clear();
        active.error = None;
        active.pan = egui::Vec2::ZERO;
        active.zoom = 1.0;
        let generation = worker.generation.fetch_add(1, Ordering::AcqRel) + 1;
        worker.pending.store(Some(Arc::new(Job {
            generation,
            source: self.source.clone(),
            origin: active.origin.clone(),
            key: active.key.clone(),
            cursor,
            style: self.reader.style(),
            ctx: ctx.clone(),
        })));
        let _ = worker.tx.try_send(());
        ctx.request_repaint();
    }
    pub(super) fn close_media_preview(&mut self, ctx: &egui::Context) {
        self.media_preview.active = None;
        if let Some(worker) = &self.media_preview.worker {
            worker.generation.fetch_add(1, Ordering::Release);
            worker.pending.store(None);
            worker.result.store(None);
        }
        ctx.request_repaint();
    }
    pub(super) fn media_preview_keyboard(&mut self, ctx: &egui::Context) -> bool {
        if self.media_preview.active.is_none() {
            return false;
        }
        if ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Escape)) {
            self.close_media_preview(ctx);
            return true;
        }
        let direction = ctx.input_mut(|i| {
            if i.consume_key(egui::Modifiers::NONE, egui::Key::ArrowLeft) {
                -1
            } else if i.consume_key(egui::Modifiers::NONE, egui::Key::ArrowRight) {
                1
            } else {
                0
            }
        });
        if direction != 0
            && let Some(active) = &self.media_preview.active
            && !active.loading
        {
            let cursor = if direction < 0 {
                active.previous
            } else {
                active.next
            };
            if let Some(cursor) = cursor {
                self.request_media_page(ctx, cursor);
            }
        }
        let zoom = ctx.input_mut(|i| {
            if i.consume_key(egui::Modifiers::NONE, egui::Key::Minus) {
                0.8
            } else if i.consume_key(egui::Modifiers::NONE, egui::Key::Equals) {
                1.25
            } else {
                1.0
            }
        });
        if zoom != 1.0
            && let Some(active) = &mut self.media_preview.active
        {
            active.zoom = (active.zoom * zoom).clamp(1.0, 16.0);
            ctx.request_repaint();
        }
        true
    }
    fn accept_media_preview_completion(&mut self, ctx: &egui::Context) {
        if let Some(worker) = &self.media_preview.worker {
            if let Some(completion) = worker.result.swap(None) {
                if completion.generation != worker.generation.load(Ordering::Acquire) {
                    return;
                }
                let Ok(completion) = Arc::try_unwrap(completion) else {
                    return;
                };
                let Some(active) = &mut self.media_preview.active else {
                    return;
                };
                active.loading = false;
                if completion.revision != self.source.content_revision() {
                    active.error =
                        Some("The book content changed. Please reopen the preview.".into());
                    return;
                }
                match completion.result {
                    Ok(ready) => {
                        active.size = egui::vec2(ready.size[0] as f32, ready.size[1] as f32);
                        active.texture = Some(ctx.load_texture(
                            "media-preview",
                            ready.image,
                            egui::TextureOptions::LINEAR,
                        ));
                        active.captions = ready.captions;
                        active.caption_textures = ready
                            .caption_images
                            .into_iter()
                            .map(|image| {
                                ctx.load_texture(
                                    "media-caption",
                                    image,
                                    egui::TextureOptions::LINEAR,
                                )
                            })
                            .collect();
                        active.previous = ready.previous;
                        active.next = ready.next;
                    }
                    Err(error) => active.error = Some(error),
                }
            }
        }
    }

    pub(super) fn media_preview_overlay(&mut self, ctx: &egui::Context) {
        self.accept_media_preview_completion(ctx);
        let language = self.language;
        let Some(active) = &mut self.media_preview.active else {
            return;
        };
        let screen = crate::ui::overlay_rect(ctx);
        let available = screen.shrink(32.0);
        let mut close = false;
        let mut navigate = None;
        #[cfg(target_os = "windows")]
        crate::app::window_chrome::dim_controls(ctx, 190.0 / 255.0);
        egui::Area::new("reader-media-preview".into())
            .order(egui::Order::Tooltip)
            .fade_in(false)
            .fixed_pos(ctx.content_rect().min)
            .constrain(false)
            .show(ctx, |ui| {
                ui.set_clip_rect(ctx.content_rect());
                let (bounds, backdrop) =
                    ui.allocate_exact_size(ctx.content_rect().size(), egui::Sense::click());
                ui.painter()
                    .rect_filled(bounds, 0.0, egui::Color32::from_black_alpha(190));
                if let Some(texture) = &active.texture {
                    let captions: Vec<_> = active
                        .caption_textures
                        .iter()
                        .map(egui::TextureHandle::size_vec2)
                        .collect();
                    let natural = media_paper_size(active.size, &captions);
                    let fit = super::egui_view::preview_fit_scale(
                        natural,
                        available.size().max(egui::Vec2::splat(1.0)),
                    );
                    let wheel = ctx.input(|i| i.smooth_scroll_delta.y);
                    if available
                        .contains(ctx.input(|i| i.pointer.hover_pos()).unwrap_or(screen.min))
                        && wheel != 0.0
                    {
                        active.zoom = (active.zoom * (wheel * 0.002).exp()).clamp(1.0, 16.0);
                    }
                    let scale = fit * active.zoom;
                    let size = natural * scale;
                    let limit = (size - available.size()).max(egui::Vec2::ZERO) * 0.5;
                    active.pan = active.pan.clamp(-limit, limit);
                    let paper = egui::Rect::from_center_size(available.center() + active.pan, size);
                    let painter = ui.painter().with_clip_rect(available);
                    painter.rect_filled(paper, 0.0, egui::Color32::WHITE);
                    let uv = egui::Rect::from_min_max(egui::Pos2::ZERO, egui::pos2(1.0, 1.0));
                    let mut top = paper.top();
                    for (texture, natural_size) in std::iter::once((texture, active.size))
                        .chain(active.caption_textures.iter().zip(captions.iter().copied()))
                    {
                        let size = natural_size * scale;
                        let rect = egui::Rect::from_min_size(
                            egui::pos2(paper.center().x - size.x * 0.5, top),
                            size,
                        );
                        painter.image(texture.id(), rect, uv, egui::Color32::WHITE);
                        top = rect.bottom();
                    }
                    painter.rect_stroke(
                        paper,
                        2.0,
                        egui::Stroke::new(1.0, egui::Color32::from_white_alpha(48)),
                        egui::StrokeKind::Inside,
                    );
                    let visible = paper.intersect(available);
                    let response = ui
                        .interact(
                            visible,
                            ui.id().with("paper"),
                            egui::Sense::click_and_drag(),
                        )
                        .on_hover_cursor(egui::CursorIcon::Grab);
                    if response.dragged() {
                        active.pan += ctx.input(|i| i.pointer.delta());
                    }
                    if response.double_clicked() {
                        active.zoom = 1.0;
                        active.pan = egui::Vec2::ZERO;
                    }
                    let pointer = ctx.input(|i| i.pointer.hover_pos());
                    let mut over_navigation = false;
                    // Navigation appears only for multipart media, near the hovered edge.
                    // Ordinary single-image/table previews have no toolbar or buttons.
                    for (cursor, x, symbol, hint, id) in [
                        (
                            active.previous,
                            (paper.left() - 24.0).max(screen.left() + 24.0),
                            "‹",
                            language.text("上一部分（←）", "Previous part (←)"),
                            "previous",
                        ),
                        (
                            active.next,
                            (paper.right() + 24.0).min(screen.right() - 24.0),
                            "›",
                            language.text("下一部分（→）", "Next part (→)"),
                            "next",
                        ),
                    ] {
                        let Some(cursor) = cursor.filter(|_| !active.loading) else {
                            continue;
                        };
                        let rect = egui::Rect::from_center_size(
                            egui::pos2(x, available.center().y),
                            egui::vec2(40.0, 64.0),
                        );
                        if !pointer.is_some_and(|p| rect.expand(20.0).contains(p)) {
                            continue;
                        }
                        let response = ui
                            .interact(rect, ui.id().with(id), egui::Sense::click())
                            .on_hover_cursor(egui::CursorIcon::PointingHand)
                            .on_hover_text(hint);
                        ui.painter().text(
                            rect.center(),
                            egui::Align2::CENTER_CENTER,
                            symbol,
                            egui::FontId::proportional(36.0),
                            egui::Color32::WHITE,
                        );
                        over_navigation |= response.hovered();
                        if response.clicked() {
                            navigate = Some(cursor);
                        }
                    }
                    close |= backdrop.clicked()
                        && !over_navigation
                        && backdrop
                            .interact_pointer_pos()
                            .is_some_and(|p| !visible.contains(p));
                } else {
                    let message = if active
                        .error
                        .as_ref()
                        .is_some_and(|error| error.contains("several"))
                    {
                        language.text(
                            "存在多个同号图表，无法确定对应内容",
                            "Several figures/tables share this number",
                        )
                    } else if active.error.is_some() {
                        language.text(
                            "未能预览对应图表，请关闭后重试",
                            "Unable to preview this figure/table",
                        )
                    } else {
                        language.text("正在加载…", "Loading…")
                    };
                    ui.painter().text(
                        available.center(),
                        egui::Align2::CENTER_CENTER,
                        message,
                        egui::FontId::proportional(16.0),
                        egui::Color32::WHITE,
                    );
                    close |= backdrop.clicked();
                }
            });
        if close {
            self.close_media_preview(ctx);
        } else if let Some(cursor) = navigate {
            self.request_media_page(ctx, cursor);
        }
    }
}

fn media_paper_size(image: egui::Vec2, captions: &[egui::Vec2]) -> egui::Vec2 {
    egui::vec2(
        captions
            .iter()
            .fold(image.x, |width, caption| width.max(caption.x))
            .max(1.0),
        (image.y + captions.iter().map(|caption| caption.y).sum::<f32>()).max(1.0),
    )
}

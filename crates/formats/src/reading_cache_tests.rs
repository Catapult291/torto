use super::*;
use crate::epub::tests::{minimal_entries, minimal_epub, note_section_epub, zip_entries};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

#[test]
#[ignore = "requires TORTO_PERF_BOOKS and TORTO_PERF_PROBE_ROOT; local library audit"]
fn local_source_cache_phase_probe() {
    let books: serde_json::Value = serde_json::from_slice(
        &fs::read(std::env::var("TORTO_PERF_BOOKS").unwrap()).unwrap(),
    ).unwrap();
    let root = PathBuf::from(std::env::var("TORTO_PERF_PROBE_ROOT").unwrap());
    let library = PathBuf::from("C:/Users/Chris/AppData/Local/Rebook/Rebook/data/library/books");
    let mut output = Vec::new();
    let sections = [20, 10, 16];
    for (book, section) in books.as_array().unwrap().iter().zip(sections) {
        let name = book["storage_name"].as_str().unwrap();
        let path = library.join(name);
        let known_id = book["id"].as_str().unwrap();
        let opened = open(&path, Some(known_id), &root).unwrap();
        opened.source().parse_section(section).unwrap();
        drain();
        let bytes = fs::read(&path).unwrap();
        let digest = *blake3::hash(&bytes).as_bytes();
        let entry = root.join(version_directory()).join(format!("epub-{}", entry_key(&digest, name)));
        let mut samples = Vec::new();
        for _ in 0..5 {
            let mut phases = serde_json::Map::new();
            macro_rules! timed {
                ($name:literal, $action:expr) => {{
                    let started = Instant::now();
                    let value = $action;
                    phases.insert($name.into(), (started.elapsed().as_secs_f64()*1000.0).into());
                    value
                }};
            }
            let file_bytes = timed!("file_read_ms", fs::read(&path).unwrap());
            let actual_digest: [u8;32] = timed!("sha256_ms", Sha256::digest(&file_bytes).into());
            assert_eq!(hex_digest(&actual_digest), known_id);
            let _ = timed!("blake3_ms", blake3::hash(&file_bytes));
            let index_bytes = timed!("index_file_ms", fs::read(entry.join("index.bin")).unwrap());
            let mut index_json = Vec::new();
            timed!("index_inflate_ms", ZlibDecoder::new(&index_bytes[HEADER_BYTES..]).read_to_end(&mut index_json).unwrap());
            let _: Index = timed!("index_json_ms", serde_json::from_slice(&index_json).unwrap());
            let section_bytes = timed!("section_file_ms", fs::read(entry.join(format!("section-{section}.bin"))).unwrap());
            let mut section_json = Vec::new();
            timed!("section_inflate_ms", ZlibDecoder::new(&section_bytes[HEADER_BYTES..]).read_to_end(&mut section_json).unwrap());
            let parsed: Section = timed!("section_json_ms", serde_json::from_slice(&section_json).unwrap());
            let fresh = timed!("uncached_index_ms", EpubPublication::open_bytes_with_id(
                Arc::from(file_bytes), rebook_publication::PublicationId::new(known_id).unwrap()).unwrap());
            let original = timed!("uncached_section_ms", fresh.parse_section(section).unwrap());
            assert_eq!(parsed, original);
            phases.insert("section_json_bytes".into(), section_json.len().into());
            phases.insert("index_json_bytes".into(), index_json.len().into());
            phases.insert("blocks".into(), parsed.blocks.len().into());
            phases.insert("anchors".into(), parsed.anchors.len().into());
            samples.push(phases);
        }
        output.push(serde_json::json!({"book":book["title"], "profile":"dev", "samples":samples}));
    }
    fs::write(root.join("source-phases.json"), serde_json::to_vec_pretty(&output).unwrap()).unwrap();
    println!("SOURCE_PHASES {}", serde_json::to_string(&output).unwrap());
}

static NEXT_TEMP: AtomicU64 = AtomicU64::new(0);
static SERIAL: Mutex<()> = Mutex::new(());
struct Temp(PathBuf);
impl Temp {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "torto-reading-ir-{}-{}",
            std::process::id(),
            NEXT_TEMP.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&path).unwrap();
        Self(path)
    }
}
impl Drop for Temp {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn entry(root: &Path, bytes: &[u8], format: &str) -> PathBuf {
    root.join(version_directory()).join(format!(
        "{format}-{}",
        entry_key(blake3::hash(bytes).as_bytes(), &format!("book.{format}"))
    ))
}

fn drain() {
    let started = Instant::now();
    while WRITER
        .get()
        .and_then(Option::as_ref)
        .is_some_and(|writer| !writer.pending.lock().unwrap().is_empty())
    {
        assert!(
            started.elapsed() < Duration::from_secs(10),
            "cache writer did not finish"
        );
        std::thread::sleep(Duration::from_millis(2));
    }
}

#[test]
fn restores_index_and_chapters_without_initializing_original_and_keeps_resources_lazy() {
    let _guard = SERIAL.lock().unwrap();
    let temp = Temp::new();
    let bytes = minimal_epub();
    let path = temp.0.join("book.epub");
    let root = temp.0.join("cache");
    fs::write(&path, &bytes).unwrap();
    let fresh = open(&path, None, &root).unwrap();
    let section = fresh.source().parse_section(0).unwrap();
    drain();
    let digest = *blake3::hash(&bytes).as_bytes();
    let directory = entry(&root, &bytes, "epub");
    let index: Index = read(&directory.join("index.bin"), &digest).unwrap();
    let cached = CachedSource {
        index,
        original: OnceLock::new(),
        bytes: Mutex::new(Some(bytes.clone().into())),
        name: "book.epub".into(),
        format: BookFormat::Epub,
        image_dimensions: ImageDimensions::open(&directory, &root, digest),
        entry: directory,
        root: root.clone(),
        digest,
        section_fingerprints: Mutex::new(HashMap::new()),
    };
    assert_eq!(cached.book(), fresh.book());
    assert_eq!(cached.parse_section(0).unwrap(), section);
    assert_eq!(cached.book().id.as_str(), hex_digest(&Sha256::digest(&bytes).into()));
    assert_eq!(cached.cached_section_fingerprint(0), Some(Sha256::digest(serde_json::to_vec(&section).unwrap()).into()));
    let chapter_file = cached.entry.join("section-0.bin");
    let mut damaged = fs::read(&chapter_file).unwrap();
    damaged[52] ^= 1;
    fs::write(&chapter_file, damaged).unwrap();
    assert!(read_entry::<Section>(&chapter_file, &digest).is_err(), "fingerprint corruption must be detected");
    assert!(
        cached.original.get().is_none(),
        "warm chapter reopened EPUB parser"
    );
    let cover = fresh.book().cover.as_ref().unwrap();
    assert_eq!(
        cached.resource(cover).unwrap().bytes,
        fresh.source().resource(cover).unwrap().bytes
    );
    assert!(cached.original.get().is_some());
    assert!(cached.bytes.lock().unwrap().is_none());
    // Removing one entry regenerates only that chapter using the cached descriptor.
    fs::remove_file(cached.entry.join("section-0.bin")).unwrap();
    assert_eq!(cached.parse_section(0).unwrap(), section);
    drain();
}

#[test]
fn round_trip_preserves_styles_structural_groups_and_source_anchors() {
    let _guard = SERIAL.lock().unwrap();
    let temp = Temp::new();
    let chapter = br#"<html><head><style>em {color:#246; font-size:small} td {text-align:right}</style></head><body>
      <h1 id="title">Heading</h1><p id="intro">Items follow:</p><ul><li><em>Emphasis</em> and <i>term</i></li><li>Second</li></ul>
      <p id="body">Ruby <ruby>base<rt>annotation</rt></ruby> <a href="chapter.xhtml#title"><sup>1</sup></a>
      <img src="../Images/cover.png" width="12" height="12" alt="inline"/></p>
      <table><caption>Table 1. Values</caption><tr><th>Heading</th><td colspan="2">Value</td></tr></table>
      <figure><img src="../Images/cover.png"/><figcaption>Figure 1. Image</figcaption></figure></body></html>"#;
    let mut entries = minimal_entries();
    let position = entries
        .iter()
        .position(|e| e.0.ends_with("chapter.xhtml"))
        .unwrap();
    entries[position].1 = chapter;
    let bytes = zip_entries(&entries);
    let source = EpubPublication::open_bytes(bytes).unwrap();
    let section = source.parse_section(0).unwrap();
    assert!(!section.anchors.is_empty());
    assert!(section.blocks.iter().any(|b| matches!(b, Block::Table(_))));
    let digest = [42; 32];
    let path = temp.0.join("section.bin");
    write(&path, &digest, &section).unwrap();
    assert_eq!(read::<Section>(&path, &digest).unwrap(), section);
}

#[test]
fn notes_and_durable_positions_are_identical_after_reopen() {
    let _guard = SERIAL.lock().unwrap();
    let temp = Temp::new();
    let path = temp.0.join("notes.epub");
    let root = temp.0.join("cache");
    fs::write(&path, note_section_epub()).unwrap();
    let fresh = open(&path, None, &root).unwrap();
    let before = (0..fresh.book().sections.len())
        .map(|i| {
            let section = fresh.source().parse_section(i).unwrap();
            drain();
            section
        })
        .collect::<Vec<_>>();
    assert!(fresh.book().sections[1].is_note_section());
    let cached = open(&path, Some(fresh.book().id.as_str()), &root).unwrap();
    assert_eq!(cached.book(), fresh.book());
    for (i, original) in before.iter().enumerate() {
        assert_eq!(&cached.source().parse_section(i).unwrap(), original);
    }
    drain();
}

#[test]
fn corruption_and_source_replacement_fall_back_to_current_parser() {
    let _guard = SERIAL.lock().unwrap();
    let temp = Temp::new();
    let path = temp.0.join("book.epub");
    let root = temp.0.join("cache");
    let bytes = minimal_epub();
    fs::write(&path, &bytes).unwrap();
    let fresh = open(&path, None, &root).unwrap();
    let before = fresh.source().parse_section(0).unwrap();
    drain();
    let directory = entry(&root, &bytes, "epub");
    fs::write(directory.join("section-0.bin"), b"truncated").unwrap();
    assert_eq!(
        open(&path, None, &root)
            .unwrap()
            .source()
            .parse_section(0)
            .unwrap(),
        before
    );
    drain();
    fs::write(directory.join("index.bin"), b"corrupt").unwrap();
    assert_eq!(open(&path, None, &root).unwrap().book(), fresh.book());
    drain();
    let changed = br#"<html><body><p id="changed">Replacement book contents.</p></body></html>"#;
    let mut entries = minimal_entries();
    let position = entries
        .iter()
        .position(|e| e.0.ends_with("chapter.xhtml"))
        .unwrap();
    entries[position].1 = changed;
    fs::write(&path, zip_entries(&entries)).unwrap();
    // Even an old shelf ID cannot force stale content at the same file path.
    let replacement = open(&path, Some(fresh.book().id.as_str()), &root).unwrap();
    assert_ne!(replacement.book().id, fresh.book().id);
    assert_ne!(replacement.source().parse_section(0).unwrap(), before);
    drain();
}

#[test]
fn fb2_uses_the_same_cache_and_unwritable_cache_does_not_prevent_reading() {
    let _guard = SERIAL.lock().unwrap();
    let temp = Temp::new();
    let bytes = br#"<?xml version="1.0"?><FictionBook xmlns="http://www.gribuser.ru/xml/fictionbook/2.0"><description><title-info><book-title>FB2 test</book-title><lang>en</lang></title-info></description><body><section><title><p>Chapter</p></title><p>Reading text.</p></section></body></FictionBook>"#;
    let path = temp.0.join("book.fb2");
    fs::write(&path, bytes).unwrap();
    let root = temp.0.join("cache");
    let fresh = open(&path, None, &root).unwrap();
    let before = fresh.source().parse_section(0).unwrap();
    drain();
    assert!(entry(&root, bytes, "fb2").join("index.bin").is_file());
    assert_eq!(
        open(&path, None, &root)
            .unwrap()
            .source()
            .parse_section(0)
            .unwrap(),
        before
    );
    let blocked = temp.0.join("not-a-directory");
    fs::write(&blocked, b"occupied").unwrap();
    assert_eq!(
        open(&path, None, &blocked)
            .unwrap()
            .source()
            .parse_section(0)
            .unwrap(),
        before
    );
    drain();
}

#[test]
fn validates_binary_envelope_and_atomically_replaces_existing_entries() {
    let _guard = SERIAL.lock().unwrap();
    let temp = Temp::new();
    let path = temp.0.join("test.bin");
    let digest = [1; 32];
    write(&path, &digest, &vec!["first"]).unwrap();
    write(&path, &digest, &vec!["second"]).unwrap();
    assert_eq!(read::<Vec<String>>(&path, &digest).unwrap(), vec!["second"]);
    assert!(read::<Vec<String>>(&path, &[2; 32]).is_err());
    let mut bytes = fs::read(&path).unwrap();
    *bytes.last_mut().unwrap() ^= 1;
    fs::write(&path, &bytes).unwrap();
    assert!(read::<Vec<String>>(&path, &digest).is_err());
    // Reject a huge claimed decompressed size before allocating or decoding.
    bytes[44..52].copy_from_slice(&u64::MAX.to_le_bytes());
    fs::write(&path, &bytes).unwrap();
    assert!(read::<Vec<String>>(&path, &digest).is_err());
    bytes[44..52].copy_from_slice(&2u64.to_le_bytes());
    fs::write(&path, &bytes).unwrap();
    assert!(read::<Vec<String>>(&path, &digest).is_err());
}

#[test]
fn disk_budget_covers_all_versions_and_eviction_is_recoverable() {
    let _guard = SERIAL.lock().unwrap();
    let temp = Temp::new();
    let old = temp.0.join("v0/book/section-0.bin");
    let new = temp.0.join("v1/book/index.bin");
    fs::create_dir_all(old.parent().unwrap()).unwrap();
    fs::create_dir_all(new.parent().unwrap()).unwrap();
    fs::write(&old, [0u8; 80]).unwrap();
    fs::OpenOptions::new()
        .write(true)
        .open(&old)
        .unwrap()
        .set_times(
            fs::FileTimes::new().set_modified(std::time::UNIX_EPOCH + Duration::from_secs(1)),
        )
        .unwrap();
    fs::write(&new, [0u8; 40]).unwrap();
    prune_with_budget(&temp.0, 100);
    assert!(!old.exists());
    assert!(new.exists());
}

fn png(width: u32, height: u32) -> Vec<u8> {
    let mut bytes = std::io::Cursor::new(Vec::new());
    image::DynamicImage::ImageRgba8(image::RgbaImage::from_pixel(
        width,
        height,
        image::Rgba([20, 80, 40, 255]),
    ))
    .write_to(&mut bytes, image::ImageFormat::Png)
    .unwrap();
    bytes.into_inner()
}

fn image_epub(width: u32, height: u32) -> Vec<u8> {
    let image = png(width, height);
    let mut entries = minimal_entries();
    entries
        .iter_mut()
        .find(|entry| entry.0 == "OPS/Images/cover.png")
        .unwrap()
        .1 = &image;
    zip_entries(&entries)
}

#[test]
fn intrinsic_dimensions_survive_reopen_without_reopening_the_resource_source() {
    let _guard = SERIAL.lock().unwrap();
    let temp = Temp::new();
    let root = temp.0.join("cache");
    let path = temp.0.join("book.epub");
    let bytes = image_epub(240, 160);
    fs::write(&path, &bytes).unwrap();
    let publication = open(&path, None, &root).unwrap();
    let href = publication.book().cover.as_ref().unwrap();
    assert_eq!(
        publication
            .source()
            .image_resource_dimensions(href)
            .unwrap(),
        Some([240, 160])
    );
    drain();
    let digest = *blake3::hash(&bytes).as_bytes();
    let directory = entry(&root, &bytes, "epub");
    let index = read::<Index>(&directory.join("index.bin"), &digest).unwrap();
    let sizes = ImageDimensions::open(&directory, &root, digest);
    assert_eq!(sizes.get(href), Some(Some([240, 160])));
    let cached = CachedSource {
        index,
        original: OnceLock::new(),
        bytes: Mutex::new(Some(bytes.into())),
        name: "book.epub".into(),
        format: BookFormat::Epub,
        entry: directory,
        root,
        digest,
        image_dimensions: sizes,
        section_fingerprints: Mutex::new(HashMap::new()),
    };
    assert_eq!(
        cached.image_resource_dimensions(href).unwrap(),
        Some([240, 160])
    );
    assert!(
        cached.original.get().is_none(),
        "cached dimensions loaded image bytes"
    );
    assert_eq!(
        cached.raster_resource_dimensions(href).unwrap(),
        None,
        "encoded images must not become source-owned decoded rasters"
    );
}

#[test]
fn damaged_dimensions_fall_back_and_replacing_the_book_invalidates_old_sizes() {
    let _guard = SERIAL.lock().unwrap();
    let temp = Temp::new();
    let root = temp.0.join("cache");
    let path = temp.0.join("book.epub");
    let bytes = image_epub(240, 160);
    fs::write(&path, &bytes).unwrap();
    let publication = open(&path, None, &root).unwrap();
    let href = publication.book().cover.as_ref().unwrap();
    assert_eq!(
        publication
            .source()
            .image_resource_dimensions(href)
            .unwrap(),
        Some([240, 160])
    );
    drain();
    fs::write(
        entry(&root, &bytes, "epub").join("image-dimensions.bin"),
        b"damaged",
    )
    .unwrap();
    assert_eq!(
        open(&path, None, &root)
            .unwrap()
            .source()
            .image_resource_dimensions(href)
            .unwrap(),
        Some([240, 160])
    );
    drain();
    fs::write(&path, image_epub(80, 60)).unwrap();
    assert_eq!(
        open(&path, Some(publication.book().id.as_str()), &root)
            .unwrap()
            .source()
            .image_resource_dimensions(href)
            .unwrap(),
        Some([80, 60])
    );
    drain();
}

#[test]
fn coalesced_image_metadata_writes_keep_the_last_updates_and_stay_bounded() {
    let _guard = SERIAL.lock().unwrap();
    let temp = Temp::new();
    let cache = ImageDimensions::open(&temp.0.join("v1/book"), &temp.0, [4; 32]);
    let bytes = png(3, 2);
    for index in 0..600 {
        let href = PublicationUrl::parse(&format!("image-{index:04}.png")).unwrap();
        assert_eq!(cache.record(&href, &bytes), Some([3, 2]));
    }
    let last = PublicationUrl::parse("image-0599.png").unwrap();
    let start = Instant::now();
    loop {
        cache.schedule_if_dirty();
        drain();
        let restored = ImageDimensions::open(&temp.0.join("v1/book"), &temp.0, [4; 32]);
        if restored.get(&last) == Some(Some([3, 2])) {
            break;
        }
        assert!(start.elapsed() < Duration::from_secs(5));
    }
    assert!(
        fs::metadata(temp.0.join("v1/book/image-dimensions.bin"))
            .unwrap()
            .len()
            < 64 * 1024
    );
    // Unsupported/corrupt headers preserve the normal decoder fallback.
    let invalid = PublicationUrl::parse("unsupported.svg").unwrap();
    assert_eq!(cache.record(&invalid, b"<svg/>"), None);
    assert_eq!(cache.get(&invalid), Some(None));
    drain();
}

#[test]
#[ignore = "requires TORTO_CACHE_AUDIT_BOOK; reads a real local EPUB without changing it"]
fn local_book_structure_cache_fidelity() {
    let path = std::env::var_os("TORTO_CACHE_AUDIT_BOOK").expect("local book path");
    let bytes: Arc<[u8]> = fs::read(path).unwrap().into();
    let digest = Sha256::digest(&bytes).into();
    let fresh = EpubPublication::open_bytes(Arc::clone(&bytes)).unwrap();
    let restored = EpubPublication::from_reading_index(bytes, fresh.reading_index()).unwrap();
    assert_eq!(fresh.book(), restored.book());
    let temp = Temp::new();
    let mut total_bytes = 0;
    let mut blocks = 0;
    let mut anchors = 0;
    let mut max_weight = 0;
    let mut skipped = Vec::new();
    for index in 0..fresh.book().sections.len() {
        let original = fresh.parse_section(index).unwrap();
        let weight = section_weight(&original);
        max_weight = max_weight.max(weight);
        if weight > MAX_QUEUED_SECTION_WEIGHT {
            skipped.push(index);
        }
        assert_eq!(
            restored.parse_section(index).unwrap(),
            original,
            "descriptor restoration changed section {index}"
        );
        let path = temp.0.join(format!("{index}.bin"));
        write(&path, &digest, &original).unwrap();
        assert_eq!(
            read::<Section>(&path, &digest).unwrap(),
            original,
            "binary cache changed section {index}"
        );
        total_bytes += fs::metadata(path).unwrap().len();
        blocks += original.blocks.len();
        anchors += original.anchors.len();
    }
    println!(
        "IR_FIDELITY {}",
        serde_json::json!({
            "book": fresh.book().metadata.title, "sections": fresh.book().sections.len(),
        "blocks": blocks, "anchors": anchors, "section_cache_bytes": total_bytes,
        "max_queue_weight": max_weight, "oversized_sections": skipped,
        })
    );
}

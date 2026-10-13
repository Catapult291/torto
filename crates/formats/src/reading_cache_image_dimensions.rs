//! One small, coalesced metadata file per book. Neither pixels nor encoded
//! resource bytes are retained. Unsupported headers use the normal fallback.
use super::*;
use std::collections::BTreeMap;
use std::io::Cursor;

const MAX_IMAGES: usize = 512;
const MAX_URI_BYTES: usize = 4096;

#[derive(Default, Serialize, Deserialize)]
struct Stored {
    images: BTreeMap<String, Option<[u32; 2]>>,
}
struct State {
    images: BTreeMap<String, Option<[u32; 2]>>,
    version: u64,
    written: u64,
}
pub(super) struct ImageDimensions {
    path: PathBuf,
    root: PathBuf,
    digest: [u8; 32],
    state: Mutex<State>,
}

impl ImageDimensions {
    pub(super) fn open(entry: &Path, root: &Path, digest: [u8; 32]) -> Arc<Self> {
        let path = entry.join("image-dimensions.bin");
        let stored = read::<Stored>(&path, &digest)
            .ok()
            .filter(|stored| {
                stored.images.len() <= MAX_IMAGES
                    && stored.images.iter().all(|(uri, dimensions)| {
                        uri.len() <= MAX_URI_BYTES
                            && PublicationUrl::parse(uri)
                                .is_ok_and(|href| href.resource_url().to_string() == *uri)
                            && dimensions.is_none_or(|[width, height]| width > 0 && height > 0)
                    })
            })
            .unwrap_or_default();
        Arc::new(Self {
            path,
            root: root.to_owned(),
            digest,
            state: Mutex::new(State {
                images: stored.images,
                version: 0,
                written: 0,
            }),
        })
    }

    pub(super) fn get(self: &Arc<Self>, href: &PublicationUrl) -> Option<Option<[u32; 2]>> {
        let result = self
            .state
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .images
            .get(&href.resource_url().to_string())
            .copied();
        self.schedule_if_dirty();
        result
    }

    pub(super) fn record(
        self: &Arc<Self>,
        href: &PublicationUrl,
        bytes: &[u8],
    ) -> Option<[u32; 2]> {
        if let Some(dimensions) = self.get(href) {
            return dimensions;
        }
        let dimensions = image::ImageReader::new(Cursor::new(bytes))
            .with_guessed_format()
            .ok()
            .and_then(|reader| reader.into_dimensions().ok())
            .filter(|(width, height)| *width > 0 && *height > 0)
            .map(|(width, height)| [width, height]);
        let uri = href.resource_url().to_string();
        if uri.len() > MAX_URI_BYTES {
            return dimensions;
        }
        {
            let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
            if state.images.len() >= MAX_IMAGES && !state.images.contains_key(&uri) {
                state.images.pop_first();
            }
            state.images.insert(uri, dimensions);
            state.version = state.version.wrapping_add(1);
        }
        self.schedule_if_dirty();
        dimensions
    }

    pub(super) fn schedule_if_dirty(self: &Arc<Self>) {
        let dirty = {
            let state = self.state.lock().unwrap_or_else(|e| e.into_inner());
            state.version != state.written
        };
        if dirty {
            enqueue(self.path.clone(), self.root.clone(), self.digest, || {
                Payload::ImageDimensions(Arc::clone(self))
            });
        }
    }

    pub(super) fn persist(&self) -> io::Result<()> {
        let (stored, version) = {
            let state = self.state.lock().unwrap_or_else(|e| e.into_inner());
            (
                Stored {
                    images: state.images.clone(),
                },
                state.version,
            )
        };
        write(&self.path, &self.digest, &stored)?;
        self.state.lock().unwrap_or_else(|e| e.into_inner()).written = version;
        Ok(())
    }
}

//! Decodes images off-thread and tracks the Kitty image ids used for them.

use std::collections::{HashMap, HashSet};
use std::hash::{DefaultHasher, Hash, Hasher};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::SystemTime;

use image::RgbaImage;

use super::{protocol, GraphicsCaps};

const MAX_FILE_BYTES: u64 = 50 * 1024 * 1024;
const MAX_ID: u32 = 0xFF_FFFF;
/// Decoded RGBA bytes kept for images that no view currently shows.
const PIXEL_BUDGET: usize = 128 * 1024 * 1024;

/// What an image shows.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum ImageSource {
    File(PathBuf),
    /// A Mermaid diagram registered with [`ImageStore::diagram`], in the
    /// dark or light variant.
    Diagram {
        hash: u64,
        dark: bool,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ImageState {
    /// Missing, unreadable, undecodable, or graphics unsupported.
    Unavailable,
    Loading,
    Ready {
        width: u32,
        height: u32,
    },
    /// A diagram that failed to render, with a one-line message.
    Error(String),
}

/// Diagrams have no file; their entries use one fixed "mtime".
const DIAGRAM_MTIME: SystemTime = SystemTime::UNIX_EPOCH;

enum Entry {
    Loading {
        mtime: SystemTime,
    },
    Ready {
        mtime: SystemTime,
        /// Evicted once no id uses the image and the store is over its
        /// budget, see [`ImageStore::retain_ids`].
        image: Option<Arc<RgbaImage>>,
        width: u32,
        height: u32,
        last_used: u64,
    },
    Failed {
        mtime: Option<SystemTime>,
        /// The message of a failed diagram.
        error: Option<String>,
    },
}

impl Entry {
    fn mtime(&self) -> Option<SystemTime> {
        match self {
            Entry::Loading { mtime } | Entry::Ready { mtime, .. } => Some(*mtime),
            Entry::Failed { mtime, .. } => *mtime,
        }
    }

    fn state(&self) -> ImageState {
        match self {
            Entry::Loading { .. } => ImageState::Loading,
            Entry::Ready { width, height, .. } => ImageState::Ready {
                width: *width,
                height: *height,
            },
            Entry::Failed {
                error: Some(error), ..
            } => ImageState::Error(error.clone()),
            Entry::Failed { error: None, .. } => ImageState::Unavailable,
        }
    }
}

type Decoded = (ImageSource, SystemTime, Result<RgbaImage, String>);
/// (source with canonical path, mtime, cols, rows)
type IdKey = (ImageSource, SystemTime, u16, u16);

pub struct ImageStore {
    caps: GraphicsCaps,
    sync_decode: bool,
    entries: HashMap<ImageSource, Entry>,
    /// The text of registered diagrams, by hash.
    diagram_sources: HashMap<u64, Arc<str>>,
    /// (source, mtime, cols, rows) -> (id, transmitted)
    ids: HashMap<IdKey, (u32, bool)>,
    next_id: u32,
    /// Complete escape sequences, wrapped for tmux when taken.
    pending: Vec<Vec<u8>>,
    generation: u64,
    finished: Arc<Mutex<Vec<Decoded>>>,
    pixel_budget: usize,
    /// Advanced on every use of an image, for least-recently-used eviction.
    tick: u64,
    /// The terminal's reported light/dark mode.
    theme_mode: Option<crate::theme::Mode>,
}

impl Default for ImageStore {
    fn default() -> Self {
        Self {
            caps: GraphicsCaps::default(),
            sync_decode: false,
            entries: HashMap::new(),
            diagram_sources: HashMap::new(),
            ids: HashMap::new(),
            next_id: id_seed(),
            pending: Vec::new(),
            generation: 0,
            finished: Arc::default(),
            pixel_budget: PIXEL_BUDGET,
            tick: 0,
            theme_mode: None,
        }
    }
}

/// A per-process starting point for ids, so that Helix instances sharing a
/// terminal's image store (tmux panes) rarely pick the same ids.
fn id_seed() -> u32 {
    let nanos = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map_or(0, |time| time.subsec_nanos() ^ time.as_secs() as u32);
    (std::process::id().wrapping_mul(0x9E37_79B9) ^ nanos) % MAX_ID
}

fn canonical(path: &Path) -> Option<PathBuf> {
    std::fs::canonicalize(path).ok()
}

fn decode(path: &Path, max_px: (u32, u32)) -> Result<RgbaImage, String> {
    let mut reader = image::ImageReader::open(path)
        .and_then(|reader| reader.with_guessed_format())
        .map_err(|err| err.to_string())?;
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(16384);
    limits.max_image_height = Some(16384);
    limits.max_alloc = Some(512 * 1024 * 1024);
    reader.limits(limits);
    let mut img = reader.decode().map_err(|err| err.to_string())?;
    if img.width() > max_px.0 || img.height() > max_px.1 {
        img = img.resize(
            max_px.0.max(1),
            max_px.1.max(1),
            image::imageops::FilterType::Triangle,
        );
    }
    Ok(img.to_rgba8())
}

enum Job {
    File(PathBuf, (u32, u32)),
    Diagram {
        source: Arc<str>,
        dark: bool,
        cell_px: (u16, u16),
        max_px: (u32, u32),
    },
}

impl Job {
    fn run(self) -> Result<RgbaImage, String> {
        match self {
            Job::File(path, max_px) => decode(&path, max_px),
            Job::Diagram {
                source,
                dark,
                cell_px,
                max_px,
            } => super::diagram::render(&source, dark, cell_px, max_px),
        }
    }
}

impl ImageStore {
    pub fn caps(&self) -> GraphicsCaps {
        self.caps
    }

    /// The terminal's reported light/dark mode, for themes without a
    /// background colour.
    pub fn theme_mode(&self) -> Option<crate::theme::Mode> {
        self.theme_mode
    }

    pub fn set_theme_mode(&mut self, mode: Option<crate::theme::Mode>) {
        if mode != self.theme_mode {
            self.theme_mode = mode;
            helix_event::request_redraw();
        }
    }

    pub fn set_caps(&mut self, caps: GraphicsCaps) {
        if caps != self.caps {
            self.generation += 1;
        }
        self.caps = caps;
    }

    /// Updates the cell and terminal sizes after a resize, keeping the
    /// previous cell size when the platform reports none.
    pub fn resize(&mut self, cell_px: Option<(u16, u16)>, term_cells: (u16, u16)) {
        let caps = self.caps;
        self.set_caps(GraphicsCaps {
            cell_px: cell_px.unwrap_or(caps.cell_px),
            term_cells,
            ..caps
        });
    }

    pub fn set_sync_decode(&mut self, sync: bool) {
        self.sync_decode = sync;
    }

    pub fn generation(&self) -> u64 {
        self.generation
    }

    fn max_px(&self) -> (u32, u32) {
        let caps = &self.caps;
        (
            caps.cell_px.0 as u32 * caps.term_cells.0 as u32,
            caps.cell_px.1 as u32 * caps.term_cells.1 as u32 / 2,
        )
    }

    fn queue_delete(&mut self, id: u32) {
        self.pending.push(protocol::delete(id));
    }

    /// Frees all ids of `key`, optionally only those of one `mtime`.
    fn free_ids(&mut self, key: &ImageSource, mtime: Option<SystemTime>) {
        let freed: Vec<IdKey> = self
            .ids
            .keys()
            .filter(|(k, m, ..)| k == key && mtime.is_none_or(|mtime| mtime == *m))
            .cloned()
            .collect();
        for key in freed {
            if let Some((id, transmitted)) = self.ids.remove(&key) {
                if transmitted {
                    self.queue_delete(id);
                }
            }
        }
    }

    /// Diagrams fit the terminal width and its height minus two rows.
    fn diagram_max_px(&self) -> (u32, u32) {
        let caps = &self.caps;
        (
            caps.cell_px.0 as u32 * caps.term_cells.0 as u32,
            caps.cell_px.1 as u32 * caps.term_cells.1.saturating_sub(2).max(1) as u32,
        )
    }

    /// Registers the text of a Mermaid diagram; [`Self::get`] renders it.
    pub fn diagram(&mut self, source: &str, dark: bool) -> ImageSource {
        let mut hasher = DefaultHasher::new();
        source.hash(&mut hasher);
        let hash = hasher.finish();
        self.diagram_sources
            .entry(hash)
            .or_insert_with(|| source.into());
        ImageSource::Diagram { hash, dark }
    }

    /// The entry key of `source`: files by canonical path.
    fn key(source: &ImageSource) -> Option<ImageSource> {
        match source {
            ImageSource::File(path) => canonical(path).map(ImageSource::File),
            diagram => Some(diagram.clone()),
        }
    }

    pub fn get(&mut self, source: &ImageSource) -> ImageState {
        if !self.caps.supported {
            return ImageState::Unavailable;
        }
        match source {
            ImageSource::File(path) => self.get_file(path),
            ImageSource::Diagram { hash, .. } => {
                if let Some(entry) = self.entries.get(source) {
                    return entry.state();
                }
                if !self.diagram_sources.contains_key(hash) {
                    return ImageState::Unavailable;
                }
                self.start_decode(source.clone(), DIAGRAM_MTIME);
                self.entries[source].state()
            }
        }
    }

    fn get_file(&mut self, path: &Path) -> ImageState {
        let Some(path) = canonical(path) else {
            return ImageState::Unavailable;
        };
        let key = ImageSource::File(path.clone());
        let meta = std::fs::metadata(&path).ok();
        let Some(meta) = meta else {
            return self.fail(key, None);
        };
        let mtime = meta.modified().unwrap_or(SystemTime::UNIX_EPOCH);
        if let Some(entry) = self.entries.get(&key) {
            if entry.mtime() == Some(mtime) {
                return entry.state();
            }
        }
        if let Some(old) = self.entries.get(&key).and_then(Entry::mtime) {
            self.free_ids(&key, Some(old));
        }
        if meta.len() > MAX_FILE_BYTES {
            return self.fail(key, Some(mtime));
        }
        self.start_decode(key.clone(), mtime);
        self.entries[&key].state()
    }

    /// Decodes `key` off-thread, or inline with sync decoding.
    fn start_decode(&mut self, key: ImageSource, mtime: SystemTime) {
        let job = match &key {
            ImageSource::File(path) => Job::File(path.clone(), self.max_px()),
            ImageSource::Diagram { hash, dark } => {
                let Some(source) = self.diagram_sources.get(hash) else {
                    return;
                };
                Job::Diagram {
                    source: source.clone(),
                    dark: *dark,
                    cell_px: self.caps.cell_px,
                    max_px: self.diagram_max_px(),
                }
            }
        };
        self.entries.insert(key.clone(), Entry::Loading { mtime });
        self.generation += 1;
        if self.sync_decode {
            let result = job.run();
            self.apply(key, mtime, result);
            return;
        }
        let finished = self.finished.clone();
        tokio::task::spawn_blocking(move || {
            let result = job.run();
            finished.lock().unwrap().push((key, mtime, result));
            helix_event::request_redraw();
        });
    }

    fn fail(&mut self, key: ImageSource, mtime: Option<SystemTime>) -> ImageState {
        let previous = self.entries.get(&key).map(|e| (e.mtime(), e.state()));
        if previous == Some((mtime, ImageState::Unavailable)) {
            return ImageState::Unavailable;
        }
        self.free_ids(&key, None);
        self.entries
            .insert(key, Entry::Failed { mtime, error: None });
        self.generation += 1;
        ImageState::Unavailable
    }

    fn apply(&mut self, key: ImageSource, mtime: SystemTime, result: Result<RgbaImage, String>) {
        let Some(Entry::Loading { mtime: current }) = self.entries.get(&key) else {
            return;
        };
        if *current != mtime {
            return;
        }
        self.tick += 1;
        let entry = match result {
            Ok(image) => Entry::Ready {
                mtime,
                width: image.width(),
                height: image.height(),
                image: Some(Arc::new(image)),
                last_used: self.tick,
            },
            Err(err) => match &key {
                ImageSource::File(path) => {
                    log::debug!("failed to decode {}: {err}", path.display());
                    Entry::Failed {
                        mtime: Some(mtime),
                        error: None,
                    }
                }
                ImageSource::Diagram { .. } => Entry::Failed {
                    mtime: Some(mtime),
                    error: Some(err),
                },
            },
        };
        self.entries.insert(key, entry);
        self.generation += 1;
    }

    pub fn poll(&mut self) {
        let done = std::mem::take(&mut *self.finished.lock().unwrap());
        for (key, mtime, result) in done {
            self.apply(key, mtime, result);
        }
    }

    fn allocate_id(&mut self) -> u32 {
        let used: HashSet<u32> = self.ids.values().map(|(id, _)| *id).collect();
        loop {
            self.next_id = if self.next_id >= MAX_ID {
                1
            } else {
                self.next_id + 1
            };
            if !used.contains(&self.next_id) {
                return self.next_id;
            }
        }
    }

    pub fn placement(&mut self, source: &ImageSource, cols: u16, rows: u16) -> Option<u32> {
        let path = Self::key(source)?;
        self.tick += 1;
        let (mtime, width, height, image) = match self.entries.get_mut(&path)? {
            Entry::Ready {
                mtime,
                width,
                height,
                image,
                last_used,
            } => {
                *last_used = self.tick;
                (*mtime, *width, *height, image.clone())
            }
            _ => return None,
        };
        let key = (path.clone(), mtime, cols, rows);
        let existing = self.ids.get(&key).copied();
        if let Some((id, true)) = existing {
            return Some(id);
        }
        let image = match image {
            Some(image) => image,
            None => {
                // The pixels were evicted; decode them again, off the render
                // thread unless decoding is synchronous. The result may no
                // longer match the size views planned with, in which case
                // they recompute.
                self.start_decode(path.clone(), mtime);
                match self.entries.get(&path) {
                    Some(Entry::Ready {
                        image: Some(image),
                        width: new_width,
                        height: new_height,
                        ..
                    }) if (*new_width, *new_height) == (width, height) => image.clone(),
                    _ => {
                        self.ids.remove(&key);
                        helix_event::request_redraw();
                        return None;
                    }
                }
            }
        };
        let id = match existing {
            Some((id, _)) => id,
            None => self.allocate_id(),
        };
        self.pending.push(protocol::transmit(
            id,
            image.as_raw(),
            width,
            height,
            cols,
            rows,
        ));
        self.ids.insert(key, (id, true));
        Some(id)
    }

    pub fn retain_ids(&mut self, live: &HashSet<u32>) {
        let dead: Vec<IdKey> = self
            .ids
            .iter()
            .filter(|(_, (id, _))| !live.contains(id))
            .map(|(key, _)| key.clone())
            .collect();
        if !dead.is_empty() {
            // a view switched away from a document keeps its placements
            self.generation += 1;
        }
        for key in dead {
            if let Some((id, transmitted)) = self.ids.remove(&key) {
                if transmitted {
                    self.queue_delete(id);
                }
            }
        }
        self.evict_pixels();
    }

    /// Drops the pixels of the least recently used images without ids while
    /// the decoded images exceed the budget.
    fn evict_pixels(&mut self) {
        let with_ids: HashSet<(&ImageSource, SystemTime)> =
            self.ids.keys().map(|(k, m, ..)| (k, *m)).collect();
        let mut total = 0;
        let mut unused = Vec::new();
        for (key, entry) in &self.entries {
            if let Entry::Ready {
                mtime,
                image: Some(image),
                last_used,
                ..
            } = entry
            {
                let bytes = image.as_raw().len();
                total += bytes;
                if !with_ids.contains(&(key, *mtime)) {
                    unused.push((*last_used, bytes, key.clone()));
                }
            }
        }
        drop(with_ids);
        unused.sort_by_key(|(last_used, ..)| *last_used);
        for (_, bytes, key) in unused {
            if total <= self.pixel_budget {
                break;
            }
            if matches!(key, ImageSource::Diagram { .. }) {
                // rendered again from its text when a view shows it
                self.entries.remove(&key);
            } else if let Some(Entry::Ready { image, .. }) = self.entries.get_mut(&key) {
                *image = None;
            }
            total -= bytes;
        }
        let entries = &self.entries;
        self.diagram_sources.retain(|hash, _| {
            entries
                .keys()
                .any(|key| matches!(key, ImageSource::Diagram { hash: h, .. } if h == hash))
        });
    }

    fn flatten(&self, sequences: Vec<Vec<u8>>) -> Vec<u8> {
        let tmux = self.caps.tmux;
        sequences
            .into_iter()
            .flat_map(|seq| if tmux { protocol::wrap_tmux(&seq) } else { seq })
            .collect()
    }

    pub fn take_pending(&mut self) -> Vec<u8> {
        // A transmission is several APC sequences, each wrapped on its own.
        let sequences = std::mem::take(&mut self.pending)
            .into_iter()
            .flat_map(|seq| split_sequences(&seq))
            .collect();
        self.flatten(sequences)
    }

    pub fn delete_all(&mut self) -> Vec<u8> {
        self.pending.clear();
        let mut sequences = Vec::new();
        for (id, transmitted) in self.ids.values_mut() {
            if std::mem::replace(transmitted, false) {
                sequences.push(protocol::delete(*id));
            }
        }
        self.generation += 1;
        self.flatten(sequences)
    }

    /// Sends every image again on the next render, without deleting any.
    /// tmux forwards passthrough only to a visible pane, and a re-attached
    /// client may be a different terminal.
    pub fn retransmit_all(&mut self) {
        for (_, transmitted) in self.ids.values_mut() {
            *transmitted = false;
        }
        self.generation += 1;
    }

    pub fn invalidate_mtimes(&mut self) {
        self.generation += 1;
    }
}

/// Splits concatenated `ESC _G … ESC \` sequences.
fn split_sequences(bytes: &[u8]) -> Vec<Vec<u8>> {
    let mut out = Vec::new();
    let mut start = 0;
    for i in 0..bytes.len().saturating_sub(1) {
        if bytes[i] == 0x1b && bytes[i + 1] == b'\\' {
            out.push(bytes[start..i + 2].to_vec());
            start = i + 2;
        }
    }
    if start < bytes.len() {
        out.push(bytes[start..].to_vec());
    }
    out
}

#[cfg(test)]
mod test {
    use super::*;
    use std::collections::HashSet;

    fn store() -> ImageStore {
        let mut store = ImageStore::default();
        store.set_caps(GraphicsCaps {
            supported: true,
            ..GraphicsCaps::default()
        });
        store.set_sync_decode(true);
        store
    }

    fn file(path: &Path) -> ImageSource {
        ImageSource::File(path.to_path_buf())
    }

    fn png(dir: &Path, name: &str, w: u32, h: u32) -> PathBuf {
        let path = dir.join(name);
        image::RgbaImage::from_pixel(w, h, image::Rgba([255, 0, 0, 255]))
            .save(&path)
            .unwrap();
        path
    }

    fn text(bytes: Vec<u8>) -> String {
        String::from_utf8(bytes).unwrap()
    }

    #[test]
    fn resize_keeps_cell_size_when_unknown() {
        let mut store = store();
        let generation = store.generation();
        store.resize(None, (100, 40));
        assert_eq!(store.caps().cell_px, (10, 20));
        assert_eq!(store.caps().term_cells, (100, 40));
        assert!(store.caps().supported);
        assert!(store.generation() > generation);
        store.resize(Some((8, 16)), (100, 40));
        assert_eq!(store.caps().cell_px, (8, 16));
    }

    #[test]
    fn decodes_png() {
        let dir = tempfile::tempdir().unwrap();
        let path = png(dir.path(), "a.png", 30, 20);
        let mut store = store();
        assert_eq!(
            store.get(&file(&path)),
            ImageState::Ready {
                width: 30,
                height: 20
            }
        );
    }

    #[test]
    fn unsupported_terminal_is_unavailable() {
        let dir = tempfile::tempdir().unwrap();
        let path = png(dir.path(), "a.png", 3, 2);
        let mut store = ImageStore::default();
        store.set_sync_decode(true);
        assert_eq!(store.get(&file(&path)), ImageState::Unavailable);
    }

    #[test]
    fn missing_and_garbage_are_unavailable() {
        let dir = tempfile::tempdir().unwrap();
        let mut store = store();
        assert_eq!(
            store.get(&file(&dir.path().join("nope.png"))),
            ImageState::Unavailable
        );
        let garbage = dir.path().join("bad.png");
        std::fs::write(&garbage, b"not an image").unwrap();
        assert_eq!(store.get(&file(&garbage)), ImageState::Unavailable);
    }

    #[test]
    fn transmits_once_per_id() {
        let dir = tempfile::tempdir().unwrap();
        let path = png(dir.path(), "a.png", 4, 4);
        let mut store = store();
        store.get(&file(&path));
        let id = store.placement(&file(&path), 2, 1).unwrap();
        assert_eq!(store.placement(&file(&path), 2, 1), Some(id));
        let pending = text(store.take_pending());
        assert_eq!(pending.matches("a=T").count(), 1);
        assert!(pending.contains(&format!("i={id},")));
        assert!(store.take_pending().is_empty());
    }

    #[test]
    fn placements_per_size_get_distinct_ids() {
        let dir = tempfile::tempdir().unwrap();
        let path = png(dir.path(), "a.png", 4, 4);
        let mut store = store();
        store.get(&file(&path));
        let a = store.placement(&file(&path), 2, 1).unwrap();
        let b = store.placement(&file(&path), 4, 2).unwrap();
        assert_ne!(a, b);
        assert_eq!(text(store.take_pending()).matches("a=T").count(), 2);
    }

    #[test]
    fn unused_ids_are_deleted() {
        let dir = tempfile::tempdir().unwrap();
        let path = png(dir.path(), "a.png", 4, 4);
        let mut store = store();
        store.get(&file(&path));
        let a = store.placement(&file(&path), 2, 1).unwrap();
        let b = store.placement(&file(&path), 4, 2).unwrap();
        store.take_pending();
        let generation = store.generation();
        store.retain_ids(&HashSet::from([a, b]));
        assert_eq!(store.generation(), generation);
        store.retain_ids(&HashSet::from([b]));
        // views still holding `a` must recompute
        assert!(store.generation() > generation);
        let pending = text(store.take_pending());
        assert!(pending.contains(&format!("a=d,d=I,i={a},")));
        assert!(!pending.contains(&format!("i={b},")));
    }

    #[test]
    fn changed_mtime_redecodes_and_frees_old_id() {
        let dir = tempfile::tempdir().unwrap();
        let path = png(dir.path(), "a.png", 4, 4);
        let mut store = store();
        store.get(&file(&path));
        let old = store.placement(&file(&path), 2, 1).unwrap();
        store.take_pending();
        let generation = store.generation();
        std::thread::sleep(std::time::Duration::from_millis(20));
        png(dir.path(), "a.png", 8, 4);
        // make sure the mtime differs even on coarse filesystems
        let later = std::time::SystemTime::now() + std::time::Duration::from_secs(2);
        std::fs::File::options()
            .write(true)
            .open(&path)
            .unwrap()
            .set_modified(later)
            .unwrap();
        assert_eq!(
            store.get(&file(&path)),
            ImageState::Ready {
                width: 8,
                height: 4
            }
        );
        assert!(store.generation() > generation);
        let new = store.placement(&file(&path), 2, 1).unwrap();
        assert_ne!(new, old);
        let pending = text(store.take_pending());
        assert!(pending.contains(&format!("a=d,d=I,i={old},")));
        assert!(pending.contains(&format!("i={new},")));
    }

    #[test]
    fn tmux_wraps_pending() {
        let dir = tempfile::tempdir().unwrap();
        let path = png(dir.path(), "a.png", 1, 1);
        let mut store = store();
        store.set_caps(GraphicsCaps {
            supported: true,
            tmux: true,
            ..GraphicsCaps::default()
        });
        store.get(&file(&path));
        store.placement(&file(&path), 1, 1);
        assert!(text(store.take_pending()).starts_with("\x1bPtmux;\x1b\x1b_G"));
    }

    #[test]
    fn delete_all_then_retransmit() {
        let dir = tempfile::tempdir().unwrap();
        let path = png(dir.path(), "a.png", 4, 4);
        let mut store = store();
        store.get(&file(&path));
        let id = store.placement(&file(&path), 2, 1).unwrap();
        store.take_pending();
        let generation = store.generation();
        assert!(text(store.delete_all()).contains(&format!("a=d,d=I,i={id},")));
        assert!(store.generation() > generation);
        assert_eq!(store.placement(&file(&path), 2, 1), Some(id));
        assert!(text(store.take_pending()).contains("a=T"));
    }

    #[test]
    fn tmux_wraps_each_chunk() {
        let dir = tempfile::tempdir().unwrap();
        // 200x200 RGBA -> 160000 bytes -> many base64 chunks
        let path = png(dir.path(), "a.png", 200, 200);
        let mut store = store();
        store.set_caps(GraphicsCaps {
            supported: true,
            tmux: true,
            ..GraphicsCaps::default()
        });
        store.get(&file(&path));
        store.placement(&file(&path), 1, 1);
        let pending = text(store.take_pending());
        let dcs = pending.matches("\x1bPtmux;").count();
        let apc = pending.matches("\x1b\x1b_G").count();
        assert!(dcs > 1);
        assert_eq!(dcs, apc);
    }

    #[test]
    fn path_forms_share_entry() {
        let dir = tempfile::tempdir().unwrap();
        let path = png(dir.path(), "a.png", 4, 4);
        let indirect = dir.path().join(".").join("a.png");
        let mut store = store();
        store.get(&file(&path));
        let id = store.placement(&file(&path), 2, 1).unwrap();
        assert_eq!(store.placement(&file(&indirect), 2, 1), Some(id));
    }

    /// Whether the decoded pixels of `path` are kept.
    fn has_pixels(store: &ImageStore, path: &Path) -> bool {
        matches!(
            store
                .entries
                .get(&ImageSource::File(canonical(path).unwrap())),
            Some(Entry::Ready { image: Some(_), .. })
        )
    }

    /// Replaces the file's contents but keeps its mtime, so that only a
    /// fresh decode notices.
    fn corrupt_keeping_mtime(path: &Path) {
        let mtime = std::fs::metadata(path).unwrap().modified().unwrap();
        std::fs::write(path, b"not an image").unwrap();
        std::fs::File::options()
            .write(true)
            .open(path)
            .unwrap()
            .set_modified(mtime)
            .unwrap();
    }

    #[test]
    fn pixels_without_ids_are_kept_under_budget() {
        let dir = tempfile::tempdir().unwrap();
        let path = png(dir.path(), "a.png", 4, 4);
        let mut store = store();
        store.get(&file(&path));
        let id = store.placement(&file(&path), 2, 1).unwrap();
        store.take_pending();
        store.retain_ids(&HashSet::new());
        assert!(text(store.take_pending()).contains(&format!("a=d,d=I,i={id},")));
        assert!(has_pixels(&store, &path));
        // transmitted again from the kept pixels, without decoding
        corrupt_keeping_mtime(&path);
        assert!(store.placement(&file(&path), 2, 1).is_some());
        assert!(text(store.take_pending()).contains("a=T"));
    }

    #[test]
    fn evicts_least_recently_used_pixels_over_budget() {
        let dir = tempfile::tempdir().unwrap();
        let a = png(dir.path(), "a.png", 4, 4);
        let b = png(dir.path(), "b.png", 4, 4);
        let c = png(dir.path(), "c.png", 4, 4);
        let mut store = store();
        // room for two 4x4 RGBA images
        store.pixel_budget = 2 * 4 * 4 * 4;
        for path in [&a, &b, &c] {
            store.get(&file(path));
        }
        store.placement(&file(&b), 2, 1).unwrap();
        store.placement(&file(&a), 2, 1).unwrap();
        let c_id = store.placement(&file(&c), 2, 1).unwrap();
        // c has a live id, so b, the least recently used of the rest, goes
        store.retain_ids(&HashSet::from([c_id]));
        assert!(has_pixels(&store, &a));
        assert!(!has_pixels(&store, &b));
        assert!(has_pixels(&store, &c));
        // live ids keep their pixels even far over budget
        store.pixel_budget = 0;
        store.retain_ids(&HashSet::from([c_id]));
        assert!(!has_pixels(&store, &a));
        assert!(has_pixels(&store, &c));
    }

    #[test]
    fn evicted_pixels_redecode_inline_when_sync() {
        let dir = tempfile::tempdir().unwrap();
        let path = png(dir.path(), "a.png", 4, 4);
        let mut store = store();
        store.pixel_budget = 0;
        store.get(&file(&path));
        store.placement(&file(&path), 2, 1).unwrap();
        store.retain_ids(&HashSet::new());
        store.take_pending();
        assert!(!has_pixels(&store, &path));
        let id = store.placement(&file(&path), 2, 1).unwrap();
        assert!(text(store.take_pending()).contains(&format!("i={id},")));
    }

    #[tokio::test]
    async fn evicted_pixels_redecode_off_thread() {
        let dir = tempfile::tempdir().unwrap();
        let path = png(dir.path(), "a.png", 4, 4);
        let mut store = store();
        store.pixel_budget = 0;
        store.get(&file(&path));
        store.placement(&file(&path), 2, 1).unwrap();
        store.retain_ids(&HashSet::new());
        store.take_pending();
        store.set_sync_decode(false);
        let generation = store.generation();
        // not decoded on the calling (render) thread
        assert_eq!(store.placement(&file(&path), 2, 1), None);
        assert_eq!(store.get(&file(&path)), ImageState::Loading);
        assert!(store.generation() > generation);
        assert!(store.take_pending().is_empty());
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while store.get(&file(&path)) == ImageState::Loading {
            assert!(
                std::time::Instant::now() < deadline,
                "decode never finished"
            );
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
            store.poll();
        }
        let id = store.placement(&file(&path), 2, 1).unwrap();
        assert!(text(store.take_pending()).contains(&format!("i={id},")));
    }

    #[test]
    fn redecode_after_caps_change_keeps_size_consistent() {
        let dir = tempfile::tempdir().unwrap();
        let path = png(dir.path(), "a.png", 2000, 1000);
        let mut store = store();
        store.pixel_budget = 0;
        store.get(&file(&path));
        assert_eq!(
            store.get(&file(&path)),
            ImageState::Ready {
                width: 480,
                height: 240
            }
        );
        store.placement(&file(&path), 2, 1).unwrap();
        store.retain_ids(&HashSet::new());
        store.take_pending();
        store.set_caps(GraphicsCaps {
            supported: true,
            term_cells: (160, 40),
            ..GraphicsCaps::default()
        });
        let generation = store.generation();
        // the re-decode differs in size, so the caller must recompute
        assert_eq!(store.placement(&file(&path), 2, 1), None);
        assert!(store.generation() > generation);
        assert!(store.take_pending().is_empty());
        let ImageState::Ready { width, height } = store.get(&file(&path)) else {
            panic!()
        };
        let id = store.placement(&file(&path), 2, 1).unwrap();
        let pending = text(store.take_pending());
        assert!(pending.contains(&format!("i={id},s={width},v={height},")));
    }

    #[test]
    fn ids_start_at_random_and_stay_in_range() {
        let store = ImageStore::default();
        assert!(store.next_id < MAX_ID);
        let mut store = self::store();
        store.next_id = MAX_ID - 1;
        assert_eq!(store.allocate_id(), MAX_ID);
        assert_eq!(store.allocate_id(), 1);
        let dir = tempfile::tempdir().unwrap();
        let path = png(dir.path(), "a.png", 4, 4);
        store.get(&file(&path));
        store.next_id = MAX_ID - 1;
        let a = store.placement(&file(&path), 1, 1).unwrap();
        store.next_id = MAX_ID - 1;
        // skips the id in use and wraps
        let b = store.placement(&file(&path), 2, 1).unwrap();
        assert_eq!((a, b), (MAX_ID, 1));
    }

    #[test]
    fn retransmit_all_resends_without_deleting() {
        let dir = tempfile::tempdir().unwrap();
        let path = png(dir.path(), "a.png", 4, 4);
        let mut store = store();
        store.get(&file(&path));
        let id = store.placement(&file(&path), 2, 1).unwrap();
        store.take_pending();
        let generation = store.generation();
        store.retransmit_all();
        assert!(store.generation() > generation);
        assert!(store.take_pending().is_empty());
        assert_eq!(store.placement(&file(&path), 2, 1), Some(id));
        let pending = text(store.take_pending());
        assert!(pending.contains(&format!("a=T,U=1,q=2,f=32,i={id},")));
        assert!(!pending.contains("a=d"));
    }

    const FLOWCHART: &str = "flowchart TD\n  A --> B\n";

    #[test]
    fn diagram_renders_and_transmits() {
        let mut store = store();
        let source = store.diagram(FLOWCHART, true);
        let ImageState::Ready { width, height } = store.get(&source) else {
            panic!("{:?}", store.get(&source))
        };
        assert!(width > 0 && height > 0);
        let id = store.placement(&source, 4, 3).unwrap();
        assert!(text(store.take_pending()).contains(&format!("i={id},")));
    }

    #[test]
    fn same_text_shares_an_entry_and_variants_do_not() {
        let mut store = store();
        let a = store.diagram(FLOWCHART, true);
        assert_eq!(store.diagram(FLOWCHART, true), a);
        let light = store.diagram(FLOWCHART, false);
        assert_ne!(light, a);
        store.get(&a);
        store.get(&light);
        assert_ne!(store.placement(&a, 4, 3), store.placement(&light, 4, 3));
    }

    #[test]
    fn diagram_error_keeps_its_message() {
        let mut store = store();
        let source = store.diagram("flowchart TD\n  A -->\n", true);
        let ImageState::Error(message) = store.get(&source) else {
            panic!("{:?}", store.get(&source))
        };
        assert!(!message.is_empty());
        assert_eq!(store.placement(&source, 4, 3), None);
        // failed diagrams are not rendered again on every refresh
        let generation = store.generation();
        store.get(&source);
        store.retain_ids(&HashSet::new());
        store.get(&source);
        assert_eq!(store.generation(), generation);
    }

    #[test]
    fn diagrams_are_unavailable_without_graphics() {
        let mut store = ImageStore::default();
        store.set_sync_decode(true);
        let source = store.diagram(FLOWCHART, true);
        assert_eq!(store.get(&source), ImageState::Unavailable);
    }

    #[test]
    fn diagrams_fit_the_terminal_minus_two_rows() {
        let mut store = store();
        store.set_caps(GraphicsCaps {
            supported: true,
            cell_px: (10, 20),
            term_cells: (8, 6),
            ..GraphicsCaps::default()
        });
        let source = store.diagram(
            "flowchart LR\n  A[a long label] --> B[another long label] --> C[and a third one]\n",
            true,
        );
        let ImageState::Ready { width, height } = store.get(&source) else {
            panic!()
        };
        // 8 columns x (6 - 2) rows of 10x20 px
        assert!(width <= 80 && height <= 80, "{width}x{height}");
    }

    #[test]
    fn unused_diagrams_over_budget_are_dropped_and_rerendered() {
        let mut store = store();
        store.pixel_budget = 0;
        let source = store.diagram(FLOWCHART, true);
        store.get(&source);
        store.placement(&source, 4, 3).unwrap();
        store.retain_ids(&HashSet::new());
        assert!(!store.entries.contains_key(&source));
        assert!(store.diagram_sources.is_empty());
        // a view showing it again registers the text again
        let source = store.diagram(FLOWCHART, true);
        assert!(matches!(store.get(&source), ImageState::Ready { .. }));
    }

    #[test]
    fn keeps_the_terminal_theme_mode() {
        let mut store = store();
        assert_eq!(store.theme_mode(), None);
        store.set_theme_mode(Some(crate::theme::Mode::Light));
        assert_eq!(store.theme_mode(), Some(crate::theme::Mode::Light));
    }
}

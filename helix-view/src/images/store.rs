//! Decodes images off-thread and tracks the Kitty image ids used for them.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::SystemTime;

use image::RgbaImage;

use super::{protocol, GraphicsCaps};

const MAX_FILE_BYTES: u64 = 50 * 1024 * 1024;
const MAX_ID: u32 = 0xFF_FFFF;
/// Decoded RGBA bytes kept for images that no view currently shows.
const PIXEL_BUDGET: usize = 128 * 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImageState {
    /// Missing, unreadable, undecodable, or graphics unsupported.
    Unavailable,
    Loading,
    Ready {
        width: u32,
        height: u32,
    },
}

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
    },
}

impl Entry {
    fn mtime(&self) -> Option<SystemTime> {
        match self {
            Entry::Loading { mtime } | Entry::Ready { mtime, .. } => Some(*mtime),
            Entry::Failed { mtime } => *mtime,
        }
    }

    fn state(&self) -> ImageState {
        match *self {
            Entry::Loading { .. } => ImageState::Loading,
            Entry::Ready { width, height, .. } => ImageState::Ready { width, height },
            Entry::Failed { .. } => ImageState::Unavailable,
        }
    }
}

type Decoded = (PathBuf, SystemTime, Result<RgbaImage, String>);
type IdKey = (PathBuf, SystemTime, u16, u16);

pub struct ImageStore {
    caps: GraphicsCaps,
    sync_decode: bool,
    entries: HashMap<PathBuf, Entry>,
    /// (path, mtime, cols, rows) -> (id, transmitted)
    ids: HashMap<IdKey, (u32, bool)>,
    next_id: u32,
    /// Complete escape sequences, wrapped for tmux when taken.
    pending: Vec<Vec<u8>>,
    generation: u64,
    finished: Arc<Mutex<Vec<Decoded>>>,
    pixel_budget: usize,
    /// Advanced on every use of an image, for least-recently-used eviction.
    tick: u64,
}

impl Default for ImageStore {
    fn default() -> Self {
        Self {
            caps: GraphicsCaps::default(),
            sync_decode: false,
            entries: HashMap::new(),
            ids: HashMap::new(),
            next_id: id_seed(),
            pending: Vec::new(),
            generation: 0,
            finished: Arc::default(),
            pixel_budget: PIXEL_BUDGET,
            tick: 0,
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

impl ImageStore {
    pub fn caps(&self) -> GraphicsCaps {
        self.caps
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

    /// Frees all ids of `path`, optionally only those of one `mtime`.
    fn free_ids(&mut self, path: &Path, mtime: Option<SystemTime>) {
        let freed: Vec<IdKey> = self
            .ids
            .keys()
            .filter(|(p, m, ..)| p == path && mtime.is_none_or(|mtime| mtime == *m))
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

    pub fn get(&mut self, path: &Path) -> ImageState {
        if !self.caps.supported {
            return ImageState::Unavailable;
        }
        let Some(path) = canonical(path) else {
            return ImageState::Unavailable;
        };
        let meta = std::fs::metadata(&path).ok();
        let Some(meta) = meta else {
            return self.fail(path, None);
        };
        let mtime = meta.modified().unwrap_or(SystemTime::UNIX_EPOCH);
        if let Some(entry) = self.entries.get(&path) {
            if entry.mtime() == Some(mtime) {
                return entry.state();
            }
        }
        if let Some(old) = self.entries.get(&path).and_then(Entry::mtime) {
            self.free_ids(&path, Some(old));
        }
        if meta.len() > MAX_FILE_BYTES {
            return self.fail(path, Some(mtime));
        }
        self.start_decode(path.clone(), mtime);
        self.entries[&path].state()
    }

    /// Decodes `path` off-thread, or inline with sync decoding.
    fn start_decode(&mut self, path: PathBuf, mtime: SystemTime) {
        self.entries.insert(path.clone(), Entry::Loading { mtime });
        self.generation += 1;

        let max_px = self.max_px();
        if self.sync_decode {
            let result = decode(&path, max_px);
            self.apply(path, mtime, result);
            return;
        }
        let finished = self.finished.clone();
        tokio::task::spawn_blocking(move || {
            let result = decode(&path, max_px);
            finished.lock().unwrap().push((path, mtime, result));
            helix_event::request_redraw();
        });
    }

    fn fail(&mut self, path: PathBuf, mtime: Option<SystemTime>) -> ImageState {
        let previous = self.entries.get(&path).map(|e| (e.mtime(), e.state()));
        if previous == Some((mtime, ImageState::Unavailable)) {
            return ImageState::Unavailable;
        }
        self.free_ids(&path, None);
        self.entries.insert(path, Entry::Failed { mtime });
        self.generation += 1;
        ImageState::Unavailable
    }

    fn apply(&mut self, path: PathBuf, mtime: SystemTime, result: Result<RgbaImage, String>) {
        let Some(Entry::Loading { mtime: current }) = self.entries.get(&path) else {
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
            Err(err) => {
                log::debug!("failed to decode {}: {err}", path.display());
                Entry::Failed { mtime: Some(mtime) }
            }
        };
        self.entries.insert(path, entry);
        self.generation += 1;
    }

    pub fn poll(&mut self) {
        let done = std::mem::take(&mut *self.finished.lock().unwrap());
        for (path, mtime, result) in done {
            self.apply(path, mtime, result);
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

    pub fn placement(&mut self, path: &Path, cols: u16, rows: u16) -> Option<u32> {
        let path = canonical(path)?;
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
        let with_ids: HashSet<(&PathBuf, SystemTime)> =
            self.ids.keys().map(|(p, m, ..)| (p, *m)).collect();
        let mut total = 0;
        let mut unused = Vec::new();
        for (path, entry) in self.entries.iter_mut() {
            if let Entry::Ready {
                mtime,
                image: Some(image),
                last_used,
                ..
            } = entry
            {
                let bytes = image.as_raw().len();
                total += bytes;
                if !with_ids.contains(&(path, *mtime)) {
                    unused.push((*last_used, bytes, entry));
                }
            }
        }
        unused.sort_by_key(|(last_used, ..)| *last_used);
        for (_, bytes, entry) in unused {
            if total <= self.pixel_budget {
                break;
            }
            if let Entry::Ready { image, .. } = entry {
                *image = None;
            }
            total -= bytes;
        }
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
            store.get(&path),
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
        assert_eq!(store.get(&path), ImageState::Unavailable);
    }

    #[test]
    fn missing_and_garbage_are_unavailable() {
        let dir = tempfile::tempdir().unwrap();
        let mut store = store();
        assert_eq!(
            store.get(&dir.path().join("nope.png")),
            ImageState::Unavailable
        );
        let garbage = dir.path().join("bad.png");
        std::fs::write(&garbage, b"not an image").unwrap();
        assert_eq!(store.get(&garbage), ImageState::Unavailable);
    }

    #[test]
    fn transmits_once_per_id() {
        let dir = tempfile::tempdir().unwrap();
        let path = png(dir.path(), "a.png", 4, 4);
        let mut store = store();
        store.get(&path);
        let id = store.placement(&path, 2, 1).unwrap();
        assert_eq!(store.placement(&path, 2, 1), Some(id));
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
        store.get(&path);
        let a = store.placement(&path, 2, 1).unwrap();
        let b = store.placement(&path, 4, 2).unwrap();
        assert_ne!(a, b);
        assert_eq!(text(store.take_pending()).matches("a=T").count(), 2);
    }

    #[test]
    fn unused_ids_are_deleted() {
        let dir = tempfile::tempdir().unwrap();
        let path = png(dir.path(), "a.png", 4, 4);
        let mut store = store();
        store.get(&path);
        let a = store.placement(&path, 2, 1).unwrap();
        let b = store.placement(&path, 4, 2).unwrap();
        store.take_pending();
        store.retain_ids(&HashSet::from([b]));
        let pending = text(store.take_pending());
        assert!(pending.contains(&format!("a=d,d=I,i={a},")));
        assert!(!pending.contains(&format!("i={b},")));
    }

    #[test]
    fn changed_mtime_redecodes_and_frees_old_id() {
        let dir = tempfile::tempdir().unwrap();
        let path = png(dir.path(), "a.png", 4, 4);
        let mut store = store();
        store.get(&path);
        let old = store.placement(&path, 2, 1).unwrap();
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
            store.get(&path),
            ImageState::Ready {
                width: 8,
                height: 4
            }
        );
        assert!(store.generation() > generation);
        let new = store.placement(&path, 2, 1).unwrap();
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
        store.get(&path);
        store.placement(&path, 1, 1);
        assert!(text(store.take_pending()).starts_with("\x1bPtmux;\x1b\x1b_G"));
    }

    #[test]
    fn delete_all_then_retransmit() {
        let dir = tempfile::tempdir().unwrap();
        let path = png(dir.path(), "a.png", 4, 4);
        let mut store = store();
        store.get(&path);
        let id = store.placement(&path, 2, 1).unwrap();
        store.take_pending();
        let generation = store.generation();
        assert!(text(store.delete_all()).contains(&format!("a=d,d=I,i={id},")));
        assert!(store.generation() > generation);
        assert_eq!(store.placement(&path, 2, 1), Some(id));
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
        store.get(&path);
        store.placement(&path, 1, 1);
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
        store.get(&path);
        let id = store.placement(&path, 2, 1).unwrap();
        assert_eq!(store.placement(&indirect, 2, 1), Some(id));
    }

    /// Whether the decoded pixels of `path` are kept.
    fn has_pixels(store: &ImageStore, path: &Path) -> bool {
        matches!(
            store.entries.get(&canonical(path).unwrap()),
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
        store.get(&path);
        let id = store.placement(&path, 2, 1).unwrap();
        store.take_pending();
        store.retain_ids(&HashSet::new());
        assert!(text(store.take_pending()).contains(&format!("a=d,d=I,i={id},")));
        assert!(has_pixels(&store, &path));
        // transmitted again from the kept pixels, without decoding
        corrupt_keeping_mtime(&path);
        assert!(store.placement(&path, 2, 1).is_some());
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
            store.get(path);
        }
        store.placement(&b, 2, 1).unwrap();
        store.placement(&a, 2, 1).unwrap();
        let c_id = store.placement(&c, 2, 1).unwrap();
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
        store.get(&path);
        store.placement(&path, 2, 1).unwrap();
        store.retain_ids(&HashSet::new());
        store.take_pending();
        assert!(!has_pixels(&store, &path));
        let id = store.placement(&path, 2, 1).unwrap();
        assert!(text(store.take_pending()).contains(&format!("i={id},")));
    }

    #[tokio::test]
    async fn evicted_pixels_redecode_off_thread() {
        let dir = tempfile::tempdir().unwrap();
        let path = png(dir.path(), "a.png", 4, 4);
        let mut store = store();
        store.pixel_budget = 0;
        store.get(&path);
        store.placement(&path, 2, 1).unwrap();
        store.retain_ids(&HashSet::new());
        store.take_pending();
        store.set_sync_decode(false);
        let generation = store.generation();
        // not decoded on the calling (render) thread
        assert_eq!(store.placement(&path, 2, 1), None);
        assert_eq!(store.get(&path), ImageState::Loading);
        assert!(store.generation() > generation);
        assert!(store.take_pending().is_empty());
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while store.get(&path) == ImageState::Loading {
            assert!(
                std::time::Instant::now() < deadline,
                "decode never finished"
            );
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
            store.poll();
        }
        let id = store.placement(&path, 2, 1).unwrap();
        assert!(text(store.take_pending()).contains(&format!("i={id},")));
    }

    #[test]
    fn redecode_after_caps_change_keeps_size_consistent() {
        let dir = tempfile::tempdir().unwrap();
        let path = png(dir.path(), "a.png", 2000, 1000);
        let mut store = store();
        store.pixel_budget = 0;
        store.get(&path);
        assert_eq!(
            store.get(&path),
            ImageState::Ready {
                width: 480,
                height: 240
            }
        );
        store.placement(&path, 2, 1).unwrap();
        store.retain_ids(&HashSet::new());
        store.take_pending();
        store.set_caps(GraphicsCaps {
            supported: true,
            term_cells: (160, 40),
            ..GraphicsCaps::default()
        });
        let generation = store.generation();
        // the re-decode differs in size, so the caller must recompute
        assert_eq!(store.placement(&path, 2, 1), None);
        assert!(store.generation() > generation);
        assert!(store.take_pending().is_empty());
        let ImageState::Ready { width, height } = store.get(&path) else {
            panic!()
        };
        let id = store.placement(&path, 2, 1).unwrap();
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
        store.get(&path);
        store.next_id = MAX_ID - 1;
        let a = store.placement(&path, 1, 1).unwrap();
        store.next_id = MAX_ID - 1;
        // skips the id in use and wraps
        let b = store.placement(&path, 2, 1).unwrap();
        assert_eq!((a, b), (MAX_ID, 1));
    }

    #[test]
    fn retransmit_all_resends_without_deleting() {
        let dir = tempfile::tempdir().unwrap();
        let path = png(dir.path(), "a.png", 4, 4);
        let mut store = store();
        store.get(&path);
        let id = store.placement(&path, 2, 1).unwrap();
        store.take_pending();
        let generation = store.generation();
        store.retransmit_all();
        assert!(store.generation() > generation);
        assert!(store.take_pending().is_empty());
        assert_eq!(store.placement(&path, 2, 1), Some(id));
        let pending = text(store.take_pending());
        assert!(pending.contains(&format!("a=T,U=1,q=2,f=32,i={id},")));
        assert!(!pending.contains("a=d"));
    }
}

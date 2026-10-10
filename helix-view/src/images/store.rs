//! Decodes images off-thread and tracks the Kitty image ids used for them.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::SystemTime;

use image::RgbaImage;

use super::{protocol, GraphicsCaps};

const MAX_FILE_BYTES: u64 = 50 * 1024 * 1024;
const MAX_ID: u32 = 0xFF_FFFF;

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
        /// Dropped once no id uses the image, see [`ImageStore::retain_ids`].
        image: Option<Arc<RgbaImage>>,
        width: u32,
        height: u32,
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

#[derive(Default)]
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
        self.entries.insert(path.clone(), Entry::Loading { mtime });
        self.generation += 1;

        let max_px = self.max_px();
        if self.sync_decode {
            let result = decode(&path, max_px);
            self.apply(path.clone(), mtime, result);
            return self.entries[&path].state();
        }
        let finished = self.finished.clone();
        let task_path = path;
        tokio::task::spawn_blocking(move || {
            let result = decode(&task_path, max_px);
            finished.lock().unwrap().push((task_path, mtime, result));
            helix_event::request_redraw();
        });
        ImageState::Loading
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
        let entry = match result {
            Ok(image) => Entry::Ready {
                mtime,
                width: image.width(),
                height: image.height(),
                image: Some(Arc::new(image)),
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
        let (mtime, width, height, image) = match self.entries.get(&path)? {
            Entry::Ready {
                mtime,
                width,
                height,
                image,
            } => (*mtime, *width, *height, image.clone()),
            _ => return None,
        };
        let key = (path.clone(), mtime, cols, rows);
        let (id, transmitted) = match self.ids.get(&key) {
            Some(&entry) => entry,
            None => {
                let id = self.allocate_id();
                self.ids.insert(key.clone(), (id, false));
                (id, false)
            }
        };
        if !transmitted {
            let image = match image {
                Some(image) => image,
                None => {
                    // The pixels were dropped; reload them. The result may no
                    // longer match the size views planned with, in which case
                    // the entry is updated and the caller recomputes.
                    let current = std::fs::metadata(&path).and_then(|m| m.modified()).ok();
                    let decoded = match current {
                        Some(current) if current == mtime => decode(&path, self.max_px()).ok(),
                        _ => None,
                    };
                    let Some(decoded) = decoded else {
                        self.ids.remove(&key);
                        return None;
                    };
                    let (new_width, new_height) = (decoded.width(), decoded.height());
                    let image = Arc::new(decoded);
                    if let Some(Entry::Ready {
                        image: slot,
                        width: w,
                        height: h,
                        ..
                    }) = self.entries.get_mut(&path)
                    {
                        *slot = Some(image.clone());
                        *w = new_width;
                        *h = new_height;
                    }
                    if (new_width, new_height) != (width, height) {
                        self.ids.remove(&key);
                        self.generation += 1;
                        return None;
                    }
                    image
                }
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
        }
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
        let with_ids: HashSet<(&PathBuf, SystemTime)> =
            self.ids.keys().map(|(p, m, ..)| (p, *m)).collect();
        for (path, entry) in self.entries.iter_mut() {
            if let Entry::Ready { mtime, image, .. } = entry {
                if !with_ids.contains(&(path, *mtime)) {
                    *image = None;
                }
            }
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

    #[test]
    fn redecode_after_caps_change_keeps_size_consistent() {
        let dir = tempfile::tempdir().unwrap();
        let path = png(dir.path(), "a.png", 2000, 1000);
        let mut store = store();
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
}

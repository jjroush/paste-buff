//! In-memory history with JSON persistence and time-based expiry.
//!
//! Text and file clips live entirely in the JSON file. Image clips keep only
//! a path in the file; the PNG itself sits next to the history under
//! `images/` and is deleted together with its entry.

use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::fs;
use std::hash::{DefaultHasher, Hash, Hasher};
use std::io;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// Entries older than this are dropped.
pub const DEFAULT_TTL: Duration = Duration::from_secs(24 * 60 * 60);
/// Hard cap so a runaway copier can't grow the file without bound.
pub const MAX_ENTRIES: usize = 2000;
/// Larger text clips are ignored (1 MiB).
pub const MAX_TEXT_BYTES: usize = 1024 * 1024;
/// Larger images are ignored (32 MiB of PNG).
pub const MAX_IMAGE_BYTES: usize = 32 * 1024 * 1024;

const FILE_VERSION: u32 = 1;
const IMAGE_DIR: &str = "images";

/// What a pasteboard change contained, before it becomes an [`Entry`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Clip {
    Text(String),
    Files(Vec<PathBuf>),
    Image {
        png: Vec<u8>,
        width: u32,
        height: u32,
    },
}

/// Payload kind of an entry. `Text` keeps everything in `Entry::text`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Content {
    #[default]
    Text,
    Files {
        paths: Vec<PathBuf>,
    },
    Image {
        /// PNG on disk, named by a hash of its bytes.
        file: PathBuf,
        width: u32,
        height: u32,
        bytes: u64,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Entry {
    pub id: u64,
    /// The clip for text; a searchable label (paths, "Image W×H") otherwise.
    pub text: String,
    /// Unix time in milliseconds of the most recent copy of this clip.
    pub copied_at_ms: u64,
    #[serde(default = "one")]
    pub times_copied: u32,
    /// Localized name of the frontmost app when the clip was copied.
    #[serde(default)]
    pub source_app: Option<String>,
    #[serde(default)]
    pub content: Content,
}

fn one() -> u32 {
    1
}

impl Entry {
    /// Single-line label for list rows, truncated to `max_chars`.
    pub fn preview(&self, max_chars: usize) -> String {
        let line = match &self.content {
            Content::Text => self
                .text
                .lines()
                .map(str::trim)
                .find(|l| !l.is_empty())
                .unwrap_or("")
                .to_string(),
            Content::Files { paths } => paths
                .iter()
                .map(|p| file_name(p))
                .collect::<Vec<_>>()
                .join(", "),
            Content::Image { width, height, .. } => format!("Image {width}×{height}"),
        };
        let collapsed = line.split_whitespace().collect::<Vec<_>>().join(" ");
        let mut out: String = collapsed.chars().take(max_chars).collect();
        if collapsed.chars().count() > max_chars {
            out.push('…');
        }
        out
    }

    /// "Text", "Link", "File", "Files" or "Image".
    pub fn kind_label(&self) -> &'static str {
        match &self.content {
            Content::Text if self.is_url() => "Link",
            Content::Text => "Text",
            Content::Files { paths } if paths.len() == 1 => "File",
            Content::Files { .. } => "Files",
            Content::Image { .. } => "Image",
        }
    }

    pub fn is_text(&self) -> bool {
        matches!(self.content, Content::Text)
    }

    pub fn char_count(&self) -> usize {
        self.text.chars().count()
    }

    pub fn word_count(&self) -> usize {
        self.text.split_whitespace().count()
    }

    pub fn line_count(&self) -> usize {
        self.text.lines().count().max(1)
    }

    pub fn is_url(&self) -> bool {
        let t = self.text.trim();
        self.is_text()
            && !t.contains(char::is_whitespace)
            && (t.starts_with("http://") || t.starts_with("https://"))
    }

    fn image_file(&self) -> Option<&Path> {
        match &self.content {
            Content::Image { file, .. } => Some(file),
            _ => None,
        }
    }
}

pub fn file_name(path: &Path) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.display().to_string())
}

pub fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

const MINUTE_MS: u64 = 60_000;
const HOUR_MS: u64 = 60 * MINUTE_MS;

/// Section label in the style of Raycast's "Today" / "This Year" headers.
pub fn age_label(copied_at_ms: u64, now_ms: u64) -> &'static str {
    match now_ms.saturating_sub(copied_at_ms) {
        a if a < 2 * MINUTE_MS => "Just now",
        a if a < HOUR_MS => "Last hour",
        a if a < 6 * HOUR_MS => "Last 6 hours",
        _ => "Earlier",
    }
}

/// Compact relative time such as `12s`, `5m`, `3h`.
pub fn relative_time(copied_at_ms: u64, now_ms: u64) -> String {
    let age = now_ms.saturating_sub(copied_at_ms);
    if age < MINUTE_MS {
        format!("{}s", age / 1000)
    } else if age < HOUR_MS {
        format!("{}m", age / MINUTE_MS)
    } else {
        format!("{}h", age / HOUR_MS)
    }
}

/// `1.2 MB`, `340 KB`, `12 B`.
pub fn human_bytes(bytes: u64) -> String {
    const KB: f64 = 1024.0;
    let b = bytes as f64;
    if b >= KB * KB {
        format!("{:.1} MB", b / (KB * KB))
    } else if b >= KB {
        format!("{:.0} KB", b / KB)
    } else {
        format!("{bytes} B")
    }
}

#[derive(Deserialize)]
struct HistoryFile {
    #[allow(dead_code)]
    version: u32,
    entries: Vec<Entry>,
}

#[derive(Serialize)]
struct HistoryFileRef<'a> {
    version: u32,
    entries: &'a [Entry],
}

/// Newest-first list of clipboard entries.
#[derive(Debug)]
pub struct History {
    entries: Vec<Entry>,
    ttl: Duration,
    path: Option<PathBuf>,
    image_dir: PathBuf,
    next_id: u64,
}

impl History {
    /// No persistence; images go to a per-process temp directory.
    pub fn in_memory(ttl: Duration) -> Self {
        Self {
            entries: Vec::new(),
            ttl,
            path: None,
            image_dir: std::env::temp_dir().join(format!("paste-buff-{}", std::process::id())),
            next_id: 1,
        }
    }

    /// Load from `path`; a missing or unreadable file yields an empty history.
    /// Images live in `images/` next to the file.
    pub fn load(path: impl Into<PathBuf>, ttl: Duration) -> Self {
        let path = path.into();
        let image_dir = path
            .parent()
            .map(|d| d.join(IMAGE_DIR))
            .unwrap_or_else(|| PathBuf::from(IMAGE_DIR));
        let mut history = Self {
            entries: Vec::new(),
            ttl,
            path: Some(path.clone()),
            image_dir,
            next_id: 1,
        };
        match fs::read(&path) {
            Ok(bytes) => match serde_json::from_slice::<HistoryFile>(&bytes) {
                Ok(file) => history.entries = file.entries,
                Err(err) => eprintln!(
                    "paste-buff: ignoring unreadable history {}: {err}",
                    path.display()
                ),
            },
            Err(err) if err.kind() == io::ErrorKind::NotFound => {}
            Err(err) => eprintln!("paste-buff: cannot read {}: {err}", path.display()),
        }
        history
            .entries
            .retain(|e| e.image_file().map_or(true, |f| f.exists()));
        history
            .entries
            .sort_by(|a, b| b.copied_at_ms.cmp(&a.copied_at_ms));
        history.next_id = history.entries.iter().map(|e| e.id).max().unwrap_or(0) + 1;
        history.prune();
        history.sweep_orphan_images();
        history
    }

    pub fn ttl(&self) -> Duration {
        self.ttl
    }

    pub fn path(&self) -> Option<&Path> {
        self.path.as_deref()
    }

    pub fn image_dir(&self) -> &Path {
        &self.image_dir
    }

    pub fn entries(&self) -> &[Entry] {
        &self.entries
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn get(&self, id: u64) -> Option<&Entry> {
        self.entries.iter().find(|e| e.id == id)
    }

    /// Record a text clip. See [`History::push_clip`].
    pub fn push(&mut self, text: impl Into<String>, source_app: Option<String>) -> Option<u64> {
        self.push_clip(Clip::Text(text.into()), source_app)
    }

    /// Record a clip. Returns the entry id, or `None` when the clip is
    /// blank, empty or too large. Re-copying an existing clip moves it to
    /// the front and bumps `times_copied`.
    pub fn push_clip(&mut self, clip: Clip, source_app: Option<String>) -> Option<u64> {
        self.push_clip_at(clip, source_app, now_ms())
    }

    pub fn push_clip_at(
        &mut self,
        clip: Clip,
        source_app: Option<String>,
        now_ms: u64,
    ) -> Option<u64> {
        let (text, content) = match clip {
            Clip::Text(text) => {
                if text.trim().is_empty() || text.len() > MAX_TEXT_BYTES {
                    return None;
                }
                (text, Content::Text)
            }
            Clip::Files(paths) => {
                if paths.is_empty() {
                    return None;
                }
                let text = paths
                    .iter()
                    .map(|p| p.display().to_string())
                    .collect::<Vec<_>>()
                    .join("\n");
                (text, Content::Files { paths })
            }
            Clip::Image { png, width, height } => {
                if png.is_empty() || png.len() > MAX_IMAGE_BYTES {
                    return None;
                }
                let file = match self.store_image(&png) {
                    Ok(file) => file,
                    Err(err) => {
                        eprintln!("paste-buff: cannot store image: {err}");
                        return None;
                    }
                };
                (
                    format!("Image {width}×{height}"),
                    Content::Image {
                        file,
                        width,
                        height,
                        bytes: png.len() as u64,
                    },
                )
            }
        };

        let existing = self
            .entries
            .iter()
            .position(|e| e.text == text && e.content == content);
        let (id, times_copied) = match existing {
            Some(pos) => {
                let old = self.entries.remove(pos);
                (old.id, old.times_copied.saturating_add(1))
            }
            None => {
                let id = self.next_id;
                self.next_id += 1;
                (id, 1)
            }
        };
        self.entries.insert(
            0,
            Entry {
                id,
                text,
                copied_at_ms: now_ms,
                times_copied,
                source_app,
                content,
            },
        );
        if self.entries.len() > MAX_ENTRIES {
            let dropped: Vec<Entry> = self.entries.drain(MAX_ENTRIES..).collect();
            self.discard(&dropped);
        }
        Some(id)
    }

    pub fn remove(&mut self, id: u64) -> bool {
        let Some(pos) = self.entries.iter().position(|e| e.id == id) else {
            return false;
        };
        let dropped = self.entries.remove(pos);
        self.discard(std::slice::from_ref(&dropped));
        true
    }

    pub fn clear(&mut self) {
        let dropped = std::mem::take(&mut self.entries);
        self.discard(&dropped);
    }

    /// Drop expired entries (and their images); returns how many were removed.
    pub fn prune(&mut self) -> usize {
        self.prune_at(now_ms())
    }

    pub fn prune_at(&mut self, now_ms: u64) -> usize {
        let ttl_ms = self.ttl.as_millis() as u64;
        let (keep, dropped): (Vec<Entry>, Vec<Entry>) = std::mem::take(&mut self.entries)
            .into_iter()
            .partition(|e| now_ms.saturating_sub(e.copied_at_ms) < ttl_ms);
        self.entries = keep;
        self.discard(&dropped);
        dropped.len()
    }

    /// Case-insensitive match; every whitespace-separated term must appear.
    pub fn search(&self, query: &str) -> Vec<&Entry> {
        let query = query.trim().to_lowercase();
        if query.is_empty() {
            return self.entries.iter().collect();
        }
        let terms: Vec<&str> = query.split_whitespace().collect();
        self.entries
            .iter()
            .filter(|e| {
                let haystack = e.text.to_lowercase();
                terms.iter().all(|t| haystack.contains(t))
            })
            .collect()
    }

    /// Atomically write the history (temp file + rename). No-op when in-memory.
    pub fn save(&self) -> io::Result<()> {
        let Some(path) = &self.path else {
            return Ok(());
        };
        if let Some(dir) = path.parent() {
            fs::create_dir_all(dir)?;
        }
        let file = HistoryFileRef {
            version: FILE_VERSION,
            entries: &self.entries,
        };
        let bytes = serde_json::to_vec(&file).map_err(io::Error::other)?;
        let tmp = path.with_extension("json.tmp");
        fs::write(&tmp, bytes)?;
        fs::rename(&tmp, path)
    }

    /// Write `png` under the image dir, named by a hash of its bytes, so the
    /// same image copied twice shares one file.
    fn store_image(&self, png: &[u8]) -> io::Result<PathBuf> {
        let mut hasher = DefaultHasher::new();
        png.hash(&mut hasher);
        let file = self.image_dir.join(format!("{:016x}.png", hasher.finish()));
        if !file.exists() {
            fs::create_dir_all(&self.image_dir)?;
            let tmp = file.with_extension("png.tmp");
            fs::write(&tmp, png)?;
            fs::rename(&tmp, &file)?;
        }
        Ok(file)
    }

    /// Delete image files of `dropped` entries that no remaining entry uses.
    fn discard(&self, dropped: &[Entry]) {
        let still_used: HashSet<&Path> = self.entries.iter().filter_map(Entry::image_file).collect();
        for file in dropped.iter().filter_map(Entry::image_file) {
            if !still_used.contains(file) {
                let _ = fs::remove_file(file);
            }
        }
    }

    /// Remove files in the image dir that no entry references.
    fn sweep_orphan_images(&self) {
        let Ok(dir) = fs::read_dir(&self.image_dir) else {
            return;
        };
        let used: HashSet<PathBuf> = self
            .entries
            .iter()
            .filter_map(|e| e.image_file().map(Path::to_path_buf))
            .collect();
        for entry in dir.flatten() {
            let path = entry.path();
            if path.extension().is_some_and(|x| x == "png") && !used.contains(&path) {
                let _ = fs::remove_file(&path);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_path(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "paste-buff-test-{}-{}",
            std::process::id(),
            name
        ));
        let _ = fs::remove_dir_all(&dir);
        dir.join("history.json")
    }

    fn png(seed: u8) -> Clip {
        Clip::Image {
            png: vec![seed; 64],
            width: 8,
            height: 2,
        }
    }

    #[test]
    fn push_dedupes_and_moves_to_front() {
        let mut h = History::in_memory(DEFAULT_TTL);
        let a = h.push_clip_at(Clip::Text("alpha".into()), None, 1_000).unwrap();
        let b = h.push_clip_at(Clip::Text("beta".into()), None, 2_000).unwrap();
        assert_eq!(h.entries()[0].id, b);
        let again = h
            .push_clip_at(Clip::Text("alpha".into()), Some("Safari".into()), 3_000)
            .unwrap();
        assert_eq!(again, a, "re-copy keeps the original id");
        assert_eq!(h.len(), 2);
        assert_eq!(h.entries()[0].id, a);
        assert_eq!(h.entries()[0].times_copied, 2);
        assert_eq!(h.entries()[0].copied_at_ms, 3_000);
        assert_eq!(h.entries()[0].source_app.as_deref(), Some("Safari"));
    }

    #[test]
    fn rejects_blank_and_oversized() {
        let mut h = History::in_memory(DEFAULT_TTL);
        assert!(h.push("   \n", None).is_none());
        assert!(h.push("x".repeat(MAX_TEXT_BYTES + 1), None).is_none());
        assert!(h.push_clip(Clip::Files(vec![]), None).is_none());
        assert!(h
            .push_clip(
                Clip::Image {
                    png: vec![0; MAX_IMAGE_BYTES + 1],
                    width: 1,
                    height: 1
                },
                None
            )
            .is_none());
        assert!(h.is_empty());
    }

    #[test]
    fn files_are_labelled_and_searchable_by_path() {
        let mut h = History::in_memory(DEFAULT_TTL);
        let paths = vec![PathBuf::from("/tmp/a/report.pdf"), PathBuf::from("/tmp/b/photo.jpg")];
        h.push_clip(Clip::Files(paths.clone()), Some("Finder".into()));
        let e = &h.entries()[0];
        assert_eq!(e.content, Content::Files { paths: paths.clone() });
        assert_eq!(e.preview(80), "report.pdf, photo.jpg");
        assert_eq!(e.kind_label(), "Files");
        assert!(!e.is_url());
        assert_eq!(h.search("photo").len(), 1);
        assert_eq!(h.search("/tmp/a").len(), 1);
        // Same paths again: dedupe, not a second entry.
        h.push_clip(Clip::Files(paths), None);
        assert_eq!(h.len(), 1);
        assert_eq!(h.entries()[0].times_copied, 2);
    }

    #[test]
    fn images_are_stored_once_and_deleted_with_their_entry() {
        let path = temp_path("images");
        let mut h = History::load(&path, DEFAULT_TTL);
        let id = h.push_clip(png(1), None).unwrap();
        let file = match &h.entries()[0].content {
            Content::Image { file, width, height, bytes } => {
                assert_eq!((*width, *height, *bytes), (8, 2, 64));
                file.clone()
            }
            other => panic!("expected image, got {other:?}"),
        };
        assert!(file.exists());
        assert!(file.starts_with(h.image_dir()));
        assert_eq!(h.entries()[0].preview(80), "Image 8×2");
        assert_eq!(h.entries()[0].kind_label(), "Image");

        // Same bytes again: same file, one entry.
        h.push_clip(png(1), None);
        assert_eq!(h.len(), 1);
        assert_eq!(fs::read_dir(h.image_dir()).unwrap().count(), 1);

        // Different bytes: second file.
        h.push_clip(png(2), None);
        assert_eq!(fs::read_dir(h.image_dir()).unwrap().count(), 2);

        assert!(h.remove(id));
        assert!(!file.exists(), "image file removed with its entry");
        assert_eq!(fs::read_dir(h.image_dir()).unwrap().count(), 1);

        h.clear();
        assert_eq!(fs::read_dir(h.image_dir()).unwrap().count(), 0);
        let _ = fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn prune_removes_expired_images_and_load_sweeps_orphans() {
        let path = temp_path("prune-images");
        let mut h = History::load(&path, Duration::from_secs(60));
        let now = now_ms();
        h.push_clip_at(png(3), None, now - 70_000);
        h.push_clip_at(png(4), None, now - 10_000);
        assert_eq!(fs::read_dir(h.image_dir()).unwrap().count(), 2);
        assert_eq!(h.prune_at(now), 1);
        assert_eq!(fs::read_dir(h.image_dir()).unwrap().count(), 1);
        h.save().unwrap();

        // An orphan file (never referenced) and a missing file for an entry.
        fs::write(h.image_dir().join("deadbeef00000000.png"), b"orphan").unwrap();
        let reloaded = History::load(&path, DEFAULT_TTL);
        assert_eq!(reloaded.len(), 1);
        assert_eq!(fs::read_dir(reloaded.image_dir()).unwrap().count(), 1);
        let file = reloaded.entries()[0].image_file().unwrap().to_path_buf();
        fs::remove_file(&file).unwrap();
        let reloaded = History::load(&path, DEFAULT_TTL);
        assert!(reloaded.is_empty(), "entry whose image vanished is dropped");
        let _ = fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn prune_drops_entries_older_than_ttl() {
        let mut h = History::in_memory(Duration::from_secs(60));
        h.push_clip_at(Clip::Text("old".into()), None, 0);
        h.push_clip_at(Clip::Text("fresh".into()), None, 50_000);
        assert_eq!(h.prune_at(60_000), 1);
        assert_eq!(h.entries()[0].text, "fresh");
        assert_eq!(h.prune_at(110_000), 1);
        assert!(h.is_empty());
    }

    #[test]
    fn caps_entry_count() {
        let mut h = History::in_memory(DEFAULT_TTL);
        for i in 0..(MAX_ENTRIES + 10) {
            h.push_clip_at(Clip::Text(format!("entry {i}")), None, i as u64);
        }
        assert_eq!(h.len(), MAX_ENTRIES);
        assert_eq!(h.entries()[0].text, format!("entry {}", MAX_ENTRIES + 9));
    }

    #[test]
    fn search_is_case_insensitive_and_multi_term() {
        let mut h = History::in_memory(DEFAULT_TTL);
        h.push_clip_at(Clip::Text("Hello World".into()), None, 1);
        h.push_clip_at(Clip::Text("hello there".into()), None, 2);
        h.push_clip_at(Clip::Text("unrelated".into()), None, 3);
        assert_eq!(h.search("").len(), 3);
        assert_eq!(h.search("HELLO").len(), 2);
        assert_eq!(h.search("hello world").len(), 1);
        assert_eq!(h.search("zzz").len(), 0);
    }

    #[test]
    fn save_and_load_roundtrip() {
        let path = temp_path("roundtrip");
        let mut h = History::load(&path, DEFAULT_TTL);
        assert!(h.is_empty());
        h.push("first", Some("Terminal".into()));
        h.push_clip(Clip::Files(vec![PathBuf::from("/tmp/x.txt")]), None);
        h.save().unwrap();

        let loaded = History::load(&path, DEFAULT_TTL);
        assert_eq!(loaded.len(), 2);
        assert_eq!(loaded.entries()[0].kind_label(), "File");
        assert_eq!(loaded.entries()[1].source_app.as_deref(), Some("Terminal"));
        assert_eq!(loaded.next_id, 3, "ids continue after reload");
        let _ = fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn loads_pre_content_files_as_text() {
        let path = temp_path("legacy");
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(
            &path,
            br#"{"version":1,"entries":[{"id":7,"text":"legacy","copied_at_ms":99999999999999}]}"#,
        )
        .unwrap();
        let h = History::load(&path, DEFAULT_TTL);
        assert_eq!(h.len(), 1);
        assert_eq!(h.entries()[0].content, Content::Text);
        assert_eq!(h.entries()[0].times_copied, 1);
        let _ = fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn load_prunes_expired_and_tolerates_garbage() {
        let path = temp_path("garbage");
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, b"not json").unwrap();
        assert!(History::load(&path, DEFAULT_TTL).is_empty());

        let mut h = History::load(&path, Duration::from_secs(1));
        h.push_clip_at(Clip::Text("stale".into()), None, 0);
        h.save().unwrap();
        assert!(History::load(&path, Duration::from_secs(1)).is_empty());
        let _ = fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn preview_and_stats() {
        let e = Entry {
            id: 1,
            text: "\n\n   first   line here\nsecond line".into(),
            copied_at_ms: 0,
            times_copied: 1,
            source_app: None,
            content: Content::Text,
        };
        assert_eq!(e.preview(100), "first line here");
        assert_eq!(e.preview(5), "first…");
        assert_eq!(e.word_count(), 5);
        assert_eq!(e.line_count(), 4);
        assert!(!e.is_url());
        assert_eq!(e.kind_label(), "Text");
        assert_eq!(age_label(0, 1_000), "Just now");
        assert_eq!(age_label(0, 7 * HOUR_MS), "Earlier");
        assert_eq!(relative_time(0, 90 * MINUTE_MS), "1h");
        assert_eq!(human_bytes(512), "512 B");
        assert_eq!(human_bytes(2048), "2 KB");
        assert_eq!(human_bytes(3 * 1024 * 1024), "3.0 MB");
    }
}

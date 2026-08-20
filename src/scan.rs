//! Background filesystem scanning: builds a bounded `DirNode` tree that the
//! city generator turns into districts and buildings.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::{Receiver, Sender};
use std::sync::{Arc, Mutex};

use bevy::prelude::*;

/// Broad category a file belongs to; drives shape, color and interaction.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FileKind {
    Image,
    Text,
    Code,
    Audio,
    Video,
    Archive,
    Executable,
    Data,
    Other,
}

impl FileKind {
    pub fn label(&self) -> &'static str {
        match self {
            FileKind::Image => "Image",
            FileKind::Text => "Text",
            FileKind::Code => "Code",
            FileKind::Audio => "Audio",
            FileKind::Video => "Video",
            FileKind::Archive => "Archive",
            FileKind::Executable => "Executable",
            FileKind::Data => "Data",
            FileKind::Other => "File",
        }
    }
}

const IMAGE_EXT: &[&str] = &[
    "png", "jpg", "jpeg", "gif", "webp", "bmp", "tiff", "tif", "ico", "svg",
];
const TEXT_EXT: &[&str] = &[
    "txt", "md", "markdown", "rst", "log", "tex", "org", "csv", "tsv", "rtf",
];
const CODE_EXT: &[&str] = &[
    "rs", "py", "js", "ts", "jsx", "tsx", "c", "h", "cpp", "hpp", "cc", "java", "kt", "swift",
    "go", "rb", "php", "cs", "sh", "zsh", "bash", "fish", "lua", "vim", "el", "clj", "ex", "exs",
    "erl", "hs", "ml", "scala", "sql", "html", "css", "scss", "sass", "less", "vue", "svelte",
    "json", "yaml", "yml", "toml", "xml", "ini", "cfg", "conf", "make", "cmake", "dockerfile",
    "gradle", "proto", "graphql", "zig", "nim", "dart", "r", "jl", "asm", "s", "plist",
];
const AUDIO_EXT: &[&str] = &["mp3", "wav", "flac", "ogg", "oga", "m4a", "aac", "aiff", "aif"];
const VIDEO_EXT: &[&str] = &["mp4", "mov", "mkv", "avi", "webm", "m4v", "flv", "wmv", "mpg"];
const ARCHIVE_EXT: &[&str] = &[
    "zip", "tar", "gz", "bz2", "xz", "zst", "7z", "rar", "dmg", "iso", "pkg", "deb", "rpm",
    "jar", "whl", "crate", "tgz",
];
const EXEC_EXT: &[&str] = &["app", "exe", "bin", "dylib", "so", "dll", "wasm", "o", "a"];
const DATA_EXT: &[&str] = &[
    "db", "sqlite", "sqlite3", "parquet", "arrow", "pdf", "doc", "docx", "xls", "xlsx", "ppt",
    "pptx", "key", "pages", "numbers", "epub", "dat", "pickle", "pkl", "npy", "npz",
    "onnx", "pt", "pth", "gguf", "safetensors", "ttf", "otf", "woff", "woff2", "heic", "heif",
];

/// Special file names (no useful extension) that are really text/code.
const TEXTY_NAMES: &[&str] = &[
    "readme", "license", "changelog", "makefile", "dockerfile", "cargo.lock", "gemfile",
    "rakefile", "justfile", "notice", "authors", "todo",
];

pub fn classify(path: &Path) -> FileKind {
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().to_lowercase())
        .unwrap_or_default();
    if TEXTY_NAMES.iter().any(|t| name == *t || name.starts_with(&format!("{t}."))) {
        return FileKind::Text;
    }
    let ext = path
        .extension()
        .map(|e| e.to_string_lossy().to_lowercase())
        .unwrap_or_default();
    let ext = ext.as_str();
    if IMAGE_EXT.contains(&ext) {
        FileKind::Image
    } else if TEXT_EXT.contains(&ext) {
        FileKind::Text
    } else if CODE_EXT.contains(&ext) {
        FileKind::Code
    } else if AUDIO_EXT.contains(&ext) {
        FileKind::Audio
    } else if VIDEO_EXT.contains(&ext) {
        FileKind::Video
    } else if ARCHIVE_EXT.contains(&ext) {
        FileKind::Archive
    } else if EXEC_EXT.contains(&ext) {
        FileKind::Executable
    } else if DATA_EXT.contains(&ext) {
        FileKind::Data
    } else {
        FileKind::Other
    }
}

#[derive(Debug, Clone)]
pub struct FileEntry {
    pub name: String,
    pub path: PathBuf,
    pub size: u64,
    pub kind: FileKind,
}

#[derive(Debug, Clone)]
pub struct DirNode {
    pub name: String,
    pub path: PathBuf,
    pub files: Vec<FileEntry>,
    pub dirs: Vec<DirNode>,
}

impl DirNode {
    pub fn file_count(&self) -> usize {
        self.files.len() + self.dirs.iter().map(|d| d.file_count()).sum::<usize>()
    }
}

/// Directories that are huge, noisy, or uninteresting to walk through.
const SKIP_DIRS: &[&str] = &[
    "node_modules",
    "target",
    "library",
    "applications",
    "__pycache__",
    "venv",
    ".venv",
    "build",
    "dist",
    "deriveddata",
    "pods",
];

#[derive(Resource, Clone)]
pub struct ScanConfig {
    pub root: PathBuf,
    pub max_depth: usize,
    pub max_files: usize,
    /// Debug: save a screenshot here shortly after the city loads, then exit.
    pub shot: Option<PathBuf>,
    /// Camera preset for `--shot`: street, neon, gallery, aerial, alley.
    pub shot_view: String,
    /// Starting time of day, 0..1 (0 = midnight, 0.5 = noon).
    pub tod: Option<f32>,
}

impl Default for ScanConfig {
    fn default() -> Self {
        let home = std::env::var("HOME").unwrap_or_else(|_| ".".into());
        Self {
            root: PathBuf::from(home),
            max_depth: 3,
            max_files: 1600,
            shot: None,
            shot_view: "street".into(),
            tod: None,
        }
    }
}

const MAX_FILES_PER_DIR: usize = 26;
const MAX_SUBDIRS_PER_DIR: usize = 10;

fn scan_dir(
    path: &Path,
    depth: usize,
    cfg: &ScanConfig,
    total: &mut usize,
    progress: &AtomicUsize,
) -> Option<DirNode> {
    if *total >= cfg.max_files {
        return None;
    }
    let entries = std::fs::read_dir(path).ok()?;
    let mut files: Vec<FileEntry> = Vec::new();
    let mut subdir_paths: Vec<PathBuf> = Vec::new();

    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        if name.starts_with('.') {
            continue;
        }
        // Skip symlinks entirely to avoid cycles and double counting.
        let Ok(ft) = entry.file_type() else { continue };
        if ft.is_symlink() {
            continue;
        }
        if ft.is_dir() {
            if depth < cfg.max_depth && !SKIP_DIRS.contains(&name.to_lowercase().as_str()) {
                subdir_paths.push(entry.path());
            }
        } else if ft.is_file() {
            let Ok(meta) = entry.metadata() else { continue };
            files.push(FileEntry {
                kind: classify(&entry.path()),
                name,
                path: entry.path(),
                size: meta.len(),
            });
        }
    }

    // Keep the biggest files: they make the most interesting buildings.
    files.sort_by(|a, b| b.size.cmp(&a.size));
    files.truncate(MAX_FILES_PER_DIR);
    let room = cfg.max_files.saturating_sub(*total);
    files.truncate(room);
    *total += files.len();
    progress.store(*total, Ordering::Relaxed);

    subdir_paths.sort();
    subdir_paths.truncate(MAX_SUBDIRS_PER_DIR);
    let mut dirs = Vec::new();
    for sub in subdir_paths {
        if *total >= cfg.max_files {
            break;
        }
        if let Some(node) = scan_dir(&sub, depth + 1, cfg, total, progress) {
            // Empty branches would generate dead districts; drop them.
            if node.file_count() > 0 {
                dirs.push(node);
            }
        }
    }

    Some(DirNode {
        name: path
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| path.to_string_lossy().to_string()),
        path: path.to_path_buf(),
        files,
        dirs,
    })
}

/// Handle to the scan running on a background thread.
#[derive(Resource)]
pub struct ScanTask {
    pub receiver: Mutex<Receiver<DirNode>>,
    pub progress: Arc<AtomicUsize>,
}

pub fn start_scan(cfg: &ScanConfig) -> ScanTask {
    let (tx, rx): (Sender<DirNode>, Receiver<DirNode>) = std::sync::mpsc::channel();
    let progress = Arc::new(AtomicUsize::new(0));
    let progress2 = progress.clone();
    let cfg = cfg.clone();
    std::thread::spawn(move || {
        let mut total = 0usize;
        let root = scan_dir(&cfg.root, 0, &cfg, &mut total, &progress2).unwrap_or(DirNode {
            name: cfg.root.to_string_lossy().to_string(),
            path: cfg.root.clone(),
            files: Vec::new(),
            dirs: Vec::new(),
        });
        let _ = tx.send(root);
    });
    ScanTask {
        receiver: Mutex::new(rx),
        progress,
    }
}

pub fn human_size(bytes: u64) -> String {
    const UNITS: &[&str] = &["B", "KB", "MB", "GB", "TB"];
    let mut v = bytes as f64;
    let mut unit = 0;
    while v >= 1024.0 && unit < UNITS.len() - 1 {
        v /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} B")
    } else {
        format!("{:.1} {}", v, UNITS[unit])
    }
}

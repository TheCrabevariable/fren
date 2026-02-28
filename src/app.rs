use std::{
    fmt, fs, fs::File, io, io::BufRead, io::BufReader, io::Write, path::Path, path::PathBuf,
    process::Command,
    thread,
    env,
};
use std::collections::HashMap;

use ratatui::layout::Rect;
use ratatui_image::protocol::Protocol;
use ratatui_image::picker::Picker;
use std::sync::mpsc::{self, Sender};
use std::num::NonZeroUsize;
use lru::LruCache;
use std::sync::{Arc, Mutex, atomic::{AtomicU64, Ordering}};
use image::GenericImageView;
use image::ImageReader;

//
// SORT MODE
//
#[derive(Clone, Copy, Debug)]
pub enum SortMode {
    Name,
    Size,
    Modified,
}

//
// CLIPBOARD MODE
//
#[derive(Clone)]
pub enum ClipboardMode {
    Copy,
    Cut,
}

#[derive(Clone, PartialEq)]
pub enum AppMode {
    Normal,
    Input(InputAction),
}

#[derive(Clone, PartialEq)]
pub enum InputAction {
    Rename,
    CreateFile,
    CreateFolder,
    ConfirmDelete,
    OpenWith,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Focus {
    Files,
    Pinned,
}
#[derive(Hash, Eq, PartialEq, Clone)]
pub struct ImageKey {
    pub path: PathBuf,
    pub width: u16,
    pub height: u16,
}
//problems with kitty dumb fonts
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IconMode {
    Ascii,
    Emoji,
    Nerd,
}
pub struct PreviewJob {
    pub request_id: u64,
    pub path: PathBuf,
    pub inner: Rect,
    pub is_pdf: bool,
}

pub struct App {
    pub current_dir: PathBuf,
    pub entries: Vec<fs::DirEntry>,
    pub selected: usize,
    pub sort_mode: SortMode,
    pub clipboard: Option<(PathBuf, ClipboardMode)>,
    pub show_hidden: bool,
    pub mode: AppMode,
    pub input: String,
    pub focus: Focus,
    pub pinned: Vec<PathBuf>,
    pub pinned_selected: usize,
    pub show_help: bool,
    pub preview_rect: Rect,
    pub image_loaded: bool,
    pub image_id: u32,
    pub current_image: Option<std::path::PathBuf>,
    pub image: Option<Protocol>,
    pub image_path: Option<std::path::PathBuf>,
    pub picker: Picker,
    pub image_rx: Option<mpsc::Receiver<(u64, Option<Protocol>)>>,
    pub image_tx: mpsc::Sender<(u64, Option<Protocol>)>,
    pub image_loading: bool,
    pub image_cache: Arc<Mutex<LruCache<ImageKey, Protocol>>>,
    pub preview_deadline: Option<std::time::Instant>,
    pub image_size: Option<(u16, u16)>,
    pub image_jobs: usize,
    pub image_request_id: u64,
    pub image_request_atomic: Arc<AtomicU64>,
    pub icon_mode: IconMode,
    pub cursor_memory: HashMap<PathBuf, usize>,
    pub preview_job_tx: Sender<PreviewJob>,
}

impl App {
    pub fn new() -> io::Result<Self> {
        let current_dir = std::env::current_dir()?;
        let show_hidden = false;

        let (image_tx, image_rx) = mpsc::channel::<(u64, Option<Protocol>)>();
        let (job_tx, job_rx) = mpsc::channel::<PreviewJob>();


        let cancel_token = Arc::new(AtomicU64::new(0));
        let worker_cancel = cancel_token.clone();

        let entries = Self::read_dir(&current_dir, SortMode::Name, show_hidden)?;
        let picker = Picker::from_query_stdio().unwrap();
        let cache_size = NonZeroUsize::new(128).unwrap();
        let picker_clone = picker.clone();
        let cache_clone = Arc::new(Mutex::new(LruCache::new(cache_size)));
        let worker_cache = cache_clone.clone();
        let result_tx = image_tx.clone();

        //worker thread
        thread::spawn(move || {
            use image::ImageReader;

            while let Ok(mut job) = job_rx.recv() {

                while let Ok(newer) = job_rx.try_recv() {
                    job = newer;
                }

                let request_id = job.request_id;

                if worker_cancel.load(Ordering::Relaxed) != request_id {
                    continue;
                }

                let result = (|| {

                    let max_w = (job.inner.width as u32 * 8).min(2048).max(1);
                    let max_h = (job.inner.height as u32 * 16).min(2048).max(1);

                    //
                    // PDF BRANCH
                    //
                    let decoded = if job.is_pdf {

                        let tmp_base = format!("/tmp/fm_preview_{}", request_id);

                        let status = std::process::Command::new("pdftoppm")
                            .arg("-png")
                            .arg("-singlefile")
                            .arg("-r")
                            .arg("96")
                            .arg(&job.path)
                            .arg(&tmp_base)
                            .status()
                            .ok()?;

                        if !status.success() {
                            return None;
                        }

                        let tmp_png = format!("{}.png", tmp_base);

                        let img = image::open(&tmp_png).ok()?;

                        let _ = std::fs::remove_file(&tmp_png);

                        img

                    } else {
                        //
                        // Normal image branch
                        //
                        let reader = ImageReader::open(&job.path).ok()?;
                        reader.decode().ok()?
                    };
                    let (w, h) = decoded.dimensions();

                    if worker_cancel.load(Ordering::Relaxed) != request_id {
                        return None;
                    }

                    let resized = if w <= max_w && h <= max_h {
                        decoded
                    } else {
                        decoded.thumbnail(max_w, max_h)
                    };

                    let protocol = picker_clone
                        .new_protocol(resized, job.inner, ratatui_image::Resize::Fit(None))
                        .ok()?;

                    Some(protocol)
                })();
                if let Some(ref protocol) = result {
                    worker_cache.lock().unwrap().put(
                        ImageKey {
                            path: job.path.clone(),
                            width: quantize(job.inner.width),
                            height: quantize(job.inner.height),
                        },
                        protocol.clone(),
                    );
                }

                let _ = result_tx.send((request_id, result));
            }
        });

        Ok(Self {
            current_dir,
            entries,
            selected: 0,
            sort_mode: SortMode::Name,
            clipboard: None,
            mode: AppMode::Normal,
            input: String::new(),
            show_hidden,
            focus: Focus::Files,
            pinned: dirs::home_dir().into_iter().collect(),
            pinned_selected: 0,
            show_help: false,
            preview_rect: Rect::default(),
            image_loaded: false,
            image_id: 0,
            current_image: None,
            picker,
            image: None,
            image_path: None,
            image_tx,
            image_rx: Some(image_rx),
            image_loading: false,
            image_cache: cache_clone,
            preview_deadline: None,
            image_size: None,
            image_jobs: 0,
            image_request_id: 0,
            image_request_atomic: cancel_token,
            icon_mode: detect_icon_mode(),
            cursor_memory: HashMap::new(),
            preview_job_tx: job_tx,
        })
    }
    //save pin dir
    pub fn save_pinned(&self) -> io::Result<()> {
        let path = dirs::config_dir()
            .unwrap_or(std::path::PathBuf::from("."))
            .join("fren")
            .join("pinned.txt");

        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }

        let mut file = File::create(path)?;

        for dir in &self.pinned {
            writeln!(file, "{}", dir.display())?;
        }

        Ok(())
    }
    pub fn load_pinned(&mut self) -> io::Result<()> {
        let path = dirs::config_dir()
            .unwrap_or(std::path::PathBuf::from("."))
            .join("fren")
            .join("pinned.txt");

        if !path.exists() {
            return Ok(());
        }

        let file = File::open(path)?;
        let reader = BufReader::new(file);

        self.pinned.clear();

        for line in reader.lines() {
            let line = line?;
            let path = std::path::PathBuf::from(line);
            if path.exists() {
                self.pinned.push(path);
            }
        }

        Ok(())
    }

    fn read_dir(
        path: &PathBuf,
        mode: SortMode,
        show_hidden: bool,
    ) -> io::Result<Vec<fs::DirEntry>> {
        use std::cmp::Ordering;
        use std::fs;

        let mut entries: Vec<_> = fs::read_dir(path)?
            .filter_map(Result::ok)
            .filter(|e| {
                if let Some(name) = e.file_name().to_str() {
                    if !show_hidden && name.starts_with('.') {
                        return false;
                    }
                }
                true
            })
            .collect();

        //
        // PRIMARY SORT
        //
        match mode {
            SortMode::Name => {
                entries.sort_by(|a, b| {
                    let a_name = a.file_name().to_string_lossy().to_string();
                    let b_name = b.file_name().to_string_lossy().to_string();
                    natord::compare_ignore_case(&a_name, &b_name)
                });
            }
            SortMode::Size => {
                entries.sort_by_key(|e| e.metadata().map(|m| m.len()).unwrap_or(0));
            }
            SortMode::Modified => {
                entries.sort_by_key(|e| e.metadata().and_then(|m| m.modified()).ok());
            }
        }

        //
        // SECONDARY SORT: directories first (stable)
        //
        entries.sort_by(|a, b| {
            let a_dir = a.file_type().map(|t| t.is_dir()).unwrap_or(false);
            let b_dir = b.file_type().map(|t| t.is_dir()).unwrap_or(false);

            if a_dir != b_dir {
                return if a_dir {
                    Ordering::Less
                } else {
                    Ordering::Greater
                };
            }

            Ordering::Equal // keep previous ordering within groups
        });

        Ok(entries)
    }

    pub fn refresh(&mut self) -> io::Result<()> {
        // reload entries first
        self.entries = Self::read_dir(&self.current_dir, self.sort_mode, self.show_hidden)?;

        // restore cursor if we have memory
        if let Some(&pos) = self.cursor_memory.get(&self.current_dir) {
            self.selected = pos.min(self.entries.len().saturating_sub(1));
        } else {
            self.selected = 0;
        }

        Ok(())
    }

    pub fn toggle_hidden(&mut self) -> io::Result<()> {
        self.show_hidden = !self.show_hidden;
        self.refresh()
    }

    pub fn cycle_sort(&mut self) -> io::Result<()> {
        self.sort_mode = match self.sort_mode {
            SortMode::Name => SortMode::Size,
            SortMode::Size => SortMode::Modified,
            SortMode::Modified => SortMode::Name,
        };
        self.refresh()
    }

    pub fn copy_selected(&mut self) {
        if let Some(entry) = self.entries.get(self.selected) {
            self.clipboard = Some((entry.path(), ClipboardMode::Copy));
        }
    }

    pub fn cut_selected(&mut self) {
        if let Some(entry) = self.entries.get(self.selected) {
            self.clipboard = Some((entry.path(), ClipboardMode::Cut));
        }
    }

    pub fn paste(&mut self) -> io::Result<()> {
        if let Some((source, mode)) = self.clipboard.clone() {
            let file_name = match source.file_name() {
                Some(name) => name,
                None => return Ok(()),
            };

            let destination = self.current_dir.join(file_name);

            if destination == source || destination.exists() {
                return Ok(());
            }

            match mode {
                ClipboardMode::Copy => Self::copy_recursively(&source, &destination)?,
                ClipboardMode::Cut => {
                    fs::rename(&source, &destination)?;
                    self.clipboard = None;
                }
            }

            self.refresh()?;
        }

        Ok(())
    }

    fn copy_recursively(src: &Path, dst: &Path) -> io::Result<()> {
        if src.is_file() {
            fs::copy(src, dst)?;
        } else if src.is_dir() {
            fs::create_dir_all(dst)?;
            for entry in fs::read_dir(src)? {
                let entry = entry?;
                let new_dst = dst.join(entry.file_name());
                Self::copy_recursively(&entry.path(), &new_dst)?;
            }
        }
        Ok(())
    }
    fn trash_path() -> PathBuf {
        if let Ok(home) = env::var("HOME") {
            PathBuf::from(home)
                .join(".local/share/Trash/files")
        } else {
            PathBuf::from(".trash")
        }
    }

    pub fn trash_selected(&mut self) -> io::Result<()> {
        if let Some(entry) = self.entries.get(self.selected) {
            let source = entry.path();
            let trash_dir = Self::trash_path();

            fs::create_dir_all(&trash_dir)?;

            let file_name = source.file_name().unwrap();
            let mut target = trash_dir.join(file_name);

            // Avoid overwrite if same name exists
            let mut counter = 1;
            while target.exists() {
                let new_name = format!(
                    "{}_{}",
                    file_name.to_string_lossy(),
                    counter
                );
                target = trash_dir.join(new_name);
                counter += 1;
            }

            fs::rename(source, target)?;
        }

        self.refresh()
    }

    pub fn enter(&mut self) -> io::Result<()> {
        if let Some(entry) = self.entries.get(self.selected) {
            let path = entry.path();

            if path.is_dir() {
                self.current_dir = path;
                self.refresh()?;
            } else if path.is_file() {
                self.open_with_program("xdg-open")?;
            }
        }
        Ok(())
    }

    pub fn up(&mut self) -> io::Result<()> {
        if let Some(parent) = self.current_dir.parent() {
            self.current_dir = parent.to_path_buf();
            self.refresh()?;
        }
        Ok(())
    }

    pub fn open_with_program(&self, program: &str) -> io::Result<()> {
        if let Some(entry) = self.entries.get(self.selected) {
            Command::new(program).arg(entry.path()).spawn()?;
        }
        Ok(())
    }

    pub fn create_folder(&mut self, name: &str) -> io::Result<()> {
        let new_path = self.current_dir.join(name);
        if !new_path.exists() {
            fs::create_dir(&new_path)?;
        }
        self.refresh()
    }

    pub fn create_file(&mut self, name: &str) -> io::Result<()> {
        let new_path = self.current_dir.join(name);
        if !new_path.exists() {
            File::create(&new_path)?;
        }
        self.refresh()
    }

    pub fn start_input(&mut self, action: InputAction, prefill: Option<String>) {
        self.input = prefill.unwrap_or_default();
        self.mode = AppMode::Input(action);
    }

    pub fn confirm_rename(&mut self) -> io::Result<()> {
        if let Some(entry) = self.entries.get(self.selected) {
            let old_path = entry.path();
            let new_path = self.current_dir.join(&self.input);
            fs::rename(old_path, new_path)?;
        }

        self.mode = AppMode::Normal;
        self.input.clear();
        self.refresh()
    }

    pub fn open_pinned(&mut self) -> io::Result<()> {
        if let Some(path) = self.pinned.get(self.pinned_selected) {
            self.current_dir = path.clone();
            self.refresh()?;
        }
        Ok(())
    }

    pub fn pin_selected(&mut self) {
        if let Some(entry) = self.entries.get(self.selected) {
            let path = entry.path();
            if path.is_dir() && !self.pinned.contains(&path) {
                self.pinned.push(path);
                let _ = self.save_pinned();
            }
        }
    }

    pub fn unpin_selected(&mut self) {
        if self.pinned_selected < self.pinned.len() {
            self.pinned.remove(self.pinned_selected);
            let _ = self.save_pinned();
            if self.pinned_selected > 0 {
                self.pinned_selected -= 1;
            }
        }
    }
    pub fn icon_for(path: &std::path::Path, mode: IconMode) -> &'static str {
        match mode {
            IconMode::Ascii => Self::ascii_icon(path),
            IconMode::Emoji => Self::emoji_icon(path),
            IconMode::Nerd => Self::nerd_icon(path),
        }
    }
    pub fn emoji_icon(path: &Path) -> &'static str {
        if path.is_dir() {
            return "📁 ";
        }

        match path.extension().and_then(|s| s.to_str()).unwrap_or("") {
            "png" | "jpg" | "jpeg" | "webp" | "gif" => "🖼  ",
            "mp3" | "wav" | "flac" => "🎵 ",
            "mp4" | "mkv" | "mov" => "🎬 ",
            "zip" | "tar" | "gz" | "rar" => "📦 ",
            "rs" => "🦀 ",
            "c" | "cpp" | "h" => "💻 ",
            "py" => "🐍 ",
            "js" | "ts" => "📜 ",
            "toml" | "json" | "yaml" | "yml" => "⚙  ",
            _ => "📄 ",
        }
    }

    pub fn ascii_icon(path: &Path) -> &'static str {
        if path.is_dir() {
            return "[D] ";
        }

        match path.extension().and_then(|s| s.to_str()).unwrap_or("") {
            "png" | "jpg" | "jpeg" | "webp" | "gif" => "[I] ",
            "mp3" | "wav" | "flac" => "[A] ",
            "mp4" | "mkv" | "mov" => "[V] ",
            "zip" | "tar" | "gz" | "rar" => "[Z] ",
            "rs" | "c" | "cpp" | "h" | "py" | "js" | "ts" => "[S] ",
            "toml" | "json" | "yaml" | "yml" => "[C] ",
            _ => "[F] ",
        }
    }

    pub fn nerd_icon(path: &Path) -> &'static str {
        if path.is_dir() {
            return "󰉋 "; // nf-md-folder
        }

        match path.extension().and_then(|s| s.to_str()).unwrap_or("") {
            "png" | "jpg" | "jpeg" | "webp" | "gif" => "󰋩 ", // nf-md-image
            "mp3" | "wav" | "flac" => "󰎈 ", // nf-md-music
            "mp4" | "mkv" | "mov" => "󰕧 ", // nf-md-video
            "zip" | "tar" | "gz" | "rar" => "󰀼 ", // nf-md-archive
            "rs" => " ", // nf-dev-rust
            "c" | "cpp" | "h" => " ", // nf-dev-c
            "py" => " ", // nf-dev-python
            "js" => " ", // nf-dev-javascript
            "ts" => " ", // nf-dev-typescript
            "toml" | "json" | "yaml" | "yml" => " ", // nf-seti-config
            _ => "󰈔 ", // nf-md-file
        }
    }

}

impl fmt::Display for SortMode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SortMode::Name => write!(f, "Name"),
            SortMode::Size => write!(f, "Size"),
            SortMode::Modified => write!(f, "Modified"),
        }
    }
}
fn detect_icon_mode() -> IconMode {
    if let Ok(mode) = std::env::var("FREN_ICON_MODE") {
        return match mode.to_lowercase().as_str() {
            "ascii" => IconMode::Ascii,
            "nerd" => IconMode::Nerd,
            "emoji" => IconMode::Emoji,
            _ => IconMode::Emoji,
        };
    }

    let term = std::env::var("TERM").unwrap_or_default().to_lowercase();
    let term_program = std::env::var("TERM_PROGRAM").unwrap_or_default().to_lowercase();

    // Dumb terminals → ASCII
    if term == "dumb" || term == "linux" {
        return IconMode::Ascii;
    }

    // Kitty rule (force ASCII)
    if term.contains("kitty") || term_program.contains("kitty") {
        return IconMode::Ascii;
    }

    // Default modern → Emoji
    IconMode::Emoji
}

pub fn quantize(v: u16) -> u16 {
    (v / 4) * 4
}
pub fn get_dimensions(path: &std::path::Path) -> Option<(u32, u32)> {
    let reader = ImageReader::open(path).ok()?;
    reader.into_dimensions().ok()
}

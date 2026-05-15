use std::collections::HashMap;
use std::{
    env, fmt, fs, fs::File, io, io::BufRead, io::BufReader, io::Write, path::Path, path::PathBuf,
    process::Command, thread,
};

use image::GenericImageView;
use image::ImageReader;
use lru::LruCache;
use ratatui::layout::Rect;
use ratatui_image::picker::Picker;
use ratatui_image::protocol::Protocol;
use std::num::NonZeroUsize;
use std::sync::mpsc::{self, Sender};
use std::sync::{
    atomic::{AtomicU64, Ordering},
    Arc, Mutex,
};

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
#[derive(Clone, PartialEq)]
pub enum ClipboardMode {
    Copy,
    Cut,
}

#[derive(Clone)]
pub enum ConflictAction {
    Skip,
    Replace,
    RenameAuto,
    Cancel,
}

#[derive(Clone)]
pub struct ConflictState {
    pub pending: Vec<(PathBuf, PathBuf, ClipboardMode)>,
    pub index: usize,
}

#[derive(Clone)]
pub enum AppMode {
    Normal,
    Input(InputAction),
    Conflict(ConflictState),
}

#[derive(Clone, PartialEq)]
pub enum InputAction {
    Rename,
    CreateFile,
    CreateFolder,
    ConfirmDelete,
    OpenWith,
    GoTo,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Focus {
    Files,
    Pinned,
    Storage,
    Clipboard,
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
    pub clipboard: Vec<(PathBuf, ClipboardMode)>,
    pub clipboard_selected: usize,
    pub show_hidden: bool,
    pub mode: AppMode,
    pub input: String,
    pub input_cursor: usize,
    pub focus: Focus,
    pub pinned: Vec<PathBuf>,
    pub pinned_selected: usize,
    pub storage: Vec<PathBuf>,
    pub storage_selected: usize,
    pub show_help: bool,
    pub _preview_rect: Rect,
    pub _image_loaded: bool,
    pub _image_id: u32,
    pub _current_image: Option<std::path::PathBuf>,
    pub image: Option<Protocol>,
    pub image_path: Option<std::path::PathBuf>,
    pub _picker: Picker,
    pub image_rx: Option<mpsc::Receiver<(u64, Option<Protocol>)>>,
    pub _image_tx: mpsc::Sender<(u64, Option<Protocol>)>,
    pub image_loading: bool,
    pub image_cache: Arc<Mutex<LruCache<ImageKey, Protocol>>>,
    pub preview_deadline: Option<std::time::Instant>,
    pub image_size: Option<(u16, u16)>,
    pub _image_jobs: usize,
    pub image_request_id: u64,
    pub image_request_atomic: Arc<AtomicU64>,
    pub icon_mode: IconMode,
    pub cursor_memory: HashMap<PathBuf, usize>,
    pub preview_job_tx: Sender<PreviewJob>,
    pub selected_indices: std::collections::HashSet<usize>,
}

impl App {
    pub fn new(remember: bool) -> io::Result<Self> {
        let current_dir = if remember {
            Self::load_session().unwrap_or_else(|| std::env::current_dir().unwrap_or_default())
        } else {
            std::env::current_dir()?
        };
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
            clipboard: Vec::new(),
            clipboard_selected: 0,
            mode: AppMode::Normal,
            input: String::new(),
            input_cursor: 0,
            show_hidden,
            focus: Focus::Files,
            pinned: dirs::home_dir().into_iter().collect(),
            pinned_selected: 0,
            storage: Self::detect_mounts(),
            storage_selected: 0,
            show_help: false,
            _preview_rect: Rect::default(),
            _image_loaded: false,
            _image_id: 0,
            _current_image: None,
            _picker: picker,
            image: None,
            image_path: None,
            _image_tx: image_tx,
            image_rx: Some(image_rx),
            image_loading: false,
            image_cache: cache_clone,
            preview_deadline: None,
            image_size: None,
            _image_jobs: 0,
            image_request_id: 0,
            image_request_atomic: cancel_token,
            icon_mode: detect_icon_mode(),
            cursor_memory: HashMap::new(),
            preview_job_tx: job_tx,
            selected_indices: std::collections::HashSet::new(),
        })
    }

    fn detect_mounts() -> Vec<PathBuf> {
        let mut mounts = Vec::new();

        // Add home directory first
        if let Some(home) = dirs::home_dir() {
            mounts.push(home);
        }

        // Check common mount locations
        let check_dirs = ["/mnt", "/media", "/run/media"];
        for dir in &check_dirs {
            let p = std::path::PathBuf::from(dir);
            if let Ok(entries) = fs::read_dir(&p) {
                for entry in entries.filter_map(Result::ok) {
                    let path = entry.path();
                    if path.is_dir() {
                        mounts.push(path);
                    }
                }
            }
        }

        // Check /run/media/username for user-mounted media
        if let Some(username) = std::env::var_os("USER").or_else(|| std::env::var_os("USERNAME")) {
            let user_media = std::path::PathBuf::from("/run/media").join(username);
            if let Ok(entries) = fs::read_dir(&user_media) {
                for entry in entries.filter_map(Result::ok) {
                    let path = entry.path();
                    if path.is_dir() {
                        mounts.push(path);
                    }
                }
            }
        }

        // Remove duplicates while preserving order
        let mut seen = std::collections::HashSet::new();
        mounts.retain(|p| seen.insert(p.clone()));

        mounts
    }

    pub fn open_storage(&mut self) -> io::Result<()> {
        if let Some(path) = self.storage.get(self.storage_selected) {
            self.current_dir = path.clone();
            self.refresh()?;
        }
        Ok(())
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

    fn load_session() -> Option<PathBuf> {
        let path = dirs::config_dir()?.join("fren").join("session.txt");
        if path.exists() {
            let content = std::fs::read_to_string(path).ok()?;
            let p = PathBuf::from(content.trim());
            if p.exists() {
                return Some(p);
            }
        }
        None
    }

    pub fn save_session(&self) -> io::Result<()> {
        let path = dirs::config_dir()
            .unwrap_or(std::path::PathBuf::from("."))
            .join("fren")
            .join("session.txt");
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        std::fs::write(path, self.current_dir.display().to_string())?;
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
        self.selected_indices.clear();
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

    pub fn toggle_selection(&mut self) {
        if self.selected_indices.contains(&self.selected) {
            self.selected_indices.remove(&self.selected);
        } else {
            self.selected_indices.insert(self.selected);
        }
    }

    fn collect_selected(&self) -> Vec<usize> {
        if self.selected_indices.is_empty() {
            vec![self.selected]
        } else {
            let mut v: Vec<usize> = self.selected_indices.iter().copied().collect();
            v.sort_unstable();
            v
        }
    }

    pub fn copy_selected(&mut self) {
        for idx in self.collect_selected() {
            if let Some(entry) = self.entries.get(idx) {
                if self.clipboard.len() >= 50 {
                    self.clipboard.remove(0);
                }
                self.clipboard.push((entry.path(), ClipboardMode::Copy));
            }
        }
    }

    pub fn cut_selected(&mut self) {
        for idx in self.collect_selected() {
            if let Some(entry) = self.entries.get(idx) {
                if self.clipboard.len() >= 50 {
                    self.clipboard.remove(0);
                }
                self.clipboard.push((entry.path(), ClipboardMode::Cut));
            }
        }
    }

    pub fn paste(&mut self) -> io::Result<()> {
        let items: Vec<(PathBuf, ClipboardMode)> = self.clipboard.clone();
        let mut no_conflict = Vec::new();
        let mut conflicts = Vec::new();

        for (source, mode) in &items {
            let file_name = match source.file_name() {
                Some(name) => name,
                None => continue,
            };
            let dest = self.current_dir.join(file_name);
            if dest == *source {
                continue;
            }
            if dest.exists() {
                conflicts.push((source.clone(), dest, mode.clone()));
            } else {
                no_conflict.push((source.clone(), dest, mode.clone()));
            }
        }

        if conflicts.is_empty() {
            for (source, dest, mode) in &no_conflict {
                match mode {
                    ClipboardMode::Copy => Self::copy_recursively(source, dest)?,
                    ClipboardMode::Cut => {
                        fs::rename(source, dest)?;
                        self.clipboard.retain(|(p, _)| p != source);
                    }
                }
            }
            self.clipboard_selected = self
                .clipboard_selected
                .min(self.clipboard.len().saturating_sub(1));
            self.refresh()?;
            return Ok(());
        }

        for (source, dest, mode) in &no_conflict {
            match mode {
                ClipboardMode::Copy => Self::copy_recursively(source, dest)?,
                ClipboardMode::Cut => {
                    fs::rename(source, dest)?;
                    self.clipboard.retain(|(p, _)| p != source);
                }
            }
        }

        self.mode = AppMode::Conflict(ConflictState {
            pending: conflicts,
            index: 0,
        });
        Ok(())
    }

    pub fn paste_selected(&mut self) -> io::Result<()> {
        if self.clipboard_selected >= self.clipboard.len() {
            return Ok(());
        }

        let (source, mode) = self.clipboard[self.clipboard_selected].clone();
        let file_name = match source.file_name() {
            Some(name) => name,
            None => return Ok(()),
        };
        let dest = self.current_dir.join(file_name);

        if dest == source {
            return Ok(());
        }

        if dest.exists() {
            self.mode = AppMode::Conflict(ConflictState {
                pending: vec![(source, dest, mode)],
                index: 0,
            });
            return Ok(());
        }

        match mode {
            ClipboardMode::Copy => Self::copy_recursively(&source, &dest)?,
            ClipboardMode::Cut => {
                fs::rename(&source, &dest)?;
                self.clipboard.remove(self.clipboard_selected);
                self.clipboard_selected = self
                    .clipboard_selected
                    .min(self.clipboard.len().saturating_sub(1));
            }
        }
        self.refresh()?;
        Ok(())
    }

    pub fn apply_conflict_action(&mut self, action: ConflictAction) -> io::Result<()> {
        let state = match &mut self.mode {
            AppMode::Conflict(s) => s,
            _ => return Ok(()),
        };

        if state.index >= state.pending.len() {
            self.mode = AppMode::Normal;
            return Ok(());
        }

        let (source, dest, mode) = &state.pending[state.index];

        match action {
            ConflictAction::Skip => {}
            ConflictAction::Replace => {
                if dest.is_dir() {
                    fs::remove_dir_all(dest)?;
                } else if dest.exists() {
                    fs::remove_file(dest)?;
                }
                match mode {
                    ClipboardMode::Copy => Self::copy_recursively(source, dest)?,
                    ClipboardMode::Cut => {
                        fs::rename(source, dest)?;
                        self.clipboard.retain(|(p, _)| p != source);
                    }
                }
            }
            ConflictAction::RenameAuto => {
                let parent = dest.parent().unwrap();
                let stem = dest
                    .file_stem()
                    .and_then(|s| s.to_str())
                    .unwrap_or("file");
                let ext = dest
                    .extension()
                    .and_then(|s| s.to_str())
                    .map(|e| format!(".{}", e))
                    .unwrap_or_default();
                let mut counter = 1;
                let mut new_dest = parent.join(format!("{}_{}{}", stem, counter, ext));
                while new_dest.exists() {
                    counter += 1;
                    new_dest = parent.join(format!("{}_{}{}", stem, counter, ext));
                }
                match mode {
                    ClipboardMode::Copy => Self::copy_recursively(source, &new_dest)?,
                    ClipboardMode::Cut => {
                        fs::rename(source, &new_dest)?;
                        self.clipboard.retain(|(p, _)| p != source);
                    }
                }
            }
            ConflictAction::Cancel => {
                self.mode = AppMode::Normal;
                return Ok(());
            }
        }

        state.index += 1;

        if state.index >= state.pending.len() {
            self.mode = AppMode::Normal;
            self.clipboard_selected = self
                .clipboard_selected
                .min(self.clipboard.len().saturating_sub(1));
            self.refresh()?;
        }

        Ok(())
    }

    pub fn recopy_clipboard_item(&mut self) {
        if self.clipboard_selected < self.clipboard.len() {
            let entry = self.clipboard[self.clipboard_selected].clone();
            if self.clipboard.len() >= 50 {
                self.clipboard.remove(0);
            }
            self.clipboard.push(entry);
            self.clipboard_selected = self.clipboard.len().saturating_sub(1);
        }
    }

    pub fn remove_clipboard_item(&mut self) {
        if self.clipboard_selected < self.clipboard.len() {
            self.clipboard.remove(self.clipboard_selected);
            self.clipboard_selected = self
                .clipboard_selected
                .min(self.clipboard.len().saturating_sub(1));
        }
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
            PathBuf::from(home).join(".local/share/Trash/files")
        } else {
            PathBuf::from(".trash")
        }
    }

    pub fn trash_selected(&mut self) -> io::Result<()> {
        for idx in self.collect_selected() {
            if let Some(entry) = self.entries.get(idx) {
                let source = entry.path();
                let trash_dir = Self::trash_path();

                fs::create_dir_all(&trash_dir)?;

                let file_name = source.file_name().unwrap();
                let mut target = trash_dir.join(file_name);

                let mut counter = 1;
                while target.exists() {
                    let new_name = format!("{}_{}", file_name.to_string_lossy(), counter);
                    target = trash_dir.join(new_name);
                    counter += 1;
                }

                fs::rename(source, target)?;
            }
        }

        self.selected_indices.clear();
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
        self.input_cursor = self.input.len();
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
            "mp3" | "wav" | "flac" => "󰎈 ",                  // nf-md-music
            "mp4" | "mkv" | "mov" => "󰕧 ",                   // nf-md-video
            "zip" | "tar" | "gz" | "rar" => "󰀼 ",            // nf-md-archive
            "rs" => " ",                                    // nf-dev-rust
            "c" | "cpp" | "h" => " ",                       // nf-dev-c
            "py" => " ",                                    // nf-dev-python
            "js" => " ",                                    // nf-dev-javascript
            "ts" => " ",                                    // nf-dev-typescript
            "toml" | "json" | "yaml" | "yml" => " ",        // nf-seti-config
            _ => "󰈔 ",                                       // nf-md-file
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
    let term_program = std::env::var("TERM_PROGRAM")
        .unwrap_or_default()
        .to_lowercase();

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

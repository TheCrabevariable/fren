use std::{
    env, fmt, fs,
    fs::File,
    io,
    io::BufRead,
    io::BufReader,
    io::Write,
    path::Path,
    path::PathBuf,
    process::{Command, Stdio},
    thread,
};

use image::DynamicImage;
use image::GenericImageView;
use image::ImageReader;
use lru::LruCache;
use ratatui::layout::Rect;
use ratatui_image::picker::Picker;
use ratatui_image::protocol::Protocol;
use std::num::NonZeroUsize;
use std::sync::mpsc::{self, Sender};
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicU64, Ordering},
};

#[derive(Clone, Copy, Debug)]
pub enum SortMode {
    Name,
    Size,
    Modified,
}

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
    pub preview_failed: std::collections::HashSet<PathBuf>,
    pub entries: Vec<fs::DirEntry>,
    pub selected: usize,
    pub meta_selected: usize,
    pub meta_cache: Vec<(String, String)>,
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
    pub image: Option<Protocol>,
    pub image_path: Option<std::path::PathBuf>,
    pub image_rx: Option<mpsc::Receiver<(u64, Option<Protocol>)>>,
    pub image_loading: bool,
    pub image_cache: Arc<Mutex<LruCache<ImageKey, Protocol>>>,
    pub image_size: Option<(u16, u16)>,
    pub image_request_id: u64,
    pub image_request_atomic: Arc<AtomicU64>,
    pub icon_mode: IconMode,
    pub cursor_memory: LruCache<PathBuf, usize>,
    pub preview_job_tx: Sender<PreviewJob>,
    pub selected_indices: std::collections::HashSet<usize>,
    pub quick_app_selected: usize,
    job_rx: Option<mpsc::Receiver<PreviewJob>>,
    image_tx: Option<mpsc::Sender<(u64, Option<Protocol>)>>,
    cache_dir: PathBuf,
    pub preload_at: usize,
    pub picker: Option<Arc<Mutex<Picker>>>,
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

        let entries = Self::read_dir(&current_dir, SortMode::Name, show_hidden)?;
        let cache_size = NonZeroUsize::new(128).unwrap();
        let image_cache = Arc::new(Mutex::new(LruCache::new(cache_size)));

        let cache_dir = dirs::cache_dir()
            .unwrap_or_else(|| dirs::home_dir().unwrap_or_default().join(".cache"))
            .join("fren")
            .join("thumbnails");
        let _ = fs::create_dir_all(&cache_dir);

        let app = Self {
            current_dir,
            entries,
            selected: 0,
            preview_failed: std::collections::HashSet::new(),
            meta_selected: usize::MAX,
            meta_cache: Vec::new(),
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
            image: None,
            image_path: None,
            image_rx: Some(image_rx),
            image_loading: false,
            image_cache,
            image_size: None,
            image_request_id: 0,
            image_request_atomic: cancel_token,
            icon_mode: detect_icon_mode(),
            cursor_memory: LruCache::new(NonZeroUsize::new(200).unwrap()),
            preview_job_tx: job_tx,
            selected_indices: std::collections::HashSet::new(),
            quick_app_selected: 0,
            job_rx: Some(job_rx),
            image_tx: Some(image_tx),
            cache_dir,
            preload_at: usize::MAX,
            picker: None,
        };
        app.preload_images_sync(20);
        Ok(app)
    }

    pub fn init_picker(&mut self) {
        let picker = Arc::new(Mutex::new(Picker::from_query_stdio().unwrap()));
        let job_rx = self.job_rx.take().unwrap();
        let cancel = self.image_request_atomic.clone();
        let cache = self.image_cache.clone();
        let tx = self.image_tx.clone().unwrap();
        let cache_dir = self.cache_dir.clone();
        self.picker = Some(picker.clone());
        Self::spawn_preview_worker(job_rx, cancel, picker, cache, tx, cache_dir);
    }

    fn spawn_preview_worker(
        job_rx: mpsc::Receiver<PreviewJob>,
        cancel: Arc<AtomicU64>,
        picker: Arc<Mutex<Picker>>,
        cache: Arc<Mutex<LruCache<ImageKey, Protocol>>>,
        tx: mpsc::Sender<(u64, Option<Protocol>)>,
        cache_dir: PathBuf,
    ) {
        thread::spawn(move || {
            use image::ImageReader;

            while let Ok(mut job) = job_rx.recv() {
                while let Ok(newer) = job_rx.try_recv() {
                    job = newer;
                }

                let id = job.request_id;
                if cancel.load(Ordering::Relaxed) != id {
                    continue;
                }

                let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    let max_w = (job.inner.width as u32 * 8).clamp(1, 2048);
                    let max_h = (job.inner.height as u32 * 16).clamp(1, 2048);

                    // Check disk cache first (for regular images, not PDFs)
                    if !job.is_pdf {
                        let disk_path = cache_path(&job.path, &cache_dir);
                        if disk_path.exists()
                            && let Ok(img) = image::open(&disk_path)
                        {
                            let (w, h_) = img.dimensions();
                            let final_img = if w <= max_w && h_ <= max_h {
                                img
                            } else {
                                img.thumbnail(max_w, max_h)
                            };
                            if let Ok(protocol) = picker.lock().unwrap().new_protocol(
                                final_img,
                                job.inner,
                                ratatui_image::Resize::Fit(None),
                            ) {
                                return Some(protocol);
                            }
                        }
                    }

                    let (decoded, save_cache) = if job.is_pdf {
                        let tmp = format!("/tmp/fm_preview_{}", id);

                        let status = std::process::Command::new("pdftoppm")
                            .arg("-png")
                            .arg("-singlefile")
                            .arg("-r")
                            .arg("96")
                            .arg(&job.path)
                            .arg(&tmp)
                            .status()
                            .ok()?;
                        if !status.success() {
                            return None;
                        }

                        let png = format!("{}.png", tmp);
                        let img = image::open(&png).ok()?;
                        let _ = std::fs::remove_file(&png);
                        (img, false)
                    } else {
                        let decoded = ImageReader::open(&job.path).ok()?.decode().ok()?;
                        (decoded, true)
                    };

                    if cancel.load(Ordering::Relaxed) != id {
                        return None;
                    }

                    // Save to disk cache in background — don't delay protocol return
                    if save_cache {
                        let path = job.path.clone();
                        let cd = cache_dir.clone();
                        let (fw, fh) = decoded.dimensions();
                        let max_c = 400u32;
                        let img_for_cache = if fw <= max_c && fh <= max_c {
                            decoded.clone()
                        } else {
                            let (nw, nh) = if fw > fh {
                                (max_c, (fh * max_c / fw).max(1))
                            } else if fh > fw {
                                ((fw * max_c / fh).max(1), max_c)
                            } else {
                                (max_c, max_c)
                            };
                            decoded.resize_exact(nw, nh, image::imageops::FilterType::Nearest)
                        };
                        thread::spawn(move || {
                            let cache_dest = cache_path(&path, &cd);
                            if let Some(parent) = cache_dest.parent() {
                                let _ = fs::create_dir_all(parent);
                            }
                            if !cache_dest.exists() {
                                save_cache_raw(&img_for_cache, &cache_dest);
                            }
                        });
                    }

                    let (w, h) = decoded.dimensions();
                    let resized = if w <= max_w && h <= max_h {
                        decoded
                    } else {
                        decoded.thumbnail(max_w, max_h)
                    };

                    let protocol = picker
                        .lock()
                        .unwrap()
                        .new_protocol(resized, job.inner, ratatui_image::Resize::Fit(None))
                        .ok()?;

                    Some(protocol)
                }))
                .ok()
                .flatten();

                if let Some(ref protocol) = result {
                    cache.lock().unwrap().put(
                        ImageKey {
                            path: job.path.clone(),
                            width: quantize(job.inner.width),
                            height: quantize(job.inner.height),
                        },
                        protocol.clone(),
                    );
                }

                let _ = tx.send((id, result));
            }
        });
    }

    fn detect_mounts() -> Vec<PathBuf> {
        let mut mounts = Vec::new();
        if let Some(home) = dirs::home_dir() {
            mounts.push(home);
        }

        let check_dirs = ["/mnt", "/media", "/run/media"];
        for dir in &check_dirs {
            let p = std::path::PathBuf::from(dir);
            if p.exists()
                && let Ok(entries) = fs::read_dir(&p)
            {
                for entry in entries.filter_map(Result::ok) {
                    let path = entry.path();
                    if path.is_dir() {
                        mounts.push(path);
                    }
                }
            }
        }

        if let Some(username) = std::env::var_os("USER") {
            let user_media = std::path::PathBuf::from("/run/media").join(username);
            if user_media.exists()
                && let Ok(entries) = fs::read_dir(&user_media)
            {
                for entry in entries.filter_map(Result::ok) {
                    let path = entry.path();
                    if path.is_dir() {
                        mounts.push(path);
                    }
                }
            }
        }

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
                if let Some(name) = e.file_name().to_str()
                    && !show_hidden
                    && name.starts_with('.')
                {
                    return false;
                }
                true
            })
            .collect();

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

            match mode {
                SortMode::Name => {
                    let a_file = a.file_name();
                    let b_file = b.file_name();
                    let a_name = a_file.to_string_lossy();
                    let b_name = b_file.to_string_lossy();
                    natord::compare_ignore_case(&a_name, &b_name)
                }
                SortMode::Size => {
                    let a_size = a.metadata().map(|m| m.len()).unwrap_or(0);
                    let b_size = b.metadata().map(|m| m.len()).unwrap_or(0);
                    a_size.cmp(&b_size)
                }
                SortMode::Modified => {
                    let a_time = a.metadata().and_then(|m| m.modified()).ok();
                    let b_time = b.metadata().and_then(|m| m.modified()).ok();
                    a_time.cmp(&b_time)
                }
            }
        });

        Ok(entries)
    }

    pub fn refresh(&mut self) -> io::Result<()> {
        self.selected_indices.clear();
        self.preview_failed.clear();
        self.entries = Self::read_dir(&self.current_dir, self.sort_mode, self.show_hidden)?;

        // restore cursor if we have memory
        if let Some(&pos) = self.cursor_memory.get(&self.current_dir) {
            self.selected = pos.min(self.entries.len().saturating_sub(1));
        } else {
            self.selected = 0;
        }

        self.preload_at = self.selected;
        self.preload_images_sync(20);

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
                let stem = dest.file_stem().and_then(|s| s.to_str()).unwrap_or("file");
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
        if let Ok(data_home) = env::var("XDG_DATA_HOME") {
            PathBuf::from(data_home).join("Trash/files")
        } else if let Ok(home) = env::var("HOME") {
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

                let file_name = match source.file_name() {
                    Some(name) => name,
                    None => continue,
                };
                let mut target = trash_dir.join(file_name);

                let mut counter = 1;
                while target.exists() {
                    let new_name = format!("{}_{}", file_name.to_string_lossy(), counter);
                    target = trash_dir.join(new_name);
                    counter += 1;
                }

                if let Err(e) = fs::rename(&source, &target) {
                    if e.kind() == std::io::ErrorKind::CrossesDevices {
                        if source.is_dir() {
                            Self::copy_recursively(&source, &target)?;
                            fs::remove_dir_all(&source)?;
                        } else {
                            fs::copy(&source, &target)?;
                            fs::remove_file(&source)?;
                        }
                    } else {
                        return Err(e);
                    }
                }
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
            Command::new("sh")
                .arg("-c")
                .arg(format!("{} \"$1\"", program))
                .arg("--")
                .arg(entry.path())
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()?;
        }
        Ok(())
    }

    fn sanitize_name(name: &str) -> Option<String> {
        let name = name.trim();
        if name.is_empty() || name.contains('/') || name == ".." || name == "." {
            return None;
        }
        Some(name.to_string())
    }

    pub fn create_folder(&mut self, name: &str) -> io::Result<()> {
        let name = Self::sanitize_name(name).unwrap_or_default();
        if name.is_empty() {
            return Ok(());
        }
        let new_path = self.current_dir.join(name);
        if !new_path.exists() {
            fs::create_dir(&new_path)?;
        }
        self.refresh()
    }

    pub fn create_file(&mut self, name: &str) -> io::Result<()> {
        let name = Self::sanitize_name(name).unwrap_or_default();
        if name.is_empty() {
            return Ok(());
        }
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
    let _term_program = std::env::var("TERM_PROGRAM")
        .unwrap_or_default()
        .to_lowercase();

    // Dumb terminals → ASCII
    if term == "dumb" || term == "linux" {
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

fn fnv1a(bytes: &[u8]) -> u64 {
    let mut hash: u64 = 14695981039346656037;
    for &b in bytes {
        hash ^= b as u64;
        hash = hash.wrapping_mul(1099511628211);
    }
    hash
}

fn cache_path(path: &Path, cache_dir: &Path) -> PathBuf {
    let mut h = fnv1a(path.to_string_lossy().as_bytes());
    if let Ok(meta) = path.metadata()
        && let Ok(mtime) = meta.modified()
    {
        let dur = mtime
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default();
        let mtime_hash = fnv1a(&dur.as_nanos().to_le_bytes());
        h ^= mtime_hash.wrapping_mul(0x517cc1b727220a95);
    }
    cache_dir.join(format!("{:016x}.cache", h))
}

fn save_cache_raw(img: &DynamicImage, path: &Path) {
    let rgba = img.to_rgba8();
    let (w, h) = rgba.dimensions();
    let pixel_data = rgba.into_raw();
    let mut data = Vec::with_capacity(8 + pixel_data.len());
    data.extend_from_slice(&w.to_le_bytes());
    data.extend_from_slice(&h.to_le_bytes());
    data.extend_from_slice(&pixel_data);
    let _ = std::fs::write(path, &data);
}

fn load_cache_raw(path: &Path) -> Option<DynamicImage> {
    let data = std::fs::read(path).ok()?;
    if data.len() < 8 {
        return None;
    }
    let w = u32::from_le_bytes(data[0..4].try_into().ok()?);
    let h = u32::from_le_bytes(data[4..8].try_into().ok()?);
    let pixel_data = data[8..].to_vec();
    Some(DynamicImage::ImageRgba8(image::ImageBuffer::from_raw(
        w, h, pixel_data,
    )?))
}

impl App {
    pub fn try_protocol_from_cache(&self, path: &Path, inner: Rect) -> Option<Protocol> {
        let disk_path = cache_path(path, &self.cache_dir);
        if let Some(img) = load_cache_raw(&disk_path) {
            let max_w = (inner.width as u32 * 8).clamp(1, 2048);
            let max_h = (inner.height as u32 * 16).clamp(1, 2048);
            let (w, h) = img.dimensions();
            let resized = if w <= max_w && h <= max_h {
                img
            } else {
                img.thumbnail(max_w, max_h)
            };
            if let Some(ref picker) = self.picker {
                return picker
                    .lock()
                    .unwrap()
                    .new_protocol(resized, inner, ratatui_image::Resize::Fit(None))
                    .ok();
            }
        }
        None
    }

    pub fn preload_images(&self, count: usize) {
        self.preload_images_inner(count, false)
    }

    pub fn preload_images_sync(&self, count: usize) {
        self.preload_images_inner(count, true)
    }

    fn preload_images_inner(&self, count: usize, sync_first: bool) {
        let entries: Vec<_> = self
            .entries
            .iter()
            .skip(self.selected)
            .take(count)
            .map(|e| e.path())
            .collect();

        let cache_dir = self.cache_dir.clone();

        // Cache the first (current) file synchronously so the next frame can use it
        if sync_first && let Some(first) = entries.first() {
            cache_one(first, &cache_dir);
        }

        // Cache the rest with a bounded thread pool — one thread per CPU core
        let start = if sync_first { 1 } else { 0 };
        let to_cache: Vec<PathBuf> = entries.iter().skip(start).cloned().collect();
        if to_cache.is_empty() {
            return;
        }
        let parallelism = std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(4)
            .max(2);
        let queue = Arc::new(Mutex::new(to_cache));
        for _ in 0..parallelism.min(queue.lock().unwrap().len()) {
            let q = Arc::clone(&queue);
            let cd = cache_dir.clone();
            thread::spawn(move || {
                loop {
                    let path = q.lock().unwrap().pop();
                    match path {
                        Some(p) => cache_one(&p, &cd),
                        None => break,
                    }
                }
            });
        }
    }
}

fn cache_one(path: &Path, cache_dir: &Path) {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    if !matches!(ext.as_str(), "png" | "jpg" | "jpeg" | "webp" | "gif") {
        return;
    }

    let disk_path = cache_path(path, cache_dir);
    if disk_path.exists() {
        return;
    }

    if let Ok(img) = image::open(path) {
        let (w, h) = img.dimensions();
        let max_cache = 400u32;
        let thumb = if w <= max_cache && h <= max_cache {
            img
        } else {
            let (nw, nh) = if w > h {
                (max_cache, (h * max_cache / w).max(1))
            } else if h > w {
                ((w * max_cache / h).max(1), max_cache)
            } else {
                (max_cache, max_cache)
            };
            img.resize_exact(nw, nh, image::imageops::FilterType::Nearest)
        };
        let _ = std::fs::create_dir_all(disk_path.parent().unwrap());
        save_cache_raw(&thumb, &disk_path);
    }
}

//! Application state + input handling. Rendering lives in `ui`.

use std::cell::Cell;
use std::thread;
use std::time::{Duration, Instant};

use crossterm::event::{Event, KeyCode, KeyEvent, KeyModifiers};
use ratatui::style::{Modifier, Style};
use tui_textarea::{CursorMove, TextArea};

use crate::input::QueryInput;
use crate::model::{
    DateFormat, Freshness, HitRate, InferenceRecord, Settings, ShardingMode,
    PROVIDERS,
};
use crate::ui::viewer::ViewerState;
use crate::{db, theme};

pub const MAX_QUERY_CHARS: usize = crate::input::MAX_CHARS;

/// Hourly budget for stage-2 contextual safety classifications.
/// Bounded so a hostile provider state can never turn the classifier into an
/// unbounded credit/cost drain; exhausted budget → fail-closed (local-only).
const MAX_CLASSIFICATIONS_PER_HOUR: usize = 30;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Screen {
    Search,
    Memory,
    Settings,
}

/// Focusable Settings fields. `ApiEndpoint` is intentionally absent: it is
/// read-only (auto-filled from the selected provider) and never focused.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SettingsField {
    // Storage & Sharding
    DateFormat,
    Storage,
    ShardingMode,
    HitRate,
    Freshness,
    // Cloud AI Provider
    Provider,
    ProviderModel,
    ApiKey,
    // Maintenance & Legal
    BackupDatabase,
    RestoreDatabase,
    #[cfg_attr(not(feature = "publisher"), allow(dead_code))]
    SyncWebArchive,
    ClearCache,
    TermsOfService,
}

#[derive(Clone, Debug)]
pub struct ConfirmModal {
    pub title: String,
    pub message: String,
    pub pending_sharding_mode: Option<ShardingMode>,
}

#[derive(Clone, Debug)]
pub struct PickerModal {
    pub title: String,
    pub items: Vec<String>,
    pub selected: usize,
    pub offset: Cell<usize>,
    pub field: SettingsField,
}

impl PickerModal {
    pub fn new(title: String, items: Vec<String>, selected: usize, field: SettingsField) -> Self {
        Self {
            title,
            items,
            selected,
            offset: Cell::new(0),
            field,
        }
    }

    /// Scroll the picker list only when the selection leaves the visible
    /// window; moving within the window just moves the highlight cursor.
    pub fn scroll_into_view(&self, height: usize) -> usize {
        let h = height.max(1);
        let cur_off = self.offset.get();
        let new_off = if self.selected < cur_off {
            self.selected
        } else if self.selected >= cur_off + h {
            self.selected - h + 1
        } else {
            cur_off
        };
        self.offset.set(new_off);
        new_off
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FileBrowserMode {
    SelectDirectory,
    SelectFile,
}

#[derive(Clone, Debug)]
pub struct FileBrowserEntry {
    pub name: String,
    pub is_dir: bool,
    pub is_action: bool,
    pub size_bytes: u64,
    pub path: std::path::PathBuf,
}

#[derive(Clone, Debug)]
pub struct FileBrowserModal {
    pub title: String,
    pub mode: FileBrowserMode,
    pub current_dir: std::path::PathBuf,
    pub entries: Vec<FileBrowserEntry>,
    pub selected: usize,
    pub offset: Cell<usize>,
}

impl FileBrowserModal {
    pub fn new(title: String, mode: FileBrowserMode) -> Self {
        let start_dir = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
        let mut modal = Self {
            title,
            mode,
            current_dir: start_dir,
            entries: Vec::new(),
            selected: 0,
            offset: Cell::new(0),
        };
        modal.refresh_entries();
        modal
    }

    pub fn refresh_entries(&mut self) {
        self.entries.clear();
        self.selected = 0;
        self.offset.set(0);

        // 1. Parent directory entry (unless at root)
        if let Some(parent) = self.current_dir.parent() {
            self.entries.push(FileBrowserEntry {
                name: ".. (parent directory)".to_string(),
                is_dir: true,
                is_action: false,
                size_bytes: 0,
                path: parent.to_path_buf(),
            });
        }

        // 2. Action item for SelectDirectory: Backup here
        if self.mode == FileBrowserMode::SelectDirectory {
            let dir_display = self
                .current_dir
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("/");
            self.entries.push(FileBrowserEntry {
                name: format!("💾 [ BACKUP TO THIS DIRECTORY: {} ]", dir_display),
                is_dir: false,
                is_action: true,
                size_bytes: 0,
                path: self.current_dir.clone(),
            });
        }

        // 3. Scan directory entries
        if let Ok(read) = std::fs::read_dir(&self.current_dir) {
            let mut dirs = Vec::new();
            let mut files = Vec::new();

            for item in read.flatten() {
                let path = item.path();
                let name = item.file_name().to_string_lossy().to_string();
                if name.starts_with('.') {
                    continue;
                }
                if let Ok(meta) = item.metadata() {
                    if meta.is_dir() {
                        dirs.push(FileBrowserEntry {
                            name,
                            is_dir: true,
                            is_action: false,
                            size_bytes: 0,
                            path,
                        });
                    } else if meta.is_file() {
                        files.push(FileBrowserEntry {
                            name,
                            is_dir: false,
                            is_action: false,
                            size_bytes: meta.len(),
                            path,
                        });
                    }
                }
            }

            dirs.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
            files.sort_by(|a, b| {
                // In SelectFile mode, put .db / .sqlite files first
                let a_is_db = a.name.ends_with(".db") || a.name.ends_with(".sqlite");
                let b_is_db = b.name.ends_with(".db") || b.name.ends_with(".sqlite");
                match (a_is_db, b_is_db) {
                    (true, false) => std::cmp::Ordering::Less,
                    (false, true) => std::cmp::Ordering::Greater,
                    _ => a.name.to_lowercase().cmp(&b.name.to_lowercase()),
                }
            });

            self.entries.extend(dirs);
            self.entries.extend(files);
        }
    }

    pub fn navigate_to(&mut self, path: std::path::PathBuf) {
        if let Ok(canonical) = path.canonicalize() {
            self.current_dir = canonical;
        } else {
            self.current_dir = path;
        }
        self.refresh_entries();
    }

    pub fn scroll_into_view(&self, height: usize) -> usize {
        let h = height.max(1);
        let cur_off = self.offset.get();
        let new_off = if self.selected < cur_off {
            self.selected
        } else if self.selected >= cur_off + h {
            self.selected - h + 1
        } else {
            cur_off
        };
        self.offset.set(new_off);
        new_off
    }
}

pub struct PendingSwarmQuery {
    pub request_id: String,
    pub question: String,
    pub simhash: u64,
    /// Deadline reference for the 2.5 s swarm wait — set when the query is
    /// actually broadcast (after the privacy jitter).
    pub started_at: std::time::Instant,
    /// Jittered moment when the query should leave the node. `None` means the
    /// query was already broadcast (or is test-injected as already sent).
    pub broadcast_at: Option<std::time::Instant>,
}

/// Transient header-bar state for the web-archive synchronization pipeline
/// (publisher builds only). The completion state auto-clears after
/// `SYNC_STATUS_TTL` so the normal flow is never disrupted.
#[cfg(feature = "publisher")]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SyncStatus {
    Running,
    Done {
        success: bool,
        shown_at: std::time::Instant,
    },
}

/// How long the "sync completed / failed" header indicator stays visible.
#[cfg(feature = "publisher")]
pub const SYNC_STATUS_TTL: std::time::Duration = std::time::Duration::from_secs(4);

/// An outbound inference awaiting its stage-2 safety classification verdict.
/// Fail-closed: no verdict (error/timeout/disconnect) → never published.
pub struct PendingSafetyCheck {
    pub msg: crate::p2p::SwarmInferenceMessage,
    pub rx: crossbeam_channel::Receiver<Result<bool, String>>,
}

pub struct ActiveStream {
    pub question: String,
    pub simhash: u64,
    pub provider: String,
    pub model: String,
    pub receiver: crossbeam_channel::Receiver<crate::api::stream::StreamMessage>,
}

pub struct App {
    pub screen: Screen,
    pub quit: bool,

    pub search_input: QueryInput,
    /// Last render width of the Ask input, used for soft-wrap + cursor motion.
    pub search_width: Cell<usize>,

    pub records: Vec<InferenceRecord>,
    pub total_records: usize,
    pub records_offset: usize,
    pub memory_selected: usize,
    /// First visible row of the memory list (manual scroll offset).
    pub memory_offset: usize,
    /// Number of visible memory rows (set during draw).
    pub memory_height: Cell<usize>,

    pub settings: Settings,
    pub focus: SettingsField,
    /// True while a text field (Storage / ApiKey) is being edited.
    pub editing: bool,
    pub edit_buffer: TextArea<'static>,

    /// Full-screen response / record viewer when active.
    pub viewer: Option<ViewerState>,
    /// Active confirmation modal (e.g. Clear Cache confirm).
    pub confirm_modal: Option<ConfirmModal>,
    /// Active scrollable selection modal for settings options.
    pub picker_modal: Option<PickerModal>,
    /// Active file / directory browser modal for backup & restore.
    pub file_browser_modal: Option<FileBrowserModal>,
    /// Active first-run or updated Terms of Service review gate.
    pub tos_gate_active: bool,
    /// Backward-compatibility alias for terms modal state.
    pub terms_modal: bool,

    /// Discovered models from active cloud provider (empty if unauthenticated/invalid key).
    pub provider_models: Vec<String>,

    /// P2P Swarm Network handle.
    pub p2p: Option<crate::p2p::P2pHandle>,
    /// Pending P2P Swarm query awaiting peer response.
    pub pending_query: Option<PendingSwarmQuery>,
    /// Real-time streaming AI inference task.
    pub active_stream: Option<ActiveStream>,
    /// Receiver for the running web-archive sync (publisher builds only).
    #[cfg(feature = "publisher")]
    pub pending_sync: Option<crossbeam_channel::Receiver<crate::cms::SyncOutcome>>,
    /// Transient web-archive sync indicator shown in the header bar.
    #[cfg(feature = "publisher")]
    pub sync_status: Option<SyncStatus>,
    /// Outbound inferences awaiting stage-2 safety classification (fail-closed).
    pub pending_safety_checks: Vec<PendingSafetyCheck>,
    /// Hourly budget bookkeeping for stage-2 classifications.
    pub safety_budget_used: usize,
    pub safety_budget_window: Instant,

    /// Last known width and height of the body viewport.
    pub body_width: Cell<usize>,
    pub body_height: Cell<usize>,
}

pub const WINDOW_SIZE: usize = 150;
pub const WINDOW_MARGIN: usize = 25;

impl App {
    pub fn new() -> Self {
        let total_records = db::count_records();
        let settings = Settings::load();

        // Enforce the user-configured fixed-GB storage ceiling at startup
        // before anything else touches the store.
        let locality_first = settings.sharding_mode == ShardingMode::QueryLocality;
        let _ = db::enforce_storage_limit_gb(settings.reserved_gb, locality_first);

        // Refresh Query Locality scores from the stored question profile so
        // the Memory list opens in personalized order.
        db::recompute_locality();

        let records = if locality_first {
            db::load_records_window(0, WINDOW_SIZE)
        } else {
            db::load_records_window_recent(0, WINDOW_SIZE)
        };

        let current_tos_ver = crate::tos::CURRENT_TOS_VERSION;
        let accepted_tos_ver = db::get_meta("terms_accepted_version");
        let tos_gate_active = accepted_tos_ver.as_deref() != Some(current_tos_ver);

        let p2p = if !tos_gate_active {
            Some(crate::p2p::start_p2p_service())
        } else {
            None
        };

        let viewer = if tos_gate_active {
            Some(ViewerState::with_tos_metadata(
                crate::tos::TERMS_OF_SERVICE_TEXT,
                current_tos_ver,
            ))
        } else {
            None
        };

        let mut app = Self {
            screen: Screen::Search,
            quit: false,
            search_input: QueryInput::new(),
            search_width: Cell::new(0),
            records,
            total_records,
            records_offset: 0,
            memory_selected: 0,
            memory_offset: 0,
            memory_height: Cell::new(0),
            settings,
            focus: SettingsField::DateFormat,
            editing: false,
            edit_buffer: single_line(""),
            viewer,
            confirm_modal: None,
            picker_modal: None,
            file_browser_modal: None,
            tos_gate_active,
            terms_modal: tos_gate_active,
            provider_models: Vec::new(),
            p2p,
            pending_query: None,
            active_stream: None,
            #[cfg(feature = "publisher")]
            pending_sync: None,
            #[cfg(feature = "publisher")]
            sync_status: None,
            pending_safety_checks: Vec::new(),
            safety_budget_used: 0,
            safety_budget_window: Instant::now(),
            body_width: Cell::new(0),
            body_height: Cell::new(0),
        };
        app.refresh_provider_models();
        app
    }

    pub fn for_screen(screen: Screen) -> Self {
        let mut app = Self::new();
        app.screen = screen;
        app.tos_gate_active = false;
        app.terms_modal = false;
        app.viewer = None;
        if app.p2p.is_none() {
            app.p2p = Some(crate::p2p::start_p2p_service());
        }
        app.ensure_window();
        app
    }

    /// Resets the sliding window cache and reloads total records from SQLite.
    pub fn reload_records(&mut self) {
        self.total_records = db::count_records();
        self.records_offset = 0;
        self.records = if self.settings.sharding_mode == ShardingMode::BlindSwarm {
            db::load_records_window_recent(0, WINDOW_SIZE)
        } else {
            db::load_records_window(0, WINDOW_SIZE)
        };
        self.memory_selected = 0;
        self.memory_offset = 0;
    }

    /// Inserts a new live record into the database cache window if at top.
    pub fn insert_record(&mut self, record: InferenceRecord) {
        self.total_records += 1;
        if self.records_offset == 0 {
            self.records.insert(0, record);
            if self.records.len() > WINDOW_SIZE {
                self.records.pop();
            }
        }
    }

    /// Checks if the currently visible memory viewport is approaching the
    /// boundaries of the in-memory window slice, and slides the window if needed.
    pub fn ensure_window(&mut self) {
        let viewport_h = self.memory_height.get().max(1);
        let view_start = self.memory_offset;
        let view_end = (self.memory_offset + viewport_h).min(self.total_records);

        let win_start = self.records_offset;
        let win_end = self.records_offset + self.records.len();

        let needs_reload = if self.records.is_empty() && self.total_records > 0 {
            true
        } else if view_start < win_start || view_end > win_end {
            true
        } else if view_start < win_start + WINDOW_MARGIN && win_start > 0 {
            true
        } else if view_end + WINDOW_MARGIN > win_end && win_end < self.total_records {
            true
        } else {
            false
        };

        if needs_reload {
            let target_start = self.memory_offset.saturating_sub(50);
            self.records = if self.settings.sharding_mode == ShardingMode::BlindSwarm {
                db::load_records_window_recent(target_start, WINDOW_SIZE)
            } else {
                db::load_records_window(target_start, WINDOW_SIZE)
            };
            self.records_offset = target_start;
        }
    }

    /// Returns the record at `index` from the loaded sliding window if present,
    /// or fetches it on demand from SQLite via lazy row lookup.
    pub fn get_memory_record(&self, index: usize) -> Option<InferenceRecord> {
        let win_start = self.records_offset;
        let win_end = self.records_offset + self.records.len();
        if index >= win_start && index < win_end {
            Some(self.records[index - win_start].clone())
        } else if self.settings.sharding_mode == ShardingMode::BlindSwarm {
            db::get_record_at_recent(index)
        } else {
            db::get_record_at(index)
        }
    }

    pub fn query_char_count(&self) -> usize {
        self.search_input.char_count()
    }

    /// Dynamically returns all active visible settings fields.
    pub fn visible_fields(&self) -> Vec<SettingsField> {
        let mut fields = vec![
            SettingsField::DateFormat,
            SettingsField::Storage,
            SettingsField::ShardingMode,
            SettingsField::HitRate,
            SettingsField::Freshness,
            SettingsField::Provider,
        ];
        if !self.provider_models.is_empty() {
            fields.push(SettingsField::ProviderModel);
        }
        fields.push(SettingsField::ApiKey);
        fields.push(SettingsField::BackupDatabase);
        fields.push(SettingsField::RestoreDatabase);
        #[cfg(feature = "publisher")]
        if crate::cms::cms_dir().is_some() || std::env::var("MBHUB_PUBLISHER").is_ok() {
            fields.push(SettingsField::SyncWebArchive);
        }
        fields.push(SettingsField::ClearCache);
        fields.push(SettingsField::TermsOfService);
        fields
    }

    pub fn refresh_provider_models(&mut self) {
        let key = self.settings.api_key.trim();
        let provider = PROVIDERS[self.settings.provider_idx];

        if key.is_empty() && provider.name != "OpenRouter" {
            self.provider_models = Vec::new();
            return;
        }

        let key_opt = if key.is_empty() { None } else { Some(key) };
        if let Ok(models) = crate::api::client::fetch_models(provider.endpoint, key_opt) {
            if !models.is_empty() {
                self.provider_models = models;
                if !self.provider_models.iter().any(|m| m == &self.settings.provider_model) {
                    self.settings.provider_model = self.provider_models[0].clone();
                }
                return;
            }
        }

        self.provider_models = Vec::new();
    }

    pub fn handle_event(&mut self, ev: Event) {
        match ev {
            Event::Key(key) => self.handle_key(key),
            Event::Paste(text) => self.handle_paste(text),
            _ => {}
        }
    }

    fn handle_key(&mut self, key: KeyEvent) {
        // Global quit — Ctrl+C is the terminal-standard way out.
        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
            self.quit = true;
            return;
        }

        // Mandatory Terms of Service review gate has absolute priority
        if self.tos_gate_active || self.terms_modal {
            match key.code {
                KeyCode::Enter | KeyCode::Char('y') | KeyCode::Char('Y') => {
                    self.accept_terms();
                }
                KeyCode::Esc
                | KeyCode::Char('q')
                | KeyCode::Char('Q')
                | KeyCode::Char('n')
                | KeyCode::Char('N') => {
                    self.quit = true;
                }
                KeyCode::Up | KeyCode::Char('k') => {
                    if let Some(v) = &mut self.viewer {
                        v.scroll_up(1);
                    }
                }
                KeyCode::Down | KeyCode::Char('j') => {
                    if let Some(v) = &mut self.viewer {
                        let w = self.body_width.get().max(1);
                        let h = v.visible_height(self.body_height.get().max(1));
                        v.scroll_down(1, w, h);
                    }
                }
                KeyCode::PageUp => {
                    if let Some(v) = &mut self.viewer {
                        let h = v.visible_height(self.body_height.get().max(1));
                        v.scroll_up(h);
                    }
                }
                KeyCode::PageDown | KeyCode::Char(' ') => {
                    if let Some(v) = &mut self.viewer {
                        let w = self.body_width.get().max(1);
                        let h = v.visible_height(self.body_height.get().max(1));
                        v.scroll_down(h, w, h);
                    }
                }
                KeyCode::Home => {
                    if let Some(v) = &mut self.viewer {
                        v.scroll_offset = 0;
                    }
                }
                KeyCode::End => {
                    if let Some(v) = &mut self.viewer {
                        let w = self.body_width.get().max(1);
                        let h = v.visible_height(self.body_height.get().max(1));
                        v.scroll_offset = v.max_offset(w, h);
                    }
                }
                _ => {}
            }
            return;
        }

        // Confirmation modal has top priority
        if self.confirm_modal.is_some() {
            match key.code {
                KeyCode::Enter | KeyCode::Char('y') | KeyCode::Char('Y') => {
                    self.execute_modal_confirm();
                }
                KeyCode::Esc | KeyCode::Char('n') | KeyCode::Char('N') => {
                    self.confirm_modal = None;
                }
                _ => {}
            }
            return;
        }

        // Scrollable picker modal
        if let Some(picker) = &mut self.picker_modal {
            match key.code {
                KeyCode::Up | KeyCode::Char('k') => {
                    if picker.selected > 0 {
                        picker.selected -= 1;
                    }
                }
                KeyCode::Down | KeyCode::Char('j') => {
                    if picker.selected + 1 < picker.items.len() {
                        picker.selected += 1;
                    }
                }
                KeyCode::PageUp => {
                    picker.selected = picker.selected.saturating_sub(6);
                }
                KeyCode::PageDown => {
                    picker.selected =
                        (picker.selected + 6).min(picker.items.len().saturating_sub(1));
                }
                KeyCode::Home | KeyCode::Char('g') => {
                    picker.selected = 0;
                }
                KeyCode::End | KeyCode::Char('G') => {
                    picker.selected = picker.items.len().saturating_sub(1);
                }
                KeyCode::Enter => {
                    self.apply_picker_selection();
                }
                KeyCode::Esc => {
                    self.picker_modal = None;
                }
                _ => {}
            }
            return;
        }

        // File / Directory browser modal
        if let Some(browser) = &mut self.file_browser_modal {
            match key.code {
                KeyCode::Up | KeyCode::Char('k') => {
                    if browser.selected > 0 {
                        browser.selected -= 1;
                    }
                }
                KeyCode::Down | KeyCode::Char('j') => {
                    if browser.selected + 1 < browser.entries.len() {
                        browser.selected += 1;
                    }
                }
                KeyCode::PageUp => {
                    browser.selected = browser.selected.saturating_sub(6);
                }
                KeyCode::PageDown => {
                    browser.selected =
                        (browser.selected + 6).min(browser.entries.len().saturating_sub(1));
                }
                KeyCode::Home | KeyCode::Char('g') => {
                    browser.selected = 0;
                }
                KeyCode::End | KeyCode::Char('G') => {
                    browser.selected = browser.entries.len().saturating_sub(1);
                }
                KeyCode::Backspace => {
                    if let Some(parent) = browser.current_dir.parent().map(|p| p.to_path_buf()) {
                        browser.navigate_to(parent);
                    }
                }
                KeyCode::Enter => {
                    self.handle_file_browser_enter();
                }
                KeyCode::Esc => {
                    self.file_browser_modal = None;
                }
                _ => {}
            }
            return;
        }

        // When the full-screen markdown viewer is open:
        if self.viewer.is_some() {
            self.handle_viewer_key(key);
            return;
        }

        // Editing a text field in Settings: every key is value input.
        if self.editing {
            match key.code {
                KeyCode::Enter => self.commit_edit(),
                KeyCode::Esc => {
                    self.editing = false;
                }
                _ => {
                    self.edit_buffer.input(key);
                }
            }
            return;
        }

        // Global navigation: Tab cycles the three screens.
        if key.code == KeyCode::Tab {
            self.next_screen();
            return;
        }

        match self.screen {
            Screen::Search => self.search_key(key),
            Screen::Memory => self.memory_key(key),
            Screen::Settings => self.settings_key(key),
        }
    }

    fn apply_picker_selection(&mut self) {
        if let Some(picker) = self.picker_modal.take() {
            let idx = picker.selected;
            match picker.field {
                SettingsField::DateFormat => {
                    if idx < DateFormat::ALL.len() {
                        self.settings.date_format = DateFormat::ALL[idx];
                    }
                }
                SettingsField::ShardingMode => {
                    if idx < ShardingMode::ALL.len() {
                        let new_mode = ShardingMode::ALL[idx];
                        if new_mode != self.settings.sharding_mode {
                            self.confirm_modal = Some(ConfirmModal {
                                title: "Switch sharding mode?".to_string(),
                                message: "Switching sharding modes requires purging local cache to rebuild shards under the new policy. Proceed with purge?".to_string(),
                                pending_sharding_mode: Some(new_mode),
                            });
                        }
                    }
                }
                SettingsField::HitRate => {
                    if idx < HitRate::ALL.len() {
                        self.settings.hit_rate = HitRate::ALL[idx];
                    }
                }
                SettingsField::Freshness => {
                    if idx < Freshness::ALL.len() {
                        self.settings.freshness = Freshness::ALL[idx];
                    }
                }
                SettingsField::Provider => {
                    if idx < PROVIDERS.len() {
                        self.set_provider(idx);
                    }
                }
                SettingsField::ProviderModel => {
                    if idx < self.provider_models.len() {
                        let model = self.provider_models[idx].clone();
                        self.settings.provider_model = model.clone();
                        let provider_name = PROVIDERS[self.settings.provider_idx].name;
                        self.settings
                            .provider_selected_models
                            .insert(provider_name.to_string(), model.clone());
                        crate::env::set_model_for_provider(provider_name, &model);
                    }
                }
                _ => {}
            }
        }
    }

    /// Selects an AI provider and restores its dedicated API key and model choice.
    pub fn set_provider(&mut self, idx: usize) {
        if idx >= PROVIDERS.len() {
            return;
        }
        self.settings.provider_idx = idx;
        let provider_name = PROVIDERS[idx].name;

        // Restore previously stored API key for this provider (.env / DB)
        self.settings.api_key = self
            .settings
            .provider_keys
            .get(provider_name)
            .cloned()
            .unwrap_or_else(|| crate::env::get_api_key_for_provider(provider_name));

        // Restore previously selected model or default model for this provider
        self.settings.provider_model = self
            .settings
            .provider_selected_models
            .get(provider_name)
            .cloned()
            .or_else(|| crate::env::get_model_for_provider(provider_name))
            .unwrap_or_else(|| crate::api::client::default_model_for_provider(provider_name));

        db::set_meta("active_provider_idx", &idx.to_string());
        crate::env::save_env_var("ACTIVE_PROVIDER", provider_name);
        self.refresh_provider_models();
    }

    fn execute_modal_confirm(&mut self) {
        if let Some(modal) = self.confirm_modal.take() {
            if let Some(new_mode) = modal.pending_sharding_mode {
                self.settings.sharding_mode = new_mode;
                // Switching modes wipes the whole local shard (as before) —
                // and the query profile too: Blind Swarm promises zero query
                // tracking, so no profile may leak across the switch.
                db::clear_profile();
            }
            let _ = db::clear_all();
            self.total_records = 0;
            self.records.clear();
            self.records_offset = 0;
            self.memory_selected = 0;
            self.memory_offset = 0;
        }
    }

    /// Marks the current version of the Terms of Service as accepted,
    /// closes the viewer gate, and initializes the P2P swarm.
    pub fn accept_terms(&mut self) {
        db::set_meta("terms_accepted_version", crate::tos::CURRENT_TOS_VERSION);
        db::set_meta("terms_accepted", "true");
        self.tos_gate_active = false;
        self.terms_modal = false;
        self.viewer = None;
        if self.p2p.is_none() {
            self.p2p = Some(crate::p2p::start_p2p_service());
        }
    }

    fn handle_viewer_key(&mut self, key: KeyEvent) {
        let w = self.body_width.get().max(1);
        let h = self
            .viewer
            .as_ref()
            .map(|v| v.visible_height(self.body_height.get().max(1)))
            .unwrap_or_else(|| self.body_height.get().max(1));

        match key.code {
            KeyCode::Esc => {
                self.viewer = None;
                self.pending_query = None;
                self.active_stream = None;
                // ASK flow: after an answer (or any search-originated
                // message) is dismissed, return to a CLEAN query input —
                // the answer is already preserved in MEMORY. Memory-side
                // viewers keep their selection untouched.
                if self.screen == Screen::Search {
                    self.search_input.clear();
                }
            }
            KeyCode::Tab => {
                self.viewer = None;
                self.pending_query = None;
                self.active_stream = None;
                self.next_screen();
            }
            KeyCode::Up | KeyCode::Char('k') => {
                if let Some(v) = &mut self.viewer {
                    v.scroll_up(1);
                }
            }
            KeyCode::Down | KeyCode::Char('j') => {
                if let Some(v) = &mut self.viewer {
                    v.scroll_down(1, w, h);
                }
            }
            KeyCode::PageUp => {
                if let Some(v) = &mut self.viewer {
                    v.scroll_page_up(h);
                }
            }
            KeyCode::PageDown | KeyCode::Char(' ') => {
                if let Some(v) = &mut self.viewer {
                    v.scroll_page_down(w, h);
                }
            }
            KeyCode::Home | KeyCode::Char('g') => {
                if let Some(v) = &mut self.viewer {
                    v.scroll_to_top();
                }
            }
            KeyCode::Delete | KeyCode::Char('d') => {
                let rec = self.viewer.as_ref().and_then(|v| v.record.clone());
                if let Some(record) = rec {
                    let (hash, simhash) = crate::db::delete_and_tombstone_record(&record, "User deleted answer via viewer");
                    if let Some(p) = &self.p2p {
                        p.broadcast_tombstone(crate::p2p::SwarmTombstoneMessage {
                            content_hash: hash,
                            simhash,
                            timestamp: chrono::Local::now().timestamp(),
                            reporter_peer_id: p.peer_id(),
                            reason: "User deleted answer".to_string(),
                            signature: Vec::new(),
                        });
                    }
                    self.viewer = None;
                    self.pending_query = None;
                    self.active_stream = None;
                    self.reload_records();
                }
            }
            KeyCode::End | KeyCode::Char('G') => {
                if let Some(v) = &mut self.viewer {
                    v.scroll_to_bottom(w, h);
                }
            }
            _ => {}
        }
    }

    /// Periodic tick called by the event loop to process streaming tokens,
    /// pending P2P swarm queries, and incoming gossip messages without blocking.
    pub fn tick(&mut self) {
        // 0. Web-archive sync bookkeeping (publisher builds): when the
        // background pipeline finishes, switch the header indicator to the
        // completion state and auto-clear it after the TTL. The normal flow
        // (Memory/Ask/Settings) is never taken over.
        #[cfg(feature = "publisher")]
        {
            let done = self
                .pending_sync
                .as_ref()
                .and_then(|rx| rx.try_recv().ok());
            if let Some(outcome) = done {
                self.pending_sync = None;
                self.sync_status = Some(SyncStatus::Done {
                    success: outcome.success,
                    shown_at: std::time::Instant::now(),
                });
            }
            if let Some(SyncStatus::Done { shown_at, .. }) = self.sync_status {
                if shown_at.elapsed() >= SYNC_STATUS_TTL {
                    self.sync_status = None;
                }
            }
        }

        // 1. Drain pending stage-2 safety classifications (fail-closed).
        self.drain_safety_checks();

        // 1. Process pending P2P swarm query
        if let Some(mut pending) = self.pending_query.take() {
            // Privacy jitter: hold the query briefly before
            // it leaves the node so consecutive questions are decorrelated.
            if let Some(broadcast_at) = pending.broadcast_at {
                if Instant::now() >= broadcast_at {
                    if let Some(p2p) = &self.p2p {
                        p2p.broadcast_query(crate::p2p::SwarmQueryRequest {
                            request_id: pending.request_id.clone(),
                            asker_peer_id: p2p.peer_id(),
                            question: pending.question.clone(),
                            simhash: pending.simhash,
                            min_similarity: self.settings.hit_rate.percentage(),
                        });
                    }
                    pending.broadcast_at = None;
                    pending.started_at = Instant::now();
                }
                self.pending_query = Some(pending);
                return;
            }

            let mut resolved = false;

            if let Some(p2p) = &self.p2p {
                while let Ok(resp) = p2p.query_response_rx.try_recv() {
                    if resp.request_id != pending.request_id {
                        continue;
                    }

                    // ── Receiver-side gates for swarm hits ──
                    // 0. Anti-Poison Hard Gate: reject answerless or short content
                    if resp.content.trim().is_empty()
                        || resp.content.trim().len() < 10
                        || resp.question.trim().is_empty()
                        || resp.question.trim().len() < 3
                    {
                        continue;
                    }
                    // 1. Content integrity: hash must match the actual bytes.
                    if !resp.passes_integrity_checks() {
                        continue;
                    }
                    // 2. DLP inbound gate: never store/display leaked secrets.
                    if crate::dlp::scan_text(&resp.content).is_sensitive {
                        continue;
                    }
                    // 3. Content safety gate: never store/display prohibited content.
                    if !crate::content_safety::screen_text(&resp.content).is_allowed() {
                        continue;
                    }

                    // 4. Persist as swarm-sourced (verified hash + replay dedupe).
                    let Some(record) = db::save_swarm_inference(
                        &resp.question,
                        &resp.content,
                        pending.simhash,
                        &resp.provider,
                        &resp.model,
                        &resp.content_hash,
                    ) else {
                        continue;
                    };
                    self.insert_record(record.clone());
                    let _ = db::enforce_storage_limit_gb(self.settings.reserved_gb, self.settings.sharding_mode == ShardingMode::QueryLocality);

                    let formatted = format!("# {}\n\n{}", resp.question, resp.content);
                    let date_str = self.settings.date_format.format(&record.ts);
                    self.viewer = Some(ViewerState::with_swarm_metadata(
                        formatted,
                        resp.provider,
                        resp.model,
                        date_str,
                    ));
                    resolved = true;
                    break;
                }
            }

            if !resolved {
                // If query deadline expired (2.5s — gossipsub mesh settles a
                // heartbeat or two after connecting), fallback to AI Provider
                if pending.started_at.elapsed() > std::time::Duration::from_millis(2_500) {
                    self.dispatch_ai_inference(&pending.question, pending.simhash);
                } else {
                    self.pending_query = Some(pending);
                }
            }
        }

        // 2. Process active AI streaming
        if let Some(stream) = self.active_stream.take() {
            let mut keep_streaming = true;
            let mut full_received: Option<(String, bool)> = None;
            let mut error_received: Option<String> = None;

            while let Ok(msg) = stream.receiver.try_recv() {
                match msg {
                    crate::api::stream::StreamMessage::Token(token) => {
                        if let Some(v) = &mut self.viewer {
                            v.append_text(&token);
                            let w = self.body_width.get().max(1);
                            let h = v.visible_height(self.body_height.get().max(1));
                            v.scroll_to_bottom(w, h);
                        }
                    }
                    crate::api::stream::StreamMessage::Done { full_text, is_truncated } => {
                        full_received = Some((full_text, is_truncated));
                        keep_streaming = false;
                        break;
                    }
                    crate::api::stream::StreamMessage::Error(err) => {
                        error_received = Some(err);
                        keep_streaming = false;
                        break;
                    }
                }
            }

            if let Some((full_text, is_truncated)) = full_received {
                if let Some(v) = &mut self.viewer {
                    v.is_streaming = false;
                }
                // Save completed inference to local SQLite using the precalculated SimHash and exact model provenance
                if let Some(record) = db::save_inference_with_truncated(
                    &stream.question,
                    &full_text,
                    stream.simhash,
                    &stream.provider,
                    &stream.model,
                    is_truncated,
                ) {
                    if let Some(v) = &mut self.viewer {
                        v.record = Some(record.clone());
                    }
                    self.insert_record(record);
                    let _ = db::enforce_storage_limit_gb(self.settings.reserved_gb, self.settings.sharding_mode == ShardingMode::QueryLocality);

                    // P2P Gate: never gossip truncated inferences or inferences exceeding the 128 KB wire ceiling
                    if !is_truncated && full_text.len() <= crate::p2p::MAX_GOSSIP_PAYLOAD {
                        self.publish_completed_inference(&stream.question, &full_text, stream.simhash, &stream.provider, &stream.model);
                    }
                }
            } else if let Some(err) = error_received {
                if let Some(v) = &mut self.viewer {
                    v.append_text(&format!("\n\n❌ **Stream Error:** {err}\n"));
                    v.is_streaming = false;
                }
            } else if keep_streaming {
                self.active_stream = Some(stream);
            }
        }

        // 3. Process incoming P2P gossip inferences from swarm
        let now_epoch = chrono::Local::now().timestamp();
        let mut inbound = Vec::new();
        if let Some(p2p) = &self.p2p {
            while let Ok(msg) = p2p.inbound_inference_rx.try_recv() {
                // ── Receiver-side gates for gossip inferences ──
                // 0. Anti-Poison Hard Gate: drop answerless, empty, short or truncated inferences
                if msg.content.trim().is_empty()
                    || msg.content.trim().len() < 10
                    || msg.question.trim().is_empty()
                    || msg.question.trim().len() < 3
                    || msg.is_truncated
                {
                    continue;
                }
                // 1. Integrity: ceiling + content hash + timestamp + hop TTL.
                if !msg.passes_integrity_checks(now_epoch) {
                    continue;
                }
                // 2. DLP inbound gate: reject gossip carrying leaked secrets.
                if crate::dlp::scan_text(&msg.content).is_sensitive {
                    continue;
                }
                // 3. Content safety gate: prohibited content never touches disk.
                if !crate::content_safety::screen_text(&msg.content).is_allowed() {
                    continue;
                }
                // 4. Replay dedupe: content-addressing makes replays idempotent.
                if db::inference_exists(&msg.content_hash) {
                    continue;
                }
                // 5. Tombstone gate: never admit content marked with a negative signal.
                if db::is_tombstoned(&msg.content_hash) {
                    continue;
                }
                inbound.push(msg);
            }
        }
        for msg in inbound {
            if let Some(record) = db::save_swarm_inference(
                &msg.question,
                &msg.content,
                msg.simhash,
                &msg.provider,
                &msg.model,
                &msg.content_hash,
            ) {
                self.insert_record(record);
                let _ = db::enforce_storage_limit_gb(self.settings.reserved_gb, self.settings.sharding_mode == ShardingMode::QueryLocality);
            }
        }

        // 4. Process incoming P2P gossip tombstones (negative signals)
        if let Some(p2p) = &self.p2p {
            let mut tomb_received = false;
            while let Ok(tomb) = p2p.inbound_tombstone_rx.try_recv() {
                if tomb.passes_integrity_checks(now_epoch) {
                    db::add_tombstone(&tomb.content_hash, tomb.simhash, &tomb.reason);
                    tomb_received = true;
                }
            }
            if tomb_received {
                self.reload_records();
            }
        }
    }

    /// Sender-side publication gate for a completed L3 inference:
    ///
    /// 1. DLP output gate: redact any secrets the model may have generated.
    /// 2. Content safety stage-1: deterministic high-confidence screen.
    /// 3. Stage-2 (only when stage-1 flags): contextual classification via the
    ///    user's existing provider, budgeted per hour and **fail-closed** —
    ///    no verdict / error / no key ⇒ the answer stays local-only.
    fn publish_completed_inference(
        &mut self,
        question: &str,
        full_text: &str,
        simhash: u64,
        provider: &str,
        model: &str,
    ) {
        let sanitized_question = crate::sanitize::strip_control_chars(question);
        let sanitized_content = crate::sanitize::strip_control_chars(full_text);
        let sanitized_provider = crate::sanitize::strip_control_chars(provider);
        let sanitized_model = crate::sanitize::strip_control_chars(model);

        // DLP Output Gate: redact secrets before gossip.
        let broadcast_text = crate::dlp::redact_secrets(&sanitized_content);

        // Anti-Poison Hard Gate: never gossip empty or short answers
        if broadcast_text.trim().is_empty()
            || broadcast_text.trim().len() < 10
            || sanitized_question.trim().is_empty()
            || sanitized_question.trim().len() < 3
        {
            return;
        }

        let content_hash = crate::content_hash::compute_content_hash(
            &sanitized_question,
            &broadcast_text,
            &sanitized_provider,
            &sanitized_model,
        );

        let msg = crate::p2p::SwarmInferenceMessage {
            question: sanitized_question,
            content: broadcast_text,
            timestamp: chrono::Local::now().timestamp(),
            simhash,
            provider: sanitized_provider,
            model: sanitized_model,
            content_hash,
            hop_ttl: crate::p2p::MAX_HOP_TTL,
            is_truncated: false,
        };

        // Content safety stage-1: gate before gossip announce.
        match crate::content_safety::screen_text(&msg.content) {
            crate::content_safety::SafetyVerdict::Allow => {
                if let Some(p2p) = &self.p2p {
                    p2p.broadcast_inference(msg);
                }
            }
            crate::content_safety::SafetyVerdict::Reject { .. } => {
                // Stage-2 disambiguation, budgeted and fail-closed.
                if self.safety_budget_available() {
                    self.spawn_safety_classification(msg);
                }
                // else: budget exhausted / no key ⇒ local-only (fail-closed).
            }
        }
    }

    /// Hourly budget check + accounting for stage-2 classifications.
    fn safety_budget_available(&mut self) -> bool {
        if self.safety_budget_window.elapsed() > Duration::from_secs(3600) {
            self.safety_budget_window = Instant::now();
            self.safety_budget_used = 0;
        }
        if self.safety_budget_used >= MAX_CLASSIFICATIONS_PER_HOUR {
            return false;
        }
        let key = self.settings.api_key.trim();
        if key.is_empty() {
            return false;
        }
        self.safety_budget_used += 1;
        true
    }

    /// Spawns a background stage-2 classification; verdicts are collected in
    /// `drain_safety_checks` during later ticks.
    fn spawn_safety_classification(&mut self, msg: crate::p2p::SwarmInferenceMessage) {
        let provider_idx = self.settings.provider_idx;
        let provider = PROVIDERS[provider_idx];
        let provider_name = provider.name.to_string();
        let endpoint = provider.endpoint.to_string();
        let model = self.settings.provider_model.clone();
        let api_key = self.settings.api_key.trim().to_string();
        let text = msg.content.clone();

        let (tx, rx) = crossbeam_channel::unbounded();
        thread::Builder::new()
            .name("mbhub-safety".to_string())
            .spawn(move || {
                let result = crate::api::client::classify_content_safety(
                    &provider_name,
                    &endpoint,
                    &model,
                    Some(&api_key),
                    &text,
                );
                let _ = tx.send(result);
            })
            .expect("failed to spawn safety classification thread");

        self.pending_safety_checks.push(PendingSafetyCheck { msg, rx });
    }

    /// Collects stage-2 verdicts. Safe ⇒ publish; unsafe / error / disconnect
    /// ⇒ drop silently (fail-closed).
    fn drain_safety_checks(&mut self) {
        let mut i = 0;
        while i < self.pending_safety_checks.len() {
            let (publish, keep) = {
                let check = &self.pending_safety_checks[i];
                match check.rx.try_recv() {
                    Ok(Ok(true)) => (true, false),
                    Ok(Ok(false)) | Ok(Err(_)) => (false, false),
                    Err(crossbeam_channel::TryRecvError::Empty) => (false, true),
                    Err(crossbeam_channel::TryRecvError::Disconnected) => (false, false),
                }
            };
            if publish {
                let check = self.pending_safety_checks.remove(i);
                if let Some(p2p) = &self.p2p {
                    p2p.broadcast_inference(check.msg);
                }
            } else if keep {
                i += 1;
            } else {
                self.pending_safety_checks.remove(i);
            }
        }
    }

    fn handle_paste(&mut self, text: String) {
        if self.terms_modal
            || self.confirm_modal.is_some()
            || self.picker_modal.is_some()
            || self.file_browser_modal.is_some()
            || self.viewer.is_some()
        {
            return;
        }
        if self.editing {
            self.edit_buffer.input(Event::Paste(text));
            return;
        }
        if self.screen == Screen::Search {
            self.search_input.insert_str(&text);
        }
    }

    fn search_key(&mut self, key: KeyEvent) {
        if key.code == KeyCode::Enter {
            let query = self.search_input.text();
            let trimmed = query.trim();
            if trimmed.is_empty() {
                return;
            }

            // DLP Pre-Flight Gate: Block sensitive data from
            // ever reaching the P2P swarm or being sent to an AI provider.
            let dlp = crate::dlp::scan_text(trimmed);
            if dlp.is_sensitive {
                self.viewer = Some(ViewerState::new(format!(
                    "# ⚠️ Sensitive Data Detected\n\n\
                     This query appears to contain **{}**.\n\n\
                     For your security, the query was blocked and not broadcast to the P2P network or AI providers.\n\n\
                     Please do not paste sensitive keys or credentials into the search input.",
                    dlp.matched_pattern.unwrap_or("sensitive data")
                )));
                return;
            }

            // Step 1: Precompute question SimHash immediately upon entry
            let q_simhash = crate::simhash::compute_simhash(trimmed);
            let min_sim = self.settings.hit_rate.percentage();

            // Record the question in the local profile (SimHash only) and
            // re-rank the shard: Memory ordering and locality-aware eviction
            // follow the user's own query history (Query Locality mode).
            db::record_profile_query(q_simhash);
            self.reload_records();

            // Step 2: Search Local SQLite Memory first — the threshold gates
            // DIRECT DISPLAY only: an answer is shown straight away when it is
            // similar enough to the question; everything verified is stored
            // regardless of relevance.
            let min_ts = self
                .settings
                .freshness
                .min_timestamp(chrono::Local::now().timestamp());
            if let Some(cached_match) = db::find_best_match_query_fresh(trimmed, min_sim, min_ts) {
                let formatted = format!("# {}\n\n{}", cached_match.question, cached_match.content);
                let date_str = self.settings.date_format.format(&cached_match.ts);
                let vs = if cached_match.is_swarm {
                    ViewerState::with_swarm_metadata(
                        formatted,
                        cached_match.provider.clone(),
                        cached_match.model.clone(),
                        date_str,
                    )
                } else {
                    ViewerState::with_metadata(
                        formatted,
                        cached_match.provider.clone(),
                        cached_match.model.clone(),
                        date_str,
                    )
                };
                self.viewer = Some(vs.with_record(cached_match));
                return;
            }

            // Step 3: Local miss -> Search P2P Swarm if peers are connected
            let connected_peers = self.p2p.as_ref().map(|p| p.connected_peers()).unwrap_or(0);
            if connected_peers > 0 {
                let request_id = format!(
                    "{:x}",
                    std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .unwrap_or_default()
                        .as_nanos()
                );

                let notice = format!(
                    "# {}\n\n🌐 **Scanning P2P Swarm ({} connected peer{})...**\n\nSearching distributed collective memory before contacting AI provider...",
                    trimmed,
                    connected_peers,
                    if connected_peers == 1 { "" } else { "s" }
                );
                self.viewer = Some(ViewerState::new(notice));
                self.pending_query = Some(PendingSwarmQuery {
                    request_id,
                    question: trimmed.to_string(),
                    simhash: q_simhash,
                    started_at: std::time::Instant::now(),
                    // Privacy jitter: the query actually leaves
                    // the node 50-300 ms later, decorrelating consecutive asks.
                    broadcast_at: Some(std::time::Instant::now() + query_jitter()),
                });
                return;
            }

            // Step 4: No peers connected -> Dispatch to AI Provider immediately
            self.dispatch_ai_inference(trimmed, q_simhash);
            return;
        }

        let width = self.search_width.get().max(1);
        self.search_input.handle_key(key, width);
    }

    /// Dispatches a query to the configured AI Provider using live token streaming.
    fn dispatch_ai_inference(&mut self, question: &str, q_simhash: u64) {
        let provider_idx = self.settings.provider_idx;
        let provider = PROVIDERS[provider_idx];
        let model = if !self.settings.provider_model.is_empty() {
            self.settings.provider_model.clone()
        } else {
            "gpt-4o".to_string()
        };
        let api_key = self.settings.api_key.clone();

        if api_key.trim().is_empty() {
            let msg = format!(
                "# {}\n\n⚠️ **API Key Required**\n\nNo cached answer reached your Hit Rate threshold ({}), and no API key is configured for **{}**.\n\nPlease go to **SETTINGS > Cloud AI provider > API key** to configure your credentials.",
                question, self.settings.hit_rate.label(), provider.name
            );
            self.viewer = Some(ViewerState::new(msg));
            return;
        }

        let initial = format!("# {}\n\n", question);
        let now_ts = crate::db::from_unix(chrono::Local::now().timestamp());
        let date_str = self.settings.date_format.format(&now_ts);
        self.viewer = Some(ViewerState::streaming_with_metadata(
            initial,
            provider.name,
            &model,
            date_str,
        ));

        let rx = crate::api::stream::spawn_stream(provider_idx, &model, &api_key, question);
        self.active_stream = Some(ActiveStream {
            question: question.to_string(),
            simhash: q_simhash,
            provider: provider.name.to_string(),
            model,
            receiver: rx,
        });
    }

    fn memory_key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Enter => {
                if self.total_records > 0 && self.memory_selected < self.total_records {
                    if let Some(record) = self.get_memory_record(self.memory_selected) {
                        let formatted = format!("# {}\n\n{}", record.question, record.content);
                        let date_str = self.settings.date_format.format(&record.ts);
                        // Swarm-sourced records carry the unverified-source label
                        // — brand claims from peers are never
                        // presented as verified provenance.
                        let vs = if record.is_swarm {
                            ViewerState::with_swarm_metadata(
                                formatted,
                                record.provider.clone(),
                                record.model.clone(),
                                date_str,
                            )
                        } else {
                            ViewerState::with_metadata(
                                formatted,
                                record.provider.clone(),
                                record.model.clone(),
                                date_str,
                            )
                        };
                        self.viewer = Some(vs.with_record(record));
                    }
                }
            }
            KeyCode::Delete | KeyCode::Char('d') => {
                if self.total_records > 0 && self.memory_selected < self.total_records {
                    if let Some(record) = self.get_memory_record(self.memory_selected) {
                        let (hash, simhash) = crate::db::delete_and_tombstone_record(&record, "User deleted memory entry");
                        if let Some(p) = &self.p2p {
                            p.broadcast_tombstone(crate::p2p::SwarmTombstoneMessage {
                                content_hash: hash,
                                simhash,
                                timestamp: chrono::Local::now().timestamp(),
                                reporter_peer_id: p.peer_id(),
                                reason: "User deleted memory entry".to_string(),
                                signature: Vec::new(),
                            });
                        }
                        // Two-way deletion: publisher builds also prune the
                        // web archive (maintainer-only, never distributed).
                        #[cfg(feature = "publisher")]
                        trigger_cms_sync_background();
                        self.reload_records();
                        if self.total_records > 0 && self.memory_selected >= self.total_records {
                            self.memory_selected = self.total_records - 1;
                        }
                        self.scroll_into_view();
                        self.ensure_window();
                    }
                }
            }
            #[cfg(feature = "publisher")]
            KeyCode::Char('p') => {
                if self.total_records > 0 && self.memory_selected < self.total_records {
                    if let Some(record) = self.get_memory_record(self.memory_selected) {
                        let _ = crate::db::toggle_publish_candidate(&record.content_hash, &record.question);
                        self.reload_records();
                        self.scroll_into_view();
                        self.ensure_window();
                    }
                }
            }
            #[cfg(feature = "publisher")]
            KeyCode::Char('s') => {
                self.start_web_sync();
            }
            KeyCode::Up | KeyCode::Char('k') => {
                if self.memory_selected > 0 {
                    self.memory_selected -= 1;
                    self.scroll_into_view();
                    self.ensure_window();
                }
            }
            KeyCode::Down | KeyCode::Char('j') => {
                if self.memory_selected + 1 < self.total_records {
                    self.memory_selected += 1;
                    self.scroll_into_view();
                    self.ensure_window();
                }
            }
            KeyCode::PageUp => {
                let h = self.memory_height.get().max(1);
                self.memory_selected = self.memory_selected.saturating_sub(h);
                self.scroll_into_view();
                self.ensure_window();
            }
            KeyCode::PageDown => {
                let h = self.memory_height.get().max(1);
                self.memory_selected =
                    (self.memory_selected + h).min(self.total_records.saturating_sub(1));
                self.scroll_into_view();
                self.ensure_window();
            }
            KeyCode::Home | KeyCode::Char('g') => {
                self.memory_selected = 0;
                self.scroll_into_view();
                self.ensure_window();
            }
            KeyCode::End | KeyCode::Char('G') => {
                self.memory_selected = self.total_records.saturating_sub(1);
                self.scroll_into_view();
                self.ensure_window();
            }
            _ => {}
        }
    }

    /// Scroll the memory list only when the selection leaves the visible
    /// window; moving within the window just moves the highlight.
    fn scroll_into_view(&mut self) {
        let h = self.memory_height.get().max(1);
        if self.memory_selected < self.memory_offset {
            self.memory_offset = self.memory_selected;
        } else if self.memory_selected >= self.memory_offset + h {
            self.memory_offset = self.memory_selected - h + 1;
        }
    }

    fn settings_key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Up | KeyCode::Char('k') => self.focus_step(-1),
            KeyCode::Down | KeyCode::Char('j') => self.focus_step(1),
            KeyCode::Left => self.value_step(-1),
            KeyCode::Right => self.value_step(1),
            KeyCode::Enter => self.begin_edit(),
            _ => {}
        }
    }

    fn next_screen(&mut self) {
        self.screen = match self.screen {
            Screen::Search => Screen::Memory,
            Screen::Memory => Screen::Settings,
            Screen::Settings => Screen::Search,
        };
        self.editing = false;
        self.viewer = None;
        self.confirm_modal = None;
        self.picker_modal = None;
        self.file_browser_modal = None;
    }

    fn focus_step(&mut self, delta: isize) {
        let fields = self.visible_fields();
        if fields.is_empty() {
            return;
        }
        let cur_idx = fields.iter().position(|f| *f == self.focus).unwrap_or(0);
        let n = fields.len() as isize;
        let new_idx = (cur_idx as isize + delta).rem_euclid(n) as usize;
        self.focus = fields[new_idx];
    }

    /// Left/Right changes a selector's value quickly; Enter opens the Modal Picker.
    fn value_step(&mut self, delta: isize) {
        match self.focus {
            SettingsField::DateFormat => {
                let n = DateFormat::ALL.len() as isize;
                let idx = (DateFormat::ALL
                    .iter()
                    .position(|d| *d == self.settings.date_format)
                    .unwrap_or(0) as isize
                    + delta)
                    .rem_euclid(n) as usize;
                self.settings.date_format = DateFormat::ALL[idx];
            }
            SettingsField::ShardingMode => {
                let n = ShardingMode::ALL.len() as isize;
                let idx = (ShardingMode::ALL
                    .iter()
                    .position(|m| *m == self.settings.sharding_mode)
                    .unwrap_or(0) as isize
                    + delta)
                    .rem_euclid(n) as usize;
                let new_mode = ShardingMode::ALL[idx];
                if new_mode != self.settings.sharding_mode {
                    self.confirm_modal = Some(ConfirmModal {
                        title: "Switch sharding mode?".to_string(),
                        message: "Switching sharding modes requires purging local cache to rebuild shards under the new policy. Proceed with purge?".to_string(),
                        pending_sharding_mode: Some(new_mode),
                    });
                }
            }
            SettingsField::HitRate => {
                let n = HitRate::ALL.len() as isize;
                let idx = (HitRate::ALL
                    .iter()
                    .position(|h| *h == self.settings.hit_rate)
                    .unwrap_or(0) as isize
                    + delta)
                    .rem_euclid(n) as usize;
                self.settings.hit_rate = HitRate::ALL[idx];
            }
            SettingsField::Freshness => {
                let n = Freshness::ALL.len() as isize;
                let idx = (Freshness::ALL
                    .iter()
                    .position(|f| *f == self.settings.freshness)
                    .unwrap_or(0) as isize
                    + delta)
                    .rem_euclid(n) as usize;
                self.settings.freshness = Freshness::ALL[idx];
            }
            SettingsField::Provider => {
                let n = PROVIDERS.len() as isize;
                let idx = (self.settings.provider_idx as isize + delta).rem_euclid(n) as usize;
                self.set_provider(idx);
            }
            SettingsField::ProviderModel => {
                if !self.provider_models.is_empty() {
                    let cur_idx = self
                        .provider_models
                        .iter()
                        .position(|m| m == &self.settings.provider_model)
                        .unwrap_or(0) as isize;
                    let n = self.provider_models.len() as isize;
                    let new_idx = (cur_idx + delta).rem_euclid(n) as usize;
                    let model = self.provider_models[new_idx].clone();
                    self.settings.provider_model = model.clone();
                    let provider_name = PROVIDERS[self.settings.provider_idx].name;
                    self.settings
                        .provider_selected_models
                        .insert(provider_name.to_string(), model.clone());
                    crate::env::set_model_for_provider(provider_name, &model);
                }
            }
            SettingsField::Storage
            | SettingsField::ApiKey
            | SettingsField::BackupDatabase
            | SettingsField::RestoreDatabase
            | SettingsField::SyncWebArchive
            | SettingsField::ClearCache
            | SettingsField::TermsOfService => {}
        }
    }

    fn begin_edit(&mut self) {
        match self.focus {
            SettingsField::DateFormat => {
                let items: Vec<String> =
                    DateFormat::ALL.iter().map(|d| d.label().to_string()).collect();
                let selected = DateFormat::ALL
                    .iter()
                    .position(|d| *d == self.settings.date_format)
                    .unwrap_or(0);
                self.picker_modal = Some(PickerModal::new(
                    "Select date format".to_string(),
                    items,
                    selected,
                    SettingsField::DateFormat,
                ));
            }
            SettingsField::ShardingMode => {
                let items: Vec<String> =
                    ShardingMode::ALL.iter().map(|m| m.label().to_string()).collect();
                let selected = ShardingMode::ALL
                    .iter()
                    .position(|m| *m == self.settings.sharding_mode)
                    .unwrap_or(0);
                self.picker_modal = Some(PickerModal::new(
                    "Select sharding mode".to_string(),
                    items,
                    selected,
                    SettingsField::ShardingMode,
                ));
            }
            SettingsField::HitRate => {
                let items: Vec<String> =
                    HitRate::ALL.iter().map(|h| h.label().to_string()).collect();
                let selected = HitRate::ALL
                    .iter()
                    .position(|h| *h == self.settings.hit_rate)
                    .unwrap_or(0);
                self.picker_modal = Some(PickerModal::new(
                    "Select hit rate threshold".to_string(),
                    items,
                    selected,
                    SettingsField::HitRate,
                ));
            }
            SettingsField::Freshness => {
                let items: Vec<String> =
                    Freshness::ALL.iter().map(|f| f.label().to_string()).collect();
                let selected = Freshness::ALL
                    .iter()
                    .position(|f| *f == self.settings.freshness)
                    .unwrap_or(0);
                self.picker_modal = Some(PickerModal::new(
                    "Select answer freshness".to_string(),
                    items,
                    selected,
                    SettingsField::Freshness,
                ));
            }
            SettingsField::Provider => {
                let items: Vec<String> = PROVIDERS.iter().map(|p| p.name.to_string()).collect();
                self.picker_modal = Some(PickerModal::new(
                    "Select AI provider".to_string(),
                    items,
                    self.settings.provider_idx,
                    SettingsField::Provider,
                ));
            }
            SettingsField::ProviderModel => {
                if !self.provider_models.is_empty() {
                    let selected = self
                        .provider_models
                        .iter()
                        .position(|m| m == &self.settings.provider_model)
                        .unwrap_or(0);
                    self.picker_modal = Some(PickerModal::new(
                        "Select active model".to_string(),
                        self.provider_models.clone(),
                        selected,
                        SettingsField::ProviderModel,
                    ));
                }
            }
            SettingsField::Storage => {
                self.edit_buffer = single_line(&self.settings.reserved_gb.to_string());
                self.editing = true;
            }
            SettingsField::ApiKey => {
                self.edit_buffer = single_line(&self.settings.api_key);
                self.editing = true;
            }
            SettingsField::BackupDatabase => {
                self.file_browser_modal = Some(FileBrowserModal::new(
                    "Backup Database: Select Target Directory".to_string(),
                    FileBrowserMode::SelectDirectory,
                ));
            }
            SettingsField::RestoreDatabase => {
                self.file_browser_modal = Some(FileBrowserModal::new(
                    "Restore Database: Select Backup (.db) File".to_string(),
                    FileBrowserMode::SelectFile,
                ));
            }
            #[cfg(feature = "publisher")]
            SettingsField::SyncWebArchive => {
                self.start_web_sync();
            }
            SettingsField::ClearCache => {
                self.confirm_modal = Some(ConfirmModal {
                    title: "Clear local storage?".to_string(),
                    message: "Are you sure you want to purge all local inference records? The database will remain empty until new queries are executed.".to_string(),
                    pending_sharding_mode: None,
                });
            }
            SettingsField::TermsOfService => {
                self.viewer = Some(ViewerState::with_tos_metadata(
                    crate::tos::TERMS_OF_SERVICE_TEXT,
                    crate::tos::CURRENT_TOS_VERSION,
                ));
            }
    // Variants hidden from non-publisher builds resolve to no-op.
    #[cfg(not(feature = "publisher"))]
    _ => {}
        }
    }

    fn handle_file_browser_enter(&mut self) {
        let Some(browser) = &mut self.file_browser_modal else {
            return;
        };
        if browser.entries.is_empty() || browser.selected >= browser.entries.len() {
            return;
        }
        let entry = browser.entries[browser.selected].clone();

        match browser.mode {
            FileBrowserMode::SelectDirectory => {
                if entry.is_action {
                    let ts = chrono::Local::now().format("%Y%m%d_%H%M%S");
                    let filename = format!("mbhub_backup_{ts}.db");
                    let dest_path = browser.current_dir.join(&filename);

                    match db::backup_to_file(&dest_path) {
                        Ok(()) => {
                            let size = dest_path.metadata().map(|m| m.len()).unwrap_or(0);
                            let size_kb = (size as f64) / 1024.0;
                            let count = self.total_records;
                            self.file_browser_modal = None;
                            self.viewer = Some(ViewerState::new(format!(
                                "# Database Backup Successful\n\n✅ Stored full SQLite database snapshot to:\n`{}`\n\n- **Size:** {:.1} KB\n- **Total Records:** {}\n\nAll inferences, SimHashes, models, and metadata have been securely exported.",
                                dest_path.display(),
                                size_kb,
                                count
                            )));
                        }
                        Err(e) => {
                            self.file_browser_modal = None;
                            self.viewer = Some(ViewerState::new(format!(
                                "# Database Backup Failed\n\n❌ Error creating database backup at `{}`:\n`{e}`\n\nPlease check write permissions.",
                                dest_path.display()
                            )));
                        }
                    }
                } else if entry.is_dir {
                    browser.navigate_to(entry.path);
                }
            }
            FileBrowserMode::SelectFile => {
                if entry.is_dir {
                    browser.navigate_to(entry.path);
                } else {
                    let src_path = entry.path;
                    match db::restore_from_file(&src_path) {
                        Ok(count) => {
                            self.reload_records();
                            self.file_browser_modal = None;
                            self.viewer = Some(ViewerState::new(format!(
                                "# Database Restore Successful\n\n✅ Successfully restored **{} records** from backup:\n`{}`\n\nYour live SQLite database has been replaced with this snapshot. You can now browse all restored inferences in **MEMORY**.",
                                count,
                                src_path.display()
                            )));
                        }
                        Err(e) => {
                            self.file_browser_modal = None;
                            self.viewer = Some(ViewerState::new(format!(
                                "# Database Restore Failed\n\n❌ Failed to restore database from `{}`:\n`{e}`\n\nPlease ensure this file is a valid MBHub SQLite database.",
                                src_path.display()
                            )));
                        }
                    }
                }
            }
        }
    }

    fn commit_edit(&mut self) {
        let raw: String = self.edit_buffer.lines().join("");
        match self.focus {
            SettingsField::Storage => {
                let digits: String = raw.chars().filter(|c| c.is_ascii_digit()).collect();
                let n: u64 = digits.parse().unwrap_or(0);
                self.settings.reserved_gb = n.max(1);
            }
            SettingsField::ApiKey => {
                let trimmed = raw.trim().to_string();
                self.settings.api_key = trimmed.clone();
                let provider_name = PROVIDERS[self.settings.provider_idx].name;
                self.settings
                    .provider_keys
                    .insert(provider_name.to_string(), trimmed.clone());
                crate::env::set_api_key_for_provider(provider_name, &trimmed);
                self.refresh_provider_models();
            }
            _ => {}
        }
        self.editing = false;
    }

    /// Maintainer-only: starts the web archive pipeline in the background.
    /// The header bar shows a transient "syncing" indicator that flips to
    /// "completed / failed" when the run finishes and auto-clears after a few
    /// seconds — the current screen and selection stay exactly as they are.
    #[cfg(feature = "publisher")]
    pub fn start_web_sync(&mut self) {
        if self.pending_sync.is_some() {
            return; // already running — do not stack duplicate pipelines
        }
        let (tx, rx) = crossbeam_channel::unbounded();
        thread::spawn(move || {
            let outcome = crate::cms::run_sync();
            let _ = tx.send(outcome);
        });
        self.pending_sync = Some(rx);
        self.sync_status = Some(SyncStatus::Running);
    }
}

impl Default for App {
    fn default() -> Self {
        Self::new()
    }
}

/// A single-line editor preset (used for numeric / key inputs in Settings).
pub fn single_line(text: &str) -> TextArea<'static> {
    let mut ta = TextArea::default();
    ta.set_cursor_line_style(Style::default());
    ta.set_cursor_style(Style::default().add_modifier(Modifier::REVERSED));
    ta.set_style(theme::text());
    ta.insert_str(text);
    ta.move_cursor(CursorMove::End);
    ta
}

/// Random-ish 50-300 ms delay decorrelating consecutive outbound queries
///. No external RNG crate needed.
fn query_jitter() -> Duration {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.subsec_nanos() as u64)
        .unwrap_or(0);
    Duration::from_millis(50 + nanos % 251)
}

/// Maintainer-only: triggers headless synchronization with the web archive
/// repository in the background. Compiled exclusively into `publisher` builds.
#[cfg(feature = "publisher")]
pub fn trigger_cms_sync_background() {
    thread::spawn(|| {
        crate::cms::trigger_sync();
    });
}

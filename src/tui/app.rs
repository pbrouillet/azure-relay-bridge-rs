use std::path::PathBuf;
use anyhow::Result;
use crossterm::event::{KeyCode, KeyModifiers};

use crate::config::{Config, LocalForward, LocalForwardBinding, RemoteForward, RemoteForwardBinding};
use super::forms::{ConnectionForm, LocalForwardForm, RemoteForwardForm};
use super::log_layer::SharedLogSender;
use super::runner::Runner;

/// Which screen the TUI is currently displaying.
#[derive(Debug, Clone, PartialEq)]
pub enum Screen {
    MainMenu,
    ScaffoldChooseType,
    ScaffoldConnection,
    ScaffoldForwards,
    ScaffoldPreview,
    Browse,
    BrowseDetail,
    Run,
}

/// Whether we are scaffolding a client or server config.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ConfigKind {
    Client,
    Server,
}

/// An entry in the filesystem browser.
#[derive(Debug, Clone)]
pub enum BrowseEntry {
    /// Navigate to parent directory.
    ParentDir,
    /// A subdirectory.
    Directory { name: String, path: PathBuf },
    /// A YAML config file (eagerly parsed).
    ConfigFile {
        name: String,
        path: PathBuf,
        config: Option<Config>,
        error: Option<String>,
    },
}

/// Holds all TUI application state.
pub struct App {
    pub screen: Screen,
    pub screen_stack: Vec<Screen>,
    pub should_quit: bool,

    // Main menu
    pub menu_index: usize,

    // Scaffold: choose type
    pub scaffold_type_index: usize,
    pub scaffold_kind: ConfigKind,

    // Scaffold: connection form
    pub connection_form: ConnectionForm,

    // Scaffold: forwards
    pub local_forwards: Vec<LocalForwardForm>,
    pub remote_forwards: Vec<RemoteForwardForm>,
    pub forward_list_index: usize,
    pub editing_forward: bool,
    pub scaffold_save_path: String,

    // Scaffold: preview
    pub preview_yaml: String,
    pub preview_scroll: u16,

    // Browse
    pub browse_dir: PathBuf,
    pub browse_entries: Vec<BrowseEntry>,
    pub browse_index: usize,

    // Browse detail (selected config file)
    pub selected_config: Option<BrowseEntry>,
    pub detail_scroll: u16,

    // Run
    pub runner: Runner,
    pub run_config_path: Option<PathBuf>,
    pub run_log_scroll: u16,
    pub run_log_hscroll: u16,
    pub run_auto_scroll: bool,
    pub log_viewport_height: u16,

    // Status message
    pub status_message: Option<String>,
}

impl App {
    pub fn new(shared_log_sender: SharedLogSender) -> Self {
        Self {
            screen: Screen::MainMenu,
            screen_stack: Vec::new(),
            should_quit: false,
            menu_index: 0,
            scaffold_type_index: 0,
            scaffold_kind: ConfigKind::Client,
            connection_form: ConnectionForm::new(),
            local_forwards: Vec::new(),
            remote_forwards: Vec::new(),
            forward_list_index: 0,
            editing_forward: false,
            scaffold_save_path: String::new(),
            preview_yaml: String::new(),
            preview_scroll: 0,
            browse_dir: std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")),
            browse_entries: Vec::new(),
            browse_index: 0,
            selected_config: None,
            detail_scroll: 0,
            runner: Runner::new(shared_log_sender),
            run_config_path: None,
            run_log_scroll: 0,
            run_log_hscroll: 0,
            run_auto_scroll: true,
            log_viewport_height: 20,
            status_message: None,
        }
    }

    fn push_screen(&mut self, screen: Screen) {
        self.screen_stack.push(self.screen.clone());
        self.screen = screen;
    }

    fn pop_screen(&mut self) {
        if let Some(prev) = self.screen_stack.pop() {
            self.screen = prev;
        }
    }

    /// Handle a key press. Returns true if the app should exit.
    pub async fn handle_key(&mut self, code: KeyCode, modifiers: KeyModifiers) -> Result<bool> {
        // Global: Ctrl+C always quits
        if code == KeyCode::Char('c') && modifiers.contains(KeyModifiers::CONTROL) {
            self.runner.stop().await;
            return Ok(true);
        }

        match &self.screen {
            Screen::MainMenu => self.handle_main_menu(code).await,
            Screen::ScaffoldChooseType => self.handle_scaffold_choose_type(code),
            Screen::ScaffoldConnection => self.handle_scaffold_connection(code),
            Screen::ScaffoldForwards => self.handle_scaffold_forwards(code),
            Screen::ScaffoldPreview => self.handle_scaffold_preview(code).await,
            Screen::Browse => self.handle_browse(code).await,
            Screen::BrowseDetail => self.handle_browse_detail(code).await,
            Screen::Run => self.handle_run(code).await,
        }
    }

    async fn handle_main_menu(&mut self, code: KeyCode) -> Result<bool> {
        match code {
            KeyCode::Up | KeyCode::Char('k') => {
                if self.menu_index > 0 {
                    self.menu_index -= 1;
                }
            }
            KeyCode::Down | KeyCode::Char('j') => {
                if self.menu_index < 3 {
                    self.menu_index += 1;
                }
            }
            KeyCode::Enter => match self.menu_index {
                0 => {
                    self.scaffold_type_index = 0;
                    self.push_screen(Screen::ScaffoldChooseType);
                }
                1 => {
                    self.scan_directory();
                    self.browse_index = 0;
                    self.push_screen(Screen::Browse);
                }
                2 => {
                    self.scan_directory();
                    self.browse_index = 0;
                    self.push_screen(Screen::Browse);
                }
                3 => return Ok(true),
                _ => {}
            },
            KeyCode::Char('q') | KeyCode::Esc => return Ok(true),
            _ => {}
        }
        Ok(false)
    }

    fn handle_scaffold_choose_type(&mut self, code: KeyCode) -> Result<bool> {
        match code {
            KeyCode::Up | KeyCode::Char('k') => {
                if self.scaffold_type_index > 0 {
                    self.scaffold_type_index -= 1;
                }
            }
            KeyCode::Down | KeyCode::Char('j') => {
                if self.scaffold_type_index < 1 {
                    self.scaffold_type_index += 1;
                }
            }
            KeyCode::Enter => {
                self.scaffold_kind = if self.scaffold_type_index == 0 {
                    ConfigKind::Client
                } else {
                    ConfigKind::Server
                };
                self.connection_form = ConnectionForm::new();
                self.local_forwards.clear();
                self.remote_forwards.clear();
                self.push_screen(Screen::ScaffoldConnection);
            }
            KeyCode::Esc => self.pop_screen(),
            _ => {}
        }
        Ok(false)
    }

    fn handle_scaffold_connection(&mut self, code: KeyCode) -> Result<bool> {
        match code {
            KeyCode::Esc => self.pop_screen(),
            KeyCode::Tab => self.connection_form.next_field(),
            KeyCode::BackTab => self.connection_form.prev_field(),
            KeyCode::Enter => {
                // Move to forward entry
                self.forward_list_index = 0;
                self.editing_forward = false;
                match self.scaffold_kind {
                    ConfigKind::Client => {
                        if self.local_forwards.is_empty() {
                            self.local_forwards.push(LocalForwardForm::new());
                        }
                    }
                    ConfigKind::Server => {
                        if self.remote_forwards.is_empty() {
                            self.remote_forwards.push(RemoteForwardForm::new());
                        }
                    }
                }
                self.push_screen(Screen::ScaffoldForwards);
            }
            KeyCode::Char(c) => self.connection_form.input_char(c),
            KeyCode::Backspace => self.connection_form.backspace(),
            KeyCode::Left => self.connection_form.cursor_left(),
            KeyCode::Right => self.connection_form.cursor_right(),
            _ => {}
        }
        Ok(false)
    }

    fn handle_scaffold_forwards(&mut self, code: KeyCode) -> Result<bool> {
        if self.editing_forward {
            return self.handle_forward_editing(code);
        }

        match code {
            KeyCode::Esc => self.pop_screen(),
            KeyCode::Up | KeyCode::Char('k') => {
                if self.forward_list_index > 0 {
                    self.forward_list_index -= 1;
                }
            }
            KeyCode::Down | KeyCode::Char('j') => {
                let max = match self.scaffold_kind {
                    ConfigKind::Client => self.local_forwards.len(),
                    ConfigKind::Server => self.remote_forwards.len(),
                };
                if self.forward_list_index < max.saturating_sub(1) {
                    self.forward_list_index += 1;
                }
            }
            KeyCode::Enter | KeyCode::Char('e') => {
                self.editing_forward = true;
            }
            KeyCode::Char('a') => {
                match self.scaffold_kind {
                    ConfigKind::Client => {
                        self.local_forwards.push(LocalForwardForm::new());
                        self.forward_list_index = self.local_forwards.len() - 1;
                    }
                    ConfigKind::Server => {
                        self.remote_forwards.push(RemoteForwardForm::new());
                        self.forward_list_index = self.remote_forwards.len() - 1;
                    }
                }
                self.editing_forward = true;
            }
            KeyCode::Char('d') => {
                match self.scaffold_kind {
                    ConfigKind::Client => {
                        if !self.local_forwards.is_empty() {
                            self.local_forwards.remove(self.forward_list_index);
                            if self.forward_list_index > 0 && self.forward_list_index >= self.local_forwards.len() {
                                self.forward_list_index = self.local_forwards.len().saturating_sub(1);
                            }
                        }
                    }
                    ConfigKind::Server => {
                        if !self.remote_forwards.is_empty() {
                            self.remote_forwards.remove(self.forward_list_index);
                            if self.forward_list_index > 0 && self.forward_list_index >= self.remote_forwards.len() {
                                self.forward_list_index = self.remote_forwards.len().saturating_sub(1);
                            }
                        }
                    }
                }
            }
            KeyCode::Char('p') => {
                self.build_preview();
                self.preview_scroll = 0;
                self.push_screen(Screen::ScaffoldPreview);
            }
            _ => {}
        }
        Ok(false)
    }

    fn handle_forward_editing(&mut self, code: KeyCode) -> Result<bool> {
        match code {
            KeyCode::Esc => {
                self.editing_forward = false;
            }
            KeyCode::Tab => match self.scaffold_kind {
                ConfigKind::Client => {
                    if let Some(f) = self.local_forwards.get_mut(self.forward_list_index) {
                        f.next_field();
                    }
                }
                ConfigKind::Server => {
                    if let Some(f) = self.remote_forwards.get_mut(self.forward_list_index) {
                        f.next_field();
                    }
                }
            },
            KeyCode::BackTab => match self.scaffold_kind {
                ConfigKind::Client => {
                    if let Some(f) = self.local_forwards.get_mut(self.forward_list_index) {
                        f.prev_field();
                    }
                }
                ConfigKind::Server => {
                    if let Some(f) = self.remote_forwards.get_mut(self.forward_list_index) {
                        f.prev_field();
                    }
                }
            },
            KeyCode::Char(c) => match self.scaffold_kind {
                ConfigKind::Client => {
                    if let Some(f) = self.local_forwards.get_mut(self.forward_list_index) {
                        f.input_char(c);
                    }
                }
                ConfigKind::Server => {
                    if let Some(f) = self.remote_forwards.get_mut(self.forward_list_index) {
                        f.input_char(c);
                    }
                }
            },
            KeyCode::Backspace => match self.scaffold_kind {
                ConfigKind::Client => {
                    if let Some(f) = self.local_forwards.get_mut(self.forward_list_index) {
                        f.backspace();
                    }
                }
                ConfigKind::Server => {
                    if let Some(f) = self.remote_forwards.get_mut(self.forward_list_index) {
                        f.backspace();
                    }
                }
            },
            KeyCode::Left => match self.scaffold_kind {
                ConfigKind::Client => {
                    if let Some(f) = self.local_forwards.get_mut(self.forward_list_index) {
                        f.cursor_left();
                    }
                }
                ConfigKind::Server => {
                    if let Some(f) = self.remote_forwards.get_mut(self.forward_list_index) {
                        f.cursor_left();
                    }
                }
            },
            KeyCode::Right => match self.scaffold_kind {
                ConfigKind::Client => {
                    if let Some(f) = self.local_forwards.get_mut(self.forward_list_index) {
                        f.cursor_right();
                    }
                }
                ConfigKind::Server => {
                    if let Some(f) = self.remote_forwards.get_mut(self.forward_list_index) {
                        f.cursor_right();
                    }
                }
            },
            KeyCode::Enter => {
                self.editing_forward = false;
            }
            _ => {}
        }
        Ok(false)
    }

    async fn handle_scaffold_preview(&mut self, code: KeyCode) -> Result<bool> {
        match code {
            KeyCode::Esc => self.pop_screen(),
            KeyCode::Up | KeyCode::Char('k') => {
                self.preview_scroll = self.preview_scroll.saturating_sub(1);
            }
            KeyCode::Down | KeyCode::Char('j') => {
                self.preview_scroll = self.preview_scroll.saturating_add(1);
            }
            KeyCode::Char('s') => {
                self.save_config()?;
            }
            _ => {}
        }
        Ok(false)
    }

    async fn handle_browse(&mut self, code: KeyCode) -> Result<bool> {
        match code {
            KeyCode::Esc => self.pop_screen(),
            KeyCode::Up | KeyCode::Char('k') => {
                if self.browse_index > 0 {
                    self.browse_index -= 1;
                }
            }
            KeyCode::Down | KeyCode::Char('j') => {
                if self.browse_index < self.browse_entries.len().saturating_sub(1) {
                    self.browse_index += 1;
                }
            }
            KeyCode::Enter => {
                if let Some(entry) = self.browse_entries.get(self.browse_index).cloned() {
                    match &entry {
                        BrowseEntry::ParentDir => {
                            if let Some(parent) = self.browse_dir.parent() {
                                self.browse_dir = parent.to_path_buf();
                                self.scan_directory();
                                self.browse_index = 0;
                            }
                        }
                        BrowseEntry::Directory { path, .. } => {
                            self.browse_dir = path.clone();
                            self.scan_directory();
                            self.browse_index = 0;
                        }
                        BrowseEntry::ConfigFile { .. } => {
                            self.selected_config = Some(entry);
                            self.detail_scroll = 0;
                            self.push_screen(Screen::BrowseDetail);
                        }
                    }
                }
            }
            KeyCode::Backspace => {
                if let Some(parent) = self.browse_dir.parent() {
                    self.browse_dir = parent.to_path_buf();
                    self.scan_directory();
                    self.browse_index = 0;
                }
            }
            KeyCode::Char('~') => {
                if let Some(home) = dirs::home_dir() {
                    self.browse_dir = home;
                    self.scan_directory();
                    self.browse_index = 0;
                }
            }
            _ => {}
        }
        Ok(false)
    }

    async fn handle_browse_detail(&mut self, code: KeyCode) -> Result<bool> {
        match code {
            KeyCode::Esc => self.pop_screen(),
            KeyCode::Up | KeyCode::Char('k') => {
                self.detail_scroll = self.detail_scroll.saturating_sub(1);
            }
            KeyCode::Down | KeyCode::Char('j') => {
                self.detail_scroll = self.detail_scroll.saturating_add(1);
            }
            KeyCode::Char('r') => {
                if let Some(BrowseEntry::ConfigFile { ref path, ref config, .. }) = self.selected_config {
                    if config.is_some() {
                        self.run_config_path = Some(path.clone());
                        self.run_log_scroll = 0;
                        self.run_log_hscroll = 0;
                        self.run_auto_scroll = true;
                        self.runner.start(path.clone()).await?;
                        self.push_screen(Screen::Run);
                    }
                }
            }
            KeyCode::Char('d') => {
                if let Some(BrowseEntry::ConfigFile { ref path, .. }) = self.selected_config {
                    let _ = std::fs::remove_file(path);
                    self.selected_config = None;
                    self.pop_screen();
                    self.scan_directory();
                    if self.browse_index >= self.browse_entries.len() {
                        self.browse_index = self.browse_entries.len().saturating_sub(1);
                    }
                }
            }
            _ => {}
        }
        Ok(false)
    }

    async fn handle_run(&mut self, code: KeyCode) -> Result<bool> {
        match code {
            KeyCode::Char('q') | KeyCode::Esc => {
                self.runner.stop().await;
                self.pop_screen();
            }
            KeyCode::Up | KeyCode::Char('k') => {
                self.run_auto_scroll = false;
                self.run_log_scroll = self.run_log_scroll.saturating_sub(1);
            }
            KeyCode::Down | KeyCode::Char('j') => {
                self.run_log_scroll = self.run_log_scroll.saturating_add(1);
            }
            KeyCode::Left | KeyCode::Char('h') => {
                self.run_log_hscroll = self.run_log_hscroll.saturating_sub(4);
            }
            KeyCode::Right | KeyCode::Char('l') => {
                self.run_log_hscroll = self.run_log_hscroll.saturating_add(4);
            }
            KeyCode::Home => {
                self.run_log_hscroll = 0;
            }
            KeyCode::PageUp => {
                self.run_auto_scroll = false;
                self.run_log_scroll = self.run_log_scroll.saturating_sub(20);
            }
            KeyCode::PageDown => {
                self.run_log_scroll = self.run_log_scroll.saturating_add(20);
            }
            KeyCode::Char('f') => {
                self.run_auto_scroll = true;
            }
            _ => {}
        }
        Ok(false)
    }

    /// Update async state each tick.
    pub async fn tick(&mut self) {
        self.runner.poll_logs().await;

        // Auto-scroll to bottom of logs
        if self.run_auto_scroll && self.screen == Screen::Run {
            let log_count = self.runner.logs.len() as u16;
            // Scroll so the last lines are visible at the bottom of the viewport
            self.run_log_scroll = log_count.saturating_sub(self.log_viewport_height);
        }
    }

    /// Scan the current `browse_dir` and populate `browse_entries` with
    /// a parent-dir entry, subdirectories (sorted), and `.yml`/`.yaml` files (sorted, eagerly parsed).
    fn scan_directory(&mut self) {
        self.browse_entries.clear();

        // Canonicalize the browse dir
        if let Ok(canonical) = std::fs::canonicalize(&self.browse_dir) {
            self.browse_dir = canonical;
        }

        // Parent directory entry (unless we're at a root)
        if self.browse_dir.parent().is_some() {
            self.browse_entries.push(BrowseEntry::ParentDir);
        }

        let entries = match std::fs::read_dir(&self.browse_dir) {
            Ok(e) => e,
            Err(_) => return,
        };

        let mut dirs: Vec<(String, PathBuf)> = Vec::new();
        let mut files: Vec<(String, PathBuf)> = Vec::new();

        for entry in entries.flatten() {
            let path = entry.path();
            let name = entry
                .file_name()
                .to_string_lossy()
                .to_string();

            // Skip hidden files/dirs
            if name.starts_with('.') {
                continue;
            }

            if path.is_dir() {
                dirs.push((name, path));
            } else if path.is_file() {
                if let Some(ext) = path.extension() {
                    let ext = ext.to_string_lossy().to_lowercase();
                    if ext == "yml" || ext == "yaml" {
                        files.push((name, path));
                    }
                }
            }
        }

        dirs.sort_by(|a, b| a.0.to_lowercase().cmp(&b.0.to_lowercase()));
        files.sort_by(|a, b| a.0.to_lowercase().cmp(&b.0.to_lowercase()));

        for (name, path) in dirs {
            self.browse_entries.push(BrowseEntry::Directory { name, path });
        }

        for (name, path) in files {
            let (config, error) = match std::fs::read_to_string(&path) {
                Ok(content) => match serde_yaml::from_str::<Config>(&content) {
                    Ok(cfg) => (Some(cfg), None),
                    Err(e) => (None, Some(format!("Parse error: {e}"))),
                },
                Err(e) => (None, Some(format!("Read error: {e}"))),
            };
            self.browse_entries.push(BrowseEntry::ConfigFile {
                name,
                path,
                config,
                error,
            });
        }
    }

    /// Build YAML preview from current scaffold state.
    fn build_preview(&mut self) {
        let config = self.build_config();
        self.preview_yaml = match serde_yaml::to_string(&config) {
            Ok(yaml) => yaml,
            Err(e) => format!("# Error generating YAML: {e}"),
        };
    }

    /// Build a Config from the current scaffold state.
    fn build_config(&self) -> Config {
        let mut config = Config::default();

        let conn = &self.connection_form;
        if !conn.endpoint.value.is_empty() {
            config.azure_relay_endpoint = Some(conn.endpoint.value.clone());
        }
        if !conn.connection_string.value.is_empty() {
            config.azure_relay_connection_string = Some(conn.connection_string.value.clone());
        }
        if !conn.sas_key_name.value.is_empty() {
            config.azure_relay_shared_access_key_name = Some(conn.sas_key_name.value.clone());
        }
        if !conn.sas_key.value.is_empty() {
            config.azure_relay_shared_access_key = Some(conn.sas_key.value.clone());
        }
        if !conn.log_level.value.is_empty() {
            config.log_level = Some(conn.log_level.value.clone());
        }

        match self.scaffold_kind {
            ConfigKind::Client => {
                for lf_form in &self.local_forwards {
                    let mut lf = LocalForward {
                        relay_name: lf_form.relay_name.value.clone(),
                        ..Default::default()
                    };
                    let port: i32 = lf_form.bind_port.value.parse().unwrap_or(0);
                    if port != 0 || !lf_form.bind_address.value.is_empty() {
                        lf.bindings.push(LocalForwardBinding {
                            bind_address: if lf_form.bind_address.value.is_empty() {
                                None
                            } else {
                                Some(lf_form.bind_address.value.clone())
                            },
                            bind_port: port,
                            port_name: if lf_form.port_name.value.is_empty() {
                                None
                            } else {
                                Some(lf_form.port_name.value.clone())
                            },
                            ..Default::default()
                        });
                    }
                    config.local_forward.push(lf);
                }
            }
            ConfigKind::Server => {
                for rf_form in &self.remote_forwards {
                    let mut rf = RemoteForward {
                        relay_name: rf_form.relay_name.value.clone(),
                        ..Default::default()
                    };
                    let port: i32 = rf_form.host_port.value.parse().unwrap_or(0);
                    if port != 0 || !rf_form.host.value.is_empty() {
                        rf.bindings.push(RemoteForwardBinding {
                            host: if rf_form.host.value.is_empty() {
                                None
                            } else {
                                Some(rf_form.host.value.clone())
                            },
                            host_port: port,
                            port_name: if rf_form.port_name.value.is_empty() {
                                None
                            } else {
                                Some(rf_form.port_name.value.clone())
                            },
                            http: rf_form.http,
                            ..Default::default()
                        });
                    }
                    config.remote_forward.push(rf);
                }
            }
        }

        config
    }

    /// Save the preview YAML to a file.
    fn save_config(&mut self) -> Result<()> {
        let path = if self.scaffold_save_path.is_empty() {
            let default_name = match self.scaffold_kind {
                ConfigKind::Client => "azbridge_client.yml",
                ConfigKind::Server => "azbridge_server.yml",
            };
            PathBuf::from(default_name)
        } else {
            PathBuf::from(&self.scaffold_save_path)
        };

        match std::fs::write(&path, &self.preview_yaml) {
            Ok(()) => {
                self.status_message = Some(format!("Saved to {}", path.display()));
            }
            Err(e) => {
                self.status_message = Some(format!("Save failed: {e}"));
            }
        }
        Ok(())
    }
}

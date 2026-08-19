//! A self-contained folder browser used by the "Open folder" button.
//!
//! This avoids depending on the XDG desktop portal's file chooser, which needs
//! a backend installed for the current desktop session. On sessions with no
//! matching backend (e.g. `niri` without `xdg-desktop-portal-gtk`) the portal
//! silently returns nothing, so the app ships its own little browser instead.

use std::path::PathBuf;

use eframe::egui;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BrowserAction {
    None,
    Open(PathBuf),
    Cancel,
}

pub struct FolderBrowser {
    cwd: PathBuf,
    entries: Vec<PathBuf>,
    selected: usize,
    error: Option<String>,
    path_text: String,
}

impl FolderBrowser {
    pub fn new(start: PathBuf) -> Self {
        let start = if start.as_os_str().is_empty() {
            home_dir().unwrap_or_else(|| PathBuf::from("/"))
        } else {
            start
        };
        let mut browser = Self {
            cwd: PathBuf::new(),
            entries: Vec::new(),
            selected: 0,
            error: None,
            path_text: String::new(),
        };
        browser.goto(start);
        browser
    }

    /// Renders the browser window; returns what the user asked to do.
    pub fn show(&mut self, ctx: &egui::Context) -> BrowserAction {
        let mut action = BrowserAction::None;
        let mut open = true;
        egui::Window::new("Open folder")
            .collapsible(false)
            .resizable(true)
            .default_size([480.0, 380.0])
            .min_size([360.0, 260.0])
            .open(&mut open)
            .show(ctx, |ui| {
                action = self.ui(ui);
            });
        if !open {
            return BrowserAction::Cancel;
        }
        action
    }

    fn ui(&mut self, ui: &mut egui::Ui) -> BrowserAction {
        let mut action = BrowserAction::None;

        // Path bar + keyboard navigation.
        ui.horizontal(|ui| {
            ui.label("Path:");
            let resp = ui.add(
                egui::TextEdit::singleline(&mut self.path_text)
                    .desired_width(ui.available_width() - 60.0),
            );
            let go = ui.button("Go").clicked()
                || (resp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)));
            if go {
                self.navigate_from_path();
            }

            let typing = resp.has_focus();
            let (up, down, right, left, enter, esc) = ui.input(|i| {
                (
                    i.key_pressed(egui::Key::ArrowUp),
                    i.key_pressed(egui::Key::ArrowDown),
                    i.key_pressed(egui::Key::ArrowRight),
                    i.key_pressed(egui::Key::ArrowLeft),
                    i.key_pressed(egui::Key::Enter),
                    i.key_pressed(egui::Key::Escape),
                )
            });
            if !typing {
                if up {
                    self.select(self.selected.saturating_sub(1));
                }
                if down {
                    self.select(self.selected + 1);
                }
                if right {
                    self.enter_selected();
                }
                if left {
                    self.up();
                }
                // Enter confirms the folder being browsed (same as "Open");
                // while typing, Enter is handled by the path field above.
                if enter && !go {
                    action = BrowserAction::Open(self.cwd.clone());
                }
            }
            if esc {
                action = BrowserAction::Cancel;
            }
        });

        // Navigation toolbar.
        ui.horizontal(|ui| {
            if ui.button("Home").clicked() {
                self.home();
            }
            if ui.button("Root").clicked() {
                self.root();
            }
            if ui.button("Up").clicked() {
                self.up();
            }
        });

        ui.add_space(4.0);

        // Directory list.
        let list_height = (ui.available_height() - 44.0).max(80.0);
        egui::ScrollArea::vertical()
            .max_height(list_height)
            .show(ui, |ui| {
                if self.entries.is_empty() && self.error.is_none() {
                    ui.label("(no subfolders)");
                }
                let mut enter: Option<PathBuf> = None;
                for (i, path) in self.entries.iter().enumerate() {
                    let name = path
                        .file_name()
                        .map(|s| s.to_string_lossy().to_string())
                        .unwrap_or_default();
                    let resp = ui.selectable_label(self.selected == i, format!("📁  {name}"));
                    if resp.clicked() {
                        self.selected = i;
                    }
                    if resp.double_clicked() {
                        enter = Some(path.clone());
                    }
                }
                if let Some(path) = enter {
                    self.goto(path);
                }
            });

        if let Some(err) = &self.error {
            ui.add_space(4.0);
            ui.colored_label(egui::Color32::from_rgb(0xe5, 0x6a, 0x6a), err);
        }

        ui.with_layout(egui::Layout::right_to_left(egui::Align::Max), |ui| {
            if ui
                .button("Open")
                .on_hover_text("Load this folder (Enter)")
                .clicked()
            {
                action = BrowserAction::Open(self.cwd.clone());
            }
            if ui.button("Cancel").clicked() {
                action = BrowserAction::Cancel;
            }
        });

        action
    }

    fn goto(&mut self, path: PathBuf) {
        let path = if path.is_absolute() {
            path
        } else if self.cwd.as_os_str().is_empty() {
            PathBuf::from("/").join(path)
        } else {
            self.cwd.join(path)
        };
        self.cwd = path;
        self.refresh();
    }

    fn refresh(&mut self) {
        self.path_text = self.cwd.to_string_lossy().to_string();
        let mut dirs = Vec::new();
        match std::fs::read_dir(&self.cwd) {
            Ok(rd) => {
                for entry in rd.flatten() {
                    if entry.path().is_dir() {
                        dirs.push(entry.path());
                    }
                }
                dirs.sort_by_key(|p| p.file_name().map(|s| s.to_string_lossy().to_lowercase()));
                self.entries = dirs;
                self.selected = 0;
                self.error = None;
            }
            Err(e) => {
                self.entries.clear();
                self.selected = 0;
                self.error = Some(format!("Cannot open {}: {e}", self.cwd.display()));
            }
        }
    }

    fn up(&mut self) {
        if let Some(parent) = self.cwd.parent() {
            let parent = parent.to_path_buf();
            if parent != self.cwd {
                self.goto(parent);
            }
        }
    }

    fn home(&mut self) {
        if let Some(home) = home_dir() {
            self.goto(home);
        }
    }

    fn root(&mut self) {
        self.goto(PathBuf::from("/"));
    }

    fn enter_selected(&mut self) {
        if let Some(path) = self.entries.get(self.selected) {
            let path = path.clone();
            self.goto(path);
        }
    }

    fn select(&mut self, idx: usize) {
        self.selected = idx.min(self.entries.len().saturating_sub(1));
    }

    fn navigate_from_path(&mut self) {
        let raw = self.path_text.trim();
        if raw.is_empty() {
            return;
        }
        let path = expand_tilde(raw);
        let path = if path.is_absolute() {
            path
        } else {
            self.cwd.join(path)
        };
        if path.is_dir() {
            self.goto(path);
        } else {
            self.error = Some(format!("Not a directory: {}", path.display()));
        }
    }
}

fn home_dir() -> Option<PathBuf> {
    std::env::var("HOME")
        .ok()
        .filter(|h| !h.is_empty())
        .map(PathBuf::from)
}

fn expand_tilde(path: &str) -> PathBuf {
    if let Some(rest) = path.strip_prefix("~/") {
        if let Some(home) = home_dir() {
            return home.join(rest);
        }
    }
    PathBuf::from(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lists_and_navigates_subfolders() {
        let dir = std::env::temp_dir().join("imora-browser-test");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("beta")).unwrap();
        std::fs::create_dir_all(dir.join("alpha")).unwrap();
        std::fs::write(dir.join("file.txt"), "x").unwrap();

        let mut browser = FolderBrowser::new(dir.clone());
        assert_eq!(browser.entries.len(), 2);
        assert!(browser.entries[0].ends_with("alpha"));
        assert!(browser.entries[1].ends_with("beta"));
        assert!(browser.error.is_none());

        browser.enter_selected(); // -> alpha
        assert_eq!(browser.cwd.file_name().unwrap().to_str(), Some("alpha"));

        browser.up(); // back to dir
        assert_eq!(browser.cwd, dir);

        // Relative + tilde path resolution.
        browser.path_text = "beta".into();
        browser.navigate_from_path();
        assert_eq!(browser.cwd.file_name().unwrap().to_str(), Some("beta"));

        // A bad path sets an error instead of panicking.
        browser.path_text = "/no/such/dir".into();
        browser.navigate_from_path();
        assert!(browser.error.is_some());

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn renders_without_panicking() {
        let ctx = egui::Context::default();
        let dir = std::env::temp_dir().join("imora-browser-render");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("sub")).unwrap();

        let mut browser = FolderBrowser::new(dir.clone());
        let mut output = ctx.run_ui(egui::RawInput::default(), |ui| {
            let _ = browser.show(ui.ctx());
        });
        // The test has no renderer to apply texture deltas to.
        output.textures_delta.clear();

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Feed a single key press through a real egui pass and return the
    /// browser's action.
    fn press_key(browser: &mut FolderBrowser, key: egui::Key) -> BrowserAction {
        let ctx = egui::Context::default();
        let input = egui::RawInput {
            events: vec![egui::Event::Key {
                key,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: Default::default(),
            }],
            ..Default::default()
        };
        let mut action = BrowserAction::None;
        let mut output = ctx.run_ui(input, |ui| {
            action = browser.show(ui.ctx());
        });
        output.textures_delta.clear();
        action
    }

    #[test]
    fn enter_opens_current_folder_and_arrows_navigate() {
        let dir = std::env::temp_dir().join("imora-browser-keys");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("alpha")).unwrap();
        std::fs::create_dir_all(dir.join("beta")).unwrap();

        let mut browser = FolderBrowser::new(dir.clone());

        // ArrowRight descends into the highlighted entry (alpha).
        let _ = press_key(&mut browser, egui::Key::ArrowRight);
        assert!(browser.cwd.ends_with("alpha"));

        // ArrowLeft goes back up.
        let _ = press_key(&mut browser, egui::Key::ArrowLeft);
        assert_eq!(browser.cwd, dir);

        // Enter confirms the browsed folder, like the "Open" button.
        let action = press_key(&mut browser, egui::Key::Enter);
        assert_eq!(action, BrowserAction::Open(dir.clone()));

        let _ = std::fs::remove_dir_all(&dir);
    }
}

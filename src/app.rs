//! The imora media gallery UI.

use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::Arc;
use std::time::{Duration, Instant};

use eframe::egui::{
    self, Align, Align2, Color32, ColorImage, FontId, Key, Layout, Margin, Pos2, PointerButton,
    Rect, RichText, ScrollArea, Sense, TextureHandle, TextureOptions, Vec2, pos2, vec2,
};

use crate::media::{self, AnimatedFrame, MediaItem, MediaKind};
use crate::thumbs::{self, ThumbJob, ThumbResult};
use crate::video::{PlayerState, VideoPlayer};

const ACCENT: Color32 = Color32::from_rgb(0x7c, 0x9a, 0xe0);
const BG: Color32 = Color32::from_rgb(0x0d, 0x0f, 0x12);
const PANEL: Color32 = Color32::from_rgb(0x12, 0x15, 0x1a);
const CELL: Color32 = Color32::from_rgb(0x1a, 0x1e, 0x25);
const TEXT: Color32 = Color32::from_rgb(0xd0, 0xd4, 0xda);
const MUTED: Color32 = Color32::from_rgb(0x8a, 0x90, 0x99);
const TRACK: Color32 = Color32::from_rgb(0x2a, 0x30, 0x3a);

const TOP_BAR_H: f32 = 42.0;
const STRIP_H: f32 = 96.0;
const THUMB_CELL: f32 = 84.0;

enum LoadData {
    Image(media::LoadedImage),
    Video(VideoPlayer),
    Failed(String),
}

struct LoadOutcome {
    gen: u64,
    data: LoadData,
}

enum Loaded {
    Image {
        texture: TextureHandle,
        frames: Vec<AnimatedFrame>,
        width: u32,
        height: u32,
    },
    Video(VideoPlayer),
    Failed(String),
}

enum Action {
    Next,
    Prev,
    Skip(isize),
    Goto(usize),
}

pub struct ImoraApp {
    folder: Option<PathBuf>,
    items: Vec<MediaItem>,
    index: usize,

    loaded: Option<Loaded>,
    load_gen: u64,
    load_tx: Sender<LoadOutcome>,
    load_rx: Receiver<LoadOutcome>,

    zoom: f32,
    pan: Vec2,
    dragging: bool,
    last_drag: Pos2,

    fade: f32,

    show_strip: bool,
    strip_need_scroll: bool,
    thumbs: HashMap<PathBuf, ThumbEntry>,
    pending_thumbs: HashSet<PathBuf>,
    thumb_job_tx: Sender<ThumbJob>,
    thumb_res_rx: Receiver<ThumbResult>,

    video_tex: RefCell<Option<TextureHandle>>,
    video_last_pts: Cell<f64>,
    scrub_active: Cell<bool>,
    scrub_frac: Cell<f32>,

    anim_idx: usize,
    anim_accum: f32,

    fullscreen: bool,
    start_fullscreen: bool,
    ui_hidden: bool,
    pointer_inside: bool,
    last_activity: Instant,
    first_hint_until: Instant,

    slideshow: bool,
    slide_interval: f32,
    slideshow_last: Instant,
}

struct ThumbEntry {
    tex: TextureHandle,
    aspect: f32,
}

impl ImoraApp {
    pub fn new(
        cc: &eframe::CreationContext<'_>,
        folder: Option<PathBuf>,
        start_fullscreen: bool,
        slide_interval: f32,
        start_slideshow: bool,
    ) -> Self {
        setup_style(&cc.egui_ctx);

        let (load_tx, load_rx) = mpsc::channel();
        let (thumb_job_tx, thumb_job_rx) = mpsc::channel();
        let (thumb_res_tx, thumb_res_rx) = mpsc::channel();
        std::thread::spawn(move || {
            while let Ok(job) = thumb_job_rx.recv() {
                if let Some(res) = thumbs::make_thumb(&job) {
                    if thumb_res_tx.send(res).is_err() {
                        break;
                    }
                }
            }
        });

        let mut app = Self {
            folder: None,
            items: Vec::new(),
            index: 0,
            loaded: None,
            load_gen: 0,
            load_tx,
            load_rx,
            zoom: 1.0,
            pan: Vec2::ZERO,
            dragging: false,
            last_drag: Pos2::ZERO,
            fade: 0.0,
            show_strip: true,
            strip_need_scroll: true,
            thumbs: HashMap::new(),
            pending_thumbs: HashSet::new(),
            thumb_job_tx,
            thumb_res_rx,
            video_tex: RefCell::new(None),
            video_last_pts: Cell::new(-1.0),
            scrub_active: Cell::new(false),
            scrub_frac: Cell::new(0.0),
            anim_idx: 0,
            anim_accum: 0.0,
            fullscreen: false,
            start_fullscreen,
            ui_hidden: false,
            pointer_inside: false,
            last_activity: Instant::now(),
            first_hint_until: Instant::now() + Duration::from_secs(6),
            slideshow: start_slideshow,
            slide_interval: slide_interval.clamp(0.5, 3600.0),
            slideshow_last: Instant::now(),
        };

        if let Some(dir) = folder {
            app.set_folder(dir);
        }
        app
    }

    // ---- state -----------------------------------------------------------

    fn set_folder(&mut self, dir: PathBuf) {
        let items = media::scan_folder(&dir);
        self.folder = Some(dir);
        self.items = items;
        self.index = 0;
        self.thumbs.clear();
        self.pending_thumbs.clear();
        self.reset_view();
        self.spawn_load();
    }

    fn open_path(&mut self, path: &Path) {
        if path.is_dir() {
            self.set_folder(path.to_path_buf());
            return;
        }
        if media::classify(path).is_none() {
            return;
        }
        if let Some(parent) = path.parent() {
            let items = media::scan_folder(parent);
            let index = items.iter().position(|it| it.path == path).unwrap_or(0);
            self.folder = Some(parent.to_path_buf());
            self.items = items;
            self.index = index;
            self.thumbs.clear();
            self.pending_thumbs.clear();
            self.reset_view();
            self.spawn_load();
        }
    }

    fn open_folder_dialog(&mut self) {
        if let Some(dir) = rfd::FileDialog::new().set_title("Open folder").pick_folder() {
            self.set_folder(dir);
        }
    }

    fn open_externally(&self) {
        if let Some(item) = self.items.get(self.index) {
            let _ = std::process::Command::new("xdg-open").arg(&item.path).spawn();
        }
    }

    fn reset_view(&mut self) {
        self.load_gen += 1;
        self.loaded = None;
        self.zoom = 1.0;
        self.pan = Vec2::ZERO;
        self.fade = 0.0;
        self.video_tex = RefCell::new(None);
        self.video_last_pts = Cell::new(-1.0);
        self.anim_idx = 0;
        self.anim_accum = 0.0;
        self.scrub_active = Cell::new(false);
        self.strip_need_scroll = true;
    }

    fn goto(&mut self, index: usize) {
        if index == self.index && self.loaded.is_some() {
            return;
        }
        self.index = index;
        self.reset_view();
        self.spawn_load();
        // A manual (or slideshow) jump restarts the slideshow countdown.
        self.slideshow_last = Instant::now();
    }

    fn toggle_slideshow(&mut self) {
        self.slideshow = !self.slideshow;
        self.slideshow_last = Instant::now();
    }

    fn spawn_load(&mut self) {
        if self.items.is_empty() {
            self.loaded = Some(Loaded::Failed("No media found here.".into()));
            return;
        }
        let item = self.items[self.index].clone();
        let gen = self.load_gen;
        let tx = self.load_tx.clone();
        std::thread::spawn(move || {
            let data = match item.kind {
                MediaKind::Image => match media::load_image(&item.path) {
                    Ok(img) => LoadData::Image(img),
                    Err(e) => LoadData::Failed(format!("{} — {e}", item.name)),
                },
                MediaKind::Video => LoadData::Video(VideoPlayer::open(item.path)),
            };
            let _ = tx.send(LoadOutcome { gen, data });
        });
    }

    fn apply_action(&mut self, a: Action) {
        let n = self.items.len();
        if n == 0 {
            return;
        }
        let cur = self.index as isize;
        let target = match a {
            Action::Next => cur + 1,
            Action::Prev => cur - 1,
            Action::Skip(d) => cur + d,
            Action::Goto(usize::MAX) => (n - 1) as isize,
            Action::Goto(i) => i as isize,
        };
        self.goto(target.rem_euclid(n as isize) as usize);
    }

    // ---- input -----------------------------------------------------------

    fn handle_keys(&mut self, ctx: &egui::Context) {
        let mut action: Option<Action> = None;

        if ctx.input(|i| i.key_pressed(Key::ArrowRight) || i.key_pressed(Key::ArrowDown)) {
            action = Some(Action::Next);
        }
        if ctx.input(|i| i.key_pressed(Key::ArrowLeft) || i.key_pressed(Key::ArrowUp)) {
            action = Some(Action::Prev);
        }
        if ctx.input(|i| i.key_pressed(Key::PageDown)) {
            action = Some(Action::Skip(10));
        }
        if ctx.input(|i| i.key_pressed(Key::PageUp)) {
            action = Some(Action::Skip(-10));
        }
        if ctx.input(|i| i.key_pressed(Key::Home)) {
            action = Some(Action::Goto(0));
        }
        if ctx.input(|i| i.key_pressed(Key::End)) {
            action = Some(Action::Goto(usize::MAX));
        }
        // `focused` is the native window's keyboard focus, not a widget's —
        // buttons consume Space via `consume_key`, so no extra guard needed.
        if ctx.input(|i| i.key_pressed(Key::Space)) {
            if let Some(Loaded::Video(p)) = &self.loaded {
                if p.state().playing {
                    p.pause();
                } else {
                    p.play();
                }
            } else {
                action = Some(Action::Next);
            }
        }
        if ctx.input(|i| i.key_pressed(Key::F)) {
            self.toggle_fullscreen(ctx);
        }
        if ctx.input(|i| i.key_pressed(Key::G)) {
            self.show_strip = !self.show_strip;
        }
        if ctx.input(|i| i.key_pressed(Key::S)) {
            self.toggle_slideshow();
        }
        if ctx.input(|i| i.key_pressed(Key::O)) {
            self.open_folder_dialog();
        }
        if ctx.input(|i| i.key_pressed(Key::E)) {
            self.open_externally();
        }
        if ctx.input(|i| i.key_pressed(Key::Escape)) && self.fullscreen {
            self.toggle_fullscreen(ctx);
        }
        if ctx.input(|i| i.key_pressed(Key::Num0)) {
            self.zoom = 1.0;
            self.pan = Vec2::ZERO;
        }
        if ctx.input(|i| i.key_pressed(Key::Plus) || i.key_pressed(Key::Equals)) {
            self.zoom = (self.zoom * 1.25).clamp(1.0, 24.0);
        }
        if ctx.input(|i| i.key_pressed(Key::Minus)) {
            self.zoom = (self.zoom / 1.25).clamp(1.0, 24.0);
        }

        if let Some(a) = action {
            self.apply_action(a);
        }
    }

    fn handle_pointer(&mut self, ctx: &egui::Context) {
        let (hover, primary_down, primary_pressed, primary_released, scroll, zoom_delta, double_clicked) =
            ctx.input(|i| {
                (
                    i.pointer.hover_pos(),
                    i.pointer.primary_down(),
                    i.pointer.primary_pressed(),
                    i.pointer.primary_released(),
                    i.smooth_scroll_delta,
                    i.zoom_delta(),
                    i.pointer.button_double_clicked(PointerButton::Primary),
                )
            });

        let Some(hover) = hover else {
            return;
        };

        if double_clicked {
            self.toggle_fullscreen(ctx);
            return;
        }

        // Only zoom when the pointer is over the central media area.
        let screen = ctx.content_rect();
        let strip_h = if self.show_strip && !self.ui_hidden {
            STRIP_H
        } else {
            0.0
        };
        let over_central =
            hover.y > TOP_BAR_H + 4.0 && hover.y < screen.height() - strip_h - 4.0;

        if over_central {
            if (zoom_delta - 1.0).abs() > 0.001 {
                self.zoom = (self.zoom * zoom_delta).clamp(1.0, 24.0);
            }
            if scroll.y != 0.0 {
                let factor = (scroll.y * 0.004).exp();
                self.zoom = (self.zoom * factor).clamp(1.0, 24.0);
            }
        }

        if primary_pressed {
            self.dragging = true;
            self.last_drag = hover;
        }
        if primary_released {
            self.dragging = false;
        }
        if self.dragging && primary_down {
            let d = hover - self.last_drag;
            self.pan += d;
            self.last_drag = hover;
        }
    }

    fn handle_drops(&mut self, ctx: &egui::Context) {
        let files = ctx.input(|i| i.raw.dropped_files.clone());
        for f in files {
            let p = f.path().to_path_buf();
            if !p.as_os_str().is_empty() {
                self.open_path(&p);
                break;
            }
        }
    }

    fn toggle_fullscreen(&mut self, ctx: &egui::Context) {
        self.fullscreen = !self.fullscreen;
        ctx.send_viewport_cmd(egui::ViewportCommand::Fullscreen(self.fullscreen));
        self.last_activity = Instant::now();
    }

    // ---- background loading ----------------------------------------------

    fn poll_load(&mut self, ctx: &egui::Context) {
        while let Ok(outcome) = self.load_rx.try_recv() {
            if outcome.gen != self.load_gen {
                continue; // stale; the LoadData is dropped here
            }
            match outcome.data {
                LoadData::Image(img) => {
                    let (w, h) = (img.width, img.height);
                    let rgba = &img.frames[0].rgba;
                    let color = ColorImage::from_rgba_unmultiplied([w as usize, h as usize], rgba);
                    let texture = ctx.load_texture("media", color, TextureOptions::LINEAR);
                    self.loaded = Some(Loaded::Image {
                        texture,
                        frames: img.frames,
                        width: w,
                        height: h,
                    });
                }
                LoadData::Video(player) => {
                    self.video_tex = RefCell::new(None);
                    self.video_last_pts = Cell::new(-1.0);
                    self.loaded = Some(Loaded::Video(player));
                }
                LoadData::Failed(e) => {
                    self.loaded = Some(Loaded::Failed(e));
                }
            }
            self.fade = 0.0;
        }
    }

    fn poll_thumbs(&mut self, ctx: &egui::Context) {
        while let Ok(res) = self.thumb_res_rx.try_recv() {
            let color = ColorImage::from_rgba_unmultiplied([res.width, res.height], &res.rgba);
            let tex = ctx.load_texture("thumb", color, TextureOptions::LINEAR);
            let aspect = res.width as f32 / res.height as f32;
            self.thumbs.insert(res.path, ThumbEntry { tex, aspect });
        }
    }

    fn request_thumbs(&mut self) {
        let n = self.items.len();
        if n == 0 {
            return;
        }
        let lo = self.index.saturating_sub(12);
        let hi = (self.index + 13).min(n);
        for i in lo..hi {
            let item = &self.items[i];
            if self.thumbs.contains_key(&item.path) || self.pending_thumbs.contains(&item.path) {
                continue;
            }
            if self
                .thumb_job_tx
                .send(ThumbJob {
                    path: item.path.clone(),
                    kind: item.kind,
                })
                .is_ok()
            {
                self.pending_thumbs.insert(item.path.clone());
            }
        }
    }

    fn tick_animation(&mut self, ctx: &egui::Context, dt: f32) {
        let Some(Loaded::Image { texture, frames, .. }) = &self.loaded else {
            return;
        };
        if frames.len() <= 1 {
            return;
        }
        self.anim_accum += dt;
        let mut guard = 0;
        while frames[self.anim_idx].delay.as_secs_f32().max(0.02) <= self.anim_accum {
            self.anim_accum -= frames[self.anim_idx].delay.as_secs_f32().max(0.02);
            self.anim_idx = (self.anim_idx + 1) % frames.len();
            guard += 1;
            if guard >= frames.len() * 2 {
                break;
            }
        }
        let f = &frames[self.anim_idx];
        let mut tex = texture.clone();
        tex.set(
            ColorImage::from_rgba_unmultiplied([f.width, f.height], &f.rgba),
            TextureOptions::LINEAR,
        );
        ctx.request_repaint();
    }

    fn needs_repaint(&self) -> bool {
        if let Some(Loaded::Video(p)) = &self.loaded {
            if p.state().playing {
                return true;
            }
        }
        if let Some(Loaded::Image { frames, .. }) = &self.loaded {
            if frames.len() > 1 {
                return true;
            }
        }
        self.loaded.is_none()
    }

    fn tick_slideshow(&mut self, ctx: &egui::Context) {
        if !self.slideshow || self.items.len() <= 1 {
            return;
        }
        let interval = Duration::from_secs_f32(self.slide_interval.max(0.5));
        let elapsed = self.slideshow_last.elapsed();
        if elapsed >= interval {
            self.slideshow_last = Instant::now();
            self.apply_action(Action::Next);
        } else {
            // Wake up in time for the next transition, even if nothing else
            // is repainting (e.g. a paused video or a still image).
            ctx.request_repaint_after(interval - elapsed);
        }
    }

    // ---- drawing ---------------------------------------------------------

    fn top_bar(&mut self, ui: &mut egui::Ui) {
        egui::Panel::top(egui::Id::new("top-bar"))
            .default_size(TOP_BAR_H)
            .min_size(TOP_BAR_H)
            .max_size(TOP_BAR_H)
            .frame(egui::Frame::default().fill(PANEL).inner_margin(Margin::symmetric(12, 6)))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.label(RichText::new("imora").strong().size(16.0).color(ACCENT));
                    ui.separator();
                    let folder_text = self
                        .folder
                        .as_deref()
                        .map(|p| p.to_string_lossy().to_string())
                        .unwrap_or_else(|| "no folder".into());
                    ui.label(RichText::new(truncate(&folder_text, 56)).color(MUTED));
                    ui.add_space(8.0);
                    ui.label(RichText::new(truncate(&self.current_name(), 40)).color(TEXT));

                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if ui.button("⛶").on_hover_text("Fullscreen (F)").clicked() {
                            self.toggle_fullscreen(ui.ctx());
                        }
                        if ui.button("▤").on_hover_text("Filmstrip (G)").clicked() {
                            self.show_strip = !self.show_strip;
                        }
                        if ui.button("⌂").on_hover_text("Open folder (O)").clicked() {
                            self.open_folder_dialog();
                        }

                        // Slideshow quick toggle + settings.
                        let label = if self.slideshow {
                            format!("⏸  {:.1}s", self.slide_interval)
                        } else {
                            format!("▶  {:.1}s", self.slide_interval)
                        };
                        let sb = ui.button(label).on_hover_text("Slideshow (S)");
                        if sb.clicked() {
                            self.toggle_slideshow();
                        }
                        let gb = ui.button("⚙").on_hover_text("Slideshow settings");
                        egui::Popup::menu(&gb).show(|ui| {
                            ui.set_min_width(180.0);
                            ui.label(RichText::new("Interval between items").color(TEXT));
                            ui.add(
                                egui::Slider::new(&mut self.slide_interval, 0.5..=60.0)
                                    .suffix(" s")
                                    .step_by(0.5),
                            );
                            ui.add_space(4.0);
                            let start = if self.slideshow {
                                "Stop slideshow"
                            } else {
                                "Start slideshow"
                            };
                            if ui
                                .button(RichText::new(start).color(ACCENT))
                                .clicked()
                            {
                                self.toggle_slideshow();
                                ui.close();
                            }
                        });
                    });
                });
            });
    }

    fn bottom_strip(&mut self, ui: &mut egui::Ui) {
        self.request_thumbs();

        egui::Panel::bottom(egui::Id::new("filmstrip"))
            .default_size(STRIP_H)
            .min_size(STRIP_H)
            .max_size(STRIP_H)
            .frame(egui::Frame::default().fill(PANEL).inner_margin(Margin::symmetric(8, 8)))
            .show(ui, |ui| {
                let cell = Vec2::splat(THUMB_CELL);
                let offset = if self.strip_need_scroll {
                    let avail_w = ui.available_width().max(1.0);
                    (self.index as f32 * (THUMB_CELL + 8.0) - avail_w * 0.5).max(0.0)
                } else {
                    0.0
                };
                self.strip_need_scroll = false;

                let mut scroll = ScrollArea::horizontal().id_salt("filmstrip-scroll");
                if offset > 0.0 {
                    scroll = scroll.horizontal_scroll_offset(offset);
                }

                let mut click: Option<usize> = None;
                scroll.show(ui, |ui| {
                    ui.horizontal(|ui| {
                        for (i, item) in self.items.iter().enumerate() {
                            let (r, resp) = ui.allocate_exact_size(cell, Sense::click());
                            ui.painter().rect_filled(r, 6.0, CELL);
                            if let Some(entry) = self.thumbs.get(&item.path) {
                                let fit = Rect::from_center_size(
                                    r.center(),
                                    fit_into(r.size(), entry.aspect),
                                );
                                ui.painter().image(entry.tex.id(), fit, uv_rect(), Color32::WHITE);
                            } else {
                                ui.painter().circle_filled(r.center(), 4.0, MUTED);
                            }
                            if i == self.index {
                                ui.painter().rect_stroke(
                                    r,
                                    6.0,
                                    egui::Stroke::new(2.0, ACCENT),
                                    egui::StrokeKind::Inside,
                                );
                            } else if resp.hovered() {
                                ui.painter().rect_stroke(
                                    r,
                                    6.0,
                                    egui::Stroke::new(1.0, Color32::from_rgb(0x3a, 0x41, 0x4c)),
                                    egui::StrokeKind::Inside,
                                );
                            }
                            if resp.clicked() {
                                click = Some(i);
                            }
                        }
                    });
                });
                if let Some(i) = click {
                    self.goto(i);
                }
            });
    }

    fn paint_media(&mut self, ui: &mut egui::Ui, rect: Rect) {
        let painter = ui.painter().clone();
        let center = rect.center();

        if self.items.is_empty() {
            self.paint_welcome(ui, rect);
            return;
        }

        match &self.loaded {
            None => {
                painter.text(
                    center,
                    Align2::CENTER_CENTER,
                    "Loading…",
                    FontId::proportional(16.0),
                    MUTED,
                );
                ui.ctx().request_repaint_after(Duration::from_millis(50));
            }
            Some(Loaded::Failed(msg)) => {
                self.paint_error(&painter, rect, msg);
            }
            Some(Loaded::Image {
                texture,
                frames,
                width,
                height,
            }) => {
                let aspect = *width as f32 / *height as f32;
                let r = self.media_rect(rect, aspect);
                let alpha = ease_out(self.fade);
                let rr = Rect::from_center_size(r.center(), r.size() * (0.96 + 0.04 * alpha));
                painter.image(
                    texture.id(),
                    rr,
                    uv_rect(),
                    Color32::from_white_alpha((alpha * 255.0) as u8),
                );
                if frames.len() > 1 && !self.fullscreen {
                    painter.text(
                        pos2(rr.max.x - 8.0, rr.min.y + 8.0),
                        Align2::RIGHT_TOP,
                        "GIF",
                        FontId::monospace(11.0),
                        Color32::from_white_alpha((alpha * 220.0) as u8),
                    );
                }
            }
            Some(Loaded::Video(player)) => {
                let st = player.state();
                let fr = player.frame();
                if fr.width > 0 {
                    let aspect = fr.width as f32 / fr.height as f32;
                    let r = self.media_rect(rect, aspect);
                    let color =
                        ColorImage::from_rgb([fr.width as usize, fr.height as usize], &fr.rgb);
                    let alpha = ease_out(self.fade);
                    {
                        let mut tex = self.video_tex.borrow_mut();
                        if let Some(t) = tex.as_mut() {
                            if fr.pts != self.video_last_pts.get() {
                                t.set(color, TextureOptions::LINEAR);
                                self.video_last_pts.set(fr.pts);
                            }
                        } else {
                            *tex = Some(ui.ctx().load_texture("video", color, TextureOptions::LINEAR));
                            self.video_last_pts.set(fr.pts);
                        }
                        let t = tex.as_ref().unwrap();
                        painter.image(
                            t.id(),
                            r,
                            uv_rect(),
                            Color32::from_white_alpha((alpha * 255.0) as u8),
                        );
                    }
                    if alpha > 0.5
                        && (!self.fullscreen || self.pointer_inside || self.scrub_active.get())
                    {
                        self.paint_controls(ui, &painter, rect, player, &st, fr.pts);
                    }
                } else if let Some(err) = &st.error {
                    self.paint_error(&painter, rect, err);
                } else {
                    painter.text(
                        center,
                        Align2::CENTER_CENTER,
                        "Preparing video…",
                        FontId::proportional(16.0),
                        MUTED,
                    );
                }
            }
        }

        // Overlay: index + name (hidden in fullscreen).
        if self.fade > 0.1 && !self.fullscreen {
            let n = self.items.len();
            let name = truncate(&self.current_name(), 60);
            let label = format!("{} — {} / {}", name, self.index + 1, n);
            let a = (self.fade * 255.0 * 0.9) as u8;
            painter.text(
                pos2(rect.center().x, rect.min.y + 20.0),
                Align2::CENTER_TOP,
                label,
                FontId::proportional(13.0),
                Color32::from_rgba_unmultiplied(0x8a, 0x90, 0x99, a),
            );
        }

        if Instant::now() < self.first_hint_until && self.items.len() > 1 && !self.fullscreen {
            painter.text(
                pos2(rect.center().x, rect.max.y - 26.0),
                Align2::CENTER_BOTTOM,
                "← → browse · space play · F fullscreen · G filmstrip · O open · E system player",
                FontId::proportional(12.0),
                MUTED,
            );
        }
    }

    fn paint_welcome(&mut self, ui: &mut egui::Ui, rect: Rect) {
        let painter = ui.painter();
        let center = rect.center();
        painter.text(
            center - vec2(0.0, 60.0),
            Align2::CENTER_CENTER,
            "imora",
            FontId::proportional(48.0),
            ACCENT,
        );
        painter.text(
            center - vec2(0.0, 20.0),
            Align2::CENTER_CENTER,
            "A tiny, elegant media gallery.",
            FontId::proportional(16.0),
            MUTED,
        );
        let btn_rect = Rect::from_center_size(center + vec2(0.0, 30.0), vec2(200.0, 36.0));
        if ui
            .put(
                btn_rect,
                egui::Button::new(RichText::new("Open folder…").size(15.0)),
            )
            .clicked()
        {
            self.open_folder_dialog();
        }
    }

    fn paint_error(&self, painter: &egui::Painter, rect: Rect, msg: &str) {
        let c = rect.center();
        painter.text(
            c + vec2(0.0, -12.0),
            Align2::CENTER_CENTER,
            "Could not open media",
            FontId::proportional(18.0),
            TEXT,
        );
        painter.text(
            c + vec2(0.0, 14.0),
            Align2::CENTER_CENTER,
            truncate(msg, 90),
            FontId::proportional(13.0),
            MUTED,
        );
        painter.text(
            c + vec2(0.0, 38.0),
            Align2::CENTER_CENTER,
            "E — open with system player · O — open another folder",
            FontId::proportional(12.0),
            MUTED,
        );
    }

    fn paint_controls(
        &self,
        ui: &mut egui::Ui,
        painter: &egui::Painter,
        rect: Rect,
        player: &VideoPlayer,
        st: &PlayerState,
        pts: f64,
    ) {
        let duration = st.duration.max(0.001);
        let pos = if st.playing || st.position > 0.0 {
            st.position
        } else {
            pts
        };
        let bar = Rect::from_min_size(
            pos2(rect.min.x + 24.0, rect.max.y - 30.0),
            vec2((rect.width() - 48.0).max(1.0), 4.0),
        );
        let resp = ui.interact(bar.expand(6.0), ui.id().with("seek"), Sense::click_and_drag());

        let frac = if self.scrub_active.get() {
            if let Some(p) = resp.interact_pointer_pos() {
                self.scrub_frac.set(((p.x - bar.min.x) / bar.width()).clamp(0.0, 1.0));
            }
            self.scrub_frac.get()
        } else {
            (pos as f32 / duration as f32).clamp(0.0, 1.0)
        };

        if resp.drag_started() {
            self.scrub_active.set(true);
            if let Some(p) = resp.interact_pointer_pos() {
                self.scrub_frac.set(((p.x - bar.min.x) / bar.width()).clamp(0.0, 1.0));
            }
        }
        if resp.drag_stopped() || resp.clicked() {
            self.scrub_active.set(false);
            player.seek(self.scrub_frac.get() as f64 * duration);
        }

        painter.rect_filled(bar, 2.0, TRACK);
        let filled = Rect::from_min_size(bar.min, vec2(bar.width() * frac, bar.height()));
        painter.rect_filled(filled, 2.0, ACCENT);
        painter.circle_filled(pos2(bar.min.x + bar.width() * frac, bar.center().y), 5.0, ACCENT);

        let t = frac as f64 * duration;
        let label = format!("{} / {}", fmt_time(t), fmt_time(duration));
        painter.text(
            pos2(rect.center().x, bar.min.y - 12.0),
            Align2::CENTER_CENTER,
            label,
            FontId::monospace(12.0),
            MUTED,
        );
    }

    fn media_rect(&self, avail: Rect, aspect: f32) -> Rect {
        let a = avail.width() / avail.height().max(1.0);
        let (mut w, mut h) = if aspect > a {
            (avail.width(), avail.width() / aspect)
        } else {
            (avail.height() * aspect, avail.height())
        };
        w *= self.zoom;
        h *= self.zoom;
        let maxx = ((w - avail.width()).max(0.0)) * 0.5;
        let maxy = ((h - avail.height()).max(0.0)) * 0.5;
        let panx = self.pan.x.clamp(-maxx, maxx);
        let pany = self.pan.y.clamp(-maxy, maxy);
        Rect::from_center_size(
            avail.center() + vec2(panx, pany),
            vec2(w.max(1.0), h.max(1.0)),
        )
    }

    fn current_name(&self) -> String {
        self.items
            .get(self.index)
            .map(|i| i.name.clone())
            .unwrap_or_default()
    }
}

impl eframe::App for ImoraApp {
    fn logic(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        let dt = ctx.input(|i| i.stable_dt.max(0.001));

        if self.start_fullscreen {
            self.start_fullscreen = false;
            ctx.send_viewport_cmd(egui::ViewportCommand::Fullscreen(true));
            self.fullscreen = true;
        }

        let activity = ctx.input(|i| !i.events.is_empty());
        if activity {
            self.last_activity = Instant::now();
        }
        // In fullscreen, show nothing but the media itself.
        self.ui_hidden = self.fullscreen;
        self.pointer_inside = ctx.input(|i| i.pointer.hover_pos().is_some());
        // Also hide the cursor once idle in fullscreen.
        if self.fullscreen && self.last_activity.elapsed() > Duration::from_secs(3) {
            ctx.set_cursor_icon(egui::CursorIcon::None);
        }

        self.handle_drops(ctx);
        self.handle_keys(ctx);
        self.handle_pointer(ctx);

        self.poll_load(ctx);
        self.poll_thumbs(ctx);
        self.tick_animation(ctx, dt);

        self.fade = (self.fade + dt / 0.16).min(1.0);

        self.tick_slideshow(ctx);

        if self.needs_repaint() {
            ctx.request_repaint_after(Duration::from_millis(16));
        }
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        if !self.ui_hidden {
            self.top_bar(ui);
        }
        if self.show_strip && !self.ui_hidden {
            self.bottom_strip(ui);
        }

        egui::CentralPanel::default()
            .frame(egui::Frame::default().fill(BG).inner_margin(Margin::same(0)))
            .show(ui, |ui| {
                let rect = ui.max_rect();
                self.paint_media(ui, rect);
            });
    }
}

// ---- style & helpers -----------------------------------------------------

fn setup_style(ctx: &egui::Context) {
    ctx.set_theme(egui::ThemePreference::Dark);
    let mut style = (*ctx.style_of(egui::Theme::Dark)).clone();
    style.visuals = egui::Visuals::dark();
    style.visuals.panel_fill = BG;
    style.visuals.window_fill = PANEL;
    style.visuals.extreme_bg_color = BG;
    style.visuals.faint_bg_color = CELL;
    style.visuals.override_text_color = Some(TEXT);
    style.visuals.selection.bg_fill = ACCENT;
    style.visuals.widgets.inactive.bg_fill = Color32::from_rgb(0x1b, 0x1f, 0x26);
    style.visuals.widgets.hovered.bg_fill = Color32::from_rgb(0x26, 0x2b, 0x34);
    style.visuals.widgets.active.bg_fill = Color32::from_rgb(0x2e, 0x35, 0x40);
    style.spacing.item_spacing = vec2(8.0, 6.0);
    style.spacing.button_padding = vec2(10.0, 4.0);
    ctx.set_style_of(egui::Theme::Dark, Arc::new(style));
}

fn ease_out(t: f32) -> f32 {
    1.0 - (1.0 - t).powi(3)
}

fn uv_rect() -> Rect {
    Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0))
}

fn fit_into(avail: Vec2, aspect: f32) -> Vec2 {
    if aspect <= 0.0 || !aspect.is_finite() {
        return avail;
    }
    let a = avail.x / avail.y.max(1.0);
    if aspect > a {
        vec2(avail.x, avail.x / aspect)
    } else {
        vec2(avail.y * aspect, avail.y)
    }
}

fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        let mut out: String = s.chars().take(max.saturating_sub(1)).collect();
        out.push('…');
        out
    }
}

fn fmt_time(secs: f64) -> String {
    let s = secs.max(0.0) as u64;
    let h = s / 3600;
    let m = (s % 3600) / 60;
    let sec = s % 60;
    if h > 0 {
        format!("{h}:{m:02}:{sec:02}")
    } else {
        format!("{m}:{sec:02}")
    }
}

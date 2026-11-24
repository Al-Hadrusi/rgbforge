//! The window: device list on the left, lighting controls in the middle.

use std::time::{Duration, Instant};

use eframe::egui::{
    self, Align, Color32, CornerRadius, FontId, Layout, Margin, Pos2, Rect, RichText, Sense, Stroke, StrokeKind,
    TextStyle, Ui, Vec2,
};

use crate::config::{self, Settings};
use crate::drivers::{self, DemoMode};
use crate::effects::{self, Effect, Lighting};
use crate::engine::{Cmd, DeviceStatus, Engine, Status};
use crate::model::{Kind, Rgb};

const BG: Color32 = Color32::from_rgb(13, 15, 20);
const PANEL: Color32 = Color32::from_rgb(20, 23, 30);
const CARD: Color32 = Color32::from_rgb(29, 33, 43);
const CARD_HOVER: Color32 = Color32::from_rgb(38, 43, 56);
const ACCENT: Color32 = Color32::from_rgb(139, 92, 246);
const TEXT: Color32 = Color32::from_rgb(230, 232, 240);
const DIM: Color32 = Color32::from_rgb(140, 146, 162);
const WARN: Color32 = Color32::from_rgb(251, 191, 36);
const ERR: Color32 = Color32::from_rgb(248, 113, 113);

const SWATCHES: [(Rgb, &str); 14] = [
    (Rgb::new(255, 0, 0), "Red"),
    (Rgb::new(255, 80, 0), "Orange"),
    (Rgb::new(255, 200, 0), "Yellow"),
    (Rgb::new(120, 255, 0), "Lime"),
    (Rgb::new(0, 255, 60), "Green"),
    (Rgb::new(0, 255, 200), "Mint"),
    (Rgb::new(0, 200, 255), "Sky"),
    (Rgb::new(0, 60, 255), "Blue"),
    (Rgb::new(140, 60, 255), "Purple"),
    (Rgb::new(255, 0, 255), "Magenta"),
    (Rgb::new(255, 40, 120), "Pink"),
    (Rgb::new(255, 160, 60), "Warm white"),
    (Rgb::new(255, 255, 255), "White"),
    (Rgb::new(160, 200, 255), "Ice"),
];

/// Everything that lives as long as RGB Forge runs, across any number of windows:
/// the lighting engine, settings, tray icon and single-instance lock.
pub struct Core {
    engine: Engine,
    settings: Settings,
    sent_plan: crate::engine::Plan,
    dirty: Option<Instant>,
    tray: Option<crate::tray::Handle>,
    tray_rx: std::sync::mpsc::Receiver<crate::tray::TrayMsg>,
    /// Copy of the settings shared with the tray icon.
    shared: std::sync::Arc<std::sync::Mutex<Settings>>,
    mirrored: Settings,
    /// "Please open the window" requests (tray icon, or launching RGB Forge again).
    open_rx: std::sync::mpsc::Receiver<()>,
    /// The open window's context, if a window is open (so other threads can wake it).
    window: crate::tray::WindowSlot,
    _instance: Option<crate::single::Guard>,
}

impl Core {
    pub fn new(mut instance: Option<crate::single::Guard>) -> Self {
        let settings = config::load();
        let shared = std::sync::Arc::new(std::sync::Mutex::new(settings.clone()));
        let window = crate::tray::WindowSlot::default();
        let (tray_tx, tray_rx) = std::sync::mpsc::channel();
        let (open_tx, open_rx) = std::sync::mpsc::channel();
        if let Some(g) = instance.as_mut() {
            g.listen(open_tx.clone(), window.clone());
        }
        let engine = Engine::start(settings.plan.clone(), settings.demo, settings.gpu_enabled, settings.ram_enabled, settings.zone_sizes.clone());
        let tray = crate::tray::start(engine.sender(), tray_tx, open_tx, window.clone(), std::sync::Arc::clone(&shared));
        Self {
            engine,
            sent_plan: settings.plan.clone(),
            mirrored: settings.clone(),
            settings,
            dirty: None,
            tray,
            tray_rx,
            shared,
            open_rx,
            window,
            _instance: instance,
        }
    }

    pub fn has_tray(&self) -> bool {
        self.tray.is_some()
    }

    /// Block until something asks for the window. `false` if that can never happen.
    pub fn wait_for_open(&self) -> bool {
        self.open_rx.recv().is_ok()
    }

    /// Apply tray changes and keep the tray's copy of the settings current.
    fn sync(&mut self) {
        while let Ok(crate::tray::TrayMsg::SetPlan(p)) = self.tray_rx.try_recv() {
            self.settings.plan = p.clone();
            self.sent_plan = p; // the tray already sent it to the engine
        }
        if self.settings != self.mirrored {
            self.mirrored = self.settings.clone();
            if let Ok(mut s) = self.shared.lock() {
                *s = self.mirrored.clone();
            }
            if let Some(t) = &self.tray {
                t.update(|_| {}); // refresh the tray menu (saved looks, on/off label)
            }
        }
    }
}

impl Drop for Core {
    fn drop(&mut self) {
        let _ = config::save(&self.settings);
    }
}

/// One open window. It borrows the [`Core`], so closing it stops nothing.
pub struct App<'a> {
    core: &'a mut Core,
    status: Status,
    /// `None` = "All devices".
    selected: Option<String>,
    show_help: bool,
    show_welcome: bool,
    profile_name: String,
    hex: [String; 2],
    toast: Option<(String, Instant)>,
    autostart: bool,
    start_minimized: bool,
}

impl<'a> App<'a> {
    pub fn new(cc: &eframe::CreationContext<'_>, core: &'a mut Core, start_minimized: bool) -> Self {
        style(&cc.egui_ctx);
        core.window.set(Some(cc.egui_ctx.clone()));
        while core.open_rx.try_recv().is_ok() {} // we're opening now anyway
        Self {
            selected: core.settings.last_selected.clone(),
            show_welcome: !core.settings.seen_welcome && !start_minimized,
            core,
            status: Status { scanning: true, ..Status::default() },
            show_help: false,
            profile_name: String::new(),
            hex: [String::new(), String::new()],
            toast: None,
            autostart: config::autostart_enabled(),
            start_minimized,
        }
    }

    fn toast(&mut self, msg: impl Into<String>) {
        self.toast = Some((msg.into(), Instant::now()));
    }

    fn rescan(&mut self) {
        self.core.engine.send(Cmd::Rescan { demo: self.core.settings.demo, gpu: self.core.settings.gpu_enabled, ram: self.core.settings.ram_enabled, zone_sizes: self.core.settings.zone_sizes.clone() });
        self.toast("Looking for devices…");
    }

    fn save_now(&mut self) {
        if let Err(e) = config::save(&self.core.settings) {
            self.toast(format!("Couldn't save settings: {e}"));
        }
        self.core.dirty = None;
    }

    fn selected_device(&self) -> Option<&DeviceStatus> {
        let id = self.selected.as_ref()?;
        self.status.devices.iter().find(|d| &d.info.id == id)
    }

    /// The lighting the controls currently edit.
    fn lighting_mut(&mut self) -> &mut Lighting {
        let plan = &mut self.core.settings.plan;
        match (&self.selected, plan.sync_all) {
            (Some(id), false) => {
                let g = plan.global;
                plan.per_device.entry(id.clone()).or_insert(g)
            }
            _ => &mut plan.global,
        }
    }
}

impl Drop for App<'_> {
    fn drop(&mut self) {
        self.core.window.set(None);
        let _ = config::save(&self.core.settings);
        self.core.dirty = None;
    }
}

impl eframe::App for App<'_> {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        if self.core.open_rx.try_recv().is_ok() {
            ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(false));
            ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
        }
        if self.start_minimized {
            self.start_minimized = false;
            ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(true));
        }
        self.core.sync();
        self.status = self.core.engine.status();
        if self.core.settings.last_selected != self.selected {
            self.core.settings.last_selected = self.selected.clone();
            self.core.dirty.get_or_insert_with(Instant::now);
        }
        if let Some(id) = &self.selected
            && !self.status.scanning && !self.status.devices.iter().any(|d| &d.info.id == id) {
                self.selected = None;
            }

        self.top_bar(ctx);
        self.bottom_bar(ctx);
        self.sidebar(ctx);
        egui::CentralPanel::default()
            .frame(egui::Frame::new().fill(BG).inner_margin(Margin::symmetric(28, 20)))
            .show(ctx, |ui| {
                egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| self.main_view(ui));
            });
        self.help_window(ctx);
        self.welcome_window(ctx);

        if self.core.settings.plan != self.core.sent_plan {
            self.core.sent_plan = self.core.settings.plan.clone();
            self.core.engine.send(crate::engine::Cmd::Plan(self.core.sent_plan.clone()));
            self.core.dirty.get_or_insert_with(Instant::now);
        }
        if self.core.dirty.is_some_and(|d| d.elapsed() > Duration::from_millis(800)) {
            self.save_now();
        }
        ctx.request_repaint_after(Duration::from_millis(33));
    }
}

// ---------------------------------------------------------------------------
// Panels
// ---------------------------------------------------------------------------

impl App<'_> {
    fn top_bar(&mut self, ctx: &egui::Context) {
        egui::TopBottomPanel::top("top")
            .frame(egui::Frame::new().fill(PANEL).inner_margin(Margin::symmetric(20, 12)))
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    icon(ui, Icon::Logo, 26.0, ACCENT);
                    ui.label(RichText::new("RGB Forge").size(22.0).strong().color(TEXT));
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if ui.add(big_button("?  Help")).on_hover_text("How to use the app, and what to do if a device is missing").clicked() {
                            self.show_help = true;
                        }
                        if ui.add(big_button("⟳  Find devices")).on_hover_text("Search for RGB devices again").clicked() {
                            self.rescan();
                        }
                        ui.add_space(16.0);
                        let mut sync = self.core.settings.plan.sync_all;
                        if toggle(ui, &mut sync)
                            .on_hover_text("On: every device shows the same lighting.\nOff: give each device its own look.")
                            .changed()
                        {
                            self.core.settings.plan.sync_all = sync;
                            self.toast(if sync { "All devices now match" } else { "Pick a device on the left to give it its own look" });
                        }
                        ui.label(RichText::new("Sync all devices").color(TEXT));
                    });
                });
            });
    }

    fn bottom_bar(&mut self, ctx: &egui::Context) {
        egui::TopBottomPanel::bottom("bottom")
            .frame(egui::Frame::new().fill(PANEL).inner_margin(Margin::symmetric(20, 8)))
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    let s = &self.status;
                    let real = s.devices.iter().filter(|d| !d.info.demo).count();
                    let demo = s.devices.len().saturating_sub(real);
                    let text = if s.scanning {
                        "Looking for devices…".to_string()
                    } else if real == 0 {
                        format!("No RGB hardware found yet. Showing {demo} demo devices so you can try things out.")
                    } else {
                        format!("{real} device{} connected", if real == 1 { "" } else { "s" })
                    };
                    ui.label(RichText::new(text).color(DIM));
                    let mut open_help = false;
                    for p in s.problems.iter().chain(&s.audio_problem) {
                        ui.separator();
                        ui.label(RichText::new(format!("⚠ {p}")).color(WARN));
                        open_help |= ui.link("Fix this").clicked();
                    }
                    if open_help {
                        self.show_help = true;
                    }
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if let Some((msg, at)) = &self.toast
                            && at.elapsed() < Duration::from_secs(3) {
                                ui.label(RichText::new(msg).color(ACCENT));
                            }
                        if self.core.dirty.is_none() {
                            ui.label(RichText::new("✔ Saved").color(DIM)).on_hover_text("Your settings save automatically");
                        }
                    });
                });
            });
    }

    fn sidebar(&mut self, ctx: &egui::Context) {
        egui::SidePanel::left("devices")
            .exact_width(290.0)
            .resizable(false)
            .frame(egui::Frame::new().fill(PANEL).inner_margin(Margin::symmetric(14, 16)))
            .show(ctx, |ui| {
                egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
                    section_title(ui, "YOUR DEVICES");
                    let all_preview: Vec<Rgb> =
                        self.status.devices.iter().flat_map(|d| d.preview.iter().flatten().copied()).collect();
                    let sub = format!("{} devices", self.status.devices.len());
                    if device_card(ui, Icon::All, "All devices", &sub, &all_preview, self.selected.is_none(), false, None).clicked() {
                        self.selected = None;
                    }
                    ui.add_space(6.0);
                    let mut clicked = None;
                    for d in &self.status.devices {
                        let selected = self.selected.as_ref() == Some(&d.info.id);
                        let off = self.core.settings.plan.disabled.contains(&d.info.id);
                        let preview: Vec<Rgb> = d.preview.iter().flatten().copied().collect();
                        let badge = if d.error.is_some() {
                            Some(("!", ERR))
                        } else if d.info.demo {
                            Some(("DEMO", DIM))
                        } else {
                            None
                        };
                        let resp = device_card(ui, Icon::Kind(d.info.kind), &d.info.name, d.info.kind.label(), &preview, selected, off, badge);
                        if resp.clicked() {
                            clicked = Some(d.info.id.clone());
                        }
                        ui.add_space(6.0);
                    }
                    if clicked.is_some() {
                        self.selected = clicked;
                    }
                    if self.status.devices.is_empty() && !self.status.scanning {
                        ui.label(RichText::new("Nothing found. Click Help to see why.").color(DIM));
                    }

                    ui.add_space(18.0);
                    self.profiles(ui);
                    ui.add_space(18.0);
                    section_title(ui, "STARTUP");
                    ui.horizontal(|ui| {
                        let mut on = self.autostart;
                        let resp = toggle(ui, &mut on).on_hover_text(
                            "Opens RGB Forge minimized when you log in, so your lighting comes back after every restart.",
                        );
                        ui.label(RichText::new("Start with my computer").color(TEXT));
                        if resp.changed() {
                            match config::set_autostart(on) {
                                Ok(()) => {
                                    self.autostart = on;
                                    self.toast(if on { "Your lighting will come back every time you log in" } else { "Won't start at login" });
                                }
                                Err(e) => self.toast(format!("Couldn't change startup setting: {e}")),
                            }
                        }
                    });
                    ui.label(RichText::new(if self.core.has_tray() {
                        "Closing this window keeps RGB Forge running in the tray, so effects keep going. Right-click the tray icon → Quit to exit fully."
                    } else {
                        "Solid colours stay on after you close RGB Forge. Animated effects need it running (minimized is fine)."
                    }).small().color(DIM));
                });
            });
    }

    fn profiles(&mut self, ui: &mut Ui) {
        section_title(ui, "SAVED LOOKS");
        if self.core.settings.profiles.is_empty() {
            ui.label(RichText::new("Save your current lighting to switch back to it with one click.").color(DIM));
        }
        let mut load = None;
        let mut delete = None;
        for name in self.core.settings.profiles.keys() {
            ui.horizontal(|ui| {
                let w = ui.available_width() - 36.0;
                if ui.add_sized([w, 32.0], egui::Button::new(RichText::new(name).color(TEXT))).on_hover_text("Apply this look").clicked() {
                    load = Some(name.clone());
                }
                if ui.add_sized([30.0, 32.0], egui::Button::new("🗑")).on_hover_text("Delete").clicked() {
                    delete = Some(name.clone());
                }
            });
        }
        if let Some(n) = load
            && let Some(p) = self.core.settings.profiles.get(&n) {
                self.core.settings.plan = p.clone();
                self.toast(format!("Applied \"{n}\""));
            }
        if let Some(n) = delete {
            self.core.settings.profiles.remove(&n);
            self.core.dirty.get_or_insert_with(Instant::now);
            self.toast(format!("Deleted \"{n}\""));
        }
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            let w = ui.available_width() - 70.0;
            let edit = ui.add_sized(
                [w, 30.0],
                egui::TextEdit::singleline(&mut self.profile_name).hint_text("Name, e.g. Gaming").char_limit(40),
            );
            let enter = edit.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
            let name = self.profile_name.trim().to_string();
            let save = ui.add_enabled(!name.is_empty(), egui::Button::new("Save").min_size(Vec2::new(62.0, 30.0)));
            if (save.clicked() || enter) && !name.is_empty() {
                self.core.settings.profiles.insert(name.clone(), self.core.settings.plan.clone());
                self.profile_name.clear();
                self.core.dirty.get_or_insert_with(Instant::now);
                self.toast(format!("Saved \"{name}\""));
            }
        });
    }

    fn main_view(&mut self, ui: &mut Ui) {
        let device = self.selected_device().cloned();
        let sync = self.core.settings.plan.sync_all;

        // ---- Header ----
        match &device {
            Some(d) => {
                ui.horizontal(|ui| {
                    icon(ui, Icon::Kind(d.info.kind), 34.0, ACCENT);
                    ui.vertical(|ui| {
                        ui.label(RichText::new(&d.info.name).size(26.0).strong().color(TEXT));
                        ui.label(RichText::new(format!("{} · {} LEDs · {}", d.info.kind.label(), d.info.led_count(), d.info.detail)).color(DIM));
                    });
                });
                if d.info.experimental {
                    notice(ui, WARN, "This driver is new and hasn't been confirmed on this exact model yet. It only sends colour commands and never writes to the device's memory, so the worst case is that the lights don't change.");
                }
                if let Some(e) = &d.error {
                    notice(ui, ERR, &format!("Problem talking to this device: {e}"));
                }
            }
            None => {
                ui.label(RichText::new("All devices").size(26.0).strong().color(TEXT));
                ui.label(RichText::new(if sync {
                    "Everything below applies to every device at once."
                } else {
                    "Default look for any device you haven't customised. Click a device on the left to give it its own look."
                }).color(DIM));
            }
        }
        ui.add_space(10.0);

        if let Some(d) = &device {
            let id = d.info.id.clone();
            let mut on = !self.core.settings.plan.disabled.contains(&id);
            ui.horizontal(|ui| {
                if toggle(ui, &mut on).changed() {
                    if on {
                        self.core.settings.plan.disabled.remove(&id);
                    } else {
                        self.core.settings.plan.disabled.insert(id.clone());
                    }
                }
                ui.label(RichText::new(if on { "RGB Forge controls this device" } else { "Not controlled: the device keeps whatever it was showing" }).color(TEXT));
            });
            ui.add_space(6.0);
            if sync {
                egui::Frame::new().fill(CARD).corner_radius(10).inner_margin(12).show(ui, |ui| {
                    ui.horizontal_wrapped(|ui| {
                        ui.label(RichText::new("🔗 Sync is on, so this device matches all the others.").color(TEXT));
                        if ui.add(accent_button("Give this device its own look")).clicked() {
                            self.core.settings.plan.sync_all = false;
                        }
                    });
                });
            } else if self.core.settings.plan.per_device.contains_key(&id)
                && ui.button("↺ Use the default look instead").clicked() {
                    self.core.settings.plan.per_device.remove(&id);
                }
            ui.add_space(8.0);
        }

        let t = ui.input(|i| i.time) as f32;
        let lighting = *self.lighting_mut();

        // ---- Effects ----
        let mut step = 1;
        section_title(ui, "1 · PICK AN EFFECT");
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing = Vec2::splat(10.0);
            for e in Effect::ALL {
                let preview = Lighting { effect: e, ..lighting };
                if effect_card(ui, e, &preview, lighting.effect == e, t, self.status.audio_level).clicked() {
                    self.lighting_mut().effect = e;
                }
            }
        });
        ui.add_space(16.0);

        // ---- Colours ----
        if lighting.effect.uses_color() {
            step += 1;
            section_title(ui, &format!("{step} · {}", if lighting.effect.uses_second_color() { "PICK TWO COLOURS" } else { "PICK A COLOUR" }));
            ui.horizontal_top(|ui| {
                let mut c = self.lighting_mut().color;
                if color_column(ui, "Colour", &mut c, &mut self.hex[0]) {
                    self.lighting_mut().color = c;
                }
                if lighting.effect.uses_second_color() {
                    ui.add_space(24.0);
                    let mut c2 = self.lighting_mut().color2;
                    if color_column(ui, "Second colour", &mut c2, &mut self.hex[1]) {
                        self.lighting_mut().color2 = c2;
                    }
                }
            });
            ui.add_space(16.0);
        }

        // ---- Sliders ----
        if lighting.effect != Effect::Off {
            step += 1;
            section_title(ui, &format!("{step} · FINE-TUNE"));
            let l = self.lighting_mut();
            let mut pct = (l.brightness * 100.0).round();
            labeled_slider(ui, "Brightness", &mut pct, 0.0..=100.0, "%", "Dim", "Bright");
            l.brightness = pct / 100.0;
            if l.effect.uses_speed() {
                labeled_slider(ui, "Speed", &mut l.speed, 1.0..=10.0, "", "Slow", "Fast");
            }
            ui.add_space(16.0);
        }

        // ---- Preview ----
        section_title(ui, "LIVE PREVIEW");
        egui::Frame::new().fill(CARD).corner_radius(12).inner_margin(16).show(ui, |ui| {
            ui.set_width(ui.available_width());
            let devices: Vec<DeviceStatus> = match &device {
                Some(d) => vec![d.clone()],
                None => self.status.devices.clone(),
            };
            if devices.is_empty() {
                ui.label(RichText::new("No devices to show.").color(DIM));
            }
            for d in &devices {
                if device.is_none() {
                    ui.horizontal(|ui| {
                        icon(ui, Icon::Kind(d.info.kind), 18.0, ACCENT);
                        ui.label(RichText::new(&d.info.name).color(TEXT).strong());
                    });
                }
                for (z, colors) in d.info.zones.iter().zip(&d.preview) {
                    draw_zone(ui, &z.name, colors);
                }
                ui.add_space(6.0);
            }
        });

        // ---- LED setup ----
        if let Some(d) = &device
            && d.info.zones.iter().any(|z| z.resizable) {
                ui.add_space(16.0);
                section_title(ui, "LED SETUP");
                if d.info.zones.iter().any(|z| z.resizable && z.leds == 0) {
                    notice(ui, WARN, "A connector below is set to 0 LEDs, so anything plugged into it stays dark. Set it to your fans' LED total (or 40 if unsure).");
                }
                ui.label(RichText::new("Fans and strips plugged into this controller can't report how many LEDs they have. Enter the total for each connector (check the box or the product page; e.g. 3 fans × 8 LEDs = 24).").color(DIM));
                ui.add_space(6.0);
                for (zi, z) in d.info.zones.iter().enumerate() {
                    if !z.resizable {
                        continue;
                    }
                    let mut n = z.leds;
                    ui.horizontal(|ui| {
                        ui.add_sized([150.0, 26.0], egui::Label::new(RichText::new(&z.name).color(TEXT)));
                        let mut changed = ui.add(egui::DragValue::new(&mut n).range(0..=z.max_leds).suffix(" LEDs").speed(0.3)).changed();
                        for (label, add) in [("−8", -8i32), ("+8", 8)] {
                            if ui.button(label).clicked() {
                                n = u16::try_from((i32::from(n) + add).clamp(0, i32::from(z.max_leds))).unwrap_or(0);
                                changed = true;
                            }
                        }
                        if changed {
                            let sizes = self.core.settings.zone_sizes.entry(d.info.id.clone()).or_default();
                            if sizes.len() <= zi {
                                sizes.resize(zi.saturating_add(1), 0);
                            }
                            if let Some(slot) = sizes.get_mut(zi) {
                                *slot = n;
                            }
                            self.core.engine.send(Cmd::Resize { id: d.info.id.clone(), zone: zi, leds: n });
                            self.core.dirty.get_or_insert_with(Instant::now);
                        }
                    });
                }
            }
    }

    fn device_report(&self) -> String {
        let mut r = format!("RGB Forge {} device report\n", env!("CARGO_PKG_VERSION"));
        for d in &self.status.devices {
            r.push_str(&format!("- {} [{}]{}\n  {}\n", d.info.name, d.info.id, if d.info.demo { " (demo)" } else { "" }, d.info.detail));
            for z in &d.info.zones {
                r.push_str(&format!("  zone '{}': {} LEDs{}\n", z.name, z.leds, if z.resizable { " (user-set)" } else { "" }));
            }
            if let Some(e) = &d.error {
                r.push_str(&format!("  error: {e}\n"));
            }
        }
        for p in &self.status.problems {
            r.push_str(&format!("! {p}\n"));
        }
        let other = drivers::unsupported_usb();
        if !other.is_empty() {
            r.push_str("USB devices without a driver yet (IDs only, for driver requests):\n");
            for line in other {
                r.push_str(&format!("  {line}\n"));
            }
        }
        r
    }

    fn welcome_window(&mut self, ctx: &egui::Context) {
        if !self.show_welcome {
            return;
        }
        let mut close = false;
        egui::Window::new(RichText::new("Welcome to RGB Forge").size(20.0).strong())
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, Vec2::ZERO)
            .default_width(460.0)
            .show(ctx, |ui| {
                ui.label("Controlling your lights takes two clicks:");
                ui.add_space(6.0);
                for step in [
                    "1.  Pick an effect (Solid, Rainbow Wave, Breathe…)",
                    "2.  Pick a colour",
                    "That's it. Everything saves automatically and comes back next time.",
                ] {
                    ui.label(RichText::new(step).color(TEXT));
                }
                ui.add_space(8.0);
                ui.label(RichText::new("Want different lighting per part? Switch off \"Sync all devices\" at the top, then click a device on the left.").color(DIM));
                ui.label(RichText::new("If your hardware isn't listed, demo devices are shown instead. Help explains why.").color(DIM));
                ui.add_space(10.0);
                if ui.add(accent_button("Let's go")).clicked() {
                    close = true;
                }
            });
        if close {
            self.show_welcome = false;
            self.core.settings.seen_welcome = true;
            self.save_now();
        }
    }

    fn help_window(&mut self, ctx: &egui::Context) {
        let mut open = self.show_help;
        let mut rescan = false;
        egui::Window::new(RichText::new("Help").size(20.0).strong())
            .open(&mut open)
            .default_width(600.0)
            .default_height(560.0)
            .show(ctx, |ui| {
                egui::ScrollArea::vertical().show(ui, |ui| {
                    egui::CollapsingHeader::new(RichText::new("My device isn't showing up").strong()).default_open(true).show(ui, |ui| {
                        ui.label("Linux only lets normal apps use USB lighting controllers after a one-time permission setup. Copy this into a terminal, then click \"Find devices\":");
                        let cmd = "rgbforge --print-udev-rules | sudo tee /etc/udev/rules.d/60-rgbforge.rules && sudo udevadm control --reload-rules && sudo udevadm trigger";
                        egui::Frame::new().fill(BG).corner_radius(8).inner_margin(10).show(ui, |ui| {
                            ui.add(egui::Label::new(RichText::new(cmd).monospace().color(TEXT)).wrap());
                        });
                        ui.horizontal(|ui| {
                            if ui.add(accent_button("Copy command")).clicked() {
                                ui.ctx().copy_text(cmd.to_string());
                                self.toast("Command copied");
                            }
                            if ui.button("⟳ Find devices").clicked() {
                                rescan = true;
                            }
                        });
                        ui.add_space(4.0);
                        ui.label(RichText::new("This only grants access to the exact controllers listed below, nothing else on your system.").color(DIM));
                    });
                    egui::CollapsingHeader::new(RichText::new("Report a problem").strong()).show(ui, |ui| {
                        ui.label("Lights not doing what the preview shows? Copy this report and send it to whoever is helping you. It lists what RGB Forge detected (no personal data).");
                        if ui.add(accent_button("Copy device report")).clicked() {
                            ui.ctx().copy_text(self.device_report());
                            self.toast("Device report copied");
                        }
                    });
                    egui::CollapsingHeader::new(RichText::new("What hardware works").strong()).default_open(true).show(ui, |ui| {
                        let mut seen = Vec::new();
                        for s in drivers::SUPPORTED {
                            if !seen.contains(&s.name) {
                                seen.push(s.name);
                                ui.label(format!("•  {}", s.name));
                            }
                        }
                        ui.add_space(4.0);
                        ui.label(RichText::new("Anything plugged into these (ARGB fans, AIO pump/fans, LED strips) works through them.").color(DIM));
                        ui.add_space(4.0);
                        ui.label(RichText::new("RAM and graphics cards").strong());
                        ui.label(RichText::new("RAM and most graphics cards are controlled over the motherboard's SMBus, a system bus that also carries the chip that tells your PC how to run your memory. Sending the wrong bytes there can permanently break a RAM stick or board, so RGB Forge only adds a RAM or GPU driver once it has been written and checked for that exact model. They aren't included yet.").color(DIM));
                    });
                    egui::CollapsingHeader::new(RichText::new("Graphics card (Sapphire Nitro+ RX 9070 XT)").strong()).show(ui, |ui| {
                        ui.label("RGB Forge can control this card's lighting directly. It's off until you switch it on, because:");
                        for line in [
                            "Freezes other owners saw came from older RGB software poking the card's power-management (SMU) bus. RGB Forge never touches that bus, only the card's dedicated RGB bus, the same fix OpenRGB made. It's still new, so save your work before switching this on. If anything goes wrong, hold the power button and switch this off again.",
                            "Experimental: written from published information about this exact card, not yet tested on one. Needs Linux 6.17 or newer.",
                        ] {
                            ui.label(RichText::new(format!("•  {line}")).color(WARN));
                        }
                        ui.label(RichText::new("What it does: talks only to this exact card model, only on its dedicated RGB bus, only to the lighting chip, and only sends colour commands. Tip: first run `rgbforge --gpu-check` in a terminal; it checks everything without touching the card.").color(DIM));
                        ui.horizontal(|ui| {
                            let mut on = self.core.settings.gpu_enabled;
                            if toggle(ui, &mut on).changed() {
                                self.core.settings.gpu_enabled = on;
                                self.save_now();
                                rescan = true;
                            }
                            ui.label(RichText::new("Control my graphics card's lighting").color(TEXT));
                        });
                    });
                    egui::CollapsingHeader::new(RichText::new("Memory (G.Skill Trident Z5 RGB)").strong()).show(ui, |ui| {
                        ui.label("RGB Forge can control your RAM's lighting. It's off until you switch it on, and it refuses to run unless every safety check passes:");
                        for line in [
                            "Your chipset's \"SPD Write Disable\" lock must be on. It's a hardware lock that blocks every write to the chips your PC reads to run its memory.",
                            "OpenRGB must not be running.",
                            "It only talks to the RAM's RGB chips (0x70-0x77), only after they identify themselves, and it can't send the save or address-change commands.",
                        ] {
                            ui.label(RichText::new(format!("•  {line}")).color(DIM));
                        }
                        ui.label(RichText::new("Experimental: not yet tested on real RAM. First run `rgbforge --ram-check` in a terminal. It only reads.").color(WARN));
                        ui.horizontal(|ui| {
                            let mut on = self.core.settings.ram_enabled;
                            if toggle(ui, &mut on).changed() {
                                self.core.settings.ram_enabled = on;
                                self.save_now();
                                rescan = true;
                            }
                            ui.label(RichText::new("Control my RAM's lighting").color(TEXT));
                        });
                    });
                    egui::CollapsingHeader::new(RichText::new("Safety").strong()).show(ui, |ui| {
                        for line in [
                            "Only talks to devices on a fixed allow-list of USB IDs; everything else is ignored.",
                            "Only sends colour commands. Never writes firmware or device flash.",
                            "Doesn't need to run as root.",
                            "No strobe or flash effects (seizure risk). All effects change smoothly.",
                            "If a device keeps failing, RGB Forge stops talking to it until you rescan.",
                            "Written in Rust with no `unsafe` code.",
                        ] {
                            ui.label(format!("✔  {line}"));
                        }
                    });
                    egui::CollapsingHeader::new(RichText::new("Demo devices").strong()).show(ui, |ui| {
                        ui.label("Demo devices let you try effects with no hardware. They don't control anything.");
                        let mut demo = self.core.settings.demo;
                        egui::ComboBox::from_label("Show demo devices")
                            .selected_text(demo_label(demo))
                            .show_ui(ui, |ui| {
                                for m in [DemoMode::Auto, DemoMode::Always, DemoMode::Never] {
                                    ui.selectable_value(&mut demo, m, demo_label(m));
                                }
                            });
                        if demo != self.core.settings.demo {
                            self.core.settings.demo = demo;
                            self.core.dirty.get_or_insert_with(Instant::now);
                            rescan = true;
                        }
                    });
                });
            });
        self.show_help = open;
        if rescan {
            self.rescan();
        }
    }
}

fn demo_label(m: DemoMode) -> &'static str {
    match m {
        DemoMode::Auto => "Only when no real hardware is found",
        DemoMode::Always => "Always",
        DemoMode::Never => "Never",
    }
}

// ---------------------------------------------------------------------------
// Widgets
// ---------------------------------------------------------------------------

fn c32(c: Rgb) -> Color32 {
    Color32::from_rgb(c.r, c.g, c.b)
}

fn style(ctx: &egui::Context) {
    let mut v = egui::Visuals::dark();
    v.panel_fill = PANEL;
    v.window_fill = PANEL;
    v.extreme_bg_color = BG;
    v.selection.bg_fill = ACCENT;
    v.hyperlink_color = ACCENT;
    v.window_corner_radius = CornerRadius::same(14);
    for w in [&mut v.widgets.inactive, &mut v.widgets.hovered, &mut v.widgets.active] {
        w.corner_radius = CornerRadius::same(8);
    }
    v.widgets.inactive.weak_bg_fill = CARD;
    v.widgets.inactive.bg_fill = CARD;
    v.widgets.hovered.weak_bg_fill = CARD_HOVER;
    v.widgets.hovered.bg_fill = CARD_HOVER;
    ctx.set_visuals(v);
    ctx.style_mut(|s| {
        s.text_styles.insert(TextStyle::Body, FontId::proportional(15.0));
        s.text_styles.insert(TextStyle::Button, FontId::proportional(15.0));
        s.text_styles.insert(TextStyle::Small, FontId::proportional(12.0));
        s.text_styles.insert(TextStyle::Monospace, FontId::monospace(13.0));
        s.spacing.button_padding = Vec2::new(12.0, 6.0);
        s.spacing.item_spacing = Vec2::new(8.0, 8.0);
        s.spacing.slider_width = 320.0;
    });
}

fn big_button(text: &str) -> egui::Button<'_> {
    egui::Button::new(RichText::new(text).color(TEXT)).min_size(Vec2::new(0.0, 32.0))
}

fn accent_button(text: &str) -> egui::Button<'_> {
    egui::Button::new(RichText::new(text).color(Color32::WHITE).strong()).fill(ACCENT).min_size(Vec2::new(0.0, 32.0))
}

fn section_title(ui: &mut Ui, text: &str) {
    ui.label(RichText::new(text).size(12.5).strong().color(DIM));
    ui.add_space(2.0);
}

fn notice(ui: &mut Ui, color: Color32, text: &str) {
    egui::Frame::new()
        .fill(color.gamma_multiply(0.12))
        .stroke(Stroke::new(1.0_f32, color.gamma_multiply(0.5)))
        .corner_radius(8)
        .inner_margin(10)
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.label(RichText::new(text).color(color));
        });
}

/// An iOS-style on/off switch.
fn toggle(ui: &mut Ui, on: &mut bool) -> egui::Response {
    let size = Vec2::new(44.0, 24.0);
    let (rect, mut resp) = ui.allocate_exact_size(size, Sense::click());
    if resp.clicked() {
        *on = !*on;
        resp.mark_changed();
    }
    let k = ui.ctx().animate_bool_responsive(resp.id, *on);
    let track = if *on { ACCENT } else { Color32::from_rgb(70, 75, 90) };
    let p = ui.painter();
    p.rect_filled(rect, CornerRadius::same(12), track);
    let x = egui::lerp((rect.left() + 12.0)..=(rect.right() - 12.0), k);
    p.circle_filled(Pos2::new(x, rect.center().y), 9.0, Color32::WHITE);
    resp
}

#[allow(clippy::too_many_arguments)]
fn device_card(
    ui: &mut Ui,
    icon: Icon,
    title: &str,
    subtitle: &str,
    preview: &[Rgb],
    selected: bool,
    off: bool,
    badge: Option<(&str, Color32)>,
) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 68.0), Sense::click());
    let p = ui.painter();
    let bg = if selected { CARD_HOVER } else if resp.hovered() { Color32::from_rgb(33, 38, 50) } else { CARD };
    p.rect_filled(rect, CornerRadius::same(12), bg);
    if selected {
        p.rect_stroke(rect, CornerRadius::same(12), Stroke::new(2.0_f32, ACCENT), StrokeKind::Inside);
    }
    let dim = if off { 0.45 } else { 1.0 };
    paint_icon(p, Pos2::new(rect.left() + 26.0, rect.center().y - 4.0), 26.0, icon, ACCENT.gamma_multiply(dim));
    let title_w = rect.width() - 64.0 - if badge.is_some() { 44.0 } else { 0.0 };
    let mut job = egui::text::LayoutJob::simple_singleline(title.to_string(), FontId::proportional(15.0), TEXT.gamma_multiply(dim));
    job.wrap = egui::text::TextWrapping::truncate_at_width(title_w);
    let galley = p.layout_job(job);
    p.galley(Pos2::new(rect.left() + 50.0, rect.top() + 10.0), galley, TEXT);
    let sub = if off { format!("{subtitle} · not controlled") } else { subtitle.to_string() };
    p.text(Pos2::new(rect.left() + 50.0, rect.top() + 32.0), egui::Align2::LEFT_TOP, sub, FontId::proportional(12.0), DIM);
    if let Some((text, color)) = badge {
        p.text(Pos2::new(rect.right() - 12.0, rect.top() + 12.0), egui::Align2::RIGHT_TOP, text, FontId::proportional(10.5), color);
    }
    // Mini LED bar.
    let bar = Rect::from_min_max(Pos2::new(rect.left() + 50.0, rect.bottom() - 12.0), Pos2::new(rect.right() - 14.0, rect.bottom() - 7.0));
    if preview.is_empty() {
        p.rect_filled(bar, CornerRadius::same(2), Color32::from_rgb(45, 50, 62));
    } else {
        let n = preview.len().min(48);
        let step = preview.len() as f32 / n as f32;
        let w = bar.width() / n as f32;
        for i in 0..n {
            let c = preview.get((i as f32 * step) as usize).copied().unwrap_or_default();
            let r = Rect::from_min_size(Pos2::new(bar.left() + i as f32 * w, bar.top()), Vec2::new(w + 0.5, bar.height()));
            p.rect_filled(r, CornerRadius::ZERO, c32(c).gamma_multiply(dim));
        }
    }
    resp.on_hover_cursor(egui::CursorIcon::PointingHand)
}

fn effect_card(ui: &mut Ui, effect: Effect, lighting: &Lighting, selected: bool, t: f32, audio: f32) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(Vec2::new(150.0, 86.0), Sense::click());
    let p = ui.painter();
    let bg = if selected { CARD_HOVER } else if resp.hovered() { Color32::from_rgb(33, 38, 50) } else { CARD };
    p.rect_filled(rect, CornerRadius::same(12), bg);
    if selected {
        p.rect_stroke(rect, CornerRadius::same(12), Stroke::new(2.0_f32, ACCENT), StrokeKind::Inside);
    }
    p.text(Pos2::new(rect.left() + 12.0, rect.top() + 10.0), egui::Align2::LEFT_TOP, effect.label(), FontId::proportional(15.0), TEXT);
    let mut buf = Vec::new();
    let preview = Lighting { brightness: lighting.brightness.max(0.6), ..*lighting };
    effects::render(&preview, t, 0.0, audio, 10, &mut buf);
    for (i, c) in buf.iter().enumerate() {
        let center = Pos2::new(rect.left() + 18.0 + i as f32 * 12.6, rect.bottom() - 22.0);
        p.circle_filled(center, 8.0, c32(*c).gamma_multiply(0.25));
        p.circle_filled(center, 4.5, c32(*c));
    }
    resp.on_hover_text(effect.description()).on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// Colour wheel + swatches + hex box. Returns true if the colour changed.
fn color_column(ui: &mut Ui, title: &str, color: &mut Rgb, hex: &mut String) -> bool {
    let mut changed = false;
    ui.vertical(|ui| {
        ui.set_max_width(330.0);
        ui.label(RichText::new(title).color(TEXT).strong());
        changed |= hsv_picker(ui, color);
        ui.add_space(4.0);
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing = Vec2::splat(6.0);
            for (s, name) in SWATCHES {
                let (rect, resp) = ui.allocate_exact_size(Vec2::splat(30.0), Sense::click());
                let p = ui.painter();
                p.circle_filled(rect.center(), 13.0, c32(s));
                if *color == s {
                    p.circle_stroke(rect.center(), 15.0, Stroke::new(2.0_f32, Color32::WHITE));
                } else if resp.hovered() {
                    p.circle_stroke(rect.center(), 15.0, Stroke::new(1.5_f32, DIM));
                }
                if resp.on_hover_text(name).on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
                    *color = s;
                    changed = true;
                }
            }
        });
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            ui.label(RichText::new("Hex").color(DIM));
            let editing = ui.memory(|m| m.focused()) == Some(ui.next_auto_id());
            if !editing {
                *hex = color.hex();
            }
            let r = ui.add(egui::TextEdit::singleline(hex).background_color(CARD).desired_width(90.0).char_limit(7).font(TextStyle::Monospace));
            if r.changed()
                && let Some(c) = Rgb::from_hex(hex) {
                    *color = c;
                    changed = true;
                }
            if r.lost_focus() {
                *hex = color.hex();
            }
        });
    });
    changed
}

fn labeled_slider(
    ui: &mut Ui,
    label: &str,
    value: &mut f32,
    range: std::ops::RangeInclusive<f32>,
    suffix: &str,
    low: &str,
    high: &str,
) {
    ui.horizontal(|ui| {
        ui.add_sized([90.0, 24.0], egui::Label::new(RichText::new(label).color(TEXT)));
        ui.label(RichText::new(low).color(DIM).small());
        ui.add(egui::Slider::new(value, range).show_value(false));
        ui.label(RichText::new(high).color(DIM).small());
        ui.label(RichText::new(format!("{:.0}{suffix}", *value)).color(TEXT).strong());
    });
}

fn draw_zone(ui: &mut Ui, name: &str, colors: &[Rgb]) {
    ui.label(RichText::new(name).small().color(DIM));
    if colors.is_empty() {
        ui.label(RichText::new("No LEDs set up yet: enter how many are connected under LED SETUP.").color(WARN).small());
        return;
    }
    let width = ui.available_width().max(50.0);
    let step = (width / colors.len() as f32).clamp(9.0, 24.0);
    let per_row = ((width / step).floor() as usize).max(1);
    let rows = colors.len().div_ceil(per_row);
    let (rect, _) = ui.allocate_exact_size(Vec2::new(width, rows as f32 * step), Sense::hover());
    let radius = (step * 0.33).clamp(3.0, 7.0);
    let p = ui.painter();
    for (i, c) in colors.iter().enumerate() {
        let (row, col) = (i / per_row, i % per_row);
        let center = Pos2::new(rect.left() + (col as f32 + 0.5) * step, rect.top() + (row as f32 + 0.5) * step);
        p.circle_filled(center, radius * 1.9, c32(*c).gamma_multiply(0.18));
        p.circle_filled(center, radius, c32(*c));
        p.circle_stroke(center, radius, Stroke::new(1.0_f32, Color32::from_white_alpha(18)));
    }
    ui.add_space(4.0);
}

#[derive(Clone, Copy)]
enum Icon {
    Logo,
    All,
    Kind(Kind),
}

fn icon(ui: &mut Ui, which: Icon, size: f32, color: Color32) {
    let (rect, _) = ui.allocate_exact_size(Vec2::splat(size), Sense::hover());
    paint_icon(ui.painter(), rect.center(), size, which, color);
}

/// Simple vector icons, so nothing depends on which glyphs the font has.
fn paint_icon(p: &egui::Painter, c: Pos2, size: f32, which: Icon, color: Color32) {
    let s = size;
    let line = Stroke::new(s * 0.07, color);
    let at = |dx: f32, dy: f32| Pos2::new(c.x + dx * s, c.y + dy * s);
    let rect = |dx: f32, dy: f32, w: f32, h: f32| Rect::from_center_size(at(dx, dy), Vec2::new(w * s, h * s));
    let diamond = |r: f32| vec![at(0.0, -r), at(r, 0.0), at(0.0, r), at(-r, 0.0)];
    match which {
        Icon::Logo => {
            p.add(egui::Shape::convex_polygon(diamond(0.46), color, Stroke::NONE));
            p.add(egui::Shape::convex_polygon(diamond(0.22), Color32::from_rgb(0, 200, 255), Stroke::NONE));
        }
        Icon::All => {
            for (dx, dy, col) in [(-0.2, -0.2, color), (0.2, -0.2, Color32::from_rgb(0, 200, 255)), (-0.2, 0.2, Color32::from_rgb(255, 60, 140)), (0.2, 0.2, Color32::from_rgb(255, 190, 0))] {
                p.rect_filled(rect(dx, dy, 0.32, 0.32), CornerRadius::same(2), col);
            }
        }
        Icon::Kind(Kind::Ram) => {
            p.rect_stroke(rect(0.0, -0.05, 0.9, 0.42), CornerRadius::same(2), line, StrokeKind::Middle);
            for dx in [-0.25, 0.0, 0.25] {
                p.rect_filled(rect(dx, -0.05, 0.14, 0.18), CornerRadius::ZERO, color);
            }
            for i in 0..6 {
                let x = -0.3 + i as f32 * 0.12;
                p.line_segment([at(x, 0.18), at(x, 0.3)], line);
            }
        }
        Icon::Kind(Kind::Gpu) => {
            p.rect_stroke(rect(0.0, 0.0, 0.9, 0.56), CornerRadius::same(3), line, StrokeKind::Middle);
            p.circle_stroke(at(0.12, 0.0), 0.18 * s, line);
            p.circle_filled(at(0.12, 0.0), 0.05 * s, color);
            p.rect_filled(rect(-0.28, 0.0, 0.12, 0.3), CornerRadius::ZERO, color);
        }
        Icon::Kind(Kind::Cooler) => {
            p.circle_stroke(c, 0.42 * s, line);
            p.circle_stroke(c, 0.24 * s, line);
            p.circle_filled(c, 0.09 * s, color);
        }
        Icon::Kind(Kind::Fans) => {
            p.circle_stroke(c, 0.44 * s, line);
            for k in 0..3 {
                let a = k as f32 * std::f32::consts::TAU / 3.0;
                p.circle_filled(at(0.2 * a.cos(), 0.2 * a.sin()), 0.13 * s, color);
            }
            p.circle_filled(c, 0.06 * s, color);
        }
        Icon::Kind(Kind::Motherboard) => {
            p.rect_stroke(rect(0.0, 0.0, 0.8, 0.8), CornerRadius::same(2), line, StrokeKind::Middle);
            p.rect_filled(rect(0.08, -0.08, 0.3, 0.3), CornerRadius::ZERO, color);
            p.line_segment([at(-0.25, 0.22), at(0.25, 0.22)], line);
        }
        Icon::Kind(Kind::Strip) => {
            p.rect_stroke(rect(0.0, 0.0, 0.92, 0.22), CornerRadius::same(4), line, StrokeKind::Middle);
            for dx in [-0.3, -0.1, 0.1, 0.3] {
                p.circle_filled(at(dx, 0.0), 0.05 * s, color);
            }
        }
        Icon::Kind(Kind::Other) => {
            p.add(egui::Shape::convex_polygon(diamond(0.4), Color32::TRANSPARENT, line));
        }
    }
}

/// A friendly colour picker: rainbow bar for the hue, a box for how vivid/bright.
fn hsv_picker(ui: &mut Ui, color: &mut Rgb) -> bool {
    let id = ui.next_auto_id();
    // Remember hue separately so it survives dragging to grey/black.
    let (h0, s0, v0) = effects::to_hsv(*color);
    let remembered: Option<(f32, Rgb)> = ui.data(|d| d.get_temp(id));
    let mut h = match remembered {
        Some((h, c)) if c == *color => h,
        _ => h0,
    };
    let (mut s, mut v) = (s0, v0);
    let width = 300.0;
    let mut changed = false;

    // Hue bar.
    let (bar, resp) = ui.allocate_exact_size(Vec2::new(width, 22.0), Sense::click_and_drag());
    let p = ui.painter();
    let mut mesh = egui::Mesh::default();
    let steps = 24;
    for i in 0..=steps {
        let t = i as f32 / steps as f32;
        let col = c32(effects::hsv(t * 360.0, 1.0, 1.0));
        mesh.colored_vertex(Pos2::new(bar.left() + t * bar.width(), bar.top()), col);
        mesh.colored_vertex(Pos2::new(bar.left() + t * bar.width(), bar.bottom()), col);
        if i > 0 {
            let k = u32::try_from(i * 2).unwrap_or(0);
            mesh.add_triangle(k - 2, k - 1, k);
            mesh.add_triangle(k - 1, k, k + 1);
        }
    }
    p.add(egui::Shape::mesh(mesh));
    if let Some(pos) = resp.interact_pointer_pos() {
        h = ((pos.x - bar.left()) / bar.width()).clamp(0.0, 0.9999) * 360.0;
        if s < 0.05 && v > 0.0 {
            s = 1.0; // picking a hue on a grey colour should show that hue
        }
        changed = true;
    }
    let hx = bar.left() + h / 360.0 * bar.width();
    p.circle_filled(Pos2::new(hx, bar.center().y), 10.0, c32(effects::hsv(h, 1.0, 1.0)));
    p.circle_stroke(Pos2::new(hx, bar.center().y), 10.0, Stroke::new(2.5_f32, Color32::WHITE));
    resp.on_hover_text("Drag to choose the colour").on_hover_cursor(egui::CursorIcon::PointingHand);

    ui.add_space(6.0);
    // Shade box: left→right more vivid, bottom→top brighter.
    let (sq, resp) = ui.allocate_exact_size(Vec2::new(width, 130.0), Sense::click_and_drag());
    let p = ui.painter();
    let hue = c32(effects::hsv(h, 1.0, 1.0));
    let mut mesh = egui::Mesh::default();
    for (pos, col) in [(sq.left_top(), Color32::WHITE), (sq.right_top(), hue), (sq.right_bottom(), hue), (sq.left_bottom(), Color32::WHITE)] {
        mesh.colored_vertex(pos, col);
    }
    for (pos, col) in [(sq.left_top(), Color32::TRANSPARENT), (sq.right_top(), Color32::TRANSPARENT), (sq.right_bottom(), Color32::BLACK), (sq.left_bottom(), Color32::BLACK)] {
        mesh.colored_vertex(pos, col);
    }
    for base in [0u32, 4] {
        mesh.add_triangle(base, base + 1, base + 2);
        mesh.add_triangle(base, base + 2, base + 3);
    }
    p.add(egui::Shape::mesh(mesh));
    if let Some(pos) = resp.interact_pointer_pos() {
        s = ((pos.x - sq.left()) / sq.width()).clamp(0.0, 1.0);
        v = 1.0 - ((pos.y - sq.top()) / sq.height()).clamp(0.0, 1.0);
        changed = true;
    }
    let knob = Pos2::new(sq.left() + s * sq.width(), sq.top() + (1.0 - v) * sq.height());
    p.circle_filled(knob, 9.0, c32(effects::hsv(h, s, v)));
    p.circle_stroke(knob, 9.0, Stroke::new(2.5_f32, Color32::WHITE));
    resp.on_hover_text("Left/right: pale to vivid · Up/down: dark to bright").on_hover_cursor(egui::CursorIcon::Crosshair);

    if changed {
        *color = effects::hsv(h, s, v);
    }
    ui.data_mut(|d| d.insert_temp(id, (h, *color)));
    changed
}

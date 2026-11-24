//! System-tray icon (KDE Plasma, and any desktop supporting StatusNotifierItem).
//!
//! Menu actions go straight to the lighting engine, so they work instantly even while the
//! window is minimized. The window is told afterwards so it shows the same thing.

use std::sync::mpsc::Sender;
use std::sync::{Arc, Mutex};

use eframe::egui;
use ksni::blocking::TrayMethods;
use ksni::menu::{StandardItem, SubMenu};
use ksni::{Icon, MenuItem};

use crate::config::{self, Settings};
use crate::effects::Effect;
use crate::engine::{Cmd, Plan};

/// The currently open window's egui context, if any. Lets background threads wake it.
#[derive(Clone, Default)]
pub struct WindowSlot(Arc<Mutex<Option<egui::Context>>>);

impl WindowSlot {
    pub fn set(&self, ctx: Option<egui::Context>) {
        *self.0.lock().unwrap_or_else(|p| p.into_inner()) = ctx;
    }

    /// Wake the open window (if there is one) so it notices new messages.
    pub fn wake(&self) {
        if let Some(ctx) = self.0.lock().unwrap_or_else(|p| p.into_inner()).as_ref() {
            ctx.request_repaint();
        }
    }
}

pub enum TrayMsg {
    /// The tray changed the lighting; the window should adopt this plan.
    SetPlan(Plan),
}

pub struct Tray {
    engine: Sender<Cmd>,
    to_app: Sender<TrayMsg>,
    open: Sender<()>,
    window: WindowSlot,
    /// Copy of the app's settings, kept current by the window.
    settings: Arc<Mutex<Settings>>,
    /// What was showing before "Turn lights off", so it can be restored.
    before_off: Option<Plan>,
}

pub type Handle = ksni::blocking::Handle<Tray>;

pub fn start(
    engine: Sender<Cmd>,
    to_app: Sender<TrayMsg>,
    open: Sender<()>,
    window: WindowSlot,
    settings: Arc<Mutex<Settings>>,
) -> Option<Handle> {
    // No tray support on this desktop → just run without one.
    Tray { engine, to_app, open, window, settings, before_off: None }.spawn().ok()
}

fn settings_of(m: &Mutex<Settings>) -> std::sync::MutexGuard<'_, Settings> {
    m.lock().unwrap_or_else(|p| p.into_inner())
}

impl Tray {
    fn apply(&mut self, plan: Plan) {
        let _ = self.engine.send(Cmd::Plan(plan.clone()));
        {
            let mut s = settings_of(&self.settings);
            s.plan = plan.clone();
            let _ = config::save(&s);
        }
        let _ = self.to_app.send(TrayMsg::SetPlan(plan));
        self.window.wake();
    }

    /// Open the window: brings an open one to the front, or opens a new one.
    fn show_window(&self) {
        let _ = self.open.send(());
        self.window.wake();
    }

    fn lights_are_off(&self) -> bool {
        let s = settings_of(&self.settings);
        s.plan.sync_all && s.plan.global.effect == Effect::Off
    }
}

/// The RGB Forge diamond, drawn as a 32×32 ARGB icon.
fn logo() -> Icon {
    let size = 32i32;
    let mut data = Vec::with_capacity(32 * 32 * 4);
    for y in 0..size {
        for x in 0..size {
            let (dx, dy) = ((x - 15).abs(), (y - 15).abs());
            let (a, r, g, b) = if dx + dy <= 7 {
                (255, 0, 200, 255) // inner cyan diamond
            } else if dx + dy <= 15 {
                (255, 139, 92, 246) // outer violet diamond
            } else {
                (0, 0, 0, 0)
            };
            data.extend_from_slice(&[a, r, g, b]);
        }
    }
    Icon { width: size, height: size, data }
}

impl ksni::Tray for Tray {
    fn id(&self) -> String {
        "rgbforge".into()
    }

    fn title(&self) -> String {
        "RGB Forge".into()
    }

    fn icon_pixmap(&self) -> Vec<Icon> {
        vec![logo()]
    }

    fn tool_tip(&self) -> ksni::ToolTip {
        ksni::ToolTip { title: "RGB Forge".into(), description: "Click to open. Right-click for quick controls.".into(), ..Default::default() }
    }

    /// Left click opens the window.
    fn activate(&mut self, _x: i32, _y: i32) {
        self.show_window();
    }

    fn menu(&self) -> Vec<MenuItem<Self>> {
        let profiles: Vec<(String, Plan)> =
            settings_of(&self.settings).profiles.iter().map(|(n, p)| (n.clone(), p.clone())).collect();
        let looks: Vec<MenuItem<Self>> = if profiles.is_empty() {
            vec![StandardItem { label: "No saved looks yet".into(), enabled: false, ..Default::default() }.into()]
        } else {
            profiles
                .into_iter()
                .map(|(name, plan)| {
                    StandardItem {
                        label: name.replace('_', "__"),
                        activate: Box::new(move |t: &mut Self| {
                            t.before_off = None;
                            t.apply(plan.clone());
                        }),
                        ..Default::default()
                    }
                    .into()
                })
                .collect()
        };
        let off = self.lights_are_off();
        vec![
            StandardItem {
                label: "Open RGB Forge".into(),
                icon_name: "preferences-desktop-color".into(),
                activate: Box::new(|t: &mut Self| t.show_window()),
                ..Default::default()
            }
            .into(),
            MenuItem::Separator,
            SubMenu { label: "Saved looks".into(), submenu: looks, ..Default::default() }.into(),
            StandardItem {
                label: if off { "Turn lights back on".into() } else { "Turn lights off".into() },
                activate: Box::new(|t: &mut Self| {
                    if t.lights_are_off() {
                        let restore = t.before_off.take().unwrap_or_default();
                        t.apply(restore);
                    } else {
                        let current = settings_of(&t.settings).plan.clone();
                        let mut dark = current.clone();
                        dark.sync_all = true;
                        dark.global.effect = Effect::Off;
                        t.before_off = Some(current);
                        t.apply(dark);
                    }
                }),
                ..Default::default()
            }
            .into(),
            MenuItem::Separator,
            StandardItem {
                label: "Quit RGB Forge".into(),
                icon_name: "application-exit".into(),
                activate: Box::new(|t: &mut Self| {
                    let _ = config::save(&settings_of(&t.settings));
                    // The lights keep their current colours (solid colours persist on the hardware).
                    std::process::exit(0);
                }),
                ..Default::default()
            }
            .into(),
        ]
    }
}

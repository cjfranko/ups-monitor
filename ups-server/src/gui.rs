//! Server-side convenience GUI: a tray icon reflecting overall health plus a
//! small egui window listing all configured UPS units and their live state.
//! Reads straight from the same shared state the API serves — no extra polling.

use std::sync::mpsc::{self, Receiver};
use std::sync::{Arc, Mutex};

use egui::{Color32, RichText};
use hidapi::HidApi;
use material_icons::{icon_to_char, Icon};
use tray_icon::menu::{Menu, MenuEvent, MenuItem};
use tray_icon::{TrayIconBuilder, TrayIconEvent};
use ups_common::PowerState;

use crate::config::{Config, UpsEntry};
use crate::hid::{self, DiscoveredUps};
use crate::logbuf;
use crate::poller::{self, PollerHandles};
use crate::state::{self, SharedState};

const FONT_NAME: &str = "material_icons";
const ARIAL_NAME: &str = "arial";

/// The app icon, embedded in the exe so there's nothing extra to deploy.
const ICON_PNG: &[u8] = include_bytes!("../assets/icon.png");

/// The window/taskbar icon, decoded from the embedded app icon.
fn load_window_icon() -> Option<egui::IconData> {
    let img = image::load_from_memory(ICON_PNG).ok()?.into_rgba8();
    let (width, height) = img.dimensions();
    Some(egui::IconData {
        rgba: img.into_raw(),
        width,
        height,
    })
}

/// Everything the GUI needs to manage UPS units live, not just display them.
pub struct GuiContext {
    pub state: SharedState,
    pub handles: PollerHandles,
    pub config: Arc<Mutex<Config>>,
    pub mock_mode: bool,
}

pub fn install_material_font(ctx: &egui::Context) {
    let mut fonts = egui::FontDefinitions::default();
    // Primary UI font: Arial from the Windows fonts folder (fall back to
    // egui's bundled font if it can't be read).
    if let Ok(arial) = std::fs::read(r"C:\Windows\Fonts\arial.ttf") {
        fonts
            .font_data
            .insert(ARIAL_NAME.to_owned(), egui::FontData::from_owned(arial));
        fonts
            .families
            .entry(egui::FontFamily::Proportional)
            .or_default()
            .insert(0, ARIAL_NAME.to_owned());
    }
    // Icon font as fallback for material icon codepoints.
    fonts.font_data.insert(
        FONT_NAME.to_owned(),
        egui::FontData::from_static(material_icons::FONT),
    );
    fonts
        .families
        .entry(egui::FontFamily::Proportional)
        .or_default()
        .push(FONT_NAME.to_owned());
    ctx.set_fonts(fonts);
}

fn icon_char(icon: Icon) -> char {
    icon_to_char(icon)
}

fn state_icon(s: PowerState) -> (char, Color32) {
    match s {
        PowerState::Online => (icon_char(Icon::Power), Color32::from_rgb(0x4c, 0xaf, 0x50)),
        PowerState::OnBattery => (icon_char(Icon::PowerOff), Color32::from_rgb(0xff, 0x98, 0x00)),
        PowerState::Unknown => (icon_char(Icon::Help), Color32::GRAY),
    }
}

/// Overall health drives the tray icon: green when all online, amber if any
/// on battery, red if any unknown/unreachable.
enum Health {
    AllOnline,
    AnyOnBattery,
    AnyUnknown,
}

fn overall_health(statuses: &[ups_common::UpsStatus]) -> Health {
    use PowerState::*;
    if statuses.iter().any(|s| s.status == Unknown) {
        Health::AnyUnknown
    } else if statuses.iter().any(|s| s.status == OnBattery) {
        Health::AnyOnBattery
    } else {
        Health::AllOnline
    }
}

const TRAY_GREEN: &[u8] = include_bytes!("../assets/tray-green.png");
const TRAY_ORANGE: &[u8] = include_bytes!("../assets/tray-orange.png");
const TRAY_RED: &[u8] = include_bytes!("../assets/tray-red.png");
const TRAY_ICON_SIZE: u32 = 32;

fn decode_tray_icon(bytes: &[u8]) -> tray_icon::Icon {
    let img = image::load_from_memory(bytes)
        .expect("bundled tray icon should decode")
        .resize_exact(TRAY_ICON_SIZE, TRAY_ICON_SIZE, image::imageops::FilterType::Lanczos3)
        .into_rgba8();
    tray_icon::Icon::from_rgba(img.into_raw(), TRAY_ICON_SIZE, TRAY_ICON_SIZE).expect("valid icon")
}

/// The three health-state tray icons, decoded once at startup.
struct TrayIcons {
    all_online: tray_icon::Icon,
    any_on_battery: tray_icon::Icon,
    any_unknown: tray_icon::Icon,
}

impl TrayIcons {
    fn load() -> Self {
        Self {
            all_online: decode_tray_icon(TRAY_GREEN),
            any_on_battery: decode_tray_icon(TRAY_ORANGE),
            any_unknown: decode_tray_icon(TRAY_RED),
        }
    }

    fn for_health(&self, h: &Health) -> tray_icon::Icon {
        match h {
            Health::AllOnline => self.all_online.clone(),
            Health::AnyOnBattery => self.any_on_battery.clone(),
            Health::AnyUnknown => self.any_unknown.clone(),
        }
    }
}

enum GuiCmd {
    Show,
    ToggleConsole,
}

/// State for the "Add UPS" dialog: a USB scan result plus one id text-edit
/// buffer per discovered-but-unconfigured device.
#[derive(Default)]
struct AddDialog {
    open: bool,
    scanned: bool,
    discovered: Vec<DiscoveredUps>,
    id_buffers: std::collections::HashMap<String, String>, // keyed by serial
    error: Option<String>,
}

/// State for the "Rename UPS" dialog.
struct RenameDialog {
    old_id: String,
    buffer: String,
    error: Option<String>,
}

pub struct StatusApp {
    state: SharedState,
    handles: PollerHandles,
    config: Arc<Mutex<Config>>,
    mock_mode: bool,
    tray: tray_icon::TrayIcon,
    tray_icons: TrayIcons,
    _menu: Menu,
    rx: Receiver<GuiCmd>,
    visible: bool,
    close_handled: bool,
    add_dialog: AddDialog,
    rename_dialog: Option<RenameDialog>,
    delete_target: Option<String>,
    show_console: bool,
}

impl StatusApp {
    pub fn new(cc: &eframe::CreationContext<'_>, ctx: GuiContext) -> Self {
        install_material_font(&cc.egui_ctx);

        let tray_icons = TrayIcons::load();
        let (tx, rx) = mpsc::channel::<GuiCmd>();

        let menu = Menu::new();
        let show = MenuItem::new("Show status", true, None);
        let console = MenuItem::new("Show Console", true, None);
        let quit = MenuItem::new("Quit", true, None);
        let _ = menu.append(&show);
        let _ = menu.append(&console);
        let _ = menu.append(&quit);

        let tray = TrayIconBuilder::new()
            .with_menu(Box::new(menu.clone()))
            .with_tooltip("UPS Monitor — server")
            .with_icon(tray_icons.for_health(&Health::AllOnline))
            .build()
            .expect("failed to create tray icon");

        {
            let show_id = show.id().clone();
            let console_id = console.id().clone();
            let quit_id = quit.id().clone();
            // Drive the viewport straight from the tray thread: when the
            // window is hidden, update() may not tick, so the mpsc command
            // would never be processed. egui::Context is Send+Sync and
            // viewport commands wake the event loop even while hidden.
            let egui_ctx = cc.egui_ctx.clone();
            std::thread::spawn(move || loop {
                if let Ok(event) = MenuEvent::receiver().recv() {
                    if event.id == show_id {
                        egui_ctx.send_viewport_cmd(egui::ViewportCommand::Visible(true));
                        egui_ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
                        egui_ctx.request_repaint();
                        let _ = tx.send(GuiCmd::Show);
                    } else if event.id == console_id {
                        egui_ctx.send_viewport_cmd(egui::ViewportCommand::Visible(true));
                        egui_ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
                        egui_ctx.request_repaint();
                        let _ = tx.send(GuiCmd::ToggleConsole);
                    } else if event.id == quit_id {
                        // Exit straight from the tray thread — going through
                        // the GUI update loop can hang after CancelClose.
                        std::process::exit(0);
                    }
                }
            });
        }
        // Drain tray events so the channel doesn't back up.
        std::thread::spawn(move || loop {
            let _ = TrayIconEvent::receiver().recv();
        });

        Self {
            state: ctx.state,
            handles: ctx.handles,
            config: ctx.config,
            mock_mode: ctx.mock_mode,
            tray,
            tray_icons,
            _menu: menu,
            rx,
            // Start visible so the user immediately sees UPS status; closing
            // the window hides it to the tray.
            visible: true,
            close_handled: false,
            add_dialog: AddDialog::default(),
            rename_dialog: None,
            delete_target: None,
            show_console: false,
        }
    }

    /// Ids currently in the config, for uniqueness checks.
    fn existing_ids(&self) -> Vec<String> {
        self.config
            .lock()
            .map(|c| c.ups.iter().map(|u| u.id.clone()).collect())
            .unwrap_or_default()
    }

    /// Add a newly-discovered UPS to the config, live state, and start a
    /// poller for it.
    fn commit_add(&mut self, serial: &str, id: String) {
        let id = id.trim().to_string();
        if id.is_empty() {
            self.add_dialog.error = Some("Id cannot be empty.".into());
            return;
        }
        if self.existing_ids().iter().any(|e| e == &id) {
            self.add_dialog.error = Some(format!("Id \"{id}\" is already in use."));
            return;
        }

        let entry = UpsEntry {
            id: id.clone(),
            serial: serial.to_string(),
        };
        let save_result = self.config.lock().map(|mut cfg| {
            cfg.ups.push(entry.clone());
            cfg.save()
        });
        if let Err(e) = save_result.unwrap_or_else(|_| Err(anyhow::anyhow!("config lock poisoned"))) {
            self.add_dialog.error = Some(format!("Failed to save config: {e}"));
            return;
        }

        state::ensure_entry(&self.state, &id);
        if self.mock_mode {
            poller::spawn_mock_poller(&self.handles, self.state.clone(), id.clone());
        } else {
            poller::spawn_real_poller(&self.handles, self.state.clone(), entry);
        }

        self.add_dialog.discovered.retain(|d| d.serial != serial);
        self.add_dialog.id_buffers.remove(serial);
        self.add_dialog.error = None;
        tracing::info!(ups = %id, serial = %serial, "UPS added via GUI");
    }

    fn commit_rename(&mut self) {
        let Some(dialog) = &self.rename_dialog else {
            return;
        };
        let old_id = dialog.old_id.clone();
        let new_id = dialog.buffer.trim().to_string();

        if new_id.is_empty() {
            self.rename_dialog.as_mut().unwrap().error = Some("Id cannot be empty.".into());
            return;
        }
        if new_id == old_id {
            self.rename_dialog = None;
            return;
        }
        if self.existing_ids().iter().any(|e| e == &new_id) {
            self.rename_dialog.as_mut().unwrap().error =
                Some(format!("Id \"{new_id}\" is already in use."));
            return;
        }

        let save_result = self.config.lock().map(|mut cfg| {
            let mut serial = String::new();
            for u in cfg.ups.iter_mut() {
                if u.id == old_id {
                    u.id = new_id.clone();
                    serial = u.serial.clone();
                }
            }
            cfg.save().map(|_| serial)
        });
        let serial = match save_result.unwrap_or_else(|_| Err(anyhow::anyhow!("config lock poisoned"))) {
            Ok(serial) => serial,
            Err(e) => {
                self.rename_dialog.as_mut().unwrap().error =
                    Some(format!("Failed to save config: {e}"));
                return;
            }
        };

        // Respawn the poller under the new id: the running one still labels
        // every reading with the id it was started with.
        poller::stop_poller(&self.handles, &old_id);
        state::rename(&self.state, &old_id, &new_id);
        if self.mock_mode {
            poller::spawn_mock_poller(&self.handles, self.state.clone(), new_id.clone());
        } else {
            poller::spawn_real_poller(
                &self.handles,
                self.state.clone(),
                UpsEntry {
                    id: new_id.clone(),
                    serial,
                },
            );
        }

        tracing::info!(old = %old_id, new = %new_id, "UPS renamed via GUI");
        self.rename_dialog = None;
    }

    fn commit_delete(&mut self, id: &str) {
        poller::stop_poller(&self.handles, id);
        state::remove(&self.state, id);
        let save_result = self.config.lock().map(|mut cfg| {
            cfg.ups.retain(|u| u.id != id);
            cfg.save()
        });
        if let Err(e) = save_result.unwrap_or_else(|_| Err(anyhow::anyhow!("config lock poisoned"))) {
            tracing::warn!(error = %e, "failed to save config after deleting UPS");
        }
        tracing::info!(ups = %id, "UPS deleted via GUI");
    }

    fn scan_for_ups(&mut self) {
        self.add_dialog.scanned = true;
        self.add_dialog.error = None;
        match HidApi::new() {
            Ok(api) => {
                let configured: Vec<String> =
                    self.existing_ids_serials();
                self.add_dialog.discovered = hid::enumerate_ups(&api)
                    .into_iter()
                    .filter(|d| !configured.contains(&d.serial))
                    .collect();
                for d in &self.add_dialog.discovered {
                    self.add_dialog
                        .id_buffers
                        .entry(d.serial.clone())
                        .or_insert_with(|| suggest_id(d));
                }
            }
            Err(e) => {
                self.add_dialog.error = Some(format!("HID scan failed: {e}"));
                self.add_dialog.discovered.clear();
            }
        }
    }

    fn existing_ids_serials(&self) -> Vec<String> {
        self.config
            .lock()
            .map(|c| c.ups.iter().map(|u| u.serial.clone()).collect())
            .unwrap_or_default()
    }
}

/// Suggest a default id for a freshly-discovered UPS from its product name
/// (or fall back to a short form of the serial).
fn suggest_id(d: &DiscoveredUps) -> String {
    if !d.product.trim().is_empty() {
        d.product
            .trim()
            .to_lowercase()
            .chars()
            .map(|c| if c.is_alphanumeric() { c } else { '-' })
            .collect()
    } else {
        format!("ups-{}", &d.serial.chars().rev().take(4).collect::<String>())
    }
}

/// What the user asked to do with a UPS card, if anything.
enum CardAction {
    None,
    Rename,
    Delete,
}

/// Renders one UPS card. Returns which action button (if any) was clicked.
fn ups_card(ui: &mut egui::Ui, s: &ups_common::UpsStatus) -> CardAction {
    let (ic, col) = state_icon(s.status);
    let mut action = CardAction::None;
    egui::Frame::group(ui.style())
        .inner_margin(egui::Margin::same(10.0))
        .show(ui, |ui| {
            ui.set_min_width(ui.available_width());
            ui.horizontal(|ui| {
                ui.allocate_ui_with_layout(
                    egui::vec2(36.0, 60.0),
                    egui::Layout::centered_and_justified(egui::Direction::TopDown),
                    |ui| {
                        ui.label(RichText::new(ic.to_string()).size(30.0).color(col));
                    },
                );
                ui.add_space(8.0);
                ui.vertical(|ui| {
                    ui.set_min_width(ui.available_width());
                    ui.horizontal(|ui| {
                        ui.label(RichText::new(&s.id).strong().size(16.0));
                        if ui
                            .small_button(icon_char(Icon::Edit).to_string())
                            .on_hover_text("Rename")
                            .clicked()
                        {
                            action = CardAction::Rename;
                        }
                        if ui
                            .small_button(icon_char(Icon::Delete).to_string())
                            .on_hover_text("Delete")
                            .clicked()
                        {
                            action = CardAction::Delete;
                        }
                        ui.add_space(6.0);
                        ui.label(RichText::new(format!("{}", s.status)).color(col).strong());
                    });
                    let battery = s
                        .battery_pct
                        .map(|p| format!("{p}%"))
                        .unwrap_or_else(|| "—".to_string());
                    let runtime = s
                        .runtime_secs
                        .map(|r| format!("{} min", r / 60))
                        .unwrap_or_else(|| "—".to_string());
                    ui.label(format!("Battery: {battery}   Runtime: {runtime}"));
                    ui.small(format!("Updated {}", s.last_updated.format("%H:%M:%S")));
                });
            });
        });
    action
}

impl eframe::App for StatusApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        // Handle tray menu commands.
        while let Ok(cmd) = self.rx.try_recv() {
            match cmd {
                GuiCmd::Show => {
                    self.visible = true;
                    self.close_handled = false;
                    ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(false));
                    ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
                }
                GuiCmd::ToggleConsole => {
                    self.visible = true;
                    self.close_handled = false;
                    self.show_console = !self.show_console;
                    ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(false));
                    ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
                }
            }
        }

        // Close button minimizes to tray rather than quitting. Minimize
        // rather than hide: eframe/winit stop delivering redraw events to a
        // truly hidden window on Windows, which means a later
        // `Visible(true)` is never actually processed and the window can
        // never come back — minimizing doesn't have that problem, and the
        // taskbar entry is suppressed via `with_taskbar(false)` at window
        // creation so this still reads as "closed to tray". Only cancel the
        // close once — re-cancelling every frame while the request is still
        // pending interferes with rendering.
        if ctx.input(|i| i.viewport().close_requested()) && self.visible && !self.close_handled {
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            self.visible = false;
            self.close_handled = true;
            // Also close any floating sub-window (Console, Add UPS, etc.) —
            // leaving one "open" while the main viewport is minimized left
            // the window unable to come back when reopened from the tray.
            self.show_console = false;
            ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(true));
        }

        let statuses = state::snapshot(&self.state);
        let health = overall_health(&statuses);
        let _ = self.tray.set_icon(Some(self.tray_icons.for_health(&health)));
        let tooltip = match health {
            Health::AllOnline => "UPS Monitor — all online".to_string(),
            Health::AnyOnBattery => "UPS Monitor — a UPS is on battery".to_string(),
            Health::AnyUnknown => "UPS Monitor — a UPS is unreachable".to_string(),
        };
        let _ = self.tray.set_tooltip(Some(tooltip));

        egui::CentralPanel::default().show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.heading(format!("{} UPS Monitor — Server", icon_char(Icon::Power)));
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui
                        .button(format!("{}  Add UPS", icon_char(Icon::Add)))
                        .clicked()
                    {
                        self.add_dialog.open = true;
                        self.add_dialog.scanned = false;
                        self.add_dialog.discovered.clear();
                        self.add_dialog.error = None;
                    }
                });
            });
            ui.add_space(6.0);

            // Simulation toggle: forces every UPS to report OnBattery so the
            // clients' alert paths can be tested without touching hardware.
            let sim = state::simulate_on_battery();
            let label = if sim {
                "■ Stop simulation (ON BATTERY)"
            } else {
                "Simulate UPS on Battery"
            };
            let btn = egui::Button::new(RichText::new(label).color(Color32::WHITE).strong())
                .fill(Color32::from_rgb(0xc6, 0x28, 0x28));
            if ui.add(btn).clicked() {
                state::set_simulate_on_battery(!sim);
                tracing::info!(enabled = !sim, "on-battery simulation toggled");
            }
            if sim {
                ui.label(
                    RichText::new("Simulation active: all units report OnBattery")
                        .color(Color32::from_rgb(0xc6, 0x28, 0x28))
                        .small(),
                );
            }
            ui.add_space(6.0);

            if statuses.is_empty() {
                ui.label("No UPS units configured.");
            }
            let mut rename_requested: Option<String> = None;
            let mut delete_requested: Option<String> = None;
            for s in &statuses {
                match ups_card(ui, s) {
                    CardAction::Rename => rename_requested = Some(s.id.clone()),
                    CardAction::Delete => delete_requested = Some(s.id.clone()),
                    CardAction::None => {}
                }
                ui.add_space(6.0);
            }
            if let Some(id) = rename_requested {
                self.rename_dialog = Some(RenameDialog {
                    buffer: id.clone(),
                    old_id: id,
                    error: None,
                });
            }
            if let Some(id) = delete_requested {
                self.delete_target = Some(id);
            }
        });

        // "Add UPS" dialog: scan USB, offer to add anything not yet configured.
        if self.add_dialog.open {
            let mut open = true;
            let mut do_scan = false;
            let mut commit: Option<(String, String)> = None;
            egui::Window::new(format!("{} Add UPS", icon_char(Icon::Usb)))
                .collapsible(false)
                .resizable(false)
                .open(&mut open)
                .show(ctx, |ui| {
                    if ui.button("Scan USB ports").clicked() {
                        do_scan = true;
                    }
                    ui.add_space(6.0);
                    if let Some(err) = &self.add_dialog.error {
                        ui.colored_label(Color32::from_rgb(0xc6, 0x28, 0x28), err);
                        ui.add_space(6.0);
                    }
                    if self.add_dialog.scanned && self.add_dialog.discovered.is_empty() {
                        ui.label("No unconfigured UPS units found on USB.");
                    }
                    for d in self.add_dialog.discovered.clone() {
                        ui.separator();
                        ui.label(RichText::new(&d.product).strong());
                        ui.small(format!("Serial: {}", d.serial));
                        ui.horizontal(|ui| {
                            ui.label("Id:");
                            let buf = self
                                .add_dialog
                                .id_buffers
                                .entry(d.serial.clone())
                                .or_insert_with(|| suggest_id(&d));
                            ui.text_edit_singleline(buf);
                            if ui.button("Add").clicked() {
                                commit = Some((d.serial.clone(), buf.clone()));
                            }
                        });
                    }
                });
            if do_scan {
                self.scan_for_ups();
            }
            if let Some((serial, id)) = commit {
                self.commit_add(&serial, id);
            }
            self.add_dialog.open = open;
        }

        // "Rename UPS" dialog.
        if let Some(dialog) = &self.rename_dialog {
            let mut open = true;
            let mut buffer = dialog.buffer.clone();
            let mut save_clicked = false;
            let mut cancel_clicked = false;
            let old_id = dialog.old_id.clone();
            let error = dialog.error.clone();
            egui::Window::new(format!("Rename \"{old_id}\""))
                .collapsible(false)
                .resizable(false)
                .open(&mut open)
                .show(ctx, |ui| {
                    if let Some(err) = &error {
                        ui.colored_label(Color32::from_rgb(0xc6, 0x28, 0x28), err);
                        ui.add_space(6.0);
                    }
                    ui.horizontal(|ui| {
                        ui.label("New id:");
                        let resp = ui.text_edit_singleline(&mut buffer);
                        if resp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                            save_clicked = true;
                        }
                    });
                    ui.add_space(6.0);
                    ui.horizontal(|ui| {
                        if ui.button("Save").clicked() {
                            save_clicked = true;
                        }
                        if ui.button("Cancel").clicked() {
                            cancel_clicked = true;
                        }
                    });
                });
            if let Some(d) = self.rename_dialog.as_mut() {
                d.buffer = buffer;
            }
            if save_clicked {
                self.commit_rename();
            } else if cancel_clicked || !open {
                self.rename_dialog = None;
            }
        }

        // "Delete UPS" confirmation.
        if let Some(id) = self.delete_target.clone() {
            let mut open = true;
            let mut confirm_clicked = false;
            let mut cancel_clicked = false;
            egui::Window::new(format!("Delete \"{id}\"?"))
                .collapsible(false)
                .resizable(false)
                .open(&mut open)
                .show(ctx, |ui| {
                    ui.label("This removes it from the config and stops polling it.");
                    ui.add_space(6.0);
                    ui.horizontal(|ui| {
                        let btn = egui::Button::new(RichText::new("Delete").color(Color32::WHITE))
                            .fill(Color32::from_rgb(0xc6, 0x28, 0x28));
                        if ui.add(btn).clicked() {
                            confirm_clicked = true;
                        }
                        if ui.button("Cancel").clicked() {
                            cancel_clicked = true;
                        }
                    });
                });
            if confirm_clicked {
                self.commit_delete(&id);
                self.delete_target = None;
            } else if cancel_clicked || !open {
                self.delete_target = None;
            }
        }

        // In-app "Console" — a real OS console can't be closed safely (see
        // `logbuf.rs`), so this shows the same log lines in a window we
        // fully control instead.
        if self.show_console {
            egui::Window::new("Console")
                .open(&mut self.show_console)
                .default_size([560.0, 360.0])
                .show(ctx, |ui| {
                    egui::ScrollArea::vertical()
                        .stick_to_bottom(true)
                        .show(ui, |ui| {
                            ui.style_mut().override_font_id =
                                Some(egui::FontId::monospace(12.0));
                            for line in logbuf::snapshot() {
                                ui.label(line);
                            }
                        });
                });
        }

        ctx.request_repaint_after(std::time::Duration::from_millis(500));
    }

    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {}

    fn persist_egui_memory(&self) -> bool {
        false
    }
}

pub fn run(ctx: GuiContext) -> eframe::Result<()> {
    let mut viewport = egui::ViewportBuilder::default()
        .with_title("UPS Monitor — Server")
        .with_inner_size([420.0, 500.0])
        .with_visible(true)
        // The tray icon is this app's persistent presence; a taskbar entry
        // would be redundant, and minimizing-to-tray (see `update`) would
        // otherwise leave a "minimized" entry sitting in the taskbar.
        .with_taskbar(false);
    if let Some(icon) = load_window_icon() {
        viewport = viewport.with_icon(icon);
    }
    let options = eframe::NativeOptions {
        viewport,
        ..Default::default()
    };
    eframe::run_native(
        "ups-server",
        options,
        Box::new(move |cc| Ok(Box::new(StatusApp::new(cc, ctx)))),
    )
}

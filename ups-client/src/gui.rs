//! Client GUI: tray icon (UPS health + server connectivity) plus a small egui
//! window listing each UPS and a "Connected / Unreachable" line.

use std::sync::mpsc::{self, Receiver};
use std::sync::{Arc, Mutex};

use egui::{Color32, RichText};
use material_icons::{icon_to_char, Icon};
use tray_icon::menu::{Menu, MenuEvent, MenuItem};
use tray_icon::{TrayIconBuilder, TrayIconEvent};
use ups_common::PowerState;

use crate::aumid;
use crate::config::Config;
use crate::logbuf;
use crate::poller::PollerControl;
use crate::state::{self, Connectivity, SharedClientState};

/// The window/taskbar icon, decoded from the same bytes embedded for the
/// toast icon (see `aumid::ICON_PNG`).
fn load_window_icon() -> Option<egui::IconData> {
    let img = image::load_from_memory(aumid::ICON_PNG).ok()?.into_rgba8();
    let (width, height) = img.dimensions();
    Some(egui::IconData {
        rgba: img.into_raw(),
        width,
        height,
    })
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
    ok: tray_icon::Icon,
    on_battery: tray_icon::Icon,
    unreachable: tray_icon::Icon,
}

impl TrayIcons {
    fn load() -> Self {
        Self {
            ok: decode_tray_icon(TRAY_GREEN),
            on_battery: decode_tray_icon(TRAY_ORANGE),
            unreachable: decode_tray_icon(TRAY_RED),
        }
    }

    fn for_state(&self, t: &TrayState) -> tray_icon::Icon {
        match t {
            TrayState::Ok => self.ok.clone(),
            TrayState::OnBattery => self.on_battery.clone(),
            TrayState::Unreachable => self.unreachable.clone(),
        }
    }
}

const FONT_NAME: &str = "material_icons";
const ARIAL_NAME: &str = "arial";

/// Everything the GUI needs to display status and manage the server
/// connection live, not just render a snapshot.
pub struct GuiContext {
    pub state: SharedClientState,
    pub config: Arc<Mutex<Config>>,
    pub poller: Arc<PollerControl>,
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

fn ic(icon: Icon) -> char {
    icon_to_char(icon)
}

fn state_icon(s: PowerState) -> (char, Color32) {
    match s {
        PowerState::Online => (ic(Icon::Power), Color32::from_rgb(0x4c, 0xaf, 0x50)),
        PowerState::OnBattery => (ic(Icon::PowerOff), Color32::from_rgb(0xff, 0x98, 0x00)),
        PowerState::Unknown => (ic(Icon::Help), Color32::GRAY),
    }
}

enum TrayState {
    Ok,          // connected, all online
    OnBattery,   // connected, some on battery
    Unreachable, // cannot reach server
}

fn tray_state(cs: &state::ClientState) -> TrayState {
    if cs.connectivity == Connectivity::Unreachable {
        return TrayState::Unreachable;
    }
    if cs.statuses.iter().any(|s| s.status == PowerState::OnBattery) {
        TrayState::OnBattery
    } else {
        TrayState::Ok
    }
}

enum GuiCmd {
    Show,
    ToggleConsole,
}

/// State for the "Server settings" dialog. Port is kept as text while being
/// edited so an in-progress edit (e.g. an empty field) doesn't get clobbered
/// by parse failures.
struct ServerDialog {
    address: String,
    port: String,
    error: Option<String>,
}

impl ServerDialog {
    fn from_config(cfg: &Config) -> Self {
        Self {
            address: cfg.server.address.clone().unwrap_or_default(),
            port: cfg.server.port.to_string(),
            error: None,
        }
    }
}

pub struct ClientApp {
    state: SharedClientState,
    config: Arc<Mutex<Config>>,
    poller: Arc<PollerControl>,
    tray: tray_icon::TrayIcon,
    tray_icons: TrayIcons,
    _menu: Menu,
    rx: Receiver<GuiCmd>,
    visible: bool,
    close_handled: bool,
    server_dialog: Option<ServerDialog>,
    show_console: bool,
}

impl ClientApp {
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
            .with_tooltip("UPS Monitor — client")
            .with_icon(tray_icons.for_state(&TrayState::Ok))
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
                        std::process::exit(0);
                    }
                }
            });
        }
        std::thread::spawn(move || loop {
            let _ = TrayIconEvent::receiver().recv();
        });

        // First run (or a config with no address yet): open the dialog
        // immediately instead of showing an empty status list.
        let server_dialog = {
            let cfg = ctx.config.lock().unwrap();
            if cfg.has_server() {
                None
            } else {
                Some(ServerDialog::from_config(&cfg))
            }
        };

        Self {
            state: ctx.state,
            config: ctx.config,
            poller: ctx.poller,
            tray,
            tray_icons,
            _menu: menu,
            rx,
            // Start visible so status is immediately clear; closing the
            // window hides it to the tray.
            visible: true,
            close_handled: false,
            server_dialog,
            show_console: false,
        }
    }

    fn commit_server_dialog(&mut self) {
        let Some(dialog) = &self.server_dialog else {
            return;
        };
        let address = dialog.address.trim().to_string();
        if address.is_empty() {
            self.server_dialog.as_mut().unwrap().error = Some("Address cannot be empty.".into());
            return;
        }
        let port: u16 = match dialog.port.trim().parse() {
            Ok(p) => p,
            Err(_) => {
                self.server_dialog.as_mut().unwrap().error =
                    Some("Port must be a number between 1 and 65535.".into());
                return;
            }
        };

        let save_result = self.config.lock().map(|mut cfg| {
            cfg.server.address = Some(address);
            cfg.server.port = port;
            cfg.save().map(|_| cfg.clone())
        });
        let cfg = match save_result.unwrap_or_else(|_| Err(anyhow::anyhow!("config lock poisoned"))) {
            Ok(cfg) => cfg,
            Err(e) => {
                self.server_dialog.as_mut().unwrap().error =
                    Some(format!("Failed to save config: {e}"));
                return;
            }
        };

        self.poller.restart(cfg, self.state.clone());
        tracing::info!("server settings updated via GUI");
        self.server_dialog = None;
    }
}

fn ups_card(ui: &mut egui::Ui, s: &ups_common::UpsStatus) {
    let (icon, col) = state_icon(s.status);
    egui::Frame::group(ui.style())
        .inner_margin(egui::Margin::same(10.0))
        .show(ui, |ui| {
            ui.set_min_width(ui.available_width());
            ui.horizontal(|ui| {
                ui.allocate_ui_with_layout(
                    egui::vec2(36.0, 60.0),
                    egui::Layout::centered_and_justified(egui::Direction::TopDown),
                    |ui| {
                        ui.label(RichText::new(icon.to_string()).size(30.0).color(col));
                    },
                );
                ui.add_space(8.0);
                ui.vertical(|ui| {
                    ui.set_min_width(ui.available_width());
                    ui.horizontal(|ui| {
                        ui.label(RichText::new(&s.id).strong().size(16.0));
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
}

impl eframe::App for ClientApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
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
            // Also close any floating sub-window (Console, Server settings)
            // — leaving one "open" while the main viewport is minimized left
            // the window unable to come back when reopened from the tray.
            self.show_console = false;
            ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(true));
        }

        let cs = state::snapshot(&self.state);
        let ts = tray_state(&cs);
        let _ = self.tray.set_icon(Some(self.tray_icons.for_state(&ts)));
        let tooltip = match ts {
            TrayState::Ok => "UPS Monitor — all online".to_string(),
            TrayState::OnBattery => "UPS Monitor — a UPS is on battery".to_string(),
            TrayState::Unreachable => "UPS Monitor — server unreachable".to_string(),
        };
        let _ = self.tray.set_tooltip(Some(tooltip));

        egui::CentralPanel::default().show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.heading(format!("{} UPS Monitor — Client", ic(Icon::Power)));
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui
                        .button(format!("{}  Server settings", ic(Icon::Settings)))
                        .clicked()
                    {
                        let cfg = self.config.lock().unwrap();
                        self.server_dialog = Some(ServerDialog::from_config(&cfg));
                    }
                });
            });
            ui.add_space(6.0);

            let configured = self.config.lock().map(|c| c.has_server()).unwrap_or(false);
            if !configured {
                ui.label(
                    RichText::new("No server configured yet — set one up in Server settings.")
                        .color(Color32::from_rgb(0xff, 0x98, 0x00)),
                );
                ui.add_space(6.0);
            }

            // Connectivity line.
            let (conn_icon, conn_text, conn_col) = match cs.connectivity {
                Connectivity::Connected => (
                    ic(Icon::Wifi),
                    "Connected to server".to_string(),
                    Color32::from_rgb(0x4c, 0xaf, 0x50),
                ),
                Connectivity::Unreachable => {
                    let last = cs
                        .last_success
                        .map(|t| format!(" — last seen {}", t.format("%H:%M:%S")))
                        .unwrap_or_default();
                    (
                        ic(Icon::WifiOff),
                        format!("Unreachable{last}"),
                        Color32::from_rgb(0xc6, 0x28, 0x28),
                    )
                }
            };
            if configured {
                ui.horizontal(|ui| {
                    ui.label(RichText::new(conn_icon.to_string()).size(20.0).color(conn_col));
                    ui.label(RichText::new(conn_text).color(conn_col).strong());
                });
                ui.add_space(6.0);
            }

            if cs.statuses.is_empty() {
                ui.label("No UPS data yet.");
            }
            for s in &cs.statuses {
                ups_card(ui, s);
                ui.add_space(6.0);
            }
        });

        // "Server settings" dialog. No native close button: if no server is
        // configured yet the dialog must be completed to proceed, and once
        // one exists closing happens via Save/Cancel.
        if let Some(dialog) = &self.server_dialog {
            let mut address = dialog.address.clone();
            let mut port = dialog.port.clone();
            let error = dialog.error.clone();
            let mut save_clicked = false;
            let mut cancel_clicked = false;
            let cancellable = self
                .config
                .lock()
                .map(|c| c.has_server())
                .unwrap_or(false);
            egui::Window::new(format!("{} Server settings", ic(Icon::Settings)))
                .collapsible(false)
                .resizable(false)
                .show(ctx, |ui| {
                    if let Some(err) = &error {
                        ui.colored_label(Color32::from_rgb(0xc6, 0x28, 0x28), err);
                        ui.add_space(6.0);
                    }
                    egui::Grid::new("server_settings_grid")
                        .num_columns(2)
                        .show(ui, |ui| {
                            ui.label("Server address:");
                            ui.text_edit_singleline(&mut address);
                            ui.end_row();
                            ui.label("Port:");
                            ui.text_edit_singleline(&mut port);
                            ui.end_row();
                        });
                    ui.add_space(6.0);
                    ui.horizontal(|ui| {
                        if ui.button("Save").clicked() {
                            save_clicked = true;
                        }
                        if cancellable && ui.button("Cancel").clicked() {
                            cancel_clicked = true;
                        }
                    });
                });
            if let Some(d) = self.server_dialog.as_mut() {
                d.address = address;
                d.port = port;
            }
            if save_clicked {
                self.commit_server_dialog();
            } else if cancel_clicked {
                self.server_dialog = None;
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
        .with_title("UPS Monitor — Client")
        .with_inner_size([420.0, 520.0])
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
        "ups-client",
        options,
        Box::new(move |cc| Ok(Box::new(ClientApp::new(cc, ctx)))),
    )
}

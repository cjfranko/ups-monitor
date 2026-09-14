//! Client GUI: tray icon (UPS health + server connectivity) plus a small egui
//! window listing each UPS and a "Connected / Unreachable" line.

use std::sync::mpsc::{self, Receiver};

use egui::{Color32, RichText};
use material_icons::{icon_to_char, Icon};
use tray_icon::menu::{Menu, MenuEvent, MenuItem};
use tray_icon::{TrayIconBuilder, TrayIconEvent};
use ups_common::PowerState;

use crate::state::{self, Connectivity, SharedClientState};

const FONT_NAME: &str = "material_icons";
const ARIAL_NAME: &str = "arial";

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

fn solid_icon(rgba: [u8; 4]) -> tray_icon::Icon {
    let mut data = Vec::with_capacity(32 * 32 * 4);
    for _ in 0..32 * 32 {
        data.extend_from_slice(&rgba);
    }
    tray_icon::Icon::from_rgba(data, 32, 32).expect("valid icon")
}

fn tray_icon_for(t: &TrayState) -> tray_icon::Icon {
    match t {
        TrayState::Ok => solid_icon([0x2e, 0x7d, 0x32, 0xff]),
        TrayState::OnBattery => solid_icon([0xf9, 0xa8, 0x25, 0xff]),
        TrayState::Unreachable => solid_icon([0xc6, 0x28, 0x28, 0xff]),
    }
}

enum GuiCmd {
    Show,
}

pub struct ClientApp {
    state: SharedClientState,
    tray: tray_icon::TrayIcon,
    _menu: Menu,
    rx: Receiver<GuiCmd>,
    visible: bool,
    close_handled: bool,
}

impl ClientApp {
    pub fn new(cc: &eframe::CreationContext<'_>, state: SharedClientState) -> Self {
        install_material_font(&cc.egui_ctx);

        let (tx, rx) = mpsc::channel::<GuiCmd>();
        let menu = Menu::new();
        let show = MenuItem::new("Show status", true, None);
        let quit = MenuItem::new("Quit", true, None);
        let _ = menu.append(&show);
        let _ = menu.append(&quit);

        let tray = TrayIconBuilder::new()
            .with_menu(Box::new(menu.clone()))
            .with_tooltip("UPS Monitor — client")
            .with_icon(tray_icon_for(&TrayState::Ok))
            .build()
            .expect("failed to create tray icon");

        {
            let show_id = show.id().clone();
            let quit_id = quit.id().clone();
            std::thread::spawn(move || loop {
                if let Ok(event) = MenuEvent::receiver().recv() {
                    if event.id == show_id {
                        let _ = tx.send(GuiCmd::Show);
                    } else if event.id == quit_id {
                        // Exit straight from the tray thread — going through
                        // the GUI update loop can hang after CancelClose.
                        std::process::exit(0);
                    }
                }
            });
        }
        std::thread::spawn(move || loop {
            let _ = TrayIconEvent::receiver().recv();
        });

        Self {
            state,
            tray,
            _menu: menu,
            rx,
            // Start visible so status is immediately clear; closing the
            // window hides it to the tray.
            visible: true,
            close_handled: false,
        }
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
                    ctx.send_viewport_cmd(egui::ViewportCommand::Visible(true));
                    ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
                }
            }
        }

        // Close button hides to tray rather than quitting. Only cancel the
        // close once — re-cancelling every frame while the request is still
        // pending interferes with rendering.
        if ctx.input(|i| i.viewport().close_requested()) && self.visible && !self.close_handled {
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            self.visible = false;
            self.close_handled = true;
            ctx.send_viewport_cmd(egui::ViewportCommand::Visible(false));
        }

        let cs = state::snapshot(&self.state);
        let ts = tray_state(&cs);
        let _ = self.tray.set_icon(Some(tray_icon_for(&ts)));
        let tooltip = match ts {
            TrayState::Ok => "UPS Monitor — all online".to_string(),
            TrayState::OnBattery => "UPS Monitor — a UPS is on battery".to_string(),
            TrayState::Unreachable => "UPS Monitor — server unreachable".to_string(),
        };
        let _ = self.tray.set_tooltip(Some(tooltip));

        egui::CentralPanel::default().show(ctx, |ui| {
            ui.heading(format!("{} UPS Monitor — Client", ic(Icon::Power)));
            ui.add_space(6.0);

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
            ui.horizontal(|ui| {
                ui.label(RichText::new(conn_icon.to_string()).size(20.0).color(conn_col));
                ui.label(RichText::new(conn_text).color(conn_col).strong());
            });
            ui.add_space(6.0);

            if cs.statuses.is_empty() {
                ui.label("No UPS data yet.");
            }
            for s in &cs.statuses {
                ups_card(ui, s);
                ui.add_space(6.0);
            }
        });

        ctx.request_repaint_after(std::time::Duration::from_millis(500));
    }

    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {}

    fn persist_egui_memory(&self) -> bool {
        false
    }
}

pub fn run(state: SharedClientState) -> eframe::Result<()> {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("UPS Monitor — Client")
            .with_inner_size([420.0, 520.0])
            .with_visible(true),
        ..Default::default()
    };
    eframe::run_native(
        "ups-client",
        options,
        Box::new(move |cc| Ok(Box::new(ClientApp::new(cc, state)))),
    )
}

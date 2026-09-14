//! Server-side convenience GUI: a tray icon reflecting overall health plus a
//! small egui window listing all configured UPS units and their live state.
//! Reads straight from the same shared state the API serves — no extra polling.

use std::sync::mpsc::{self, Receiver};

use egui::{Color32, RichText};
use material_icons::{icon_to_char, Icon};
use tray_icon::menu::{Menu, MenuEvent, MenuItem};
use tray_icon::{TrayIconBuilder, TrayIconEvent};
use ups_common::PowerState;

use crate::state::{self, SharedState};

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

fn solid_icon(rgba: [u8; 4]) -> tray_icon::Icon {
    let mut data = Vec::with_capacity(32 * 32 * 4);
    for _ in 0..32 * 32 {
        data.extend_from_slice(&rgba);
    }
    tray_icon::Icon::from_rgba(data, 32, 32).expect("valid icon")
}

fn health_icon(h: &Health) -> tray_icon::Icon {
    match h {
        Health::AllOnline => solid_icon([0x2e, 0x7d, 0x32, 0xff]),
        Health::AnyOnBattery => solid_icon([0xf9, 0xa8, 0x25, 0xff]),
        Health::AnyUnknown => solid_icon([0xc6, 0x28, 0x28, 0xff]),
    }
}

enum GuiCmd {
    Show,
}

pub struct StatusApp {
    state: SharedState,
    tray: tray_icon::TrayIcon,
    _menu: Menu,
    rx: Receiver<GuiCmd>,
    visible: bool,
    close_handled: bool,
}

impl StatusApp {
    pub fn new(cc: &eframe::CreationContext<'_>, state: SharedState) -> Self {
        install_material_font(&cc.egui_ctx);

        let (tx, rx) = mpsc::channel::<GuiCmd>();

        let menu = Menu::new();
        let show = MenuItem::new("Show status", true, None);
        let quit = MenuItem::new("Quit", true, None);
        let _ = menu.append(&show);
        let _ = menu.append(&quit);

        let tray = TrayIconBuilder::new()
            .with_menu(Box::new(menu.clone()))
            .with_tooltip("UPS Monitor — server")
            .with_icon(health_icon(&Health::AllOnline))
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
        // Drain tray events so the channel doesn't back up.
        std::thread::spawn(move || loop {
            let _ = TrayIconEvent::receiver().recv();
        });

        Self {
            state,
            tray,
            _menu: menu,
            rx,
            // Start visible so the user immediately sees UPS status; closing
            // the window hides it to the tray.
            visible: true,
            close_handled: false,
        }
    }
}

fn ups_card(ui: &mut egui::Ui, s: &ups_common::UpsStatus) {
    let (ic, col) = state_icon(s.status);
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

impl eframe::App for StatusApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        // Handle tray menu commands.
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

        let statuses = state::snapshot(&self.state);
        let health = overall_health(&statuses);
        let _ = self.tray.set_icon(Some(health_icon(&health)));
        let tooltip = match health {
            Health::AllOnline => "UPS Monitor — all online".to_string(),
            Health::AnyOnBattery => "UPS Monitor — a UPS is on battery".to_string(),
            Health::AnyUnknown => "UPS Monitor — a UPS is unreachable".to_string(),
        };
        let _ = self.tray.set_tooltip(Some(tooltip));

        egui::CentralPanel::default().show(ctx, |ui| {
            ui.heading(format!("{} UPS Monitor — Server", icon_char(Icon::Power)));
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
            for s in &statuses {
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

pub fn run(state: SharedState) -> eframe::Result<()> {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("UPS Monitor — Server")
            .with_inner_size([420.0, 500.0])
            .with_visible(true),
        ..Default::default()
    };
    eframe::run_native(
        "ups-server",
        options,
        Box::new(move |cc| Ok(Box::new(StatusApp::new(cc, state)))),
    )
}

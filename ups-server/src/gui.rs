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

pub fn install_material_font(ctx: &egui::Context) {
    let mut fonts = egui::FontDefinitions::default();
    fonts.font_data.insert(
        FONT_NAME.to_owned(),
        egui::FontData::from_static(material_icons::FONT),
    );
    fonts
        .families
        .entry(egui::FontFamily::Proportional)
        .or_default()
        .insert(0, FONT_NAME.to_owned());
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
    Quit,
}

pub struct StatusApp {
    state: SharedState,
    tray: tray_icon::TrayIcon,
    _menu: Menu,
    rx: Receiver<GuiCmd>,
    visible: bool,
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
                        let _ = tx.send(GuiCmd::Quit);
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
            visible: false,
        }
    }
}

impl eframe::App for StatusApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        // Handle tray menu commands.
        while let Ok(cmd) = self.rx.try_recv() {
            match cmd {
                GuiCmd::Show => {
                    self.visible = true;
                    ctx.send_viewport_cmd(egui::ViewportCommand::Visible(true));
                    ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
                }
                GuiCmd::Quit => {
                    ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                    std::process::exit(0);
                }
            }
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

        if self.visible {
            egui::CentralPanel::default().show(ctx, |ui| {
                ui.heading(format!("{} UPS Monitor — Server", icon_char(Icon::Power)));
                ui.separator();
                for s in &statuses {
                    ui.horizontal(|ui| {
                        let (ic, col) = state_icon(s.status);
                        ui.label(RichText::new(ic.to_string()).size(22.0).color(col));
                        ui.vertical(|ui| {
                            ui.label(RichText::new(&s.id).strong());
                            ui.label(format!("Status: {}", s.status));
                            if let Some(p) = s.battery_pct {
                                ui.label(format!("Battery: {p}%"));
                            }
                            if let Some(r) = s.runtime_secs {
                                ui.label(format!("Runtime: {} min", r / 60));
                            }
                            ui.small(format!(
                                "Updated {}",
                                s.last_updated.format("%H:%M:%S")
                            ));
                        });
                    });
                    ui.separator();
                }
            });
        }

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
            .with_inner_size([360.0, 420.0])
            .with_visible(false),
        ..Default::default()
    };
    eframe::run_native(
        "ups-server",
        options,
        Box::new(move |cc| Ok(Box::new(StatusApp::new(cc, state)))),
    )
}

//! The PN-Patch window: status, Install / Uninstall / Play, settings, Save.

use eframe::egui;
use pn_shared::{Config, Filter, Mode, Scale};

use crate::Paths;
use crate::install::{self, Status};

struct App {
    p: Paths,
    cfg: Config,
    status: Status,
    message: String,
}

pub fn run(p: Paths) -> eframe::Result {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([460.0, 400.0])
            .with_resizable(false),
        ..Default::default()
    };
    eframe::run_native(
        "Private Nurse — Modern Windows Patch",
        options,
        Box::new(|_cc| Ok(Box::new(App::new(p)))),
    )
}

fn load_config(p: &Paths) -> (Config, String) {
    let Some(path) = pn_shared::existing_path(Some(&p.game)) else {
        return (pn_shared::DEFAULT, "No settings file yet: defaults.".into());
    };
    match std::fs::read_to_string(&path) {
        Ok(text) => {
            let (cfg, bad) = pn_shared::parse(&text);
            let note = if bad.is_empty() {
                format!("Settings from {}", path.display())
            } else {
                format!(
                    "Settings from {} (skipped: {})",
                    path.display(),
                    bad.join("; ")
                )
            };
            (cfg, note)
        }
        Err(e) => (
            pn_shared::DEFAULT,
            format!("{} unreadable: {e}", path.display()),
        ),
    }
}

impl App {
    fn new(p: Paths) -> Self {
        let (cfg, message) = load_config(&p);
        let status = install::status(&p.game, &p.payload);
        Self {
            p,
            cfg,
            status,
            message,
        }
    }

    /// Install or uninstall, through UAC when the folder is read-only.
    fn change(&mut self, installing: bool) {
        let result = if install::writable(&self.p.game) {
            if installing {
                install::install(&self.p.game, &self.p.payload)
            } else {
                install::uninstall(&self.p.game, &self.p.payload, pn_shared::appdata_path())
            }
        } else {
            let verb = if installing {
                "--install"
            } else {
                "--uninstall"
            };
            match crate::win::run_elevated(&format!(
                "{verb} --game {}",
                crate::win::quoted(&self.p.game)
            )) {
                Ok(0) => Ok("Done (as administrator).".into()),
                Ok(code) => Err(format!("The administrator run failed (exit code {code}).")),
                Err(e) => Err(e),
            }
        };
        self.message = result.unwrap_or_else(|e| format!("Failed: {e}"));
        self.status = install::status(&self.p.game, &self.p.payload);
    }

    fn save(&mut self) {
        self.message = match install::save_target(&self.p.game) {
            None => "Failed: no writable place for the settings (%APPDATA% unset).".into(),
            Some(path) => {
                let written = path
                    .parent()
                    .map_or(Ok(()), std::fs::create_dir_all)
                    .and_then(|()| std::fs::write(&path, pn_shared::to_ini(&self.cfg)));
                match written {
                    Ok(()) => match install::shadowed_by(&self.p.game, &path) {
                        None => format!("Saved to {}", path.display()),
                        Some(active) => format!(
                            "Saved to {}, but the game reads {} instead: delete that file (as administrator) for the new settings to apply.",
                            path.display(),
                            active.display()
                        ),
                    },
                    Err(e) => format!("Failed to save {}: {e}", path.display()),
                }
            }
        };
    }

    fn play(&mut self) {
        let exe = self.p.game.join(install::GAME_EXE);
        if let Err(e) = std::process::Command::new(&exe)
            .current_dir(&self.p.game)
            .spawn()
        {
            self.message = format!("Failed to start {}: {e}", exe.display());
        }
    }

    fn status_text(&self) -> String {
        match &self.status {
            Status::NotInstalled => "Not installed.".into(),
            Status::Installed { current: true } => "Installed.".into(),
            Status::Installed { current: false } => {
                "Installed (a different version: Install updates it).".into()
            }
            Status::Broken(why) => format!("Cannot install here: {why}"),
        }
    }

    fn actions(&mut self, ui: &mut egui::Ui) {
        let installed = matches!(self.status, Status::Installed { .. });
        ui.horizontal(|ui| {
            let can_install = !matches!(
                self.status,
                Status::Broken(_) | Status::Installed { current: true }
            );
            if ui
                .add_enabled(can_install, egui::Button::new("Install"))
                .clicked()
            {
                self.change(true);
            }
            if ui
                .add_enabled(installed, egui::Button::new("Uninstall"))
                .clicked()
            {
                self.change(false);
            }
            if ui.button("Play").clicked() {
                self.play();
            }
        });
    }

    fn settings(&mut self, ui: &mut egui::Ui) {
        let c = &mut self.cfg;
        ui.checkbox(
            &mut c.enabled,
            "Presenter on (off = the game's original 640x480 mode)",
        );
        ui.add_enabled_ui(c.enabled, |ui| {
            egui::ComboBox::from_label("Mode")
                .selected_text(format!("{:?}", c.mode))
                .show_ui(ui, |ui| {
                    ui.selectable_value(&mut c.mode, Mode::Windowed, "Windowed");
                    ui.selectable_value(&mut c.mode, Mode::Fullscreen, "Fullscreen (borderless)");
                });
            egui::ComboBox::from_label("Scale")
                .selected_text(scale_name(c.scale))
                .show_ui(ui, |ui| {
                    for s in [
                        Scale::Auto,
                        Scale::Fit,
                        Scale::Preset(1),
                        Scale::Preset(2),
                        Scale::Preset(3),
                        Scale::Preset(4),
                    ] {
                        ui.selectable_value(&mut c.scale, s, scale_name(s));
                    }
                });
            egui::ComboBox::from_label("Filter")
                .selected_text(format!("{:?}", c.filter))
                .show_ui(ui, |ui| {
                    ui.selectable_value(&mut c.filter, Filter::Auto, "Auto");
                    ui.selectable_value(&mut c.filter, Filter::Nearest, "Nearest (sharp pixels)");
                    ui.selectable_value(&mut c.filter, Filter::Smooth, "Smooth");
                });
            ui.checkbox(&mut c.dpi_aware, "Sharp on high-DPI displays (recommended)");
        });
    }
}

fn scale_name(s: Scale) -> String {
    match s {
        Scale::Auto => "Auto (largest whole number)".into(),
        Scale::Fit => "Fit (fill the screen height)".into(),
        Scale::Preset(n) => format!("{n}x"),
    }
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        egui::CentralPanel::default().show(ui, |ui| {
            ui.heading("Private Nurse — Modern Windows Patch");
            ui.label(format!("Game folder: {}", self.p.game.display()));
            ui.label(self.status_text());
            self.actions(ui);
            ui.separator();
            ui.strong("Settings (used the next time the game starts)");
            self.settings(ui);
            ui.horizontal(|ui| {
                if ui.button("Save").clicked() {
                    self.save();
                }
                if ui.button("Defaults").clicked() {
                    self.cfg = pn_shared::DEFAULT;
                }
            });
            ui.separator();
            ui.label(&self.message);
        });
    }
}

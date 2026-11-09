//! The script card of the assistant: the script, what the app read in it, a dry run in the sandbox,
//! then a real run once confirmed, and a place in the library.

use std::sync::mpsc::{self, Receiver};

use egui::{RichText, ScrollArea};

use crate::actions::{Env, Level};
use crate::brand::{self, Safety};
use crate::journal;
use crate::scripts::{self, Run, Summary};

enum Stage {
    /// Waiting for the dry run.
    Written,
    /// The dry run (or the real one) is going.
    Running { dry: bool, rx: Receiver<Result<Run, String>> },
    Tried(Run),
    /// An irreversible script waiting for its second click.
    Confirming(Run),
    Ran(Run),
    Failed(String),
}

pub struct ScriptCard {
    pub purpose: String,
    pub script: String,
    summary: Summary,
    stage: Stage,
    saved: Option<String>,
}

impl ScriptCard {
    pub fn new(purpose: String, script: String, saved: Option<String>) -> Self {
        let summary = scripts::summarize(&script);
        ScriptCard { purpose, script, summary, stage: Stage::Written, saved }
    }

    fn start(&mut self, ctx: &egui::Context, dry: bool) {
        let (tx, rx) = mpsc::channel();
        let script = self.script.clone();
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let _ = tx.send(scripts::run(&script, dry));
            ctx.request_repaint();
        });
        self.stage = Stage::Running { dry, rx };
    }

    fn poll(&mut self, env: Option<&Env>) {
        let Stage::Running { dry, rx } = &self.stage else { return };
        let dry = *dry;
        let Ok(result) = rx.try_recv() else { return };
        self.stage = match result {
            Ok(run) if dry => Stage::Tried(run),
            Ok(run) => {
                if let Some(env) = env {
                    journal::record_script(env, journal::Origin::Script, &self.purpose, &self.script, &run);
                }
                Stage::Ran(run)
            }
            Err(e) => Stage::Failed(e),
        };
    }

    pub fn show(&mut self, ui: &mut egui::Ui, env: Option<&Env>) {
        self.poll(env);
        let level = self.summary.level();
        ui.horizontal(|ui| {
            let color = if level == Level::Irreversible { ui.visuals().error_fg_color } else { ui.visuals().weak_text_color() };
            ui.label(RichText::new(level.label()).small().color(color));
            ui.label(RichText::new(&self.purpose).strong());
        });
        ScrollArea::vertical().id_salt("script").max_height(160.0).show(ui, |ui| {
            ui.add(egui::TextEdit::multiline(&mut self.script.as_str()).code_editor().desired_width(f32::INFINITY));
        });
        let s = &self.summary;
        let list = |v: &[String]| if v.is_empty() { "aucun".to_string() } else { v.join(", ") };
        ui.label(RichText::new(format!("Commandes : {}", list(&s.commands))).weak());
        ui.label(RichText::new(format!("Écrit dans : {}", list(&s.writes))).weak());
        let mut flags = Vec::new();
        if s.sudo {
            flags.push("demande sudo");
        }
        if s.deletes {
            flags.push("supprime définitivement");
        }
        if s.network {
            flags.push("utilise le réseau");
        }
        if !flags.is_empty() {
            ui.horizontal(|ui| {
                brand::safety_icon(ui, Safety::Risky);
                ui.label(flags.join(" ; "));
            });
        }
        ui.label(RichText::new("Lecture mot à mot : ce que le script construit en s'exécutant n'apparaît qu'à l'essai à blanc.").small().weak());
        ui.add_space(4.0);

        let mut next = None;
        match &self.stage {
            Stage::Written => {
                if ui.button("Essai à blanc").on_hover_text("Dans un bac à sable : ni écriture, ni réseau, ni service système.").clicked() {
                    self.start(ui.ctx(), true);
                }
            }
            Stage::Running { dry, .. } => {
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.label(if *dry { "Essai à blanc en cours…" } else { "Exécution en cours…" });
                });
            }
            Stage::Tried(run) | Stage::Confirming(run) => {
                output(ui, "Ce qu'il aurait fait (essai à blanc)", run);
                if s.sudo {
                    ui.horizontal(|ui| {
                        brand::safety_icon(ui, Safety::Risky);
                        ui.label("Ce script demande sudo : Mac Anal ne le lance pas. Copie-le dans le Terminal si tu le veux vraiment.");
                    });
                } else if matches!(self.stage, Stage::Confirming(_)) {
                    ui.horizontal(|ui| {
                        brand::safety_icon(ui, Safety::Risky);
                        ui.label("Ce script modifie le Mac et ne peut pas être annulé.");
                    });
                    ui.horizontal(|ui| {
                        if ui.button("Confirmer").clicked() {
                            next = Some(true);
                        }
                        if ui.button("Renoncer").clicked() {
                            self.stage = Stage::Written;
                        }
                    });
                } else if ui.button(if level == Level::Irreversible { "Lancer pour de vrai…" } else { "Lancer pour de vrai" }).clicked() {
                    if level == Level::Irreversible {
                        if let Stage::Tried(run) = &self.stage {
                            self.stage = Stage::Confirming(run.clone());
                        }
                    } else {
                        next = Some(true);
                    }
                }
            }
            Stage::Ran(run) => output(ui, "Résultat", run),
            Stage::Failed(e) => {
                ui.colored_label(ui.visuals().error_fg_color, e);
            }
        }
        if next.is_some() {
            self.start(ui.ctx(), false);
        }

        ui.add_space(4.0);
        match (&self.saved, env) {
            (Some(name), _) => {
                ui.label(RichText::new(format!("Dans la bibliothèque : {name}")).small().weak());
            }
            (None, Some(env)) => {
                if ui.small_button("Enregistrer dans la bibliothèque").clicked() {
                    match scripts::save(env, &self.purpose, &self.script, journal::now()) {
                        Ok(saved) => self.saved = Some(saved.name),
                        Err(e) => self.stage = Stage::Failed(e),
                    }
                }
            }
            (None, None) => {}
        }
    }
}

fn output(ui: &mut egui::Ui, title: &str, run: &Run) {
    let status = match (run.timed_out, run.status) {
        (true, _) => "arrêté au bout de 60 s".to_string(),
        (false, Some(0)) => "terminé sans erreur".to_string(),
        (false, Some(c)) => format!("code de sortie {c}"),
        (false, None) => "interrompu".to_string(),
    };
    ui.label(RichText::new(format!("{title} : {status}")).strong());
    ScrollArea::vertical().id_salt(title).max_height(120.0).show(ui, |ui| {
        let text = if run.output.trim().is_empty() { "(aucune sortie)" } else { run.output.as_str() };
        ui.label(RichText::new(text).monospace().small());
    });
}

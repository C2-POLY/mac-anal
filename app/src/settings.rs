//! What the user chose, kept in `~/Library/Application Support/mac-anal/settings.json`.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::actions::Env;

/// A widget of the menu bar.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Widget {
    Disk,
    Memory,
    Cpu,
    Gpu,
    Network,
    Temperature,
    Fan,
    Battery,
    Swap,
}

impl Widget {
    pub const ALL: [Widget; 9] = [Widget::Disk, Widget::Memory, Widget::Cpu, Widget::Gpu, Widget::Network, Widget::Temperature, Widget::Fan, Widget::Battery, Widget::Swap];

    pub fn name(self) -> &'static str {
        match self {
            Widget::Disk => "Disque",
            Widget::Memory => "Mémoire",
            Widget::Cpu => "Processeur",
            Widget::Gpu => "GPU",
            Widget::Network => "Réseau",
            Widget::Temperature => "Température",
            Widget::Fan => "Ventilateur",
            Widget::Battery => "Batterie",
            Widget::Swap => "Swap",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    /// The widgets shown in the menu bar, in order, left to right.
    pub widgets: Vec<Widget>,
}

impl Default for Settings {
    fn default() -> Self {
        Settings { widgets: vec![Widget::Disk] }
    }
}

impl Settings {
    pub fn path(env: &Env) -> PathBuf {
        env.home.join("Library/Application Support/mac-anal/settings.json")
    }

    /// The saved settings, or the defaults when there are none or they cannot be read.
    pub fn load(path: &Path) -> Self {
        let mut s: Settings = std::fs::read(path).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default();
        let mut seen = Vec::new();
        s.widgets.retain(|w| !seen.contains(w) && {
            seen.push(*w);
            true
        });
        s
    }

    pub fn save(&self, path: &Path) -> Result<(), String> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(|e| format!("{} : {e}", dir.display()))?;
        }
        let json = serde_json::to_vec_pretty(self).map_err(|e| e.to_string())?;
        // Written aside then renamed, so a crash never leaves half a file.
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, json).map_err(|e| format!("{} : {e}", tmp.display()))?;
        std::fs::rename(&tmp, path).map_err(|e| format!("{} : {e}", path.display()))
    }

    pub fn toggle(&mut self, w: Widget) {
        if let Some(i) = self.widgets.iter().position(|x| *x == w) {
            self.widgets.remove(i);
        } else {
            self.widgets.push(w);
        }
    }

    /// Moves a shown widget one place left (`-1`) or right (`1`).
    pub fn shift(&mut self, w: Widget, by: isize) {
        let Some(i) = self.widgets.iter().position(|x| *x == w) else { return };
        let j = i as isize + by;
        if j >= 0 && (j as usize) < self.widgets.len() {
            self.widgets.swap(i, j as usize);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_or_broken_files_give_the_defaults() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");
        assert_eq!(Settings::load(&path), Settings::default());
        std::fs::write(&path, "{ not json").unwrap();
        assert_eq!(Settings::load(&path), Settings::default());
        std::fs::write(&path, r#"{"widgets":["disk","toaster"]}"#).unwrap();
        assert_eq!(Settings::load(&path), Settings::default());
    }

    #[test]
    fn settings_survive_a_save() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a/b/settings.json");
        let s = Settings { widgets: vec![Widget::Cpu, Widget::Disk] };
        s.save(&path).unwrap();
        assert_eq!(Settings::load(&path), s);
        std::fs::write(&path, r#"{"widgets":["cpu","cpu","memory"]}"#).unwrap();
        assert_eq!(Settings::load(&path).widgets, vec![Widget::Cpu, Widget::Memory]);
    }

    #[test]
    fn toggling_and_shifting_keep_the_order() {
        let mut s = Settings { widgets: vec![Widget::Disk, Widget::Memory] };
        s.toggle(Widget::Cpu);
        assert_eq!(s.widgets, vec![Widget::Disk, Widget::Memory, Widget::Cpu]);
        s.shift(Widget::Cpu, -1);
        assert_eq!(s.widgets, vec![Widget::Disk, Widget::Cpu, Widget::Memory]);
        s.shift(Widget::Disk, -1);
        assert_eq!(s.widgets[0], Widget::Disk);
        s.toggle(Widget::Disk);
        assert_eq!(s.widgets, vec![Widget::Cpu, Widget::Memory]);
    }
}

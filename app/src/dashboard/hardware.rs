//! What is plugged in and when Time Machine last ran. `system_profiler` takes seconds, so this is
//! read once a minute on its own thread, never with the fast readings.

use serde_json::Value;

use super::collect;

/// The slow reading.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Hardware {
    pub displays: Vec<Display>,
    /// USB devices by name, hubs included.
    pub usb: Vec<String>,
    /// Connected Bluetooth devices: name and kind (« Mouse », « Headphones »).
    pub bluetooth: Vec<(String, Option<String>)>,
    pub backup: Backup,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Display {
    pub name: String,
    /// « 2560 x 1440 @ 60.00Hz ».
    pub resolution: Option<String>,
    pub main: bool,
}

/// The last Time Machine backup.
#[derive(Clone, Debug, Default, PartialEq)]
pub enum Backup {
    /// Its date, « 2026-11-08 14:32 ».
    Last(String),
    /// No backup was ever made.
    Never,
    /// `tmutil` could not tell: destination unmounted, no access.
    Unavailable(String),
    /// Not read yet.
    #[default]
    Unknown,
}

impl Hardware {
    pub fn sample() -> Self {
        Self {
            displays: vec![
                Display { name: "Studio Display".into(), resolution: Some("2560 x 1440 @ 60.00Hz".into()), main: true },
                Display { name: "Écran portable".into(), resolution: Some("1600 x 2560 @ 60.00Hz".into()), main: false },
            ],
            usb: vec!["Clavier USB".into(), "Hub USB-C".into(), "Disque externe".into()],
            bluetooth: vec![("Souris".into(), Some("Mouse".into())), ("Casque".into(), Some("Headphones".into()))],
            backup: Backup::Last("2026-11-09 08:14".into()),
        }
    }
}

/// Reads everything: about five seconds, so never on the window's thread.
pub fn read() -> Hardware {
    let profile = collect::output("/usr/sbin/system_profiler", &["-json", "-detailLevel", "mini", "SPDisplaysDataType", "SPUSBDataType", "SPUSBHostDataType", "SPBluetoothDataType"])
        .and_then(|s| serde_json::from_str::<Value>(&s).ok())
        .unwrap_or(Value::Null);
    let backup = match collect::run("/usr/bin/tmutil", &["latestbackup"]) {
        Some(r) => parse_backup(&r.stdout, &r.stderr),
        None => Backup::Unavailable("tmutil ne répond pas".into()),
    };
    Hardware { displays: displays(&profile), usb: usb(&profile), bluetooth: bluetooth(&profile), backup }
}

fn text(v: &Value, key: &str) -> Option<String> {
    v.get(key).and_then(Value::as_str).map(str::to_string)
}

fn items<'a>(v: &'a Value, key: &str) -> &'a [Value] {
    v.get(key).and_then(Value::as_array).map(Vec::as_slice).unwrap_or(&[])
}

/// The screens of every graphics processor.
pub fn displays(profile: &Value) -> Vec<Display> {
    items(profile, "SPDisplaysDataType")
        .iter()
        .flat_map(|gpu| items(gpu, "spdisplays_ndrvs"))
        .filter_map(|d| {
            Some(Display {
                name: text(d, "_name")?,
                resolution: text(d, "_spdisplays_resolution").or_else(|| text(d, "spdisplays_resolution")),
                main: text(d, "spdisplays_main").as_deref() == Some("spdisplays_yes"),
            })
        })
        .collect()
}

/// The USB devices, under the host controllers. macOS 26 fills `SPUSBHostDataType`, older
/// versions `SPUSBDataType`: both are read.
pub fn usb(profile: &Value) -> Vec<String> {
    fn walk(list: &[Value], out: &mut Vec<String>) {
        for item in list {
            if let Some(name) = text(item, "_name") {
                out.push(name);
            }
            walk(items(item, "_items"), out);
        }
    }
    let mut out = Vec::new();
    for key in ["SPUSBHostDataType", "SPUSBDataType"] {
        for controller in items(profile, key) {
            walk(items(controller, "_items"), &mut out);
        }
    }
    out.dedup();
    out
}

/// The connected Bluetooth devices. Each entry of `device_connected` is a one-key object, the key
/// being the device's name.
pub fn bluetooth(profile: &Value) -> Vec<(String, Option<String>)> {
    items(profile, "SPBluetoothDataType")
        .iter()
        .flat_map(|c| items(c, "device_connected"))
        .filter_map(Value::as_object)
        .flat_map(|o| o.iter().map(|(name, d)| (name.clone(), text(d, "device_minorType"))))
        .collect()
}

/// `tmutil latestbackup` prints the path of the last backup, whose name holds its date
/// (`2026-11-08-143205`), or says on stderr why it cannot.
pub fn parse_backup(stdout: &str, stderr: &str) -> Backup {
    let line = stdout.lines().map(str::trim).find(|l| !l.is_empty());
    if let Some(date) = line.and_then(backup_date) {
        return Backup::Last(date);
    }
    let err = stderr.lines().map(str::trim).find(|l| !l.is_empty()).or(line);
    match err {
        None => Backup::Never,
        Some(e) if e.contains("No machine directory") || e.contains("No backups") => Backup::Never,
        Some(e) if e.contains("mount") => Backup::Unavailable("destination indisponible".into()),
        Some(e) if e.contains("Full Disk Access") || e.contains("permission") => Backup::Unavailable("accès complet au disque requis".into()),
        Some(e) => Backup::Unavailable(e.chars().take(80).collect()),
    }
}

/// « 2026-11-08 14:32 » from a path ending in `2026-11-08-143205` (with or without `.backup`).
fn backup_date(path: &str) -> Option<String> {
    let name = path.rsplit('/').next()?.trim_end_matches(".backup");
    let (date, time) = (name.get(..10)?, name.get(11..15)?);
    let ok = date.bytes().enumerate().all(|(i, b)| if i == 4 || i == 7 { b == b'-' } else { b.is_ascii_digit() }) && time.bytes().all(|b| b.is_ascii_digit());
    ok.then(|| format!("{date} {}:{}", &time[..2], &time[2..]))
}

#[cfg(test)]
mod tests {
    use super::*;

    const PROFILE: &str = r#"{
      "SPDisplaysDataType": [{ "_name": "GPU", "spdisplays_ndrvs": [
        { "_name": "Écran A", "_spdisplays_resolution": "2560 x 1440 @ 60.00Hz", "spdisplays_main": "spdisplays_yes" },
        { "_name": "Écran B", "spdisplays_resolution": "1600 x 2560 @ 60.00Hz" } ] }],
      "SPUSBDataType": [],
      "SPUSBHostDataType": [{ "_name": "Bus", "_items": [
        { "_name": "Hub", "_items": [ { "_name": "Clavier" } ] },
        { "_name": "Disque" } ] }],
      "SPBluetoothDataType": [{ "controller_properties": {},
        "device_connected": [ { "Souris": { "device_minorType": "Mouse" } }, { "Montre": {} } ],
        "device_not_connected": [ { "Casque": { "device_minorType": "Headphones" } } ] }]
    }"#;

    #[test]
    fn displays_usb_and_bluetooth_are_read_from_system_profiler() {
        let p: Value = serde_json::from_str(PROFILE).unwrap();
        let d = displays(&p);
        assert_eq!(d.len(), 2);
        assert!(d[0].main && !d[1].main);
        assert_eq!(d[1].resolution.as_deref(), Some("1600 x 2560 @ 60.00Hz"));
        assert_eq!(usb(&p), ["Hub", "Clavier", "Disque"]);
        assert_eq!(bluetooth(&p), [("Souris".to_string(), Some("Mouse".to_string())), ("Montre".to_string(), None)]);
    }

    #[test]
    fn a_missing_or_broken_profile_gives_empty_lists() {
        for p in [Value::Null, serde_json::json!({ "SPDisplaysDataType": "?" })] {
            assert!(displays(&p).is_empty() && usb(&p).is_empty() && bluetooth(&p).is_empty());
        }
    }

    #[test]
    fn the_last_backup_is_read_from_tmutil() {
        assert_eq!(parse_backup("/Volumes/TM/Backups.backupdb/Mac/2026-11-08-143205\n", ""), Backup::Last("2026-11-08 14:32".into()));
        assert_eq!(parse_backup("/Volumes/.timemachine/ABC/2026-11-08-091500.backup/2026-11-08-091500.backup\n", ""), Backup::Last("2026-11-08 09:15".into()));
        assert_eq!(parse_backup("", ""), Backup::Never);
        assert_eq!(parse_backup("", "No machine directory found for host.\n"), Backup::Never);
        assert_eq!(
            parse_backup("", "Failed to mount backup destination, error: Error Domain=com.apple.backupd.ErrorDomain Code=17\n"),
            Backup::Unavailable("destination indisponible".into())
        );
        assert!(matches!(parse_backup("", "something odd"), Backup::Unavailable(_)));
    }
}

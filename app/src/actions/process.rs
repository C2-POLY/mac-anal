//! `quit_process`: a process of the user is asked to quit (SIGTERM). Irreversible: unsaved work may
//! be lost, so it never runs without a confirmation.

use std::path::Path;
use std::process::Command;

use super::Done;

pub fn prepare(pid: u32) -> Result<(String, String, String), String> {
    let name = check(pid)?;
    let preview = format!(
        "Demande au processus {pid} ({name}) de quitter. Le travail qu'il n'a pas enregistré peut être perdu ; cela ne s'annule pas."
    );
    Ok((format!("Quitter {name}"), preview, name))
}

/// Quits only if `pid` is still the process the preview showed: a pid can be reused meanwhile.
pub fn execute(pid: u32, expected: &str) -> Result<Done, String> {
    let name = check(pid)?;
    if name != expected {
        return Err(format!("Le processus {pid} est maintenant {name}, plus {expected} : rien n'a été fait."));
    }
    let pid_t = libc::pid_t::try_from(pid).map_err(|_| format!("Aucun processus {pid}."))?;
    // SAFETY: kill only sends a signal; check() refused launchd, mac-anal and other users' processes.
    if unsafe { libc::kill(pid_t, libc::SIGTERM) } != 0 {
        return Err(format!("Le processus {pid} n'a pas pu être quitté : {}", std::io::Error::last_os_error()));
    }
    Ok(Done { summary: format!("{expected} ({pid}) a reçu la demande de quitter."), undo: None })
}

/// The process name, if mac-anal may quit it: not launchd, not itself, and owned by the user.
fn check(pid: u32) -> Result<String, String> {
    if pid <= 1 || pid == std::process::id() {
        return Err(format!("Le processus {pid} ne peut pas être quitté par mac-anal."));
    }
    let (uid, name) = owner_and_name(pid).ok_or_else(|| format!("Aucun processus {pid}."))?;
    // SAFETY: getuid has no precondition.
    if uid != unsafe { libc::getuid() } {
        return Err(format!("Le processus {pid} ({name}) n'appartient pas à votre session."));
    }
    Ok(name)
}

fn owner_and_name(pid: u32) -> Option<(u32, String)> {
    let out = Command::new("ps").args(["-p", &pid.to_string(), "-o", "uid=,comm="]).output().ok()?;
    let line = String::from_utf8_lossy(&out.stdout).trim().to_string();
    let (uid, comm) = line.split_once(char::is_whitespace)?;
    let comm = comm.trim();
    let name = Path::new(comm).file_name().map_or_else(|| comm.to_string(), |n| n.to_string_lossy().into_owned());
    Some((uid.trim().parse().ok()?, name))
}

#[cfg(test)]
mod tests {
    use std::process::Command;

    use crate::actions::tests::home;
    use crate::actions::*;

    #[test]
    fn launchd_and_mac_anal_are_refused() {
        let (_d, env) = home();
        assert!(prepare(&env, Action::QuitProcess { pid: 0 }).is_err());
        assert!(prepare(&env, Action::QuitProcess { pid: 1 }).is_err());
        assert!(prepare(&env, Action::QuitProcess { pid: std::process::id() }).is_err());
        assert!(prepare(&env, Action::QuitProcess { pid: u32::MAX }).is_err());
    }

    #[test]
    fn quitting_needs_a_confirmation() {
        let (_d, env) = home();
        let mut child = Command::new("sleep").arg("30").spawn().unwrap();
        let p = prepare(&env, Action::QuitProcess { pid: child.id() }).unwrap();
        assert_eq!(p.level, Level::Irreversible);
        assert_eq!(p.title, "Quitter sleep");

        assert!(execute(&env, &p, false).is_err());
        std::thread::sleep(std::time::Duration::from_millis(100));
        assert!(child.try_wait().unwrap().is_none(), "ran without confirmation");

        execute(&env, &p, true).unwrap();
        let status = child.wait().unwrap();
        assert!(!status.success());
    }

    #[test]
    fn a_process_gone_since_the_preview_is_not_signalled() {
        let (_d, env) = home();
        let mut child = Command::new("sleep").arg("30").spawn().unwrap();
        let p = prepare(&env, Action::QuitProcess { pid: child.id() }).unwrap();
        child.kill().unwrap();
        child.wait().unwrap();
        assert!(execute(&env, &p, true).is_err());
    }
}

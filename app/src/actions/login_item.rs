//! `disable_login_item`: a user LaunchAgent is set aside in mac-anal's folder, and can be put back.

use super::{free_path, move_to, plain_name, Done, Env, Undo};

pub fn prepare(env: &Env, label: &str) -> Result<(String, String, String), String> {
    plain_name(label)?;
    let plist = env.launch_agents().join(format!("{label}.plist"));
    if !std::fs::symlink_metadata(&plist).is_ok_and(|m| m.is_file()) {
        return Err(format!("Aucun élément de démarrage « {label} »."));
    }
    let preview = format!(
        "Met {} de côté dans {} : il ne se lancera plus à la prochaine ouverture de session. S'il tourne, il continue jusque-là.",
        plist.display(),
        env.disabled_items().display()
    );
    Ok((format!("Désactiver {label} au démarrage"), preview, plist.display().to_string()))
}

pub fn execute(env: &Env, label: &str) -> Result<Done, String> {
    let file = format!("{label}.plist");
    let from = env.launch_agents().join(&file);
    let to = free_path(&env.disabled_items(), &file);
    move_to(&from, &to)?;
    Ok(Done { summary: format!("{label} ne se lancera plus au démarrage."), undo: Some(Undo { moves: vec![(from, to)] }) })
}

#[cfg(test)]
mod tests {
    use crate::actions::tests::home;
    use crate::actions::*;

    #[test]
    fn a_login_item_is_set_aside_and_put_back() {
        let (_d, env) = home();
        std::fs::create_dir_all(env.launch_agents()).unwrap();
        let plist = env.launch_agents().join("com.example.agent.plist");
        std::fs::write(&plist, "<plist/>").unwrap();

        let p = prepare(&env, Action::DisableLoginItem { label: "com.example.agent".into() }).unwrap();
        assert_eq!(p.level, Level::Reversible);
        let done = execute(&env, &p, false).unwrap();
        assert!(!plist.exists());
        assert!(env.disabled_items().join("com.example.agent.plist").is_file());

        undo(done.undo.as_ref().unwrap()).unwrap();
        assert_eq!(std::fs::read_to_string(&plist).unwrap(), "<plist/>");
        assert!(prepare(&env, Action::DisableLoginItem { label: "missing".into() }).is_err());
    }
}

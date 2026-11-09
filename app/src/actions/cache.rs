//! `clear_cache`: a folder directly under `~/Library/Caches` goes to the Trash, and can come back.

use super::{folder_size, free_path, move_to, plain_name, Done, Env, Undo};
use crate::overview::format_bytes;

pub fn prepare(env: &Env, name: &str) -> Result<(String, String, String), String> {
    plain_name(name)?;
    let dir = env.caches().join(name);
    // A real folder only: a link would send its target's name to the Trash, not the cache.
    if !std::fs::symlink_metadata(&dir).is_ok_and(|m| m.is_dir()) {
        return Err(format!("Le cache « {name} » n'existe pas."));
    }
    let (bytes, files) = folder_size(&dir);
    let preview = format!(
        "Envoie {} à la Corbeille : {} fichier{}, {} libérés une fois la Corbeille vidée. L'app qui s'en sert le recrée au besoin.",
        dir.display(),
        files,
        if files > 1 { "s" } else { "" },
        format_bytes(bytes)
    );
    Ok((format!("Vider le cache {name}"), preview, dir.display().to_string()))
}

pub fn execute(env: &Env, name: &str) -> Result<Done, String> {
    let from = env.caches().join(name);
    let to = free_path(&env.trash(), name);
    move_to(&from, &to)?;
    Ok(Done { summary: format!("{name} est dans la Corbeille."), undo: Some(Undo { moves: vec![(from, to)] }) })
}

#[cfg(test)]
mod tests {
    use crate::actions::tests::home;
    use crate::actions::*;

    #[test]
    fn a_cache_goes_to_the_trash_and_comes_back() {
        let (_d, env) = home();
        let cache = env.caches().join("com.example.app");
        std::fs::create_dir_all(cache.join("sub")).unwrap();
        std::fs::write(cache.join("sub/blob"), vec![0u8; 2048]).unwrap();

        let p = prepare(&env, Action::ClearCache { name: "com.example.app".into() }).unwrap();
        assert_eq!(p.level, Level::Reversible);
        assert!(p.preview.contains("1 fichier,"), "{}", p.preview);
        assert!(p.preview.contains("2,0 Ko"), "{}", p.preview);

        let done = execute(&env, &p, false).unwrap();
        assert!(!cache.exists());
        assert!(env.trash().join("com.example.app/sub/blob").is_file());

        undo(done.undo.as_ref().unwrap()).unwrap();
        assert!(cache.join("sub/blob").is_file());
        assert!(!env.trash().join("com.example.app").exists());
    }

    #[test]
    fn a_name_already_in_the_trash_is_kept() {
        let (_d, env) = home();
        std::fs::create_dir_all(env.caches().join("c")).unwrap();
        std::fs::create_dir_all(env.trash().join("c")).unwrap();
        let p = prepare(&env, Action::ClearCache { name: "c".into() }).unwrap();
        execute(&env, &p, false).unwrap();
        assert!(env.trash().join("c").is_dir());
        assert_eq!(std::fs::read_dir(env.trash()).unwrap().count(), 2);
    }

    #[test]
    fn a_missing_cache_a_file_or_a_link_is_refused() {
        let (d, env) = home();
        std::fs::create_dir_all(env.caches()).unwrap();
        std::fs::write(env.caches().join("file"), "x").unwrap();
        std::os::unix::fs::symlink(d.path(), env.caches().join("link")).unwrap();
        for name in ["missing", "file", "link"] {
            assert!(prepare(&env, Action::ClearCache { name: name.into() }).is_err(), "{name}");
        }
    }
}

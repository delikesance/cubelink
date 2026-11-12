//! Commandes de gestion d'instance : arrêt du jeu, suppression, paramètres, dossier, logs.

use std::io::{Read, Seek, SeekFrom};
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use core_lib::types::Instance;
use serde::Serialize;
use tauri::State;

use crate::commands_instances::valid_id;
use crate::error::user_msg;
use crate::state::{lock, AppState};

/// Taille max renvoyée par `instances_logs`.
pub const MAX_LOG_BYTES: u64 = 512 * 1024;
/// Les lignes de sortie du jeu sont regroupées : 10 évènements `launch://log` par seconde au plus.
pub const LOG_FLUSH_EVERY: Duration = Duration::from_millis(100);
/// Borne d'un lot (lignes les plus anciennes abandonnées au-delà).
const MAX_BATCH_LINES: usize = 500;
const MAX_LINE_CHARS: usize = 4000;

#[derive(Serialize)]
struct LogEvent<'a> {
    id: &'a str,
    line: &'a str,
}

/// Accumule les lignes du jeu entre deux envois.
#[derive(Default)]
pub struct LogBatcher {
    lines: Vec<String>,
}

impl LogBatcher {
    pub fn push(&mut self, line: &str) {
        if self.lines.len() >= MAX_BATCH_LINES {
            self.lines.remove(0);
        }
        let l = if line.chars().count() > MAX_LINE_CHARS { line.chars().take(MAX_LINE_CHARS).collect() } else { line.to_string() };
        self.lines.push(l);
    }

    /// Lignes accumulées, jointes par `\n` ; `None` si vide.
    pub fn take(&mut self) -> Option<String> {
        if self.lines.is_empty() {
            return None;
        }
        let out = self.lines.join("\n");
        self.lines.clear();
        Some(out)
    }
}

/// Fin du fichier (max `max` octets), coupée sur une ligne entière.
pub fn tail_file(path: &Path, max: u64) -> std::io::Result<String> {
    let mut f = std::fs::File::open(path)?;
    let len = f.metadata()?.len();
    let start = len.saturating_sub(max);
    f.seek(SeekFrom::Start(start))?;
    let mut buf = Vec::new();
    f.take(max).read_to_end(&mut buf)?;
    if start > 0 {
        // Premier morceau probablement coupé en milieu de ligne : on le jette.
        match buf.iter().position(|&b| b == b'\n') {
            Some(i) => buf.drain(..=i).for_each(drop),
            None => buf.clear(),
        }
    }
    Ok(String::from_utf8_lossy(&buf).into_owned())
}

fn check_id(id: &str) -> Result<(), String> {
    if valid_id(id) {
        Ok(())
    } else {
        Err("identifiant d'instance invalide".into())
    }
}

impl AppState {
    pub(crate) fn emit_logs(&self, id: &str, batch: &mut LogBatcher) {
        if let Some(line) = batch.take() {
            self.emit_event("launch://log", &LogEvent { id, line: &line });
        }
    }

    /// Arrête le jeu de cette instance. L'évènement `launch://state` passe à `exited` via la boucle de lancement.
    pub async fn instances_kill(&self, id: &str) -> Result<(), String> {
        check_id(id)?;
        let handle = {
            let h = lock(&self.game_handle);
            match h.as_ref() {
                Some((gid, g)) if gid == id => Some(g.clone()),
                _ => None,
            }
        };
        match handle {
            Some(g) => g.kill().await.map_err(|e| user_msg(&e)),
            None if lock(&self.game).as_deref() == Some(id) => {
                Err("le jeu est encore en préparation : réessayez dans un instant".into())
            }
            None => Err("cette instance n'est pas en cours d'exécution".into()),
        }
    }

    pub fn instances_delete(&self, id: &str) -> Result<(), String> {
        check_id(id)?;
        if lock(&self.game).as_deref() == Some(id) {
            return Err("impossible de supprimer une instance en cours de lancement ou de jeu".into());
        }
        self.instances.delete(id).map_err(|e| user_msg(&e))?;
        // Nettoyage best effort ; les fichiers du magasin partagé restent au GC.
        let _ = std::fs::remove_dir_all(self.paths.logs.join(id));
        let _ = std::fs::remove_dir_all(self.paths.natives.join(id));
        Ok(())
    }

    pub fn instances_update(
        &self,
        id: &str,
        name: Option<String>,
        ram_mb: Option<u32>,
        java_path: Option<String>,
        jvm_args: Option<String>,
    ) -> Result<Instance, String> {
        check_id(id)?;
        self.instances
            .update(id, name.as_deref(), ram_mb, java_path.as_deref(), jvm_args.as_deref())
            .map_err(|e| user_msg(&e))
    }

    pub fn instances_open_folder(&self, id: &str) -> Result<(), String> {
        check_id(id)?;
        self.instances.get(id).map_err(|e| user_msg(&e))?;
        let dir = self.instances.game_dir(id).map_err(|e| user_msg(&e))?;
        std::fs::create_dir_all(&dir).map_err(|_| "impossible de créer le dossier de jeu".to_string())?;
        if crate::test_hooks::hooks().test_mode {
            return Ok(());
        }
        let program = if cfg!(windows) {
            "explorer"
        } else if cfg!(target_os = "macos") {
            "open"
        } else {
            "xdg-open"
        };
        std::process::Command::new(program)
            .arg(&dir)
            .spawn()
            .map(|_| ())
            .map_err(|_| "impossible d'ouvrir le dossier".to_string())
    }

    /// Fin du dernier log (`latest.log`, sinon sa rotation `.old`) ; chaîne vide s'il n'y en a pas.
    pub fn instances_logs(&self, id: &str) -> Result<String, String> {
        check_id(id)?;
        let dir = self.paths.logs.join(id);
        for name in ["latest.log", "latest.log.old"] {
            let p = dir.join(name);
            if p.is_file() {
                return tail_file(&p, MAX_LOG_BYTES).map_err(|_| "impossible de lire les logs".to_string());
            }
        }
        Ok(String::new())
    }
}

/// Délai maximal pour que le jeu en cours se ferme (kill propre 5 s, puis forcé).
pub const GAME_STOP_TIMEOUT: Duration = Duration::from_secs(30);
/// Fermeture propre d'un jeu dans la zone integree : on laisse le temps d'enregistrer un gros monde, sans jamais forcer.
pub const STAGE_CLOSE_TIMEOUT: Duration = Duration::from_secs(180);
/// Délai maximal du téléchargement du monde d'un ami avant d'abandonner l'attente.
const JOIN_WAIT_TIMEOUT: Duration = Duration::from_secs(2 * 3600);

impl AppState {
    /// Ferme le jeu en cours (s'il y en a un) et attend la libération du verrou `game` (fin de la tâche de jeu,
    /// après l'événement `launch://state` `exited`). Un jeu encore en préparation est attendu puis fermé.
    pub async fn stop_game_and_wait(&self, timeout: Duration) -> Result<(), String> {
        let deadline = tokio::time::Instant::now() + timeout;
        let mut killed = false;
        let mut retry_at = tokio::time::Instant::now();
        loop {
            let Some(id) = lock(&self.game).clone() else { return Ok(()) };
            let now = tokio::time::Instant::now();
            if !killed && now >= retry_at {
                let has_handle = lock(&self.game_handle).as_ref().is_some_and(|(g, _)| *g == id);
                if has_handle {
                    match self.instances_kill(&id).await {
                        Ok(()) => killed = true,
                        // Le jeu a pu se terminer entre-temps : la boucle le constate. Sinon on réessaie.
                        Err(_) => retry_at = now + Duration::from_millis(500),
                    }
                }
            }
            if now >= deadline {
                return Err("le jeu ne se ferme pas : fermez-le à la main puis réessayez".into());
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    }

    /// Ferme le jeu en cours avant de lancer `next_title`. Avec la zone de jeu integree : fermeture PROPRE (comme la croix de la
    /// fenetre : le monde est quitte et enregistre, Java s'arrete tout seul), barre de chargement de 0 a 50 %, jamais de kill ;
    /// si le jeu ne s'est pas ferme dans `STAGE_CLOSE_TIMEOUT`, on renonce et le jeu reste ouvert. Sinon : `stop_game_and_wait`.
    pub async fn close_game_for_switch(&self, next_title: &str) -> Result<(), String> {
        let attached = self.stage.enabled() && lock(&self.game_handle).is_some();
        if !attached {
            return self.stop_game_and_wait(GAME_STOP_TIMEOUT).await;
        }
        self.stage.begin_close(next_title);
        self.stage.request_close();
        let deadline = tokio::time::Instant::now() + STAGE_CLOSE_TIMEOUT;
        while lock(&self.game).is_some() {
            if tokio::time::Instant::now() >= deadline {
                self.stage.cancel_close();
                return Err("le jeu ne s'est pas fermé à temps (il n'a pas été forcé) : fermez-le depuis le jeu puis réessayez".into());
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        Ok(())
    }

    /// Change de modpack : ferme le jeu en cours (la confirmation est à la charge de l'interface), attend sa fin,
    /// puis lance `instance_id`. Les vérifications (instance, compte) passent AVANT la fermeture du jeu.
    pub async fn game_switch(self: &Arc<Self>, instance_id: &str) -> Result<(), String> {
        check_id(instance_id)?;
        self.instances.get(instance_id).map_err(|e| user_msg(&e))?;
        if self.auth_status_refresh().await.is_none() {
            return Err("connectez-vous avant de lancer le jeu".into());
        }
        let title = self.instances.get(instance_id).map(|i| i.name).unwrap_or_default();
        self.close_game_for_switch(&title).await?;
        self.launch_with(instance_id, None).await
    }

    /// Rejoint le monde d'un ami depuis n'importe où (p. ex. le panneau en jeu). Enchaîne `join_plan` (échec
    /// rapide : ami hors ligne ou rien d'ouvert, le jeu en cours n'est alors pas touché), `join_start` (sauf si
    /// `instance_id`, une instance invitée déjà créée, est fournie), fermeture du jeu en cours, puis `join_play`.
    /// Le jeu actuel reste ouvert pendant le téléchargement ; la progression passe par `transfer://progress`.
    pub async fn game_join_friend(self: &Arc<Self>, public_key: &str, instance_id: Option<&str>) -> Result<(), String> {
        self.check_friend(public_key)?;
        if let Some(id) = instance_id {
            check_id(id)?;
            self.instances.get(id).map_err(|e| user_msg(&e))?;
        }
        if self.auth_status_refresh().await.is_none() {
            return Err("connectez-vous avant de lancer le jeu".into());
        }
        self.join_plan(public_key).await?;
        let instance = match instance_id {
            Some(id) => id.to_string(),
            None => {
                self.join_start(public_key).await?;
                let deadline = tokio::time::Instant::now() + JOIN_WAIT_TIMEOUT;
                while lock(&self.join).is_some() {
                    if tokio::time::Instant::now() >= deadline {
                        return Err("le téléchargement du monde de l'ami est trop long".into());
                    }
                    tokio::time::sleep(Duration::from_millis(100)).await;
                }
                lock(&self.last_join_instance).clone().ok_or("la préparation du monde de l'ami a échoué")?
            }
        };
        let title = self.instances.get(&instance).map(|i| i.name).unwrap_or_default();
        self.close_game_for_switch(&title).await?;
        self.join_play(public_key, &instance).await
    }
}

#[tauri::command(rename_all = "snake_case")]
pub async fn game_switch(state: State<'_, Arc<AppState>>, instance_id: String) -> Result<(), String> {
    state.inner().game_switch(&instance_id).await
}

#[tauri::command(rename_all = "snake_case")]
pub async fn game_join_friend(
    state: State<'_, Arc<AppState>>,
    public_key: String,
    instance_id: Option<String>,
) -> Result<(), String> {
    state.inner().game_join_friend(&public_key, instance_id.as_deref()).await
}

#[tauri::command(rename_all = "snake_case")]
pub async fn instances_kill(state: State<'_, Arc<AppState>>, id: String) -> Result<(), String> {
    state.instances_kill(&id).await
}

#[tauri::command(rename_all = "snake_case")]
pub fn instances_delete(state: State<'_, Arc<AppState>>, id: String) -> Result<(), String> {
    state.instances_delete(&id)
}

#[tauri::command(rename_all = "snake_case")]
pub fn instances_update(
    state: State<'_, Arc<AppState>>,
    id: String,
    name: Option<String>,
    ram_mb: Option<u32>,
    java_path: Option<String>,
    jvm_args: Option<String>,
) -> Result<Instance, String> {
    state.instances_update(&id, name, ram_mb, java_path, jvm_args)
}

#[tauri::command(rename_all = "snake_case")]
pub fn instances_open_folder(state: State<'_, Arc<AppState>>, id: String) -> Result<(), String> {
    state.instances_open_folder(&id)
}

#[tauri::command(rename_all = "snake_case")]
pub fn instances_logs(state: State<'_, Arc<AppState>>, id: String) -> Result<String, String> {
    state.instances_logs(&id)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::test_support::test_state;
    use core_lib::types::{Loader, LoaderSpec};

    fn vanilla() -> LoaderSpec {
        LoaderSpec { loader: Loader::Vanilla, version: String::new() }
    }

    #[test]
    fn update_refuse_nom_et_ram_invalides() {
        let dir = tempfile::tempdir().unwrap();
        let (st, _) = test_state(dir.path());
        let i = st.instances.create("Base", "1.21.1", vanilla(), 2048).unwrap();
        assert!(st.instances_update(&i.id, Some("a/b".into()), None, None, None).is_err());
        assert!(st.instances_update(&i.id, None, Some(10), None, None).is_err());
        assert!(st.instances_update("../x", None, None, None, None).is_err());
        let u = st.instances_update(&i.id, Some("Neuf".into()), Some(3072), Some("java".into()), Some("-Xss2M".into())).unwrap();
        assert_eq!((u.name.as_str(), u.ram_mb, u.java_path.as_deref(), u.jvm_args.as_str()), ("Neuf", 3072, Some("java"), "-Xss2M"));
    }

    #[test]
    fn suppression_refusee_pendant_le_jeu_puis_nettoie() {
        let dir = tempfile::tempdir().unwrap();
        let (st, _) = test_state(dir.path());
        let i = st.instances.create("Del", "1.21.1", vanilla(), 2048).unwrap();
        std::fs::create_dir_all(st.paths.logs.join(&i.id)).unwrap();
        std::fs::create_dir_all(st.paths.natives.join(&i.id)).unwrap();
        *lock(&st.game) = Some(i.id.clone());
        assert!(st.instances_delete(&i.id).unwrap_err().contains("en cours"));
        assert!(st.instances.get(&i.id).is_ok());
        *lock(&st.game) = None;
        st.instances_delete(&i.id).unwrap();
        assert!(st.instances.get(&i.id).is_err());
        assert!(!st.paths.logs.join(&i.id).exists() && !st.paths.natives.join(&i.id).exists());
    }

    #[test]
    fn kill_sans_jeu_donne_une_erreur_claire() {
        let dir = tempfile::tempdir().unwrap();
        let (st, _) = test_state(dir.path());
        let e = tauri::async_runtime::block_on(st.instances_kill("abc")).unwrap_err();
        assert!(e.contains("pas en cours"), "{e}");
        *lock(&st.game) = Some("abc".into());
        let e = tauri::async_runtime::block_on(st.instances_kill("abc")).unwrap_err();
        assert!(e.contains("préparation"), "{e}");
        assert!(tauri::async_runtime::block_on(st.instances_kill("../x")).is_err());
    }

    #[test]
    fn logs_fin_de_fichier_coupee_sur_une_ligne() {
        let dir = tempfile::tempdir().unwrap();
        let (st, _) = test_state(dir.path());
        assert_eq!(st.instances_logs("abc").unwrap(), "");
        let ld = st.paths.logs.join("abc");
        std::fs::create_dir_all(&ld).unwrap();
        std::fs::write(ld.join("latest.log"), "un\ndeux\ntrois\n").unwrap();
        assert_eq!(st.instances_logs("abc").unwrap(), "un\ndeux\ntrois\n");
        let big: String = (0..80_000).map(|n| format!("ligne {n}\n")).collect();
        assert!(big.len() as u64 > MAX_LOG_BYTES);
        std::fs::write(ld.join("latest.log"), &big).unwrap();
        let t = st.instances_logs("abc").unwrap();
        assert!(t.len() as u64 <= MAX_LOG_BYTES);
        assert!(t.starts_with("ligne ") && t.ends_with("ligne 79999\n"));
        let first: u64 = t.lines().next().unwrap()[6..].parse().unwrap();
        assert!(big.contains(&format!("ligne {}\n", first - 1)), "début coupé proprement");
    }

    fn login(st: &AppState) {
        st.auth_offline_login("Joueur").unwrap();
    }

    #[tokio::test]
    async fn stop_game_and_wait_attend_la_liberation_du_verrou() {
        let dir = tempfile::tempdir().unwrap();
        let (st, _) = test_state(dir.path());
        st.stop_game_and_wait(Duration::from_millis(100)).await.unwrap();
        *lock(&st.game) = Some("x".into());
        let s2 = st.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(300)).await;
            *lock(&s2.game) = None;
        });
        st.stop_game_and_wait(Duration::from_secs(5)).await.unwrap();
        assert!(lock(&st.game).is_none());
        // Verrou jamais libéré : délai dépassé, message clair.
        *lock(&st.game) = Some("y".into());
        let e = st.stop_game_and_wait(Duration::from_millis(300)).await.unwrap_err();
        assert!(e.contains("ne se ferme pas"), "{e}");
    }

    #[tokio::test]
    async fn game_switch_verifie_avant_de_fermer_le_jeu() {
        let dir = tempfile::tempdir().unwrap();
        let (st, _) = test_state(dir.path());
        *lock(&st.game) = Some("ancien".into());
        assert!(st.game_switch("../x").await.is_err());
        assert!(st.game_switch("inconnue").await.is_err());
        let inst = st.instances.create("Neuf", "1.21.1", vanilla(), 2048).unwrap();
        // Pas connecté : refus, le jeu en cours n'est pas touché.
        let e = st.game_switch(&inst.id).await.unwrap_err();
        assert!(e.contains("connectez-vous"), "{e}");
        assert_eq!(lock(&st.game).as_deref(), Some("ancien"));
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn game_switch_attend_la_fin_du_jeu_puis_lance_la_nouvelle_instance() {
        let dir = tempfile::tempdir().unwrap();
        let (st, events) = test_state(dir.path());
        login(&st);
        let inst = st.instances.create("Neuf", "1.21.1", vanilla(), 2048).unwrap();
        *lock(&st.game) = Some("ancien".into());
        let s2 = st.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(300)).await;
            *lock(&s2.game) = None;
        });
        st.game_switch(&inst.id).await.unwrap();
        // Le nouveau jeu a pris le verrou et annonce sa préparation.
        let seen = lock(&events).iter().any(|(n, v)| n == "launch://state" && v["id"] == inst.id.as_str());
        let holds = lock(&st.game).as_deref() == Some(inst.id.as_str());
        assert!(seen || holds, "lancement de la nouvelle instance");
    }

    #[tokio::test]
    async fn game_join_friend_refus_avant_de_fermer_le_jeu() {
        let dir = tempfile::tempdir().unwrap();
        let (st, _) = test_state(dir.path());
        *lock(&st.game) = Some("ancien".into());
        let key = "d".repeat(64);
        assert!(st.game_join_friend("zz", None).await.unwrap_err().contains("inconnu"));
        st.friends
            .add_friend(&core_lib::types::Friend { public_key: key.clone(), name: "Bob".into(), trust: core_lib::types::TrustLevel::Ask, added_at: 1 })
            .unwrap();
        assert!(st.game_join_friend(&key, Some("../x")).await.is_err());
        assert!(st.game_join_friend(&key, Some("inconnue")).await.is_err());
        assert!(st.game_join_friend(&key, None).await.unwrap_err().contains("connectez-vous"));
        login(&st);
        // Réseau indisponible : le plan échoue, le jeu en cours reste en place.
        assert!(st.game_join_friend(&key, None).await.is_err());
        assert_eq!(lock(&st.game).as_deref(), Some("ancien"));
    }

    #[test]
    fn lots_de_logs() {
        let mut b = LogBatcher::default();
        assert!(b.take().is_none());
        b.push("a");
        b.push("b");
        assert_eq!(b.take().as_deref(), Some("a\nb"));
        for n in 0..(MAX_BATCH_LINES + 10) {
            b.push(&n.to_string());
        }
        let t = b.take().unwrap();
        assert_eq!(t.lines().count(), MAX_BATCH_LINES);
        assert!(t.starts_with("10\n"));
    }
}

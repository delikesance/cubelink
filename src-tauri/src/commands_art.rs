//! Illustrations des instances : lecture d'une image (data URL), choix manuel, recherche automatique.
//!
//! La logique est dans `core_lib::art` ; ici : validation d'id et erreurs en `String`. Le décodage, la lecture et
//! l'écriture d'images se font hors du thread principal (`spawn_blocking`), sinon la fenêtre et la Scène saccadent.
//! Les illustrations vivent dans `art/` et non dans le dossier de jeu : pas d'interdiction pendant le jeu.

use std::sync::Arc;

use core_lib::art;
use core_lib::types::Instance;
use tauri::State;

use crate::commands_browse::providers;
use crate::commands_instances::valid_id;
use crate::error::user_msg;
use crate::state::AppState;

/// Décodages d'images simultanés au plus : un accueil de nombreuses bannières ne doit pas décoder dix images de
/// plusieurs dizaines de Mpx à la fois (pic de mémoire).
const MAX_DECODES: usize = 3;
static DECODE_SLOTS: tokio::sync::Semaphore = tokio::sync::Semaphore::const_new(MAX_DECODES);

impl AppState {
    fn art_id(&self, id: &str) -> Result<(), String> {
        if valid_id(id) {
            Ok(())
        } else {
            Err("identifiant d'instance invalide".into())
        }
    }

    pub async fn instances_art_image(&self, instance_id: &str, file: &str, max_px: Option<u32>) -> Result<String, String> {
        self.art_id(instance_id)?;
        let (store, id, file) = (self.instances.clone(), instance_id.to_string(), file.to_string());
        let _slot = DECODE_SLOTS.acquire().await.map_err(|_| "lecture interrompue".to_string())?;
        tokio::task::spawn_blocking(move || art::image_data_url_sized(&store, &id, &file, max_px.unwrap_or(art::THUMB_MAX_PX)))
            .await
            .map_err(|_| "lecture interrompue".to_string())?
            .map_err(|e| user_msg(&e))
    }

    pub async fn instances_art_set(&self, instance_id: &str, path: &str, role: &str) -> Result<Instance, String> {
        self.art_id(instance_id)?;
        let (store, id, path, role) = (self.instances.clone(), instance_id.to_string(), path.to_string(), role.to_string());
        tokio::task::spawn_blocking(move || art::set_custom_image(&store, &id, &path, &role))
            .await
            .map_err(|_| "écriture interrompue".to_string())?
            .map_err(|e| user_msg(&e))
    }

    pub async fn instances_description(&self, instance_id: &str) -> Result<Option<String>, String> {
        self.art_id(instance_id)?;
        art::instance_description(&providers(), &self.instances, instance_id).await.map_err(|e| user_msg(&e))
    }

    pub async fn instances_art_lookup(&self, instance_id: &str, auto: bool) -> Result<Instance, String> {
        self.art_id(instance_id)?;
        art::lookup_and_apply(&providers(), &self.instances, instance_id, auto).await.map_err(|e| user_msg(&e))
    }
}

/// Data URL d'un fichier d'illustration (`icon` ou une capture), réduit à `max_px` de côté (512 par défaut). Décodage hors du thread principal.
#[tauri::command(rename_all = "snake_case")]
#[specta::specta(rename_all = "snake_case")]
pub async fn instances_art_image(state: State<'_, Arc<AppState>>, instance_id: String, file: String, max_px: Option<u32>) -> Result<String, String> {
    state.instances_art_image(&instance_id, &file, max_px).await
}

/// Remplace l'icône ou la bannière (`role` : "icon" ou "banner") par une image choisie par le joueur. Lecture et écriture hors du thread principal ; autorisé pendant le jeu (`art/` n'est pas le dossier de jeu).
#[tauri::command(rename_all = "snake_case")]
#[specta::specta(rename_all = "snake_case")]
pub async fn instances_art_set(state: State<'_, Arc<AppState>>, instance_id: String, path: String, role: String) -> Result<Instance, String> {
    state.instances_art_set(&instance_id, &path, &role).await
}

/// Description longue du modpack, en HTML déjà nettoyé ; `None` sans projet d'origine.
#[tauri::command(rename_all = "snake_case")]
#[specta::specta(rename_all = "snake_case")]
pub async fn instances_description(state: State<'_, Arc<AppState>>, instance_id: String) -> Result<Option<String>, String> {
    state.instances_description(&instance_id).await
}

/// Cherche l'illustration du modpack et l'applique. Si l'instance a une origine (`Instance.pack`), c'est ce projet exact qui est utilisé, sans recherche par nom (et sans repli par nom en cas d'échec réseau). Sinon, `auto` : recherche silencieuse de l'accueil (nom identique seulement).
#[tauri::command(rename_all = "snake_case")]
#[specta::specta(rename_all = "snake_case")]
pub async fn instances_art_lookup(state: State<'_, Arc<AppState>>, instance_id: String, auto: Option<bool>) -> Result<Instance, String> {
    state.instances_art_lookup(&instance_id, auto.unwrap_or(false)).await
}

#[cfg(test)]
mod tests {
    use crate::state::test_support::test_state;
    use core_lib::types::{Loader, LoaderSpec};

    const PNG: &[u8] = &[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A, 1, 2, 3];

    #[tokio::test]
    async fn commandes_art_valident_et_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let (st, _) = test_state(dir.path());
        let i = st.instances.create("Pack", "1.21.1", LoaderSpec { loader: Loader::Vanilla, version: String::new() }, 2048).unwrap();
        let img = dir.path().join("x.png");
        std::fs::write(&img, PNG).unwrap();
        let p = img.display().to_string();
        assert!(st.instances_art_set("../x", &p, "icon").await.is_err());
        assert!(st.instances_art_image("../x", "icon.png", None).await.is_err());
        assert!(st.instances_art_set(&i.id, &p, "autre").await.is_err());
        let u = st.instances_art_set(&i.id, &p, "icon").await.unwrap();
        assert_eq!(u.art.unwrap().icon.as_deref(), Some("icon.png"));
        assert!(st.instances_art_image(&i.id, "icon.png", None).await.unwrap().starts_with("data:image/png;base64,"));
        assert!(st.instances_art_image(&i.id, "instance.json", None).await.is_err());
    }

    /// Régression #81 : le décodage d'une grosse image ne doit pas bloquer l'exécuteur (sur le thread principal de
    /// l'app, il gèlerait la fenêtre et la Scène). Runtime à un seul thread : un décodage inline y empêcherait
    /// la tâche témoin de s'exécuter pendant toute sa durée.
    #[tokio::test(flavor = "current_thread")]
    async fn art_image_ne_bloque_pas_l_executeur() {
        let dir = tempfile::tempdir().unwrap();
        let (st, _) = test_state(dir.path());
        let i = st.instances.create("Pack", "1.21.1", LoaderSpec { loader: Loader::Vanilla, version: String::new() }, 2048).unwrap();
        let img = dir.path().join("big.png");
        // Bruit pseudo-aléatoire : le PNG reste gros et le décodage long.
        let mut seed = 0x2545_F491_u32;
        let raw = image::RgbImage::from_fn(4096, 2304, |_, _| {
            seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            image::Rgb([(seed >> 8) as u8, (seed >> 16) as u8, (seed >> 24) as u8])
        });
        raw.save(&img).unwrap();
        if std::fs::metadata(&img).unwrap().len() > 8 * 1024 * 1024 {
            // Au-delà du plafond de lecture : on se contente d'une image plus petite mais encore décodée.
            image::RgbImage::from_fn(2048, 1152, |x, y| image::Rgb([(x ^ y) as u8, (x * 3) as u8, (y * 5) as u8])).save(&img).unwrap();
        }
        st.instances_art_set(&i.id, &img.display().to_string(), "banner").await.unwrap();
        let file = st.instances.get(&i.id).unwrap().art.unwrap().screenshots[0].clone();

        // Tâche témoin : bat toutes les 5 ms et garde le plus grand écart entre deux battements.
        let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let flag = stop.clone();
        let ticker = tokio::spawn(async move {
            let mut worst = std::time::Duration::ZERO;
            let mut last = std::time::Instant::now();
            while !flag.load(std::sync::atomic::Ordering::Relaxed) {
                tokio::time::sleep(std::time::Duration::from_millis(5)).await;
                worst = worst.max(last.elapsed());
                last = std::time::Instant::now();
            }
            worst
        });
        tokio::time::sleep(std::time::Duration::from_millis(30)).await; // le témoin bat déjà
        let started = std::time::Instant::now();
        let url = st.instances_art_image(&i.id, &file, None).await.unwrap();
        let took = started.elapsed();
        stop.store(true, std::sync::atomic::Ordering::Relaxed);
        let worst = ticker.await.unwrap();
        assert!(url.starts_with("data:image/"));
        // Décodé sur l'exécuteur, le plus grand écart égalerait `took` ; hors exécuteur, il reste proche des 5 ms.
        // Sur une machine assez rapide pour décoder en moins de 100 ms, la mesure n'a pas de sens : on ne juge pas.
        if took > std::time::Duration::from_millis(100) {
            assert!(worst * 3 < took, "exécuteur bloqué : pire écart {worst:?} pour un décodage de {took:?}");
        }
    }
}

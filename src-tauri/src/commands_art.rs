//! Illustrations des instances : lecture d'une image (data URL), choix manuel, recherche automatique.
//!
//! La logique est dans `core_lib::art` ; ici : validation d'id, interdiction pendant le jeu et erreurs en `String`.

use std::sync::Arc;

use core_lib::art;
use core_lib::types::Instance;
use tauri::State;

use crate::commands_browse::providers;
use crate::commands_instances::valid_id;
use crate::error::user_msg;
use crate::state::AppState;

impl AppState {
    fn art_id(&self, id: &str) -> Result<(), String> {
        if valid_id(id) {
            Ok(())
        } else {
            Err("identifiant d'instance invalide".into())
        }
    }

    pub fn instances_art_image(&self, instance_id: &str, file: &str, max_px: Option<u32>) -> Result<String, String> {
        self.art_id(instance_id)?;
        art::image_data_url_sized(&self.instances, instance_id, file, max_px.unwrap_or(art::THUMB_MAX_PX)).map_err(|e| user_msg(&e))
    }

    pub fn instances_art_set(&self, instance_id: &str, path: &str, role: &str) -> Result<Instance, String> {
        self.art_id(instance_id)?;
        art::set_custom_image(&self.instances, instance_id, path, role).map_err(|e| user_msg(&e))
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

#[tauri::command(rename_all = "snake_case")]
pub fn instances_art_image(state: State<'_, Arc<AppState>>, instance_id: String, file: String, max_px: Option<u32>) -> Result<String, String> {
    state.instances_art_image(&instance_id, &file, max_px)
}

#[tauri::command(rename_all = "snake_case")]
pub fn instances_art_set(state: State<'_, Arc<AppState>>, instance_id: String, path: String, role: String) -> Result<Instance, String> {
    state.instances_art_set(&instance_id, &path, &role)
}

#[tauri::command(rename_all = "snake_case")]
pub async fn instances_description(state: State<'_, Arc<AppState>>, instance_id: String) -> Result<Option<String>, String> {
    state.instances_description(&instance_id).await
}

#[tauri::command(rename_all = "snake_case")]
pub async fn instances_art_lookup(state: State<'_, Arc<AppState>>, instance_id: String, auto: Option<bool>) -> Result<Instance, String> {
    state.instances_art_lookup(&instance_id, auto.unwrap_or(false)).await
}

#[cfg(test)]
mod tests {
    use crate::state::test_support::test_state;
    use core_lib::types::{Loader, LoaderSpec};

    const PNG: &[u8] = &[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A, 1, 2, 3];

    #[test]
    fn commandes_art_valident_et_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let (st, _) = test_state(dir.path());
        let i = st.instances.create("Pack", "1.21.1", LoaderSpec { loader: Loader::Vanilla, version: String::new() }, 2048).unwrap();
        let img = dir.path().join("x.png");
        std::fs::write(&img, PNG).unwrap();
        let p = img.display().to_string();
        assert!(st.instances_art_set("../x", &p, "icon").is_err());
        assert!(st.instances_art_image("../x", "icon.png", None).is_err());
        assert!(st.instances_art_set(&i.id, &p, "autre").is_err());
        let u = st.instances_art_set(&i.id, &p, "icon").unwrap();
        assert_eq!(u.art.unwrap().icon.as_deref(), Some("icon.png"));
        assert!(st.instances_art_image(&i.id, "icon.png", None).unwrap().starts_with("data:image/png;base64,"));
        assert!(st.instances_art_image(&i.id, "instance.json", None).is_err());
    }
}

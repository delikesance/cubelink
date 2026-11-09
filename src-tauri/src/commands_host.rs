//! Hôte : partage d'une instance et du monde LAN (`host_start_sharing` / `host_stop_sharing`).

use std::sync::Arc;

use tauri::State;

use crate::error::user_msg;
use crate::state::AppState;
use crate::test_hooks::{set_tunnel_port, validate_world_port};

impl AppState {
    /// `world_port` : port du monde LAN local (1024 à 65535). L'instance doit exister.
    pub async fn host_start_sharing(&self, instance_id: &str, world_port: u32) -> Result<(), String> {
        let port = validate_world_port(world_port)?;
        if !crate::commands_instances::valid_id(instance_id) {
            return Err("identifiant d'instance invalide".into());
        }
        self.instances.get(instance_id).map_err(|e| user_msg(&e))?;
        self.backend().host_start_sharing(instance_id.to_string(), port).await?;
        set_tunnel_port(port);
        Ok(())
    }

    pub fn host_stop_sharing(&self) {
        self.backend().host_stop_sharing();
        set_tunnel_port(0);
    }
}

#[tauri::command(rename_all = "snake_case")]
pub async fn host_start_sharing(state: State<'_, Arc<AppState>>, instance_id: String, world_port: u32) -> Result<(), String> {
    state.host_start_sharing(&instance_id, world_port).await
}

#[tauri::command(rename_all = "snake_case")]
pub fn host_stop_sharing(state: State<'_, Arc<AppState>>) -> Result<(), String> {
    state.host_stop_sharing();
    Ok(())
}

#[cfg(test)]
mod tests {
    use crate::state::test_support::test_state;

    #[tokio::test]
    async fn refuse_port_invalide_et_instance_inconnue() {
        let dir = tempfile::tempdir().unwrap();
        let (st, _) = test_state(dir.path());
        for p in [0, 80, 1023, 65536] {
            assert!(st.host_start_sharing("x", p).await.unwrap_err().contains("port"), "{p}");
        }
        assert!(st.host_start_sharing("../x", 25565).await.unwrap_err().contains("invalide"));
        assert!(st.host_start_sharing("inconnue", 25565).await.is_err());
    }
}

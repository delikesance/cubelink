//! Coque Tauri. Propriété : tâche U13 (commandes), selon docs/contrats.md.

pub mod authz;
pub mod commands_auth;
pub mod commands_friends;
pub mod commands_host;
pub mod commands_instances;
pub mod commands_join;
pub mod commands_settings;
pub mod error;
pub mod iroh_backend;
pub mod install;
pub mod p2p_backend;
pub mod state;
pub mod test_hooks;

use std::sync::Arc;

use tauri::{Emitter, Manager};

/// `test_state` n'existe dans le gestionnaire d'invocation qu'en mode test.
fn select_handler<R: tauri::Runtime>(
    test_mode: bool,
    normal: impl Fn(tauri::ipc::Invoke<R>) -> bool + Send + Sync + 'static,
    with_test: impl Fn(tauri::ipc::Invoke<R>) -> bool + Send + Sync + 'static,
) -> impl Fn(tauri::ipc::Invoke<R>) -> bool + Send + Sync + 'static {
    move |inv| if test_mode { with_test(inv) } else { normal(inv) }
}

pub(crate) fn iroh_options(h: &test_hooks::Hooks) -> iroh_backend::IrohOptions {
    use p2p_lib::relay::RelayChoice;
    let mut o = iroh_backend::IrohOptions::default();
    if let Some(s) = h.transfer_idle_secs {
        o.inactivity = std::time::Duration::from_secs(s);
        o.stall_retry = o.stall_retry.min(o.inactivity / 2);
    }
    if let Some(r) = &h.relay {
        // Mode test, relais coupé : le code ami contient les adresses directes (sinon aucun chemin sans relais).
        if h.test_mode && *r == test_hooks::RelayMode::Disabled {
            o.advertise_addrs = Some(true);
        }
        o.relay = match r {
            test_hooks::RelayMode::Disabled => RelayChoice::Disabled,
            test_hooks::RelayMode::Default => RelayChoice::Default,
            test_hooks::RelayMode::Custom(u) => RelayChoice::Custom(u.clone()),
        };
    }
    o
}

pub fn run() {
    let hooks = test_hooks::init().unwrap_or_else(|e| panic!("configuration de test invalide : {e}"));
    let test_mode = hooks.test_mode;
    tauri::Builder::default()
        .setup(move |app| {
            let root = match &hooks.data_dir {
                Some(d) => d.clone(),
                None => app.path().app_data_dir()?,
            };
            let handle = app.handle().clone();
            let sink: state::EventSink = Arc::new(move |name, payload| {
                let _ = handle.emit(name, payload);
            });
            let (st, _backend) = tauri::async_runtime::block_on(iroh_backend::start_app(&root, sink, iroh_options(hooks)))?;
            app.manage(st);
            Ok(())
        })
        .invoke_handler(select_handler(test_mode, tauri::generate_handler![
            commands_auth::auth_status,
            commands_auth::auth_begin_login,
            commands_auth::auth_poll_login,
            commands_auth::auth_logout,
            commands_auth::auth_offline_login,
            commands_instances::instances_list,
            commands_instances::instances_create,
            commands_instances::instances_import_mrpack,
            commands_instances::instances_launch,
            commands_instances::minecraft_versions,
            commands_friends::friend_my_code,
            commands_friends::friend_regenerate_code,
            commands_friends::friend_list,
            commands_friends::friend_send_request,
            commands_friends::friend_requests,
            commands_friends::friend_accept,
            commands_friends::friend_refuse,
            commands_friends::friend_refuse_all,
            commands_join::join_plan,
            commands_join::join_start,
            commands_join::join_pause,
            commands_join::join_resume,
            commands_join::join_cancel,
            commands_join::join_open_tunnel,
            commands_join::join_close_tunnel,
            commands_settings::settings_get,
            commands_settings::settings_set,
            commands_host::host_start_sharing,
            commands_host::host_stop_sharing,
        ], tauri::generate_handler![
            commands_auth::auth_status,
            commands_auth::auth_begin_login,
            commands_auth::auth_poll_login,
            commands_auth::auth_logout,
            commands_auth::auth_offline_login,
            commands_instances::instances_list,
            commands_instances::instances_create,
            commands_instances::instances_import_mrpack,
            commands_instances::instances_launch,
            commands_instances::minecraft_versions,
            commands_friends::friend_my_code,
            commands_friends::friend_regenerate_code,
            commands_friends::friend_list,
            commands_friends::friend_send_request,
            commands_friends::friend_requests,
            commands_friends::friend_accept,
            commands_friends::friend_refuse,
            commands_friends::friend_refuse_all,
            commands_join::join_plan,
            commands_join::join_start,
            commands_join::join_pause,
            commands_join::join_resume,
            commands_join::join_cancel,
            commands_join::join_open_tunnel,
            commands_join::join_close_tunnel,
            commands_settings::settings_get,
            commands_settings::settings_set,
            commands_host::host_start_sharing,
            commands_host::host_stop_sharing,
            test_hooks::test_state,
        ]))
        .run(tauri::generate_context!())
        .expect("erreur au lancement de Cubelink");
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hooks(test_mode: bool, relay: Option<test_hooks::RelayMode>) -> test_hooks::Hooks {
        test_hooks::Hooks { test_mode, relay, ..Default::default() }
    }

    #[test]
    fn relais_coupe_en_test_publie_les_adresses_directes() {
        let o = iroh_options(&hooks(true, Some(test_hooks::RelayMode::Disabled)));
        assert_eq!(o.advertise_addrs, Some(true));
        assert!(matches!(o.relay, p2p_lib::relay::RelayChoice::Disabled));
        // Autres modes de test : comportement inchangé.
        assert_eq!(iroh_options(&hooks(true, Some(test_hooks::RelayMode::Default))).advertise_addrs, None);
        assert_eq!(iroh_options(&hooks(true, None)).advertise_addrs, None);
        // Hors mode test, le relais coupé ne change rien.
        assert_eq!(iroh_options(&hooks(false, Some(test_hooks::RelayMode::Disabled))).advertise_addrs, None);
    }

    #[test]
    fn delai_d_inactivite_configurable_en_test() {
        let mut h = hooks(true, None);
        h.transfer_idle_secs = Some(4);
        let o = iroh_options(&h);
        assert_eq!(o.inactivity, std::time::Duration::from_secs(4));
        assert!(o.stall_retry <= std::time::Duration::from_secs(2));
        assert_eq!(iroh_options(&hooks(true, None)).inactivity, iroh_backend::DEFAULT_INACTIVITY);
    }
}

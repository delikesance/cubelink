//! Coque Tauri. Propriété : tâche U13 (commandes), selon docs/contrats.md.

pub mod authz;
pub mod commands_art;
pub mod commands_auth;
pub mod commands_browse;
pub mod commands_friends;
pub mod commands_game;
pub mod commands_host;
pub mod commands_instances;
pub mod commands_join;
pub mod commands_settings;
pub mod commands_update;
pub mod error;
pub mod foreground;
pub mod iroh_backend;
pub mod install;
pub mod invites;
pub mod hotkey;
pub mod p2p_backend;
pub mod stage;
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
    // Charge un éventuel `.env` de dev (ex. CUBELINK_MS_CLIENT_ID). Silencieux si absent ;
    // n'écrase jamais une variable déjà définie dans l'environnement du processus.
    let _ = dotenvy::dotenv();
    let hooks = test_hooks::init().unwrap_or_else(|e| panic!("configuration de test invalide : {e}"));
    let test_mode = hooks.test_mode;
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .plugin(tauri_plugin_global_shortcut::Builder::new().build())
        .plugin(tauri_plugin_notification::init())
        .setup(move |app| {
            let root = match &hooks.data_dir {
                Some(d) => d.clone(),
                None => app.path().app_data_dir()?,
            };
            let handle = app.handle().clone();
            let sink: state::EventSink = Arc::new(move |name, payload| {
                let _ = handle.emit(name, payload);
            });
            // L'état est créé tout de suite avec un backend « en démarrage » ; Iroh démarre en tâche de fond
            // (les commandes réseau attendent sa disponibilité, voir `state::PendingBackend`).
            let pending = Arc::new(state::PendingBackend::new());
            let st = Arc::new(state::AppState::new(&root, sink, pending.clone())?);
            // Agent LAN livré en ressource (`CUBELINK_LAN_AGENT` le remplace, voir commands_instances).
            if let Ok(p) = app.path().resolve("cubelink-lan-agent.jar", tauri::path::BaseDirectory::Resource) {
                if p.is_file() {
                    *state::lock(&st.lan_agent) = Some(p);
                }
            }
            app.manage(st.clone());
            commands_update::setup(app, test_mode);
            hotkey::setup(app.handle(), test_mode);
            // Zone de jeu integree : la fenetre du jeu est adoptee dans celle du launcher (CUBELINK_NO_STAGE=1 : desactive).
            if std::env::var_os("CUBELINK_NO_STAGE").is_none() && (!test_mode || std::env::var_os("CUBELINK_STAGE_TEST").is_some()) {
                if let Some(w) = app.get_webview_window("main") {
                    let h = app.handle().clone();
                    let sink: state::EventSink = Arc::new(move |name, payload| {
                        let _ = h.emit(name, payload);
                    });
                    st.stage.attach(&w, sink);
                    let (st2, h2) = (st.clone(), app.handle().clone());
                    w.on_window_event(move |e| {
                        // Fermer la fenetre du launcher ferme aussi le jeu rattache : fermeture propre (monde enregistre), jamais
                        // abandonne en tache de fond ; force seulement apres 60 s.
                        if let tauri::WindowEvent::CloseRequested { api, .. } = e {
                            static CLOSING: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
                            if st2.stage.state().attached {
                                api.prevent_close();
                                if !CLOSING.swap(true, std::sync::atomic::Ordering::SeqCst) {
                                    let (st3, h3) = (st2.clone(), h2.clone());
                                    tauri::async_runtime::spawn(async move {
                                        st3.stage.request_close();
                                        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(60);
                                        loop {
                                            let current = state::lock(&st3.game).clone();
                                            let Some(id) = current else { break };
                                            if tokio::time::Instant::now() >= deadline {
                                                let _ = st3.instances_kill(&id).await;
                                                tokio::time::sleep(std::time::Duration::from_secs(2)).await;
                                                break;
                                            }
                                            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
                                        }
                                        h3.exit(0);
                                    });
                                }
                            }
                        }
                        if matches!(e, tauri::WindowEvent::Resized(_) | tauri::WindowEvent::Moved(_) | tauri::WindowEvent::ScaleFactorChanged { .. }) {
                            if let Some(w) = h2.get_webview_window("main") {
                                st2.stage.relayout(&w);
                            }
                        }
                    });
                }
            }
            // Diagnostic : lance une instance sans clic (CUBELINK_STAGE_AUTOLAUNCH=<id>), jamais utilise en production.
            if let Ok(id) = std::env::var("CUBELINK_STAGE_AUTOLAUNCH") {
                let st = st.clone();
                tauri::async_runtime::spawn(async move {
                    tokio::time::sleep(std::time::Duration::from_secs(4)).await;
                    if let Err(e) = st.instances_launch(&id).await {
                        eprintln!("[Cubelink] lancement automatique : {e}");
                    }
                });
            }
            let opts = iroh_options(hooks);
            tauri::async_runtime::spawn(async move {
                match iroh_backend::IrohBackend::start(&st, opts).await {
                    Ok(be) => {
                        st.set_backend(be.clone());
                        pending.ready(be);
                    }
                    Err(e) => {
                        eprintln!("démarrage du réseau P2P impossible : {e}");
                        pending.failed(e);
                    }
                }
            });
            Ok(())
        })
        .invoke_handler(select_handler(test_mode, tauri::generate_handler![
            commands_auth::auth_status,
            commands_auth::auth_accounts,
            commands_auth::auth_switch,
            commands_auth::auth_remove,
            commands_auth::auth_begin_login,
            commands_auth::auth_poll_login,
            commands_auth::auth_logout,
            commands_auth::auth_offline_login,
            commands_instances::instances_list,
            commands_instances::instances_create,
            commands_instances::instances_import_mrpack,
            commands_instances::instances_launch,
            commands_instances::minecraft_versions,
            commands_game::instances_kill,
            commands_game::instances_delete,
            commands_game::instances_update,
            commands_game::instances_open_folder,
            commands_game::instances_logs,
            commands_art::instances_art_image,
            commands_art::instances_art_set,
            commands_art::instances_art_lookup,
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
            commands_join::join_play,
            commands_update::update_state,
            commands_update::update_check,
            commands_update::update_install,
            commands_settings::settings_get,
            commands_settings::settings_set,
            commands_browse::browse_search,
            commands_browse::browse_versions,
            commands_browse::browse_install,
            commands_browse::mods_list,
            commands_browse::mods_toggle,
            commands_browse::mods_remove,
            commands_browse::mods_add,
            commands_host::host_start_sharing,
            commands_host::host_stop_sharing,
            commands_host::host_state,
            invites::friend_invite,
            invites::friend_invites,
            invites::friend_invite_dismiss,
            commands_game::game_switch,
            commands_game::game_join_friend,
            stage::stage_view,
            stage::stage_state,
            stage::stage_panel,
            stage::stage_layout,
        ], tauri::generate_handler![
            commands_auth::auth_status,
            commands_auth::auth_accounts,
            commands_auth::auth_switch,
            commands_auth::auth_remove,
            commands_auth::auth_begin_login,
            commands_auth::auth_poll_login,
            commands_auth::auth_logout,
            commands_auth::auth_offline_login,
            commands_instances::instances_list,
            commands_instances::instances_create,
            commands_instances::instances_import_mrpack,
            commands_instances::instances_launch,
            commands_instances::minecraft_versions,
            commands_game::instances_kill,
            commands_game::instances_delete,
            commands_game::instances_update,
            commands_game::instances_open_folder,
            commands_game::instances_logs,
            commands_art::instances_art_image,
            commands_art::instances_art_set,
            commands_art::instances_art_lookup,
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
            commands_join::join_play,
            commands_settings::settings_get,
            commands_settings::settings_set,
            commands_browse::browse_search,
            commands_browse::browse_versions,
            commands_browse::browse_install,
            commands_browse::mods_list,
            commands_browse::mods_toggle,
            commands_browse::mods_remove,
            commands_browse::mods_add,
            commands_host::host_start_sharing,
            commands_host::host_stop_sharing,
            commands_host::host_state,
            invites::friend_invite,
            invites::friend_invites,
            invites::friend_invite_dismiss,
            commands_game::game_switch,
            commands_game::game_join_friend,
            stage::stage_view,
            stage::stage_state,
            stage::stage_panel,
            stage::stage_layout,
            test_hooks::test_state,
            test_hooks::test_cut_guest_connection,
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

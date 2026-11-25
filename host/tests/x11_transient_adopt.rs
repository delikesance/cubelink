//! Scene X11, chemin TRANSITOIRE (celui de tout bureau : un gestionnaire de fenetres tourne, `wm_present`) : la fenetre du jeu n'est
//! PAS reparentee dans la scene, elle reste fille de la racine avec `WM_TRANSIENT_FOR` = launcher, `_MOTIF_WM_HINTS` sans decoration et
//! `SKIP_TASKBAR` ; calee en coordonnees ECRAN, garee en -32000 hors du jeu, panneau decoupe par XShape, restituee au repli.
//! Le gestionnaire est SIMULE par la seule propriete `_NET_SUPPORTING_WM_CHECK` (`FakeWmMark`, posee AVANT `GameHost::embed`, qui
//! lit `wm_present` une fois) : le serveur execute telles quelles les requetes de placement, les lectures X11 sont exactes. Ce que
//! cela ne dit pas (cadre, contraintes de position, focus repris) : voir `x11_transient_openbox`. Voir aussi `common_x11`.
#![cfg(target_os = "linux")]

mod common_x11;

use std::time::Duration;

use common_x11::*;
use cubelink_host::{FallbackReason, GameHost};
use x11rb::connection::Connection;
use x11rb::protocol::xproto::*;
use x11rb::wrapper::ConnectionExt as _;

const T: Duration = Duration::from_secs(15);

#[test]
fn jeu_transitoire_adopte_pose_clippe_gare_et_rendu() {
    if !available() {
        return;
    }
    let parent = Parent::new();
    let _wm = FakeWmMark::set(parent.xid); // AVANT l'hote
    let host = GameHost::embed(parent.id()).expect("embed");
    let events = Events::default();
    host.set_observer(events.observer());
    let (cw, ch) = parent.client_size();
    let zone = (100, 60, cw - 200, ch - 120); // PAS toute la fenetre : de quoi verifier le calage en coordonnees ecran
    host.relayout(zone.0, zone.1, zone.2, zone.3);
    let (root, curtain) = (x().1, curtain());
    let (_, _, zw, zh) = parent.to_screen(zone);
    let skip = atom_named("_NET_WM_STATE_SKIP_TASKBAR");

    // ---- lancement : un echantillonneur guette toute fenetre du jeu a l'ecran AILLEURS que sur la zone de jeu (le joueur ne doit
    // jamais apercevoir le jeu autonome ; sous le rideau, sur la zone, c'est normal)
    give_focus(parent.xid); // avec un gestionnaire, le rideau ne se ferme que si le launcher est la fenetre active
    host.begin("Modpack de test", "Lancement", None, None);
    let mut game = FakeGame::spawn(&["GLFW30", "500", "glfwhelper", "late=150"]);
    let pid = game.pid();
    let zone_screen = parent.to_screen(zone);
    let sampler = Sampler::spawn(move || {
        use x11rb::rust_connection::RustConnection;
        let (c, n) = RustConnection::connect(None).expect("connexion de l'echantillonneur");
        let root = c.setup().roots[n].root;
        let pid_atom = c.intern_atom(false, b"_NET_WM_PID").unwrap().reply().unwrap().atom;
        let (sw, sh) = {
            let g = c.get_geometry(root).unwrap().reply().unwrap();
            (g.width as i32, g.height as i32)
        };
        move |t0: std::time::Instant, seen: &mut Vec<String>| {
            for w in c.query_tree(root).unwrap().reply().unwrap().children {
                let p = c.get_property(false, w, pid_atom, AtomEnum::CARDINAL, 0, 1).ok().and_then(|k| k.reply().ok()).and_then(|r| r.value32().and_then(|mut v| v.next()));
                if p != Some(pid) {
                    continue;
                }
                let (Some(a), Some(g)) = (c.get_window_attributes(w).ok().and_then(|k| k.reply().ok()), c.get_geometry(w).ok().and_then(|k| k.reply().ok())) else { continue };
                let (gx, gy) = (g.x as i32, g.y as i32);
                let on_screen = gx < sw && gy < sh && gx + g.width as i32 > 0 && gy + g.height as i32 > 0;
                let on_zone = (gx, gy) == (zone_screen.0, zone_screen.1);
                if a.map_state == MapState::VIEWABLE && on_screen && !on_zone {
                    seen.push(format!("+{} ms : fenetre {w:#x} visible a l'ecran en ({gx}, {gy}), hors de la zone de jeu", t0.elapsed().as_millis()));
                }
            }
        }
    });
    host.watch(game.pid());
    wait_until("evenement Adopted", T, || !events.adopted().is_empty());
    let win = game.window("GLFW30").expect("fenetre du jeu");
    assert_eq!(events.adopted(), vec!["GLFW30".to_string()]);

    // ---- adoption transitoire : pas de reparentage, les marques du repli
    assert_eq!(parent_of(win), root, "chemin transitoire : la fenetre reste fille de la racine (jamais de la scene)");
    assert_eq!(prop32_any(win, "WM_TRANSIENT_FOR"), vec![parent.xid], "WM_TRANSIENT_FOR = launcher");
    assert_eq!(prop32_any(win, "_MOTIF_WM_HINTS"), vec![2, 0, 0, 0, 0], "aucune decoration");
    let state = prop32_any(win, "_NET_WM_STATE");
    assert!(state.contains(&skip) && state.contains(&atom_named("_NET_WM_STATE_SKIP_PAGER")), "pas de barre des taches ni de pager : {state:?}");
    events.assert_logged("adoptee en");
    assert!(!events.logged("rattachement refuse"), "journal :\n{}", events.logs().join("\n"));

    // ---- posee sur la zone de jeu en coordonnees ECRAN, sous le rideau ferme
    wait_until("fenetre posee sur la zone de jeu", T, || visible(win) && rect(win) == parent.to_screen(zone));
    assert!(visible(curtain) && !host.in_game(), "le rideau reste ferme jusqu'a `reveal`");
    host.reveal();
    wait_until("rideau ouvert sur le jeu", T, || host.in_game() && !visible(curtain));
    assert_eq!(rect(win), parent.to_screen(zone));
    assert_eq!(parent_of(win), root);
    let (samples, seen) = sampler.stop();
    assert!(samples > 200, "echantillonnage trop maigre pour conclure : {samples}");
    assert!(seen.is_empty(), "fenetre de jeu autonome apercue avant la pose ({samples} echantillons) :\n{}", seen.join("\n"));
    let (px, py, pw, ph) = parent.to_screen(zone);
    assert_eq!(window_under_pointer((px + 10) as i16, (py + 10) as i16), win, "la souris atteint le jeu");

    // ---- panneau « en jeu » : le jeu garde sa taille, sa bande de droite est rognee (forme ET zone d'entree) ; le launcher dessous
    assert!(host.set_panel(true, 150));
    wait_until("jeu rogne par le panneau", T, || input_shape(win).len() == 1 && input_shape(win)[0].width as i32 == zw - 150);
    assert_eq!(size(win), (zw, zh), "le jeu garde sa taille (rognee, pas redimensionnee)");
    for shape in [input_shape(win), bounding_shape(win)] {
        assert_eq!((shape[0].x, shape[0].y, shape[0].width as i32, shape[0].height as i32), (0, 0, zw - 150, zh), "forme du jeu rognee de la bande du panneau");
    }
    assert_eq!(window_under_pointer((px + pw - 20) as i16, (py + ph / 2) as i16), parent.xid, "la bande du panneau est rendue au launcher");
    assert_eq!(window_under_pointer((px + 10) as i16, (py + 10) as i16), win, "le reste du jeu reste cliquable");
    assert!(host.set_panel(false, 0));
    wait_until("forme d'origine rendue", T, || !bounding_shaped(win) && !input_shaped(win));
    assert_eq!(window_under_pointer((px + pw - 20) as i16, (py + ph / 2) as i16), win, "plus de panneau : toute la zone est au jeu");

    // ---- le launcher se deplace : le jeu le suit, en coordonnees ecran
    move_window(parent.xid, (260, 140));
    wait_until("jeu recale sur la zone apres deplacement du launcher", T, || rect(win) == parent.to_screen(zone));
    assert_ne!(rect(win).0, px, "le launcher a bien bouge");

    // ---- retour au launcher sans arreter le jeu : gare hors ecran (toujours affichee), puis retour
    host.set_view(false);
    wait_until("jeu gare hors ecran", T, || !on_screen(win));
    assert!(mapped(win) && !game.exited(), "gare, pas ferme ni cache");
    assert_eq!(host.view(), (true, false));
    host.set_view(true);
    wait_until("retour dans le jeu", T, || host.in_game() && on_screen(win) && rect(win) == parent.to_screen(zone));

    // ---- le jeu reecrit ses decorations (GLFW) : l'hote les retire de nouveau
    let motif = atom_named("_MOTIF_WM_HINTS");
    let (c, _) = x();
    c.change_property32(PropMode::REPLACE, win, motif, motif, &[2, 0, 1, 0, 0]).unwrap();
    c.flush().unwrap();
    wait_until("decorations retirees de nouveau", T, || prop32_any(win, "_MOTIF_WM_HINTS") == vec![2, 0, 0, 0, 0]);
    events.assert_logged("le jeu a remis ses decorations");

    // ---- plein ecran EWMH : annonce, etat pose SANS perdre SKIP_TASKBAR, puis rendu
    let fs = fullscreen_atom();
    game_sends_net_wm_state(win, 1);
    wait_until("plein ecran annonce", T, || events.fullscreens() == vec![true]);
    wait_until("etat plein ecran pose", T, || net_wm_state(win).contains(&fs));
    assert!(net_wm_state(win).contains(&skip), "SKIP_TASKBAR conserve en plein ecran");
    game_sends_net_wm_state(win, 0);
    wait_until("sortie annoncee", T, || events.fullscreens() == vec![true, false]);
    wait_until("etat plein ecran retire", T, || !net_wm_state(win).contains(&fs));
    // ... et par la taille de la fenetre (F11 de LWJGL 2 / SDL sans EWMH)
    game_grabs_screen(win);
    wait_until("plein ecran annonce (fenetre de la taille de l'ecran)", T, || events.fullscreens() == vec![true, false, true]);
    wait_until("fenetre ramenee sur la zone", T, || rect(win) == parent.to_screen(zone));
    game_leaves_screen(win);
    wait_until("sortie annoncee", T, || events.fullscreens() == vec![true, false, true, false]);
    wait_until("fenetre sur la zone", T, || rect(win) == parent.to_screen(zone));
    assert!(host.in_game() && events.fallbacks().is_empty());

    // ---- repli : la fenetre est RENDUE au jeu (plus de marque transitoire, sur le bureau, visible, a l'ecran)
    host.release("demande par le test");
    wait_until("evenement Fallback", T, || !events.fallbacks().is_empty());
    assert_eq!(events.fallbacks(), vec![(FallbackReason::Requested, "demande par le test".to_string())]);
    assert_given_back(win);
    assert_eq!(prop32_any(win, "_NET_WM_STATE"), Vec::<u32>::new(), "plus d'etat transitoire (SKIP_TASKBAR...)");
    assert_ne!((rect(win).0, rect(win).1), (parent.to_screen(zone).0, parent.to_screen(zone).1), "la fenetre a quitte la zone de jeu");
    assert!(!game.exited(), "le jeu n'est jamais tue par un repli");
    pump_for(Duration::from_millis(800));
    assert_given_back(win); // plus jamais retouchee
    game.kill();
}

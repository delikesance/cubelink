//! Harnais COMMUN aux scenarios de la Scene sous Windows et sous X11 (`tests/scene_*.rs`).
//!
//! Un scenario ecrit ici tourne sur les deux systemes : il ne parle que le vocabulaire commun ci-dessous (fenetre parente, rideau,
//! faux jeu, evenements, lectures « visible / rectangle ») et les quelques constats dont la forme depend du systeme (le jeu est
//! dans la Scene, le panneau est applique, la fenetre est rendue au bureau) sont donnes par une fonction de meme nom de chaque
//! cote. Les outils propres a chaque systeme restent dans `common` (Win32) et `common_x11` (X11) ; les scenarios qui verifient un
//! detail d'un seul systeme (region Win32, `_NET_WM_STATE`, focus X11...) gardent leur fichier `stage_*` ou `x11_*`.
//!
//! Memes regles qu'eux : aucune entree souris / clavier, un seul hote par processus, faux jeu tue a la fin. Sous Linux, le scenario
//! se met en retrait sans serveur X11 nu (voir `common_x11::available`) ; lancer avec `xvfb-run -a`.
#![allow(dead_code)] // chaque scenario n'utilise qu'une partie des outils

#[cfg(windows)]
#[path = "../common/mod.rs"]
pub mod sys;
#[cfg(target_os = "linux")]
#[path = "../common_x11/mod.rs"]
pub mod sys;

#[allow(unused_imports)] // chaque scenario n'utilise qu'une partie des outils
pub use sys::{assert_given_back, curtain, pump_for, rect, visible, wait_until, Events, FakeGame, Parent};

/// Identifiant d'une fenetre (HWND, XID).
#[cfg(windows)]
pub type Win = isize;
#[cfg(target_os = "linux")]
pub type Win = u32;

/// Le scenario peut tourner ici (Windows : toujours, avec une session de bureau ; X11 : serveur nu joignable).
pub fn available() -> bool {
    #[cfg(windows)]
    return true;
    #[cfg(target_os = "linux")]
    return sys::available();
}

/// La fenetre `w` est tenue par la Scene du launcher `parent` : possedee et sans cadre (Windows), reparentee dans la scene (X11).
pub fn in_scene(parent: &Parent, w: Win) -> bool {
    #[cfg(windows)]
    return sys::owner(w) == parent.id() && !sys::framed(w);
    #[cfg(target_os = "linux")]
    {
        let _ = parent;
        sys::parent_of(w) == sys::stage()
    }
}

/// Le panneau « en jeu » de `width` pixels est applique sur la zone `zone` : decoupe de la fenetre du jeu (Windows), scene
/// reduite (X11).
pub fn panel_applied(w: Win, zone: (i32, i32, i32, i32), width: i32) -> bool {
    #[cfg(windows)]
    {
        let _ = (zone, width);
        sys::panel_cut(w)
    }
    #[cfg(target_os = "linux")]
    {
        let _ = w;
        sys::size(sys::stage()).0 == zone.2 - width
    }
}

/// Plus rien de la Scene n'est visible a la place du jeu (X11 : la scene est retiree ; Windows : la Scene n'a pas de fenetre a elle
/// sous le jeu).
pub fn scene_withdrawn() -> bool {
    #[cfg(windows)]
    return true;
    #[cfg(target_os = "linux")]
    return !visible(sys::stage());
}

/// La fenetre n'a pas encore ete affichee par le jeu : cachee et garee hors ecran (Windows), non affichee (X11).
pub fn not_shown_yet(w: Win) -> bool {
    #[cfg(windows)]
    return !visible(w) && !sys::on_screen(w);
    #[cfg(target_os = "linux")]
    return !visible(w) && !sys::mapped(w);
}

/// Une fenetre CACHEE rendue au jeu est de nouveau la sienne, a sa place d'origine : sans proprietaire, avec son cadre, a l'ecran
/// (Windows), sur la racine (X11). Elle n'est pas montree a sa place (le jeu l'affichera lui-meme).
pub fn hidden_given_back(w: Win) -> bool {
    #[cfg(windows)]
    return !visible(w) && sys::owner(w) == 0 && sys::framed(w) && sys::on_screen(w);
    #[cfg(target_os = "linux")]
    return !visible(w) && !sys::mapped(w) && sys::parent_of(w) == sys::x().1;
}

/// Taille d'origine du faux jeu (870 x 520), bornee a l'ecran sous X11 (Xvfb peut etre plus petit).
pub fn fake_game_size() -> (i32, i32) {
    #[cfg(windows)]
    return (870, 520);
    #[cfg(target_os = "linux")]
    {
        let screen = sys::size(sys::x().1);
        (870.min(screen.0), 520.min(screen.1))
    }
}

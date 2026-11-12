//! Appels SYNCHRONES vers la fenetre du jeu, sur UN fil dedie.
//!
//! Changer le style, le proprietaire, le parent ou la decoupe d'une fenetre envoie un message a son fil et attend la reponse ; or le
//! jeu ne pompe pas ses messages pendant qu'il cree son contexte graphique ou charge ses ressources. Ces appels ne partent donc
//! jamais du fil de l'interface (le launcher « ne repondrait plus ») ni du fil de surveillance (l'adoption suivante attendrait) :
//! ils sont mis en file ici et executes dans l'ordre. Les appels deja asynchrones (`SWP_ASYNCWINDOWPOS`, `PostMessageW`) restent
//! chez l'appelant.

use std::ptr::null_mut;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Mutex};

use windows_sys::Win32::Foundation::HWND;
use windows_sys::Win32::Graphics::Gdi::{CreateRectRgn, SetWindowRgn};
use windows_sys::Win32::UI::WindowsAndMessaging::*;

use super::debug::dbg;
use super::util::hw;
use super::{mark_shown, shared, Shared};

/// Bits de cadre seuls : leur retour (plein ecran quitte, F11) est ce qui declenche une correction. `WS_POPUP` n'en fait PAS partie :
/// le mode integre le pose lui-meme, le tester ici relancerait la correction a chaque synchronisation.
pub(super) const FRAME_BITS: u32 = WS_CAPTION | WS_THICKFRAME | WS_SYSMENU | WS_MINIMIZEBOX | WS_MAXIMIZEBOX | WS_BORDER | WS_DLGFRAME;
/// Bits de style a retirer a une fenetre de jeu adoptee (legende, bordures, boutons).
pub(super) const STRIP: u32 = WS_POPUP | FRAME_BITS;

pub(super) enum Cmd {
    /// Rattacher une fenetre du jeu qui vient d'etre detectee.
    Adopt(isize),
    /// Mode integre : remettre style, proprietaire et style etendu ; `region` : (largeur, hauteur, bande) de la decoupe a appliquer.
    Fix { win: isize, parent: isize, region: Option<(i32, i32, i32)> },
    /// Rendre a ces fenetres toute leur surface (panneau « en jeu » ferme).
    ClearRegion(Vec<isize>),
    /// Afficher la fenetre du jeu sur `rect` (ecran), juste sous `after` (le rideau).
    Show { win: isize, after: isize, rect: (i32, i32, i32, i32) },
    /// Cacher la fenetre du jeu.
    Hide(isize),
    /// Appliquer le dernier calage demande (`PLACE`).
    Place,
    /// Donner le premier plan a cette fenetre (celle du jeu, ou celle du launcher quand le jeu est cache).
    Foreground(isize),
}

/// Une correction est en file ou en cours : les suivantes sont inutiles (la prochaine synchronisation refait le constat).
static FIX_QUEUED: AtomicBool = AtomicBool::new(false);

/// Dernier calage demande, pas encore applique : seul le plus recent compte (le jeu en chargement n'en traite que ~12 par seconde).
#[derive(Default)]
struct Placement {
    /// Fenetre a deplacer et rectangle ecran voulu.
    rect: Option<(isize, (i32, i32, i32, i32))>,
    /// Fenetre a remettre juste sous `after` (le rideau).
    below: Option<(isize, isize)>,
}
static PLACE: Mutex<Placement> = Mutex::new(Placement { rect: None, below: None });
static PLACE_QUEUED: AtomicBool = AtomicBool::new(false);

/// Demarre le fil (une fois, a la creation de l'etat partage).
pub(super) fn spawn(rx: mpsc::Receiver<Cmd>) -> Result<(), String> {
    std::thread::Builder::new().name("cubelink-host-game".into()).spawn(move || serve(rx)).map(|_| ()).map_err(|e| e.to_string())
}

fn serve(rx: mpsc::Receiver<Cmd>) {
    while let Ok(cmd) = rx.recv() {
        let Some(s) = shared() else { continue };
        match cmd {
            Cmd::Adopt(win) => adopt_now(s, win),
            Cmd::Fix { win, parent, region } => {
                unsafe { fix_now(s, hw(win), parent, region) };
                FIX_QUEUED.store(false, Ordering::SeqCst);
            }
            Cmd::Show { win, after, rect: (x, y, w, h) } => unsafe {
                SetWindowPos(hw(win), hw(after), x, y, w, h, SWP_NOACTIVATE | SWP_SHOWWINDOW | SWP_ASYNCWINDOWPOS);
            },
            Cmd::Hide(win) => unsafe {
                SetWindowPos(hw(win), null_mut(), 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE | SWP_HIDEWINDOW | SWP_ASYNCWINDOWPOS);
            },
            Cmd::Place => {
                PLACE_QUEUED.store(false, Ordering::SeqCst);
                let p = std::mem::take(&mut *PLACE.lock().unwrap_or_else(|e| e.into_inner()));
                unsafe {
                    if let Some((win, (x, y, w, h))) = p.rect {
                        SetWindowPos(hw(win), null_mut(), x, y, w, h, SWP_NOZORDER | SWP_NOACTIVATE | SWP_ASYNCWINDOWPOS);
                    }
                    if let Some((win, after)) = p.below {
                        SetWindowPos(hw(win), hw(after), 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE | SWP_ASYNCWINDOWPOS);
                    }
                }
            }
            Cmd::Foreground(win) => {
                let t = std::time::Instant::now();
                let ok = unsafe { SetForegroundWindow(hw(win)) } != 0;
                let ms = t.elapsed().as_millis();
                if !ok || ms >= 50 {
                    dbg(&format!("premier plan : {} en {ms} ms", if ok { "accorde" } else { "REFUSE" }));
                }
            }
            Cmd::ClearRegion(wins) => {
                for g in wins {
                    unsafe { SetWindowRgn(hw(g), null_mut(), 1) };
                }
            }
        }
    }
}

/// Met en file l'adoption de `win`.
pub(super) fn adopt(s: &Shared, win: isize) {
    let _ = s.game.send(Cmd::Adopt(win));
}

/// Met en file une correction de la fenetre du jeu, sauf si une autre attend deja (fusion : une seule a la fois).
pub(super) fn request_fix(s: &Shared, win: isize, parent: isize, region: Option<(i32, i32, i32)>) {
    if FIX_QUEUED.swap(true, Ordering::SeqCst) {
        return;
    }
    if s.game.send(Cmd::Fix { win, parent, region }).is_err() {
        FIX_QUEUED.store(false, Ordering::SeqCst);
    }
}

/// Met en file l'affichage de la fenetre du jeu sur `rect`, sous `after`.
///
/// Pourquoi aucun `SetWindowPos` sur la fenetre du jeu ne part du fil de l'interface : `SWP_ASYNCWINDOWPOS` ne poste la demande que
/// si les deux fils ont des files d'entrees DIFFERENTES ; or une fenetre possedee partage celle de son proprietaire. Depuis le fil de
/// l'interface l'appel est donc synchrone (mesure : ~81 ms par battement pendant le chargement du jeu, jusqu'a 1,2 s). Ce fil-ci n'a
/// aucune fenetre : sa file est distincte, l'appel y est vraiment asynchrone.
pub(super) fn show(s: &Shared, win: isize, after: isize, rect: (i32, i32, i32, i32)) {
    let _ = s.game.send(Cmd::Show { win, after, rect });
}

/// Met en file le masquage de la fenetre du jeu.
pub(super) fn hide(s: &Shared, win: isize) {
    let _ = s.game.send(Cmd::Hide(win));
}

/// Demande un calage de la fenetre du jeu : nouveau rectangle et/ou remise sous `below`. Fusionne avec la demande en attente.
pub(super) fn place(s: &Shared, win: isize, rect: Option<(i32, i32, i32, i32)>, below: Option<isize>) {
    {
        let mut p = PLACE.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(r) = rect {
            p.rect = Some((win, r));
        }
        if let Some(b) = below {
            p.below = Some((win, b));
        }
    }
    if !PLACE_QUEUED.swap(true, Ordering::SeqCst) && s.game.send(Cmd::Place).is_err() {
        PLACE_QUEUED.store(false, Ordering::SeqCst);
    }
}

/// Met en file le passage de `win` au premier plan. Le premier plan change la fenetre active : files d'entrees attachees
/// (fenetre possedee), l'appel attend que le fil du jeu reponde ; mesure : jusqu'a ~3 s de gel du fil appelant pendant le chargement.
pub(super) fn foreground(s: &Shared, win: isize) {
    let _ = s.game.send(Cmd::Foreground(win));
}

/// Met en file le retrait de la decoupe de `wins`.
pub(super) fn clear_region(s: &Shared, wins: Vec<isize>) {
    if !wins.is_empty() {
        let _ = s.game.send(Cmd::ClearRegion(wins));
    }
}

fn adopt_now(s: &Shared, win: isize) {
    unsafe {
        let t = std::time::Instant::now();
        s.mode.adopt(s, win);
        dbg(&format!("adoption de {win:#x} terminee en {} ms, visible={}", t.elapsed().as_millis(), IsWindowVisible(hw(win)) != 0));
        // deja visible (adoption tardive) : equivalent d'un affichage
        let visible = IsWindowVisible(hw(win)) != 0;
        let became_current = {
            let mut st = s.lock();
            let t = st.started.map_or(0, |x| x.elapsed().as_millis());
            st.adopt_log.push(format!("+{t} ms : fenetre adoptee"));
            let first = visible && st.current.is_none();
            if first {
                mark_shown(&mut st, win, None);
            }
            first
        };
        if became_current {
            s.cv.notify_all();
        }
    }
}

/// Aucune icone de barre des taches / Alt+Tab pour une fenetre du jeu : outil (pas de bouton), jamais « fenetre d'application ».
/// Le style etendu n'est lu par le shell que quand la fenetre (re)devient visible : on la cache le temps du changement si besoin.
pub(super) unsafe fn hide_from_taskbar(win: HWND) {
    let ex = GetWindowLongPtrW(win, GWL_EXSTYLE) as u32;
    let want = (ex & !WS_EX_APPWINDOW) | WS_EX_TOOLWINDOW;
    if want == ex {
        return;
    }
    let visible = IsWindowVisible(win) != 0;
    if visible {
        ShowWindow(win, SW_HIDE);
    }
    SetWindowLongPtrW(win, GWL_EXSTYLE, want as isize);
    SetWindowPos(win, null_mut(), 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE | SWP_FRAMECHANGED);
    if visible {
        ShowWindow(win, SW_SHOWNA);
    }
}

/// Decoupe la bande de droite de la fenetre du jeu : le launcher (dessous) y apparait, le reste du jeu reste visible et intact.
unsafe fn apply_region(g: HWND, w: i32, h: i32, pw: i32) {
    if pw > 0 && pw < w {
        SetWindowRgn(g, CreateRectRgn(0, 0, w - pw, h), 1);
    } else {
        SetWindowRgn(g, null_mut(), 1);
    }
}

/// Corrections de la fenetre du jeu (style etendu, proprietaire, cadre, decoupe) : elles peuvent bloquer tant que le jeu ne pompe
/// pas ses messages.
unsafe fn fix_now(s: &Shared, g: HWND, parent: isize, region: Option<(i32, i32, i32)>) {
    hide_from_taskbar(g);
    if GetWindow(g, GW_OWNER) as isize != parent {
        SetWindowLongPtrW(g, GWLP_HWNDPARENT, parent);
    }
    let style = GetWindowLongPtrW(g, GWL_STYLE) as u32;
    if style & FRAME_BITS != 0 || style & WS_POPUP == 0 {
        SetWindowLongPtrW(g, GWL_STYLE, ((style & !STRIP) | WS_POPUP) as isize);
        SetWindowPos(g, null_mut(), 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE | SWP_FRAMECHANGED);
    }
    if let Some((w, h, pw)) = region {
        apply_region(g, w, h, pw);
        s.lock().region = if pw > 0 { (w, h, pw) } else { (0, 0, 0) };
    }
}

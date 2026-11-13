//! Appels SYNCHRONES vers la fenetre du jeu, sur UN fil dedie.
//!
//! Changer le style, le proprietaire ou la decoupe d'une fenetre envoie un message a son fil et attend la reponse ; or le jeu ne
//! pompe pas ses messages pendant qu'il cree son contexte graphique ou charge ses ressources. Ces appels ne partent donc jamais du
//! fil de l'interface (le launcher « ne repondrait plus ») ni du fil de surveillance (l'adoption suivante attendrait) : ils sont mis
//! en file ici et executes dans l'ordre. Les appels deja asynchrones (`SWP_ASYNCWINDOWPOS`, `PostMessageW`) restent chez l'appelant.
//!
//! C'est aussi ici qu'une fenetre est RENDUE au jeu (repli) : meme fil, donc toujours APRES les corrections deja en file, et toute
//! commande arrivee ensuite pour une fenetre rendue est ignoree (`ours`).

use std::ptr::null_mut;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Mutex};
use std::time::{Duration, Instant};

use windows_sys::Win32::Foundation::{HWND, RECT};
use windows_sys::Win32::Graphics::Gdi::{CreateRectRgn, GetMonitorInfoW, MonitorFromWindow, SetWindowRgn, MONITORINFO, MONITOR_DEFAULTTONULL, MONITOR_DEFAULTTOPRIMARY};
use windows_sys::Win32::UI::WindowsAndMessaging::*;

use super::debug::dbg;
use super::embedded::{self, Adoption};
use super::util::{class_name, hw};
use super::{mark_shown, shared, Refused, Shared};
use crate::guard::returned_rect;
use crate::health::{FallbackReason, HostEvent};

/// Bits de cadre seuls : leur retour (plein ecran quitte, F11) est ce qui declenche une correction. `WS_POPUP` n'en fait PAS partie :
/// le mode integre le pose lui-meme, le tester ici relancerait la correction a chaque synchronisation.
pub(super) const FRAME_BITS: u32 = WS_CAPTION | WS_THICKFRAME | WS_SYSMENU | WS_MINIMIZEBOX | WS_MAXIMIZEBOX | WS_BORDER | WS_DLGFRAME;
/// Bits de style a retirer a une fenetre de jeu adoptee (legende, bordures, boutons).
pub(super) const STRIP: u32 = WS_POPUP | FRAME_BITS;

/// Une fenetre du jeu telle qu'elle etait AVANT qu'on y touche (releve a sa detection) : ce qu'il faut pour la rendre.
#[derive(Clone, Copy, Debug)]
pub(super) struct Origin {
    pub style: u32,
    pub ex: u32,
    pub owner: isize,
    /// Rectangle ecran (x, y, largeur, hauteur).
    pub rect: (i32, i32, i32, i32),
}

impl Origin {
    /// Lectures locales (aucun message vers le jeu) : utilisable depuis le fil de surveillance, verrou d'etat tenu.
    pub(super) unsafe fn of(win: HWND) -> Origin {
        let mut r: RECT = std::mem::zeroed();
        GetWindowRect(win, &mut r);
        Origin {
            style: GetWindowLongPtrW(win, GWL_STYLE) as u32,
            ex: GetWindowLongPtrW(win, GWL_EXSTYLE) as u32,
            owner: GetWindow(win, GW_OWNER) as isize,
            rect: (r.left, r.top, r.right - r.left, r.bottom - r.top),
        }
    }
}

/// Une fenetre a rendre au jeu.
#[derive(Clone, Copy, Debug)]
pub(super) struct Returned {
    pub win: isize,
    pub origin: Origin,
    /// LA fenetre du jeu : rendue visible et avec un cadre quoi qu'il arrive (les autres gardent leur visibilite).
    pub main: bool,
}

pub(super) enum Cmd {
    /// Rattacher une fenetre du jeu qui vient d'etre detectee.
    Adopt(isize),
    /// Remettre style, proprietaire et style etendu ; `region` : (largeur, hauteur, bande) de la decoupe a appliquer.
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
    /// Repli : rendre ces fenetres au jeu, puis annoncer `HostEvent::Fallback`.
    Release { wins: Vec<Returned>, reason: FallbackReason, detail: String },
    /// Verifier que les fenetres rendues le sont restees ; les rendre a nouveau sinon.
    Reassert(Vec<Returned>),
    /// Lancement cache, hors Scene : montrer cette fenetre du jeu, que personne d'autre n'affichera (lancement `gen`).
    Rescue { win: isize, gen: u64 },
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

/// `win` est toujours une fenetre de jeu dont on s'occupe. Faux des qu'elle a ete rendue (repli) ou que son jeu est parti : une
/// commande mise en file juste avant ne doit pas lui retirer a nouveau son cadre ni la deplacer.
fn ours(s: &Shared, win: isize) -> bool {
    s.lock().origin.contains_key(&win)
}

fn serve(rx: mpsc::Receiver<Cmd>) {
    while let Ok(cmd) = rx.recv() {
        let Some(s) = shared() else { continue };
        match cmd {
            Cmd::Adopt(win) => {
                if ours(s, win) {
                    adopt_now(s, win);
                }
            }
            Cmd::Fix { win, parent, region } => {
                // jamais sur une fenetre a qui on a demande de se fermer : elle se defait, il n'y a plus rien a corriger
                if ours(s, win) && !s.lock().close_asked {
                    unsafe { fix_now(s, hw(win), parent, region) };
                }
                FIX_QUEUED.store(false, Ordering::SeqCst);
            }
            Cmd::Show { win, after, rect: (x, y, w, h) } => {
                if ours(s, win) {
                    unsafe {
                        // Le cadre D'ABORD, la ou est la fenetre (hors ecran avant sa premiere pose), puis la pose : une fenetre a
                        // qui le jeu a remis son cadre depuis l'adoption (mesure : NeoForge) n'est jamais amenee sur la zone avec
                        // lui. Synchrone, sur ce fil : la pose, postee ensuite, passe forcement apres.
                        if strip_frame(hw(win)) {
                            s.log("cadre remis par le jeu avant la pose : retire hors ecran, puis pose");
                        }
                        SetWindowPos(hw(win), hw(after), x, y, w, h, SWP_NOACTIVATE | SWP_SHOWWINDOW | SWP_ASYNCWINDOWPOS);
                    }
                }
            }
            Cmd::Rescue { win, gen } => unsafe {
                let g = hw(win);
                if s.lock().gen == gen && IsWindow(g) != 0 && IsWindowVisible(g) == 0 {
                    give_back(s.parent, &Returned { win, origin: Origin::of(g), main: true });
                    s.lock().released.push(win);
                    s.log(format!("lancement cache : fenetre du jeu ({}) montree hors de la Scene (le jeu ne l'aurait jamais affichee)", class_name(g)));
                }
            },
            Cmd::Hide(win) => {
                if ours(s, win) {
                    unsafe { SetWindowPos(hw(win), null_mut(), 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE | SWP_HIDEWINDOW | SWP_ASYNCWINDOWPOS) };
                }
            }
            Cmd::Place => {
                PLACE_QUEUED.store(false, Ordering::SeqCst);
                let p = std::mem::take(&mut *PLACE.lock().unwrap_or_else(|e| e.into_inner()));
                unsafe {
                    if let Some((win, (x, y, w, h))) = p.rect.filter(|(win, _)| ours(s, *win)) {
                        SetWindowPos(hw(win), null_mut(), x, y, w, h, SWP_NOZORDER | SWP_NOACTIVATE | SWP_ASYNCWINDOWPOS);
                    }
                    if let Some((win, after)) = p.below.filter(|(win, _)| ours(s, *win)) {
                        SetWindowPos(hw(win), hw(after), 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE | SWP_ASYNCWINDOWPOS);
                    }
                }
            }
            Cmd::Foreground(win) => {
                let t = Instant::now();
                let ok = unsafe { SetForegroundWindow(hw(win)) } != 0;
                let ms = t.elapsed().as_millis();
                if !ok || ms >= 50 {
                    dbg(&format!("premier plan : {} en {ms} ms", if ok { "accorde" } else { "REFUSE" }));
                }
            }
            Cmd::ClearRegion(wins) => {
                for g in wins.into_iter().filter(|g| ours(s, *g)) {
                    let (cur, (w, h, pw)) = {
                        let st = s.lock();
                        (st.current == Some(g), st.region)
                    };
                    unsafe {
                        if cur && w > 0 {
                            // LA fenetre du jeu : la bande se referme en glissant (le panneau s'efface dans le meme temps), la
                            // region redevient pleine
                            animate_region(s, hw(g), w, h, pw, 0);
                            s.lock().region = (w, h, 0);
                        } else {
                            SetWindowRgn(hw(g), null_mut(), 1);
                        }
                    }
                }
            }
            Cmd::Release { wins, reason, detail } => release_now(s, wins, reason, detail),
            Cmd::Reassert(wins) => {
                let mut again = 0;
                for r in &wins {
                    unsafe {
                        if still_taken(r) {
                            give_back(s.parent, r);
                            again += 1;
                        }
                    }
                }
                if again > 0 {
                    s.log(format!("repli : {again} fenetre(s) rendue(s) a nouveau (un deplacement en attente l'avait reprise)"));
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

/// Met en file l'affichage, hors Scene, d'une fenetre du jeu restee cachee (lancement cache).
pub(super) fn rescue(s: &Shared, win: isize, gen: u64) {
    let _ = s.game.send(Cmd::Rescue { win, gen });
}

/// Retire le cadre de `g` s'il en a un (ou s'il n'est pas `WS_POPUP`), sans la deplacer. Vrai si des bits de cadre etaient presents.
/// Synchrone vers le jeu.
unsafe fn strip_frame(g: HWND) -> bool {
    let style = GetWindowLongPtrW(g, GWL_STYLE) as u32;
    if (style & FRAME_BITS == 0 && style & WS_POPUP != 0) || IsWindow(g) == 0 {
        return false;
    }
    SetWindowLongPtrW(g, GWL_STYLE, ((style & !STRIP) | WS_POPUP) as isize);
    SetWindowPos(g, null_mut(), 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE | SWP_FRAMECHANGED);
    style & FRAME_BITS != 0
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

/// Met en file la restitution de `wins` ; `HostEvent::Fallback` est emis quand elles sont REELLEMENT rendues (un jeu qui ne pompe
/// pas ses messages retarde donc l'annonce : on n'annonce jamais « ton jeu est dans sa propre fenetre » avant que ce soit vrai).
pub(super) fn release(s: &Shared, wins: Vec<Returned>, reason: FallbackReason, detail: String) {
    if let Err(mpsc::SendError(Cmd::Release { reason, detail, .. })) = s.game.send(Cmd::Release { wins, reason, detail }) {
        // fil arrete (ne devrait pas arriver) : l'appelant doit quand meme apprendre que la Scene a renonce
        s.emit(HostEvent::Fallback { reason, detail });
    }
}

fn adopt_now(s: &Shared, win: isize) {
    unsafe {
        let t = Instant::now();
        if embedded::adopt(s, win) == Adoption::Gone {
            return;
        }
        let ms = t.elapsed().as_millis();
        let owned = note_owner(s, win);
        s.log(format!("fenetre {} adoptee en {ms} ms{}", class_name(hw(win)), if owned { "" } else { " (proprietaire refuse)" }));
        // deja visible (adoption tardive) : equivalent d'un affichage
        if IsWindowVisible(hw(win)) != 0 {
            let mut st = s.lock();
            if st.current.is_none() && st.origin.contains_key(&win) {
                mark_shown(&mut st, win);
            }
        }
        announce(s, win);
    }
}

/// Releve, apres une tentative TERMINEE, si `win` est bien possedee par la fenetre du launcher, et tient le compte des refus pour
/// la garde (`crate::guard`). Vrai si elle l'est (ou n'existe plus : rien a reprocher).
fn note_owner(s: &Shared, win: isize) -> bool {
    let (owned, owner) = unsafe {
        let owner = GetWindow(hw(win), GW_OWNER) as isize;
        (IsWindow(hw(win)) == 0 || owner == s.parent, owner)
    };
    enum Note {
        Nothing,
        /// Premier refus d'une fenetre qui peut etre LA fenetre du jeu.
        Refused,
        /// Premier refus d'une AUTRE fenetre alors que LA fenetre du jeu est tenue : sans consequence.
        Shrugged,
    }
    let note = {
        let mut st = s.lock();
        if !st.origin.contains_key(&win) {
            return owned; // rendue entre-temps
        }
        if owned {
            st.refused.remove(&win);
            Note::Nothing
        } else if st.current.is_some_and(|c| c != win) {
            // LA fenetre du jeu est connue et ce n'est pas celle-ci : le refus d'une fenetre secondaire ne dit rien de la Scene
            // (le joueur voit et joue son jeu). Ni compte, ni nouvelle tentative, ni repli : une ligne de journal.
            st.refused.remove(&win);
            if st.shrugged.insert(win) {
                Note::Shrugged
            } else {
                Note::Nothing
            }
        } else if let Some(r) = st.refused.get_mut(&win) {
            r.attempts += 1;
            Note::Nothing
        } else {
            let now = Instant::now();
            st.refused.insert(win, Refused { since: now, attempts: 1, asked: now });
            Note::Refused
        }
    };
    match note {
        Note::Refused => s.log(format!("rattachement refuse : proprietaire de la fenetre = {owner:#x} (attendu {:#x}), nouvelles tentatives", s.parent)),
        Note::Shrugged => s.log(format!("fenetre secondaire ({}) non rattachee : sans consequence, la fenetre du jeu est dans la Scene", unsafe { class_name(hw(win)) })),
        Note::Nothing => {}
    }
    owned
}

/// Annonce `HostEvent::Adopted` (une fois par lancement) : `win` est LA fenetre du jeu, affichee et possedee par le launcher.
pub(super) fn announce(s: &Shared, win: isize) {
    let class = unsafe {
        if IsWindow(hw(win)) == 0 || GetWindow(hw(win), GW_OWNER) as isize != s.parent {
            return;
        }
        class_name(hw(win))
    };
    let after_ms = {
        let mut st = s.lock();
        if st.announced || st.current != Some(win) {
            return;
        }
        st.announced = true;
        st.started.map_or(0, |t| t.elapsed().as_millis() as u64)
    };
    s.log(format!("fenetre du jeu ({class}) rattachee a la Scene, {after_ms} ms apres le lancement"));
    s.emit(HostEvent::Adopted { class, after_ms });
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
/// Sans panneau : region PLEINE, jamais aucune region. En plein ecran, une fenetre sans region qui couvre l'ecran est affichee
/// par Windows sans composition (« independent flip ») ; en sortir, ou poser une premiere region, coute une image noire (vu
/// par le joueur a chaque Ctrl+Maj+C, en plein ecran seulement). Avec une region permanente, ouvrir le panneau n'en change que
/// la forme.
unsafe fn apply_region(g: HWND, w: i32, h: i32, pw: i32) {
    let keep = if pw > 0 && pw < w { w - pw } else { w };
    SetWindowRgn(g, CreateRectRgn(0, 0, keep, h), 1);
}

/// Duree du glissement de la bande du panneau ; meme duree et meme courbe que le panneau cote interface (`overlay.css`).
const REGION_SLIDE: std::time::Duration = std::time::Duration::from_millis(180);

/// Pur : largeur de bande a l'instant `t` (0..=1) d'un glissement de `from` a `to`, sortie douce (cubique).
pub(crate) fn region_step(from: i32, to: i32, t: f32) -> i32 {
    let t = t.clamp(0.0, 1.0);
    let e = 1.0 - (1.0 - t).powi(3);
    from + ((to - from) as f32 * e).round() as i32
}

/// Fait glisser la bande decoupee de `from` a `to` (~60 images/s), puis pose la decoupe finale. Sur ce fil (celui de la fenetre
/// du jeu) : `SetWindowRgn` attend le jeu. S'interrompt si la fenetre n'est plus a nous (jeu ferme, repli).
unsafe fn animate_region(s: &Shared, g: HWND, w: i32, h: i32, from: i32, to: i32) {
    if from != to && from >= 0 && to >= 0 {
        let start = Instant::now();
        loop {
            let t = start.elapsed().as_secs_f32() / REGION_SLIDE.as_secs_f32();
            if t >= 1.0 || IsWindow(g) == 0 || !ours(s, g as isize) {
                break;
            }
            apply_region(g, w, h, region_step(from, to, t));
            std::thread::sleep(std::time::Duration::from_millis(12));
        }
    }
    apply_region(g, w, h, to);
}

/// Corrections de la fenetre du jeu (style etendu, proprietaire, cadre, decoupe) : elles peuvent bloquer tant que le jeu ne pompe
/// pas ses messages.
unsafe fn fix_now(s: &Shared, g: HWND, parent: isize, region: Option<(i32, i32, i32)>) {
    if IsWindow(g) == 0 {
        return; // detruite (jeu ferme ou tue) : ses styles se lisent a zero, ce qui ressemblerait a un cadre a corriger
    }
    hide_from_taskbar(g);
    if GetWindow(g, GW_OWNER) as isize != parent {
        SetWindowLongPtrW(g, GWLP_HWNDPARENT, parent);
        // Aller-retour synchrone vers le jeu : quand il revient, le jeu a traite le changement. Le proprietaire relu plus bas est
        // donc celui qui tient vraiment, pas une valeur que le jeu s'apprete a defaire.
        SetWindowPos(g, null_mut(), 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE | SWP_FRAMECHANGED);
    }
    let style = GetWindowLongPtrW(g, GWL_STYLE) as u32;
    // `IsWindow` relu APRES le style : une fenetre detruite entre-temps rend un style nul, qui n'est pas un cadre remis
    if (style & FRAME_BITS != 0 || style & WS_POPUP == 0) && IsWindow(g) != 0 {
        SetWindowLongPtrW(g, GWL_STYLE, ((style & !STRIP) | WS_POPUP) as isize);
        SetWindowPos(g, null_mut(), 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE | SWP_FRAMECHANGED);
        if style & FRAME_BITS != 0 {
            log_style_fix(s);
        }
    }
    if let Some((w, h, pw)) = region {
        let (aw, ah, apw) = s.lock().region;
        // meme taille de zone : la bande s'ouvre ou se ferme en glissant ; zone redimensionnee : tout de suite (suivre la fenetre)
        let from = if (aw, ah) == (w, h) { apw } else { pw };
        animate_region(s, g, w, h, from, pw);
        s.lock().region = (w, h, pw);
    }
    if note_owner(s, g as isize) {
        // Rattachement obtenu apres coup. Une fenetre affichee pendant qu'elle n'etait pas possedee n'a pas ete reconnue comme LA
        // fenetre du jeu (l'evenement d'affichage l'a seulement garee) : c'est le moment.
        if IsWindowVisible(g) != 0 {
            let mut st = s.lock();
            if st.current.is_none() && st.adopted.contains(&(g as isize)) {
                mark_shown(&mut st, g as isize);
            }
        }
        announce(s, g as isize);
    }
}

/// Journal de la correction du cadre, au plus une ligne par seconde : un jeu qui insisterait ne doit pas noyer le journal. Le
/// libelle dit ce qui est constate (des bits de cadre sur une fenetre vivante), pas pourquoi : le jeu repose son style en quittant
/// le plein ecran (F11), mais aussi en changeant de taille ou d'attributs pendant son demarrage (mesure : NeoForge).
fn log_style_fix(s: &Shared) {
    static LAST: Mutex<Option<Instant>> = Mutex::new(None);
    {
        let mut l = LAST.lock().unwrap_or_else(|e| e.into_inner());
        if l.is_some_and(|t| t.elapsed() < Duration::from_secs(1)) {
            return;
        }
        *l = Some(Instant::now());
    }
    s.log("cadre de la fenetre remis par le jeu : retire a nouveau");
}

/// Zone de travail (gauche, haut, droite, bas) de l'ecran ou se trouve la fenetre du launcher (a defaut : l'ecran principal).
unsafe fn work_area(parent: isize) -> (i32, i32, i32, i32) {
    let mon = MonitorFromWindow(hw(parent), MONITOR_DEFAULTTOPRIMARY);
    let mut mi: MONITORINFO = std::mem::zeroed();
    mi.cbSize = std::mem::size_of::<MONITORINFO>() as u32;
    if GetMonitorInfoW(mon, &mut mi) == 0 {
        return (0, 0, GetSystemMetrics(SM_CXSCREEN), GetSystemMetrics(SM_CYSCREEN));
    }
    (mi.rcWork.left, mi.rcWork.top, mi.rcWork.right, mi.rcWork.bottom)
}

/// Style rendu a une fenetre. LA fenetre du jeu retrouve un cadre complet meme si son style d'origine n'en avait pas (jeu regle
/// en plein ecran) : une fenetre sans barre de titre ne peut pas etre deplacee par le joueur.
fn returned_style(r: &Returned) -> u32 {
    let style = r.origin.style & !(WS_VISIBLE | WS_MINIMIZE | WS_MAXIMIZE);
    if r.main && style & WS_CAPTION != WS_CAPTION {
        (style & !WS_POPUP) | WS_OVERLAPPEDWINDOW
    } else {
        style
    }
}

/// Rend sa fenetre au jeu : plus de decoupe, proprietaire et styles d'origine (cadre, bouton de barre des taches), position a
/// l'ecran. LA fenetre du jeu (et toute fenetre deja visible) est montree, centree sur l'ecran du launcher ; une fenetre que le jeu
/// n'a pas encore affichee reste cachee, a sa place d'origine : il l'affichera lui-meme, comme si de rien n'etait. En lancement
/// cache, ou il ne l'affichera jamais, l'appelant marque `main` toute candidate principale (voir `fallback`) : elle est montree.
/// Appels synchrones, dans cet ordre : a leur retour la fenetre EST rendue.
unsafe fn give_back(parent: isize, r: &Returned) {
    let g = hw(r.win);
    if IsWindow(g) == 0 {
        return;
    }
    let visible = IsWindowVisible(g) != 0;
    let show = r.main || visible;
    SetWindowRgn(g, null_mut(), 0);
    if visible {
        ShowWindow(g, SW_HIDE); // le shell ne relit le style etendu (bouton de barre des taches) qu'au reaffichage
    }
    SetWindowLongPtrW(g, GWLP_HWNDPARENT, r.origin.owner);
    SetWindowLongPtrW(g, GWL_STYLE, returned_style(r) as isize);
    let ex = if r.main { r.origin.ex & !WS_EX_TOOLWINDOW } else { r.origin.ex };
    SetWindowLongPtrW(g, GWL_EXSTYLE, ex as isize);
    let (ox, oy, ow, oh) = r.origin.rect;
    if show {
        let (x, y, w, h) = returned_rect(work_area(parent), (ow, oh));
        // devant le launcher ; le premier plan seulement si le joueur etait dans le launcher (jamais de vol de focus a une autre application)
        let front = GetForegroundWindow() == hw(parent);
        SetWindowPos(g, HWND_TOP, x, y, w, h, SWP_FRAMECHANGED | SWP_SHOWWINDOW | if front { 0 } else { SWP_NOACTIVATE });
        if front {
            SetForegroundWindow(g);
        }
    } else {
        // releve hors ecran (ne devrait pas arriver : la detection precede le garage) : place visible quand meme
        let (x, y, w, h) = if ox <= -30000 || oy <= -30000 { returned_rect(work_area(parent), (ow, oh)) } else { (ox, oy, ow, oh) };
        SetWindowPos(g, null_mut(), x, y, w, h, SWP_NOZORDER | SWP_NOACTIVATE | SWP_FRAMECHANGED);
    }
}

/// Une fenetre rendue ne l'est plus : hors de tout ecran, a nouveau possedee, cachee alors qu'elle doit se voir, ou sans cadre.
/// Lectures locales. Une fenetre reduite par le joueur est « hors ecran » pour Windows : on n'y touche pas.
unsafe fn still_taken(r: &Returned) -> bool {
    let g = hw(r.win);
    if IsWindow(g) == 0 || IsIconic(g) != 0 {
        return false;
    }
    let offscreen = MonitorFromWindow(g, MONITOR_DEFAULTTONULL).is_null();
    let owned = GetWindow(g, GW_OWNER) as isize != r.origin.owner;
    let hidden = r.main && IsWindowVisible(g) == 0;
    let bare = r.main && GetWindowLongPtrW(g, GWL_STYLE) as u32 & WS_CAPTION != WS_CAPTION;
    offscreen || owned || hidden || bare
}

/// Echeances (ms apres la restitution) des verifications : un deplacement asynchrone deja poste au jeu (garage hors ecran, calage)
/// peut etre traite APRES nos appels synchrones, qui passent devant lui dans la file du jeu.
const REASSERT_AT_MS: [u64; 3] = [150, 600, 1500];

fn release_now(s: &Shared, wins: Vec<Returned>, reason: FallbackReason, detail: String) {
    let t = Instant::now();
    let alive: Vec<Returned> = wins.into_iter().filter(|r| unsafe { IsWindow(hw(r.win)) } != 0).collect();
    for r in &alive {
        unsafe { give_back(s.parent, r) };
    }
    s.log(format!("repli ({}) : {} fenetre(s) rendue(s) au jeu en {} ms", reason.as_str(), alive.len(), t.elapsed().as_millis()));
    s.emit(HostEvent::Fallback { reason, detail });
    if alive.is_empty() {
        return;
    }
    let tx = s.game.clone();
    let _ = std::thread::Builder::new().name("cubelink-host-reassert".into()).spawn(move || {
        let mut slept = 0;
        for at in REASSERT_AT_MS {
            std::thread::sleep(Duration::from_millis(at - slept));
            slept = at;
            if tx.send(Cmd::Reassert(alive.clone())).is_err() {
                return;
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::region_step;

    #[test]
    fn glissement_de_la_bande() {
        assert_eq!(region_step(0, 380, 0.0), 0);
        assert_eq!(region_step(0, 380, 1.0), 380);
        assert_eq!(region_step(380, 0, 1.0), 0);
        assert_eq!(region_step(0, 380, 2.0), 380, "borne");
        // sortie douce : a mi-temps, deja bien plus de la moitie du chemin
        assert!(region_step(0, 380, 0.5) > 300);
        assert!(region_step(380, 0, 0.5) < 80);
        // monotone
        let v: Vec<i32> = (0..=10).map(|i| region_step(0, 380, i as f32 / 10.0)).collect();
        assert!(v.windows(2).all(|p| p[0] <= p[1]), "{v:?}");
    }
}

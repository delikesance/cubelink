//! Outils communs aux tests d'integration Win32 de la Scene.
//!
//! Chaque fichier de `tests/` est un processus a part (un seul hote par processus) avec UN test : il cree une fenetre « launcher »
//! sur son propre fil, y installe l'hote, lance un faux jeu (`src/bin/fake-game.rs`) et verifie le resultat par des LECTURES Win32
//! (proprietaire, styles, rectangle, visibilite). Aucune entree souris / clavier n'est envoyee ni simulee, nulle part.
//!
//! Ces tests ont besoin d'une session de bureau (creation de fenetres, hooks WinEvent) : poste de developpement ou runner CI
//! `windows-latest`. Ils n'ont PAS besoin du premier plan ni d'une fenetre active.

#![allow(dead_code)] // chaque scenario n'utilise qu'une partie des outils

use std::ffi::c_void;
use std::ptr::{null, null_mut};
use std::time::{Duration, Instant};

use windows_sys::Win32::Foundation::{CloseHandle, HANDLE, HWND, LPARAM, POINT, RECT, WAIT_OBJECT_0};
use windows_sys::Win32::System::Threading::{TerminateProcess, WaitForSingleObject, PROCESS_INFORMATION, STARTF_USESHOWWINDOW, STARTUPINFOW};
use windows_sys::Win32::Graphics::Gdi::{ClientToScreen, CreateRectRgn, DeleteObject, GetRgnBox, GetWindowRgn, MonitorFromWindow, MONITOR_DEFAULTTONULL};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::UI::HiDpi::{SetProcessDpiAwarenessContext, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2};
use windows_sys::Win32::UI::WindowsAndMessaging::*;

const CREATE_NO_WINDOW: u32 = 0x0800_0000;

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

// ------------------------------------------------------------------------------------------------ fenetre « launcher »

/// Fenetre de premier niveau du test, dans le role de celle du launcher. Detruite a la fin du test, meme en cas d'echec.
pub struct Parent {
    pub hwnd: isize,
}

impl Parent {
    /// A appeler sur le fil du test : c'est sa boucle de messages (`pump`) qui anime le rideau.
    pub fn new() -> Parent {
        unsafe {
            // comme le launcher : coordonnees en pixels physiques (sinon Windows les arrondit et les relectures different d'un pixel)
            SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
            let hinst = GetModuleHandleW(null());
            let cls = wide("CubelinkTestParent");
            let wc = WNDCLASSW {
                style: 0,
                lpfnWndProc: Some(DefWindowProcW),
                cbClsExtra: 0,
                cbWndExtra: 0,
                hInstance: hinst,
                hIcon: null_mut(),
                hCursor: null_mut(),
                hbrBackground: null_mut(),
                lpszMenuName: null(),
                lpszClassName: cls.as_ptr(),
            };
            RegisterClassW(&wc);
            let title = wide("Test de la Scene Cubelink");
            let hwnd = CreateWindowExW(0, cls.as_ptr(), title.as_ptr(), WS_OVERLAPPEDWINDOW | WS_CLIPCHILDREN, 120, 80, 960, 640, null_mut(), null_mut(), hinst, null());
            assert!(!hwnd.is_null(), "CreateWindowExW a echoue : ces tests ont besoin d'une session de bureau");
            ShowWindow(hwnd, SW_SHOWNOACTIVATE);
            Parent { hwnd: hwnd as isize }
        }
    }

    pub fn client_size(&self) -> (i32, i32) {
        unsafe {
            let mut r: RECT = std::mem::zeroed();
            GetClientRect(self.hwnd as HWND, &mut r);
            (r.right, r.bottom)
        }
    }

    /// Rectangle ecran d'une zone `(x, y, largeur, hauteur)` donnee en coordonnees client.
    pub fn to_screen(&self, zone: (i32, i32, i32, i32)) -> (i32, i32, i32, i32) {
        unsafe {
            let mut pt = POINT { x: zone.0, y: zone.1 };
            ClientToScreen(self.hwnd as HWND, &mut pt);
            (pt.x, pt.y, zone.2, zone.3)
        }
    }
}

impl Drop for Parent {
    fn drop(&mut self) {
        unsafe { DestroyWindow(self.hwnd as HWND) };
    }
}

/// Traite les messages en attente du fil du test (minuteur et messages du rideau, zone de jeu).
pub fn pump() {
    unsafe {
        let mut msg: MSG = std::mem::zeroed();
        while PeekMessageW(&mut msg, null_mut(), 0, 0, PM_REMOVE) != 0 {
            TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
}

/// Pompe les messages pendant `d`.
pub fn pump_for(d: Duration) {
    let end = Instant::now() + d;
    while Instant::now() < end {
        pump();
        std::thread::sleep(Duration::from_millis(2));
    }
}

/// Pompe les messages jusqu'a ce que `cond` soit vraie ; echec du test (avec `what`) si elle ne l'est pas dans `timeout`.
#[track_caller]
pub fn wait_until(what: &str, timeout: Duration, mut cond: impl FnMut() -> bool) {
    let end = Instant::now() + timeout;
    loop {
        pump();
        if cond() {
            return;
        }
        assert!(Instant::now() < end, "delai depasse ({} s) : {what}", timeout.as_secs());
        std::thread::sleep(Duration::from_millis(2));
    }
}

/// Le rideau de l'hote de CE processus (un vrai launcher, ou un autre test, peut avoir le sien sur le meme bureau).
pub fn curtain() -> isize {
    *windows_of(std::process::id(), "CubelinkCurtain").first().expect("fenetre du rideau introuvable")
}

// ------------------------------------------------------------------------------------------------ faux jeu

/// Un faux jeu en cours d'execution. TUE par son pid a la fin du test, meme en cas d'echec (`Drop` pendant le deroulement du panic).
pub struct FakeGame {
    process: isize,
    pid: u32,
}

// `CreateProcessW` declare ici : la bibliotheque standard ne sait pas (hors nightly) passer `wShowWindow`, et la declaration de
// `windows-sys` demanderait une fonctionnalite de plus pour un seul appel de test.
#[link(name = "kernel32")]
extern "system" {
    fn CreateProcessW(app: *const u16, cmd: *mut u16, pa: *const c_void, ta: *const c_void, inherit: i32, flags: u32, env: *const c_void, dir: *const u16, si: *const STARTUPINFOW, pi: *mut PROCESS_INFORMATION) -> i32;
}

impl FakeGame {
    /// Arguments : voir `src/bin/fake-game.rs`.
    pub fn spawn(args: &[&str]) -> FakeGame {
        Self::start(args, false)
    }

    /// Comme `spawn`, lance CACHE : `STARTUPINFO` avec `STARTF_USESHOWWINDOW` et `SW_HIDE`, comme le launcher en lancement cache.
    pub fn spawn_hidden(args: &[&str]) -> FakeGame {
        Self::start(args, true)
    }

    fn start(args: &[&str], hidden: bool) -> FakeGame {
        // arguments du faux jeu : des mots simples, sans espace ni guillemet
        let line = format!("\"{}\" {}", env!("CARGO_BIN_EXE_fake-game"), args.join(" "));
        let mut line: Vec<u16> = line.encode_utf16().chain(std::iter::once(0)).collect();
        unsafe {
            let mut si: STARTUPINFOW = std::mem::zeroed();
            si.cb = std::mem::size_of::<STARTUPINFOW>() as u32;
            if hidden {
                si.dwFlags = STARTF_USESHOWWINDOW;
                si.wShowWindow = SW_HIDE as u16;
            }
            let mut pi: PROCESS_INFORMATION = std::mem::zeroed();
            let ok = CreateProcessW(null(), line.as_mut_ptr(), null(), null(), 0, CREATE_NO_WINDOW, null(), null(), &si, &mut pi);
            assert!(ok != 0, "lancement du faux jeu");
            CloseHandle(pi.hThread);
            FakeGame { process: pi.hProcess as isize, pid: pi.dwProcessId }
        }
    }

    pub fn pid(&self) -> u32 {
        self.pid
    }

    /// Le processus s'est arrete.
    pub fn exited(&mut self) -> bool {
        unsafe { WaitForSingleObject(self.process as HANDLE, 0) == WAIT_OBJECT_0 }
    }

    /// La fenetre de premier niveau de classe `class` du faux jeu, si elle existe.
    /// (La fenetre d'aide de GLFW 3.3, de meme classe que la fenetre du jeu, n'en est pas une.)
    pub fn window(&self, class: &str) -> Option<isize> {
        windows_of(self.pid(), class).into_iter().find(|h| title(*h) != GLFW_HELPER_TITLE)
    }
}

impl Drop for FakeGame {
    fn drop(&mut self) {
        unsafe {
            TerminateProcess(self.process as HANDLE, 1);
            WaitForSingleObject(self.process as HANDLE, 5000);
            CloseHandle(self.process as HANDLE);
        }
    }
}

pub const GLFW_HELPER_TITLE: &str = "GLFW message window";

/// Titre d'une fenetre, lu sans envoyer de message a son processus.
pub fn title(h: isize) -> String {
    let mut buf = [0u16; 128];
    let n = unsafe { InternalGetWindowText(h as HWND, buf.as_mut_ptr(), 128) };
    String::from_utf16_lossy(&buf[..n.clamp(0, 128) as usize])
}

/// Etat du bureau autour de la fenetre parente `parent`, pour le message d'un echec : une fenetre possedee (rideau, jeu adopte)
/// disparait avec sa proprietaire si celle-ci est reduite ou cachee par autre chose que le test.
pub fn desktop_state(parent: isize) -> String {
    let fg = unsafe { GetForegroundWindow() } as isize;
    let mut pid = 0u32;
    unsafe { GetWindowThreadProcessId(fg as HWND, &mut pid) };
    format!("fenetre parente : visible {}, reduite {} ; premier plan : {} (processus {pid}, le test est {})", visible(parent), unsafe { IsIconic(parent as HWND) } != 0, class_of(fg), std::process::id())
}

/// `a` est au-dessus de `b` dans l'ordre Z.
pub fn above(a: isize, b: isize) -> bool {
    let mut w = b as HWND;
    loop {
        w = unsafe { GetWindow(w, GW_HWNDPREV) };
        if w.is_null() {
            return false;
        }
        if w as isize == a {
            return true;
        }
    }
}

/// Fenetres de premier niveau de classe `class` du processus `pid`.
pub fn windows_of(pid: u32, class: &str) -> Vec<isize> {
    struct Ctx<'a> {
        pid: u32,
        class: &'a str,
        out: Vec<isize>,
    }
    unsafe extern "system" fn cb(h: HWND, lp: LPARAM) -> i32 {
        let ctx = &mut *(lp as *mut Ctx);
        let mut pid = 0u32;
        GetWindowThreadProcessId(h, &mut pid);
        if pid == ctx.pid && class_of(h as isize) == ctx.class {
            ctx.out.push(h as isize);
        }
        1
    }
    let mut ctx = Ctx { pid, class, out: Vec::new() };
    unsafe { EnumWindows(Some(cb), &mut ctx as *mut Ctx as LPARAM) };
    ctx.out
}

/// Demande au faux jeu d'occuper de lui-meme tout l'ecran avec sa fenetre `win` (message applicatif `WM_APP + 1`, pas une entree),
/// et attend qu'il l'ait fait : c'est le jeu qui deplace sa fenetre, comme un passage en plein ecran.
#[track_caller]
pub fn game_grabs_screen(win: isize) {
    ask_game(win, WM_APP + 1);
}

/// Demande au faux jeu de remettre lui-meme son cadre a `win` et de l'agrandir autour de sa zone cliente (`WM_APP + 2`), comme
/// GLFW quand le jeu change ses attributs de fenetre ; attend qu'il l'ait fait.
#[track_caller]
pub fn game_reframes(win: isize) {
    ask_game(win, WM_APP + 2);
}

#[track_caller]
fn ask_game(win: isize, msg: u32) {
    let before = unsafe { GetWindowLongPtrW(win as HWND, GWLP_USERDATA) };
    assert!(unsafe { PostMessageW(win as HWND, msg, 0, 0) } != 0, "message au faux jeu");
    wait_until("le faux jeu a obei", Duration::from_secs(5), || unsafe { GetWindowLongPtrW(win as HWND, GWLP_USERDATA) } > before);
}

/// Fenetres de premier niveau VISIBLES du processus `pid`, toutes classes : ce que le joueur (et la barre des taches) peut voir.
pub fn visible_windows_of(pid: u32) -> Vec<isize> {
    struct Ctx {
        pid: u32,
        out: Vec<isize>,
    }
    unsafe extern "system" fn cb(h: HWND, lp: LPARAM) -> i32 {
        let ctx = &mut *(lp as *mut Ctx);
        let mut pid = 0u32;
        GetWindowThreadProcessId(h, &mut pid);
        if pid == ctx.pid && IsWindowVisible(h) != 0 {
            ctx.out.push(h as isize);
        }
        1
    }
    let mut ctx = Ctx { pid, out: Vec::new() };
    unsafe { EnumWindows(Some(cb), &mut ctx as *mut Ctx as LPARAM) };
    ctx.out
}

/// Fenetres « message seulement » (parent `HWND_MESSAGE`) de classe `class` du processus `pid`. `EnumWindows` ne les voit pas.
pub fn message_windows_of(pid: u32, class: &str) -> Vec<isize> {
    let cls = wide(class);
    let (mut out, mut prev) = (Vec::new(), null_mut());
    loop {
        prev = unsafe { FindWindowExW(HWND_MESSAGE, prev, cls.as_ptr(), null()) };
        if prev.is_null() {
            return out;
        }
        let mut owner_pid = 0u32;
        unsafe { GetWindowThreadProcessId(prev, &mut owner_pid) };
        if owner_pid == pid {
            out.push(prev as isize);
        }
    }
}

// ------------------------------------------------------------------------------------------------ lectures Win32

pub fn class_of(h: isize) -> String {
    let mut buf = [0u16; 128];
    let n = unsafe { GetClassNameW(h as HWND, buf.as_mut_ptr(), 128) };
    String::from_utf16_lossy(&buf[..n.max(0) as usize])
}

/// Rectangle ecran (x, y, largeur, hauteur).
pub fn rect(h: isize) -> (i32, i32, i32, i32) {
    unsafe {
        let mut r: RECT = std::mem::zeroed();
        GetWindowRect(h as HWND, &mut r);
        (r.left, r.top, r.right - r.left, r.bottom - r.top)
    }
}

pub fn style(h: isize) -> u32 {
    unsafe { GetWindowLongPtrW(h as HWND, GWL_STYLE) as u32 }
}

pub fn exstyle(h: isize) -> u32 {
    unsafe { GetWindowLongPtrW(h as HWND, GWL_EXSTYLE) as u32 }
}

pub fn owner(h: isize) -> isize {
    unsafe { GetWindow(h as HWND, GW_OWNER) as isize }
}

pub fn visible(h: isize) -> bool {
    unsafe { IsWindowVisible(h as HWND) != 0 }
}

pub fn exists(h: isize) -> bool {
    unsafe { IsWindow(h as HWND) != 0 }
}

/// La fenetre a un cadre complet (barre de titre).
pub fn framed(h: isize) -> bool {
    style(h) & WS_CAPTION == WS_CAPTION
}

/// La fenetre touche un ecran (une fenetre garee en (-32000, -32000) n'en touche aucun).
pub fn on_screen(h: isize) -> bool {
    unsafe { !MonitorFromWindow(h as HWND, MONITOR_DEFAULTTONULL).is_null() }
}

/// La bande du panneau « en jeu » est decoupee : la region de la fenetre est plus etroite qu'elle. (Une fenetre posee porte
/// toujours une region, pleine sans panneau.)
pub fn panel_cut(h: isize) -> bool {
    unsafe {
        let probe = CreateRectRgn(0, 0, 0, 0);
        let kind = GetWindowRgn(h as HWND, probe);
        let mut b: RECT = std::mem::zeroed();
        GetRgnBox(probe, &mut b);
        DeleteObject(probe as _);
        let (_, _, w, _) = rect(h);
        kind != 0 && b.right - b.left < w
    }
}

/// La fenetre porte une region, quelle qu'elle soit.
pub fn has_region(h: isize) -> bool {
    unsafe {
        let probe = CreateRectRgn(0, 0, 0, 0);
        let kind = GetWindowRgn(h as HWND, probe);
        DeleteObject(probe as _);
        kind != 0 // 0 = ERROR : aucune region
    }
}

/// La fenetre est telle qu'un joueur la retrouve apres un repli : visible, a l'ecran, avec son cadre, sans proprietaire, sans
/// decoupe, avec un bouton dans la barre des taches (ni fenetre-outil ni possedee).
#[track_caller]
pub fn assert_given_back(h: isize) {
    assert!(exists(h), "la fenetre rendue n'existe plus");
    assert!(visible(h), "fenetre rendue invisible");
    assert!(on_screen(h), "fenetre rendue hors ecran : {:?}", rect(h));
    assert!(framed(h), "fenetre rendue sans cadre : style {:#x}", style(h));
    assert_eq!(owner(h), 0, "fenetre rendue encore possedee");
    assert!(!has_region(h), "fenetre rendue encore decoupee");
    assert_eq!(exstyle(h) & WS_EX_TOOLWINDOW, 0, "fenetre rendue absente de la barre des taches");
}

#[path = "../shared/events.rs"]
mod events;
#[allow(unused_imports)] // chaque scenario n'utilise qu'une partie des outils
pub use events::Events;

// ------------------------------------------------------------------------------------------------ echantillonnage

#[path = "../shared/sampler.rs"]
mod sampler;
#[allow(unused_imports)]
pub use sampler::Sampler;

/// Echantillonne en continu (toutes les ~0,5 ms, sur un fil a part) les fenetres de classe `class` du processus `pid` et retient
/// tout instant ou l'une d'elles est a la fois VISIBLE, A L'ECRAN et AVEC SON CADRE : c'est la fenetre de jeu autonome que le joueur
/// ne doit jamais apercevoir tant que le jeu est dans la Scene.
///
/// `strict` (lancement cache) : la fenetre ne doit se voir QUE posee, sans cadre, exactement sur `zone` ; tout autre instant ou
/// elle est visible a l'ecran est retenu.
impl Sampler {
    pub fn start(pid: u32, class: &str) -> Sampler {
        Self::run(pid, class, None)
    }

    pub fn strict(pid: u32, class: &str, zone: (i32, i32, i32, i32)) -> Sampler {
        Self::run(pid, class, Some(zone))
    }

    fn run(pid: u32, class: &str, zone: Option<(i32, i32, i32, i32)>) -> Sampler {
        let class = class.to_string();
        Sampler::spawn(move || {
            move |t0: Instant, seen: &mut Vec<String>| {
                for h in windows_of(pid, &class) {
                    // lectures non atomiques : un etat fautif n'est retenu que s'il se relit a l'identique
                    let bad = |h: isize| visible(h) && on_screen(h) && (framed(h) || zone.is_some_and(|z| rect(h) != z));
                    if bad(h) && bad(h) {
                        seen.push(format!("+{} ms : fenetre {h:#x} visible en {:?}, style {:#x}", t0.elapsed().as_millis(), rect(h), style(h)));
                    }
                }
            }
        })
    }
}

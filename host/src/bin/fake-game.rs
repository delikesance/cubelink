//! Faux jeu des tests d'integration de la Scene (`host/tests`) : ce que l'hote voit d'un vrai jeu, sans Java ni carte graphique.
//!
//!   `fake-game <classe> <delai_ms> [disown] [helpers]`
//!                                             : cree (cachee) une fenetre de premier niveau de la classe demandee (`GLFW30`,
//!                                               `SDL_app`, `LWJGL`), l'affiche apres `delai_ms`, pompe ses messages, se ferme
//!                                               proprement sur `WM_CLOSE`. `disown` : defait tout rattachement (le proprietaire
//!                                               est retire a chaque changement de position ou de cadre), pour le repli
//!                                               « adoption refusee ». `helpers` : comme SDL 3, cree APRES l'affichage de la
//!                                               fenetre principale deux fenetres d'aide « message seulement » (parent
//!                                               `HWND_MESSAGE`), `SDLHelperWindowInputCatcher` et `SDL_HIDAPI_DEVICE_DETECTION`,
//!                                               plus une vraie fenetre secondaire cachee (`SDL_aux`) qui refuse tout proprietaire.
//!                                               Sur le message `WM_APP + 1` (poste par le test), la fenetre principale
//!                                               occupe d'elle-meme tout l'ecran, comme un jeu qui passe en plein ecran, et
//!                                               compte la demande dans `GWLP_USERDATA`.
//!                                               `glfwhelper` : comme GLFW, cree AVANT la fenetre principale ses fenetres
//!                                               d'aide cachees, de premier niveau : `GLFW3 Helper` (GLFW 3.4) et, si la classe
//!                                               demandee est `GLFW30`, une `GLFW30` titree `GLFW message window` (GLFW 3.3).
//!                                               `stall=<ms>` : comme un jeu qui charge ses mods, ne pompe aucun message entre la creation
//!                                               de sa fenetre et `ms` apres l'avoir AFFICHEE (au premier plan de l'ordre Z).
//!                                               `reframe=<ms>` : `ms` apres la creation, remet son cadre a sa fenetre et
//!                                               l'agrandit autour de sa zone cliente, comme GLFW (`updateWindowStyles`) ; idem
//!                                               sur le message `WM_APP + 2`. `late=<ms>` : ne cree sa fenetre qu'apres `ms`.
//!                                               Premier affichage : comme GLFW 3.4, la commande de `STARTUPINFO`
//!                                               (`STARTF_USESHOWWINDOW`) remplace `SW_SHOWNA` : lance avec `SW_HIDE`, le faux
//!                                               jeu n'affiche donc JAMAIS sa fenetre lui-meme. `nostartup` : ignore la consigne
//!                                               (GLFW 3.3, SDL, LWJGL 2).
//!   `fake-game none`                          : aucune fenetre, le processus vit (repli « aucune fenetre »).
//!
//! Dans tous les cas le processus s'arrete seul au bout de deux minutes : un test interrompu ne laisse rien trainer.

/// Duree de vie maximale.
#[cfg(windows)]
const LIFETIME_MS: u32 = 120_000;

#[cfg(windows)]
mod imp {
    use std::ptr::{null, null_mut};
    use std::sync::atomic::{AtomicBool, Ordering};

    use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
    use windows_sys::Win32::Graphics::Gdi::{ClientToScreen, GetStockObject, DKGRAY_BRUSH};
    use windows_sys::Win32::System::Threading::{GetStartupInfoW, STARTF_USESHOWWINDOW, STARTUPINFOW};
    use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
    use windows_sys::Win32::UI::HiDpi::{SetProcessDpiAwarenessContext, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2};
    use windows_sys::Win32::UI::WindowsAndMessaging::*;

    use super::LIFETIME_MS;

    const TIMER_SHOW: usize = 1;
    const TIMER_LIFETIME: usize = 2;
    const TIMER_HELPERS: usize = 3;
    /// « Passe en plein ecran » (voir l'en-tete).
    const WM_APP_GRAB: u32 = WM_APP + 1;
    /// Delai entre l'affichage de la fenetre principale et la creation des fenetres d'aide.
    const HELPERS_AFTER_MS: u32 = 200;

    static DISOWN: AtomicBool = AtomicBool::new(false);
    static HELPERS: AtomicBool = AtomicBool::new(false);
    static GLFW_HELPER: AtomicBool = AtomicBool::new(false);
    static STALL_MS: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);

    static NO_STARTUP: AtomicBool = AtomicBool::new(false);
    static REFRAME_MS: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
    static LATE_MS: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
    const TIMER_REFRAME: usize = 4;
    /// « Remets ton cadre » (voir l'en-tete).
    const WM_APP_REFRAME: u32 = WM_APP + 2;

    /// Commande du premier affichage : celle du lanceur (`STARTUPINFO`) si elle est donnee, comme GLFW 3.4.
    unsafe fn show_cmd() -> i32 {
        let mut si: STARTUPINFOW = std::mem::zeroed();
        si.cb = std::mem::size_of::<STARTUPINFOW>() as u32;
        GetStartupInfoW(&mut si);
        if !NO_STARTUP.load(Ordering::Relaxed) && si.dwFlags & STARTF_USESHOWWINDOW != 0 {
            si.wShowWindow as i32
        } else {
            SW_SHOWNA
        }
    }

    /// Comme `updateWindowStyles` de GLFW : cadre complet, rectangle agrandi autour de la zone cliente.
    unsafe fn reframe(hwnd: HWND) {
        let mut rc: RECT = std::mem::zeroed();
        GetClientRect(hwnd, &mut rc);
        let style = (GetWindowLongPtrW(hwnd, GWL_STYLE) as u32 & !WS_POPUP) | WS_OVERLAPPEDWINDOW;
        let mut tl = POINT { x: rc.left, y: rc.top };
        ClientToScreen(hwnd, &mut tl);
        let mut full = RECT { left: tl.x, top: tl.y, right: tl.x + rc.right, bottom: tl.y + rc.bottom };
        AdjustWindowRectEx(&mut full, style, 0, GetWindowLongPtrW(hwnd, GWL_EXSTYLE) as u32);
        SetWindowLongPtrW(hwnd, GWL_STYLE, style as isize);
        SetWindowPos(hwnd, null_mut(), full.left, full.top, full.right - full.left, full.bottom - full.top, SWP_FRAMECHANGED | SWP_NOZORDER | SWP_NOACTIVATE);
    }

    /// Options par mot-cle (`glfwhelper`, `stall=<ms>`, `reframe=<ms>`, `late=<ms>`, `nostartup`).
    pub fn options(args: &[String]) {
        let num = |key: &str| args.iter().find_map(|a| a.strip_prefix(key)).and_then(|v| v.parse().ok()).unwrap_or(0);
        NO_STARTUP.store(args.iter().any(|a| a == "nostartup"), Ordering::Relaxed);
        REFRAME_MS.store(num("reframe="), Ordering::Relaxed);
        LATE_MS.store(num("late="), Ordering::Relaxed);
        GLFW_HELPER.store(args.iter().any(|a| a == "glfwhelper"), Ordering::Relaxed);
        let stall = args.iter().find_map(|a| a.strip_prefix("stall=")).and_then(|v| v.parse().ok()).unwrap_or(0);
        STALL_MS.store(stall, Ordering::Relaxed);
    }

    /// Fenetre d'aide de GLFW, creee comme lui : premier niveau, cachee, 1 x 1 demande.
    unsafe fn create_glfw_helper(class: &str, register: bool) {
        let hinst = GetModuleHandleW(null());
        let cls = wide(class);
        if register {
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
        }
        let title = wide("GLFW message window");
        CreateWindowExW(WS_EX_OVERLAPPEDWINDOW, cls.as_ptr(), title.as_ptr(), WS_CLIPSIBLINGS | WS_CLIPCHILDREN, 0, 0, 1, 1, null_mut(), null_mut(), hinst, null());
    }

    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(std::iter::once(0)).collect()
    }

    /// Fenetre secondaire : defait tout rattachement, ne s'affiche jamais.
    unsafe extern "system" fn aux_proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
        if msg == WM_WINDOWPOSCHANGING {
            SetWindowLongPtrW(hwnd, GWLP_HWNDPARENT, 0);
        }
        DefWindowProcW(hwnd, msg, wp, lp)
    }

    /// Les fenetres d'aide de SDL 3, creees comme lui : « message seulement », jamais affichees, sans proprietaire possible. Et
    /// une fenetre secondaire de premier niveau, cachee, d'une classe de jeu, qui refuse son rattachement.
    unsafe fn create_helpers() {
        let hinst = GetModuleHandleW(null());
        let helpers: [(&str, u32, i32, HWND, WNDPROC); 3] = [
            ("SDLHelperWindowInputCatcher", WS_OVERLAPPED, CW_USEDEFAULT, HWND_MESSAGE, Some(DefWindowProcW)),
            ("SDL_HIDAPI_DEVICE_DETECTION", 0, 0, HWND_MESSAGE, Some(DefWindowProcW)),
            ("SDL_aux", WS_OVERLAPPEDWINDOW, 300, null_mut(), Some(aux_proc)),
        ];
        for (class, style, pos, parent, wndproc) in helpers {
            let cls = wide(class);
            let wc = WNDCLASSW {
                style: 0,
                lpfnWndProc: wndproc,
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
            CreateWindowExW(0, cls.as_ptr(), cls.as_ptr(), style, pos, pos, pos, pos, parent, null_mut(), hinst, null());
        }
    }

    unsafe extern "system" fn proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
        match msg {
            WM_TIMER if wp == TIMER_SHOW => {
                KillTimer(hwnd, TIMER_SHOW);
                ShowWindow(hwnd, show_cmd());
                if HELPERS.load(Ordering::Relaxed) {
                    SetTimer(hwnd, TIMER_HELPERS, HELPERS_AFTER_MS, None);
                }
                0
            }
            WM_TIMER if wp == TIMER_HELPERS => {
                KillTimer(hwnd, TIMER_HELPERS);
                create_helpers();
                0
            }
            WM_TIMER if wp == TIMER_LIFETIME => {
                DestroyWindow(hwnd);
                0
            }
            WM_WINDOWPOSCHANGING if DISOWN.load(Ordering::Relaxed) => {
                SetWindowLongPtrW(hwnd, GWLP_HWNDPARENT, 0);
                DefWindowProcW(hwnd, msg, wp, lp)
            }
            WM_TIMER if wp == TIMER_REFRAME => {
                KillTimer(hwnd, TIMER_REFRAME);
                reframe(hwnd);
                0
            }
            WM_APP_REFRAME => {
                reframe(hwnd);
                SetWindowLongPtrW(hwnd, GWLP_USERDATA, GetWindowLongPtrW(hwnd, GWLP_USERDATA) + 1);
                0
            }
            WM_APP_GRAB => {
                SetWindowPos(hwnd, null_mut(), 0, 0, GetSystemMetrics(SM_CXSCREEN), GetSystemMetrics(SM_CYSCREEN), SWP_NOZORDER | SWP_NOACTIVATE);
                SetWindowLongPtrW(hwnd, GWLP_USERDATA, GetWindowLongPtrW(hwnd, GWLP_USERDATA) + 1);
                0
            }
            WM_CLOSE => {
                DestroyWindow(hwnd);
                0
            }
            WM_DESTROY => {
                PostQuitMessage(0);
                0
            }
            _ => DefWindowProcW(hwnd, msg, wp, lp),
        }
    }

    pub fn run(class: &str, show_ms: u32, disown: bool, helpers: bool) {
        DISOWN.store(disown, Ordering::Relaxed);
        HELPERS.store(helpers, Ordering::Relaxed);
        unsafe {
            // comme Java : tailles et positions en pixels physiques
            SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
            let hinst = GetModuleHandleW(null());
            let cls: Vec<u16> = class.encode_utf16().chain(std::iter::once(0)).collect();
            let title: Vec<u16> = "Faux jeu Cubelink".encode_utf16().chain(std::iter::once(0)).collect();
            let wc = WNDCLASSW {
                style: 0,
                lpfnWndProc: Some(proc),
                cbClsExtra: 0,
                cbWndExtra: 0,
                hInstance: hinst,
                hIcon: null_mut(),
                hCursor: LoadCursorW(null_mut(), IDC_ARROW),
                hbrBackground: GetStockObject(DKGRAY_BRUSH) as _,
                lpszMenuName: null(),
                lpszClassName: cls.as_ptr(),
            };
            RegisterClassW(&wc);
            if GLFW_HELPER.load(Ordering::Relaxed) {
                create_glfw_helper("GLFW3 Helper", true);
                if class == "GLFW30" {
                    create_glfw_helper("GLFW30", false);
                }
            }
            let late = LATE_MS.load(Ordering::Relaxed);
            if late > 0 {
                std::thread::sleep(std::time::Duration::from_millis(late as u64));
            }
            // creee CACHEE puis affichee plus tard, comme un jeu qui prepare son contexte graphique avant de se montrer
            let hwnd = CreateWindowExW(0, cls.as_ptr(), title.as_ptr(), WS_OVERLAPPEDWINDOW, 200, 150, 870, 520, null_mut(), null_mut(), hinst, null());
            if hwnd.is_null() {
                eprintln!("fake-game : CreateWindowExW a echoue");
                std::process::exit(2);
            }
            let stall = STALL_MS.load(Ordering::Relaxed);
            if stall > 0 {
                // Aucun message traite entre la creation et l'affichage, ni apres : la fenetre est affichee par le jeu, passe devant
                // tout APRES coup (comme `glfwFocusWindow`), puis le fil se tait.
                let nap = |ms: u32| std::thread::sleep(std::time::Duration::from_millis(ms as u64));
                nap(show_ms);
                ShowWindow(hwnd, show_cmd());
                nap(300);
                // Un processus d'arriere-plan (ce faux jeu, sous `cargo test`) n'obtient pas le haut de la pile par
                // `BringWindowToTop` ; l'aller-retour par « toujours au-dessus » l'y met vraiment, comme un jeu au premier plan.
                let keep = SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE;
                SetWindowPos(hwnd, HWND_TOPMOST, 0, 0, 0, 0, keep);
                SetWindowPos(hwnd, HWND_NOTOPMOST, 0, 0, 0, 0, keep);
                nap(stall);
            } else {
                SetTimer(hwnd, TIMER_SHOW, show_ms.max(1), None);
                let reframe_ms = REFRAME_MS.load(Ordering::Relaxed);
                if reframe_ms > 0 {
                    SetTimer(hwnd, TIMER_REFRAME, reframe_ms, None);
                }
            }
            SetTimer(hwnd, TIMER_LIFETIME, LIFETIME_MS, None);
            let mut msg: MSG = std::mem::zeroed();
            while GetMessageW(&mut msg, null_mut(), 0, 0) > 0 {
                TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
        }
    }
}

#[cfg(windows)]
fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("none") => std::thread::sleep(std::time::Duration::from_millis(LIFETIME_MS as u64)),
        Some(class) => {
            let show_ms = args.get(1).and_then(|a| a.parse().ok()).unwrap_or(300);
            imp::options(&args);
            imp::run(class, show_ms, args.iter().any(|a| a == "disown"), args.iter().any(|a| a == "helpers"));
        }
        None => {
            eprintln!("usage : fake-game <classe> <delai_ms> [disown] [helpers] [glfwhelper] [stall=<ms>] [reframe=<ms>] [late=<ms>] [nostartup] | fake-game none");
            std::process::exit(2);
        }
    }
}

#[cfg(not(windows))]
fn main() {
    eprintln!("fake-game : faux jeu des tests Win32 de la Scene, sans objet sur cette plateforme");
}

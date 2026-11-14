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
//!
//! Variante LINUX (X11, `x11rb`, pour `tests/x11_*.rs`) : meme contrat d'arguments, sans Win32. Fenetre 870 x 520 en (200, 150),
//! `WM_CLASS` = la classe demandee, `_NET_WM_PID`, `WM_DELETE_WINDOW` (le processus s'arrete dessus). Options reprises : `disown`
//! (quand son parent n'est plus la racine, se reparente sur la racine), `glfwhelper` (cree AVANT sa fenetre deux fenetres d'aide
//! minuscules, jamais affichees : `GLFW3 Helper` et `SDLHelperWindowInputCatcher`), `stall=<ms>`, `late=<ms>`. Message client
//! `_FAKE_GAME_GRAB` : la fenetre prend d'elle-meme la taille de l'ecran (plein ecran), `_FAKE_GAME_UNGRAB` : elle reprend sa
//! taille ; chacun incremente la propriete `_FAKE_GAME_ACKS` de la fenetre (le test sait que le jeu a obei). Pas de gestionnaire de
//! fenetres : rien n'y est emule (cadre, barre des taches).
//! `syncwait` (Linux) : comme SDL 3 (`SDL_SyncWindow`), toutes les 700 ms apres l'affichage il demande un deplacement ET un
//! redimensionnement de sa fenetre puis ATTEND un `ConfigureNotify` SYNTHETIQUE (ICCCM 4.1.5 : `send_event`, coordonnees absolues en
//! racine ; `syncmax=<n>` : s'arrete apres `n` demandes) ; sans reponse en 2 s c'est un echec (SDL : « Failed to synchronize SDL window »). Compteurs : proprietes
//! `_FAKE_GAME_SYNC_OK` / `_FAKE_GAME_SYNC_FAIL` de la fenetre ; `_FAKE_GAME_CN` = dernier `ConfigureNotify` synthetique recu
//! `[x, y, largeur, hauteur, nombre]`, qu'il ait ete demande ou non (le launcher deplace sans que le jeu ait rien demande).

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

#[cfg(target_os = "linux")]
mod imp_x11 {
    use std::time::{Duration, Instant};

    use x11rb::connection::Connection;
    use x11rb::protocol::xproto::*;
    use x11rb::protocol::Event;
    use x11rb::rust_connection::RustConnection;
    use x11rb::wrapper::ConnectionExt as _;

    const LIFETIME: Duration = Duration::from_secs(120);

    fn atom(c: &RustConnection, name: &str) -> u32 {
        c.intern_atom(false, name.as_bytes()).expect("intern_atom").reply().expect("atome").atom
    }

    /// Cree une fenetre de premier niveau (cachee) de la classe `class`.
    fn create(c: &RustConnection, root: u32, class: &str, rect: (i16, i16, u16, u16)) -> u32 {
        let win = c.generate_id().expect("id");
        let screen = &c.setup().roots[0];
        let aux = CreateWindowAux::new().background_pixel(screen.white_pixel).event_mask(EventMask::STRUCTURE_NOTIFY);
        c.create_window(0, win, root, rect.0, rect.1, rect.2, rect.3, 0, WindowClass::INPUT_OUTPUT, 0, &aux).expect("create_window");
        let cls = format!("{class}\0{class}\0");
        c.change_property8(PropMode::REPLACE, win, AtomEnum::WM_CLASS, AtomEnum::STRING, cls.as_bytes()).unwrap();
        c.change_property8(PropMode::REPLACE, win, AtomEnum::WM_NAME, AtomEnum::STRING, class.as_bytes()).unwrap();
        c.change_property32(PropMode::REPLACE, win, atom(c, "_NET_WM_PID"), AtomEnum::CARDINAL, &[std::process::id()]).unwrap();
        c.change_property32(PropMode::REPLACE, win, atom(c, "WM_PROTOCOLS"), AtomEnum::ATOM, &[atom(c, "WM_DELETE_WINDOW")]).unwrap();
        win
    }

    pub fn run(args: &[String]) {
        let class = args[0].as_str();
        let show_ms: u64 = args.get(1).and_then(|a| a.parse().ok()).unwrap_or(300);
        let opt = |k: &str| args.iter().find_map(|a| a.strip_prefix(k)).and_then(|v| v.parse::<u64>().ok()).unwrap_or(0);
        let (disown, helpers) = (args.iter().any(|a| a == "disown"), args.iter().any(|a| a == "glfwhelper"));
        let (stall, late) = (opt("stall="), opt("late="));
        let syncwait = args.iter().any(|a| a == "syncwait");
        let (mut sync_ok, mut sync_fail, mut cn_count) = (0u32, 0u32, 0u32);
        let mut pending: Option<Instant> = None;
        let mut idle_since = Instant::now();
        let (syncmax, mut asked) = (opt("syncmax=") as u32, 0u32);
        let mut toggle = false;
        let (c, n) = RustConnection::connect(None).expect("connexion X11 (DISPLAY)");
        let root = c.setup().roots[n].root;
        let (p_ok, p_fail, p_cn) = (atom(&c, "_FAKE_GAME_SYNC_OK"), atom(&c, "_FAKE_GAME_SYNC_FAIL"), atom(&c, "_FAKE_GAME_CN"));
        let (grab, ungrab, acks, delete) = (atom(&c, "_FAKE_GAME_GRAB"), atom(&c, "_FAKE_GAME_UNGRAB"), atom(&c, "_FAKE_GAME_ACKS"), atom(&c, "WM_DELETE_WINDOW"));
        // comme GLFW / SDL : `_NET_WM_STATE` (data[0] : 0 retire, 1 ajoute, 2 bascule ; data[1] : plein ecran) envoye a la RACINE
        let (ewmh, net_state, net_fs) = ([atom(&c, "_FAKE_GAME_EWMH_REMOVE"), atom(&c, "_FAKE_GAME_EWMH_ADD"), atom(&c, "_FAKE_GAME_EWMH_TOGGLE")], atom(&c, "_NET_WM_STATE"), atom(&c, "_NET_WM_STATE_FULLSCREEN"));
        let screen_size = (c.setup().roots[n].width_in_pixels, c.setup().roots[n].height_in_pixels);
        let t0 = Instant::now();
        let mut win = None;
        let mut mapped = false;
        let mut n_acks = 0u32;
        let mut helper_made = false;
        loop {
            if t0.elapsed() > LIFETIME {
                return;
            }
            if helpers && !helper_made {
                create(&c, root, "GLFW3 Helper", (0, 0, 1, 1));
                create(&c, root, "SDLHelperWindowInputCatcher", (0, 0, 10, 10));
                helper_made = true;
            }
            if win.is_none() && t0.elapsed() >= Duration::from_millis(late) {
                win = Some(create(&c, root, class, (200, 150, 870, 520)));
                c.flush().unwrap();
            }
            if let Some(w) = win {
                if !mapped && t0.elapsed() >= Duration::from_millis(late + show_ms) {
                    c.map_window(w).unwrap();
                    c.flush().unwrap();
                    mapped = true;
                    if stall > 0 {
                        std::thread::sleep(Duration::from_millis(stall)); // ne pompe plus rien : comme un jeu qui charge ses mods
                    }
                }
            }
            if let (true, Some(w), true) = (syncwait, win, mapped) {
                if pending.is_none() && (syncmax == 0 || asked < syncmax) && idle_since.elapsed() >= Duration::from_millis(700) {
                    toggle = !toggle;
                    asked += 1;
                    let (x, y, cw, ch) = if toggle { (10, 12, 800, 500) } else { (14, 8, 810, 490) };
                    c.configure_window(w, &ConfigureWindowAux::new().x(x).y(y).width(cw).height(ch)).unwrap();
                    c.flush().unwrap();
                    pending = Some(Instant::now());
                } else if pending.is_some_and(|t| t.elapsed() > Duration::from_secs(2)) {
                    sync_fail += 1;
                    c.change_property32(PropMode::REPLACE, w, p_fail, AtomEnum::CARDINAL, &[sync_fail]).unwrap();
                    c.flush().unwrap();
                    pending = None;
                    idle_since = Instant::now();
                }
            }
            while let Ok(Some(ev)) = c.poll_for_event() {
                match ev {
                    Event::ConfigureNotify(e) if syncwait && Some(e.window) == win && e.response_type & 0x80 != 0 => {
                        cn_count += 1;
                        c.change_property32(PropMode::REPLACE, e.window, p_cn, AtomEnum::CARDINAL, &[e.x as i32 as u32, e.y as i32 as u32, e.width as u32, e.height as u32, cn_count]).unwrap();
                        if pending.take().is_some() {
                            sync_ok += 1;
                            c.change_property32(PropMode::REPLACE, e.window, p_ok, AtomEnum::CARDINAL, &[sync_ok]).unwrap();
                            idle_since = Instant::now();
                        }
                        c.flush().unwrap();
                    }
                    Event::ClientMessage(m) if m.type_ == grab || m.type_ == ungrab => {
                        let Some(w) = win else { continue };
                        let rect: (i32, i32, u32, u32) = if m.type_ == grab { (0, 0, screen_size.0 as u32, screen_size.1 as u32) } else { (200, 150, 870, 520) };
                        let aux = ConfigureWindowAux::new().x(rect.0).y(rect.1).width(rect.2).height(rect.3);
                        c.configure_window(w, &aux).unwrap();
                        n_acks += 1;
                        c.change_property32(PropMode::REPLACE, w, acks, AtomEnum::CARDINAL, &[n_acks]).unwrap();
                        c.flush().unwrap();
                    }
                    Event::ClientMessage(m) if ewmh.contains(&m.type_) => {
                        let Some(w) = win else { continue };
                        let action = ewmh.iter().position(|a| *a == m.type_).unwrap_or(0) as u32;
                        let msg = ClientMessageEvent::new(32, w, net_state, [action, net_fs, 0, 1, 0]);
                        c.send_event(false, root, EventMask::SUBSTRUCTURE_NOTIFY | EventMask::SUBSTRUCTURE_REDIRECT, msg).unwrap();
                        n_acks += 1;
                        c.change_property32(PropMode::REPLACE, w, acks, AtomEnum::CARDINAL, &[n_acks]).unwrap();
                        c.flush().unwrap();
                    }
                    Event::ClientMessage(m) if m.format == 32 && m.data.as_data32()[0] == delete => return,
                    Event::ReparentNotify(e) if disown && Some(e.window) == win && e.parent != root => {
                        c.reparent_window(e.window, root, 200, 150).unwrap();
                        c.flush().unwrap();
                    }
                    _ => {}
                }
            }
            std::thread::sleep(Duration::from_millis(2));
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

#[cfg(target_os = "linux")]
fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("none") => std::thread::sleep(std::time::Duration::from_secs(120)),
        Some(_) => imp_x11::run(&args),
        None => {
            eprintln!("usage : fake-game <classe> <delai_ms> [disown] [glfwhelper] [stall=<ms>] [late=<ms>] | fake-game none");
            std::process::exit(2);
        }
    }
}

#[cfg(not(any(windows, target_os = "linux")))]
fn main() {
    eprintln!("fake-game : faux jeu des tests de la Scene, sans objet sur cette plateforme");
}

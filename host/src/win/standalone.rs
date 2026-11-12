//! Mode autonome (binaire `host-proto`) : fenetre hote propre, a cadre et barre de titre dessines par nous ; le jeu y est une
//! fenetre FILLE (`SetParent`), le rideau une fenetre fille a transparence par pixel au-dessus de lui.

use std::cell::Cell;
use std::ptr::{null, null_mut};
use std::sync::atomic::Ordering;
use std::sync::mpsc;
use std::time::Duration;

use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows_sys::Win32::Graphics::Gdi::*;
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::UI::HiDpi::{GetDpiForWindow, GetSystemMetricsForDpi, SetProcessDpiAwarenessContext};
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{SetFocus, TrackMouseEvent, TME_LEAVE, TRACKMOUSEEVENT};
use windows_sys::Win32::UI::WindowsAndMessaging::*;

use super::curtain_window::{self, CURTAIN, TIMER_CURTAIN};
use super::game_window::STRIP;
use super::util::{bitmap_info, dpi_scale, hi, hw, lo, paint_background, register_class, wide};
use super::{on_reveal, shared, HostConfig, Shared, WM_APP_CURTAIN_ON, WM_APP_REVEAL, WM_APP_SYNC};
use crate::curtain;

const WM_MOUSELEAVE: u32 = 0x02A3;
/// Barre de titre (comme `--titlebar-h` du launcher) et bord de la fenetre, en pixels a 96 dpi.
const TITLEBAR_H: f32 = 36.0;
const BORDER: f32 = 5.0;
static ICON_256: &[u8] = include_bytes!("../../assets/icon-256.png");
static ICON_32: &[u8] = include_bytes!("../../assets/icon-32.png");

thread_local! {
    /// Bouton de la barre de titre survole (-1 : aucun).
    static TB_HOVER: Cell<i8> = const { Cell::new(-1) };
}

/// Cree la fenetre hote sur un fil dedie (avec sa boucle de messages) et attend qu'elle existe.
pub(super) fn start(cfg: HostConfig) -> Result<(), String> {
    let (tx, rx) = mpsc::channel::<Result<(), String>>();
    std::thread::Builder::new().name("cubelink-host-ui".into()).spawn(move || ui_thread(cfg, tx)).map_err(|e| e.to_string())?;
    rx.recv_timeout(Duration::from_secs(10)).map_err(|_| "fenetre hote : delai depasse".to_string())?
}

fn ui_thread(cfg: HostConfig, ready: mpsc::Sender<Result<(), String>>) {
    unsafe {
        let r = match shared() {
            Some(s) => create(s, &cfg),
            None => Err("hote non demarre".to_string()),
        };
        let failed = r.is_err();
        let _ = ready.send(r);
        if failed {
            return;
        }
        let mut msg: MSG = std::mem::zeroed();
        while GetMessageW(&mut msg, null_mut(), 0, 0) > 0 {
            TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
}

unsafe fn create(s: &Shared, cfg: &HostConfig) -> Result<(), String> {
    SetProcessDpiAwarenessContext(-4isize as _); // PER_MONITOR_AWARE_V2
    let hinst = GetModuleHandleW(null());
    register_class("CubelinkHost", Some(host_proc), GetStockObject(BLACK_BRUSH) as isize);
    register_class("CubelinkCurtain", Some(curtain_proc), 0);
    register_class("CubelinkTitlebar", Some(titlebar_proc), 0);
    let cls = wide("CubelinkHost");
    let title = wide(&cfg.title);
    let host = CreateWindowExW(
        0, cls.as_ptr(), title.as_ptr(), WS_OVERLAPPEDWINDOW | WS_CLIPCHILDREN | WS_VISIBLE,
        100, 60, cfg.width, cfg.height, null_mut(), null_mut(), hinst, null(),
    );
    if host.is_null() {
        return Err("CreateWindowExW (hote) a echoue".into());
    }
    s.host.store(host as isize, Ordering::SeqCst);
    // icone de l'application (barre des taches, Alt+Tab)
    for (bytes, px, kind) in [(ICON_256, 256, 1usize /* ICON_BIG */), (ICON_32, 32, 0usize /* ICON_SMALL */)] {
        let ic = CreateIconFromResourceEx(bytes.as_ptr(), bytes.len() as u32, 1, 0x0003_0000, px, px, 0);
        if !ic.is_null() {
            SendMessageW(host, WM_SETICON, kind, ic as isize);
        }
    }
    SetWindowPos(host, null_mut(), 0, 0, 0, 0, SWP_FRAMECHANGED | SWP_NOMOVE | SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE);
    let tcls = wide("CubelinkTitlebar");
    let mut r: RECT = std::mem::zeroed();
    GetClientRect(host, &mut r);
    let tb = titlebar_px(host);
    let tbar = CreateWindowExW(0, tcls.as_ptr(), null(), WS_CHILD | WS_VISIBLE | WS_CLIPSIBLINGS, 0, 0, r.right, tb, host, null_mut(), hinst, null());
    if tbar.is_null() {
        return Err("CreateWindowExW (barre de titre) a echoue".into());
    }
    s.titlebar.store(tbar as isize, Ordering::SeqCst);
    let ccls = wide("CubelinkCurtain");
    let (gx, gy, gw, gh) = game_rect(host);
    let curtain = CreateWindowExW(WS_EX_LAYERED, ccls.as_ptr(), null(), WS_CHILD | WS_CLIPSIBLINGS, gx, gy, gw, gh, host, null_mut(), hinst, null());
    if curtain.is_null() {
        return Err("CreateWindowExW (rideau) a echoue".into());
    }
    s.curtain.store(curtain as isize, Ordering::SeqCst);
    SetTimer(curtain, TIMER_CURTAIN, 16, None);
    Ok(())
}

/// Hauteur de la barre de titre en pixels physiques.
unsafe fn titlebar_px(host: HWND) -> i32 {
    (TITLEBAR_H * dpi_scale(host)).round() as i32
}

/// Zone du jeu (x, y, largeur, hauteur) dans la zone cliente de l'hote : sous la barre de titre, dans le cadre.
unsafe fn game_rect(host: HWND) -> (i32, i32, i32, i32) {
    let mut r: RECT = std::mem::zeroed();
    GetClientRect(host, &mut r);
    let tb = titlebar_px(host);
    let b = if IsZoomed(host) != 0 { 0 } else { (BORDER * dpi_scale(host)).round() as i32 };
    (b, tb, (r.right - 2 * b).max(1), (r.bottom - tb - b).max(1))
}

/// Recale la barre de titre, le rideau et les fenetres adoptees sur la taille de l'hote.
unsafe fn fit_children(s: &Shared) {
    let host = s.host();
    let mut r: RECT = std::mem::zeroed();
    GetClientRect(host, &mut r);
    let (gx, gy, gw, gh) = game_rect(host);
    let tb = titlebar_px(host);
    let tbar = hw(s.titlebar.load(Ordering::SeqCst));
    if !tbar.is_null() {
        SetWindowPos(tbar, null_mut(), 0, 0, r.right, tb, SWP_NOZORDER | SWP_NOACTIVATE);
        InvalidateRect(tbar, null(), 0);
    }
    SetWindowPos(s.curtain(), null_mut(), gx, gy, gw, gh, SWP_NOZORDER | SWP_NOACTIVATE);
    let adopted: Vec<isize> = s.lock().adopted.clone();
    for a in adopted {
        // asynchrone : ne bloque jamais l'interface sur un jeu occupe
        SetWindowPos(hw(a), null_mut(), gx, gy, gw, gh, SWP_NOZORDER | SWP_NOACTIVATE | SWP_ASYNCWINDOWPOS);
    }
}

/// Titre affiche : « Cubelink » ou « Cubelink · modpack ».
fn full_title(s: &Shared) -> String {
    let title = s.lock().title.clone();
    if title.is_empty() {
        "Cubelink".to_string()
    } else {
        format!("Cubelink \u{b7} {title}")
    }
}

unsafe extern "system" fn host_proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    let Some(s) = shared() else { return DefWindowProcW(hwnd, msg, wp, lp) };
    match msg {
        WM_SIZE => {
            fit_children(s);
            InvalidateRect(hwnd, null(), 0);
            0
        }
        // Cadre custom : toute la fenetre est « client » ; la barre de titre est dessinee par nous.
        WM_NCCALCSIZE => {
            if wp != 0 && IsZoomed(hwnd) != 0 {
                let dpi = GetDpiForWindow(hwnd).max(96);
                let fx = GetSystemMetricsForDpi(SM_CXFRAME, dpi) + GetSystemMetricsForDpi(SM_CXPADDEDBORDER, dpi);
                let fy = GetSystemMetricsForDpi(SM_CYFRAME, dpi) + GetSystemMetricsForDpi(SM_CXPADDEDBORDER, dpi);
                let p = &mut *(lp as *mut NCCALCSIZE_PARAMS);
                p.rgrc[0].left += fx;
                p.rgrc[0].right -= fx;
                p.rgrc[0].top += fy;
                p.rgrc[0].bottom -= fy;
            }
            0
        }
        WM_NCHITTEST => {
            let (x, y) = (lo(lp), hi(lp));
            let mut wr: RECT = std::mem::zeroed();
            GetWindowRect(hwnd, &mut wr);
            if IsZoomed(hwnd) == 0 {
                let b = (BORDER * dpi_scale(hwnd)).round() as i32;
                let (l, r, t, bt) = (x < wr.left + b, x >= wr.right - b, y < wr.top + b, y >= wr.bottom - b);
                let code = match (l, r, t, bt) {
                    (true, _, true, _) => HTTOPLEFT,
                    (_, true, true, _) => HTTOPRIGHT,
                    (true, _, _, true) => HTBOTTOMLEFT,
                    (_, true, _, true) => HTBOTTOMRIGHT,
                    (true, ..) => HTLEFT,
                    (_, true, ..) => HTRIGHT,
                    (_, _, true, _) => HTTOP,
                    (_, _, _, true) => HTBOTTOM,
                    _ => 0,
                };
                if code != 0 {
                    return code as LRESULT;
                }
            }
            let mut pt = POINT { x: 0, y: 0 };
            ClientToScreen(hwnd, &mut pt);
            if y < pt.y + titlebar_px(hwnd) {
                return HTCAPTION as LRESULT;
            }
            HTCLIENT as LRESULT
        }
        WM_ERASEBKGND => 1,
        WM_PAINT => {
            paint_background(hwnd, IsZoomed(hwnd) == 0);
            0
        }
        WM_SETFOCUS => {
            let cur = s.lock().current;
            if let Some(c) = cur {
                SetFocus(hw(c)); // meme file d'entrees que l'hote : le clavier va au jeu
            }
            0
        }
        WM_APP_CURTAIN_ON => {
            // le rideau se ferme (ou change de texte) : titre a jour, rideau affiche au-dessus du jeu et dessine tout de suite
            let c = s.curtain();
            SetWindowTextW(hwnd, wide(&full_title(s)).as_ptr());
            InvalidateRect(hw(s.titlebar.load(Ordering::SeqCst)), null(), 0);
            let (gx, gy, gw, gh) = game_rect(hwnd);
            SetWindowPos(c, HWND_TOP, gx, gy, gw, gh, SWP_NOACTIVATE | SWP_SHOWWINDOW);
            curtain_window::present(s, c);
            0
        }
        WM_APP_REVEAL => {
            on_reveal(s, hwnd, wp != 0);
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

unsafe extern "system" fn curtain_proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    let Some(s) = shared() else { return DefWindowProcW(hwnd, msg, wp, lp) };
    match msg {
        WM_APP_SYNC => {
            curtain_window::sync_taken();
            0
        }
        WM_TIMER => {
            // minuteur a cadence fixe ; le jeu, fenetre fille, est toujours en place : seuls comptent le dessin et le retrait du rideau
            let todo = curtain_window::tick(s);
            curtain_window::finish_tick(s, hwnd, &todo);
            0
        }
        _ => curtain_window::base_proc(hwnd, msg, wp, lp),
    }
}

/// Index du bouton (0 reduire, 1 agrandir, 2 fermer) sous le point `x` de la barre de titre, s'il y en a un.
fn title_button(x: i32, w: i32, scale: f32) -> Option<u8> {
    let bw = (46.0 * scale) as i32;
    (0..3u8).find(|i| x >= w - bw * (3 - *i as i32) && x < w - bw * (2 - *i as i32))
}

unsafe extern "system" fn titlebar_proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    let Some(s) = shared() else { return DefWindowProcW(hwnd, msg, wp, lp) };
    let host = s.host();
    match msg {
        WM_ERASEBKGND => 1,
        WM_NCHITTEST => {
            // seuls les boutons sont a nous ; le reste appartient a l'hote (deplacement, bords de redimensionnement)
            let mut pt = POINT { x: lo(lp), y: hi(lp) };
            ScreenToClient(hwnd, &mut pt);
            let mut r: RECT = std::mem::zeroed();
            GetClientRect(hwnd, &mut r);
            if title_button(pt.x, r.right, dpi_scale(hwnd)).is_some() {
                HTCLIENT as LRESULT
            } else {
                HTTRANSPARENT as LRESULT
            }
        }
        WM_MOUSEMOVE => {
            let mut r: RECT = std::mem::zeroed();
            GetClientRect(hwnd, &mut r);
            let hv = title_button(lo(lp), r.right, dpi_scale(hwnd)).map_or(-1, |b| b as i8);
            if TB_HOVER.with(|c| c.replace(hv)) != hv {
                InvalidateRect(hwnd, null(), 0);
            }
            let mut tme = TRACKMOUSEEVENT { cbSize: std::mem::size_of::<TRACKMOUSEEVENT>() as u32, dwFlags: TME_LEAVE, hwndTrack: hwnd, dwHoverTime: 0 };
            TrackMouseEvent(&mut tme);
            0
        }
        WM_MOUSELEAVE => {
            if TB_HOVER.with(|c| c.replace(-1)) != -1 {
                InvalidateRect(hwnd, null(), 0);
            }
            0
        }
        WM_LBUTTONUP => {
            let mut r: RECT = std::mem::zeroed();
            GetClientRect(hwnd, &mut r);
            let cmd = match title_button(lo(lp), r.right, dpi_scale(hwnd)) {
                Some(0) => SC_MINIMIZE,
                Some(1) => {
                    if IsZoomed(host) != 0 {
                        SC_RESTORE
                    } else {
                        SC_MAXIMIZE
                    }
                }
                Some(_) => SC_CLOSE,
                None => return 0,
            };
            PostMessageW(host, WM_SYSCOMMAND, cmd as usize, 0);
            0
        }
        WM_PAINT => {
            let mut ps: PAINTSTRUCT = std::mem::zeroed();
            let dc = BeginPaint(hwnd, &mut ps);
            let mut r: RECT = std::mem::zeroed();
            GetClientRect(hwnd, &mut r);
            let (w, h) = (r.right.max(1), r.bottom.max(1));
            let text = full_title(s);
            let hv = TB_HOVER.with(|c| c.get());
            let (scale, zoomed) = (dpi_scale(hwnd), IsZoomed(host) != 0);
            // jamais de panic dans une procedure de fenetre : si le moteur est deja emprunte, cette image est sautee
            let pm = CURTAIN.with(|c| c.try_borrow_mut().ok().map(|mut c| c.render_titlebar(&text, w as u32, h as u32, scale, (hv >= 0).then_some(hv as u8), zoomed)));
            if let Some(pm) = pm {
                let bgra = curtain::to_bgra(&pm);
                let bi = bitmap_info(w, h);
                SetDIBitsToDevice(dc, 0, 0, w as u32, h as u32, 0, 0, 0, h as u32, bgra.as_ptr() as _, &bi, DIB_RGB_COLORS);
            }
            EndPaint(hwnd, &ps);
            0
        }
        _ => DefWindowProcW(hwnd, msg, wp, lp),
    }
}

/// Les fenetres adoptees sont des filles de l'hote.
pub(super) unsafe fn is_attached(s: &Shared, hwnd: HWND) -> bool {
    let host = s.host();
    !host.is_null() && GetParent(hwnd) == host
}

/// LA fenetre du jeu vient de s'afficher : le rideau reste au-dessus d'elle, le clavier va au jeu.
pub(super) unsafe fn on_game_shown(s: &Shared, hwnd: HWND) {
    SetWindowPos(s.curtain(), HWND_TOP, 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE);
    SetFocus(hwnd);
}

/// Le jeu se recentre lui-meme apres l'adoption : on remet sa fenetre a sa place des qu'elle bouge.
pub(super) unsafe fn on_moved(s: &Shared, hwnd: HWND) {
    let host = s.host();
    if host.is_null() || hwnd == host || hwnd == s.curtain() || GetParent(hwnd) != host {
        return;
    }
    let (gx, gy, gw, gh) = game_rect(host);
    let mut wr: RECT = std::mem::zeroed();
    GetWindowRect(hwnd, &mut wr);
    let mut pt = POINT { x: wr.left, y: wr.top };
    ScreenToClient(host, &mut pt);
    if pt.x != gx || pt.y != gy || wr.right - wr.left != gw || wr.bottom - wr.top != gh {
        SetWindowPos(hwnd, null_mut(), gx, gy, gw, gh, SWP_NOZORDER | SWP_NOACTIVATE | SWP_ASYNCWINDOWPOS);
    }
}

/// Adoption : la fenetre du jeu devient une fille de l'hote, sous le rideau. Sur le fil de la fenetre du jeu (appels synchrones
/// vers le jeu).
pub(super) unsafe fn adopt(s: &Shared, h: isize) {
    let (win, host) = (hw(h), s.host());
    let style = GetWindowLongPtrW(win, GWL_STYLE) as u32;
    SetWindowLongPtrW(win, GWL_STYLE, ((style & !STRIP) | WS_CHILD | WS_CLIPSIBLINGS) as isize);
    SetParent(win, host);
    let (gx, gy, gw, gh) = game_rect(host);
    s.lock().adopted.push(h);
    SetWindowPos(win, HWND_BOTTOM, gx, gy, gw, gh, SWP_FRAMECHANGED | SWP_NOACTIVATE);
    SetWindowPos(s.curtain(), HWND_TOP, 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE);
}

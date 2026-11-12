//! Petits outils Win32 sans etat : chaines, classes de fenetre, captures.

use std::collections::HashSet;
use std::ptr::{null, null_mut};

use windows_sys::Win32::Foundation::{HWND, LPARAM, POINT, RECT};
use windows_sys::Win32::Graphics::Gdi::*;
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::UI::HiDpi::GetDpiForWindow;
use windows_sys::Win32::UI::WindowsAndMessaging::*;

use super::TopLevel;

pub(super) fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

pub(super) fn hw(v: isize) -> HWND {
    v as HWND
}

pub(super) fn lo(lp: LPARAM) -> i32 {
    (lp & 0xFFFF) as i16 as i32
}

pub(super) fn hi(lp: LPARAM) -> i32 {
    ((lp >> 16) & 0xFFFF) as i16 as i32
}

/// Enregistre une classe de fenetre (curseur fleche ; pinceau de fond `brush`, 0 = aucun).
pub(super) unsafe fn register_class(name: &str, proc: WNDPROC, brush: isize) {
    let cls = wide(name);
    let wc = WNDCLASSW {
        style: 0,
        lpfnWndProc: proc,
        cbClsExtra: 0,
        cbWndExtra: 0,
        hInstance: GetModuleHandleW(null()),
        hIcon: null_mut(),
        hCursor: LoadCursorW(null_mut(), IDC_ARROW),
        hbrBackground: brush as _,
        lpszMenuName: null(),
        lpszClassName: cls.as_ptr(),
    };
    RegisterClassW(&wc);
}

/// Fond de la zone de jeu (couleur du launcher) ; `frame` : lisere de la fenetre hote autonome.
pub(super) unsafe fn paint_background(hwnd: HWND, frame: bool) {
    let mut ps: PAINTSTRUCT = std::mem::zeroed();
    let dc = BeginPaint(hwnd, &mut ps);
    let mut r: RECT = std::mem::zeroed();
    GetClientRect(hwnd, &mut r);
    let bg = CreateSolidBrush(0x00_16_11_13);
    FillRect(dc, &r, bg);
    DeleteObject(bg as _);
    if frame {
        let line = CreateSolidBrush(0x00_3A_2E_33);
        FrameRect(dc, &r, line);
        DeleteObject(line as _);
    }
    EndPaint(hwnd, &ps);
}

pub(super) unsafe fn class_name(h: HWND) -> String {
    let mut buf = [0u16; 128];
    let n = GetClassNameW(h, buf.as_mut_ptr(), 128);
    String::from_utf16_lossy(&buf[..n.max(0) as usize])
}

unsafe fn window_title(h: HWND) -> String {
    let n = GetWindowTextLengthW(h);
    let mut buf = vec![0u16; n.max(0) as usize + 1];
    let n = GetWindowTextW(h, buf.as_mut_ptr(), buf.len() as i32);
    String::from_utf16_lossy(&buf[..n.max(0) as usize])
}

/// Facteur d'echelle (dpi) de la fenetre.
pub(super) unsafe fn dpi_scale(hwnd: HWND) -> f32 {
    (GetDpiForWindow(hwnd).max(96) as f32) / 96.0
}

pub(super) unsafe fn rect_differs(h: HWND, x: i32, y: i32, w: i32, hgt: i32) -> bool {
    let mut r: RECT = std::mem::zeroed();
    GetWindowRect(h, &mut r);
    r.left != x || r.top != y || r.right - r.left != w || r.bottom - r.top != hgt
}

pub(super) fn toplevel_of(pids: &HashSet<u32>) -> Vec<TopLevel> {
    struct Ctx<'a> {
        pids: &'a HashSet<u32>,
        out: Vec<TopLevel>,
    }
    unsafe extern "system" fn cb(h: HWND, lp: LPARAM) -> i32 {
        let ctx = &mut *(lp as *mut Ctx);
        let mut pid = 0u32;
        GetWindowThreadProcessId(h, &mut pid);
        if ctx.pids.contains(&pid) {
            let mut r: RECT = std::mem::zeroed();
            GetWindowRect(h, &mut r);
            ctx.out.push(TopLevel {
                class: class_name(h),
                title: window_title(h),
                visible: IsWindowVisible(h) != 0,
                x: r.left,
                y: r.top,
                w: r.right - r.left,
                h: r.bottom - r.top,
            });
        }
        1
    }
    let mut ctx = Ctx { pids, out: Vec::new() };
    unsafe { EnumWindows(Some(cb), &mut ctx as *mut Ctx as LPARAM) };
    ctx.out
}

pub(super) fn bitmap_info(w: i32, h: i32) -> BITMAPINFO {
    BITMAPINFO {
        bmiHeader: BITMAPINFOHEADER {
            biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
            biWidth: w,
            biHeight: -h, // haut en bas
            biPlanes: 1,
            biBitCount: 32,
            biCompression: BI_RGB,
            biSizeImage: 0,
            biXPelsPerMeter: 0,
            biYPelsPerMeter: 0,
            biClrUsed: 0,
            biClrImportant: 0,
        },
        bmiColors: [RGBQUAD { rgbBlue: 0, rgbGreen: 0, rgbRed: 0, rgbReserved: 0 }],
    }
}

/// Copie de la zone cliente de `hwnd` telle qu'affichee a l'ecran (le jeu enfant, le rideau, tout).
pub(super) unsafe fn capture_client(hwnd: HWND) -> Option<(i32, i32, Vec<u8>)> {
    let mut r: RECT = std::mem::zeroed();
    GetClientRect(hwnd, &mut r);
    let (w, h) = (r.right, r.bottom);
    if w <= 0 || h <= 0 {
        return None;
    }
    let mut pt = POINT { x: 0, y: 0 };
    ClientToScreen(hwnd, &mut pt);
    let screen = GetDC(null_mut());
    let mem = CreateCompatibleDC(screen);
    let bmp = CreateCompatibleBitmap(screen, w, h);
    let old = SelectObject(mem, bmp as _);
    BitBlt(mem, 0, 0, w, h, screen, pt.x, pt.y, SRCCOPY);
    let mut buf = vec![0u8; (w * h * 4) as usize];
    let bi = bitmap_info(w, h);
    let ok = GetDIBits(mem, bmp, 0, h as u32, buf.as_mut_ptr() as _, &bi as *const _ as *mut _, DIB_RGB_COLORS);
    SelectObject(mem, old);
    DeleteObject(bmp as _);
    DeleteDC(mem);
    ReleaseDC(null_mut(), screen);
    if ok == 0 {
        None
    } else {
        Some((w, h, buf))
    }
}

/// Ecrit une image BGRA en BMP 32 bits (pour les rapports de test).
pub fn write_bmp(path: &std::path::Path, w: i32, h: i32, bgra: &[u8]) -> std::io::Result<()> {
    use std::io::Write;
    let size = 54 + bgra.len() as u32;
    let mut f = std::fs::File::create(path)?;
    f.write_all(b"BM")?;
    f.write_all(&size.to_le_bytes())?;
    f.write_all(&[0, 0, 0, 0])?;
    f.write_all(&54u32.to_le_bytes())?;
    f.write_all(&40u32.to_le_bytes())?;
    f.write_all(&w.to_le_bytes())?;
    f.write_all(&(-h).to_le_bytes())?;
    f.write_all(&1u16.to_le_bytes())?;
    f.write_all(&32u16.to_le_bytes())?;
    f.write_all(&[0; 24])?;
    f.write_all(bgra)
}

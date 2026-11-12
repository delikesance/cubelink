//! Petits outils Win32 sans etat : chaines, classes de fenetre, geometrie.

use std::ptr::{null, null_mut};

use windows_sys::Win32::Foundation::{HWND, RECT};
use windows_sys::Win32::Graphics::Gdi::*;
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::UI::HiDpi::GetDpiForWindow;
use windows_sys::Win32::UI::WindowsAndMessaging::*;

pub(super) fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

pub(super) fn hw(v: isize) -> HWND {
    v as HWND
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

/// Fond de la zone de jeu (couleur du launcher).
pub(super) unsafe fn paint_background(hwnd: HWND) {
    let mut ps: PAINTSTRUCT = std::mem::zeroed();
    let dc = BeginPaint(hwnd, &mut ps);
    let mut r: RECT = std::mem::zeroed();
    GetClientRect(hwnd, &mut r);
    let bg = CreateSolidBrush(0x00_16_11_13);
    FillRect(dc, &r, bg);
    DeleteObject(bg as _);
    EndPaint(hwnd, &ps);
}

pub(super) unsafe fn class_name(h: HWND) -> String {
    let mut buf = [0u16; 128];
    let n = GetClassNameW(h, buf.as_mut_ptr(), 128);
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

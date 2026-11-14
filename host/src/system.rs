//! Préférences du système qui règlent la Scène : mouvement réduit (rideau sans animation).

/// Preference systeme « afficher les animations dans Windows » desactivee : le rideau ne doit pas s'animer. Illisible :
/// animations gardees (comportement par defaut de Windows).
#[cfg(windows)]
pub fn reduced_motion() -> bool {
    use windows_sys::Win32::UI::WindowsAndMessaging::{SystemParametersInfoW, SPI_GETCLIENTAREAANIMATION};
    let mut animations: i32 = 1;
    // SAFETY : `SPI_GETCLIENTAREAANIMATION` ecrit un BOOL (i32) a l'adresse fournie, valide pendant l'appel.
    let ok = unsafe { SystemParametersInfoW(SPI_GETCLIENTAREAANIMATION, 0, (&mut animations as *mut i32).cast(), 0) };
    ok != 0 && animations == 0
}

#[cfg(not(any(windows, target_os = "linux")))]
pub fn reduced_motion() -> bool {
    false
}

/// Linux : reglage GNOME « animations » desactive (`gsettings get org.gnome.desktop.interface enable-animations` rend
/// `false`). Illisible (pas de gsettings, autre bureau) : animations gardees.
#[cfg(target_os = "linux")]
pub fn reduced_motion() -> bool {
    let out = std::process::Command::new("gsettings")
        .args(["get", "org.gnome.desktop.interface", "enable-animations"])
        .stdin(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .output();
    out.is_ok_and(|o| o.status.success() && animations_off(&String::from_utf8_lossy(&o.stdout)))
}

/// Pur : sortie de `gsettings get ... enable-animations` -> animations desactivees ?
#[cfg(any(target_os = "linux", test))]
fn animations_off(gsettings_out: &str) -> bool {
    gsettings_out.trim() == "false"
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn animations_gnome() {
        assert!(animations_off("false\n"));
        assert!(!animations_off("true\n"));
        assert!(!animations_off(""));
    }
}

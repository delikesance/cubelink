//! Mémoire vive physique de l'ordinateur, sans dépendance de plus : `/proc/meminfo` sous Linux,
//! `GlobalMemoryStatusEx` sous Windows. Sert à choisir la mémoire « Automatique » du jeu (voir
//! `instance::auto_ram_mb`) et à avertir quand un réglage manuel en demande trop.

/// RAM physique totale en Mo, ou `None` si le système ne la donne pas (plateforme non gérée, lecture refusée).
pub fn physical_ram_mb() -> Option<u64> {
    #[cfg(target_os = "linux")]
    {
        parse_meminfo(&std::fs::read_to_string("/proc/meminfo").ok()?)
    }
    #[cfg(windows)]
    {
        windows_total_mb()
    }
    #[cfg(not(any(target_os = "linux", windows)))]
    {
        None
    }
}

/// Lit `MemTotal` (en kio) dans le texte de `/proc/meminfo` et le rend en Mo. `None` si absent, mal formé ou nul.
pub fn parse_meminfo(text: &str) -> Option<u64> {
    let line = text.lines().find(|l| l.starts_with("MemTotal:"))?;
    let mut parts = line["MemTotal:".len()..].split_whitespace();
    let kib: u64 = parts.next()?.parse().ok()?;
    match parts.next() {
        Some("kB") | None => {}
        Some(_) => return None,
    }
    (kib >= 1024).then_some(kib / 1024)
}

#[cfg(windows)]
fn windows_total_mb() -> Option<u64> {
    use windows_sys::Win32::System::SystemInformation::{GlobalMemoryStatusEx, MEMORYSTATUSEX};
    // SAFETY: `MEMORYSTATUSEX` est une structure de nombres ; `dwLength` doit valoir sa taille avant l'appel,
    // qui n'écrit que dans la structure passée.
    unsafe {
        let mut st: MEMORYSTATUSEX = std::mem::zeroed();
        st.dwLength = std::mem::size_of::<MEMORYSTATUSEX>() as u32;
        if GlobalMemoryStatusEx(&mut st) == 0 {
            return None;
        }
        let mb = st.ullTotalPhys / (1024 * 1024);
        (mb > 0).then_some(mb)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn meminfo_valide() {
        let t = "MemTotal:       16384000 kB\nMemFree:         1000 kB\n";
        assert_eq!(parse_meminfo(t), Some(16000));
        assert_eq!(parse_meminfo("MemFree: 5 kB\nMemTotal: 2097152 kB"), Some(2048));
    }

    #[test]
    fn meminfo_mal_forme() {
        assert_eq!(parse_meminfo(""), None);
        assert_eq!(parse_meminfo("MemTotal: beaucoup kB"), None);
        assert_eq!(parse_meminfo("MemTotal: 0 kB"), None);
        assert_eq!(parse_meminfo("MemTotal: 12 MB"), None);
        assert_eq!(parse_meminfo("MemFree: 100 kB"), None);
    }

    /// Lecture réelle : la machine qui exécute le test répond, avec une valeur plausible.
    #[test]
    #[cfg(any(target_os = "linux", windows))]
    fn lecture_reelle() {
        let mb = physical_ram_mb().expect("la RAM physique doit se lire");
        assert!((256..16 * 1024 * 1024).contains(&mb), "valeur invraisemblable : {mb} Mo");
    }
}

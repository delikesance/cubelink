//! Mémoire physique de la machine (sans dépendance supplémentaire). Sert à dimensionner le serveur de groupe
//! (`hearth::server::server_ram_mb`). C'est la mémoire **totale** : stable, contrairement à la mémoire libre.

/// Mémoire physique totale en Mo, ou `None` si elle ne peut pas être lue (cible non gérée, échec de l'appel).
pub fn physical_memory_mb() -> Option<u64> {
    imp::physical_memory_mb()
}

/// Lit `MemTotal` (en kB) dans le texte de `/proc/meminfo` et le donne en Mo. `None` : ligne absente ou mal formée, ou valeur nulle.
pub fn parse_meminfo(text: &str) -> Option<u64> {
    let line = text.lines().find(|l| l.starts_with("MemTotal:"))?;
    let mut it = line["MemTotal:".len()..].split_whitespace();
    let kb: u64 = it.next()?.parse().ok()?;
    // Le noyau écrit toujours « kB » ; une autre unité voudrait dire un format inconnu.
    if it.next().is_some_and(|u| !u.eq_ignore_ascii_case("kb")) {
        return None;
    }
    (kb >= 1024).then_some(kb / 1024)
}

#[cfg(windows)]
mod imp {
    use windows_sys::Win32::System::SystemInformation::{GlobalMemoryStatusEx, MEMORYSTATUSEX};

    pub fn physical_memory_mb() -> Option<u64> {
        // SAFETY : `MEMORYSTATUSEX` est une structure de nombres, `dwLength` est renseigné comme l'API l'exige.
        let mut st: MEMORYSTATUSEX = unsafe { std::mem::zeroed() };
        st.dwLength = std::mem::size_of::<MEMORYSTATUSEX>() as u32;
        if unsafe { GlobalMemoryStatusEx(&mut st) } == 0 {
            return None;
        }
        let mb = st.ullTotalPhys / (1024 * 1024);
        (mb > 0).then_some(mb)
    }
}

#[cfg(all(unix, not(windows)))]
mod imp {
    pub fn physical_memory_mb() -> Option<u64> {
        super::parse_meminfo(&std::fs::read_to_string("/proc/meminfo").ok()?)
    }
}

#[cfg(not(any(windows, unix)))]
mod imp {
    pub fn physical_memory_mb() -> Option<u64> {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn meminfo_parsing() {
        let sample = "MemTotal:       16384000 kB\nMemFree:         1234567 kB\n";
        assert_eq!(parse_meminfo(sample), Some(16000));
        assert_eq!(parse_meminfo("MemFree: 12 kB\n"), None);
        assert_eq!(parse_meminfo("MemTotal: beaucoup kB\n"), None);
        assert_eq!(parse_meminfo("MemTotal: 2048 pages\n"), None);
        assert_eq!(parse_meminfo("MemTotal: 0 kB\n"), None);
        assert_eq!(parse_meminfo(""), None);
    }

    /// Lecture réelle de la machine qui exécute les tests (Linux : `/proc/meminfo` ; Windows : `GlobalMemoryStatusEx`).
    #[cfg(any(unix, windows))]
    #[test]
    fn real_physical_memory_is_plausible() {
        let mb = physical_memory_mb().expect("la mémoire physique doit être lisible");
        assert!((256..16 * 1024 * 1024).contains(&mb), "valeur invraisemblable : {mb} Mo");
    }
}

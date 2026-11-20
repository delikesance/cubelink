//! Espace disque : mesure de l'espace libre et avertissement avant un gros transfert.
//!
//! Partagé par Hearth (`free_bytes` des garde-fous de réception) et par « Rejoindre » (avertissement non bloquant).

use std::path::Path;

/// Marge ajoutée à ce qui sera reçu avant d'avertir : fichiers temporaires, copies des configs dans l'instance,
/// bibliothèques et ressources du jeu, journaux. Un avertissement n'est jamais un refus : la mesure peut être fausse.
pub const JOIN_RESERVE_BYTES: u64 = 1 << 30;

/// Espace libre (octets) sur le disque qui porte `path`, s'il se mesure ; `None` sinon (la vérification est alors
/// sautée, jamais un refus).
pub fn free_space(path: &Path) -> Option<u64> {
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStrExt;
        #[link(name = "kernel32")]
        extern "system" {
            fn GetDiskFreeSpaceExW(dir: *const u16, available: *mut u64, total: *mut u64, free: *mut u64) -> i32;
        }
        let wide: Vec<u16> = path.as_os_str().encode_wide().chain(std::iter::once(0)).collect();
        let mut available = 0u64;
        // SAFETY : `wide` est une chaîne terminée par zéro qui vit pendant l'appel ; les pointeurs de sortie inutiles sont nuls
        // (permis par l'API) et `available` est un entier valide.
        let ok = unsafe { GetDiskFreeSpaceExW(wide.as_ptr(), &mut available, std::ptr::null_mut(), std::ptr::null_mut()) };
        (ok != 0).then_some(available)
    }
    #[cfg(not(windows))]
    {
        let out = std::process::Command::new("df").arg("-Pk").arg(path).output().ok()?;
        let text = String::from_utf8_lossy(&out.stdout);
        let kib: u64 = text.lines().nth(1)?.split_whitespace().nth(3)?.parse().ok()?;
        kib.checked_mul(1024)
    }
}

/// Comme [`free_space`], mais sur le plus proche ancêtre existant de `path` : le dossier visé peut ne pas être créé
/// encore (premier lancement), et `df` comme `GetDiskFreeSpaceExW` échouent sur un chemin absent.
pub fn free_space_near(path: &Path) -> Option<u64> {
    let existing = path.ancestors().find(|p| !p.as_os_str().is_empty() && p.exists())?;
    free_space(existing)
}

/// `Some((besoin, libre))` quand l'espace libre ne couvre pas `to_receive_bytes` plus [`JOIN_RESERVE_BYTES`].
/// `None` : assez de place, rien à recevoir (tout est déjà dans le cache) ou espace libre inconnu.
pub fn disk_shortfall(to_receive_bytes: u64, free: Option<u64>) -> Option<(u64, u64)> {
    let free = free?;
    if to_receive_bytes == 0 {
        return None;
    }
    let need = to_receive_bytes.saturating_add(JOIN_RESERVE_BYTES);
    (free < need).then_some((need, free))
}

#[cfg(test)]
mod tests {
    use super::*;

    const GIB: u64 = 1 << 30;

    #[test]
    fn manque_de_place() {
        // 3 Gio à recevoir + 1 Gio de marge = 4 Gio ; il reste 1 Gio.
        assert_eq!(disk_shortfall(3 * GIB, Some(GIB)), Some((4 * GIB, GIB)));
    }

    #[test]
    fn assez_de_place_ou_mesure_inconnue() {
        assert_eq!(disk_shortfall(3 * GIB, Some(10 * GIB)), None);
        assert_eq!(disk_shortfall(3 * GIB, None), None);
    }

    #[test]
    fn limite_exacte_sans_avertissement() {
        assert_eq!(disk_shortfall(GIB, Some(2 * GIB)), None);
        assert_eq!(disk_shortfall(GIB, Some(2 * GIB - 1)), Some((2 * GIB, 2 * GIB - 1)));
    }

    #[test]
    fn rien_a_recevoir_jamais_d_avertissement() {
        assert_eq!(disk_shortfall(0, Some(0)), None);
    }

    #[test]
    fn besoin_sans_debordement() {
        assert_eq!(disk_shortfall(u64::MAX, Some(5)), Some((u64::MAX, 5)));
    }

    #[test]
    fn mesure_sur_un_dossier_existant_et_sur_son_ancetre() {
        let dir = tempfile::tempdir().unwrap();
        let direct = free_space(dir.path()).expect("df sur un dossier existant");
        assert!(direct > 0);
        // Dossier pas encore créé : l'ancêtre existant porte la mesure.
        let absent = dir.path().join("magasin").join("pas-encore");
        assert!(free_space_near(&absent).is_some());
    }

    /// Essai réel sur un volume choisi (par exemple un tmpfs de 20 Mio) :
    /// `CUBELINK_TEST_DISK_DIR=/mnt/petit cargo test -p cubelink-core --lib disk::tests::volume_reel -- --ignored`.
    #[test]
    #[ignore = "demande un volume réel : CUBELINK_TEST_DISK_DIR"]
    fn volume_reel() {
        let dir = std::env::var("CUBELINK_TEST_DISK_DIR").expect("CUBELINK_TEST_DISK_DIR");
        let free = free_space_near(&Path::new(&dir).join("absent")).expect("espace libre mesuré");
        eprintln!("libre mesuré : {free} octets");
        assert!(disk_shortfall(free, Some(free)).is_some());
        assert!(disk_shortfall(1, Some(free)).is_some() == (free < 1 + JOIN_RESERVE_BYTES));
    }
}

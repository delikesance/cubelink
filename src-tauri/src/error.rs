//! Messages d'erreur renvoyés au front : français, sans jeton ni chemin système.

use core_lib::types::Error;

/// Message affichable pour une erreur du coeur.
pub fn user_msg(e: &Error) -> String {
    sanitize(&e.to_string())
}

/// Convertit une erreur anyhow (p2p) en message affichable.
pub fn user_msg_any(e: &dyn std::fmt::Display) -> String {
    sanitize(&format!("{e:#}"))
}

fn looks_like_path(w: &str) -> bool {
    let b = w.as_bytes();
    (b.len() >= 3 && b[0].is_ascii_alphabetic() && b[1] == b':' && (b[2] == b'\\' || b[2] == b'/'))
        || w.starts_with("\\\\")
        || (w.starts_with('/') && w[1..].contains('/'))
        || w.contains(":\\")
}

fn looks_like_secret(w: &str) -> bool {
    if w.contains("://") || w.len() < 32 {
        return false;
    }
    let charset = w.chars().all(|c| c.is_ascii_alphanumeric() || "-_.=+".contains(c));
    let pure_hex = w.chars().all(|c| c.is_ascii_hexdigit());
    charset && !pure_hex && (w.contains('.') || w.len() >= 100)
}

fn clean_word(w: &str) -> String {
    let core = w.trim_matches(|c| matches!(c, '"' | '\'' | '(' | ')' | ',' | ';' | '[' | ']' | '«' | '»'));
    if core.is_empty() {
        return w.to_string();
    }
    if looks_like_path(core) {
        w.replacen(core, "<chemin>", 1)
    } else if looks_like_secret(core) {
        w.replacen(core, "***", 1)
    } else {
        w.to_string()
    }
}

/// Dossiers connus à masquer même sans séparateur de chemin derrière (profil, données).
/// Lus une seule fois dans l'environnement ; les valeurs trop courtes ou égales à une racine sont ignorées.
fn home_prefixes() -> &'static [String] {
    static P: std::sync::OnceLock<Vec<String>> = std::sync::OnceLock::new();
    P.get_or_init(|| {
        let vars = ["HOME", "USERPROFILE", "APPDATA", "LOCALAPPDATA", "CUBELINK_DATA_DIR"];
        let mut v: Vec<String> = vars
            .iter()
            .filter_map(std::env::var_os)
            .filter_map(|o| o.into_string().ok())
            .map(|s| s.trim_end_matches(['/', '\\']).to_string())
            .filter(|s| s.chars().count() >= 4)
            .collect();
        v.sort_by_key(|s| std::cmp::Reverse(s.len()));
        v.dedup();
        v
    })
}

fn is_sep(c: char) -> bool {
    c == '/' || c == '\\'
}

/// Début de chemin en `i` (déjà à une frontière de mot) : renvoie `(fin_du_préfixe, unix)`.
fn path_start(c: &[char], i: usize, prefixes: &[String]) -> Option<(usize, bool)> {
    for p in prefixes {
        let pc: Vec<char> = p.chars().collect();
        if c[i..].starts_with(&pc) {
            let end = i + pc.len();
            // Le préfixe doit s'arrêter là : fin de mot ou séparateur.
            if c.get(end).is_none_or(|&x| is_sep(x) || x.is_whitespace() || ",;:)]\"'»".contains(x)) {
                return Some((end, !p.contains('\\')));
            }
        }
    }
    let at = |k: usize| c.get(i + k).copied();
    if at(0).is_some_and(|x| x.is_ascii_alphabetic()) && at(1) == Some(':') && at(2).is_some_and(is_sep) {
        return Some((i + 3, false));
    }
    if at(0) == Some('\\') && at(1) == Some('\\') {
        // Préfixe verbatim (`\\?\C:\...`, produit par `canonicalize`) : sauter aussi la lettre de lecteur.
        if matches!(at(2), Some('?' | '.')) && at(3) == Some('\\') {
            let drive = at(4).is_some_and(|x| x.is_ascii_alphabetic()) && at(5) == Some(':') && at(6).is_some_and(is_sep);
            return Some((i + if drive { 7 } else { 4 }, false));
        }
        return Some((i + 2, false));
    }
    if at(0) == Some('/') && at(1).is_some_and(|x| !x.is_whitespace() && x != '/') {
        let rest = c[i + 1..].iter().take_while(|x| !x.is_whitespace());
        if rest.into_iter().any(|&x| x == '/') {
            return Some((i + 1, true));
        }
    }
    None
}

/// Fin (exclue) du chemin commencé avant `from`. Les espaces sont inclus tant que le mot suivant
/// porte un séparateur de chemin ; on s'arrête au premier séparateur de phrase.
fn path_end(c: &[char], from: usize, unix: bool) -> usize {
    let has_sep = |w: &[char]| {
        let s: String = w.iter().collect();
        !s.contains("://") && w.iter().any(|&x| if unix { x == '/' } else { is_sep(x) })
    };
    let word_end = |k: usize| c[k..].iter().position(|x| x.is_whitespace()).map_or(c.len(), |p| k + p);
    let mut j = from;
    let mut parens = 0usize;
    while j < c.len() {
        let x = c[j];
        match x {
            '(' => parens += 1,
            // Une parenthèse ouverte dans le chemin (`Jean (perso)`) se referme dedans.
            ')' if parens > 0 => parens -= 1,
            '"' | '\'' | ')' | ']' | ',' | ';' | '»' | '\n' | '\r' | ':' => break,
            _ => {}
        }
        if x.is_whitespace() {
            let mut k = j;
            while k < c.len() && c[k].is_whitespace() && c[k] != '\n' && c[k] != '\r' {
                k += 1;
            }
            if k < c.len() && c[k] != '\n' && c[k] != '\r' {
                // Le mot suivant porte un séparateur : le chemin continue. Sous Windows, jusqu'à deux mots
                // intermédiaires à majuscule (`Jean Marie Dupont\...`) sont aussi absorbés.
                let mut w = k;
                let mut extra = 0;
                loop {
                    let e = word_end(w);
                    if has_sep(&c[w..e]) {
                        j = w;
                        break;
                    }
                    let plain = !unix
                        && extra < 2
                        && c[w].is_uppercase()
                        && !c[w..e].iter().any(|&y| "\"',;:)]»".contains(y))
                        && e < c.len()
                        && c[e] == ' ';
                    if !plain {
                        break;
                    }
                    extra += 1;
                    w = e + 1;
                    if w >= c.len() {
                        break;
                    }
                }
                if j == w && has_sep(&c[w..word_end(w)]) {
                    continue;
                }
            }
            break;
        }
        j += 1;
    }
    j
}

/// Masque les chemins sur le message entier, espaces compris (`C:\Users\Jean Dupont\...`).
/// Limite connue : un dernier segment avec espace sans séparateur derrière et hors des dossiers
/// connus (`/data/mon fichier.txt`) n'est masqué que jusqu'au premier espace.
fn mask_paths(msg: &str, prefixes: &[String]) -> String {
    let c: Vec<char> = msg.chars().collect();
    let mut out = String::with_capacity(msg.len());
    let mut i = 0;
    while i < c.len() {
        let boundary = i == 0 || c[i - 1].is_whitespace() || "\"'([«=,".contains(c[i - 1]);
        if boundary {
            if let Some((after, unix)) = path_start(&c, i, prefixes) {
                let mut end = path_end(&c, after, unix);
                while end > after && c[end - 1] == '.' {
                    end -= 1;
                }
                out.extend(["<chemin>"]);
                i = end.max(i + 1);
                continue;
            }
        }
        out.push(c[i]);
        i += 1;
    }
    out
}

/// Remplace les chemins absolus et les chaînes ressemblant à un jeton.
pub fn sanitize(msg: &str) -> String {
    sanitize_with(msg, home_prefixes())
}

fn sanitize_with(msg: &str, prefixes: &[String]) -> String {
    mask_paths(msg, prefixes)
        .split_inclusive(char::is_whitespace)
        .map(|w| {
            let t = w.trim_end();
            format!("{}{}", clean_word(t), &w[t.len()..])
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn masque_chemins_et_jetons() {
        let s = sanitize("introuvable : C:\\Users\\bob\\AppData\\x.json et /home/bob/.cache/a ok");
        assert!(!s.contains("bob"), "{s}");
        assert!(s.contains("<chemin>"));
        let t = sanitize("jeton eyJhbGciOiJIUzI1NiJ9.eyJzdWIiOiIxMjM0NTY3ODkwIn0.abcdefghijk refusé");
        assert!(!t.contains("eyJ"), "{t}");
        assert!(sanitize("https://cdn.modrinth.com/data/a/b.jar").contains("https://"));
    }

    #[test]
    fn garde_les_hash() {
        let h = "a".repeat(64);
        assert!(sanitize(&format!("hash {h}")).contains(&h));
    }

    fn p(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn masque_chemins_avec_espaces() {
        let s = sanitize("introuvable : C:\\Users\\Jean Dupont\\AppData\\Roaming\\app.cubelink.launcher\\x.json");
        assert_eq!(s, "introuvable : <chemin>");
        let s = sanitize("lecture de /home/jean dupont/.local/share/x ok");
        assert_eq!(s, "lecture de <chemin> ok");
        assert_eq!(sanitize("\\\\srv\\partage nom\\f.txt"), "<chemin>");
        assert_eq!(sanitize("C:\\Users\\Léa Müller\\x"), "<chemin>");
        assert_eq!(sanitize("C:\\Users\\A B\\x et /home/c d/y ok"), "<chemin> et <chemin> ok");
    }

    #[test]
    fn garde_le_texte_autour_du_chemin() {
        assert_eq!(sanitize("C:\\Users\\Jean Dupont\\a.json: refusé"), "<chemin>: refusé");
        assert_eq!(sanitize("fichier /tmp/x/y: permission denied"), "fichier <chemin>: permission denied");
        assert_eq!(sanitize("échec (\"C:\\Users\\Jean Dupont\\a.json\"), puis"), "échec (\"<chemin>\"), puis");
        assert_eq!(sanitize("C:\\Users\\bob et ok"), "<chemin> et ok");
        assert_eq!(sanitize("voir /home/bob/a. Suite"), "voir <chemin>. Suite");
    }

    #[test]
    fn masque_prefixe_verbatim_noms_multiples_et_parentheses() {
        assert_eq!(sanitize("introuvable : \\\\?\\C:\\Users\\Jean Dupont\\AppData\\Roaming\\x.json"), "introuvable : <chemin>");
        assert_eq!(sanitize("C:\\Users\\Jean Marie Dupont\\AppData\\x.json ok"), "<chemin> ok");
        assert_eq!(sanitize("C:\\Users\\Jean (perso)\\x"), "<chemin>");
        assert_eq!(sanitize("fichier C:\\a\\b Introuvable dans le dossier"), "fichier <chemin> Introuvable dans le dossier");
    }

    #[test]
    fn ne_masque_pas_le_reste() {
        assert_eq!(sanitize("lecture/écriture a/b"), "lecture/écriture a/b");
        assert_eq!(sanitize("https://x/y z/w"), "https://x/y z/w");
        assert_eq!(sanitize("C:\\a\\b https://cdn.x/y.jar"), "<chemin> https://cdn.x/y.jar");
    }

    #[test]
    fn masque_les_dossiers_connus() {
        let px = p(&["/home/jean dupont"]);
        assert_eq!(sanitize_with("introuvable /home/jean dupont", &px), "introuvable <chemin>");
        // Résidu documenté : le dernier segment avec espace n'est masqué que jusqu'à l'espace,
        // mais le profil (nom, arborescence) ne fuit pas.
        let r = sanitize_with("lire /home/jean dupont/Mon fichier.txt", &px);
        assert!(!r.contains("dupont") && !r.contains("jean") && !r.contains("Mon"), "{r}");
        assert_eq!(sanitize_with("dossier=/home/jean dupont, ok", &px), "dossier=<chemin>, ok");
        // Le préfixe doit s'arrêter à une frontière : `dupontage` n'est pas le dossier connu.
        assert_eq!(sanitize_with("/home/jean dupontage", &px), "<chemin> dupontage");
    }

    #[test]
    fn erreur_disque_reelle_dans_un_dossier_avec_espace() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("Jean Dupont").join("AppData");
        std::fs::create_dir_all(&dir).unwrap();
        let fichier = dir.join("absent.json");
        let io = std::fs::read(&fichier).unwrap_err();
        let msg = user_msg(&Error::NotFound(format!("{} : {io}", fichier.display())));
        assert!(!msg.contains("Jean") && !msg.contains("Dupont") && !msg.contains("AppData"), "{msg}");
        assert!(msg.starts_with("introuvable : <chemin>"), "{msg}");
    }
}

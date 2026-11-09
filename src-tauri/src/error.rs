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

/// Remplace les chemins absolus et les chaînes ressemblant à un jeton.
pub fn sanitize(msg: &str) -> String {
    msg.split_inclusive(char::is_whitespace)
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
}

//! Liaisons TypeScript générées depuis Rust (`src/services/bindings.ts`) : une fonction par commande du mode normal
//! et les types qu'elles échangent, plus ceux des charges utiles d'événements. Ne jamais éditer le fichier à la main.
//!
//! Régénérer : `npm run bindings` (lance `CUBELINK_WRITE_BINDINGS=1 cargo test -p cubelink --lib bindings`).
//! Le test `liaisons_a_jour` échoue si le fichier ne correspond plus au code Rust (la CI le vérifie).

use specta_typescript::Typescript;

/// Chemin du fichier généré, relatif au dossier du crate `src-tauri`.
pub const BINDINGS_PATH: &str = "../src/services/bindings.ts";
/// Contrat lisible des commandes : le tableau entre ces deux marqueurs de `docs/contrats.md` est généré.
pub const CONTRACTS_PATH: &str = "../docs/contrats.md";
const TABLE_START: &str = "<!-- commandes : début (généré par src-tauri/src/bindings.rs, ne pas éditer) -->";
const TABLE_END: &str = "<!-- commandes : fin -->";

const HEADER: &str = "// Fichier généré par src-tauri/src/bindings.rs : ne pas éditer. Régénérer avec `npm run bindings`.";

/// Constructeur des liaisons : commandes du mode normal et types des événements.
pub fn export_builder() -> tauri_specta::Builder<tauri::Wry> {
    crate::specta_builder()
        // Les tailles et débits sont des u64 côté Rust ; ils restent sous 2^53 et le front les lit déjà en `number`.
        .dangerously_cast_bigints_to_number()
        .typ::<core_lib::types::TransferProgress>()
        .typ::<core_lib::types::Invite>()
        .typ::<core_lib::types::FriendRequest>()
        .typ::<core_lib::launch_run::LaunchStateEvent>()
        .typ::<crate::commands_host::HostState>()
        .typ::<crate::stage::StageState>()
}

/// Rend le contenu du fichier de liaisons (sans l'écrire à sa place).
pub fn render() -> String {
    let dir = std::env::temp_dir().join(format!("cubelink-bindings-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("dossier temporaire");
    let tmp = dir.join("bindings.ts");
    export_builder().export(Typescript::default().header(HEADER), &tmp).expect("export des liaisons TypeScript");
    let out = std::fs::read_to_string(&tmp).expect("lecture des liaisons");
    let _ = std::fs::remove_dir_all(&dir);
    snake_case_args(&out)
}

/// Les commandes sont déclarées `rename_all = "snake_case"` : Tauri attend `{ instance_id }`. tauri-specta 2.0.0-rc.29
/// écrit pourtant toujours les clés en camelCase (`{ instanceId }`) ; on réécrit donc chaque objet d'arguments
/// `{ instanceId, file }` en `{ instance_id: instanceId, file }`. Les paramètres des fonctions TS restent en camelCase.
fn snake_case_args(ts: &str) -> String {
    const MARK: &str = "\", { ";
    let mut out = String::with_capacity(ts.len() + 1024);
    for line in ts.split_inclusive('\n') {
        let rewritten = (|| {
            let start = line.find("__TAURI_INVOKE<")?;
            let open = start + line[start..].find(MARK)? + MARK.len();
            let close = open + line[open..].find(" }, ")?;
            let args = line[open..close]
                .split(", ")
                .map(|a| {
                    let snake = to_snake(a);
                    if snake == a { a.to_string() } else { format!("{snake}: {a}") }
                })
                .collect::<Vec<_>>()
                .join(", ");
            Some(format!("{}{}{}", &line[..open], args, &line[close..]))
        })();
        out.push_str(rewritten.as_deref().unwrap_or(line));
    }
    out
}

/// Tableau Markdown des commandes, tiré des liaisons générées : nom Tauri, arguments (clés JSON envoyées), retour, rôle.
pub fn commands_table(ts: &str) -> String {
    let mut out = String::from("| Commande | Arguments | Retour | Rôle |\n|---|---|---|---|\n");
    let mut doc: Vec<String> = Vec::new();
    let mut in_doc = false;
    for line in ts.lines() {
        let t = line.trim();
        if t.starts_with("/**") {
            in_doc = true;
            doc.clear();
        }
        if in_doc {
            let text = t.trim_start_matches("/**").trim_end_matches("*/").trim_start_matches('*').trim();
            if !text.is_empty() {
                doc.push(text.to_string());
            }
            if t.ends_with("*/") {
                in_doc = false;
            }
            continue;
        }
        let Some(inv) = t.find("__TAURI_INVOKE<") else {
            doc.clear();
            continue;
        };
        let params = t.find(": (").map(|i| &t[i + 3..]).and_then(|r| r.find(") => ").map(|j| &r[..j])).unwrap_or("");
        let args: Vec<String> = split_top(params)
            .into_iter()
            .filter(|p| !p.starts_with("options?"))
            .map(|p| match p.split_once(": ") {
                Some((n, ty)) => format!("`{}: {}`", to_snake(n), ty),
                None => format!("`{p}`"),
            })
            .collect();
        let after = &t[inv + "__TAURI_INVOKE<".len()..];
        let ret_end = after.find(">(\"").unwrap_or(after.len());
        let ret = &after[..ret_end];
        let name = after[ret_end..].trim_start_matches(">(\"").split('"').next().unwrap_or("");
        let ret = if ret == "null" || ret == "void" { "—".to_string() } else { format!("`{ret}`") };
        let args = if args.is_empty() { "—".to_string() } else { args.join(", ") };
        let role = doc.join(" ");
        out.push_str(&format!("| `{name}` | {} | {} | {} |\n", cell(&args), cell(&ret), cell(&role)));
        doc.clear();
    }
    out
}

/// Remplace le tableau des commandes de `docs/contrats.md` (entre les marqueurs) par `table`.
pub fn with_table(doc: &str, table: &str) -> Option<String> {
    let a = doc.find(TABLE_START)? + TABLE_START.len();
    let b = doc[a..].find(TABLE_END)? + a;
    Some(format!("{}\n{}{}", &doc[..a], table, &doc[b..]))
}

fn cell(s: &str) -> String {
    s.replace('|', "\\|")
}

/// Découpe une liste de paramètres TypeScript aux virgules de premier niveau.
fn split_top(s: &str) -> Vec<String> {
    let (mut parts, mut cur, mut depth) = (Vec::new(), String::new(), 0i32);
    for c in s.chars() {
        match c {
            '<' | '{' | '(' | '[' => depth += 1,
            '>' | '}' | ')' | ']' => depth -= 1,
            ',' if depth == 0 => {
                parts.push(cur.trim().to_string());
                cur.clear();
                continue;
            }
            _ => {}
        }
        cur.push(c);
    }
    if !cur.trim().is_empty() {
        parts.push(cur.trim().to_string());
    }
    parts
}

fn to_snake(ident: &str) -> String {
    let mut s = String::with_capacity(ident.len() + 4);
    for c in ident.chars() {
        if c.is_ascii_uppercase() {
            s.push('_');
            s.push(c.to_ascii_lowercase());
        } else {
            s.push(c);
        }
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    fn path() -> std::path::PathBuf {
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(BINDINGS_PATH)
    }

    fn contracts_path() -> std::path::PathBuf {
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(CONTRACTS_PATH)
    }

    /// `src/services/bindings.ts` correspond au code Rust ; `CUBELINK_WRITE_BINDINGS=1` le réécrit.
    #[test]
    fn arguments_en_snake_case() {
        let l = "\tf: (instanceId: string, file: string, options?: InvokeOptions) => __TAURI_INVOKE<null>(\"f\", { instanceId, file }, options),\n";
        assert_eq!(
            snake_case_args(l),
            "\tf: (instanceId: string, file: string, options?: InvokeOptions) => __TAURI_INVOKE<null>(\"f\", { instance_id: instanceId, file }, options),\n"
        );
        let none = "\tg: (options?: InvokeOptions) => __TAURI_INVOKE<null>(\"g\", undefined, options),\n";
        assert_eq!(snake_case_args(none), none);
    }

    #[test]
    fn liaisons_a_jour() {
        let fresh = render();
        let doc = std::fs::read_to_string(contracts_path()).expect("docs/contrats.md").replace("\r\n", "\n");
        let fresh_doc = with_table(&doc, &commands_table(&fresh)).expect("marqueurs du tableau des commandes dans docs/contrats.md");
        if std::env::var_os("CUBELINK_WRITE_BINDINGS").is_some() {
            std::fs::write(path(), &fresh).expect("écriture des liaisons");
            std::fs::write(contracts_path(), &fresh_doc).expect("écriture de docs/contrats.md");
            return;
        }
        let cur = std::fs::read_to_string(path()).unwrap_or_default().replace("\r\n", "\n");
        assert!(cur == fresh, "src/services/bindings.ts n'est plus à jour : lancer `npm run bindings` et committer le fichier.");
        assert!(doc == fresh_doc, "Le tableau des commandes de docs/contrats.md n'est plus à jour : lancer `npm run bindings`.");
    }

    #[test]
    fn tableau_des_commandes() {
        let ts = "\t/**  Ajoute un mod. */\n\tmodsAdd: (instanceId: string, path: string | null, options?: InvokeOptions) => __TAURI_INVOKE<ModEntry>(\"mods_add\", { instance_id: instanceId, path }, options),\n\tjoinPause: (options?: InvokeOptions) => __TAURI_INVOKE<null>(\"join_pause\", undefined, options),\n";
        let t = commands_table(ts);
        assert!(t.contains("| `mods_add` | `instance_id: string`, `path: string \\| null` | `ModEntry` | Ajoute un mod. |"), "{t}");
        assert!(t.contains("| `join_pause` | — | — |  |"), "{t}");
        assert_eq!(with_table(&format!("a\n{TABLE_START}\nvieux\n{TABLE_END}\nb"), "neuf\n").as_deref(), Some(format!("a\n{TABLE_START}\nneuf\n{TABLE_END}\nb").as_str()));
    }
}

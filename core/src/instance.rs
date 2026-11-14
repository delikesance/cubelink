//! Tâche I1 : création et lecture d'instances sur disque.
//!
//! Layout : `<instances_dir>/<id>/instance.json` (métadonnées) et `<id>/minecraft/` (dossier de jeu).

use std::fs;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::types::{Error, Instance, InstanceArt, LoaderSpec, Result};

const META_FILE: &str = "instance.json";
const META_TMP: &str = "instance.json.tmp";
const GAME_DIR: &str = "minecraft";
const ART_DIR: &str = "art";
const MAX_NAME_CHARS: usize = 64;
const MAX_ID_LEN: usize = 96;
pub const MIN_RAM_MB: u32 = 512;
pub const MAX_RAM_MB: u32 = 65536;
/// RAM d'une instance « Par défaut » : la valeur globale des paramètres, lue au lancement.
pub const RAM_DEFAULT: u32 = 0;

/// RAM réellement donnée au jeu : celle de l'instance, ou la valeur globale si l'instance suit le défaut.
pub fn effective_ram_mb(instance_mb: u32, global_mb: u32) -> u32 {
    if instance_mb == RAM_DEFAULT { global_mb } else { instance_mb }
}

/// Avant de rejoindre un ami : désactive l'écran d'accueil « accessibilité » de Minecraft (1.19.4+).
/// Sur un dossier de jeu neuf (sans `options.txt`), il s'affiche au premier lancement et bloque le
/// `--quickPlayMultiplayer` : le jeu ne se connecte jamais. Ne touche à rien d'autre dans `options.txt`.
pub fn skip_accessibility_onboarding(game_dir: &Path) -> std::io::Result<()> {
    let path = game_dir.join("options.txt");
    let text = match fs::read_to_string(&path) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(e) => return Err(e),
    };
    let mut found = false;
    let mut out: Vec<String> = text
        .lines()
        .map(|l| {
            if l.trim_start().starts_with("onboardAccessibility:") {
                found = true;
                "onboardAccessibility:false".to_string()
            } else {
                l.to_string()
            }
        })
        .collect();
    if !found {
        out.push("onboardAccessibility:false".into());
    }
    let new = out.join("\n") + "\n";
    if new != text {
        fs::create_dir_all(game_dir)?;
        fs::write(&path, new)?;
    }
    Ok(())
}

/// Origine d'une instance : créée par l'utilisateur ou reçue d'un ami.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum InstanceKind {
    #[default]
    Own,
    Guest,
}

/// Forme sérialisée dans `instance.json`.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct StoredInstance {
    #[serde(flatten)]
    instance: Instance,
    /// Secondes depuis l'epoch Unix.
    created_at: i64,
    #[serde(default)]
    kind: InstanceKind,
    /// Créée à la main dans Cubelink (pas importée d'un modpack) : jamais d'illustration automatique.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    custom: bool,
}

/// Dossier ignoré par `list_with_issues`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkippedInstance {
    pub dir_name: String,
    pub reason: String,
}

#[derive(Debug, Clone)]
pub struct InstanceStore {
    root: PathBuf,
}

impl InstanceStore {
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        Self { root: dir.into() }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Crée une instance de type `Own`.
    pub fn create(&self, name: &str, minecraft: &str, loader: LoaderSpec, ram_mb: u32) -> Result<Instance> {
        self.create_with_kind(name, minecraft, loader, ram_mb, InstanceKind::Own)
    }

    pub fn create_with_kind(
        &self,
        name: &str,
        minecraft: &str,
        loader: LoaderSpec,
        ram_mb: u32,
        kind: InstanceKind,
    ) -> Result<Instance> {
        let name = validate_name(name)?;
        check_ram(ram_mb)?;
        if minecraft.trim().is_empty() {
            return Err(Error::Invalid("version de Minecraft vide".into()));
        }
        fs::create_dir_all(&self.root)?;
        let base = slugify(&name);
        let mut n = 1u32;
        let id = loop {
            let candidate = if n == 1 { base.clone() } else { format!("{base}-{n}") };
            match fs::create_dir(self.root.join(&candidate)) {
                Ok(()) => break candidate,
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => n += 1,
                Err(e) => return Err(e.into()),
            }
        };
        let instance = Instance {
            id: id.clone(),
            name,
            minecraft: minecraft.trim().to_string(),
            loader,
            ram_mb,
            mod_count: 0,
            java_path: None,
            jvm_args: String::new(),
            wrapper: String::new(),
            art: None,
        };
        let stored = StoredInstance { instance: instance.clone(), created_at: now_secs(), kind, custom: false };
        let res = fs::create_dir_all(self.root.join(&id).join(GAME_DIR))
            .map_err(Error::from)
            .and_then(|_| self.write_meta(&id, &stored));
        if let Err(e) = res {
            let _ = fs::remove_dir_all(self.root.join(&id));
            return Err(e);
        }
        Ok(instance)
    }

    /// Liste les instances valides. Les dossiers illisibles sont ignorés et journalisés.
    pub fn list(&self) -> Vec<Instance> {
        self.list_with_issues().0
    }

    /// Comme `list`, avec en plus les dossiers ignorés et la raison.
    pub fn list_with_issues(&self) -> (Vec<Instance>, Vec<SkippedInstance>) {
        let mut ok = Vec::new();
        let mut skipped = Vec::new();
        let Ok(entries) = fs::read_dir(&self.root) else {
            return (ok, skipped);
        };
        for entry in entries.flatten() {
            if !entry.file_type().map(|t| t.is_dir()).unwrap_or(false) {
                continue;
            }
            let dir_name = entry.file_name().to_string_lossy().into_owned();
            match self.get(&dir_name) {
                Ok(i) => ok.push(i),
                Err(e) => {
                    tracing::warn!("instance ignorée ({dir_name}) : {e}");
                    skipped.push(SkippedInstance { dir_name, reason: e.to_string() });
                }
            }
        }
        ok.sort_by(|a, b| a.id.cmp(&b.id));
        skipped.sort_by(|a, b| a.dir_name.cmp(&b.dir_name));
        (ok, skipped)
    }

    pub fn get(&self, id: &str) -> Result<Instance> {
        let stored = self.read_meta(id)?;
        Ok(self.with_live_mod_count(id, stored.instance))
    }

    pub fn kind(&self, id: &str) -> Result<InstanceKind> {
        Ok(self.read_meta(id)?.kind)
    }

    /// Instance créée à la main (voir [`Self::mark_custom`]).
    pub fn is_custom(&self, id: &str) -> bool {
        self.read_meta(id).is_ok_and(|m| m.custom)
    }

    /// Marque l'instance comme créée à la main : la recherche automatique d'illustration l'ignore.
    pub fn mark_custom(&self, id: &str) -> Result<()> {
        let mut m = self.read_meta(id)?;
        m.custom = true;
        self.write_meta(id, &m)
    }

    pub fn created_at(&self, id: &str) -> Result<i64> {
        Ok(self.read_meta(id)?.created_at)
    }

    pub fn delete(&self, id: &str) -> Result<()> {
        validate_id(id)?;
        let path = self.root.join(id);
        let meta = fs::symlink_metadata(&path).map_err(|_| Error::NotFound(format!("instance {id}")))?;
        if meta.file_type().is_symlink() || !meta.is_dir() {
            return Err(Error::Invalid(format!("{id} n'est pas un dossier d'instance")));
        }
        let root = fs::canonicalize(&self.root)?;
        let real = fs::canonicalize(&path)?;
        if real.parent() != Some(root.as_path()) {
            return Err(Error::Invalid(format!("{id} hors de la racine des instances")));
        }
        fs::remove_dir_all(&real)?;
        Ok(())
    }

    pub fn update_ram(&self, id: &str, mb: u32) -> Result<Instance> {
        check_ram(mb)?;
        let mut stored = self.read_meta(id)?;
        stored.instance.ram_mb = mb;
        self.write_meta(id, &stored)?;
        Ok(self.with_live_mod_count(id, stored.instance))
    }

    /// Met à jour nom, RAM, Java, arguments JVM et commande d'enrobage. `None` : champ inchangé ; pour `java_path`,
    /// `jvm_args` et `wrapper`, une chaîne vide efface la valeur. L'id (donc le dossier) ne change jamais.
    pub fn update(
        &self,
        id: &str,
        name: Option<&str>,
        ram_mb: Option<u32>,
        java_path: Option<&str>,
        jvm_args: Option<&str>,
        wrapper: Option<&str>,
    ) -> Result<Instance> {
        let mut stored = self.read_meta(id)?;
        // Tout est validé avant la moindre écriture.
        let name = name.map(validate_name).transpose()?;
        if let Some(mb) = ram_mb {
            check_ram(mb)?;
        }
        let java_path = java_path.map(validate_java_path).transpose()?;
        let jvm_args = jvm_args.map(validate_jvm_args).transpose()?;
        let wrapper = wrapper.map(validate_wrapper).transpose()?;
        if let Some(n) = name {
            stored.instance.name = n;
        }
        if let Some(mb) = ram_mb {
            stored.instance.ram_mb = mb;
        }
        if let Some(j) = java_path {
            stored.instance.java_path = (!j.is_empty()).then_some(j);
        }
        if let Some(a) = jvm_args {
            stored.instance.jvm_args = a;
        }
        if let Some(w) = wrapper {
            stored.instance.wrapper = w;
        }
        self.write_meta(id, &stored)?;
        Ok(self.with_live_mod_count(id, stored.instance))
    }

    /// Dossier de jeu (`<id>/minecraft`). L'id est validé ; le dossier peut ne pas exister.
    pub fn game_dir(&self, id: &str) -> Result<PathBuf> {
        validate_id(id)?;
        Ok(self.root.join(id).join(GAME_DIR))
    }

    /// Dossier des illustrations (`<id>/art`), à côté de `instance.json`. Le dossier peut ne pas exister.
    pub fn art_dir(&self, id: &str) -> Result<PathBuf> {
        validate_id(id)?;
        Ok(self.root.join(id).join(ART_DIR))
    }

    /// Remplace (ou efface) les métadonnées d'illustration. Les fichiers sont gérés par l'appelant.
    pub fn set_art(&self, id: &str, art: Option<InstanceArt>) -> Result<Instance> {
        let mut stored = self.read_meta(id)?;
        stored.instance.art = art;
        self.write_meta(id, &stored)?;
        Ok(self.with_live_mod_count(id, stored.instance))
    }

    /// Nombre de fichiers `.jar` dans `mods/`.
    pub fn mod_count(&self, id: &str) -> Result<u32> {
        let mods = self.game_dir(id)?.join("mods");
        Ok(count_jars(&mods))
    }

    fn with_live_mod_count(&self, id: &str, mut instance: Instance) -> Instance {
        instance.id = id.to_string();
        instance.mod_count = count_jars(&self.root.join(id).join(GAME_DIR).join("mods"));
        instance
    }

    fn read_meta(&self, id: &str) -> Result<StoredInstance> {
        validate_id(id)?;
        let path = self.root.join(id).join(META_FILE);
        let bytes = match fs::read(&path) {
            Ok(b) => b,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                return Err(Error::NotFound(format!("instance {id}")))
            }
            Err(e) => return Err(e.into()),
        };
        serde_json::from_slice(&bytes).map_err(|e| Error::Invalid(format!("instance.json de {id} corrompu : {e}")))
    }

    /// Écriture atomique : fichier temporaire, fsync, puis renommage.
    fn write_meta(&self, id: &str, stored: &StoredInstance) -> Result<()> {
        let dir = self.root.join(id);
        let tmp = dir.join(META_TMP);
        let data = serde_json::to_vec_pretty(stored).map_err(|e| Error::Other(e.to_string()))?;
        let res = (|| -> std::io::Result<()> {
            let mut f = fs::File::create(&tmp)?;
            f.write_all(&data)?;
            f.sync_all()?;
            fs::rename(&tmp, dir.join(META_FILE))
        })();
        if let Err(e) = res {
            let _ = fs::remove_file(&tmp);
            return Err(e.into());
        }
        Ok(())
    }
}

fn now_secs() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0)
}

fn check_ram(mb: u32) -> Result<()> {
    if mb == RAM_DEFAULT || (MIN_RAM_MB..=MAX_RAM_MB).contains(&mb) {
        Ok(())
    } else {
        Err(Error::Invalid(format!("RAM hors bornes ({MIN_RAM_MB}-{MAX_RAM_MB} Mo) : {mb}")))
    }
}

const MAX_JVM_ARGS_CHARS: usize = 2048;
const MAX_JAVA_PATH_CHARS: usize = 512;

fn validate_java_path(p: &str) -> Result<String> {
    let p = p.trim();
    if p.chars().count() > MAX_JAVA_PATH_CHARS || p.chars().any(|c| c.is_control()) {
        return Err(Error::Invalid("chemin Java invalide".into()));
    }
    Ok(p.to_string())
}

fn validate_jvm_args(a: &str) -> Result<String> {
    let a = a.trim();
    if a.chars().count() > MAX_JVM_ARGS_CHARS {
        return Err(Error::Invalid(format!("arguments JVM trop longs (max {MAX_JVM_ARGS_CHARS} caractères)")));
    }
    if a.chars().any(|c| c.is_control()) {
        return Err(Error::Invalid("arguments JVM : caractères de contrôle interdits".into()));
    }
    Ok(a.to_string())
}

fn validate_wrapper(w: &str) -> Result<String> {
    let w = w.trim();
    if w.chars().count() > MAX_JVM_ARGS_CHARS {
        return Err(Error::Invalid(format!("commande d'enrobage trop longue (max {MAX_JVM_ARGS_CHARS} caractères)")));
    }
    if w.chars().any(|c| c.is_control()) {
        return Err(Error::Invalid("commande d'enrobage : caractères de contrôle interdits".into()));
    }
    Ok(w.to_string())
}

/// Découpe la chaîne d'arguments JVM d'une instance (séparateur : espaces).
pub fn split_jvm_args(args: &str) -> Vec<String> {
    args.split_whitespace().map(str::to_string).collect()
}

fn count_jars(mods: &Path) -> u32 {
    let Ok(entries) = fs::read_dir(mods) else { return 0 };
    entries
        .flatten()
        .filter(|e| {
            e.path().extension().is_some_and(|x| x.eq_ignore_ascii_case("jar"))
                && e.file_type().map(|t| t.is_file()).unwrap_or(false)
        })
        .count() as u32
}

fn is_reserved(stem: &str) -> bool {
    let s = stem.trim_end().to_ascii_uppercase();
    matches!(s.as_str(), "CON" | "PRN" | "AUX" | "NUL")
        || ["COM", "LPT"].iter().any(|p| {
            s.strip_prefix(p).is_some_and(|d| d.len() == 1 && matches!(d.as_bytes()[0], b'1'..=b'9'))
        })
}

/// Valide et normalise (trim) un nom d'instance.
fn validate_name(name: &str) -> Result<String> {
    let name = name.trim();
    if name.is_empty() {
        return Err(Error::Invalid("nom vide".into()));
    }
    if name.chars().count() > MAX_NAME_CHARS {
        return Err(Error::Invalid(format!("nom trop long (max {MAX_NAME_CHARS} caractères)")));
    }
    if let Some(c) = name.chars().find(|c| c.is_control() || "<>:\"/\\|?*".contains(*c)) {
        return Err(Error::Invalid(format!("caractère interdit dans le nom : {c:?}")));
    }
    if name.ends_with('.') {
        return Err(Error::Invalid("le nom ne peut pas finir par un point".into()));
    }
    let stem = name.split('.').next().unwrap_or(name);
    if is_reserved(stem) {
        return Err(Error::Invalid(format!("nom réservé : {stem}")));
    }
    Ok(name.to_string())
}

/// Slug ASCII minuscule ; "instance" si rien n'est exploitable.
fn slugify(name: &str) -> String {
    let mut out = String::new();
    let mut dash = true;
    for c in name.chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c.to_ascii_lowercase());
            dash = false;
        } else if !dash {
            out.push('-');
            dash = true;
        }
    }
    let out = out.trim_end_matches('-').to_string();
    if out.is_empty() || is_reserved(&out) {
        "instance".to_string()
    } else {
        out
    }
}

/// Refuse tout id pouvant sortir de la racine : `..`, séparateurs, chemin absolu, caractères exotiques.
fn validate_id(id: &str) -> Result<()> {
    let ok = !id.is_empty()
        && id.len() <= MAX_ID_LEN
        && id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_');
    if ok {
        Ok(())
    } else {
        Err(Error::Invalid(format!("id d'instance invalide : {id:?}")))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::Loader;

    fn spec() -> LoaderSpec {
        LoaderSpec { loader: Loader::NeoForge, version: "21.1.77".into() }
    }

    #[test]
    fn accueil_accessibilite_desactive_avant_de_rejoindre() {
        let d = tempfile::tempdir().unwrap();
        let gd = d.path().join("minecraft");
        // Dossier neuf : le fichier est créé avec la seule option.
        skip_accessibility_onboarding(&gd).unwrap();
        assert_eq!(fs::read_to_string(gd.join("options.txt")).unwrap(), "onboardAccessibility:false\n");
        // Réglages existants : seule la ligne d'accueil change, le reste est gardé.
        fs::write(gd.join("options.txt"), "fov:90\nonboardAccessibility:true\nlang:fr_fr\n").unwrap();
        skip_accessibility_onboarding(&gd).unwrap();
        assert_eq!(fs::read_to_string(gd.join("options.txt")).unwrap(), "fov:90\nonboardAccessibility:false\nlang:fr_fr\n");
        fs::write(gd.join("options.txt"), "fov:90\n").unwrap();
        skip_accessibility_onboarding(&gd).unwrap();
        assert_eq!(fs::read_to_string(gd.join("options.txt")).unwrap(), "fov:90\nonboardAccessibility:false\n");
    }

    fn store() -> (tempfile::TempDir, InstanceStore) {
        let t = tempfile::tempdir().unwrap();
        let s = InstanceStore::new(t.path().join("instances"));
        (t, s)
    }

    #[test]
    fn roundtrip() {
        let (_t, s) = store();
        let i = s.create("Create: Aeronautics".replace(':', "").as_str(), "1.21.1", spec(), 4096).unwrap();
        assert_eq!(i.id, "create-aeronautics");
        assert_eq!(s.get(&i.id).unwrap(), i);
        assert_eq!(s.list(), vec![i.clone()]);
        assert!(s.game_dir(&i.id).unwrap().is_dir());
        assert_eq!(s.kind(&i.id).unwrap(), InstanceKind::Own);
        assert!(s.created_at(&i.id).unwrap() > 0);
        let g = s.create_with_kind("Copain", "1.21.1", spec(), 2048, InstanceKind::Guest).unwrap();
        assert_eq!(s.kind(&g.id).unwrap(), InstanceKind::Guest);
    }

    #[test]
    fn unique_ids() {
        let (_t, s) = store();
        let a = s.create("Mon Pack", "1.21.1", spec(), 2048).unwrap();
        let b = s.create("mon pack", "1.21.1", spec(), 2048).unwrap();
        let c = s.create("Mon  Pack!", "1.21.1", spec(), 2048).unwrap();
        assert_eq!((a.id.as_str(), b.id.as_str(), c.id.as_str()), ("mon-pack", "mon-pack-2", "mon-pack-3"));
    }

    #[test]
    fn invalid_names() {
        let (_t, s) = store();
        let long = "a".repeat(65);
        for bad in ["", "   ", long.as_str(), "a/b", "a\\b", "a:b", "a*b", "a?b", "a<b", "a|b", "a\"b", "a\u{0}b", "nom.", "CON", "nul", "Com1", "lpt9", "con.txt"] {
            assert!(matches!(s.create(bad, "1.21.1", spec(), 2048), Err(Error::Invalid(_))), "{bad:?}");
        }
        assert!(s.create(&"a".repeat(64), "1.21.1", spec(), 2048).is_ok());
        assert!(s.create("Console", "1.21.1", spec(), 2048).is_ok());
        assert!(s.create("日本語", "1.21.1", spec(), 2048).unwrap().id == "instance");
        assert!(s.create("ok", "1.21.1", spec(), 100).is_err());
        assert!(s.list().len() == 3);
    }

    #[test]
    fn list_skips_corrupted() {
        let (_t, s) = store();
        let good = s.create("Bon", "1.21.1", spec(), 2048).unwrap();
        fs::create_dir_all(s.root().join("vide")).unwrap();
        fs::create_dir_all(s.root().join("casse")).unwrap();
        fs::write(s.root().join("casse").join(META_FILE), b"{ pas du json").unwrap();
        fs::write(s.root().join("fichier.txt"), b"x").unwrap();
        let (ok, skipped) = s.list_with_issues();
        assert_eq!(ok, vec![good.clone()]);
        let names: Vec<_> = skipped.iter().map(|k| k.dir_name.as_str()).collect();
        assert_eq!(names, vec!["casse", "vide"]);
        assert_eq!(s.list(), vec![good]);
    }

    #[test]
    fn delete_rules() {
        let (t, s) = store();
        let i = s.create("A supprimer", "1.21.1", spec(), 2048).unwrap();
        let outside = t.path().join("dehors");
        fs::create_dir_all(&outside).unwrap();
        for bad in ["..", "../dehors", "a/b", "a\\b", "", ".", outside.to_str().unwrap()] {
            assert!(matches!(s.delete(bad), Err(Error::Invalid(_))), "{bad:?}");
        }
        assert!(outside.exists());
        assert!(matches!(s.delete("inconnu"), Err(Error::NotFound(_))));
        s.delete(&i.id).unwrap();
        assert!(s.list().is_empty());
        assert!(!s.root().join(&i.id).exists());
        assert!(s.game_dir("../x").is_err());
    }

    #[test]
    fn mod_count_is_live() {
        let (_t, s) = store();
        let i = s.create("Mods", "1.21.1", spec(), 2048).unwrap();
        assert_eq!(s.mod_count(&i.id).unwrap(), 0);
        let mods = s.game_dir(&i.id).unwrap().join("mods");
        fs::create_dir_all(&mods).unwrap();
        for f in ["a.jar", "b.JAR", "c.txt", "d.jar.disabled"] {
            fs::write(mods.join(f), b"x").unwrap();
        }
        fs::create_dir(mods.join("sous.jar")).unwrap();
        assert_eq!(s.mod_count(&i.id).unwrap(), 2);
        assert_eq!(s.get(&i.id).unwrap().mod_count, 2);
        assert_eq!(s.list()[0].mod_count, 2);
    }

    #[test]
    fn update_ram_bounds() {
        let (_t, s) = store();
        let i = s.create("Ram", "1.21.1", spec(), 2048).unwrap();
        assert_eq!(s.update_ram(&i.id, 8192).unwrap().ram_mb, 8192);
        assert_eq!(s.get(&i.id).unwrap().ram_mb, 8192);
        assert!(s.update_ram(&i.id, 511).is_err());
        assert!(s.update_ram(&i.id, 65537).is_err());
        assert!(s.update_ram(&i.id, 512).is_ok());
        assert!(s.update_ram(&i.id, 65536).is_ok());
        assert!(matches!(s.update_ram("nope", 1024), Err(Error::NotFound(_))));
        assert_eq!(s.get(&i.id).unwrap().ram_mb, 65536);
    }

    #[test]
    fn ram_par_defaut_suit_la_valeur_globale() {
        let d = tempfile::tempdir().unwrap();
        let s = InstanceStore::new(d.path().to_path_buf());
        let i = s.create("Défaut", "1.21.1", LoaderSpec { loader: Loader::Vanilla, version: String::new() }, RAM_DEFAULT).unwrap();
        assert_eq!(i.ram_mb, RAM_DEFAULT);
        assert_eq!(effective_ram_mb(i.ram_mb, 10240), 10240);
        // Valeur propre à l'instance, puis retour au défaut.
        assert_eq!(effective_ram_mb(s.update_ram(&i.id, 6144).unwrap().ram_mb, 10240), 6144);
        assert_eq!(s.update(&i.id, None, Some(RAM_DEFAULT), None, None, None).unwrap().ram_mb, RAM_DEFAULT);
        assert!(s.update_ram(&i.id, 100).is_err());
    }

    #[test]
    fn update_fields() {
        let (_t, s) = store();
        let i = s.create("Avant", "1.21.1", spec(), 2048).unwrap();
        let u = s.update(&i.id, Some("  Après "), Some(4096), Some(" /opt/java/bin/java "), Some(" -Xss1M  -Dfoo=bar "), None).unwrap();
        assert_eq!((u.name.as_str(), u.ram_mb, u.id.as_str()), ("Après", 4096, i.id.as_str()));
        assert_eq!(u.java_path.as_deref(), Some("/opt/java/bin/java"));
        assert_eq!(split_jvm_args(&u.jvm_args), vec!["-Xss1M", "-Dfoo=bar"]);
        assert_eq!(s.get(&i.id).unwrap(), u);
        // Champs absents : inchangés ; chaîne vide : effacé.
        let u2 = s.update(&i.id, None, None, Some(""), None, None).unwrap();
        let w = s.update(&i.id, None, None, None, None, Some("  gamemoderun  mangohud ")).unwrap();
        assert_eq!(w.wrapper, "gamemoderun  mangohud");
        assert_eq!(s.update(&i.id, None, None, None, None, Some("")).unwrap().wrapper, "");
        assert_eq!(u2.java_path, None);
        assert_eq!(u2.name, "Après");
        assert_eq!(u2.jvm_args, "-Xss1M  -Dfoo=bar");
    }

    #[test]
    fn update_rejects_invalid_without_writing() {
        let (_t, s) = store();
        let i = s.create("Stable", "1.21.1", spec(), 2048).unwrap();
        assert!(matches!(s.update(&i.id, Some("a/b"), None, None, None, None), Err(Error::Invalid(_))));
        assert!(matches!(s.update(&i.id, Some(""), None, None, None, None), Err(Error::Invalid(_))));
        // Nom valide mais RAM invalide : rien ne doit être écrit.
        assert!(s.update(&i.id, Some("Autre"), Some(100), None, None, None).is_err());
        assert!(s.update(&i.id, None, None, None, Some("-Da
b"), None).is_err());
        assert!(s.update(&i.id, None, None, None, None, Some("prime-run\u{7}")).is_err());
        assert!(s.update(&i.id, None, None, None, Some(&"x".repeat(3000)), None).is_err());
        assert!(s.update(&i.id, None, None, Some("a b"), None, None).is_err());
        assert_eq!(s.get(&i.id).unwrap(), i);
        assert!(matches!(s.update("nope", Some("x"), None, None, None, None), Err(Error::NotFound(_))));
    }

    #[test]
    fn legacy_instance_json_still_loads() {
        let (_t, s) = store();
        let dir = s.root().join("vieux");
        fs::create_dir_all(dir.join(GAME_DIR)).unwrap();
        fs::write(
            dir.join(META_FILE),
            br#"{"id":"vieux","name":"Vieux","minecraft":"1.20.1","loader":{"loader":"fabric","version":"0.15"},"ram_mb":3072,"mod_count":0,"created_at":5}"#,
        )
        .unwrap();
        let i = s.get("vieux").unwrap();
        assert_eq!((i.java_path, i.jvm_args.as_str(), i.ram_mb), (None, "", 3072));
        assert_eq!(i.art, None);
        assert_eq!(s.update("vieux", None, None, None, Some("-Xmx1G"), None).unwrap().jvm_args, "-Xmx1G");
    }

    #[test]
    fn art_roundtrip_and_delete_removes_art_dir() {
        let (_t, s) = store();
        let i = s.create("Art", "1.21.1", spec(), 2048).unwrap();
        assert_eq!(i.art, None);
        let art = InstanceArt {
            source: "modrinth".into(),
            project_id: "P".into(),
            title: "T".into(),
            author: "a".into(),
            summary: "s".into(),
            color: Some("#aabbcc".into()),
            icon: Some("icon.png".into()),
            screenshots: vec!["shot-1.png".into()],
            page_url: None,
        };
        let u = s.set_art(&i.id, Some(art.clone())).unwrap();
        assert_eq!(u.art, Some(art.clone()));
        assert_eq!(s.get(&i.id).unwrap().art, Some(art));
        let dir = s.art_dir(&i.id).unwrap();
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("icon.png"), b"x").unwrap();
        s.delete(&i.id).unwrap();
        assert!(!dir.exists());
        assert!(s.art_dir("../x").is_err());
    }

    #[test]
    fn atomic_write() {
        let (_t, s) = store();
        let i = s.create("Atomique", "1.21.1", spec(), 2048).unwrap();
        let dir = s.root().join(&i.id);
        // Un tmp périmé (crash précédent) ne perturbe ni la lecture ni l'écriture suivante.
        fs::write(dir.join(META_TMP), b"garbage").unwrap();
        assert_eq!(s.get(&i.id).unwrap(), i);
        s.update_ram(&i.id, 4096).unwrap();
        assert!(!dir.join(META_TMP).exists());
        let v: serde_json::Value = serde_json::from_slice(&fs::read(dir.join(META_FILE)).unwrap()).unwrap();
        assert_eq!(v["ram_mb"], 4096);
        assert_eq!(v["kind"], "own");
        assert!(v["created_at"].as_i64().unwrap() > 0);
        assert_eq!(v["loader"]["loader"], "neoforge");
    }
}

/// Arguments JVM par défaut hors Windows (Linux, macOS) : tas fixe et G1 réglé pour des pauses courtes, afin d'éviter les
/// saccades dues à la croissance du tas. Ils précèdent `jvm_args` de l'instance (`user`). La JVM refuse de démarrer si un
/// `-Xms` dépasse le `-Xmx` final ou si deux ramasse-miettes sont choisis : le tas fixe est donc omis quand l'utilisateur
/// règle lui-même `-Xms`/`-Xmx`, et les réglages G1 quand il choisit un ramasse-miettes. Windows : aucun (inchangé).
pub fn default_jvm_args(windows: bool, ram_mb: u32, user: &[String]) -> Vec<String> {
    if windows {
        return Vec::new();
    }
    let user_heap = user.iter().any(|a| a.starts_with("-Xms") || a.starts_with("-Xmx"));
    let user_gc = user.iter().any(|a| a.starts_with("-XX:+Use") && a.ends_with("GC"));
    let mut a = Vec::new();
    if !user_heap {
        a.push(format!("-Xms{ram_mb}M"));
    }
    if !user_gc {
        a.extend(["-XX:+UseG1GC", "-XX:+ParallelRefProcEnabled", "-XX:MaxGCPauseMillis=130"].map(String::from));
    }
    a.push("-XX:+DisableExplicitGC".into());
    a
}

#[cfg(test)]
mod default_jvm_args_tests {
    use super::*;

    #[test]
    fn windows_untouched_and_linux_fixed_heap() {
        assert!(default_jvm_args(true, 4096, &[]).is_empty());
        let l = default_jvm_args(false, 4096, &[]);
        assert_eq!(l[0], "-Xms4096M");
        assert!(l.contains(&"-XX:+UseG1GC".to_string()));
    }

    #[test]
    fn user_heap_and_gc_win() {
        // `-Xms4096M ... -Xmx2G` ferait refuser le démarrage à la JVM ; idem pour deux ramasse-miettes.
        let l = default_jvm_args(false, 4096, &split_jvm_args("-Xmx2G -XX:+UseZGC"));
        assert!(!l.iter().any(|a| a.starts_with("-Xms")));
        assert!(!l.iter().any(|a| a.contains("G1") || a.contains("MaxGCPause")));
        let l = default_jvm_args(false, 4096, &split_jvm_args("-XX:+UseG1GC"));
        assert!(l.contains(&"-Xms4096M".to_string()));
        assert!(!l.contains(&"-XX:+UseG1GC".to_string()));
    }
}

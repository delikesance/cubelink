//! Correction automatique des problèmes de mods (Modrinth + CurseForge) : prévisualiser, appliquer, annuler.
//!
//! - `plan` n'écrit rien dans l'instance : il télécharge les candidats dans le magasin partagé et ne garde une
//!   action que si les métadonnées du jar téléchargé règlent réellement le problème.
//! - `apply` place les fichiers ; tout jar remplacé est déplacé dans `mods/.cubelink-backup/<id>/` avec un
//!   manifeste, et chaque activation / désactivation y est consignée.
//! - `undo` relit le manifeste et remet l'instance dans son état d'origine.
//!
//! Règle d'or : un pack de plusieurs centaines de mods ne doit jamais être cassé ; dans le doute, rien n'est
//! proposé (le problème passe dans `unresolved`).

use std::collections::{BTreeMap, HashMap, HashSet};
use std::future::Future;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::Arc;

use serde::{Deserialize, Serialize};

use crate::browse::{self, BrowseKind, BrowseSource, Planned, ProgressCb, Providers};
use crate::mod_check::{self, DepDetail, Index, ModCheck, ModIssue};
use crate::mod_meta::{jar_meta, DepKind, JarMeta};
use crate::mods::{self, Provenance};
use crate::providers::curseforge_api::{self as cfapi, CfFile};
use crate::providers::modrinth_api::Version;
use crate::providers::modrinth_dl::{DlOptions, StoredObject};
use crate::store::index::StoreIndex;
use crate::store::link::link_into;
use crate::types::{Error, Loader, Result};
use crate::version_range::{self as vr, Flavor};

pub const BACKUP_DIR: &str = ".cubelink-backup";
/// Sauvegardes conservées par instance.
const KEEP_BACKUPS: usize = 5;
/// Versions candidates téléchargées au plus par projet.
const MAX_TRIES: usize = 3;
/// Versions essayées quand on cherche une version d'un mod qui accepte une dépendance déjà installée.
const MAX_REALIGN_TRIES: usize = 10;
/// Profondeur maximale des dépendances de dépendances.
const MAX_DEPTH: u32 = 3;
/// Nombre maximal d'actions « installer » dans un plan.
const MAX_INSTALLS: usize = 40;
/// Projets de dépendances regardés au plus pour retrouver un mod par son identifiant.
const MAX_HINTS: usize = 8;

// ---------- Contrat (sérialisé vers l'interface) ----------

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, specta::Type)]
pub struct FixAction {
    pub id: String,
    /// "install" | "update" | "enable" | "disable"
    pub kind: String,
    pub title: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    /// "modrinth" | "curseforge"
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    /// Jar existant concerné (mis à jour / désactivé / réactivé).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub file: Option<String>,
    /// `ModIssue.file` des problèmes que l'action règle.
    pub issue_files: Vec<String>,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, specta::Type)]
pub struct FixUnresolved {
    pub file: String,
    pub mod_name: String,
    pub message: String,
    pub reason: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub page_url: Option<String>,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, specta::Type)]
pub struct FixPlan {
    pub plan_id: String,
    pub actions: Vec<FixAction>,
    pub unresolved: Vec<FixUnresolved>,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, specta::Type)]
pub struct FixFailure {
    pub title: String,
    pub error: String,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, specta::Type)]
pub struct FixResult {
    pub applied: Vec<String>,
    pub failed: Vec<FixFailure>,
    pub backup_id: Option<String>,
    pub check: ModCheck,
}

// ---------- Plan préparé (gardé en mémoire par l'application) ----------

#[derive(Clone)]
enum Op {
    /// Nom activé du jar.
    Enable(String),
    Disable(String),
    Install { pl: Planned, obj: StoredObject, replace: Vec<String> },
}

#[derive(Clone)]
struct Step {
    action: FixAction,
    op: Op,
}

/// Plan calculé : la partie visible (`plan`) et les objets déjà téléchargés dans le magasin.
#[derive(Clone)]
pub struct PreparedFix {
    pub plan: FixPlan,
    /// Empreinte du dossier `mods` au moment du plan (voir [`mods_fingerprint`]).
    pub fingerprint: String,
    steps: Vec<Step>,
}

/// Empreinte du dossier `mods` (noms, tailles, dates des jars) : un plan n'est valable que tant qu'elle ne change pas.
pub fn mods_fingerprint(game_dir: &Path) -> String {
    use std::hash::{Hash, Hasher};
    let mut items: Vec<(String, u64, u128)> = Vec::new();
    if let Ok(rd) = std::fs::read_dir(game_dir.join("mods")) {
        for e in rd.flatten() {
            let Ok(name) = e.file_name().into_string() else { continue };
            let Ok(m) = e.metadata() else { continue };
            if !m.is_file() || name.starts_with('.') {
                continue;
            }
            let ns = m.modified().ok().and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map_or(0, |d| d.as_nanos());
            items.push((name, m.len(), ns));
        }
    }
    items.sort();
    let mut h = std::collections::hash_map::DefaultHasher::new();
    items.hash(&mut h);
    format!("{}-{:x}", items.len(), h.finish())
}

// ---------- Outils ----------

fn invalid(m: impl Into<String>) -> Error {
    Error::Invalid(m.into())
}

fn loader_str(l: Loader) -> &'static str {
    match l {
        Loader::Vanilla => "vanilla",
        Loader::NeoForge => "neoforge",
        Loader::Fabric => "fabric",
        Loader::Quilt => "quilt",
        Loader::Forge => "forge",
    }
}

fn source_label(s: BrowseSource) -> &'static str {
    match s {
        BrowseSource::Modrinth => "Modrinth",
        BrowseSource::Curseforge => "CurseForge",
    }
}

fn norm_slug(s: &str) -> String {
    s.trim().to_ascii_lowercase().replace('_', "-")
}

fn base_name(file: &str) -> &str {
    file.strip_suffix(".disabled").unwrap_or(file)
}

/// Mod principal d'un jar pour ce loader : (id, nom, version).
fn main_mod(meta: &JarMeta, loader: &str) -> Option<(String, String, Option<String>)> {
    let sel = mod_check::selected(meta, loader);
    let m = sel.first().copied().or_else(|| meta.mods.first())?;
    Some((m.id.clone(), m.name.clone().unwrap_or_else(|| m.id.clone()), m.version.clone()))
}

fn clean_version(v: Option<&str>) -> Option<String> {
    v.map(str::trim).filter(|v| !v.is_empty() && !v.contains('$')).map(str::to_string)
}

fn join_names(names: &[String]) -> String {
    let mut seen = Vec::new();
    for n in names {
        if !seen.contains(n) {
            seen.push(n.clone());
        }
    }
    let shown: Vec<String> = seen.iter().take(3).cloned().collect();
    let mut s = shown.join(", ");
    if seen.len() > 3 {
        s.push_str(&format!(" et {} autres", seen.len() - 3));
    }
    s
}

/// Besoins d'un remplacement : chaque id doit être fourni par le jar, avec une version dans toutes les plages.
#[derive(Clone, Default)]
struct Need {
    reqs: Vec<(String, Vec<String>, Flavor)>,
    /// Le candidat doit accepter ces dépendances telles qu'installées : `Some(v)` = sa plage doit contenir `v`,
    /// `None` = il ne doit pas l'exiger du tout.
    accept: Vec<(String, Option<String>)>,
    /// Jar que le candidat remplacerait (pour la simulation dans le pack).
    replace: Option<String>,
}

impl Need {
    fn reqs(reqs: Vec<(String, Vec<String>, Flavor)>) -> Self {
        Need { reqs, ..Default::default() }
    }
    fn tries(&self) -> usize {
        if self.accept.is_empty() { MAX_TRIES } else { MAX_REALIGN_TRIES }
    }
}

fn version_ok(ranges: &[String], flavor: Flavor, version: Option<&str>) -> bool {
    match version {
        // Version inconnue : indécidable, on ne bloque pas.
        None => true,
        Some(v) => ranges.iter().all(|r| vr::matches(r, v, flavor) != Some(false)),
    }
}

/// Versions sous lesquelles `meta` fournit `id` (mod, `provides`, jars imbriqués).
fn provided_versions(meta: &JarMeta, loader: &str, id: &str) -> Vec<Option<String>> {
    let mut out = Vec::new();
    for m in mod_check::selected(meta, loader) {
        if m.id == id {
            out.push(m.version.clone());
        }
        for (p, pv) in &m.provides {
            if p == id {
                out.push(pv.clone().or_else(|| m.version.clone()));
            }
        }
    }
    for (n, v) in &meta.nested {
        if n == id {
            out.push(v.clone());
        }
    }
    out
}

fn meta_satisfies(meta: &JarMeta, loader: &str, need: &Need) -> bool {
    need.reqs.iter().all(|(id, ranges, flavor)| provided_versions(meta, loader, id).iter().any(|v| version_ok(ranges, *flavor, v.as_deref())))
        && meta_accepts(meta, loader, &need.accept)
}

/// Les dépendances obligatoires déclarées par le jar acceptent les versions données (`None` : ne pas l'exiger).
fn meta_accepts(meta: &JarMeta, loader: &str, accept: &[(String, Option<String>)]) -> bool {
    accept.iter().all(|(id, have)| {
        mod_check::selected(meta, loader).iter().all(|m| {
            m.deps.iter().filter(|d| d.kind == DepKind::Required && d.id == *id).all(|d| match have {
                None => false,
                Some(v) => version_ok(std::slice::from_ref(&d.range), m.flavor(), Some(v)),
            })
        })
    })
}

/// Le jar vise bien ce Minecraft et ce loader.
fn compat_ok(meta: &Arc<JarMeta>, mc: &str, loader: &str) -> bool {
    let (issues, _) = mod_check::analyze(&[("candidat.jar".to_string(), meta.clone())], mc, loader);
    !issues.iter().any(|i| i.kind == "wrong_minecraft" || i.kind == "wrong_loader")
}

// ---------- Identification ----------

#[derive(Clone, Default)]
struct Hints {
    pref: Option<BrowseSource>,
    /// (project_id, version_id) de dépendances obligatoires Modrinth.
    mr_deps: Vec<(Option<String>, Option<String>)>,
    cf_deps: Vec<u64>,
}

#[derive(Clone, Default)]
struct Ident {
    mr_project: Option<String>,
    cf_mod: Option<u64>,
    hints: Hints,
}

impl Ident {
    fn preferred(&self) -> BrowseSource {
        self.hints.pref.unwrap_or(BrowseSource::Modrinth)
    }
}

fn mr_hints(v: &Version) -> Hints {
    Hints {
        pref: Some(BrowseSource::Modrinth),
        mr_deps: v.dependencies.iter().filter(|d| d.dependency_type == "required").map(|d| (d.project_id.clone(), d.version_id.clone())).collect(),
        cf_deps: Vec::new(),
    }
}

fn cf_hints(f: &CfFile) -> Hints {
    Hints {
        pref: Some(BrowseSource::Curseforge),
        mr_deps: Vec::new(),
        cf_deps: f.dependencies.iter().filter(|d| d.relation_type == cfapi::REL_REQUIRED).map(|d| d.mod_id).collect(),
    }
}

struct Ctx<'a> {
    p: &'a Providers,
    mc: &'a str,
    loader: Loader,
    loader_s: &'static str,
    game_dir: &'a Path,
    store_root: &'a Path,
    index: &'a StoreIndex,
    dl: &'a DlOptions,
    /// Jars actifs du pack et erreurs de départ (fichier, message) : sert à simuler un candidat dans le pack.
    jars: &'a [(String, Arc<JarMeta>)],
    baseline: HashSet<(String, String)>,
}

impl Ctx<'_> {
    /// Le candidat, placé dans le pack (à la place de `replace`), crée-t-il un problème qui n'existait pas ?
    /// Les dépendances absentes du candidat ne comptent pas : elles sont ajoutées au plan ensuite.
    fn breaks_pack(&self, name: &str, meta: &Arc<JarMeta>, replace: Option<&str>) -> bool {
        !self.new_issues(name, meta, replace.into_iter().collect::<Vec<_>>().as_slice()).is_empty()
    }

    /// Erreurs nouvelles si `meta` (fichier `name`) entre dans le pack à la place de `replace`.
    fn new_issues(&self, name: &str, meta: &Arc<JarMeta>, replace: &[&str]) -> Vec<ModIssue> {
        let mut jars: Vec<(String, Arc<JarMeta>)> = self.jars.iter().filter(|(f, _)| !replace.contains(&f.as_str())).cloned().collect();
        jars.push((name.to_string(), meta.clone()));
        let (issues, details) = mod_check::analyze(&jars, self.mc, self.loader_s);
        issues
            .into_iter()
            .filter(|i| i.severity == "error")
            .filter(|i| {
                if self.baseline.contains(&(i.file.clone(), i.message.clone())) {
                    return false;
                }
                let absent = i.kind == "missing_dep" && details.get(&(i.file.clone(), i.message.clone())).is_some_and(|d| d.provider.is_none());
                !(absent && i.file == name)
            })
            .collect()
    }

    fn mods_dir(&self) -> PathBuf {
        self.game_dir.join("mods")
    }
    fn loader_opt(&self) -> Option<Loader> {
        Some(self.loader).filter(|l| *l != Loader::Vanilla)
    }
}

/// Identifie les jars donnés : provenance, puis Modrinth (sha1, un appel), puis CurseForge (empreinte, un appel).
async fn identify(ctx: &Ctx<'_>, files: &[String]) -> HashMap<String, Ident> {
    let mut out: HashMap<String, Ident> = HashMap::new();
    if files.is_empty() {
        return out;
    }
    let prov = mods::read_provenance(ctx.game_dir);
    let dir = ctx.mods_dir();
    let mut sha_to_file: HashMap<String, String> = HashMap::new();
    for f in files {
        let mut id = Ident::default();
        if let Some(p) = prov.get(base_name(f)) {
            match p.source.as_str() {
                "modrinth" if !p.id.is_empty() => {
                    id.mr_project = Some(p.id.clone());
                    id.hints.pref = Some(BrowseSource::Modrinth);
                }
                "curseforge" => {
                    if let Ok(n) = p.id.parse::<u64>() {
                        id.cf_mod = Some(n);
                        id.hints.pref = Some(BrowseSource::Curseforge);
                    }
                }
                _ => {}
            }
        }
        out.insert(f.clone(), id);
        if let Ok(bytes) = std::fs::read(dir.join(f)) {
            sha_to_file.insert(crate::mojang::versions::sha1_hex(&bytes), f.clone());
        }
    }
    let hashes: Vec<String> = sha_to_file.keys().cloned().collect();
    match ctx.p.modrinth.versions_from_hashes(&hashes).await {
        Ok(map) => {
            for (h, v) in map {
                if let Some(f) = sha_to_file.get(&h.to_ascii_lowercase()).or_else(|| sha_to_file.get(&h)) {
                    let e = out.entry(f.clone()).or_default();
                    e.mr_project = Some(v.project_id.clone());
                    let h2 = mr_hints(&v);
                    e.hints.mr_deps = h2.mr_deps;
                    e.hints.pref = Some(BrowseSource::Modrinth);
                }
            }
        }
        Err(e) => tracing::warn!("identification Modrinth impossible : {e}"),
    }
    // CurseForge : seulement ce que Modrinth ne connaît pas.
    if ctx.p.cf.has_key() {
        let mut fps: HashMap<u32, Vec<String>> = HashMap::new();
        for f in files {
            if out.get(f).is_some_and(|i| i.mr_project.is_some() && !i.hints.mr_deps.is_empty()) {
                continue;
            }
            if let Ok(bytes) = std::fs::read(dir.join(f)) {
                fps.entry(cfapi::murmur2_fingerprint(&bytes)).or_default().push(f.clone());
            }
        }
        let list: Vec<u32> = fps.keys().copied().collect();
        match ctx.p.cf.fingerprints(&list).await {
            Ok(ms) => {
                for m in ms {
                    let fp = m.file.file_fingerprint as u32;
                    for f in fps.get(&fp).into_iter().flatten() {
                        let e = out.entry(f.clone()).or_default();
                        e.cf_mod = Some(m.id);
                        let h = cf_hints(&m.file);
                        e.hints.cf_deps = h.cf_deps;
                        if e.hints.pref.is_none() {
                            e.hints.pref = Some(BrowseSource::Curseforge);
                        }
                    }
                }
            }
            Err(e) => tracing::warn!("identification CurseForge impossible : {e}"),
        }
    }
    out
}

// ---------- Candidats ----------

#[derive(Clone)]
struct Cand {
    pl: Planned,
    version: String,
    hints: Hints,
}

struct Found {
    cand: Cand,
    obj: StoredObject,
    meta: Arc<JarMeta>,
}

enum Outcome {
    Found(Box<Found>),
    /// Fichier non distribuable (page éventuelle).
    Closed(Option<String>),
    Nothing,
}

async fn mr_cands(ctx: &Ctx<'_>, project: &str, limit: usize) -> Vec<Cand> {
    match browse::modrinth_versions(ctx.p, project, BrowseKind::Mod, Some(ctx.mc), ctx.loader_opt()).await {
        Ok(vs) => vs
            .iter()
            .filter_map(|v| browse::plan_modrinth(ctx.p, v, ".jar").ok().map(|pl| Cand { pl, version: v.version_number.clone(), hints: mr_hints(v) }))
            .take(limit)
            .collect(),
        Err(e) => {
            tracing::debug!("versions Modrinth de {project} : {e}");
            Vec::new()
        }
    }
}

/// Candidats CurseForge et présence d'un fichier non distribuable.
async fn cf_cands(ctx: &Ctx<'_>, mod_id: u64, limit: usize) -> (Vec<Cand>, bool) {
    match browse::cf_versions(ctx.p, &mod_id.to_string(), BrowseKind::Mod, Some(ctx.mc), ctx.loader_opt()).await {
        Ok(fs) => {
            let closed = fs.iter().any(|f| f.url().is_none());
            let c = fs
                .iter()
                .filter_map(|f| {
                    browse::plan_cf(ctx.p, f, mod_id, ".jar").ok().map(|pl| Cand {
                        pl,
                        version: if f.display_name.trim().is_empty() { f.file_name.clone() } else { f.display_name.clone() },
                        hints: cf_hints(f),
                    })
                })
                .take(limit)
                .collect();
            (c, closed)
        }
        Err(e) => {
            tracing::debug!("fichiers CurseForge de {mod_id} : {e}");
            (Vec::new(), false)
        }
    }
}

/// Télécharge les candidats dans le magasin (le plus récent d'abord) et garde le premier qui convient.
async fn try_cands(ctx: &Ctx<'_>, cands: Vec<Cand>, need: &Need) -> Option<Box<Found>> {
    for cand in cands {
        if browse::check_url(&cand.pl.url, cand.pl.source, ctx.p.allow_loopback).is_err() || need.replace.as_deref().is_some_and(|r| base_name(r) == cand.pl.name) {
            continue;
        }
        let obj = match browse::fetch(&cand.pl, ctx.store_root, ctx.index, ctx.dl).await {
            Ok(o) => o,
            Err(e) => {
                tracing::warn!("téléchargement de {} : {e}", cand.pl.name);
                continue;
            }
        };
        let Some(meta) = jar_meta(&ctx.store_root.join(&obj.rel_path)) else { continue };
        if !compat_ok(&meta, ctx.mc, ctx.loader_s) || !meta_satisfies(&meta, ctx.loader_s, need) {
            continue;
        }
        if need.reqs.is_empty() && mod_check::selected(&meta, ctx.loader_s).is_empty() {
            continue;
        }
        if ctx.breaks_pack(&cand.pl.name, &meta, need.replace.as_deref()) {
            tracing::debug!("{} écarté : il créerait un nouveau problème dans le pack", cand.pl.name);
            continue;
        }
        return Some(Box::new(Found { cand, obj, meta }));
    }
    None
}

async fn in_project(ctx: &Ctx<'_>, src: BrowseSource, project: &str, need: &Need) -> Outcome {
    match src {
        BrowseSource::Modrinth => {
            if browse::safe_token(project, "projet").is_err() {
                return Outcome::Nothing;
            }
            let c = mr_cands(ctx, project, need.tries()).await;
            try_cands(ctx, c, need).await.map_or(Outcome::Nothing, Outcome::Found)
        }
        BrowseSource::Curseforge => {
            let Ok(id) = project.parse::<u64>() else { return Outcome::Nothing };
            let (c, closed) = cf_cands(ctx, id, need.tries()).await;
            if let Some(f) = try_cands(ctx, c, need).await {
                return Outcome::Found(f);
            }
            if closed {
                let url = ctx.p.cf.mod_info(id).await.ok().and_then(|m| m.links.and_then(|l| l.website_url)).filter(|u| u.starts_with("https://"));
                return Outcome::Closed(url);
            }
            Outcome::Nothing
        }
    }
}

/// Projets de dépendances déclarés : ceux dont le slug ressemble à `slug` d'abord, puis (sans slug trouvé) quelques autres.
async fn by_hints(ctx: &Ctx<'_>, src: BrowseSource, hints: &Hints, slug: &str, need: &Need) -> Outcome {
    let want = norm_slug(slug);
    let mut known: Vec<(String, String)> = Vec::new(); // (identifiant du projet, slug)
    match src {
        BrowseSource::Modrinth => {
            for (pid, vid) in hints.mr_deps.iter().take(MAX_HINTS) {
                let pid = match (pid, vid) {
                    (Some(p), _) => p.clone(),
                    (None, Some(v)) if browse::safe_token(v, "version").is_ok() => match ctx.p.modrinth.version(v).await {
                        Ok(v) => v.project_id,
                        Err(_) => continue,
                    },
                    _ => continue,
                };
                if browse::safe_token(&pid, "projet").is_err() {
                    continue;
                }
                let s = ctx.p.modrinth.project(&pid).await.map(|p| p.slug).unwrap_or_default();
                known.push((pid, s));
            }
        }
        BrowseSource::Curseforge => {
            for id in hints.cf_deps.iter().take(MAX_HINTS) {
                let s = ctx.p.cf.mod_info(*id).await.map(|m| m.slug).unwrap_or_default();
                known.push((id.to_string(), s));
            }
        }
    }
    let (exact, others): (Vec<_>, Vec<_>) = known.into_iter().partition(|(_, s)| norm_slug(s) == want);
    let order: Vec<String> = if exact.is_empty() { others.into_iter().take(4).map(|(p, _)| p).collect() } else { exact.into_iter().map(|(p, _)| p).collect() };
    let mut closed = None;
    for pid in order {
        match in_project(ctx, src, &pid, need).await {
            Outcome::Found(f) => return Outcome::Found(f),
            Outcome::Closed(u) => closed = Some(u),
            Outcome::Nothing => {}
        }
    }
    closed.map_or(Outcome::Nothing, Outcome::Closed)
}

/// Repli : le slug du projet est souvent l'identifiant du mod.
async fn by_slug(ctx: &Ctx<'_>, src: BrowseSource, slug: &str, need: &Need) -> Outcome {
    let want = norm_slug(slug);
    if want.is_empty() {
        return Outcome::Nothing;
    }
    match src {
        BrowseSource::Modrinth => {
            let mut tried = Vec::new();
            for s in [slug.to_string(), want.clone()] {
                if tried.contains(&s) {
                    continue;
                }
                tried.push(s.clone());
                if let Outcome::Found(f) = in_project(ctx, src, &s, need).await {
                    return Outcome::Found(f);
                }
            }
            Outcome::Nothing
        }
        BrowseSource::Curseforge => {
            if !ctx.p.cf.has_key() {
                return Outcome::Nothing;
            }
            let Ok(r) = ctx.p.cf.search(cfapi::CLASS_MOD, slug, Some(ctx.mc), None, cfapi::sort_field(None), 0, 5).await else { return Outcome::Nothing };
            let mut closed = None;
            for hit in r.data.into_iter().filter(|m| norm_slug(&m.slug) == want) {
                match in_project(ctx, src, &hit.id.to_string(), need).await {
                    Outcome::Found(f) => return Outcome::Found(f),
                    Outcome::Closed(u) => closed = Some(u),
                    Outcome::Nothing => {}
                }
            }
            closed.map_or(Outcome::Nothing, Outcome::Closed)
        }
    }
}

fn other(s: BrowseSource) -> BrowseSource {
    match s {
        BrowseSource::Modrinth => BrowseSource::Curseforge,
        BrowseSource::Curseforge => BrowseSource::Modrinth,
    }
}

/// Dépendance absente (ou trop ancienne) : via les dépendances du mod qui la requiert, puis par slug.
async fn resolve_dependency(ctx: &Ctx<'_>, need: &Need, hints: &Hints, slug: &str) -> Outcome {
    let first = hints.pref.unwrap_or(BrowseSource::Modrinth);
    let mut closed = None;
    for src in [first, other(first)] {
        for o in [by_hints(ctx, src, hints, slug, need).await, by_slug(ctx, src, slug, need).await] {
            match o {
                Outcome::Found(f) => return Outcome::Found(f),
                Outcome::Closed(u) => closed = Some(u),
                Outcome::Nothing => {}
            }
        }
    }
    closed.map_or(Outcome::Nothing, Outcome::Closed)
}

/// Mise à jour d'un jar identifié vers la dernière version compatible de son projet (puis, à défaut, par slug).
async fn resolve_update(ctx: &Ctx<'_>, ident: &Ident, slug: &str, need: &Need) -> Outcome {
    let first = ident.preferred();
    let mut closed = None;
    for src in [first, other(first)] {
        let project = match src {
            BrowseSource::Modrinth => ident.mr_project.clone(),
            BrowseSource::Curseforge => ident.cf_mod.map(|n| n.to_string()),
        };
        let mut outs = Vec::new();
        if let Some(p) = project {
            outs.push(in_project(ctx, src, &p, need).await);
        }
        outs.push(by_slug(ctx, src, slug, need).await);
        for o in outs {
            match o {
                Outcome::Found(f) => return Outcome::Found(f),
                Outcome::Closed(u) => closed = Some(u),
                Outcome::Nothing => {}
            }
        }
    }
    closed.map_or(Outcome::Nothing, Outcome::Closed)
}

// ---------- Construction du plan ----------

#[derive(Default)]
struct State {
    steps: Vec<Step>,
    unresolved: Vec<FixUnresolved>,
    /// Ids fournis par les installations / mises à jour prévues : (id, version, index du pas).
    provided: Vec<(String, Option<String>, usize)>,
    installs: usize,
}

impl State {
    fn add_issue_files(&mut self, idx: usize, files: &[String]) {
        for f in files {
            if !self.steps[idx].action.issue_files.contains(f) {
                self.steps[idx].action.issue_files.push(f.clone());
            }
        }
    }

    /// Pas déjà prévus qui couvrent tous les besoins (alors on les réutilise).
    fn covering(&self, need: &Need) -> Option<Vec<usize>> {
        let mut out = Vec::new();
        for (id, ranges, flavor) in &need.reqs {
            let hit = self.provided.iter().find(|(pid, v, _)| pid == id && version_ok(ranges, *flavor, v.as_deref()))?;
            if !out.contains(&hit.2) {
                out.push(hit.2);
            }
        }
        (!out.is_empty()).then_some(out)
    }

    fn push(&mut self, mut action: FixAction, op: Op) -> usize {
        action.id = format!("a{}", self.steps.len() + 1);
        self.steps.push(Step { action, op });
        self.steps.len() - 1
    }

    fn unresolve(&mut self, file: &str, mod_name: &str, message: &str, reason: String, page_url: Option<String>) {
        let u = FixUnresolved { file: file.to_string(), mod_name: mod_name.to_string(), message: message.to_string(), reason, page_url };
        if !self.unresolved.contains(&u) {
            self.unresolved.push(u);
        }
    }
}

fn no_version_reason(ctx: &Ctx<'_>) -> String {
    let l = browse::accepted(ctx.loader_opt()).join("/");
    format!("Aucune version compatible avec Minecraft {} ({}) sur Modrinth ni CurseForge.", ctx.mc, if l.is_empty() { "tout loader" } else { &l })
}

fn failure_reason(ctx: &Ctx<'_>, o: &Outcome) -> (String, Option<String>) {
    match o {
        Outcome::Closed(u) => ("L'auteur interdit le téléchargement par les applications (CurseForge).".into(), u.clone()),
        _ => (no_version_reason(ctx), None),
    }
}

/// Enregistre une installation ou une mise à jour trouvée.
#[allow(clippy::too_many_arguments)]
fn stage_found(ctx: &Ctx<'_>, st: &mut State, f: &Found, kind: &str, file: Option<&str>, issue_files: &[String], detail: Option<String>, old_name: Option<&str>) -> usize {
    // Même fichier déjà prévu : on réutilise.
    if let Some(i) = st.steps.iter().position(|s| matches!(&s.op, Op::Install { pl, .. } if pl.name == f.cand.pl.name)) {
        st.add_issue_files(i, issue_files);
        return i;
    }
    let (id, name, ver) = main_mod(&f.meta, ctx.loader_s).unwrap_or_else(|| (f.cand.pl.name.clone(), f.cand.pl.name.clone(), None));
    let _ = id;
    let version = clean_version(ver.as_deref()).unwrap_or_else(|| f.cand.version.clone());
    let src = source_label(f.cand.pl.source);
    let title = if kind == "update" {
        format!("Mettre à jour {} vers {version} ({src})", old_name.unwrap_or(&name))
    } else {
        format!("Installer {name} {version} ({src})")
    };
    let replace: Vec<String> = file.map(|f| vec![f.to_string()]).unwrap_or_default();
    let action = FixAction {
        id: String::new(),
        kind: kind.into(),
        title,
        detail,
        source: Some(f.cand.pl.source.as_str().to_string()),
        file: file.map(str::to_string),
        issue_files: issue_files.to_vec(),
    };
    let idx = st.push(action, Op::Install { pl: f.cand.pl.clone(), obj: f.obj.clone(), replace });
    st.installs += 1;
    for m in mod_check::selected(&f.meta, ctx.loader_s) {
        st.provided.push((m.id.clone(), m.version.clone(), idx));
        for (p, pv) in &m.provides {
            st.provided.push((p.clone(), pv.clone().or_else(|| m.version.clone()), idx));
        }
    }
    for (n, v) in &f.meta.nested {
        st.provided.push((n.clone(), v.clone(), idx));
    }
    idx
}

/// Dépendances obligatoires du jar qu'aucun jar actif, aucune autre action ni lui-même ne fournit.
fn unmet_deps(ctx: &Ctx<'_>, st: &State, active: &Index, meta: &JarMeta) -> Vec<(String, Vec<String>, Flavor)> {
    let mixinextras_builtin = ctx.loader_s != "forge" && (ctx.loader_s != "neoforge" || vr::compare(ctx.mc, "1.20.4").is_some_and(|o| o != std::cmp::Ordering::Less));
    let mut out: Vec<(String, Vec<String>, Flavor)> = Vec::new();
    for m in mod_check::selected(meta, ctx.loader_s) {
        for d in &m.deps {
            if d.kind != DepKind::Required || d.side == "SERVER" || d.id == "minecraft" || mod_check::BUILTIN_IDS.contains(&d.id.as_str()) || (d.id == "mixinextras" && mixinextras_builtin) {
                continue;
            }
            let ranges = vec![d.range.clone()];
            let flavor = m.flavor();
            let by_active = active.get(&d.id).is_some_and(|ps| ps.iter().any(|p| version_ok(&ranges, flavor, p.version.as_deref())));
            let by_plan = st.provided.iter().any(|(id, v, _)| *id == d.id && version_ok(&ranges, flavor, v.as_deref()));
            let by_self = provided_versions(meta, ctx.loader_s, &d.id).iter().any(|v| version_ok(&ranges, flavor, v.as_deref()));
            if !(by_active || by_plan || by_self) && !out.iter().any(|(i, _, _)| *i == d.id) {
                out.push((d.id.clone(), ranges, flavor));
            }
        }
    }
    out
}

/// Installe les dépendances obligatoires du candidat retenu (profondeur bornée).
fn install_deps<'a>(ctx: &'a Ctx<'a>, st: &'a mut State, active: &'a Index, parent: &'a Found, parent_title: String, issue_files: Vec<String>, depth: u32) -> Pin<Box<dyn Future<Output = ()> + Send + 'a>> {
    Box::pin(async move {
        if depth >= MAX_DEPTH {
            return;
        }
        let deps = unmet_deps(ctx, st, active, &parent.meta);
        let pname = main_mod(&parent.meta, ctx.loader_s).map_or(parent.cand.pl.name.clone(), |m| m.1);
        for (id, ranges, flavor) in deps {
            if st.installs >= MAX_INSTALLS {
                return;
            }
            let need = Need::reqs(vec![(id.clone(), ranges, flavor)]);
            if let Some(idx) = st.covering(&need) {
                for i in idx {
                    st.add_issue_files(i, &issue_files);
                }
                continue;
            }
            match resolve_dependency(ctx, &need, &parent.cand.hints, &id).await {
                Outcome::Found(f) => {
                    stage_found(ctx, st, &f, "install", None, &issue_files, Some(format!("Requis par {pname}.")), None);
                    install_deps(ctx, st, active, &f, parent_title.clone(), issue_files.clone(), depth + 1).await;
                }
                o => {
                    let (reason, url) = failure_reason(ctx, &o);
                    let file = issue_files.first().cloned().unwrap_or_default();
                    st.unresolve(&file, &pname, &format!("Requiert {id} (absent)."), reason, url);
                }
            }
        }
    })
}

/// Un jar désactivé qui fournit les ids manquants.
fn find_disabled(ctx: &Ctx<'_>, entries: &[mods::ModEntry], need: &Need) -> Option<(String, String)> {
    let dir = ctx.mods_dir();
    for e in entries.iter().filter(|e| !e.enabled) {
        let Some(meta) = jar_meta(&dir.join(&e.file)) else { continue };
        if meta_satisfies(&meta, ctx.loader_s, need) && compat_ok(&meta, ctx.mc, ctx.loader_s) {
            let base = base_name(&e.file).to_string();
            let name = main_mod(&meta, ctx.loader_s).map_or(base.clone(), |m| m.1);
            return Some((base, name));
        }
    }
    None
}

fn requirers_text(items: &[(&ModIssue, &DepDetail)]) -> String {
    let names: Vec<String> = items.iter().map(|(i, _)| i.mod_name.clone()).collect();
    join_names(&names)
}

/// Change de version chaque mod qui exige une dépendance introuvable ou trop récente : la plus récente compatible
/// avec ce Minecraft / loader qui accepte la dépendance telle qu'installée (`have` : `None` = ne pas l'exiger).
/// Renvoie les problèmes restés sans solution.
#[allow(clippy::too_many_arguments)]
async fn realign<'i>(
    ctx: &Ctx<'_>,
    st: &mut State,
    active: &Index,
    metas: &HashMap<String, Arc<JarMeta>>,
    ident_of: &(dyn Fn(&str) -> Ident + Sync),
    items: &[(&'i ModIssue, &'i DepDetail)],
    have: &(dyn Fn(&DepDetail) -> Option<String> + Sync),
    dep_label: &str,
    replaced: &mut HashSet<String>,
) -> Vec<(&'i ModIssue, &'i DepDetail)> {
    let mut rest = Vec::new();
    for (i, d) in items.iter().copied() {
        if replaced.contains(&i.file) || st.installs >= MAX_INSTALLS {
            rest.push((i, d));
            continue;
        }
        let Some(meta) = metas.get(&i.file) else {
            rest.push((i, d));
            continue;
        };
        let Some((mid, name, old_ver)) = main_mod(meta, ctx.loader_s) else {
            rest.push((i, d));
            continue;
        };
        let have_v = have(d);
        let need = Need { reqs: vec![(mid.clone(), Vec::new(), d.flavor)], accept: vec![(d.id.clone(), have_v.clone())], replace: Some(i.file.clone()) };
        match resolve_update(ctx, &ident_of(&i.file), &mid, &need).await {
            Outcome::Found(f) => {
                let issue_files = vec![i.file.clone()];
                let detail = match &have_v {
                    Some(v) => format!("Accepte {dep_label} {} déjà installé : aucun {dep_label} plus récent n'existe pour Minecraft {}.", clean_version(Some(v)).unwrap_or_else(|| v.clone()), ctx.mc),
                    None => format!("N'a plus besoin de {dep_label}, introuvable pour Minecraft {}.", ctx.mc),
                };
                let idx = stage_found(ctx, st, &f, "update", Some(&i.file), &issue_files, Some(detail), Some(&name));
                let new_ver = main_mod(&f.meta, ctx.loader_s).and_then(|m| clean_version(m.2.as_deref())).unwrap_or_else(|| f.cand.version.clone());
                let older = old_ver.as_deref().and_then(|o| vr::compare(&new_ver, o)) == Some(std::cmp::Ordering::Less);
                if older {
                    st.steps[idx].action.title = format!("Revenir à {name} {new_ver} ({})", source_label(f.cand.pl.source));
                }
                replaced.insert(i.file.clone());
                install_deps(ctx, st, active, &f, name.clone(), issue_files, 0).await;
            }
            _ => rest.push((i, d)),
        }
    }
    rest
}

/// Calcule le plan de correction. N'écrit rien dans `game_dir` (les candidats vont dans le magasin).
#[allow(clippy::too_many_arguments)]
pub async fn plan(p: &Providers, mc: &str, loader: Loader, game_dir: &Path, store_root: &Path, index: &StoreIndex, dl: &DlOptions, progress: ProgressCb<'_>) -> Result<PreparedFix> {
    let fingerprint = mods_fingerprint(game_dir);
    let det = mod_check::check_detailed(game_dir, mc, loader_str(loader))?;
    let baseline = det.check.issues.iter().filter(|i| i.severity == "error").map(|i| (i.file.clone(), i.message.clone())).collect();
    let ctx = Ctx { p, mc, loader, loader_s: loader_str(loader), game_dir, store_root, index, dl, jars: &det.jars, baseline };
    let metas: HashMap<String, Arc<JarMeta>> = det.jars.iter().cloned().collect();
    let active: Index = mod_check::build_index(&det.jars);
    let entries = mods::list(game_dir)?;
    let mut st = State::default();

    let errors: Vec<&ModIssue> = det.check.issues.iter().filter(|i| i.severity == "error").collect();
    let mut dups: BTreeMap<String, Vec<&ModIssue>> = BTreeMap::new();
    let mut wrongs: BTreeMap<String, Vec<&ModIssue>> = BTreeMap::new();
    let mut absent: BTreeMap<String, Vec<(&ModIssue, &DepDetail)>> = BTreeMap::new();
    let mut outdated: BTreeMap<String, Vec<(&ModIssue, &DepDetail)>> = BTreeMap::new();
    for i in &errors {
        match i.kind.as_str() {
            "incompatible" => st.unresolve(&i.file, &i.mod_name, &i.message, "Incompatibilité : choisis toi-même le mod à désactiver.".into(), None),
            "duplicate" => dups.entry(i.related.clone().unwrap_or_default()).or_default().push(i),
            "wrong_minecraft" | "wrong_loader" => wrongs.entry(i.file.clone()).or_default().push(i),
            "missing_dep" => match det.details.get(&(i.file.clone(), i.message.clone())) {
                Some(d) => match &d.provider {
                    None => absent.entry(d.id.clone()).or_default().push((i, d)),
                    Some(pf) => outdated.entry(pf.clone()).or_default().push((i, d)),
                },
                None => st.unresolve(&i.file, &i.mod_name, &i.message, "Problème non corrigeable automatiquement.".into(), None),
            },
            _ => st.unresolve(&i.file, &i.mod_name, &i.message, "Problème non corrigeable automatiquement.".into(), None),
        }
    }
    let total = (dups.len() + wrongs.len() + absent.len() + outdated.len()) as u32 + 1;
    let done = std::sync::atomic::AtomicU32::new(0);
    let tick = |label: &str| {
        let n = done.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1;
        progress(n, total, label);
    };
    progress(0, total, "Analyse des mods");

    // 1) Doublons : désactiver les exemplaires les plus anciens, sans réseau.
    let mut disabled: HashSet<String> = HashSet::new();
    for (reference, issues) in &dups {
        let mut members: Vec<String> = vec![reference.clone()];
        members.extend(issues.iter().map(|i| i.file.clone()));
        members.dedup();
        let ver = |f: &str| metas.get(f).and_then(|m| main_mod(m, ctx.loader_s)).and_then(|m| m.2);
        let mut best = reference.clone();
        let mut decided = false;
        for f in &members {
            if let (Some(a), Some(b)) = (ver(f), ver(&best)) {
                match vr::compare(&a, &b) {
                    Some(std::cmp::Ordering::Greater) => {
                        best = f.clone();
                        decided = true;
                    }
                    Some(std::cmp::Ordering::Less) => decided = true,
                    _ => {}
                }
            }
        }
        let issue_files: Vec<String> = issues.iter().map(|i| i.file.clone()).collect();
        for f in members.iter().filter(|f| **f != best) {
            let name = issues.iter().find(|i| i.file == *f).map(|i| i.mod_name.clone()).or_else(|| metas.get(f).and_then(|m| main_mod(m, ctx.loader_s)).map(|m| m.1)).unwrap_or_else(|| f.clone());
            let title = if decided { format!("Désactiver l'ancien exemplaire de {name}") } else { format!("Désactiver le doublon de {name}") };
            let action = FixAction { id: String::new(), kind: "disable".into(), title, detail: Some(format!("Garde {best}.")), source: None, file: Some(f.clone()), issue_files: issue_files.clone() };
            st.push(action, Op::Disable(f.clone()));
            disabled.insert(f.clone());
        }
        tick("Doublons");
    }

    // 2) Dépendances absentes : réactiver d'abord un jar désactivé qui les fournit (hors ligne).
    let mut net_absent: Vec<(String, Vec<(&ModIssue, &DepDetail)>)> = Vec::new();
    for (id, items) in absent {
        let items: Vec<(&ModIssue, &DepDetail)> = items.into_iter().filter(|(i, _)| !disabled.contains(&i.file)).collect();
        if items.is_empty() {
            continue;
        }
        let need = Need::reqs(vec![(id.clone(), items.iter().map(|(_, d)| d.range.clone()).collect(), items[0].1.flavor)]);
        let issue_files: Vec<String> = items.iter().map(|(i, _)| i.file.clone()).collect();
        if let Some((base, name)) = find_disabled(&ctx, &entries, &need) {
            if let Some(i) = st.steps.iter().position(|s| matches!(&s.op, Op::Enable(f) if *f == base)) {
                st.add_issue_files(i, &issue_files);
            } else {
                let action = FixAction {
                    id: String::new(),
                    kind: "enable".into(),
                    title: format!("Réactiver {name}"),
                    detail: Some(format!("Requis par {}.", requirers_text(&items))),
                    source: None,
                    file: Some(base.clone()),
                    issue_files,
                };
                st.push(action, Op::Enable(base));
            }
            tick("Réactivation");
        } else {
            net_absent.push((id, items));
        }
    }

    // 3) Identification (un seul lot Modrinth, un seul lot CurseForge) des jars concernés.
    let mut to_identify: Vec<String> = Vec::new();
    for f in wrongs.keys() {
        to_identify.push(f.clone());
    }
    for (pf, items) in &outdated {
        if items.iter().any(|(i, _)| !disabled.contains(&i.file)) {
            to_identify.push(pf.clone());
            to_identify.extend(items.iter().map(|(i, _)| i.file.clone()));
        }
    }
    for (_, items) in &net_absent {
        for (i, _) in items {
            to_identify.push(i.file.clone());
        }
    }
    to_identify.sort();
    to_identify.dedup();
    to_identify.retain(|f| metas.contains_key(f));
    progress(done.load(std::sync::atomic::Ordering::Relaxed), total, "Identification des mods concernés");
    let idents = identify(&ctx, &to_identify).await;
    let ident_of = |f: &str| idents.get(f).cloned().unwrap_or_default();

    // 4) Mauvais Minecraft / loader : mise à jour du jar.
    let mut replaced: HashSet<String> = HashSet::new();
    for (file, issues) in &wrongs {
        let Some(meta) = metas.get(file) else { continue };
        let (mid, name, _) = main_mod(meta, ctx.loader_s).unwrap_or_else(|| (file.clone(), file.clone(), None));
        progress(done.load(std::sync::atomic::Ordering::Relaxed), total, &format!("Recherche d'une version de {name}"));
        let o = resolve_update(&ctx, &ident_of(file), &mid, &Need { replace: Some(file.clone()), ..Default::default() }).await;
        match o {
            Outcome::Found(f) => {
                let issue_files = vec![file.clone()];
                stage_found(&ctx, &mut st, &f, "update", Some(file), &issue_files, Some(issues[0].message.clone()), Some(&name));
                replaced.insert(file.clone());
                install_deps(&ctx, &mut st, &active, &f, name.clone(), issue_files, 0).await;
            }
            o => {
                let (reason, url) = failure_reason(&ctx, &o);
                for i in issues {
                    st.unresolve(&i.file, &i.mod_name, &i.message, reason.clone(), url.clone());
                }
            }
        }
        tick(&name);
    }

    // 5) Fournisseur trop ancien : mise à jour du fournisseur.
    for (provider, items) in &outdated {
        let items: Vec<(&ModIssue, &DepDetail)> = items.iter().copied().filter(|(i, _)| !disabled.contains(&i.file) && !replaced.contains(&i.file)).collect();
        if items.is_empty() {
            continue;
        }
        let mut reqs: Vec<(String, Vec<String>, Flavor)> = Vec::new();
        for (_, d) in &items {
            match reqs.iter_mut().find(|r| r.0 == d.id) {
                Some(r) => r.1.push(d.range.clone()),
                None => reqs.push((d.id.clone(), vec![d.range.clone()], d.flavor)),
            }
        }
        let need = Need { reqs, replace: Some(provider.clone()), ..Default::default() };
        let issue_files: Vec<String> = items.iter().map(|(i, _)| i.file.clone()).collect();
        if let Some(idx) = st.covering(&need) {
            for i in idx {
                st.add_issue_files(i, &issue_files);
            }
            continue;
        }
        let Some(meta) = metas.get(provider) else {
            continue;
        };
        let (mid, name, _) = main_mod(meta, ctx.loader_s).unwrap_or_else(|| (provider.clone(), provider.clone(), None));
        progress(done.load(std::sync::atomic::Ordering::Relaxed), total, &format!("Recherche d'une version de {name}"));
        let o = resolve_update(&ctx, &ident_of(provider), &mid, &need).await;
        match o {
            Outcome::Found(f) => {
                let detail = format!("Requis par {}.", requirers_text(&items));
                stage_found(&ctx, &mut st, &f, "update", Some(provider), &issue_files, Some(detail), Some(&name));
                replaced.insert(provider.clone());
                install_deps(&ctx, &mut st, &active, &f, name.clone(), issue_files, 0).await;
            }
            o => {
                // Pas de version du fournisseur qui convienne : on prend l'autre côté, une version de chaque mod
                // qui l'exige qui accepte le fournisseur installé.
                let have = |d: &DepDetail| provided_versions(meta, ctx.loader_s, &d.id).into_iter().flatten().next();
                let rest = realign(&ctx, &mut st, &active, &metas, &ident_of, &items, &have, &name, &mut replaced).await;
                let (reason, url) = failure_reason(&ctx, &o);
                for (i, _) in rest {
                    st.unresolve(&i.file, &i.mod_name, &i.message, reason.clone(), url.clone());
                }
            }
        }
        tick(&name);
    }

    // 6) Dépendances absentes : installation.
    for (id, items) in net_absent {
        let items: Vec<(&ModIssue, &DepDetail)> = items.into_iter().filter(|(i, _)| !disabled.contains(&i.file) && !replaced.contains(&i.file)).collect();
        if items.is_empty() {
            continue;
        }
        let need = Need::reqs(vec![(id.clone(), items.iter().map(|(_, d)| d.range.clone()).collect(), items[0].1.flavor)]);
        let issue_files: Vec<String> = items.iter().map(|(i, _)| i.file.clone()).collect();
        if let Some(idx) = st.covering(&need) {
            for i in idx {
                st.add_issue_files(i, &issue_files);
            }
            continue;
        }
        if st.installs >= MAX_INSTALLS {
            for (i, _) in &items {
                st.unresolve(&i.file, &i.mod_name, &i.message, "Trop de fichiers à installer en une fois : relance la correction ensuite.".into(), None);
            }
            continue;
        }
        let mut hints = Hints::default();
        for (i, _) in &items {
            let h = ident_of(&i.file).hints;
            hints.pref = hints.pref.or(h.pref);
            hints.mr_deps.extend(h.mr_deps);
            hints.cf_deps.extend(h.cf_deps);
        }
        progress(done.load(std::sync::atomic::Ordering::Relaxed), total, &format!("Recherche de {id}"));
        let o = resolve_dependency(&ctx, &need, &hints, &id).await;
        match o {
            Outcome::Found(f) => {
                let detail = format!("Requis par {}.", requirers_text(&items));
                stage_found(&ctx, &mut st, &f, "install", None, &issue_files, Some(detail), None);
                install_deps(&ctx, &mut st, &active, &f, id.clone(), issue_files, 0).await;
            }
            o => {
                let rest = realign(&ctx, &mut st, &active, &metas, &ident_of, &items, &|_: &DepDetail| None, &id, &mut replaced).await;
                let (reason, url) = failure_reason(&ctx, &o);
                for (i, _) in rest {
                    st.unresolve(&i.file, &i.mod_name, &i.message, reason.clone(), url.clone());
                }
            }
        }
        tick(&id);
    }
    progress(total, total, "Terminé");

    // Ordre d'application : activer / désactiver d'abord, puis installer / mettre à jour (les identifiants restent stables).
    let steps = st.steps;
    let actions: Vec<FixAction> = steps.iter().map(|s| s.action.clone()).collect();
    Ok(PreparedFix { plan: FixPlan { plan_id: String::new(), actions, unresolved: st.unresolved }, fingerprint, steps })
}

// ---------- Compatibilité d'un contenu avant installation (fiche Parcourir) ----------

/// Verdict pour une version d'un projet dans une instance.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, specta::Type)]
pub struct VersionVerdict {
    pub version_id: String,
    pub ok: bool,
    /// Problèmes que cette version créerait (« Iris : Requiert Sodium 0.9.x… »).
    pub problems: Vec<String>,
}

/// Résultat de [`check_content`].
#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq, specta::Type)]
pub struct ContentCheck {
    /// Fichiers de l'instance qui sont déjà ce projet (nom activé, même s'ils sont désactivés).
    pub installed: Vec<String>,
    /// Version la plus récente qui n'ajoute aucun problème au pack.
    pub recommended: Option<String>,
    pub verdicts: Vec<VersionVerdict>,
    /// Mods déjà présents qui empêchent toute version testée (noms affichables).
    pub blockers: Vec<String>,
}

/// Versions essayées pour trouver la version recommandée.
const CONTENT_TRIES: usize = 8;

/// Essaie les versions d'un mod dans le pack (simulation, rien n'est écrit dans l'instance) : version recommandée,
/// verdict par version, fichiers déjà installés. `only` : ne juger que cette version.
#[allow(clippy::too_many_arguments)]
pub async fn check_content(
    p: &Providers,
    mc: &str,
    loader: Loader,
    game_dir: &Path,
    store_root: &Path,
    index: &StoreIndex,
    dl: &DlOptions,
    source: BrowseSource,
    project: &str,
    only: Option<&str>,
) -> Result<ContentCheck> {
    let det = mod_check::check_detailed(game_dir, mc, loader_str(loader))?;
    let baseline = det.check.issues.iter().filter(|i| i.severity == "error").map(|i| (i.file.clone(), i.message.clone())).collect();
    let ctx = Ctx { p, mc, loader, loader_s: loader_str(loader), game_dir, store_root, index, dl, jars: &det.jars, baseline };
    let entries = mods::list(game_dir)?;
    let mut out = ContentCheck::default();

    // Déjà installé d'après la provenance.
    for (file, pr) in mods::read_provenance(game_dir) {
        if pr.source == source.as_str() && pr.id == project && entries.iter().any(|e| base_name(&e.file) == file) && !out.installed.contains(&file) {
            out.installed.push(file);
        }
    }

    // Versions compatibles Minecraft / loader, la plus récente d'abord : (id de version, candidat ou raison).
    let mut cands: Vec<(String, std::result::Result<Cand, String>)> = Vec::new();
    match source {
        BrowseSource::Modrinth => {
            for v in browse::modrinth_versions(p, project, BrowseKind::Mod, Some(mc), ctx.loader_opt()).await? {
                let c = browse::plan_modrinth(p, &v, ".jar").map(|pl| Cand { pl, version: v.version_number.clone(), hints: mr_hints(&v) }).map_err(|e| e.to_string());
                cands.push((v.id.clone(), c));
            }
        }
        BrowseSource::Curseforge => {
            let id: u64 = project.parse().map_err(|_| invalid("identifiant CurseForge invalide"))?;
            for f in browse::cf_versions(p, project, BrowseKind::Mod, Some(mc), ctx.loader_opt()).await? {
                let c = browse::plan_cf(p, &f, id, ".jar")
                    .map(|pl| Cand { pl, version: f.display_name.clone(), hints: cf_hints(&f) })
                    .map_err(|_| "L'auteur interdit le téléchargement par les applications.".to_string());
                cands.push((f.id.to_string(), c));
            }
        }
    }
    if let Some(o) = only {
        cands.retain(|(id, _)| id == o);
    }

    let dir = ctx.mods_dir();
    let mut first_problems: Option<Vec<ModIssue>> = None;
    for (vid, cand) in cands.into_iter().take(if only.is_some() { 1 } else { CONTENT_TRIES }) {
        let cand = match cand {
            Ok(c) => c,
            Err(reason) => {
                out.verdicts.push(VersionVerdict { version_id: vid, ok: false, problems: vec![reason] });
                continue;
            }
        };
        if browse::check_url(&cand.pl.url, cand.pl.source, p.allow_loopback).is_err() {
            continue;
        }
        let obj = match browse::fetch(&cand.pl, store_root, index, dl).await {
            Ok(o) => o,
            Err(e) => {
                out.verdicts.push(VersionVerdict { version_id: vid, ok: false, problems: vec![format!("Téléchargement impossible : {e}")] });
                continue;
            }
        };
        let Some(meta) = jar_meta(&store_root.join(&obj.rel_path)) else { continue };
        // Le même mod déjà présent (activé ou non) : il serait remplacé, donc on le retire de la simulation.
        let ids: HashSet<String> = mod_check::selected(&meta, ctx.loader_s).iter().map(|m| m.id.clone()).collect();
        let mut same: Vec<String> = Vec::new();
        for e in &entries {
            let m = if e.enabled { det.jars.iter().find(|(f, _)| *f == e.file).map(|(_, m)| m.clone()) } else { jar_meta(&dir.join(&e.file)) };
            if m.is_some_and(|m| main_mod(&m, ctx.loader_s).is_some_and(|(id, _, _)| ids.contains(&id))) {
                let b = base_name(&e.file).to_string();
                if !same.contains(&b) {
                    same.push(b);
                }
            }
        }
        for f in &same {
            if !out.installed.contains(f) {
                out.installed.push(f.clone());
            }
        }
        let replace: Vec<&str> = same.iter().map(String::as_str).collect();
        let issues = ctx.new_issues(&cand.pl.name, &meta, &replace);
        let ok = issues.is_empty();
        out.verdicts.push(VersionVerdict { version_id: vid.clone(), ok, problems: issues.iter().map(|i| format!("{} : {}", i.mod_name, i.message)).collect() });
        if ok {
            out.recommended = Some(vid);
            break;
        }
        if first_problems.is_none() {
            first_problems = Some(issues);
        }
    }
    if out.recommended.is_none() {
        // Ce qui bloque toutes les versions essayées (d'après la plus récente).
        for i in first_problems.unwrap_or_default() {
            let label = format!("{} : {}", i.mod_name, i.message);
            if !out.blockers.contains(&label) {
                out.blockers.push(label);
            }
        }
    }
    Ok(out)
}

// ---------- Sauvegarde, application, annulation ----------

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
struct Manifest {
    id: String,
    /// Fichiers déplacés dans la sauvegarde (nom tel qu'il était dans `mods/`) et leur provenance.
    moved: Vec<Moved>,
    /// Fichiers ajoutés dans `mods/`.
    added: Vec<Added>,
    /// Activations / désactivations (nom activé, état d'origine).
    toggled: Vec<Toggled>,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
struct Moved {
    file: String,
    #[serde(default)]
    provenance: Option<Provenance>,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
struct Added {
    file: String,
    sha256: String,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
struct Toggled {
    file: String,
    was_enabled: bool,
}

impl Manifest {
    fn is_empty(&self) -> bool {
        self.moved.is_empty() && self.added.is_empty() && self.toggled.is_empty()
    }
}

fn backup_root(game_dir: &Path) -> PathBuf {
    game_dir.join("mods").join(BACKUP_DIR)
}

fn valid_backup_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= 64 && id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
}

fn new_backup_id() -> String {
    static N: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
    let ms = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_millis());
    format!("{ms:015}-{:03}", N.fetch_add(1, std::sync::atomic::Ordering::Relaxed) % 1000)
}

fn write_manifest(dir: &Path, m: &Manifest) -> Result<()> {
    std::fs::create_dir_all(dir)?;
    let json = serde_json::to_vec_pretty(m).map_err(|e| Error::Other(e.to_string()))?;
    let tmp = dir.join("manifest.json.tmp");
    std::fs::write(&tmp, json)?;
    std::fs::rename(&tmp, dir.join("manifest.json"))?;
    Ok(())
}

fn prune_backups(game_dir: &Path) {
    let Ok(rd) = std::fs::read_dir(backup_root(game_dir)) else { return };
    let mut dirs: Vec<String> = rd.flatten().filter(|e| e.path().is_dir()).filter_map(|e| e.file_name().into_string().ok()).collect();
    dirs.sort();
    while dirs.len() > KEEP_BACKUPS {
        let old = dirs.remove(0);
        let _ = std::fs::remove_dir_all(backup_root(game_dir).join(old));
    }
}

/// Déplace `mods/<file>` dans la sauvegarde et oublie sa provenance. `Ok(None)` si le fichier n'existe pas.
fn move_to_backup(game_dir: &Path, bdir: &Path, file: &str) -> Result<Option<Moved>> {
    mods::validate_file_name(file)?;
    let src = game_dir.join("mods").join(file);
    if !src.is_file() {
        return Ok(None);
    }
    let prov = mods::read_provenance(game_dir).get(base_name(file)).cloned();
    std::fs::create_dir_all(bdir)?;
    let dst = bdir.join(file);
    if dst.exists() {
        return Err(invalid(format!("la sauvegarde contient déjà {file}")));
    }
    std::fs::rename(&src, &dst)?;
    mods::forget_provenance(game_dir, &[file.to_string()])?;
    Ok(Some(Moved { file: file.to_string(), provenance: prov }))
}

fn restore_moved(game_dir: &Path, bdir: &Path, m: &Moved) -> Result<()> {
    let dst = game_dir.join("mods").join(&m.file);
    if dst.exists() {
        return Err(invalid(format!("{} existe déjà", m.file)));
    }
    std::fs::rename(bdir.join(&m.file), &dst)?;
    if let Some(p) = &m.provenance {
        mods::record_provenance(game_dir, &[(base_name(&m.file).to_string(), p.clone())])?;
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn do_install(game_dir: &Path, store_root: &Path, index: &StoreIndex, bdir: &Path, mf: &mut Manifest, pl: &Planned, obj: &StoredObject, replace: &[String]) -> Result<()> {
    mods::validate_file_name(&pl.name)?;
    std::fs::create_dir_all(game_dir.join("mods"))?;
    let mut targets: Vec<String> = replace.to_vec();
    for n in [pl.name.clone(), format!("{}.disabled", pl.name)] {
        if !targets.contains(&n) {
            targets.push(n);
        }
    }
    let mut moved_now: Vec<Moved> = Vec::new();
    let rollback = |moved: &[Moved]| {
        for m in moved.iter().rev() {
            let _ = restore_moved(game_dir, bdir, m);
        }
    };
    for t in &targets {
        match move_to_backup(game_dir, bdir, t) {
            Ok(Some(m)) => moved_now.push(m),
            Ok(None) => {}
            Err(e) => {
                rollback(&moved_now);
                return Err(e);
            }
        }
    }
    if let Err(e) = link_into(game_dir, &format!("mods/{}", pl.name), &store_root.join(&obj.rel_path)) {
        rollback(&moved_now);
        return Err(e);
    }
    index.add_ref(&obj.sha256)?;
    mods::record_provenance(game_dir, &[(pl.name.clone(), Provenance { source: pl.source.as_str().into(), id: pl.project_id.clone() })])?;
    mf.moved.extend(moved_now);
    mf.added.push(Added { file: pl.name.clone(), sha256: obj.sha256.clone() });
    Ok(())
}

/// Applique les actions choisies (ordre : activer / désactiver, puis installer / mettre à jour).
/// Une action en échec n'arrête pas les autres. Renvoie `backup_id = None` si rien n'a été modifié.
#[allow(clippy::too_many_arguments)]
pub fn apply(prep: &PreparedFix, action_ids: &[String], game_dir: &Path, store_root: &Path, index: &StoreIndex, minecraft: &str, loader: Loader, progress: ProgressCb<'_>) -> Result<FixResult> {
    let chosen: Vec<&Step> = prep
        .steps
        .iter()
        .filter(|s| action_ids.contains(&s.action.id))
        .collect();
    let ordered: Vec<&Step> = chosen.iter().copied().filter(|s| !matches!(s.op, Op::Install { .. })).chain(chosen.iter().copied().filter(|s| matches!(s.op, Op::Install { .. }))).collect();
    let id = new_backup_id();
    let bdir = backup_root(game_dir).join(&id);
    let mut mf = Manifest { id: id.clone(), ..Default::default() };
    let (mut applied, mut failed) = (Vec::new(), Vec::new());
    let total = ordered.len() as u32;
    progress(0, total, "Application des correctifs");
    for (n, s) in ordered.iter().enumerate() {
        let r: Result<()> = match &s.op {
            Op::Enable(f) | Op::Disable(f) => {
                let enable = matches!(s.op, Op::Enable(_));
                mods::validate_file_name(f).and_then(|_| mods::toggle(game_dir, f, enable)).map(|_| {
                    mf.toggled.push(Toggled { file: f.clone(), was_enabled: !enable });
                })
            }
            Op::Install { pl, obj, replace } => do_install(game_dir, store_root, index, &bdir, &mut mf, pl, obj, replace),
        };
        match r {
            Ok(()) => applied.push(s.action.title.clone()),
            Err(e) => failed.push(FixFailure { title: s.action.title.clone(), error: e.to_string() }),
        }
        if !mf.is_empty() {
            let _ = write_manifest(&bdir, &mf);
        }
        progress(n as u32 + 1, total, &s.action.title);
    }
    let backup_id = if mf.is_empty() {
        let _ = std::fs::remove_dir_all(&bdir);
        None
    } else {
        write_manifest(&bdir, &mf)?;
        prune_backups(game_dir);
        Some(id)
    };
    let check = mod_check::check(game_dir, minecraft, loader_str(loader))?;
    Ok(FixResult { applied, failed, backup_id, check })
}

/// Annule une correction : retire les fichiers ajoutés, remet les fichiers déplacés, défait les activations.
pub fn undo(game_dir: &Path, backup_id: &str, index: &StoreIndex, minecraft: &str, loader: Loader) -> Result<ModCheck> {
    if !valid_backup_id(backup_id) {
        return Err(invalid("identifiant de sauvegarde invalide"));
    }
    let bdir = backup_root(game_dir).join(backup_id);
    let raw = std::fs::read(bdir.join("manifest.json")).map_err(|_| Error::NotFound("sauvegarde introuvable (déjà annulée ou supprimée)".into()))?;
    let mf: Manifest = serde_json::from_slice(&raw).map_err(|e| invalid(format!("sauvegarde illisible : {e}")))?;
    let mut errors: Vec<String> = Vec::new();
    for a in mf.added.iter().rev() {
        match mods::remove(game_dir, &a.file) {
            Ok(()) => {
                let _ = index.release_ref(&a.sha256);
            }
            Err(Error::NotFound(_)) => {}
            Err(e) => errors.push(e.to_string()),
        }
    }
    for m in mf.moved.iter().rev() {
        if let Err(e) = restore_moved(game_dir, &bdir, m) {
            errors.push(format!("{} : {e}", m.file));
        }
    }
    for t in mf.toggled.iter().rev() {
        if let Err(e) = mods::toggle(game_dir, &t.file, t.was_enabled) {
            errors.push(format!("{} : {e}", t.file));
        }
    }
    if errors.is_empty() {
        let _ = std::fs::remove_dir_all(&bdir);
    } else {
        return Err(Error::Other(format!("annulation incomplète ({}) ; la sauvegarde est conservée dans mods/{BACKUP_DIR}/{backup_id}", errors.join(" ; "))));
    }
    mod_check::check(game_dir, minecraft, loader_str(loader))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mod_meta::tests::zip_bytes;
    use crate::providers::curseforge_api::CfApi;
    use crate::providers::modrinth_api::ModrinthClient;
    use serde_json::json;
    use sha2::{Digest, Sha512};
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn providers(mr: &MockServer, cf: &MockServer) -> Providers {
        Providers { modrinth: ModrinthClient::with_base_url(format!("{}/v2", mr.uri()), "test"), cf: CfApi::with_base(cf.uri(), "k"), allow_loopback: true }
    }

    struct Env {
        _d: tempfile::TempDir,
        store: PathBuf,
        index: StoreIndex,
        game: PathBuf,
    }

    fn env() -> Env {
        let d = tempfile::tempdir().unwrap();
        let store = d.path().join("store");
        std::fs::create_dir_all(&store).unwrap();
        let index = StoreIndex::open(store.join("index.sqlite")).unwrap();
        let game = d.path().join("inst").join("minecraft");
        std::fs::create_dir_all(game.join("mods")).unwrap();
        Env { _d: d, store, index, game }
    }

    fn dlo() -> DlOptions {
        DlOptions { attempts: 1, ..Default::default() }
    }

    fn fab(id: &str, version: &str, extra: &str) -> Vec<u8> {
        format!(r#"{{"id":"{id}","version":"{version}","name":"{}"{extra}}}"#, id.to_uppercase()).into_bytes()
    }

    fn fabjar(id: &str, version: &str, extra: &str) -> Vec<u8> {
        zip_bytes(&[("fabric.mod.json", &fab(id, version, extra))])
    }

    fn put(e: &Env, name: &str, bytes: &[u8]) {
        std::fs::write(e.game.join("mods").join(name), bytes).unwrap();
    }

    fn sha512(b: &[u8]) -> String {
        hex::encode(Sha512::digest(b))
    }

    fn mr_version(s: &MockServer, id: &str, project: &str, number: &str, file: &str, body: &[u8], deps: serde_json::Value) -> serde_json::Value {
        json!({"id": id, "project_id": project, "name": id, "version_number": number, "date_published": "2024-01-01T00:00:00Z",
               "game_versions": ["1.21.1"], "loaders": ["fabric"],
               "files": [{"url": format!("{}/cdn/{file}", s.uri()), "filename": file, "primary": true, "size": body.len(),
                          "hashes": {"sha1": "", "sha512": sha512(body)}}],
               "dependencies": deps})
    }

    async fn serve(s: &MockServer, p: &str, body: &[u8]) {
        Mock::given(method("GET")).and(path(p.to_string())).respond_with(ResponseTemplate::new(200).set_body_bytes(body.to_vec())).mount(s).await;
    }

    async fn project_versions(s: &MockServer, project: &str, versions: serde_json::Value) {
        Mock::given(method("GET")).and(path(format!("/v2/project/{project}/version"))).respond_with(ResponseTemplate::new(200).set_body_json(versions)).mount(s).await;
    }

    async fn known_hashes(s: &MockServer, entries: serde_json::Value) {
        Mock::given(method("POST")).and(path("/v2/version_files")).respond_with(ResponseTemplate::new(200).set_body_json(entries)).mount(s).await;
    }

    async fn no_cf_fingerprints(s: &MockServer) {
        Mock::given(method("POST")).and(path("/v1/fingerprints/432")).respond_with(ResponseTemplate::new(200).set_body_json(json!({"data": {"exactMatches": []}}))).mount(s).await;
    }

    fn sha1_of(e: &Env, name: &str) -> String {
        crate::mojang::versions::sha1_hex(&std::fs::read(e.game.join("mods").join(name)).unwrap())
    }

    async fn run_plan(e: &Env, mr: &MockServer, cf: &MockServer) -> PreparedFix {
        plan(&providers(mr, cf), "1.21.1", Loader::Fabric, &e.game, &e.store, &e.index, &dlo(), &|_, _, _| {}).await.unwrap()
    }

    fn listing(e: &Env) -> Vec<(String, u64)> {
        fn walk(dir: &Path, base: &Path, out: &mut Vec<(String, u64)>) {
            for en in std::fs::read_dir(dir).unwrap().flatten() {
                let p = en.path();
                if p.is_dir() {
                    walk(&p, base, out);
                } else {
                    out.push((p.strip_prefix(base).unwrap().display().to_string(), p.metadata().unwrap().len()));
                }
            }
        }
        let mut out = Vec::new();
        let root = e.game.parent().unwrap();
        walk(root, root, &mut out);
        out.sort();
        out
    }

    #[tokio::test]
    async fn reactivates_disabled_jar_offline() {
        let (mr, cf) = (MockServer::start().await, MockServer::start().await);
        let e = env();
        put(&e, "a.jar", &fabjar("a", "1.0", r#","depends":{"fabric-api":">=0.100"}"#));
        put(&e, "fabric-api-0.110.jar.disabled", &fabjar("fabric-api", "0.110.0", ""));
        let p = run_plan(&e, &mr, &cf).await;
        assert_eq!(p.plan.actions.len(), 1, "{:?}", p.plan);
        let a = &p.plan.actions[0];
        assert_eq!((a.kind.as_str(), a.title.as_str(), a.file.as_deref()), ("enable", "Réactiver FABRIC-API", Some("fabric-api-0.110.jar")));
        assert_eq!(a.issue_files, vec!["a.jar"]);
        assert!(p.plan.unresolved.is_empty());
        assert!(mr.received_requests().await.unwrap().is_empty());
        let r = apply(&p, std::slice::from_ref(&a.id), &e.game, &e.store, &e.index, "1.21.1", Loader::Fabric, &|_, _, _| {}).unwrap();
        assert_eq!(r.applied.len(), 1);
        assert!(r.check.issues.is_empty(), "{:?}", r.check);
        assert!(e.game.join("mods/fabric-api-0.110.jar").is_file());
    }

    #[tokio::test]
    async fn installs_missing_dependency_from_requirer_version_dependencies() {
        let (mr, cf) = (MockServer::start().await, MockServer::start().await);
        let e = env();
        put(&e, "a.jar", &fabjar("a", "1.0", r#","depends":{"fabric-api":"*"}"#));
        let api = fabjar("fabric-api", "0.110.0", "");
        let a_version = mr_version(&mr, "va", "AAA", "1.0", "a.jar", b"x", json!([{"project_id": "FAPI", "version_id": null, "dependency_type": "required"}]));
        known_hashes(&mr, json!({ sha1_of(&e, "a.jar"): a_version })).await;
        Mock::given(method("GET")).and(path("/v2/project/FAPI")).respond_with(ResponseTemplate::new(200).set_body_json(json!({"id": "FAPI", "slug": "fabric-api"}))).mount(&mr).await;
        project_versions(&mr, "FAPI", json!([mr_version(&mr, "vf", "FAPI", "0.110.0", "fabric-api-0.110.0.jar", &api, json!([]))])).await;
        serve(&mr, "/cdn/fabric-api-0.110.0.jar", &api).await;
        no_cf_fingerprints(&cf).await;
        let before = listing(&e);
        let p = run_plan(&e, &mr, &cf).await;
        assert_eq!(p.plan.actions.len(), 1, "{:?}", p.plan);
        let a = &p.plan.actions[0];
        assert_eq!((a.kind.as_str(), a.title.as_str(), a.source.as_deref()), ("install", "Installer FABRIC-API 0.110.0 (Modrinth)", Some("modrinth")));
        assert_eq!(a.detail.as_deref(), Some("Requis par A."));
        assert_eq!(a.issue_files, vec!["a.jar"]);
        // Le plan n'a rien écrit dans l'instance.
        assert_eq!(listing(&e), before);

        let r = apply(&p, std::slice::from_ref(&a.id), &e.game, &e.store, &e.index, "1.21.1", Loader::Fabric, &|_, _, _| {}).unwrap();
        assert_eq!(r.applied.len(), 1, "{:?}", r.failed);
        assert!(r.check.issues.is_empty());
        assert_eq!(mods::read_provenance(&e.game)["fabric-api-0.110.0.jar"], Provenance { source: "modrinth".into(), id: "FAPI".into() });
        let bid = r.backup_id.clone().unwrap();
        // `mods::list` ne voit pas le dossier de sauvegarde.
        assert!(mods::list(&e.game).unwrap().iter().all(|m| m.file.ends_with(".jar")));
        undo(&e.game, &bid, &e.index, "1.21.1", Loader::Fabric).unwrap();
        assert_eq!(listing(&e), before);
    }

    #[tokio::test]
    async fn slug_fallback_when_requirer_is_unknown() {
        let (mr, cf) = (MockServer::start().await, MockServer::start().await);
        let e = env();
        put(&e, "a.jar", &fabjar("a", "1.0", r#","depends":{"lib-x":"*"}"#));
        let lib = fabjar("lib-x", "2.0", "");
        known_hashes(&mr, json!({})).await;
        no_cf_fingerprints(&cf).await;
        project_versions(&mr, "lib-x", json!([mr_version(&mr, "vl", "LIBX", "2.0", "lib-x.jar", &lib, json!([]))])).await;
        serve(&mr, "/cdn/lib-x.jar", &lib).await;
        let p = run_plan(&e, &mr, &cf).await;
        assert_eq!(p.plan.actions.len(), 1, "{:?}", p.plan);
        assert_eq!(p.plan.actions[0].title, "Installer LIB-X 2.0 (Modrinth)");
    }

    #[tokio::test]
    async fn candidate_that_does_not_provide_the_id_is_refused() {
        let (mr, cf) = (MockServer::start().await, MockServer::start().await);
        let e = env();
        put(&e, "a.jar", &fabjar("a", "1.0", r#","depends":{"lib-x":"*"}"#));
        let wrong = fabjar("something-else", "2.0", "");
        known_hashes(&mr, json!({})).await;
        no_cf_fingerprints(&cf).await;
        project_versions(&mr, "lib-x", json!([mr_version(&mr, "vl", "LIBX", "2.0", "x.jar", &wrong, json!([]))])).await;
        serve(&mr, "/cdn/x.jar", &wrong).await;
        let p = run_plan(&e, &mr, &cf).await;
        assert!(p.plan.actions.is_empty(), "{:?}", p.plan);
        assert_eq!(p.plan.unresolved.len(), 1);
        assert_eq!(p.plan.unresolved[0].reason, "Aucune version compatible avec Minecraft 1.21.1 (fabric) sur Modrinth ni CurseForge.");
        assert_eq!(p.plan.unresolved[0].file, "a.jar");
    }

    #[tokio::test]
    async fn updates_too_old_provider_and_roundtrips() {
        let (mr, cf) = (MockServer::start().await, MockServer::start().await);
        let e = env();
        put(&e, "a.jar", &fabjar("a", "1.0", r#","depends":{"flywheel":">=1.0"}"#));
        put(&e, "fly-0.9.jar", &fabjar("flywheel", "0.9", ""));
        mods::record_provenance(&e.game, &[("fly-0.9.jar".into(), Provenance { source: "modrinth".into(), id: "FLY".into() })]).unwrap();
        let new = fabjar("flywheel", "1.2", "");
        known_hashes(&mr, json!({})).await;
        no_cf_fingerprints(&cf).await;
        project_versions(&mr, "FLY", json!([mr_version(&mr, "v2", "FLY", "1.2", "fly-1.2.jar", &new, json!([]))])).await;
        serve(&mr, "/cdn/fly-1.2.jar", &new).await;
        let before = listing(&e);
        let p = run_plan(&e, &mr, &cf).await;
        assert_eq!(p.plan.actions.len(), 1, "{:?}", p.plan);
        let a = &p.plan.actions[0];
        assert_eq!((a.kind.as_str(), a.title.as_str(), a.file.as_deref()), ("update", "Mettre à jour FLYWHEEL vers 1.2 (Modrinth)", Some("fly-0.9.jar")));
        let r = apply(&p, std::slice::from_ref(&a.id), &e.game, &e.store, &e.index, "1.21.1", Loader::Fabric, &|_, _, _| {}).unwrap();
        assert!(r.check.issues.is_empty(), "{:?}", r.check);
        assert!(!e.game.join("mods/fly-0.9.jar").exists() && e.game.join("mods/fly-1.2.jar").is_file());
        assert!(!mods::read_provenance(&e.game).contains_key("fly-0.9.jar"));
        assert!(e.game.join("mods").join(BACKUP_DIR).join(r.backup_id.clone().unwrap()).join("fly-0.9.jar").is_file());
        let c = undo(&e.game, &r.backup_id.unwrap(), &e.index, "1.21.1", Loader::Fabric).unwrap();
        assert_eq!(c.issues.len(), 1);
        assert_eq!(listing(&e), before);
        assert_eq!(mods::read_provenance(&e.game)["fly-0.9.jar"].id, "FLY");
    }

    /// Cas réel (Iris / Sodium sur Minecraft 26.1) : la dépendance n'a pas de version assez récente pour ce Minecraft,
    /// alors on prend une version du mod qui l'exige qui accepte la dépendance installée.
    #[tokio::test]
    async fn downgrades_requirer_when_provider_cannot_be_updated() {
        let (mr, cf) = (MockServer::start().await, MockServer::start().await);
        let e = env();
        let iris_new = fabjar("iris", "1.11.4", r#","depends":{"sodium":"0.9.x"}"#);
        put(&e, "iris-1.11.4.jar", &iris_new);
        put(&e, "sodium-0.8.9.jar", &fabjar("sodium", "0.8.9", ""));
        mods::record_provenance(&e.game, &[
            ("iris-1.11.4.jar".into(), Provenance { source: "modrinth".into(), id: "IRIS".into() }),
            ("sodium-0.8.9.jar".into(), Provenance { source: "modrinth".into(), id: "SOD".into() }),
        ]).unwrap();
        known_hashes(&mr, json!({})).await;
        no_cf_fingerprints(&cf).await;
        // Sodium : rien de plus récent pour ce Minecraft (la seule version est celle installée).
        project_versions(&mr, "SOD", json!([mr_version(&mr, "s1", "SOD", "0.8.9", "sodium-0.8.9.jar", b"x", json!([]))])).await;
        let iris_old = fabjar("iris", "1.10.9", r#","depends":{"sodium":"0.8.x"}"#);
        project_versions(&mr, "IRIS", json!([
            mr_version(&mr, "i2", "IRIS", "1.11.4", "iris-1.11.4.jar", &iris_new, json!([])),
            mr_version(&mr, "i1", "IRIS", "1.10.9", "iris-1.10.9.jar", &iris_old, json!([])),
        ])).await;
        serve(&mr, "/cdn/iris-1.10.9.jar", &iris_old).await;
        let p = run_plan(&e, &mr, &cf).await;
        assert_eq!(p.plan.actions.len(), 1, "{:?}", p.plan);
        assert!(p.plan.unresolved.is_empty(), "{:?}", p.plan.unresolved);
        let a = &p.plan.actions[0];
        assert_eq!((a.kind.as_str(), a.title.as_str(), a.file.as_deref()), ("update", "Revenir à IRIS 1.10.9 (Modrinth)", Some("iris-1.11.4.jar")));
        let r = apply(&p, std::slice::from_ref(&a.id), &e.game, &e.store, &e.index, "1.21.1", Loader::Fabric, &|_, _, _| {}).unwrap();
        assert!(r.check.issues.is_empty(), "{:?}", r.check);
        assert!(e.game.join("mods/iris-1.10.9.jar").is_file() && e.game.join("mods/sodium-0.8.9.jar").is_file());
    }

    async fn check_iris(e: &Env, mr: &MockServer, cf: &MockServer, only: Option<&str>) -> ContentCheck {
        check_content(&providers(mr, cf), "1.21.1", Loader::Fabric, &e.game, &e.store, &e.index, &dlo(), BrowseSource::Modrinth, "IRIS", only).await.unwrap()
    }

    /// Fiche Parcourir : la version proposée par défaut est la plus récente qui va avec le pack, et le mod déjà
    /// présent (même sans provenance, d'après son id) est reconnu.
    #[tokio::test]
    async fn content_check_recommends_compatible_version_and_sees_installed() {
        let (mr, cf) = (MockServer::start().await, MockServer::start().await);
        let e = env();
        put(&e, "sodium-0.8.9.jar", &fabjar("sodium", "0.8.9", ""));
        put(&e, "iris-old.jar.disabled", &fabjar("iris", "1.9.0", r#","depends":{"sodium":"0.8.x"}"#));
        let new = fabjar("iris", "1.11.4", r#","depends":{"sodium":"0.9.x"}"#);
        let old = fabjar("iris", "1.10.9", r#","depends":{"sodium":"0.8.x"}"#);
        project_versions(&mr, "IRIS", json!([
            mr_version(&mr, "i2", "IRIS", "1.11.4", "iris-1.11.4.jar", &new, json!([])),
            mr_version(&mr, "i1", "IRIS", "1.10.9", "iris-1.10.9.jar", &old, json!([])),
        ])).await;
        serve(&mr, "/cdn/iris-1.11.4.jar", &new).await;
        serve(&mr, "/cdn/iris-1.10.9.jar", &old).await;
        let c = check_iris(&e, &mr, &cf, None).await;
        assert_eq!(c.recommended.as_deref(), Some("i1"), "{c:?}");
        assert_eq!(c.installed, vec!["iris-old.jar"]);
        assert!(!c.verdicts[0].ok && c.verdicts[0].problems[0].to_lowercase().contains("sodium"), "{c:?}");
        assert!(c.blockers.is_empty());
        // Version précise demandée : verdict de celle-là seulement.
        let c = check_iris(&e, &mr, &cf, Some("i2")).await;
        assert_eq!((c.verdicts.len(), c.verdicts[0].ok, c.recommended.is_none()), (1, false, true));
    }

    /// Un mod déjà présent incompatible avec toutes les versions : aucune recommandée, blocage expliqué.
    #[tokio::test]
    async fn content_check_reports_blockers() {
        let (mr, cf) = (MockServer::start().await, MockServer::start().await);
        let e = env();
        put(&e, "embeddium.jar", &fabjar("embeddium", "1.0", ""));
        let iris = fabjar("iris", "1.11.4", r#","breaks":{"embeddium":"*"}"#);
        project_versions(&mr, "IRIS", json!([mr_version(&mr, "i2", "IRIS", "1.11.4", "iris-1.11.4.jar", &iris, json!([]))])).await;
        serve(&mr, "/cdn/iris-1.11.4.jar", &iris).await;
        let c = check_iris(&e, &mr, &cf, None).await;
        assert!(c.recommended.is_none() && c.installed.is_empty(), "{c:?}");
        assert_eq!(c.blockers.len(), 1, "{c:?}");
        assert!(c.blockers[0].contains("EMBEDDIUM"), "{c:?}");
    }

    /// Une mise à jour du fournisseur qui casserait un autre mod du pack est écartée (simulation dans le pack).
    #[tokio::test]
    async fn provider_update_that_breaks_another_mod_is_refused() {
        let (mr, cf) = (MockServer::start().await, MockServer::start().await);
        let e = env();
        let a_new = fabjar("a", "2.0", r#","depends":{"flywheel":">=1.0"}"#);
        put(&e, "a-2.0.jar", &a_new);
        put(&e, "b.jar", &fabjar("b", "1.0", r#","depends":{"flywheel":"<1.0"}"#));
        put(&e, "fly-0.9.jar", &fabjar("flywheel", "0.9", ""));
        mods::record_provenance(&e.game, &[
            ("a-2.0.jar".into(), Provenance { source: "modrinth".into(), id: "AAA".into() }),
            ("fly-0.9.jar".into(), Provenance { source: "modrinth".into(), id: "FLY".into() }),
        ]).unwrap();
        known_hashes(&mr, json!({})).await;
        no_cf_fingerprints(&cf).await;
        let fly_new = fabjar("flywheel", "1.2", "");
        project_versions(&mr, "FLY", json!([mr_version(&mr, "f2", "FLY", "1.2", "fly-1.2.jar", &fly_new, json!([]))])).await;
        serve(&mr, "/cdn/fly-1.2.jar", &fly_new).await;
        let a_old = fabjar("a", "1.5", r#","depends":{"flywheel":">=0.9"}"#);
        project_versions(&mr, "AAA", json!([
            mr_version(&mr, "a2", "AAA", "2.0", "a-2.0.jar", &a_new, json!([])),
            mr_version(&mr, "a1", "AAA", "1.5", "a-1.5.jar", &a_old, json!([])),
        ])).await;
        serve(&mr, "/cdn/a-1.5.jar", &a_old).await;
        let p = run_plan(&e, &mr, &cf).await;
        assert_eq!(p.plan.actions.len(), 1, "{:?}", p.plan);
        assert_eq!(p.plan.actions[0].title, "Revenir à A 1.5 (Modrinth)");
        let r = apply(&p, &[p.plan.actions[0].id.clone()], &e.game, &e.store, &e.index, "1.21.1", Loader::Fabric, &|_, _, _| {}).unwrap();
        assert!(r.check.issues.is_empty(), "{:?}", r.check);
    }

    #[tokio::test]
    async fn disables_older_duplicate_without_network() {
        let (mr, cf) = (MockServer::start().await, MockServer::start().await);
        let e = env();
        put(&e, "a-new.jar", &fabjar("same", "2.0", ""));
        put(&e, "a-old.jar", &fabjar("same", "1.0", ""));
        let before = listing(&e);
        let p = run_plan(&e, &mr, &cf).await;
        assert_eq!(p.plan.actions.len(), 1, "{:?}", p.plan);
        let a = &p.plan.actions[0];
        // `a-new.jar` vient avant `a-old.jar` par ordre de nom : la référence de `check` est la plus récente ici.
        assert_eq!((a.kind.as_str(), a.file.as_deref(), a.title.as_str()), ("disable", Some("a-old.jar"), "Désactiver l'ancien exemplaire de SAME"));
        assert!(mr.received_requests().await.unwrap().is_empty() && cf.received_requests().await.unwrap().is_empty());
        let r = apply(&p, std::slice::from_ref(&a.id), &e.game, &e.store, &e.index, "1.21.1", Loader::Fabric, &|_, _, _| {}).unwrap();
        assert!(r.check.issues.is_empty());
        assert!(e.game.join("mods/a-old.jar.disabled").is_file());
        undo(&e.game, &r.backup_id.unwrap(), &e.index, "1.21.1", Loader::Fabric).unwrap();
        assert_eq!(listing(&e), before);
    }

    #[tokio::test]
    async fn incompatible_is_never_automatic() {
        let (mr, cf) = (MockServer::start().await, MockServer::start().await);
        let e = env();
        put(&e, "a.jar", &fabjar("a", "1.0", r#","breaks":{"b":"*"}"#));
        put(&e, "b.jar", &fabjar("b", "1.0", ""));
        let p = run_plan(&e, &mr, &cf).await;
        assert!(p.plan.actions.is_empty());
        assert_eq!(p.plan.unresolved.len(), 1);
        assert_eq!(p.plan.unresolved[0].reason, "Incompatibilité : choisis toi-même le mod à désactiver.");
    }

    #[tokio::test]
    async fn curseforge_non_distributable_is_unresolved_with_page() {
        let (mr, cf) = (MockServer::start().await, MockServer::start().await);
        let e = env();
        put(&e, "a.jar", &fabjar("a", "1.0", r#","depends":{"closed-lib":"*"}"#));
        known_hashes(&mr, json!({})).await;
        no_cf_fingerprints(&cf).await;
        Mock::given(method("GET")).and(path("/v1/mods/search")).respond_with(ResponseTemplate::new(200).set_body_json(json!({"data": [
            {"id": 5, "name": "Closed", "slug": "closed-lib"}], "pagination": {"totalCount": 1}}))).mount(&cf).await;
        Mock::given(method("GET")).and(path("/v1/mods/5/files")).respond_with(ResponseTemplate::new(200).set_body_json(json!({"data": [
            {"id": 50, "modId": 5, "fileName": "closed.jar", "fileDate": "2024-01-01T00:00:00Z", "downloadUrl": null, "gameVersions": ["1.21.1", "Fabric"]}]}))).mount(&cf).await;
        Mock::given(method("GET")).and(path("/v1/mods/5")).respond_with(ResponseTemplate::new(200).set_body_json(json!({"data": {"id": 5, "name": "Closed", "slug": "closed-lib",
            "links": {"websiteUrl": "https://www.curseforge.com/minecraft/mc-mods/closed-lib"}}}))).mount(&cf).await;
        let p = run_plan(&e, &mr, &cf).await;
        assert!(p.plan.actions.is_empty(), "{:?}", p.plan);
        let u = &p.plan.unresolved[0];
        assert_eq!(u.reason, "L'auteur interdit le téléchargement par les applications (CurseForge).");
        assert_eq!(u.page_url.as_deref(), Some("https://www.curseforge.com/minecraft/mc-mods/closed-lib"));
    }

    #[tokio::test]
    async fn installs_from_curseforge_when_modrinth_has_nothing() {
        let (mr, cf) = (MockServer::start().await, MockServer::start().await);
        let e = env();
        put(&e, "a.jar", &fabjar("a", "1.0", r#","depends":{"lib-y":"*"}"#));
        known_hashes(&mr, json!({})).await;
        no_cf_fingerprints(&cf).await;
        let lib = fabjar("lib-y", "3.0", "");
        Mock::given(method("GET")).and(path("/v1/mods/search")).respond_with(ResponseTemplate::new(200).set_body_json(json!({"data": [{"id": 7, "name": "Y", "slug": "lib-y"}]}))).mount(&cf).await;
        Mock::given(method("GET")).and(path("/v1/mods/7/files")).respond_with(ResponseTemplate::new(200).set_body_json(json!({"data": [
            {"id": 70, "modId": 7, "displayName": "Lib Y 3.0", "fileName": "lib-y.jar", "fileDate": "2024-01-01T00:00:00Z", "fileLength": lib.len(),
             "downloadUrl": format!("{}/dl/lib-y.jar", cf.uri()), "gameVersions": ["1.21.1", "Fabric"], "hashes": []}]}))).mount(&cf).await;
        serve(&cf, "/dl/lib-y.jar", &lib).await;
        let p = run_plan(&e, &mr, &cf).await;
        assert_eq!(p.plan.actions.len(), 1, "{:?}", p.plan);
        assert_eq!((p.plan.actions[0].title.as_str(), p.plan.actions[0].source.as_deref()), ("Installer LIB-Y 3.0 (CurseForge)", Some("curseforge")));
        let r = apply(&p, &[p.plan.actions[0].id.clone()], &e.game, &e.store, &e.index, "1.21.1", Loader::Fabric, &|_, _, _| {}).unwrap();
        assert!(r.check.issues.is_empty(), "{:?}", r.check);
        assert_eq!(mods::read_provenance(&e.game)["lib-y.jar"], Provenance { source: "curseforge".into(), id: "7".into() });
    }

    #[tokio::test]
    async fn partial_apply_failure_and_empty_backup() {
        let (mr, cf) = (MockServer::start().await, MockServer::start().await);
        let e = env();
        put(&e, "a-new.jar", &fabjar("same", "2.0", ""));
        put(&e, "a-old.jar", &fabjar("same", "1.0", ""));
        let p = run_plan(&e, &mr, &cf).await;
        // Le fichier disparaît entre le plan et l'application : l'action échoue, rien n'est sauvegardé.
        std::fs::remove_file(e.game.join("mods/a-old.jar")).unwrap();
        let r = apply(&p, &[p.plan.actions[0].id.clone()], &e.game, &e.store, &e.index, "1.21.1", Loader::Fabric, &|_, _, _| {}).unwrap();
        assert_eq!((r.applied.len(), r.failed.len()), (0, 1));
        assert!(r.backup_id.is_none());
        assert!(!e.game.join("mods").join(BACKUP_DIR).exists() || std::fs::read_dir(e.game.join("mods").join(BACKUP_DIR)).unwrap().count() == 0);
    }

    #[test]
    fn fingerprint_changes_with_mods_and_ignores_backups() {
        let e = env();
        put(&e, "a.jar", b"1");
        let f1 = mods_fingerprint(&e.game);
        assert_eq!(f1, mods_fingerprint(&e.game));
        std::fs::create_dir_all(e.game.join("mods").join(BACKUP_DIR).join("x")).unwrap();
        assert_eq!(f1, mods_fingerprint(&e.game));
        put(&e, "b.jar", b"2");
        assert_ne!(f1, mods_fingerprint(&e.game));
    }

    #[test]
    fn keeps_only_five_backups_and_validates_ids() {
        let e = env();
        for i in 0..8 {
            let d = backup_root(&e.game).join(format!("{i:03}"));
            write_manifest(&d, &Manifest { id: format!("{i:03}"), ..Default::default() }).unwrap();
        }
        prune_backups(&e.game);
        let left: Vec<String> = std::fs::read_dir(backup_root(&e.game)).unwrap().flatten().map(|e| e.file_name().into_string().unwrap()).collect();
        assert_eq!(left.len(), 5);
        assert!(!left.contains(&"000".to_string()) && left.contains(&"007".to_string()));
        assert!(undo(&e.game, "../x", &e.index, "1.21.1", Loader::Fabric).is_err());
        assert!(matches!(undo(&e.game, "nope", &e.index, "1.21.1", Loader::Fabric), Err(Error::NotFound(_))));
    }

    /// Réseau réel, copie TEMPORAIRE d'un pack (jamais d'écriture dans APPDATA) : Fabric API retirée, le plan doit
    /// proposer de l'installer. `cargo test -p cubelink-core --lib -- --ignored --nocapture plan_reel_fabric_api_retiree`.
    #[tokio::test]
    #[ignore = "réseau réel"]
    async fn plan_reel_fabric_api_retiree() {
        let Some(appdata) = std::env::var_os("APPDATA") else { return };
        let src = PathBuf::from(appdata).join("app.cubelink.launcher").join("instances").join("fabulously-optimized");
        let inst: crate::types::Instance = serde_json::from_str(&std::fs::read_to_string(src.join("instance.json")).unwrap()).unwrap();
        let e = env();
        let mut removed = Vec::new();
        for m in mods::list(&src.join("minecraft")).unwrap() {
            let from = src.join("minecraft/mods").join(&m.file);
            let is_api = m.enabled && jar_meta(&from).is_some_and(|meta| meta.mods.iter().any(|x| x.id == "fabric-api"));
            if is_api {
                removed.push(m.file);
                continue;
            }
            std::fs::copy(&from, e.game.join("mods").join(&m.file)).unwrap();
        }
        println!("== {} ({} {:?}) : retiré {:?}", inst.name, inst.minecraft, inst.loader.loader, removed);
        assert!(!removed.is_empty(), "Fabric API absente de l'instance d'origine");
        let p = Providers { modrinth: ModrinthClient::new("cubelink-test"), cf: CfApi::new(), allow_loopback: false };
        let t = std::time::Instant::now();
        let prep = plan(&p, &inst.minecraft, inst.loader.loader, &e.game, &e.store, &e.index, &dlo(), &|d, t, l| println!("   [{d}/{t}] {l}")).await.unwrap();
        println!("plan en {:?}", t.elapsed());
        for a in &prep.plan.actions {
            println!("ACTION {} [{}] {} | {:?} | {:?}", a.id, a.kind, a.title, a.detail, a.issue_files);
        }
        for u in &prep.plan.unresolved {
            println!("NON RESOLU {} ({}) : {} -> {} {:?}", u.mod_name, u.file, u.message, u.reason, u.page_url);
        }
        assert!(prep.plan.actions.iter().any(|a| a.kind == "install" && a.title.starts_with("Installer Fabric API")), "{:?}", prep.plan);
        let ids: Vec<String> = prep.plan.actions.iter().map(|a| a.id.clone()).collect();
        let r = apply(&prep, &ids, &e.game, &e.store, &e.index, &inst.minecraft, inst.loader.loader, &|_, _, _| {}).unwrap();
        println!("appliqué : {:?} ; échecs : {:?} ; problèmes restants : {}", r.applied, r.failed, r.check.issues.len());
        for i in &r.check.issues {
            println!("   [{}] {} : {}", i.kind, i.mod_name, i.message);
        }
    }

    /// Réseau réel sur une copie TEMPORAIRE de l'instance `CUBELINK_FIX_INSTANCE` (dossier sous APPDATA, jamais modifié) :
    /// imprime le plan puis l'applique à la copie.
    /// `CUBELINK_FIX_INSTANCE=trop-styl cargo test -p cubelink-core --lib -- --ignored --nocapture plan_reel_instance`.
    #[tokio::test]
    #[ignore = "réseau réel"]
    async fn plan_reel_instance() {
        let (Some(appdata), Ok(name)) = (std::env::var_os("APPDATA"), std::env::var("CUBELINK_FIX_INSTANCE")) else { return };
        let src = PathBuf::from(appdata).join("app.cubelink.launcher").join("instances").join(name);
        let inst: crate::types::Instance = serde_json::from_str(&std::fs::read_to_string(src.join("instance.json")).unwrap()).unwrap();
        let e = env();
        for m in mods::list(&src.join("minecraft")).unwrap() {
            std::fs::copy(src.join("minecraft/mods").join(&m.file), e.game.join("mods").join(&m.file)).unwrap();
        }
        let before = mod_check::check(&e.game, &inst.minecraft, loader_str(inst.loader.loader)).unwrap();
        println!("== {} ({} {:?}) : {} problème(s) au départ", inst.name, inst.minecraft, inst.loader.loader, before.issues.len());
        for i in &before.issues {
            println!("   [{}] {} : {}", i.kind, i.mod_name, i.message);
        }
        let p = Providers { modrinth: ModrinthClient::new("cubelink-test"), cf: CfApi::new(), allow_loopback: false };
        let t = std::time::Instant::now();
        let prep = plan(&p, &inst.minecraft, inst.loader.loader, &e.game, &e.store, &e.index, &dlo(), &|d, t, l| println!("   [{d}/{t}] {l}")).await.unwrap();
        println!("plan en {:?}", t.elapsed());
        for a in &prep.plan.actions {
            println!("ACTION {} [{}] {} | {:?}", a.id, a.kind, a.title, a.detail);
        }
        for u in &prep.plan.unresolved {
            println!("NON RESOLU {} ({}) : {} -> {}", u.mod_name, u.file, u.message, u.reason);
        }
        let ids: Vec<String> = prep.plan.actions.iter().map(|a| a.id.clone()).collect();
        let r = apply(&prep, &ids, &e.game, &e.store, &e.index, &inst.minecraft, inst.loader.loader, &|_, _, _| {}).unwrap();
        println!("appliqué : {:?} ; échecs : {:?} ; problèmes restants : {}", r.applied, r.failed, r.check.issues.len());
        for i in &r.check.issues {
            println!("   [{}] {} : {}", i.kind, i.mod_name, i.message);
        }
    }

    /// Réseau réel : compatibilité du projet Modrinth `CUBELINK_CHECK_PROJECT` avec l'instance `CUBELINK_FIX_INSTANCE`
    /// (lecture seule de l'instance ; les jars candidats vont dans un magasin temporaire).
    #[tokio::test]
    #[ignore = "réseau réel"]
    async fn content_check_reel() {
        let (Some(appdata), Ok(name), Ok(project)) = (std::env::var_os("APPDATA"), std::env::var("CUBELINK_FIX_INSTANCE"), std::env::var("CUBELINK_CHECK_PROJECT")) else { return };
        let src = PathBuf::from(appdata).join("app.cubelink.launcher").join("instances").join(name);
        let inst: crate::types::Instance = serde_json::from_str(&std::fs::read_to_string(src.join("instance.json")).unwrap()).unwrap();
        let e = env();
        let p = Providers { modrinth: ModrinthClient::new("cubelink-test"), cf: CfApi::new(), allow_loopback: false };
        let t = std::time::Instant::now();
        let c = check_content(&p, &inst.minecraft, inst.loader.loader, &src.join("minecraft"), &e.store, &e.index, &dlo(), BrowseSource::Modrinth, &project, None).await.unwrap();
        println!("en {:?} : {c:#?}", t.elapsed());
    }
}

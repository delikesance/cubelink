//! Suivi REEL du demarrage d'un jeu, a partir de sa sortie (journal) : phase courante, compteurs, derniere activite, et
//! estimation du temps restant APPRISE des lancements precedents du meme modpack. Aucune dependance systeme : le meme code
//! servira sur toutes les plateformes (le launcher lui passe les lignes de `LaunchEvent::Output`).

use std::collections::BTreeMap;
use std::path::Path;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

/// Etapes d'un demarrage, dans l'ordre. Une etape ne peut qu'avancer.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum Phase {
    Java,
    Scan,
    Mixin,
    Bootstrap,
    ModLoad,
    Window,
    Resources,
    Textures,
    /// Ouverture d'un monde (lancement direct dans un monde) : visible seulement s'il se produit.
    World,
    Ready,
}

impl Phase {
    pub const ALL: [Phase; 10] = [Phase::Java, Phase::Scan, Phase::Mixin, Phase::Bootstrap, Phase::ModLoad, Phase::Window, Phase::Resources, Phase::Textures, Phase::World, Phase::Ready];

    pub fn label(self) -> &'static str {
        match self {
            Phase::Java => "Démarrage de Java",
            Phase::Scan => "Recherche des mods",
            Phase::Mixin => "Préparation du code des mods",
            Phase::Bootstrap => "Initialisation du jeu",
            Phase::ModLoad => "Chargement des mods",
            Phase::Window => "Ouverture de la fenêtre",
            Phase::Resources => "Chargement des ressources",
            Phase::Textures => "Textures et son",
            Phase::World => "Chargement du monde",
            Phase::Ready => "Prêt",
        }
    }

    /// Phrase de la ligne principale quand l'etape est en cours.
    pub fn doing(self) -> &'static str {
        match self {
            Phase::Java => "Java démarre…",
            Phase::Scan => "Cubelink repère les mods installés…",
            Phase::Mixin => "Les mods adaptent le code du jeu…",
            Phase::Bootstrap => "Le jeu s'initialise…",
            Phase::ModLoad => "Les mods se chargent…",
            Phase::Window => "La fenêtre du jeu s'ouvre…",
            Phase::Resources => "Textures, modèles et sons sont lus…",
            Phase::Textures => "Les textures sont assemblées…",
            Phase::World => "Le monde se prépare…",
            Phase::Ready => "C'est prêt !",
        }
    }

    fn key(self) -> &'static str {
        match self {
            Phase::Java => "java",
            Phase::Scan => "scan",
            Phase::Mixin => "mixin",
            Phase::Bootstrap => "bootstrap",
            Phase::ModLoad => "modload",
            Phase::Window => "window",
            Phase::Resources => "resources",
            Phase::Textures => "textures",
            Phase::World => "world",
            Phase::Ready => "ready",
        }
    }

    /// Etapes qui n'existent que pour un jeu avec mods.
    pub fn modded_only(self) -> bool {
        matches!(self, Phase::Scan | Phase::Mixin | Phase::ModLoad)
    }
}

/// Historique d'un modpack : temps de debut de chaque etape (ms depuis le lancement), compteurs, total.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Profile {
    pub total_ms: u64,
    pub starts_ms: BTreeMap<String, u64>,
    pub counts: BTreeMap<String, u32>,
    pub runs: u32,
}

impl Profile {
    pub fn load(path: &Path) -> Option<Profile> {
        serde_json::from_str(&std::fs::read_to_string(path).ok()?).ok()
    }

    pub fn save(&self, path: &Path) {
        if let Some(d) = path.parent() {
            let _ = std::fs::create_dir_all(d);
        }
        let _ = std::fs::write(path, serde_json::to_string_pretty(self).unwrap_or_default());
    }
}

/// Ce que le rideau affiche, a un instant donne.
#[derive(Clone, Debug)]
pub struct Snapshot {
    pub phase: Phase,
    /// Ligne principale (« Les mods se chargent… »).
    pub doing: String,
    /// Fermeture propre d'un jeu (et non lancement).
    pub closing: bool,
    /// Etapes affichees (selon jeu vanilla / avec mods), avec etat.
    pub stages: Vec<StageView>,
    /// Avancement global 0..1 (monotone).
    pub fraction: f32,
    pub elapsed: Duration,
    /// Temps restant estime d'apres les lancements precedents ; `None` au premier lancement.
    pub remaining: Option<Duration>,
    /// Plus long que d'habitude pour l'etape en cours.
    pub overdue: bool,
    pub first_run: bool,
    /// Derniere activite du jeu (nom de mod, fichier...), si recente.
    pub activity: Option<String>,
    /// Temps depuis la derniere ligne du jeu.
    pub silent_for: Duration,
    pub detail: Option<String>,
}

#[derive(Clone, Debug)]
pub struct StageView {
    pub phase: Phase,
    pub label: String,
    pub done: bool,
    pub current: bool,
    /// Duree reelle de l'etape terminee.
    pub took: Option<Duration>,
    /// Compteur de l'etape ("48 / 147").
    pub counter: Option<String>,
}

pub struct Tracker {
    start: Instant,
    modded: bool,
    mods_in_dir: u32,
    phase: Phase,
    starts: BTreeMap<Phase, Duration>,
    counts: BTreeMap<Phase, u32>,
    totals: BTreeMap<Phase, u32>,
    prev: Option<Profile>,
    last_line: Instant,
    activity: Option<(String, Instant)>,
    shown: f32,
    ready_marker: bool,
    window_seen: bool,
}

fn between<'a>(s: &'a str, a: &str, b: &str) -> Option<&'a str> {
    let i = s.find(a)? + a.len();
    let j = s[i..].find(b)? + i;
    Some(&s[i..j])
}

impl Tracker {
    /// `mods_in_dir` : nombre de jars dans le dossier des mods (0 = jeu sans mods).
    pub fn new(mods_in_dir: u32, prev: Option<Profile>) -> Tracker {
        let now = Instant::now();
        Tracker {
            start: now,
            modded: mods_in_dir > 0,
            mods_in_dir,
            phase: Phase::Java,
            starts: BTreeMap::from([(Phase::Java, Duration::ZERO)]),
            counts: BTreeMap::new(),
            totals: BTreeMap::new(),
            prev,
            last_line: now,
            activity: None,
            shown: 0.0,
            ready_marker: false,
            window_seen: false,
        }
    }

    pub fn phase(&self) -> Phase {
        self.phase
    }

    fn advance(&mut self, to: Phase) {
        if to > self.phase {
            let t = self.start.elapsed();
            // les etapes sautees commencent (et finissent) maintenant
            for p in Phase::ALL {
                if p > self.phase && p <= to {
                    self.starts.entry(p).or_insert(t);
                }
            }
            self.phase = to;
        }
    }

    fn bump(&mut self, p: Phase) {
        *self.counts.entry(p).or_insert(0) += 1;
    }

    /// Evenement hote : la fenetre du jeu est apparue (adoptee).
    pub fn window_shown(&mut self) {
        self.window_seen = true;
        self.advance(Phase::Window);
    }

    pub fn mark_ready(&mut self) {
        self.advance(Phase::Ready);
    }

    /// Comme `feed_line`, pour un bruit repete : la ligne est analysee mais ne compte pas comme de l'activite (la quietude de
    /// `looks_ready` ne repart pas de zero).
    pub fn feed_line_quiet(&mut self, line: &str) {
        let last = self.last_line;
        self.feed_line(line);
        self.last_line = last;
    }

    /// Temps ecoule depuis la derniere ligne utile.
    pub fn silent_for(&self) -> Duration {
        self.last_line.elapsed()
    }

    /// Analyse une ligne de la sortie du jeu.
    pub fn feed_line(&mut self, line: &str) {
        self.last_line = Instant::now();
        let l = line.trim_end();
        if l.contains("[Cubelink] READY") {
            self.ready_marker = true;
            self.advance(Phase::Ready);
            return;
        }
        // --- mods : recherche
        if l.contains("Found mod file") || l.contains("Found library file") || l.contains("Found gamelibrary file") {
            self.advance(Phase::Scan);
            if l.contains("Found mod file") {
                self.bump(Phase::Scan);
            }
            if let Some(n) = between(l, "Found mod file \"", "\"").or_else(|| between(l, "Found library file \"", "\"")) {
                self.activity = Some((n.to_string(), Instant::now()));
            }
            return;
        }
        if let Some(rest) = l.split("]: Loading ").nth(1) {
            // Fabric / Quilt : « Loading 147 mods: »
            if let Some(n) = rest.split_whitespace().next().and_then(|x| x.parse::<u32>().ok()) {
                if rest.contains("mods") {
                    self.advance(Phase::Scan);
                    self.totals.insert(Phase::Scan, n);
                    self.counts.insert(Phase::Scan, n);
                    return;
                }
            }
        }
        if l.contains("Vanilla bootstrap took") || l.contains("Setting user:") || l.contains("[minecraft/Bootstrap]") || l.contains("Loading Minecraft ") {
            self.advance(Phase::Bootstrap);
            return;
        }
        let low = l.to_ascii_lowercase();
        if low.contains("mixin") {
            if self.phase < Phase::Bootstrap {
                self.advance(Phase::Mixin);
                if let Some(n) = between(l, "Reference map '", "'") {
                    self.activity = Some((n.to_string(), Instant::now()));
                    self.bump(Phase::Mixin);
                }
            }
            return;
        }
        if l.contains("[modloading-") {
            self.advance(Phase::ModLoad);
            if let Some(name) = between(l, "/INFO] [", "/]") {
                self.activity = Some((name.to_string(), Instant::now()));
            }
            self.bump(Phase::ModLoad);
            return;
        }
        if l.contains("Backend library:") || l.contains("Using SDL") {
            self.advance(Phase::Window);
            return;
        }
        if l.contains("Reloading ResourceManager") {
            self.advance(Phase::Resources);
            return;
        }
        if l.contains("joined the game") {
            // le joueur est dans le monde : le jeu est pret
            self.advance(Phase::World);
            self.ready_marker = true;
            self.advance(Phase::Ready);
            return;
        }
        if l.contains("Preparing spawn area") || l.contains("Starting integrated server") || l.contains("chunks for player spawn") || l.contains("Preparing level") {
            self.advance(Phase::World);
            if let Some(p) = between(l, "area: ", "%").and_then(|x| x.trim().parse::<u32>().ok()) {
                self.counts.insert(Phase::World, p);
                self.totals.insert(Phase::World, 100);
            }
            return;
        }
        if l.contains("TextureAtlas") || (l.contains("Created:") && l.contains("atlas")) || l.contains("Sound engine started") || l.contains("OpenAL initialized") {
            self.advance(Phase::Textures);
            if l.contains("Created:") {
                self.bump(Phase::Textures);
                if let Some(n) = l.split_whitespace().last() {
                    self.activity = Some((n.trim_end_matches("-atlas").trim_end_matches(".png").to_string(), Instant::now()));
                }
            }
            return;
        }
        if self.phase >= Phase::ModLoad && l.contains("Registering") {
            self.bump(Phase::ModLoad);
        }
    }

    // ---- estimation

    fn default_starts(&self) -> BTreeMap<Phase, f32> {
        // secondes depuis le lancement ; a defaut d'historique, un profil « plausible » selon la taille du modpack
        let k = if self.modded { 1.0 + (self.mods_in_dir as f32 / 40.0).min(6.0) } else { 1.0 };
        let mut t = 0.0;
        let mut m = BTreeMap::new();
        for p in Phase::ALL {
            m.insert(p, t);
            t += match p {
                Phase::Java => 1.8,
                Phase::Scan => 1.0 * k,
                Phase::Mixin => 2.0 * k,
                Phase::Bootstrap => 2.0 * k,
                Phase::ModLoad => 3.0 * k,
                Phase::Window => 1.2,
                Phase::Resources => 2.5 * k,
                Phase::Textures => 2.5 * k,
                Phase::World => 3.0,
                Phase::Ready => 0.0,
            };
        }
        m
    }

    fn predicted_starts(&self) -> BTreeMap<Phase, f32> {
        let mut out = self.default_starts();
        if let Some(p) = &self.prev {
            if p.total_ms > 0 {
                // l'historique remplace les valeurs par defaut ; les etapes inconnues du profil sont interpolees
                let mut last = 0.0;
                for ph in Phase::ALL {
                    if let Some(ms) = p.starts_ms.get(ph.key()) {
                        last = *ms as f32 / 1000.0;
                    }
                    out.insert(ph, last);
                }
                out.insert(Phase::Ready, p.total_ms as f32 / 1000.0);
            }
        }
        out
    }

    fn shown_phases(&self) -> Vec<Phase> {
        Phase::ALL.into_iter().filter(|p| (self.modded || !p.modded_only()) && (*p != Phase::World || self.phase >= Phase::World)).collect()
    }

    /// Image de l'etat courant (appelee ~30 fois / s).
    pub fn snapshot(&mut self) -> Snapshot {
        let now = Instant::now();
        let elapsed = now.duration_since(self.start);
        let ps = self.predicted_starts();
        let phases = self.shown_phases();
        let first_run = self.prev.as_ref().is_none_or(|p| p.total_ms == 0);
        let total = ps[&Phase::Ready].max(1.0);
        // debut / fin predits de l'etape courante (en sautant les etapes masquees)
        let cur = if phases.contains(&self.phase) { self.phase } else { *phases.iter().rev().find(|p| **p <= self.phase).unwrap_or(&Phase::Java) };
        let idx = phases.iter().position(|p| *p == cur).unwrap_or(0);
        let next = phases.get(idx + 1).copied().unwrap_or(Phase::Ready);
        let s0 = ps[&cur];
        let s1 = ps[&next].max(s0 + 0.5);
        let in_phase = elapsed.as_secs_f32() - self.starts.get(&cur).map_or(0.0, |d| d.as_secs_f32());
        let counter_frac = self.counter_fraction(cur);
        let time_frac = in_phase / (s1 - s0);
        // dans l'etape : le compteur reel prime ; sinon le temps, avec une asymptote a 92 % (jamais d'« arrivee » avant la vraie)
        let within = match counter_frac {
            Some(f) => f.clamp(0.0, 0.97),
            None => 0.92 * (1.0 - (-2.2 * time_frac.max(0.0)).exp()),
        };
        let overdue = counter_frac.is_none() && time_frac > 1.6 && cur != Phase::Ready;
        let target = if cur == Phase::Ready { 1.0 } else { ((s0 + within * (s1 - s0)) / total).clamp(0.0, 0.985) };
        // monotone + lissage (n'avance jamais a reculons, ne saute pas)
        self.shown = if target > self.shown { self.shown + (target - self.shown) * 0.12 + 0.0005 } else { self.shown };
        if cur == Phase::Ready {
            self.shown = 1.0;
        }
        let fraction = self.shown.min(1.0);
        let remaining = if first_run || cur == Phase::Ready {
            None
        } else {
            let rem = (total - (s0 + within * (s1 - s0))).max(0.0);
            Some(Duration::from_secs_f32(rem))
        };
        let stages = phases
            .iter()
            .enumerate()
            .map(|(i, p)| {
                let done = i < idx || cur == Phase::Ready;
                let took = if done {
                    let a = self.starts.get(p).copied();
                    let b = phases.get(i + 1).and_then(|n| self.starts.get(n)).copied().or(Some(elapsed));
                    match (a, b) {
                        (Some(a), Some(b)) if b > a => Some(b - a),
                        _ => None,
                    }
                } else {
                    None
                };
                StageView { phase: *p, label: p.label().to_string(), done, current: *p == cur && cur != Phase::Ready, took, counter: self.counter_text(*p) }
            })
            .collect();
        let activity = self.activity.as_ref().filter(|(_, t)| now.duration_since(*t) < Duration::from_secs(6)).map(|(a, _)| a.clone());
        Snapshot {
            phase: cur,
            doing: cur.doing().to_string(),
            closing: false,
            stages,
            fraction,
            elapsed,
            remaining,
            overdue,
            first_run,
            activity,
            silent_for: now.duration_since(self.last_line),
            detail: self.counter_text(cur),
        }
    }

    fn expected(&self, p: Phase) -> Option<u32> {
        self.totals.get(&p).copied().or_else(|| self.prev.as_ref().and_then(|pr| pr.counts.get(p.key()).copied())).or(match p {
            Phase::Scan if self.mods_in_dir > 0 => Some(self.mods_in_dir),
            _ => None,
        })
    }

    fn counter_fraction(&self, p: Phase) -> Option<f32> {
        let c = *self.counts.get(&p)? as f32;
        let e = self.expected(p)? as f32;
        if e > 0.0 {
            Some(c / e)
        } else {
            None
        }
    }

    fn counter_text(&self, p: Phase) -> Option<String> {
        let c = *self.counts.get(&p)?;
        Some(match (p, self.expected(p)) {
            (Phase::Scan, Some(e)) => format!("{} / {}", c.min(e.max(c)), e.max(c)),
            (Phase::Textures, _) => format!("{c} atlas"),
            (Phase::World, _) => format!("Zone de départ : {c} %"),
            (Phase::Scan, None) => format!("{c} mods"),
            (_, Some(e)) if e >= c => format!("{c} / {e}"),
            _ => return None,
        })
    }

    /// Profil a enregistrer apres un lancement reussi.
    pub fn finish_profile(&self) -> Profile {
        let total = self.start.elapsed().as_millis() as u64;
        Profile {
            total_ms: total,
            starts_ms: self.starts.iter().map(|(p, d)| (p.key().to_string(), d.as_millis() as u64)).collect(),
            counts: self.counts.iter().map(|(p, c)| (p.key().to_string(), *c)).collect(),
            runs: self.prev.as_ref().map_or(0, |p| p.runs) + 1,
        }
    }

    /// Heuristique de « pret » en l'absence de marqueur de l'agent : le jeu a fini ses textures et se tait depuis `quiet`.
    pub fn looks_ready(&self, quiet: Duration) -> bool {
        self.ready_marker || (self.phase >= Phase::Textures && self.window_seen && self.last_line.elapsed() >= quiet)
    }
}

/// Etapes d'une FERMETURE propre : on demande au jeu de se fermer comme le ferait le joueur (croix de la fenetre) ; il quitte
/// le monde, arrete le serveur integre, enregistre, puis s'eteint. Jamais de coup de force : seul le jeu dit quand il a fini.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum ClosePhase {
    Ask,
    Quit,
    Save,
    Exit,
}

impl ClosePhase {
    const ALL: [ClosePhase; 4] = [ClosePhase::Ask, ClosePhase::Quit, ClosePhase::Save, ClosePhase::Exit];

    pub fn label(self) -> &'static str {
        match self {
            ClosePhase::Ask => "Demande de fermeture au jeu",
            ClosePhase::Quit => "Sortie du monde",
            ClosePhase::Save => "Enregistrement du monde",
            ClosePhase::Exit => "Arrêt de Java",
        }
    }

    fn doing(self) -> &'static str {
        match self {
            ClosePhase::Ask => "Cubelink demande au jeu de se fermer…",
            ClosePhase::Quit => "Le jeu quitte le monde…",
            ClosePhase::Save => "Le monde s'enregistre, rien n'est coupé…",
            ClosePhase::Exit => "Java s'éteint proprement…",
        }
    }
}

pub struct Closer {
    start: Instant,
    phase: ClosePhase,
    starts: BTreeMap<ClosePhase, Duration>,
    last_line: Instant,
    exited: bool,
    shown: f32,
    activity: Option<(String, Instant)>,
}

impl Default for Closer {
    fn default() -> Self {
        Self::new()
    }
}

impl Closer {
    pub fn new() -> Closer {
        let now = Instant::now();
        Closer { start: now, phase: ClosePhase::Ask, starts: BTreeMap::from([(ClosePhase::Ask, Duration::ZERO)]), last_line: now, exited: false, shown: 0.0, activity: None }
    }

    pub fn phase(&self) -> ClosePhase {
        self.phase
    }

    fn advance(&mut self, to: ClosePhase) {
        if to > self.phase {
            let t = self.start.elapsed();
            for p in ClosePhase::ALL {
                if p > self.phase && p <= to {
                    self.starts.entry(p).or_insert(t);
                }
            }
            self.phase = to;
        }
    }

    pub fn feed_line(&mut self, line: &str) {
        self.last_line = Instant::now();
        let l = line.trim_end();
        if l.contains("All chunks are saved") || l.contains("All dimensions are saved") {
            self.advance(ClosePhase::Exit); // l'enregistrement est fini : il ne reste que l'arret de Java
        } else if l.contains("Saving worlds") || l.contains("Saving chunks") || l.contains("Saving players") || l.contains("Saving level") {
            self.advance(ClosePhase::Save);
            self.activity = Some((l.split("]: ").last().unwrap_or(l).chars().take(80).collect(), Instant::now()));
        } else if l.contains("Stopping server") || l.contains("Stopping singleplayer server") || l.contains("Disconnecting") || l.contains("Server stopped") || l.contains("Stopping worker threads") || l.contains("Stopping!") {
            self.advance(ClosePhase::Quit);
            self.activity = Some((l.split("]: ").last().unwrap_or(l).chars().take(80).collect(), Instant::now()));
        }
    }

    pub fn mark_exited(&mut self) {
        self.exited = true;
        self.advance(ClosePhase::Exit);
    }

    pub fn snapshot(&mut self) -> Snapshot {
        let now = Instant::now();
        let elapsed = now.duration_since(self.start);
        let idx = ClosePhase::ALL.iter().position(|p| *p == self.phase).unwrap_or(0);
        let el = elapsed.as_secs_f32();
        // les etapes n'ont pas de duree connue : l'avancement suit les etapes REELLES, avec un mouvement doux au sein de chacune
        let within = 0.9 * (1.0 - (-0.5 * (el - self.starts.get(&self.phase).map_or(0.0, |d| d.as_secs_f32())).max(0.0)).exp());
        let target = if self.exited { 1.0 } else { ((idx as f32 + within) / ClosePhase::ALL.len() as f32).min(0.97) };
        if target > self.shown {
            self.shown += (target - self.shown) * 0.15 + 0.0005;
        }
        let stages = ClosePhase::ALL
            .iter()
            .enumerate()
            .map(|(i, p)| {
                let done = self.exited || i < idx;
                let took = if done {
                    let a = self.starts.get(p).copied();
                    let b = ClosePhase::ALL.get(i + 1).and_then(|n| self.starts.get(n)).copied().or(Some(elapsed));
                    match (a, b) {
                        (Some(a), Some(b)) if b > a => Some(b - a),
                        _ => None,
                    }
                } else {
                    None
                };
                StageView { phase: Phase::Ready, label: p.label().to_string(), done, current: !self.exited && i == idx, took, counter: None }
            })
            .collect();
        Snapshot {
            phase: Phase::Ready,
            doing: if self.exited { "Jeu fermé proprement".to_string() } else { self.phase.doing().to_string() },
            closing: true,
            stages,
            fraction: self.shown.min(1.0),
            elapsed,
            remaining: None,
            overdue: false,
            first_run: false,
            activity: self.activity.as_ref().filter(|(_, t)| now.duration_since(*t) < Duration::from_secs(8)).map(|(a, _)| a.clone()),
            silent_for: now.duration_since(self.last_line),
            detail: None,
        }
    }
}

/// Ligne du jeu sans son horodatage `[hh:mm:ss] ` de tete.
pub fn line_body(line: &str) -> &str {
    let l = line.trim_end();
    let b = l.as_bytes();
    if b.len() > 11 && b[0] == b'[' && b[3] == b':' && b[6] == b':' && b[9] == b']' && b[10] == b' ' && [1, 2, 4, 5, 7, 8].iter().all(|i| b[*i].is_ascii_digit()) {
        &l[11..]
    } else {
        l
    }
}

/// Bruit repete qui ne doit pas compter comme de l'activite du jeu : l'avertissement SDL 3 « Failed to synchronize SDL window » (une
/// fois par image tant qu'une synchronisation de fenetre echoue), ou une ligne identique a la precedente, horodatage mis a part.
pub fn is_noise(body: &str, last: Option<&str>) -> bool {
    body.contains("Failed to synchronize SDL window") || last == Some(body)
}

/// Passe `line` au suivi du lancement ; vrai si c'etait du bruit (analysee mais sans compter comme activite).
pub fn feed_tracker(t: &mut Tracker, last_body: &mut Option<String>, line: &str) -> bool {
    let body = line_body(line);
    let noise = is_noise(body, last_body.as_deref());
    if last_body.as_deref() != Some(body) {
        *last_body = Some(body.to_string());
    }
    if noise {
        t.feed_line_quiet(line);
    } else {
        t.feed_line(line);
    }
    noise
}

/// Nombre de `.jar` du dossier des mods (0 s'il est illisible).
pub fn count_jars(dir: &Path) -> u32 {
    std::fs::read_dir(dir).map_or(0, |d| d.filter_map(|e| e.ok()).filter(|e| e.path().extension().is_some_and(|x| x == "jar")).count() as u32)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn feed(t: &mut Tracker, lines: &[&str]) {
        for l in lines {
            t.feed_line(l);
        }
    }

    #[test]
    fn neoforge_phases_and_counters() {
        let mut t = Tracker::new(147, None);
        assert_eq!(t.phase(), Phase::Java);
        feed(&mut t, &[
            "[03:24:18.758] [main/INFO] [loading.moddiscovery.ModDiscoverer/SCAN]: Found mod file \"actuallyadditions-1.3.26+mc1.21.1.jar\" [locator: {mods folder locator}]",
            "[03:24:18.788] [main/INFO] [loading.moddiscovery.ModDiscoverer/SCAN]: Found mod file \"ae2wtlib-19.5.1.jar\" [locator: {mods folder locator}]",
        ]);
        assert_eq!(t.phase(), Phase::Scan);
        let s = t.snapshot();
        assert_eq!(s.detail.as_deref(), Some("2 / 147"));
        assert_eq!(s.activity.as_deref(), Some("ae2wtlib-19.5.1.jar"));
        feed(&mut t, &["[03:24:24.898] [main/WARN] [mixin/]: Reference map 'balm.refmap.json' for balm.mixins.json could not be read."]);
        assert_eq!(t.phase(), Phase::Mixin);
        feed(&mut t, &["[03:24:34] [pool-19-thread-1/INFO] [minecraft/Bootstrap]: Vanilla bootstrap took 4238 milliseconds"]);
        assert_eq!(t.phase(), Phase::Bootstrap);
        feed(&mut t, &["[03:24:36] [modloading-worker-0/INFO] [sfm/]: Registered bus=MOD"]);
        assert_eq!(t.phase(), Phase::ModLoad);
        // une ligne « mixin » tardive ne fait pas reculer
        feed(&mut t, &["[03:24:26.815] [main/INFO] [Colorful Hearts/]: Applying mixin X as mod appleskin is present."]);
        assert_eq!(t.phase(), Phase::ModLoad);
        feed(&mut t, &["[03:24:56] [Render thread/INFO] [minecraft/ReloadableResourceManager]: Reloading ResourceManager: vanilla"]);
        assert_eq!(t.phase(), Phase::Resources);
        feed(&mut t, &["[03:25:07] [Render thread/INFO] [minecraft/TextureAtlas]: Created: 8192x8192x4 minecraft:textures/atlas/blocks.png-atlas"]);
        assert_eq!(t.phase(), Phase::Textures);
        assert_eq!(t.snapshot().detail.as_deref(), Some("1 atlas"));
    }

    #[test]
    fn world_launch_tracks_spawn_area_and_join() {
        let mut t = Tracker::new(0, None);
        t.feed_line("[04:01:13] [Render thread/INFO]: Reloading ResourceManager: vanilla");
        t.feed_line("[04:01:13] [Render thread/INFO]: Created: 1024x1024x0 minecraft:textures/atlas/gui.png-atlas");
        assert_eq!(t.phase(), Phase::Textures);
        assert!(t.snapshot().stages.iter().all(|s| s.phase != Phase::World));
        t.feed_line("[04:01:13] [Server thread/INFO]: Preparing spawn area: 16%");
        assert_eq!(t.phase(), Phase::World);
        let s = t.snapshot();
        assert_eq!(s.detail.as_deref(), Some("Zone de départ : 16 %"));
        assert!(s.stages.iter().any(|x| x.phase == Phase::World && x.current));
        t.feed_line("[04:01:14] [Server thread/INFO]: System chat: Testeur joined the game");
        assert_eq!(t.phase(), Phase::Ready);
    }

    /// Lignes reelles d'un lancement Linux (logs/test/latest.log) : le WARN SDL repete ne repousse pas la quietude, et analyser une
    /// ligne en silence donne la meme etape / le meme detail que `feed_line` (les journaux Windows passent par `feed_line`, inchange).
    #[test]
    fn repeated_sdl_warning_does_not_reset_quiet() {
        let real = [
            "[20:33:30] [Render thread/INFO]: Backend library: LWJGL version 3.4.3",
            "[20:33:31] [Render thread/INFO]: Reloading ResourceManager: vanilla",
            "[20:33:31] [Render thread/INFO]: Created: 1024x1024x0 minecraft:textures/atlas/gui.png-atlas",
        ];
        let warn = "[20:33:32] [Render thread/WARN]: Failed to synchronize SDL window: ";
        let (mut a, mut b) = (Tracker::new(0, None), Tracker::new(0, None));
        for l in real {
            a.feed_line(l);
            b.feed_line(l);
        }
        a.feed_line(warn);
        b.feed_line(warn);
        assert_eq!((a.phase(), a.snapshot().detail), (b.phase(), b.snapshot().detail));
        a.window_shown();
        b.window_shown();
        std::thread::sleep(Duration::from_millis(60));
        for _ in 0..20 {
            a.feed_line_quiet(warn); // bruit : la quietude court toujours
            b.feed_line(warn); // activite : elle repart de zero
        }
        assert!(a.looks_ready(Duration::from_millis(50)), "le bruit repete ne doit pas empecher la quietude");
        assert!(!b.looks_ready(Duration::from_millis(50)));
        assert!(a.silent_for() >= Duration::from_millis(50));
    }

    #[test]
    fn fabric_loading_n_mods_sets_total() {
        let mut t = Tracker::new(147, None);
        feed(&mut t, &["[03:25:28] [main/INFO]: Loading 147 mods:"]);
        assert_eq!(t.phase(), Phase::Scan);
        assert_eq!(t.snapshot().detail.as_deref(), Some("147 / 147"));
        feed(&mut t, &["[03:25:35] [Render thread/INFO]: Setting user: Gelo__", "[03:25:37] [Render thread/INFO]: Backend library: LWJGL version 3.4.3"]);
        assert_eq!(t.phase(), Phase::Window);
    }

    #[test]
    fn vanilla_hides_mod_stages_and_never_regresses() {
        let mut t = Tracker::new(0, None);
        let s = t.snapshot();
        assert!(s.stages.iter().all(|x| !x.phase.modded_only()));
        assert!(s.first_run && s.remaining.is_none());
        let mut last = 0.0;
        for _ in 0..50 {
            let f = t.snapshot().fraction;
            assert!(f >= last);
            last = f;
        }
        t.mark_ready();
        assert_eq!(t.snapshot().fraction, 1.0);
    }

    #[test]
    fn closer_follows_real_shutdown_lines() {
        let mut c = Closer::new();
        assert_eq!(c.phase(), ClosePhase::Ask);
        c.feed_line("[10:00:00] [Render thread/INFO]: Stopping!");
        assert_eq!(c.phase(), ClosePhase::Quit);
        c.feed_line("[10:00:01] [Server thread/INFO]: Stopping server");
        assert_eq!(c.phase(), ClosePhase::Quit);
        c.feed_line("[10:00:01] [Server thread/INFO]: Saving players");
        assert_eq!(c.phase(), ClosePhase::Save);
        c.feed_line("[10:00:02] [Server thread/INFO]: Saving worlds");
        c.feed_line("[10:00:03] [Server thread/INFO]: ThreadedAnvilChunkStorage: All dimensions are saved");
        assert_eq!(c.phase(), ClosePhase::Exit);
        let s = c.snapshot();
        assert!(s.closing && s.stages.len() == 4 && s.stages[0].done && s.stages[3].current);
        c.mark_exited();
        let s = c.snapshot();
        assert!(s.stages.iter().all(|x| x.done));
    }

    #[test]
    fn ready_marker_and_profile_roundtrip() {
        let mut t = Tracker::new(10, None);
        feed(&mut t, &["[x] [Render thread/INFO]: [STDOUT]: [Cubelink] READY title"]);
        assert_eq!(t.phase(), Phase::Ready);
        let p = t.finish_profile();
        assert_eq!(p.runs, 1);
        let t2 = Tracker::new(10, Some(p));
        let mut t2 = t2;
        assert!(!t2.snapshot().first_run || t2.snapshot().remaining.is_none());
    }

    // lignes COPIEES du journal reel d'un lancement (Minecraft 26.3, LWJGL 3.4.3 / SDL 3, XWayland) ou l'avertissement SDL
    // revenait ~10 fois par seconde apres le dernier message utile
    const SPAM: &str = "[20:33:35] [Render thread/WARN]: Failed to synchronize SDL window: ";
    const SPAM_NEXT_SECOND: &str = "[20:33:36] [Render thread/WARN]: Failed to synchronize SDL window: ";
    const RELOAD: &str = "[20:33:32] [Render thread/INFO]: Reloading ResourceManager: vanilla, vanilla";
    const SOUND: &str = "[20:33:34] [Render thread/INFO]: Sound engine started";
    const ATLAS: &str = "[20:33:34] [Render thread/INFO]: Created: 1024x1024x0 minecraft:textures/atlas/gui.png-atlas";

    #[test]
    fn horodatage_retire() {
        assert_eq!(line_body(SPAM), "[Render thread/WARN]: Failed to synchronize SDL window:");
        assert_eq!(line_body("sans horodatage  "), "sans horodatage");
        assert_eq!(line_body("[Cubelink] READY"), "[Cubelink] READY");
    }

    #[test]
    fn le_bruit_sdl_repete_ne_repousse_pas_la_quietude() {
        let (mut t, mut last) = (Tracker::new(0, None), None);
        for l in [RELOAD, SOUND, ATLAS] {
            assert!(!feed_tracker(&mut t, &mut last, l), "ligne utile prise pour du bruit : {l}");
        }
        t.window_shown();
        std::thread::sleep(Duration::from_millis(300));
        // le jeu se plaint en boucle, une ligne « nouvelle » (horodatage) a chaque seconde : ce n'est pas de l'activite
        for l in [SPAM, SPAM, SPAM_NEXT_SECOND, SPAM_NEXT_SECOND] {
            assert!(feed_tracker(&mut t, &mut last, l));
        }
        assert!(t.looks_ready(Duration::from_millis(200)), "le bruit a repousse la quietude");
        assert!(t.silent_for() >= Duration::from_millis(300));
    }

    #[test]
    fn une_ligne_utile_repousse_la_quietude() {
        let (mut t, mut last) = (Tracker::new(0, None), None);
        for l in [RELOAD, SOUND, ATLAS] {
            feed_tracker(&mut t, &mut last, l);
        }
        t.window_shown();
        std::thread::sleep(Duration::from_millis(300));
        assert!(!feed_tracker(&mut t, &mut last, "[20:33:37] [Render thread/INFO]: Narrator loaded"), "une vraie ligne n'est pas du bruit");
        assert!(!t.looks_ready(Duration::from_millis(200)), "une ligne utile doit relancer la quietude");
    }

    #[test]
    fn une_ligne_identique_a_la_precedente_est_du_bruit_pas_la_premiere() {
        assert!(!is_noise("a", None));
        assert!(!is_noise("a", Some("b")));
        assert!(is_noise("a", Some("a")));
        assert!(is_noise("[Render thread/WARN]: Failed to synchronize SDL window:", None));
    }
}

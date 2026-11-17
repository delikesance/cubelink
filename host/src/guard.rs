//! Gardes de la Scene : QUAND renoncer a tenir le jeu dans la fenetre du launcher, et OU reposer la fenetre qu'on lui rend.
//!
//! Decisions PURES (aucune horloge, aucun appel systeme : les durees ecoulees sont des parametres), testees ici ; la couche Win32
//! les interroge a chaque battement du rideau et execute le repli. Le risque couvert : un jeu qui tourne mais que le joueur ne
//! voit nulle part (fenetre garee hors ecran jamais rattachee, rideau ferme sur une fenetre qui n'arrivera pas).

use std::time::Duration;

use crate::health::FallbackReason;

/// Delai, apres `watch`, au bout duquel un jeu qui n'a toujours AFFICHE aucune fenetre est sorti de la Scene.
///
/// Avant sa fenetre le jeu demarre la JVM, decouvre les mods et applique mixins / transformations de classes ; la fenetre arrive
/// AVANT le gros du chargement (c'est elle qui porte l'ecran de chargement du jeu). Sur un gros modpack et un disque lent cette
/// phase se compte en dizaines de secondes, exceptionnellement une a deux minutes. 4 minutes : assez pour ne jamais ejecter un
/// lancement lent mais sain, assez peu pour qu'un rideau ferme sur rien ne passe pas pour un launcher plante. Se tromper dans le
/// sens « trop court » reste benin : le jeu n'est jamais tue, il apparait simplement dans sa propre fenetre.
pub const NO_WINDOW_GUARD: Duration = Duration::from_secs(240);

/// Duree pendant laquelle le rattachement (proprietaire) d'une fenetre peut rester REFUSE avant de la rendre.
///
/// Seules comptent les tentatives TERMINEES : un jeu qui ne pompe pas ses messages (chargement) bloque l'appel sans le refuser, et
/// ce cas-la n'est pas un echec (le rideau couvre l'attente). Une tentative qui revient avec un proprietaire incorrect est en
/// revanche un vrai refus (droits differents, jeu qui defait le rattachement) : inutile d'insister longtemps, 10 s de nouvelles
/// tentatives suffisent a ecarter un etat transitoire.
pub const ADOPTION_GUARD: Duration = Duration::from_secs(10);

/// Nombre minimal de tentatives terminees avant de conclure a un refus (jamais sur une seule mesure).
pub const ADOPTION_MIN_ATTEMPTS: u32 = 3;

/// Intervalle entre deux nouvelles tentatives de rattachement.
pub const ADOPTION_RETRY: Duration = Duration::from_millis(250);

/// Delais de garde en vigueur. Les valeurs par defaut sont celles de production ; `GameHost::set_guards` les raccourcit dans les
/// tests d'integration (personne n'attend 4 minutes).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Guards {
    pub no_window: Duration,
    pub adoption: Duration,
}

impl Default for Guards {
    fn default() -> Self {
        Guards { no_window: NO_WINDOW_GUARD, adoption: ADOPTION_GUARD }
    }
}

/// Ce que la garde observe du lancement en cours.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct Watched {
    /// Un processus de jeu est surveille et le lancement est toujours dans la Scene.
    pub active: bool,
    /// Temps ecoule depuis `watch` (`None` : pas de surveillance).
    pub since_watch: Option<Duration>,
    /// LA fenetre du jeu a ete affichee par le jeu.
    pub window_shown: bool,
    /// Rattachement refuse : (depuis combien de temps, nombre de tentatives terminees).
    pub refused: Option<(Duration, u32)>,
}

/// Faut-il renoncer, et pourquoi ? `None` : on continue.
pub fn verdict(g: &Guards, w: &Watched) -> Option<FallbackReason> {
    if !w.active {
        return None;
    }
    // une fenetre vue mais refusee est plus precise que « aucune fenetre » : elle passe d'abord
    if let Some((since, attempts)) = w.refused {
        if attempts >= ADOPTION_MIN_ATTEMPTS && since >= g.adoption {
            return Some(FallbackReason::AdoptionFailed);
        }
    }
    if !w.window_shown && w.since_watch.is_some_and(|d| d >= g.no_window) {
        return Some(FallbackReason::NoWindow);
    }
    None
}

/// Taille rendue a une fenetre dont on ne connait pas (ou plus) la taille d'origine.
const DEFAULT_SIZE: (i32, i32) = (1280, 720);
/// En dessous, la taille d'origine n'est pas celle d'une fenetre de jeu (fenetre technique, taille pas encore fixee).
const MIN_SIZE: (i32, i32) = (320, 240);

/// Rectangle `(x, y, largeur, hauteur)` ou reposer une fenetre RENDUE au jeu : sa taille d'origine `want` (a defaut 1280 x 720),
/// jamais plus grande que la zone de travail `work` (`(gauche, haut, droite, bas)` de l'ecran du launcher), centree dedans. La
/// fenetre est donc entierement visible, barre de titre comprise : le joueur peut la deplacer.
pub fn returned_rect(work: (i32, i32, i32, i32), want: (i32, i32)) -> (i32, i32, i32, i32) {
    let (l, t, r, b) = work;
    let (ww, wh) = ((r - l).max(1), (b - t).max(1));
    let (w, h) = if want.0 >= MIN_SIZE.0 && want.1 >= MIN_SIZE.1 { want } else { DEFAULT_SIZE };
    let (w, h) = (w.min(ww), h.min(wh));
    (l + (ww - w) / 2, t + (wh - h) / 2, w, h)
}

/// Classe d'une fenetre d'AIDE de SDL 3 ou de GLFW 3.4 (cachee, « message seulement », creee avant ou apres la fenetre du jeu) :
/// jamais une fenetre de jeu, jamais touchee. Mesure sur un vrai Minecraft SDL 3 : les prendre pour des fenetres de jeu declenchait
/// un repli « rattachement refuse » alors que LA fenetre du jeu etait adoptee et posee ; sur un gros modpack, l'adoption de la
/// fenetre d'aide de GLFW attendait 21 s le fil du jeu, et celle de LA fenetre du jeu derriere elle.
pub fn is_helper_class(c: &str) -> bool {
    c.starts_with("SDLHelperWindow") || c.starts_with("SDL_HIDAPI") || c.starts_with("GLFW3 Helper")
}

/// Taille minimale d'une candidate PRINCIPALE (une fenetre qui peut etre LA fenetre du jeu). En dessous : fenetres d'essai et
/// fenetres techniques (SDL 3 en cree une quinzaine par lancement, 10 x 10 ou 136 x 39), jamais tenues pour la fenetre du jeu.
pub const MAIN_MIN: (i32, i32) = (200, 150);

/// `(largeur, hauteur)` est une taille de fenetre de jeu.
pub fn main_sized(w: i32, h: i32) -> bool {
    w >= MAIN_MIN.0 && h >= MAIN_MIN.1
}

/// Premiere pose : duree minimale entre l'apparition de LA fenetre du jeu et sa pose sur la zone. Un jeu repose le cadre de sa
/// fenetre et l'agrandit autour de sa zone cliente juste apres l'avoir affichee (mesure : NeoForge, ~185 ms apres l'adoption).
/// Posee avant, elle se retrouvait a l'ecran avec son cadre, debordant du rideau, le temps que le jeu traite la correction
/// (mesure : 154 a 248 ms). Garee hors ecran pendant ce delai, elle y recoit sa correction et n'est posee qu'ensuite. Sans cout
/// visible : le rideau est ferme, le jeu charge.
pub const POSE_SETTLE: Duration = Duration::from_millis(400);
/// ... et duree de calme exigee apres un cadre remis par le jeu pendant cette attente.
pub const POSE_CALM: Duration = Duration::from_millis(250);
/// Au-dela, on pose quoi qu'il arrive (le cadre est alors retire juste avant la pose, hors ecran) : un jeu qui remettrait son
/// cadre sans cesse ne reste pas hors de la Scene.
pub const POSE_MAX: Duration = Duration::from_secs(2);

/// La premiere pose doit-elle encore attendre ? `since_shown` : depuis l'apparition de la fenetre ; `since_reframe` : depuis le
/// dernier cadre remis par le jeu ; `framed` : la fenetre a des bits de cadre en ce moment.
pub fn pose_waits(since_shown: Option<Duration>, since_reframe: Option<Duration>, framed: bool) -> bool {
    let Some(shown) = since_shown else { return false };
    shown < POSE_MAX && (framed || shown < POSE_SETTLE || since_reframe.is_some_and(|d| d < POSE_CALM))
}

/// Lancement CACHE (`GameHost::expect_hidden`) : duree pendant laquelle une candidate principale adoptee doit rester la seule,
/// cachee, avant d'etre tenue pour LA fenetre du jeu sans attendre que le jeu l'affiche (il ne l'affichera pas). Assez pour qu'une
/// fenetre d'essai ephemere ait disparu et qu'un jeu qui ignore la consigne se soit montre lui-meme (le chemin normal reste alors
/// le seul) ; trop court pour se remarquer derriere le rideau.
pub const HIDDEN_SETTLE: Duration = Duration::from_millis(400);

/// Lancement cache, apres un repli sans fenetre a rendre : duree pendant laquelle l'hote guette encore une fenetre principale
/// cachee du jeu pour la MONTRER (personne d'autre ne le fera). Dix minutes couvrent le demarrage le plus lent ; au-dela le
/// processus est fige ou parti.
pub const RESCUE_FOR: Duration = Duration::from_secs(600);
/// Intervalle de ce guet.
pub const RESCUE_EVERY: Duration = Duration::from_millis(500);
/// Duree pendant laquelle une fenetre doit etre vue, cachee, avant d'etre montree par le guet (jamais une fenetre d'essai ephemere).
pub const RESCUE_SETTLE: Duration = Duration::from_secs(1);

/// Le rectangle ecran `rect` deborde-t-il de `zone` (tous deux `(x, y, largeur, hauteur)`) ? Sert a dire, dans le journal, si une
/// fenetre de jeu que le rideau ne peut que recouvrir (jeu qui ne repond pas) se voit autour de la zone de jeu.
pub fn overflows(rect: (i32, i32, i32, i32), zone: (i32, i32, i32, i32)) -> bool {
    rect.0 < zone.0 || rect.1 < zone.1 || rect.0 + rect.2 > zone.0 + zone.2 || rect.1 + rect.3 > zone.1 + zone.3
}

#[cfg(test)]
mod tests {
    use super::*;

    const G: Guards = Guards { no_window: NO_WINDOW_GUARD, adoption: ADOPTION_GUARD };

    fn secs(s: u64) -> Duration {
        Duration::from_secs(s)
    }

    #[test]
    fn valeurs_de_production() {
        assert_eq!(Guards::default(), G);
        assert!((180..=300).contains(&NO_WINDOW_GUARD.as_secs()), "de l'ordre de 3 a 5 minutes");
        assert!(ADOPTION_GUARD < NO_WINDOW_GUARD);
        assert!(ADOPTION_RETRY * ADOPTION_MIN_ATTEMPTS < ADOPTION_GUARD, "les tentatives minimales tiennent dans le delai");
    }

    #[test]
    fn aucune_fenetre() {
        let w = |s: u64, shown: bool| Watched { active: true, since_watch: Some(secs(s)), window_shown: shown, refused: None };
        assert_eq!(verdict(&G, &w(0, false)), None);
        assert_eq!(verdict(&G, &w(239, false)), None, "un gros modpack lent n'est pas ejecte");
        assert_eq!(verdict(&G, &w(240, false)), Some(FallbackReason::NoWindow));
        assert_eq!(verdict(&G, &w(100_000, false)), Some(FallbackReason::NoWindow));
        // une fenetre affichee : plus jamais de repli « aucune fenetre », meme des heures plus tard
        assert_eq!(verdict(&G, &w(240, true)), None);
        assert_eq!(verdict(&G, &w(100_000, true)), None);
        // delai injecte (tests d'integration)
        let short = Guards { no_window: Duration::from_millis(500), ..G };
        assert_eq!(verdict(&short, &w(1, false)), Some(FallbackReason::NoWindow));
    }

    #[test]
    fn rien_sans_jeu_surveille() {
        // pas de jeu, jeu deja rendu, jeu termine : aucune garde ne se declenche
        let w = Watched { active: false, since_watch: Some(secs(100_000)), window_shown: false, refused: Some((secs(100_000), 99)) };
        assert_eq!(verdict(&G, &w), None);
        // surveille mais sans horodatage (ne devrait pas arriver) : pas de repli sur une duree inconnue
        assert_eq!(verdict(&G, &Watched { active: true, ..Watched::default() }), None);
    }

    #[test]
    fn rattachement_refuse() {
        let w = |s: u64, attempts: u32| Watched { active: true, since_watch: Some(secs(s + 5)), window_shown: true, refused: Some((secs(s), attempts)) };
        assert_eq!(verdict(&G, &w(0, 1)), None);
        assert_eq!(verdict(&G, &w(9, 30)), None, "delai pas encore ecoule");
        assert_eq!(verdict(&G, &w(10, 30)), Some(FallbackReason::AdoptionFailed));
        // le delai seul ne suffit pas : un jeu qui ne repond pas BLOQUE la tentative, il ne la refuse pas
        assert_eq!(verdict(&G, &w(3600, 0)), None);
        assert_eq!(verdict(&G, &w(3600, ADOPTION_MIN_ATTEMPTS - 1)), None);
        assert_eq!(verdict(&G, &w(3600, ADOPTION_MIN_ATTEMPTS)), Some(FallbackReason::AdoptionFailed));
    }

    #[test]
    fn le_refus_passe_avant_l_absence_de_fenetre() {
        let w = Watched { active: true, since_watch: Some(secs(1000)), window_shown: false, refused: Some((secs(60), 10)) };
        assert_eq!(verdict(&G, &w), Some(FallbackReason::AdoptionFailed));
        // refus pas encore etabli : l'absence de fenetre affichee decide
        let w = Watched { refused: Some((secs(1), 1)), ..w };
        assert_eq!(verdict(&G, &w), Some(FallbackReason::NoWindow));
    }

    #[test]
    fn fenetre_rendue_centree_et_entiere() {
        // ecran 1920 x 1080, barre des taches de 40 px en bas
        let work = (0, 0, 1920, 1040);
        assert_eq!(returned_rect(work, (870, 520)), (525, 260, 870, 520));
        // taille inconnue ou fenetre technique : taille par defaut
        assert_eq!(returned_rect(work, (0, 0)), (320, 160, 1280, 720));
        assert_eq!(returned_rect(work, (160, 28)), (320, 160, 1280, 720));
        // plus grande que l'ecran (jeu qui etait en plein ecran sur un autre moniteur) : ramenee a la zone de travail
        assert_eq!(returned_rect((0, 0, 1280, 680), (2560, 1440)), (0, 0, 1280, 680));
        // second ecran a gauche du principal (coordonnees negatives)
        assert_eq!(returned_rect((-1920, 0, 0, 1040), (800, 600)), (-1360, 220, 800, 600));
        // zone de travail degeneree : jamais de taille nulle ni negative
        let (_, _, w, h) = returned_rect((10, 10, 10, 10), (800, 600));
        assert!(w >= 1 && h >= 1);
        // toujours a l'interieur de la zone de travail
        for want in [(1, 1), (320, 240), (854, 480), (5000, 300), (300, 5000)] {
            let (x, y, w, h) = returned_rect(work, want);
            assert!(x >= work.0 && y >= work.1 && x + w <= work.2 && y + h <= work.3, "{want:?} -> {:?}", (x, y, w, h));
        }
    }

    #[test]
    fn attente_de_la_premiere_pose() {
        let ms = Duration::from_millis;
        assert!(pose_waits(Some(ms(0)), None, false) && pose_waits(Some(ms(399)), None, false));
        assert!(!pose_waits(Some(ms(400)), None, false), "fenetre calme : posee apres le delai");
        assert!(pose_waits(Some(ms(600)), Some(ms(100)), false), "cadre remis il y a peu : on attend le calme");
        assert!(!pose_waits(Some(ms(600)), Some(ms(250)), false));
        assert!(pose_waits(Some(ms(900)), Some(ms(900)), true), "jamais posee avec son cadre...");
        assert!(!pose_waits(Some(ms(2000)), Some(ms(10)), true), "... sauf a ne jamais finir");
        assert!(!pose_waits(None, None, true), "fenetre deja posee une fois (retour au jeu) : immediate");
    }

    #[test]
    fn taille_de_fenetre_principale() {
        assert!(main_sized(854, 480) && main_sized(200, 150));
        for (w, h) in [(10, 10), (136, 39), (1, 1), (199, 600), (800, 149)] {
            assert!(!main_sized(w, h), "{w} x {h}");
        }
        assert!(HIDDEN_SETTLE < ADOPTION_GUARD && RESCUE_SETTLE < RESCUE_FOR && RESCUE_FOR > NO_WINDOW_GUARD);
    }

    #[test]
    fn debordement_de_la_zone() {
        let zone = (100, 100, 800, 600);
        assert!(!overflows(zone, zone));
        assert!(!overflows((200, 150, 300, 200), zone));
        assert!(overflows((99, 100, 800, 600), zone), "a gauche");
        assert!(overflows((100, 90, 100, 100), zone), "en haut");
        assert!(overflows((500, 300, 401, 100), zone), "a droite");
        assert!(overflows((500, 300, 100, 401), zone), "en bas");
        assert!(overflows((525, 269, 870, 519), (300, 200, 960, 540)), "cas mesure : fenetre de jeu plus basse que la zone");
    }
}

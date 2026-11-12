//! Sante de la Scene : ce que l'hote annonce a son appelant (le launcher). Sans systeme : compile sur toutes les plateformes.
//!
//! CONTRAT entre le crate `cubelink-host` et le launcher (`src-tauri/src/stage.rs`). L'observateur est appele depuis des fils de
//! l'hote, HORS du verrou d'etat : il doit rendre la main vite et ne jamais rappeler l'hote de facon synchrone.

use std::sync::Arc;

/// Pourquoi la Scene a renonce pour le lancement en cours.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FallbackReason {
    /// Les hooks de surveillance n'ont pas pu etre poses sur le processus du jeu.
    HookFailed,
    /// La fenetre du jeu a ete vue mais n'a pas pu etre rattachee (proprietaire / styles refuses) dans le delai.
    AdoptionFailed,
    /// Aucune fenetre de jeu n'est apparue dans le delai de garde alors que le processus tourne toujours.
    NoWindow,
    /// L'appelant l'a demande (`GameHost::release`).
    Requested,
}

impl FallbackReason {
    /// Identifiant stable (journal, evenement envoye a l'interface).
    pub fn as_str(self) -> &'static str {
        match self {
            FallbackReason::HookFailed => "hook_failed",
            FallbackReason::AdoptionFailed => "adoption_failed",
            FallbackReason::NoWindow => "no_window",
            FallbackReason::Requested => "requested",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HostEvent {
    /// LA fenetre du jeu est rattachee a la Scene (une fois par lancement).
    Adopted { class: String, after_ms: u64 },
    /// La Scene renonce pour CE lancement : toute fenetre du jeu deja touchee a ete RENDUE (cadre, barre des taches, position
    /// visible a l'ecran, plus de proprietaire), le rideau s'est rouvert sur le launcher, la surveillance est arretee. Le jeu
    /// continue dans sa propre fenetre ; il n'est jamais tue. Emis au plus une fois par lancement.
    Fallback { reason: FallbackReason, detail: String },
    /// Ligne de journal, TOUJOURS emise (pas seulement avec `CUBELINK_STAGE_DEBUG`) : etapes cles seulement (lancement, hooks,
    /// detection, adoption, pose, plein ecran corrige, fermeture), jamais un flux par image.
    Log(String),
}

pub type Observer = Arc<dyn Fn(&HostEvent) + Send + Sync>;

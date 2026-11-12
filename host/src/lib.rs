//! La Scene Cubelink : le jeu tourne DANS la fenetre du launcher (Windows), un « navigateur pour Minecraft ».
//!
//! Le jeu garde son propre processus (une JVM par modpack : Java, loader et natives different). Sa fenetre native est ADOPTEE des
//! sa creation, avant d'avoir ete affichee : elle reste une fenetre de premier niveau (le jeu n'accepte clavier et souris que
//! s'il est au premier plan de Windows, ce qu'une fenetre fille n'obtient jamais), mais sans cadre, possedee par la fenetre du
//! launcher et calee sur la zone de jeu. Le joueur ne voit donc jamais une fenetre de jeu autonome. Changer de modpack = remplacer
//! le processus ; un rideau (deux panneaux aux couleurs du launcher) couvre l'intervalle : etapes reelles du demarrage lues dans la
//! sortie du jeu, barre et estimation apprises des lancements precedents, puis ouverture quand le nouveau jeu est pret.
//!
//! Quand la Scene ne peut pas tenir le jeu (surveillance refusee, rattachement refuse, aucune fenetre dans le delai de garde), elle
//! y RENONCE proprement pour ce lancement : la fenetre est rendue au jeu, visible et avec son cadre, et l'appelant en est informe.
//!
//! * [`progress`] : suivi des etapes (sans systeme, teste sur de vrais journaux).
//! * [`curtain`]  : rendu du rideau (logiciel, aux tokens du launcher).
//! * [`flow`]     : deroulement de la vue (machine a etats pure : launcher, rideau, jeu), teste sans fenetre.
//! * [`guard`]    : quand renoncer, ou reposer la fenetre rendue (decisions pures, testees sans horloge).
//! * [`health`]   : ce que l'hote annonce a son appelant (adoption, repli, journal) : contrat avec le launcher.
//! * `win` (Windows) : [`GameHost`], la zone de jeu, l'adoption, le rideau. macOS et Wayland ne permettent pas d'adopter la
//!   fenetre d'un autre processus : le jeu y garde sa propre fenetre.

pub mod curtain;
pub mod flow;
pub mod guard;
pub mod health;
pub mod progress;

pub use guard::Guards;
pub use health::{FallbackReason, HostEvent, Observer};

#[cfg(windows)]
mod win;
#[cfg(windows)]
pub use win::*;

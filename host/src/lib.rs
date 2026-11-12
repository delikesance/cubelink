//! Fenetre hote Cubelink : un « navigateur pour Minecraft ».
//!
//! Le jeu tourne dans son propre processus (une JVM par modpack : Java, loader et natives different) ; sa fenetre native est
//! ADOPTEE par notre fenetre (Windows : `SetParent`) des sa CREATION, avant d'avoir ete affichee, donc le joueur ne voit
//! jamais une fenetre de jeu autonome. Changer de modpack = remplacer le processus ; un rideau couvre l'intervalle :
//! derniere image figee sous le voile du design system, etapes reelles du demarrage (lues dans la sortie du jeu), barre et
//! estimation apprises des lancements precedents, puis fondu quand le nouveau jeu est pret.
//!
//! * [`progress`] : suivi des etapes (sans systeme, teste sur de vrais journaux).
//! * [`curtain`]  : rendu du rideau (logiciel, aux tokens du launcher).
//! * [`flow`]     : deroulement de la vue (machine a etats pure : launcher, rideau, jeu), teste sans fenetre.
//! * `win` (Windows) : fenetre hote, adoption, processus. macOS et Wayland ne permettent pas d'adopter la fenetre d'un autre
//!   processus : repli a prevoir (le meme rideau y servirait de transition).

pub mod curtain;
pub mod flow;
pub mod progress;

#[cfg(windows)]
mod win;
#[cfg(windows)]
pub use win::*;

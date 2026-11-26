//! Mise à jour d'un modpack (Modrinth, CurseForge) déjà installé (ticket #137) : détection, application, annulation.
//!
//! - [`check`] compare la version installée (`Instance.pack`) aux versions publiées compatibles avec l'instance
//!   (même Minecraft, même loader) et renvoie la plus récente si elle est plus récente que l'installée.
//! - [`apply`] compare l'ANCIEN et le NOUVEAU `.mrpack` (index et overrides) au contenu réel du disque :
//!   un fichier du pack n'est remplacé ou retiré que s'il est encore identique à celui de l'ancien pack. Un fichier
//!   que le joueur a modifié est gardé tel quel (décision : le joueur gagne, sans question) ; un fichier que le
//!   joueur a ajouté, `saves/`, `options.txt`, `servers.dat`... ne sont jamais touchés.
//! - Tout est téléchargé et extrait AVANT la première modification de l'instance (un échec réseau ne change rien).
//!   Ensuite, chaque fichier retiré ou remplacé est déplacé dans `.cubelink-pack-backup/<id>/files/` et un
//!   manifeste décrit l'opération ; `Instance.pack` est mis à jour en dernier. Un échec en cours de route remet tout.
//! - [`undo`] relit le manifeste : retire les fichiers ajoutés (sauf ceux que le joueur a modifiés depuis),
//!   remet les fichiers déplacés, la provenance des mods, l'ancienne origine du pack et l'ancien loader.
//!
//! Hors périmètre : les mises à jour qui changent la version de Minecraft ou le loader (une nouvelle instance est plus sûre : mondes et mods ne suivraient pas).

use std::collections::{BTreeMap, HashSet};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use futures_util::stream::{self, StreamExt};
use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha512};
use zip::ZipArchive;

use crate::browse::{self, BrowseKind, Providers};
use crate::instance::InstanceStore;
use crate::mods::{self, Provenance};
use crate::mrpack::{
    self, FilesProgress, ImportOptions, Layout, MrpackFile, MrpackIndex, OverrideEntry,
};
use crate::providers::curseforge::CfResolver;
use crate::providers::modrinth_dl::{DlOptions, StoredObject};
use crate::store::index::StoreIndex;
use crate::store::link::link_into;
use crate::types::{Error, Instance, Loader, LoaderSpec, PackOrigin, Result};

/// Dossier des sauvegardes, à la racine du dossier de jeu (hors de `mods/`, il couvre aussi `config/`...).
pub const BACKUP_DIR: &str = ".cubelink-pack-backup";
/// Sauvegardes conservées par instance.
const KEEP_BACKUPS: usize = 3;

fn invalid(m: impl Into<String>) -> Error {
    Error::Invalid(m.into())
}

mod apply;
mod backup;
mod check;
mod curseforge;
mod record;
mod digest;
mod plan;
mod state;
mod undo;
pub use apply::*;
use backup::*;
pub use check::*;
pub use record::needs_old_pack;
use digest::*;
use plan::*;
use state::*;
pub use undo::*;

// ---------- Contrat (sérialisé vers l'interface) ----------

/// Mise à jour proposée pour un modpack installé.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, specta::Type)]
pub struct ModpackUpdate {
    /// Identifiant de la version Modrinth à installer.
    pub version_id: String,
    /// Nom affiché de la version (« Mon pack 1.4.2 »).
    pub name: String,
    /// Numéro de version (« 1.4.2 »).
    pub version_number: String,
    /// Date de publication ISO 8601.
    pub published: String,
}

/// Résultat d'une mise à jour appliquée.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, specta::Type)]
pub struct ModpackUpdateResult {
    pub added: u32,
    pub removed: u32,
    pub replaced: u32,
    /// Chemins (relatifs à l'instance) que le joueur a modifiés ou ajoutés lui-même et que la mise à jour n'a pas touchés.
    pub kept: Vec<String>,
    /// Fichiers du nouveau pack introuvables (mods CurseForge non redistribuables) : à poser par le joueur.
    pub missing: Vec<String>,
    /// Sauvegarde à passer à `modpack_update_undo`.
    pub backup_id: String,
}

#[cfg(test)]
mod tests;

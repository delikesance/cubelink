//! Hearth : serveurs de groupe P2P (voir `docs/hearth.md`).
//! Chaque sous-module a un seul propriétaire ; logique pure dans `model`, `version`, `lease`.
pub mod model;
pub mod version;
pub mod lease;
pub mod proto;
pub mod store;
pub mod sync;
pub mod outbox;

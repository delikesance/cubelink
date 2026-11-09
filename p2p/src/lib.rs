//! Cubelink P2P. Chaque module est la propriété d'une seule tâche.
pub mod identity;
pub mod friend_code;
pub mod verify_code;
pub mod friends_db;
pub mod requests;
pub mod friend_proto;
pub mod presence;
pub mod tunnel;
pub mod relay;
pub mod transfer;
pub mod manifest_channel;

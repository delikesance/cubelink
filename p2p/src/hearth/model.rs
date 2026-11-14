//! Hearth : modèle d'un groupe, chaîne d'opérations signées et gouvernance (voir `docs/hearth.md`, § 3.1).
//!
//! Logique pure, sans réseau ni disque : tout ce qui vient d'un pair est validé ici (tailles, signatures, règles).
//!
//! # Chaîne d'opérations
//!
//! L'état d'un groupe ([`GroupState`]) se reconstruit avec [`replay`] à partir d'une liste de [`SignedOp`]
//! numérotées `op_seq = 1, 2, …`. Chaque opération porte l'empreinte (BLAKE3) de la précédente (`prev`, 32 octets
//! à zéro pour la première) : la chaîne ne peut ni être réordonnée, ni tronquée au milieu, ni recoupée.
//!
//! # Encodage canonique et signature
//!
//! Le « corps » signé d'une opération est `{ op_seq, prev, op }`. Son encodage canonique est le JSON compact
//! (UTF-8, sans espace) produit par `serde_json` à partir des types ci-dessous : les structures ont un **ordre de
//! champs fixe** (celui de leur déclaration), les ensembles de clés sont des `BTreeSet` (ordre croissant), les
//! clés et signatures sont du hexadécimal minuscule, les énumérations portent leur étiquette en premier champ
//! (`"type"`). L'encodage est toujours recalculé à partir de la valeur typée, jamais copié depuis les octets reçus.
//!
//! * empreinte de l'opération = `BLAKE3(encodage canonique du corps)` (les signatures n'y entrent pas, sinon deux
//!   ensembles de signatures valides donneraient deux chaînes) ;
//! * message signé = [`OP_DOMAIN`] `||` encodage canonique du corps (séparation de domaine) ;
//! * signature = ed25519 stricte de la clé d'identité Cubelink.
//!
//! # Gouvernance
//!
//! | Opération | Autorisation |
//! |---|---|
//! | `Create` | un seul signataire : le créateur |
//! | `AddMember`, `RemoveMember`, `PackUpdate` | `Creator` : la signature du créateur ; `Majority` : majorité stricte des membres |
//! | `ContinueWithout` | majorité stricte des membres **hors absent** (l'absent ne peut pas signer) ; l'opération atteste
//! que l'absent n'a pas donné signe de vie depuis au moins 7 jours (`absent_since`, `at`) |
//! | `SetGovernance` | le créateur seul, ou la majorité stricte des autres membres une fois le créateur absent depuis
//! 30 jours de plus que le délai du `ContinueWithout` qui l'a écarté |
//! | `Leave` | le membre qui part, seul |
//!
//! Écarts et précisions par rapport à la spec :
//! * `Create` porte aussi l'identifiant du groupe (`id`) ; `members` liste les membres autres que le créateur
//!   (le créateur est toujours membre, il est ajouté d'office) ;
//! * un `Leave` du créateur sous gouvernance `Creator` bascule en `Majority` (sinon plus personne ne gouverne) ;
//! * `ContinueWithout` ne change **pas** la gouvernance, même quand l'absent est le créateur : les membres qui restent
//!   jouent sur la nouvelle époque mais n'invitent ni ne retirent personne tant que `SetGovernance` n'a pas basculé
//!   en `Majority`, ce qu'il ne peut faire que 30 jours après la décision ;
//! * le créateur ne peut pas être retiré par `RemoveMember` tant que la gouvernance est `Creator` ;
//! * le dernier membre ne peut ni partir ni être retiré ;
//! * le fait que la clé ajoutée soit un ami du signataire n'est pas vérifiable ici : c'est au protocole.

use std::collections::BTreeSet;
use std::fmt;

use core_lib::types::{Loader, LoaderSpec};
use ed25519_dalek::{Signature, Signer as _, SigningKey, VerifyingKey};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use thiserror::Error;

use super::version::VersionId;
use crate::identity::Identity;
use crate::presence::PeerKey;

/// Séparation de domaine des signatures d'opérations.
pub const OP_DOMAIN: &[u8] = b"cubelink/hearth/op/1";
/// Nombre maximal de membres d'un groupe.
pub const MAX_MEMBERS: usize = 64;
/// Longueur maximale (en caractères) du nom d'un groupe.
pub const MAX_NAME_CHARS: usize = 64;
/// Nombre maximal d'opérations dans une chaîne (borne la mémoire et le temps de rejeu).
pub const MAX_OPS: usize = 10_000;
/// Absence minimale avant de continuer sans un membre : 7 jours sans signe de vie.
pub const MIN_ABSENCE_SECS: u64 = 7 * 24 * 3600;
/// Délai, après un `ContinueWithout` visant le créateur, avant que les autres membres puissent passer en `Majority`.
pub const DORMANCY_SECS: u64 = 30 * 24 * 3600;
/// Empreinte d'une opération (BLAKE3).
pub type OpHash = [u8; 32];
/// Valeur de `prev` de la première opération.
pub const GENESIS_PREV: OpHash = [0; 32];

const MAX_MC_CHARS: usize = 32;
const MAX_LOADER_VERSION_CHARS: usize = 64;

// ---------------------------------------------------------------------------------------------------------
// Sérialisation hexadécimale (clés, empreintes, signatures)
// ---------------------------------------------------------------------------------------------------------

/// Décode exactement `N` octets depuis du hexadécimal.
pub(crate) fn decode_array<const N: usize>(s: &str) -> Result<[u8; N], &'static str> {
    if s.len() != 2 * N {
        return Err("longueur hexadécimale invalide");
    }
    let bytes = hex::decode(s).map_err(|_| "hexadécimal invalide")?;
    bytes.try_into().map_err(|_| "longueur hexadécimale invalide")
}

/// `#[serde(with = "hex_bytes")]` : tableau d'octets de taille fixe en chaîne hexadécimale minuscule.
pub(crate) mod hex_bytes {
    use serde::de::Error;
    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S: Serializer, const N: usize>(b: &[u8; N], s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&hex::encode(b))
    }

    pub fn deserialize<'de, D: Deserializer<'de>, const N: usize>(d: D) -> Result<[u8; N], D::Error> {
        let s = String::deserialize(d)?;
        super::decode_array::<N>(&s).map_err(D::Error::custom)
    }
}

/// `#[serde(with = "hex_key_set")]` : ensemble de clés, tableau JSON de chaînes hexadécimales triées.
pub(crate) mod hex_key_set {
    use std::collections::BTreeSet;

    use serde::de::Error;
    use serde::{Deserialize, Deserializer, Serializer};

    use crate::presence::PeerKey;

    pub fn serialize<S: Serializer>(set: &BTreeSet<PeerKey>, s: S) -> Result<S::Ok, S::Error> {
        s.collect_seq(set.iter().map(hex::encode))
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<BTreeSet<PeerKey>, D::Error> {
        let raw = Vec::<String>::deserialize(d)?;
        if raw.len() > super::MAX_MEMBERS {
            return Err(D::Error::custom("trop de clés"));
        }
        let mut out = BTreeSet::new();
        for h in &raw {
            let key = super::decode_array::<32>(h).map_err(D::Error::custom)?;
            if !out.insert(key) {
                return Err(D::Error::custom("clé en double"));
            }
        }
        Ok(out)
    }
}

/// `#[serde(with = "sig_list")]` : liste de couples `[clé, signature]` en hexadécimal (au plus `MAX_MEMBERS`).
pub(crate) mod sig_list {
    use serde::de::Error;
    use serde::ser::SerializeSeq;
    use serde::{Deserialize, Deserializer, Serializer};

    use super::Sig;
    use crate::presence::PeerKey;

    pub fn serialize<S: Serializer>(v: &[(PeerKey, Sig)], s: S) -> Result<S::Ok, S::Error> {
        let mut seq = s.serialize_seq(Some(v.len()))?;
        for (k, sig) in v {
            seq.serialize_element(&(hex::encode(k), sig))?;
        }
        seq.end()
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Vec<(PeerKey, Sig)>, D::Error> {
        let raw = Vec::<(String, Sig)>::deserialize(d)?;
        if raw.len() > super::MAX_MEMBERS {
            return Err(D::Error::custom("trop de signatures"));
        }
        raw.into_iter()
            .map(|(k, sig)| Ok((super::decode_array::<32>(&k).map_err(D::Error::custom)?, sig)))
            .collect()
    }
}

/// Signature ed25519 (64 octets), sérialisée en hexadécimal.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct Sig(pub [u8; 64]);

impl fmt::Debug for Sig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Sig({})", &hex::encode(self.0)[..16])
    }
}

impl Serialize for Sig {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&hex::encode(self.0))
    }
}

impl<'de> Deserialize<'de> for Sig {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        use serde::de::Error;
        let s = String::deserialize(d)?;
        decode_array::<64>(&s).map(Sig).map_err(D::Error::custom)
    }
}

// ---------------------------------------------------------------------------------------------------------
// Signature par domaine
// ---------------------------------------------------------------------------------------------------------

/// Ce qui sait signer pour Hearth : l'identité de l'utilisateur (et, pour les tests, une clé ed25519 brute).
pub trait HearthSigner {
    /// Clé publique de l'identité.
    fn peer_key(&self) -> PeerKey;
    /// Signature ed25519 brute du message.
    fn sign_raw(&self, msg: &[u8]) -> Sig;
}

impl HearthSigner for Identity {
    fn peer_key(&self) -> PeerKey {
        self.public_key().to_bytes()
    }
    fn sign_raw(&self, msg: &[u8]) -> Sig {
        Sig(self.sign(msg).to_bytes())
    }
}

impl HearthSigner for SigningKey {
    fn peer_key(&self) -> PeerKey {
        self.verifying_key().to_bytes()
    }
    fn sign_raw(&self, msg: &[u8]) -> Sig {
        Sig(self.sign(msg).to_bytes())
    }
}

fn domain_message(domain: &[u8], body: &[u8]) -> Vec<u8> {
    let mut m = Vec::with_capacity(domain.len() + body.len());
    m.extend_from_slice(domain);
    m.extend_from_slice(body);
    m
}

/// Signe `domaine || corps`.
pub(crate) fn sign_domain(signer: &dyn HearthSigner, domain: &[u8], body: &[u8]) -> Sig {
    signer.sign_raw(&domain_message(domain, body))
}

/// Vérifie (ed25519 strict) la signature de `domaine || corps` par `key`. Une clé invalide donne `false`.
pub(crate) fn verify_domain(key: &PeerKey, domain: &[u8], body: &[u8], sig: &Sig) -> bool {
    let Ok(vk) = VerifyingKey::from_bytes(key) else { return false };
    vk.verify_strict(&domain_message(domain, body), &Signature::from_bytes(&sig.0)).is_ok()
}

/// Texte saisi : non vide après `trim`, borné, sans caractère de contrôle.
pub(crate) fn text_ok(s: &str, max_chars: usize) -> bool {
    !s.trim().is_empty() && s.chars().count() <= max_chars && !s.chars().any(char::is_control)
}

/// Chaîne hexadécimale de 64 caractères minuscules (SHA-256 ou BLAKE3).
pub(crate) fn is_hash_hex(s: &str) -> bool {
    s.len() == 64 && s.bytes().all(|c| matches!(c, b'0'..=b'9' | b'a'..=b'f'))
}

// ---------------------------------------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------------------------------------

/// Identifiant d'un groupe : 16 octets aléatoires (hexadécimal dans les noms de fichiers et le JSON).
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct GroupId(pub [u8; 16]);

impl GroupId {
    /// Nouvel identifiant aléatoire.
    pub fn random() -> Self {
        use rand::RngCore;
        let mut b = [0u8; 16];
        rand::rngs::OsRng.fill_bytes(&mut b);
        Self(b)
    }

    pub fn to_hex(&self) -> String {
        hex::encode(self.0)
    }

    /// Décode 32 caractères hexadécimaux (casse indifférente).
    pub fn from_hex(s: &str) -> Option<Self> {
        decode_array::<16>(s).ok().map(Self)
    }
}

impl fmt::Display for GroupId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.to_hex())
    }
}

impl fmt::Debug for GroupId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "GroupId({})", self.to_hex())
    }
}

impl Serialize for GroupId {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&self.to_hex())
    }
}

impl<'de> Deserialize<'de> for GroupId {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        use serde::de::Error;
        let s = String::deserialize(d)?;
        decode_array::<16>(&s).map(Self).map_err(D::Error::custom)
    }
}

/// Modpack épinglé par le groupe.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PackPin {
    /// Empreinte SHA-256 du manifeste (64 caractères hexadécimaux minuscules).
    pub manifest_hash: String,
    pub minecraft: String,
    pub loader: LoaderSpec,
}

impl PackPin {
    /// Valide les champs (le contenu vient d'un pair).
    pub fn validate(&self) -> Result<(), ModelError> {
        if !is_hash_hex(&self.manifest_hash) {
            return Err(ModelError::InvalidPack("empreinte du manifeste"));
        }
        if !text_ok(&self.minecraft, MAX_MC_CHARS) {
            return Err(ModelError::InvalidPack("version de Minecraft"));
        }
        let v = &self.loader.version;
        let version_ok = if self.loader.loader == Loader::Vanilla {
            v.chars().count() <= MAX_LOADER_VERSION_CHARS && !v.chars().any(char::is_control)
        } else {
            text_ok(v, MAX_LOADER_VERSION_CHARS)
        };
        if !version_ok {
            return Err(ModelError::InvalidPack("version du loader"));
        }
        Ok(())
    }
}

/// Qui peut décider des changements du groupe.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Governance {
    /// Le créateur seul.
    Creator,
    /// Majorité stricte des membres.
    Majority,
}

/// Passage à une nouvelle époque (`ContinueWithout`), conservé pour classer les versions du monde.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Continuation {
    /// Époque qui commence.
    pub epoch: u64,
    #[serde(with = "hex_bytes")]
    pub absent: PeerKey,
    /// Version de départ de la nouvelle époque.
    pub base_version: VersionId,
    /// Numéro de l'opération qui l'a décidé.
    pub op_seq: u64,
    /// Heure (secondes Unix, horloge du proposant) à laquelle la décision a été prise.
    pub at: u64,
}

/// État d'un groupe, obtenu en rejouant la chaîne d'opérations.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GroupState {
    pub id: GroupId,
    pub name: String,
    pub pack: PackPin,
    #[serde(with = "hex_bytes")]
    pub creator: PeerKey,
    #[serde(with = "hex_key_set")]
    pub members: BTreeSet<PeerKey>,
    pub governance: Governance,
    pub epoch: u64,
    /// Base de l'époque courante (`None` tant qu'aucun `ContinueWithout` n'a eu lieu).
    pub base_version: Option<VersionId>,
    /// Numéro de la dernière opération appliquée.
    pub op_seq: u64,
    /// Empreinte de la dernière opération appliquée (`prev` de la suivante).
    #[serde(with = "hex_bytes")]
    pub head: OpHash,
    /// Un élément par `ContinueWithout`, dans l'ordre (l'époque `n` est `continuations[n - 1]`).
    pub continuations: Vec<Continuation>,
}

/// Opération sur un groupe.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum GroupOp {
    /// Crée le groupe. Signée par le créateur seul ; `members` : les membres autres que le créateur.
    Create {
        id: GroupId,
        name: String,
        pack: PackPin,
        #[serde(with = "hex_key_set")]
        members: BTreeSet<PeerKey>,
    },
    AddMember {
        #[serde(with = "hex_bytes")]
        key: PeerKey,
    },
    RemoveMember {
        #[serde(with = "hex_bytes")]
        key: PeerKey,
    },
    ContinueWithout {
        #[serde(with = "hex_bytes")]
        absent: PeerKey,
        base_version: VersionId,
        /// Dernier signe de vie de l'absent connu du proposant (secondes Unix).
        absent_since: u64,
        /// Heure de la décision (secondes Unix) : au moins `MIN_ABSENCE_SECS` après `absent_since`.
        at: u64,
    },
    PackUpdate {
        pack: PackPin,
    },
    /// Passe la gouvernance à la majorité. `at` : heure de la décision (secondes Unix).
    SetGovernance {
        at: u64,
    },
    Leave,
}

/// Corps signé d'une opération (voir l'encodage canonique en tête de module).
#[derive(Serialize)]
struct BodyRef<'a> {
    op_seq: u64,
    #[serde(with = "hex_bytes")]
    prev: OpHash,
    op: &'a GroupOp,
}

/// Encodage canonique du corps d'une opération.
pub fn canonical_body(op_seq: u64, prev: &OpHash, op: &GroupOp) -> Vec<u8> {
    // Chaînes, entiers et énumérations sans clé non textuelle : la sérialisation ne peut pas échouer.
    serde_json::to_vec(&BodyRef { op_seq, prev: *prev, op }).expect("sérialisation JSON de types simples")
}

/// Empreinte d'une opération (BLAKE3 de l'encodage canonique du corps).
pub fn op_hash(op_seq: u64, prev: &OpHash, op: &GroupOp) -> OpHash {
    *blake3::hash(&canonical_body(op_seq, prev, op)).as_bytes()
}

/// Opération signée, prête à être rejouée.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SignedOp {
    pub op_seq: u64,
    #[serde(with = "hex_bytes")]
    pub prev: OpHash,
    pub op: GroupOp,
    #[serde(with = "sig_list")]
    pub sigs: Vec<(PeerKey, Sig)>,
}

impl SignedOp {
    /// Empreinte de l'opération.
    pub fn hash(&self) -> OpHash {
        op_hash(self.op_seq, &self.prev, &self.op)
    }

    /// Opération `Create` signée par le créateur. `others` : les autres membres (le créateur est ajouté d'office).
    pub fn create(
        signer: &dyn HearthSigner,
        id: GroupId,
        name: &str,
        pack: PackPin,
        others: &BTreeSet<PeerKey>,
    ) -> Result<Self, ModelError> {
        let creator = signer.peer_key();
        let mut members = others.clone();
        members.remove(&creator);
        let op = GroupOp::Create { id, name: name.to_string(), pack, members };
        check_create(&op)?;
        let body = canonical_body(1, &GENESIS_PREV, &op);
        let sig = sign_domain(signer, OP_DOMAIN, &body);
        Ok(Self { op_seq: 1, prev: GENESIS_PREV, op, sigs: vec![(creator, sig)] })
    }
}

fn check_create(op: &GroupOp) -> Result<(), ModelError> {
    let GroupOp::Create { name, pack, members, .. } = op else { return Err(ModelError::FirstMustBeCreate) };
    if !text_ok(name, MAX_NAME_CHARS) {
        return Err(ModelError::InvalidName);
    }
    pack.validate()?;
    // Le créateur s'ajoute : `members` ne doit donc pas dépasser `MAX_MEMBERS - 1`.
    if members.len() >= MAX_MEMBERS {
        return Err(ModelError::TooManyMembers);
    }
    Ok(())
}

/// Erreurs de validation du modèle (messages affichables).
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum ModelError {
    #[error("nom de groupe invalide (1 à 64 caractères, sans caractère de contrôle)")]
    InvalidName,
    #[error("modpack épinglé invalide : {0}")]
    InvalidPack(&'static str),
    #[error("trop de membres (64 au maximum)")]
    TooManyMembers,
    #[error("chaîne d'opérations vide")]
    EmptyChain,
    #[error("chaîne d'opérations trop longue")]
    TooManyOps,
    #[error("la première opération doit créer le groupe")]
    FirstMustBeCreate,
    #[error("la création du groupe ne peut pas être répétée")]
    CreateNotFirst,
    #[error("la création doit être signée par le seul créateur")]
    BadCreateSigners,
    #[error("numéro d'opération {got} au lieu de {expected}")]
    BadSeq { expected: u64, got: u64 },
    #[error("l'opération ne suit pas la précédente (empreinte différente)")]
    BrokenChain,
    #[error("aucune signature")]
    NoSignature,
    #[error("trop de signatures")]
    TooManySignatures,
    #[error("signataire en double")]
    DuplicateSigner,
    #[error("signature invalide")]
    BadSignature,
    #[error("signataire ou cible qui n'est pas membre du groupe")]
    NotMember,
    #[error("déjà membre du groupe")]
    AlreadyMember,
    #[error("signatures insuffisantes ({have} sur {need})")]
    NotAuthorized { have: usize, need: usize },
    #[error("le membre absent ne peut pas voter sa propre mise à l'écart")]
    AbsentCannotVote,
    #[error("le créateur ne peut pas être retiré tant qu'il gouverne seul")]
    CannotRemoveCreator,
    #[error("le dernier membre ne peut pas quitter le groupe")]
    LastMember,
    #[error("le modpack épinglé est déjà celui-ci")]
    PackUnchanged,
    #[error("il faut au moins deux membres pour continuer sans l'un d'eux")]
    NeedsTwoMembers,
    #[error("proposition périmée : la chaîne a avancé")]
    StaleProposal,
    #[error("on ne continue sans un membre qu'après 7 jours sans signe de vie de sa part")]
    AbsenceTooShort,
    #[error("le groupe est déjà gouverné à la majorité")]
    AlreadyMajority,
    #[error("le créateur n'est pas absent depuis assez longtemps pour que les autres membres décident à sa place")]
    CreatorNotDormant,
}

// ---------------------------------------------------------------------------------------------------------
// État du groupe
// ---------------------------------------------------------------------------------------------------------

/// Vérifie toutes les signatures d'un corps et renvoie l'ensemble des signataires.
fn verify_signatures(body: &[u8], sigs: &[(PeerKey, Sig)]) -> Result<BTreeSet<PeerKey>, ModelError> {
    if sigs.len() > MAX_MEMBERS {
        return Err(ModelError::TooManySignatures);
    }
    let mut signers = BTreeSet::new();
    for (key, sig) in sigs {
        if !verify_domain(key, OP_DOMAIN, body, sig) {
            return Err(ModelError::BadSignature);
        }
        if !signers.insert(*key) {
            return Err(ModelError::DuplicateSigner);
        }
    }
    Ok(signers)
}

impl GroupState {
    /// État initial issu d'une opération `Create` valide (signature comprise).
    pub fn from_create(signed: &SignedOp) -> Result<Self, ModelError> {
        if signed.op_seq != 1 {
            return Err(ModelError::BadSeq { expected: 1, got: signed.op_seq });
        }
        if signed.prev != GENESIS_PREV {
            return Err(ModelError::BrokenChain);
        }
        let GroupOp::Create { id, name, pack, members } = &signed.op else {
            return Err(ModelError::FirstMustBeCreate);
        };
        check_create(&signed.op)?;
        if signed.sigs.len() != 1 {
            return Err(ModelError::BadCreateSigners);
        }
        let body = canonical_body(signed.op_seq, &signed.prev, &signed.op);
        let signers = verify_signatures(&body, &signed.sigs)?;
        let creator = signed.sigs[0].0;
        debug_assert!(signers.contains(&creator));
        let mut all = members.clone();
        all.insert(creator);
        Ok(Self {
            id: *id,
            name: name.clone(),
            pack: pack.clone(),
            creator,
            members: all,
            governance: Governance::Creator,
            epoch: 0,
            base_version: None,
            op_seq: 1,
            head: signed.hash(),
            continuations: Vec::new(),
        })
    }

    /// Le créateur a-t-il été écarté par un `ContinueWithout` décidé il y a plus de 30 jours à l'heure `at` ?
    pub fn creator_dormant(&self, at: u64) -> bool {
        self.continuations.iter().any(|c| c.absent == self.creator && at >= c.at.saturating_add(DORMANCY_SECS))
    }

    pub fn is_member(&self, key: &PeerKey) -> bool {
        self.members.contains(key)
    }

    /// Numéro que doit porter la prochaine opération.
    pub fn next_op_seq(&self) -> u64 {
        self.op_seq + 1
    }

    /// Base de l'époque `epoch` (`None` pour l'époque 0 ou une époque inconnue).
    pub fn base_for_epoch(&self, epoch: u64) -> Option<VersionId> {
        let idx = usize::try_from(epoch.checked_sub(1)?).ok()?;
        self.continuations.get(idx).map(|c| c.base_version)
    }

    /// Nombre de signatures de membres éligibles qu'il faut pour `op`.
    pub fn votes_needed(&self, op: &GroupOp) -> usize {
        match op {
            GroupOp::Create { .. } | GroupOp::Leave => 1,
            GroupOp::ContinueWithout { absent, .. } => {
                let others = self.members.len() - usize::from(self.members.contains(absent));
                others / 2 + 1
            }
            // Le créateur seul, ou la majorité stricte des autres membres (voir `check_authorization`).
            GroupOp::SetGovernance { .. } => 1,
            _ => match self.governance {
                Governance::Creator => 1,
                Governance::Majority => self.members.len() / 2 + 1,
            },
        }
    }

    /// Règles de l'opération elle-même, indépendamment de ses signataires.
    pub fn validate_op(&self, op: &GroupOp) -> Result<(), ModelError> {
        match op {
            GroupOp::Create { .. } => Err(ModelError::CreateNotFirst),
            GroupOp::AddMember { key } => {
                if self.members.contains(key) {
                    Err(ModelError::AlreadyMember)
                } else if self.members.len() >= MAX_MEMBERS {
                    Err(ModelError::TooManyMembers)
                } else {
                    Ok(())
                }
            }
            GroupOp::RemoveMember { key } => {
                if !self.members.contains(key) {
                    Err(ModelError::NotMember)
                } else if self.members.len() <= 1 {
                    Err(ModelError::LastMember)
                } else if self.governance == Governance::Creator && *key == self.creator {
                    Err(ModelError::CannotRemoveCreator)
                } else {
                    Ok(())
                }
            }
            GroupOp::ContinueWithout { absent, absent_since, at, .. } => {
                if !self.members.contains(absent) {
                    Err(ModelError::NotMember)
                } else if self.members.len() < 2 {
                    Err(ModelError::NeedsTwoMembers)
                } else if *at < absent_since.saturating_add(MIN_ABSENCE_SECS) {
                    Err(ModelError::AbsenceTooShort)
                } else {
                    Ok(())
                }
            }
            GroupOp::SetGovernance { .. } => {
                if self.governance == Governance::Majority {
                    Err(ModelError::AlreadyMajority)
                } else {
                    Ok(())
                }
            }
            GroupOp::PackUpdate { pack } => {
                pack.validate()?;
                if *pack == self.pack {
                    Err(ModelError::PackUnchanged)
                } else {
                    Ok(())
                }
            }
            GroupOp::Leave => {
                if self.members.len() <= 1 {
                    Err(ModelError::LastMember)
                } else {
                    Ok(())
                }
            }
        }
    }

    /// Les signataires (déjà vérifiés cryptographiquement, tous membres) suffisent-ils pour `op` ?
    pub fn check_authorization(&self, op: &GroupOp, signers: &BTreeSet<PeerKey>) -> Result<(), ModelError> {
        if signers.iter().any(|k| !self.members.contains(k)) {
            return Err(ModelError::NotMember);
        }
        let need = self.votes_needed(op);
        match op {
            GroupOp::Create { .. } => Err(ModelError::CreateNotFirst),
            GroupOp::Leave => {
                if signers.len() == 1 {
                    Ok(())
                } else {
                    Err(ModelError::NotAuthorized { have: signers.len(), need: 1 })
                }
            }
            GroupOp::ContinueWithout { absent, .. } => {
                if signers.contains(absent) {
                    return Err(ModelError::AbsentCannotVote);
                }
                enough(signers.len(), need)
            }
            GroupOp::SetGovernance { at } => {
                if signers.contains(&self.creator) {
                    // Le créateur cède lui-même ; ses signatures ne se mélangent pas à celles d'une prise de pouvoir.
                    return if signers.len() == 1 { Ok(()) } else { Err(ModelError::NotAuthorized { have: signers.len(), need: 1 }) };
                }
                if !self.creator_dormant(*at) {
                    return Err(ModelError::CreatorNotDormant);
                }
                enough(signers.len(), (self.members.len() - 1) / 2 + 1)
            }
            _ => match self.governance {
                Governance::Creator if signers.contains(&self.creator) => Ok(()),
                Governance::Creator => Err(ModelError::NotAuthorized { have: 0, need: 1 }),
                Governance::Majority => enough(signers.len(), need),
            },
        }
    }

    /// Applique une opération signée (chaînage, signatures, règles). L'état n'est modifié qu'en cas de succès.
    pub fn apply(&mut self, signed: &SignedOp) -> Result<(), ModelError> {
        if signed.op_seq != self.next_op_seq() {
            return Err(ModelError::BadSeq { expected: self.next_op_seq(), got: signed.op_seq });
        }
        if signed.prev != self.head {
            return Err(ModelError::BrokenChain);
        }
        if signed.sigs.is_empty() {
            return Err(ModelError::NoSignature);
        }
        let body = canonical_body(signed.op_seq, &signed.prev, &signed.op);
        let signers = verify_signatures(&body, &signed.sigs)?;
        self.validate_op(&signed.op)?;
        self.check_authorization(&signed.op, &signers)?;

        match &signed.op {
            GroupOp::Create { .. } => return Err(ModelError::CreateNotFirst),
            GroupOp::AddMember { key } => {
                self.members.insert(*key);
            }
            GroupOp::RemoveMember { key } => {
                self.members.remove(key);
            }
            GroupOp::ContinueWithout { absent, base_version, at, .. } => {
                self.epoch += 1;
                self.base_version = Some(*base_version);
                self.continuations.push(Continuation {
                    epoch: self.epoch,
                    absent: *absent,
                    base_version: *base_version,
                    op_seq: signed.op_seq,
                    at: *at,
                });
            }
            GroupOp::SetGovernance { .. } => self.governance = Governance::Majority,
            GroupOp::PackUpdate { pack } => self.pack = pack.clone(),
            GroupOp::Leave => {
                // `check_authorization` garantit un seul signataire, membre.
                for key in &signers {
                    self.members.remove(key);
                    if *key == self.creator {
                        self.governance = Governance::Majority;
                    }
                }
            }
        }
        self.op_seq = signed.op_seq;
        self.head = signed.hash();
        Ok(())
    }

    /// Signe seul une opération qu'un seul signataire suffit à autoriser (crée une proposition, vote, finalise).
    pub fn sign_single(&self, op: GroupOp, signer: &dyn HearthSigner) -> Result<SignedOp, ModelError> {
        let mut p = Proposal::new(self, op)?;
        p.vote(self, signer)?;
        p.into_signed(self)
    }
}

fn enough(have: usize, need: usize) -> Result<(), ModelError> {
    if have >= need {
        Ok(())
    } else {
        Err(ModelError::NotAuthorized { have, need })
    }
}

/// Reconstruit l'état d'un groupe en rejouant sa chaîne : numéros, chaînage, signatures et règles de chaque
/// opération sont vérifiés. La moindre irrégularité est une erreur (jamais d'état partiel).
pub fn replay(ops: &[SignedOp]) -> Result<GroupState, ModelError> {
    if ops.len() > MAX_OPS {
        return Err(ModelError::TooManyOps);
    }
    let (first, rest) = ops.split_first().ok_or(ModelError::EmptyChain)?;
    let mut state = GroupState::from_create(first)?;
    for op in rest {
        state.apply(op)?;
    }
    Ok(state)
}

// ---------------------------------------------------------------------------------------------------------
// Propositions et votes
// ---------------------------------------------------------------------------------------------------------

/// Opération en attente de signatures : chacun vote à son prochain lancement, la proposition devient une
/// [`SignedOp`] quand le seuil est atteint. Elle est liée à la tête de chaîne qui l'a vue naître : si la chaîne
/// avance, elle est périmée (`StaleProposal`) et doit être refaite.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Proposal {
    pub op_seq: u64,
    #[serde(with = "hex_bytes")]
    pub prev: OpHash,
    pub op: GroupOp,
    #[serde(with = "sig_list")]
    pub sigs: Vec<(PeerKey, Sig)>,
}

impl Proposal {
    /// Nouvelle proposition sans signature, après les règles de l'opération.
    pub fn new(state: &GroupState, op: GroupOp) -> Result<Self, ModelError> {
        state.validate_op(&op)?;
        Ok(Self { op_seq: state.next_op_seq(), prev: state.head, op, sigs: Vec::new() })
    }

    /// Identifiant de la proposition : l'empreinte de l'opération qu'elle deviendra (les votes s'y réfèrent).
    pub fn id(&self) -> OpHash {
        op_hash(self.op_seq, &self.prev, &self.op)
    }

    fn check_base(&self, state: &GroupState) -> Result<(), ModelError> {
        if self.op_seq != state.next_op_seq() || self.prev != state.head {
            return Err(ModelError::StaleProposal);
        }
        Ok(())
    }

    /// Valide une proposition reçue : base, règles de l'opération, signatures (toutes valides, de membres).
    pub fn validate(&self, state: &GroupState) -> Result<(), ModelError> {
        self.check_base(state)?;
        state.validate_op(&self.op)?;
        let signers = verify_signatures(&canonical_body(self.op_seq, &self.prev, &self.op), &self.sigs)?;
        if signers.iter().any(|k| !state.is_member(k)) {
            return Err(ModelError::NotMember);
        }
        if let GroupOp::ContinueWithout { absent, .. } = &self.op {
            if signers.contains(absent) {
                return Err(ModelError::AbsentCannotVote);
            }
        }
        Ok(())
    }

    /// Ajoute le vote de `key` (message `Vote { proposal_id, sig }`). `Ok(true)` si le vote est nouveau,
    /// `Ok(false)` s'il était déjà là.
    pub fn add_vote(&mut self, state: &GroupState, key: PeerKey, sig: Sig) -> Result<bool, ModelError> {
        self.check_base(state)?;
        state.validate_op(&self.op)?;
        if !state.is_member(&key) {
            return Err(ModelError::NotMember);
        }
        if let GroupOp::ContinueWithout { absent, .. } = &self.op {
            if *absent == key {
                return Err(ModelError::AbsentCannotVote);
            }
        }
        if !verify_domain(&key, OP_DOMAIN, &canonical_body(self.op_seq, &self.prev, &self.op), &sig) {
            return Err(ModelError::BadSignature);
        }
        if self.sigs.iter().any(|(k, _)| *k == key) {
            return Ok(false);
        }
        if self.sigs.len() >= MAX_MEMBERS {
            return Err(ModelError::TooManySignatures);
        }
        self.sigs.push((key, sig));
        Ok(true)
    }

    /// Signature de `signer` sur cette proposition (à envoyer en `Vote`), sans l'ajouter.
    pub fn sign(&self, signer: &dyn HearthSigner) -> Sig {
        sign_domain(signer, OP_DOMAIN, &canonical_body(self.op_seq, &self.prev, &self.op))
    }

    /// Vote de `signer` (ajouté à la proposition).
    pub fn vote(&mut self, state: &GroupState, signer: &dyn HearthSigner) -> Result<bool, ModelError> {
        let sig = self.sign(signer);
        self.add_vote(state, signer.peer_key(), sig)
    }

    fn signers(&self) -> BTreeSet<PeerKey> {
        self.sigs.iter().map(|(k, _)| *k).collect()
    }

    /// `(votes éligibles, votes nécessaires)`, pour afficher « 2 sur 3 ».
    pub fn approvals(&self, state: &GroupState) -> (usize, usize) {
        let have = self.signers().iter().filter(|k| state.is_member(k)).count();
        (have, state.votes_needed(&self.op))
    }

    /// Le seuil est-il atteint ?
    pub fn is_ready(&self, state: &GroupState) -> bool {
        self.check_base(state).is_ok() && state.check_authorization(&self.op, &self.signers()).is_ok()
    }

    /// Transforme la proposition en opération signée si le seuil est atteint.
    pub fn into_signed(self, state: &GroupState) -> Result<SignedOp, ModelError> {
        self.check_base(state)?;
        if self.sigs.is_empty() {
            return Err(ModelError::NoSignature);
        }
        state.check_authorization(&self.op, &self.signers())?;
        Ok(SignedOp { op_seq: self.op_seq, prev: self.prev, op: self.op, sigs: self.sigs })
    }
}

// ---------------------------------------------------------------------------------------------------------
// Aides de test partagées avec `version` et `lease`
// ---------------------------------------------------------------------------------------------------------

#[cfg(test)]
pub(crate) mod testutil {
    use super::*;

    pub fn key(seed: u8) -> SigningKey {
        SigningKey::from_bytes(&[seed; 32])
    }

    pub fn pack() -> PackPin {
        PackPin {
            manifest_hash: "ab".repeat(32),
            minecraft: "1.21.1".into(),
            loader: LoaderSpec { loader: Loader::NeoForge, version: "21.1.77".into() },
        }
    }

    /// Groupe créé par la clé `seeds[0]`, avec les autres graines comme membres.
    pub fn group(seeds: &[u8]) -> GroupState {
        let creator = key(seeds[0]);
        let others: BTreeSet<PeerKey> = seeds[1..].iter().map(|s| key(*s).peer_key()).collect();
        let op = SignedOp::create(&creator, GroupId([7; 16]), "Les copains", pack(), &others).unwrap();
        GroupState::from_create(&op).unwrap()
    }
}

#[cfg(test)]
mod tests {
    use super::testutil::*;
    use super::*;
    use proptest::prelude::*;

    fn pk(seed: u8) -> PeerKey {
        key(seed).peer_key()
    }

    fn chain_start(seeds: &[u8]) -> (Vec<SignedOp>, GroupState) {
        let creator = key(seeds[0]);
        let others: BTreeSet<PeerKey> = seeds[1..].iter().map(|s| pk(*s)).collect();
        let op = SignedOp::create(&creator, GroupId([7; 16]), "Les copains", pack(), &others).unwrap();
        let state = GroupState::from_create(&op).unwrap();
        (vec![op], state)
    }

    fn push(ops: &mut Vec<SignedOp>, state: &mut GroupState, signed: SignedOp) {
        state.apply(&signed).unwrap();
        ops.push(signed);
    }

    fn vid(n: u8) -> VersionId {
        VersionId([n; 32])
    }

    /// Instant de référence des tests (secondes Unix) et `ContinueWithout` décidé 7 jours après le dernier signe de vie.
    const T0: u64 = 1_700_000_000;

    fn cw(absent: PeerKey, base: VersionId) -> GroupOp {
        GroupOp::ContinueWithout { absent, base_version: base, absent_since: T0, at: T0 + MIN_ABSENCE_SECS }
    }

    /// Applique `op` signée par les clés `signers` (graines).
    fn run(ops: &mut Vec<SignedOp>, st: &mut GroupState, op: GroupOp, signers: &[u8]) -> Result<(), ModelError> {
        let mut p = Proposal::new(st, op)?;
        for s in signers {
            p.vote(st, &key(*s))?;
        }
        let signed = p.into_signed(st)?;
        st.apply(&signed)?;
        ops.push(signed);
        Ok(())
    }

    #[test]
    fn create_sets_initial_state() {
        let (ops, st) = chain_start(&[1, 2, 3]);
        assert_eq!(st.creator, pk(1));
        assert_eq!(st.members.len(), 3);
        assert_eq!(st.governance, Governance::Creator);
        assert_eq!((st.epoch, st.op_seq, st.base_version), (0, 1, None));
        assert_eq!(replay(&ops).unwrap(), st);
        assert_eq!(st.head, ops[0].hash());
    }

    #[test]
    fn create_rejects_bad_inputs() {
        let c = key(1);
        let none = BTreeSet::new();
        assert_eq!(SignedOp::create(&c, GroupId([1; 16]), "  ", pack(), &none).unwrap_err(), ModelError::InvalidName);
        let long = "x".repeat(65);
        assert_eq!(SignedOp::create(&c, GroupId([1; 16]), &long, pack(), &none).unwrap_err(), ModelError::InvalidName);
        assert!(SignedOp::create(&c, GroupId([1; 16]), &"é".repeat(64), pack(), &none).is_ok());
        assert_eq!(SignedOp::create(&c, GroupId([1; 16]), "a\nb", pack(), &none).unwrap_err(), ModelError::InvalidName);
        let mut bad = pack();
        bad.manifest_hash = "XYZ".into();
        assert!(matches!(
            SignedOp::create(&c, GroupId([1; 16]), "ok", bad, &none),
            Err(ModelError::InvalidPack(_))
        ));
        let mut bad = pack();
        bad.loader.version.clear();
        assert!(matches!(
            SignedOp::create(&c, GroupId([1; 16]), "ok", bad, &none),
            Err(ModelError::InvalidPack(_))
        ));
        // 64 membres : le créateur plus 63 autres passe, 64 autres non.
        let many = |n: u8| (10..10 + n).map(pk).collect::<BTreeSet<_>>();
        assert!(SignedOp::create(&c, GroupId([1; 16]), "ok", pack(), &many(63)).is_ok());
        assert_eq!(
            SignedOp::create(&c, GroupId([1; 16]), "ok", pack(), &many(64)).unwrap_err(),
            ModelError::TooManyMembers
        );
    }

    #[test]
    fn create_member_list_includes_creator_even_if_listed() {
        let c = key(1);
        let mut others = BTreeSet::new();
        others.insert(pk(1));
        others.insert(pk(2));
        let op = SignedOp::create(&c, GroupId([1; 16]), "g", pack(), &others).unwrap();
        assert_eq!(GroupState::from_create(&op).unwrap().members.len(), 2);
    }

    #[test]
    fn replay_rejects_forged_create() {
        let (mut ops, _) = chain_start(&[1, 2]);
        // Signature d'une autre clé que celle déclarée.
        let other = key(9);
        ops[0].sigs[0].1 = ops[0].sign_with(&other);
        assert_eq!(replay(&ops).unwrap_err(), ModelError::BadSignature);
        // Nom modifié après signature.
        let (mut ops, _) = chain_start(&[1, 2]);
        if let GroupOp::Create { name, .. } = &mut ops[0].op {
            *name = "Autre".into();
        }
        assert_eq!(replay(&ops).unwrap_err(), ModelError::BadSignature);
        assert_eq!(replay(&[]).unwrap_err(), ModelError::EmptyChain);
    }

    impl SignedOp {
        fn sign_with(&self, signer: &dyn HearthSigner) -> Sig {
            sign_domain(signer, OP_DOMAIN, &canonical_body(self.op_seq, &self.prev, &self.op))
        }
    }

    #[test]
    fn creator_governance_requires_creator() {
        let (mut ops, mut st) = chain_start(&[1, 2, 3]);
        // Un simple membre ne peut pas ajouter quelqu'un.
        let err = st.sign_single(GroupOp::AddMember { key: pk(4) }, &key(2)).unwrap_err();
        assert!(matches!(err, ModelError::NotAuthorized { .. }));
        let add = st.sign_single(GroupOp::AddMember { key: pk(4) }, &key(1)).unwrap();
        push(&mut ops, &mut st, add);
        assert!(st.is_member(&pk(4)));
        assert_eq!(st.op_seq, 2);
        assert_eq!(replay(&ops).unwrap(), st);
        // Doublon, retrait du créateur.
        assert_eq!(st.validate_op(&GroupOp::AddMember { key: pk(4) }), Err(ModelError::AlreadyMember));
        assert_eq!(
            st.validate_op(&GroupOp::RemoveMember { key: pk(1) }),
            Err(ModelError::CannotRemoveCreator)
        );
        assert_eq!(st.validate_op(&GroupOp::RemoveMember { key: pk(9) }), Err(ModelError::NotMember));
        let rm = st.sign_single(GroupOp::RemoveMember { key: pk(3) }, &key(1)).unwrap();
        push(&mut ops, &mut st, rm);
        assert!(!st.is_member(&pk(3)));
        assert_eq!(replay(&ops).unwrap(), st);
    }

    #[test]
    fn majority_threshold_is_strict() {
        let (mut ops, mut st) = chain_start(&[1, 2, 3, 4]);
        // Le créateur part : gouvernance à la majorité.
        let leave = st.sign_single(GroupOp::Leave, &key(1)).unwrap();
        push(&mut ops, &mut st, leave);
        assert_eq!(st.governance, Governance::Majority);
        assert_eq!(st.members.len(), 3);
        let op = GroupOp::AddMember { key: pk(5) };
        assert_eq!(st.votes_needed(&op), 2);
        let mut p = Proposal::new(&st, op).unwrap();
        assert!(p.vote(&st, &key(2)).unwrap());
        assert!(!p.is_ready(&st));
        assert_eq!(p.approvals(&st), (1, 2));
        assert!(!p.vote(&st, &key(2)).unwrap(), "vote en double ignoré");
        assert_eq!(p.sigs.len(), 1);
        assert!(matches!(p.clone().into_signed(&st), Err(ModelError::NotAuthorized { have: 1, need: 2 })));
        p.vote(&st, &key(3)).unwrap();
        assert!(p.is_ready(&st));
        let signed = p.into_signed(&st).unwrap();
        push(&mut ops, &mut st, signed);
        assert!(st.is_member(&pk(5)));
        assert_eq!(replay(&ops).unwrap(), st);
        // 4 membres : il faut 3 voix.
        assert_eq!(st.votes_needed(&GroupOp::PackUpdate { pack: pack() }), 3);
    }

    #[test]
    fn proposal_rejects_outsiders_and_forged_votes() {
        let (_, st) = chain_start(&[1, 2, 3]);
        let mut p = Proposal::new(&st, GroupOp::AddMember { key: pk(4) }).unwrap();
        assert_eq!(p.vote(&st, &key(9)).unwrap_err(), ModelError::NotMember);
        let good = p.sign(&key(2));
        assert_eq!(p.add_vote(&st, pk(3), good).unwrap_err(), ModelError::BadSignature);
        let mut flipped = good;
        flipped.0[0] ^= 1;
        assert_eq!(p.add_vote(&st, pk(2), flipped).unwrap_err(), ModelError::BadSignature);
        assert_eq!(p.sigs.len(), 0);
        assert!(p.add_vote(&st, pk(2), good).unwrap());
        p.validate(&st).unwrap();
        // Vote sur un invalide (clé qui n'est pas une clé ed25519 valide).
        assert_eq!(p.add_vote(&st, [0xff; 32], good).unwrap_err(), ModelError::NotMember);
    }

    #[test]
    fn proposal_becomes_stale_when_chain_advances() {
        let (_, mut st) = chain_start(&[1, 2, 3]);
        let mut p = Proposal::new(&st, GroupOp::AddMember { key: pk(4) }).unwrap();
        p.vote(&st, &key(1)).unwrap();
        let other = st.sign_single(GroupOp::AddMember { key: pk(5) }, &key(1)).unwrap();
        st.apply(&other).unwrap();
        assert!(!p.is_ready(&st));
        assert_eq!(p.validate(&st).unwrap_err(), ModelError::StaleProposal);
        assert_eq!(p.vote(&st, &key(2)).unwrap_err(), ModelError::StaleProposal);
    }

    #[test]
    fn proposal_validate_rejects_forged_signature_list() {
        let (_, st) = chain_start(&[1, 2, 3]);
        let mut p = Proposal::new(&st, GroupOp::AddMember { key: pk(4) }).unwrap();
        p.vote(&st, &key(1)).unwrap();
        let mut forged = p.clone();
        forged.sigs.push((pk(2), Sig([1; 64])));
        assert_eq!(forged.validate(&st).unwrap_err(), ModelError::BadSignature);
        let mut dup = p.clone();
        dup.sigs.push(dup.sigs[0]);
        assert_eq!(dup.validate(&st).unwrap_err(), ModelError::DuplicateSigner);
        let mut stranger = p.clone();
        let s = stranger.sign(&key(9));
        stranger.sigs.push((pk(9), s));
        assert_eq!(stranger.validate(&st).unwrap_err(), ModelError::NotMember);
        p.validate(&st).unwrap();
    }

    #[test]
    fn continue_without_bumps_epoch_and_keeps_creator_governance() {
        let (mut ops, mut st) = chain_start(&[1, 2, 3, 4]);
        let op = cw(pk(1), vid(5));
        assert_eq!(st.votes_needed(&op), 2, "majorité stricte de 3 membres hors absent");
        let mut p = Proposal::new(&st, op).unwrap();
        // L'absent ne peut pas voter.
        assert_eq!(p.vote(&st, &key(1)).unwrap_err(), ModelError::AbsentCannotVote);
        p.vote(&st, &key(2)).unwrap();
        assert!(!p.is_ready(&st));
        p.vote(&st, &key(3)).unwrap();
        assert!(p.is_ready(&st));
        let signed = p.into_signed(&st).unwrap();
        push(&mut ops, &mut st, signed);
        assert_eq!(st.epoch, 1);
        assert_eq!(st.base_version, Some(vid(5)));
        assert_eq!(st.base_for_epoch(1), Some(vid(5)));
        assert_eq!(st.base_for_epoch(0), None);
        assert_eq!(st.base_for_epoch(2), None);
        assert_eq!(st.governance, Governance::Creator, "la gouvernance ne bascule pas avec l'époque");
        assert!(st.is_member(&pk(1)), "l'absent reste membre");
        assert_eq!(st.continuations.len(), 1);
        assert_eq!(st.continuations[0].at, T0 + MIN_ABSENCE_SECS);
        assert_eq!(replay(&ops).unwrap(), st);
    }

    #[test]
    fn deux_membres_ne_prennent_pas_le_pouvoir_en_ecartant_le_createur() {
        // C (créateur) est écarté par A et B : plus d'époque pour lui, mais aucun pouvoir de plus pour eux.
        let (mut ops, mut st) = chain_start(&[1, 2, 3]);
        run(&mut ops, &mut st, cw(pk(1), vid(5)), &[2, 3]).unwrap();
        assert_eq!((st.epoch, st.governance), (1, Governance::Creator));
        // Retirer le créateur, inviter, changer le modpack : toujours réservé au créateur.
        assert_eq!(run(&mut ops, &mut st, GroupOp::RemoveMember { key: pk(1) }, &[2, 3]).unwrap_err(), ModelError::CannotRemoveCreator);
        assert!(matches!(run(&mut ops, &mut st, GroupOp::AddMember { key: pk(9) }, &[2, 3]), Err(ModelError::NotAuthorized { .. })));
        // Passer à la majorité : refusé avant 30 jours après la décision, accepté ensuite.
        let early = GroupOp::SetGovernance { at: T0 + MIN_ABSENCE_SECS + DORMANCY_SECS - 1 };
        assert_eq!(run(&mut ops, &mut st, early, &[2, 3]).unwrap_err(), ModelError::CreatorNotDormant);
        let late = GroupOp::SetGovernance { at: T0 + MIN_ABSENCE_SECS + DORMANCY_SECS };
        // Une seule voix sur deux autres membres ne suffit pas.
        assert!(matches!(run(&mut ops, &mut st, late.clone(), &[2]), Err(ModelError::NotAuthorized { .. })));
        run(&mut ops, &mut st, late, &[2, 3]).unwrap();
        assert_eq!(st.governance, Governance::Majority);
        run(&mut ops, &mut st, GroupOp::RemoveMember { key: pk(1) }, &[2, 3]).unwrap();
        assert!(!st.is_member(&pk(1)));
        assert_eq!(replay(&ops).unwrap(), st);
        // Déjà à la majorité : plus de bascule.
        assert_eq!(st.validate_op(&GroupOp::SetGovernance { at: T0 }).unwrap_err(), ModelError::AlreadyMajority);
    }

    #[test]
    fn le_createur_cede_seul_et_une_absence_trop_courte_est_refusee() {
        let (mut ops, mut st) = chain_start(&[1, 2, 3]);
        // Moins de 7 jours sans signe de vie : refusé dès la validation, signé ou non.
        let short = GroupOp::ContinueWithout { absent: pk(3), base_version: vid(1), absent_since: T0, at: T0 + MIN_ABSENCE_SECS - 1 };
        assert_eq!(st.validate_op(&short).unwrap_err(), ModelError::AbsenceTooShort);
        assert_eq!(Proposal::new(&st, short).unwrap_err(), ModelError::AbsenceTooShort);
        // Un horodatage qui déborde ne contourne pas la règle.
        let overflow = GroupOp::ContinueWithout { absent: pk(3), base_version: vid(1), absent_since: u64::MAX, at: 5 };
        assert_eq!(st.validate_op(&overflow).unwrap_err(), ModelError::AbsenceTooShort);
        // Le créateur passe en majorité de lui-même, à tout moment, mais seul.
        let mut p = Proposal::new(&st, GroupOp::SetGovernance { at: 0 }).unwrap();
        p.vote(&st, &key(1)).unwrap();
        p.vote(&st, &key(2)).unwrap();
        assert!(matches!(p.clone().into_signed(&st), Err(ModelError::NotAuthorized { .. })));
        run(&mut ops, &mut st, GroupOp::SetGovernance { at: 0 }, &[1]).unwrap();
        assert_eq!(st.governance, Governance::Majority);
    }

    #[test]
    fn continue_without_noncreator_keeps_governance_and_creator_may_vote() {
        let (_, st) = chain_start(&[1, 2, 3]);
        let op = cw(pk(3), vid(1));
        let mut p = Proposal::new(&st, op).unwrap();
        p.vote(&st, &key(1)).unwrap(); // 2 membres hors absent : il faut 2 voix
        assert!(!p.is_ready(&st));
        p.vote(&st, &key(2)).unwrap();
        let signed = p.into_signed(&st).unwrap();
        let mut st2 = st.clone();
        st2.apply(&signed).unwrap();
        assert_eq!(st2.governance, Governance::Creator);
        assert_eq!(st2.epoch, 1);
        // Absent inconnu ou groupe d'un seul membre.
        assert_eq!(
            st.validate_op(&cw(pk(9), vid(1))),
            Err(ModelError::NotMember)
        );
        let solo = group(&[1]);
        assert_eq!(
            solo.validate_op(&cw(pk(1), vid(1))),
            Err(ModelError::NeedsTwoMembers)
        );
    }

    #[test]
    fn leave_rules() {
        let (_, st) = chain_start(&[1, 2]);
        // Un tiers ne peut pas faire partir quelqu'un d'autre : le signataire est celui qui part.
        let signed = st.sign_single(GroupOp::Leave, &key(2)).unwrap();
        let mut st2 = st.clone();
        st2.apply(&signed).unwrap();
        assert_eq!(st2.members.len(), 1);
        assert!(!st2.is_member(&pk(2)));
        // Le dernier ne peut pas partir.
        assert_eq!(st2.validate_op(&GroupOp::Leave), Err(ModelError::LastMember));
        // Deux signataires sur un Leave : refusé.
        let mut p = Proposal::new(&st, GroupOp::Leave).unwrap();
        p.vote(&st, &key(1)).unwrap();
        p.vote(&st, &key(2)).unwrap();
        assert!(matches!(p.into_signed(&st), Err(ModelError::NotAuthorized { .. })));
        // Un non-membre ne peut pas signer un Leave.
        assert_eq!(st.sign_single(GroupOp::Leave, &key(9)).unwrap_err(), ModelError::NotMember);
    }

    #[test]
    fn pack_update_requires_change_and_valid_pack() {
        let (_, st) = chain_start(&[1, 2]);
        assert_eq!(
            st.validate_op(&GroupOp::PackUpdate { pack: pack() }),
            Err(ModelError::PackUnchanged)
        );
        let mut np = pack();
        np.manifest_hash = "cd".repeat(32);
        let signed = st.sign_single(GroupOp::PackUpdate { pack: np.clone() }, &key(1)).unwrap();
        let mut st2 = st.clone();
        st2.apply(&signed).unwrap();
        assert_eq!(st2.pack, np);
        np.minecraft = String::new();
        assert!(matches!(
            st.validate_op(&GroupOp::PackUpdate { pack: np }),
            Err(ModelError::InvalidPack(_))
        ));
    }

    #[test]
    fn member_cap_enforced_on_add() {
        let seeds: Vec<u8> = (1..=64).collect();
        let st = group(&seeds);
        assert_eq!(st.members.len(), 64);
        assert_eq!(st.validate_op(&GroupOp::AddMember { key: pk(200) }), Err(ModelError::TooManyMembers));
    }

    #[test]
    fn chain_integrity() {
        let (mut ops, mut st) = chain_start(&[1, 2, 3]);
        let a = st.sign_single(GroupOp::AddMember { key: pk(4) }, &key(1)).unwrap();
        push(&mut ops, &mut st, a);
        let b = st.sign_single(GroupOp::AddMember { key: pk(5) }, &key(1)).unwrap();
        push(&mut ops, &mut st, b);
        // Réordonnée, tronquée au milieu, rejouée.
        let swapped = vec![ops[0].clone(), ops[2].clone(), ops[1].clone()];
        assert!(matches!(replay(&swapped), Err(ModelError::BadSeq { .. })));
        let gap = vec![ops[0].clone(), ops[2].clone()];
        assert!(matches!(replay(&gap), Err(ModelError::BadSeq { expected: 2, got: 3 })));
        let dup = vec![ops[0].clone(), ops[1].clone(), ops[1].clone()];
        assert!(matches!(replay(&dup), Err(ModelError::BadSeq { .. })));
        // Une opération ré-étiquetée avec un autre `prev`.
        let mut bad = ops.clone();
        bad[2].prev = [9; 32];
        assert_eq!(replay(&bad).unwrap_err(), ModelError::BrokenChain);
        // Deuxième `Create`.
        let mut again = ops.clone();
        again[1] = ops[0].clone();
        assert!(replay(&again).is_err());
        // Une chaîne signée valide d'un autre groupe ne se greffe pas.
        let (other_ops, _) = chain_start(&[1, 2, 3]);
        let mut grafted = other_ops;
        grafted.push(ops[1].clone());
        // Même créateur, mêmes paramètres, donc même Create : la greffe est (légitimement) valide ici.
        assert!(replay(&grafted).is_ok());
        let creator = key(1);
        let mut others = BTreeSet::new();
        others.insert(pk(2));
        let c2 = SignedOp::create(&creator, GroupId([8; 16]), "Autre", pack(), &others).unwrap();
        assert_eq!(replay(&[c2, ops[1].clone()]).unwrap_err(), ModelError::BrokenChain);
    }

    #[test]
    fn replay_bounds_chain_length() {
        let (ops, _) = chain_start(&[1]);
        let long = vec![ops[0].clone(); MAX_OPS + 1];
        assert_eq!(replay(&long).unwrap_err(), ModelError::TooManyOps);
    }

    #[test]
    fn canonical_encoding_is_stable() {
        let (ops, _) = chain_start(&[1, 2]);
        let body = canonical_body(ops[0].op_seq, &ops[0].prev, &ops[0].op);
        let text = String::from_utf8(body.clone()).unwrap();
        assert!(text.starts_with(r#"{"op_seq":1,"prev":"0000"#), "{text}");
        assert!(text.contains(r#""op":{"type":"create","id":"07070707070707070707070707070707","name":"Les copains""#));
        assert!(text.contains(r#""loader":{"loader":"neoforge","version":"21.1.77"}"#));
        assert!(!text.contains(' ') || text.contains("Les copains"));
        // Le JSON reçu puis ré-encodé donne le même corps : l'encodage ne dépend pas des octets reçus.
        let json = serde_json::to_string(&ops[0]).unwrap();
        let back: SignedOp = serde_json::from_str(&json).unwrap();
        assert_eq!(back, ops[0]);
        assert_eq!(canonical_body(back.op_seq, &back.prev, &back.op), body);
        assert_eq!(back.hash(), ops[0].hash());
        // Hash = BLAKE3 du corps ; la signature n'y entre pas.
        assert_eq!(ops[0].hash(), *blake3::hash(&body).as_bytes());
        let mut other_sig = ops[0].clone();
        other_sig.sigs[0].1 = Sig([0; 64]);
        assert_eq!(other_sig.hash(), ops[0].hash());
    }

    #[test]
    fn domain_separation_blocks_cross_protocol_signatures() {
        let (ops, _) = chain_start(&[1, 2]);
        let body = canonical_body(1, &GENESIS_PREV, &ops[0].op);
        let wrong = sign_domain(&key(1), b"cubelink/hearth/version/1", &body);
        let mut forged = ops[0].clone();
        forged.sigs[0].1 = wrong;
        assert_eq!(replay(&[forged]).unwrap_err(), ModelError::BadSignature);
        // Signature brute sans domaine : refusée aussi.
        let raw = key(1).sign_raw(&body);
        let mut forged = ops[0].clone();
        forged.sigs[0].1 = raw;
        assert_eq!(replay(&[forged]).unwrap_err(), ModelError::BadSignature);
    }

    #[test]
    fn json_rejects_malformed_input() {
        let (ops, _) = chain_start(&[1, 2]);
        let good = serde_json::to_value(&ops[0]).unwrap();
        // Champ inconnu, clé trop courte, signature trop courte, doublon de membre.
        let mut v = good.clone();
        v["extra"] = serde_json::json!(1);
        assert!(serde_json::from_value::<SignedOp>(v).is_err());
        let mut v = good.clone();
        v["sigs"][0][0] = serde_json::json!("abcd");
        assert!(serde_json::from_value::<SignedOp>(v).is_err());
        let mut v = good.clone();
        v["sigs"][0][1] = serde_json::json!("abcd");
        assert!(serde_json::from_value::<SignedOp>(v).is_err());
        let mut v = good.clone();
        let k = hex::encode(pk(2));
        v["op"]["members"] = serde_json::json!([k.clone(), k]);
        assert!(serde_json::from_value::<SignedOp>(v).is_err());
        let mut v = good.clone();
        v["sigs"] = serde_json::Value::Array((0..65).map(|_| good["sigs"][0].clone()).collect());
        assert!(serde_json::from_value::<SignedOp>(v).is_err());
        let mut v = good;
        v["op"]["members"] = serde_json::Value::Array((0..65u8).map(|i| serde_json::json!(hex::encode([i; 32]))).collect());
        assert!(serde_json::from_value::<SignedOp>(v).is_err());
    }

    #[test]
    fn group_id_hex_roundtrip() {
        let id = GroupId::random();
        assert_eq!(GroupId::from_hex(&id.to_hex()), Some(id));
        assert_eq!(GroupId::from_hex("zz"), None);
        assert_eq!(GroupId::from_hex(&"ab".repeat(15)), None);
        assert_ne!(GroupId::random(), GroupId::random());
        let json = serde_json::to_string(&id).unwrap();
        assert_eq!(serde_json::from_str::<GroupId>(&json).unwrap(), id);
    }

    #[test]
    fn identity_signs_ops() {
        let dir = tempfile::tempdir().unwrap();
        let id = Identity::load_or_create(dir.path()).unwrap();
        let op = SignedOp::create(&id, GroupId([3; 16]), "Réel", pack(), &BTreeSet::new()).unwrap();
        let st = replay(&[op]).unwrap();
        assert_eq!(st.creator, id.public_key().to_bytes());
    }

    // --- propriétés ---------------------------------------------------------------------------------------

    /// Construit une chaîne valide à partir d'intentions aléatoires (les intentions impossibles sont ignorées).
    fn build_chain(intents: &[(u8, u8, u8)]) -> (Vec<SignedOp>, GroupState) {
        let (mut ops, mut st) = chain_start(&[1, 2, 3]);
        for (n, &(kind, a, b)) in intents.iter().enumerate() {
            let actor = 1 + a % 6;
            let target = 1 + b % 6;
            let op = match kind % 5 {
                0 => GroupOp::AddMember { key: pk(target) },
                1 => GroupOp::RemoveMember { key: pk(target) },
                2 => cw(pk(target), vid(n as u8)),
                3 => {
                    let mut p = pack();
                    p.manifest_hash = hex::encode([n as u8; 32]);
                    GroupOp::PackUpdate { pack: p }
                }
                _ => GroupOp::Leave,
            };
            let Ok(mut prop) = Proposal::new(&st, op.clone()) else { continue };
            let voters: Vec<PeerKey> = if matches!(op, GroupOp::Leave) {
                vec![pk(actor)]
            } else {
                // Créateur d'abord (gouvernance Creator), puis les autres membres.
                let mut v: Vec<PeerKey> = st.members.iter().copied().collect();
                v.sort_by_key(|k| *k != st.creator);
                v
            };
            for k in voters {
                let Some(seed) = (1..=6u8).find(|s| pk(*s) == k) else { continue };
                if prop.is_ready(&st) {
                    break;
                }
                let _ = prop.vote(&st, &key(seed));
            }
            if let Ok(signed) = prop.into_signed(&st) {
                st.apply(&signed).unwrap();
                ops.push(signed);
            }
        }
        (ops, st)
    }

    fn intents() -> impl Strategy<Value = Vec<(u8, u8, u8)>> {
        prop::collection::vec((any::<u8>(), any::<u8>(), any::<u8>()), 0..14)
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(48))]

        #[test]
        fn replay_matches_incremental_state(it in intents()) {
            let (ops, st) = build_chain(&it);
            prop_assert_eq!(replay(&ops).unwrap(), st);
        }

        #[test]
        fn replay_of_prefix_is_prefix_state(it in intents(), cut in 1usize..16) {
            let (ops, _) = build_chain(&it);
            let cut = cut.min(ops.len());
            let prefix = replay(&ops[..cut]).unwrap();
            prop_assert_eq!(prefix.op_seq as usize, cut);
        }

        #[test]
        fn json_roundtrip_preserves_chain(it in intents()) {
            let (ops, st) = build_chain(&it);
            let json = serde_json::to_string(&ops).unwrap();
            let back: Vec<SignedOp> = serde_json::from_str(&json).unwrap();
            prop_assert_eq!(&back, &ops);
            prop_assert_eq!(replay(&back).unwrap(), st);
        }

        #[test]
        fn forged_signature_is_refused(it in intents(), which in any::<usize>(), byte in 0usize..64, bit in 0u8..8) {
            let (mut ops, _) = build_chain(&it);
            let i = which % ops.len();
            let j = which % ops[i].sigs.len();
            ops[i].sigs[j].1 .0[byte] ^= 1 << bit;
            prop_assert!(replay(&ops).is_err());
        }

        #[test]
        fn signer_swap_is_refused(it in intents(), which in any::<usize>()) {
            let (mut ops, _) = build_chain(&it);
            let i = which % ops.len();
            // Même signature attribuée à une autre clé.
            ops[i].sigs[0].0 = pk(100);
            prop_assert!(replay(&ops).is_err());
        }

        #[test]
        fn dropping_a_signature_is_refused(it in intents(), which in any::<usize>()) {
            // Les chaînes construites n'ont que le nombre exact de signatures : en retirer une brise le seuil.
            let (mut ops, _) = build_chain(&it);
            let i = which % ops.len();
            let j = which % ops[i].sigs.len();
            ops[i].sigs.remove(j);
            prop_assert!(replay(&ops).is_err());
        }

        #[test]
        fn reordering_is_refused(it in intents(), a in any::<usize>(), b in any::<usize>()) {
            let (mut ops, _) = build_chain(&it);
            prop_assume!(ops.len() >= 2);
            let (i, j) = (a % ops.len(), b % ops.len());
            prop_assume!(i != j);
            ops.swap(i, j);
            prop_assert!(replay(&ops).is_err());
        }

        #[test]
        fn removing_a_middle_op_is_refused(it in intents(), a in any::<usize>()) {
            let (mut ops, _) = build_chain(&it);
            prop_assume!(ops.len() >= 3);
            let i = 1 + a % (ops.len() - 2);
            ops.remove(i);
            prop_assert!(replay(&ops).is_err());
        }

        #[test]
        fn members_never_exceed_cap_and_creator_rule_holds(it in intents()) {
            let (_, st) = build_chain(&it);
            prop_assert!(st.members.len() <= MAX_MEMBERS && !st.members.is_empty());
            if st.governance == Governance::Creator {
                prop_assert!(st.members.contains(&st.creator));
            }
        }
    }
}

//! Commandes `hearth_*` : serveurs de groupe (docs/hearth.md § 7 à 9, docs/contrats.md).
//!
//! Le runtime (`hearth.rs`) tient les groupes, les baux et les versions ; ici on enchaîne ce qui touche le reste de
//! l'application : instances, compte, lancement du jeu, partage du tunnel.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use core_lib::hearth::world;
use core_lib::instance::InstanceKind;
use core_lib::manifest::{build_manifest, manifest_hash, ManifestOptions};
use core_lib::types::{
    HearthCard, HearthDetail, HearthHostPrompt, HearthInvite, HearthWorld, Instance,
};
use p2p_lib::hearth::lease::Decision;
use p2p_lib::hearth::model::{GroupOp, GroupId, PackPin};
use p2p_lib::hearth::version::{VersionId, VersionKind};
use p2p_lib::presence::PeerKey;
use serde::Serialize;
use tauri::State;

use crate::error::{sanitize, user_msg};
use crate::hearth::{key_hex, parse_key, Begin, HearthRuntime, HostParams, LeaveError, Play, EULA_REQUIRED, HOST_STEPS, JOIN_STEPS, LEAVE_FLUSH_WAIT};
use crate::state::AppState;

/// Longueur maximale du nom d'un groupe (caractères).
const MAX_NAME: usize = 64;

/// Réponse de `hearth_eula`.
#[derive(Debug, Clone, Serialize, PartialEq, Eq, specta::Type)]
pub struct HearthEula {
    pub accepted: bool,
    pub url: String,
}

fn gid(id: &str) -> Result<GroupId, String> {
    HearthRuntime::parse_id(id)
}

fn key(k: &str) -> Result<PeerKey, String> {
    parse_key(k).ok_or_else(|| "clé publique invalide".to_string())
}

impl AppState {
    // --- lecture ---------------------------------------------------------------------------------------

    pub fn hearth_list(&self) -> Vec<HearthCard> {
        self.hearth.cards()
    }

    pub async fn hearth_detail(&self, id: &str) -> Result<HearthDetail, String> {
        let g = gid(id)?;
        let rt = self.hearth.clone();
        tokio::task::spawn_blocking(move || rt.detail(&g)).await.map_err(|_| "lecture du groupe interrompue".to_string())?
    }

    /// Mondes d'une instance, avec leur taille, pour la création d'un groupe.
    pub async fn hearth_worlds(&self, instance_id: &str) -> Result<Vec<HearthWorld>, String> {
        self.hearth.require_enabled()?;
        if !crate::commands_instances::valid_id(instance_id) {
            return Err("identifiant d'instance invalide".into());
        }
        self.instances.get(instance_id).map_err(|e| user_msg(&e))?;
        let dir = self.instances.game_dir(instance_id).map_err(|e| user_msg(&e))?;
        tokio::task::spawn_blocking(move || -> Result<Vec<HearthWorld>, String> {
            let names = world::list_instance_saves(&dir).map_err(|e| user_msg(&e))?;
            Ok(names
                .into_iter()
                .map(|n| HearthWorld { size_bytes: dir_bytes(&dir.join("saves").join(&n)), name: n })
                .collect())
        })
        .await
        .map_err(|_| "lecture des mondes interrompue".to_string())?
    }

    /// Interrupteur de sécurité (docs/hearth.md § 12.1 n° 63) : `false` par défaut.
    pub fn hearth_enabled(&self) -> bool {
        self.hearth.enabled()
    }

    pub fn hearth_set_enabled(&self, on: bool) {
        self.hearth.set_enabled(on);
    }

    pub fn hearth_invites(&self) -> Vec<HearthInvite> {
        self.hearth.invites()
    }

    pub fn hearth_eula(&self) -> HearthEula {
        HearthEula { accepted: self.hearth.eula_accepted(), url: "https://aka.ms/MinecraftEULA".into() }
    }

    // --- modpack du groupe ------------------------------------------------------------------------------

    /// Empreinte du modpack d'une instance : mods et versions, sans le nom de l'instance.
    async fn pack_pin_of(&self, instance_id: &str) -> Result<PackPin, String> {
        let inst = self.instances.get(instance_id).map_err(|e| user_msg(&e))?;
        let dir = self.instances.game_dir(instance_id).map_err(|e| user_msg(&e))?;
        let i2 = inst.clone();
        let m = tokio::task::spawn_blocking(move || build_manifest(&i2, &dir, &ManifestOptions::default()))
            .await
            .map_err(|_| "calcul de l'empreinte du modpack interrompu".to_string())?
            .map_err(|e| user_msg(&e))?;
        let mut m = m;
        m.name = String::new();
        Ok(PackPin { manifest_hash: manifest_hash(&m), minecraft: inst.minecraft, loader: inst.loader })
    }

    /// L'instance locale correspond-elle au modpack du groupe ? Les mods sont figés : sinon le jeu ne se connecterait pas.
    async fn hearth_check_pack(&self, g: &GroupId, instance_id: &str) -> Result<(), String> {
        let st = self.hearth.require_state(g)?;
        let pin = self.pack_pin_of(instance_id).await?;
        if pin == st.pack {
            Ok(())
        } else {
            Err("Le modpack de ton instance n'est plus celui du groupe (les mods sont figés). Remets les mêmes mods, ou propose la mise à jour du modpack au groupe depuis la page du serveur.".into())
        }
    }

    // --- groupes ------------------------------------------------------------------------------------------

    /// Crée un groupe depuis une instance : `world` est une sauvegarde de l'instance (copiée chez le groupe), ou `None`
    /// pour un nouveau monde (créé au premier lancement). `members` : clés d'amis acceptés, invités ensuite.
    pub async fn hearth_create(
        self: &Arc<Self>,
        name: &str,
        instance_id: &str,
        world_name: Option<String>,
        members: Vec<String>,
    ) -> Result<HearthCard, String> {
        self.hearth.require_enabled()?;
        let name = name.trim();
        if name.is_empty() || name.chars().count() > MAX_NAME || name.chars().any(char::is_control) {
            return Err("le nom du serveur doit faire entre 1 et 64 caractères".into());
        }
        if !crate::commands_instances::valid_id(instance_id) {
            return Err("identifiant d'instance invalide".into());
        }
        self.instances.get(instance_id).map_err(|e| user_msg(&e))?;
        if self.instances.kind(instance_id).map_err(|e| user_msg(&e))? != InstanceKind::Own {
            return Err("seule une de tes instances peut devenir un serveur de groupe".into());
        }
        let mut keys: Vec<PeerKey> = Vec::new();
        for m in &members {
            let k = key(m)?;
            if !self.friends.is_friend(m).map_err(|e| user_msg(&e))? {
                return Err("tu ne peux inviter que des amis acceptés".into());
            }
            if k != self.hearth.me() && !keys.contains(&k) {
                keys.push(k);
            }
        }
        if keys.is_empty() {
            return Err("choisis au moins un ami à inviter".into());
        }
        let game_dir = self.instances.game_dir(instance_id).map_err(|e| user_msg(&e))?;
        let pin = self.pack_pin_of(instance_id).await?;
        let rt = self.hearth.clone();
        // Le magasin de blobs doit être prêt avant de créer quoi que ce soit.
        rt.world()?;
        let g = rt.create_group(name, pin, &keys, instance_id)?;
        let built = async {
            if let Some(save) = world_name {
                let dest = rt.server_dir(&g).join("world");
                let (src_dir, save2, dest2) = (game_dir.clone(), save, dest.clone());
                tokio::task::spawn_blocking(move || {
                    std::fs::create_dir_all(dest2.parent().unwrap_or(&dest2)).map_err(|_| "impossible de créer le dossier du groupe".to_string())?;
                    world::copy_instance_save(&src_dir, &save2, &dest2).map_err(|e| user_msg(&e)).map(|_| ())
                })
                .await
                .map_err(|_| "copie du monde interrompue".to_string())??;
                let v = rt.publish_version(g, &dest, VersionKind::Session, None, 0).await?;
                rt.set_materialized_version(&g, v.id());
            }
            Ok::<(), String>(())
        }
        .await;
        if let Err(e) = built {
            let _ = rt.forget_group(&g);
            return Err(e);
        }
        let rt2 = rt.clone();
        crate::hearth::spawn(async move {
            rt2.send_pending_invites(g).await;
            rt2.probe_group(g).await;
        });
        rt.card(&g).ok_or_else(|| "le groupe n'a pas pu être créé".to_string())
    }

    pub fn hearth_invite_accept(&self, id: &str) -> Result<(), String> {
        self.hearth.require_enabled()?;
        self.hearth.accept_invite(&gid(id)?)
    }

    pub fn hearth_invite_dismiss(&self, id: &str) -> Result<(), String> {
        self.hearth.require_enabled()?;
        self.hearth.dismiss_invite(&gid(id)?);
        Ok(())
    }

    /// Relie le groupe à une instance locale, si son modpack est celui du groupe (après l'avoir installé).
    pub async fn hearth_set_instance(&self, id: &str, instance_id: &str) -> Result<(), String> {
        self.hearth.require_enabled()?;
        let g = gid(id)?;
        if !crate::commands_instances::valid_id(instance_id) {
            return Err("identifiant d'instance invalide".into());
        }
        self.hearth_check_pack(&g, instance_id).await?;
        self.hearth.set_instance(&g, Some(instance_id.to_string()));
        Ok(())
    }

    pub async fn hearth_add_member(&self, id: &str, public_key: &str) -> Result<(), String> {
        self.hearth.require_enabled()?;
        let g = gid(id)?;
        let k = key(public_key)?;
        if !self.friends.is_friend(public_key).map_err(|e| user_msg(&e))? {
            return Err("tu ne peux ajouter que des amis acceptés".into());
        }
        self.hearth.propose(g, GroupOp::AddMember { key: k }).await
    }

    pub async fn hearth_remove_member(&self, id: &str, public_key: &str) -> Result<(), String> {
        self.hearth.require_enabled()?;
        let (g, k) = (gid(id)?, key(public_key)?);
        if k == self.hearth.me() {
            return Err("pour partir, utilise « Quitter le groupe »".into());
        }
        self.hearth.propose(g, GroupOp::RemoveMember { key: k }).await
    }

    /// `force` saute l'attente de la file d'envoi : ma dernière partie, si elle n'est pas encore partie, est perdue pour le groupe.
    pub async fn hearth_leave(self: &Arc<Self>, id: &str, force: bool) -> Result<(), LeaveError> {
        self.hearth.require_enabled()?;
        let g = gid(id)?;
        // Dernier membre : refus AVANT d'arrêter mon serveur, il ne doit rien arriver au monde.
        if self.hearth.state(&g).is_some_and(|s| s.members.len() <= 1) {
            return Err(LeaveError::LastMember);
        }
        if self.hearth.active(&g).is_some() {
            self.hearth_stop(id).await?;
        }
        // #56 : `forget_group` jette la file d'envoi du groupe. Ma dernière partie (publiée par l'arrêt ci-dessus) doit
        // d'abord être remise à un membre ; sinon rien n'est supprimé et l'écran propose « Quitter quand même ».
        if !force && !self.hearth.flush_group_outbox(&g, LEAVE_FLUSH_WAIT).await {
            return Err(LeaveError::OutboxPending);
        }
        self.hearth.propose_leave(g).await?;
        // Le groupe a été quitté si la chaîne l'a appliqué ; `forget_group` est aussi appelé par le runtime.
        if self.hearth.state(&g).is_some() {
            self.hearth.forget_group_wait(&g).await?;
        }
        Ok(())
    }

    /// Supprime le serveur de CE PC seulement : arrête mon serveur s'il tourne, ferme l'accès, libère les blobs et efface le
    /// dossier, sans rien annoncer au groupe. Sert quand « Quitter » ne peut pas aboutir (aucun membre joignable) ou que je veux
    /// seulement faire le ménage ici. Les autres membres me comptent encore dans le groupe : je peux être réinvité.
    pub async fn hearth_forget(self: &Arc<Self>, id: &str) -> Result<(), String> {
        self.hearth.require_enabled()?;
        let g = gid(id)?;
        self.hearth.require_state(&g)?;
        if self.hearth.active(&g).is_some() {
            self.hearth_stop(id).await?;
        }
        self.hearth.forget_group_wait(&g).await
    }

    pub async fn hearth_vote(&self, id: &str, proposal_id: &str) -> Result<(), String> {
        self.hearth.require_enabled()?;
        let g = gid(id)?;
        let p: [u8; 32] = hex::decode(proposal_id).ok().and_then(|b| b.try_into().ok()).ok_or("identifiant de proposition invalide")?;
        self.hearth.vote(g, p).await
    }

    pub async fn hearth_continue_without(&self, id: &str, public_key: &str) -> Result<(), String> {
        self.hearth.require_enabled()?;
        self.hearth.propose_continue(gid(id)?, key(public_key)?).await
    }

    pub async fn hearth_pack_update(&self, id: &str, instance_id: &str) -> Result<(), String> {
        self.hearth.require_enabled()?;
        let g = gid(id)?;
        if !crate::commands_instances::valid_id(instance_id) {
            return Err("identifiant d'instance invalide".into());
        }
        self.instances.get(instance_id).map_err(|e| user_msg(&e))?;
        let pin = self.pack_pin_of(instance_id).await?;
        if pin == self.hearth.require_state(&g)?.pack {
            return Err("le modpack de cette instance est déjà celui du groupe".into());
        }
        // L'instance liée ne change qu'une fois `PackUpdate` appliqué (voir `HearthRuntime::finalize`).
        self.hearth.propose_pack_update(g, pin, instance_id).await
    }

    pub async fn hearth_restore(&self, id: &str, version_id: &str) -> Result<(), String> {
        self.hearth.require_enabled()?;
        let v = VersionId::from_hex(version_id).ok_or("identifiant de version invalide")?;
        self.hearth.restore(gid(id)?, v).await.map(|_| ())
    }

    pub async fn hearth_resolve_fork(&self, id: &str, version_id: &str) -> Result<(), String> {
        self.hearth.require_enabled()?;
        let v = VersionId::from_hex(version_id).ok_or("identifiant de version invalide")?;
        self.hearth.resolve_fork(gid(id)?, v).await
    }

    pub fn hearth_set_background(&self, id: &str, on: bool) -> Result<(), String> {
        self.hearth.require_enabled()?;
        let g = gid(id)?;
        self.hearth.require_state(&g)?;
        self.hearth.set_keep_background(&g, on);
        Ok(())
    }

    pub fn hearth_eula_accept(&self) {
        self.hearth.accept_eula();
    }

    // --- Jouer -----------------------------------------------------------------------------------------------

    /// Le bouton Jouer (docs/hearth.md § 3.3 et § 7) : rejoint la partie en cours, ou démarre le serveur sur ce PC.
    pub async fn hearth_play(self: &Arc<Self>, id: &str) -> Result<(), String> {
        self.hearth.require_enabled()?;
        let g = gid(id)?;
        let rt = self.hearth.clone();
        rt.require_state(&g)?;
        if self.auth_status_refresh().await.is_none() {
            return Err("connectez-vous avant de lancer le jeu".into());
        }
        let instance = rt.instance_of(&g).ok_or("Installe d'abord le modpack de ce groupe : rejoins la partie d'un ami du groupe pour le récupérer.")?;
        // Mon propre serveur tourne encore en arrière-plan : on y rouvre simplement mon jeu.
        if let Some(a) = rt.active(&g) {
            self.close_same_instance_game(&instance).await?;
            return self.hearth_launch_client(g, &instance, a.port(), true).await;
        }
        if !rt.try_begin_play(&g, HOST_STEPS) {
            return Err("un lancement de ce serveur est déjà en cours".into());
        }
        let r = self.hearth_play_inner(g, &instance).await;
        if r.is_err() {
            rt.set_play(&g, None);
        }
        r
    }

    /// « Jouer depuis la version précédente » : la dernière version est connue mais personne de joignable n'en a le
    /// contenu (`head_unavailable`). Le serveur démarre depuis la dernière version complète ici ; quand l'auteur revient avec
    /// la sienne, les deux branches forment une fourche que le groupe résout depuis la page du serveur (docs/hearth-et-si.md).
    pub async fn hearth_play_from_previous(self: &Arc<Self>, id: &str) -> Result<(), String> {
        self.hearth.require_enabled()?;
        let g = gid(id)?;
        let rt = self.hearth.clone();
        rt.require_state(&g)?;
        if self.auth_status_refresh().await.is_none() {
            return Err("connectez-vous avant de lancer le jeu".into());
        }
        let instance = rt.instance_of(&g).ok_or("Installe d'abord le modpack de ce groupe : rejoins la partie d'un ami du groupe pour le récupérer.")?;
        if rt.active(&g).is_some() {
            return Err("le serveur de ce groupe tourne déjà".into());
        }
        if rt.is_fork(&g) {
            return Err("Deux versions du monde existent : choisis laquelle garder depuis la page du serveur.".into());
        }
        let head = rt.head(&g).ok_or("aucune version connue pour repartir")?;
        // Le contenu de la tête est revenu entre-temps (l'auteur, ou un membre qui l'a reçu) : un simple « Jouer » suffit.
        if rt.world_complete(&head) || rt.online_member_has_world(&g, &head).await {
            return Err("Le monde de la dernière version est de nouveau disponible : clique sur Jouer.".into());
        }
        let prev = rt.complete_head(&g).ok_or("aucune version complète sur ce PC pour repartir : il faut attendre l'auteur")?;
        if !rt.try_begin_play(&g, HOST_STEPS) {
            return Err("un lancement de ce serveur est déjà en cours".into());
        }
        let r = self.hearth_host(g, &instance, Some(prev.id())).await;
        if r.is_err() {
            rt.set_play(&g, None);
        }
        r
    }

    /// Ferme le jeu en cours s'il s'agit de la même instance (par exemple après une passation, le client est resté au menu).
    async fn close_same_instance_game(self: &Arc<Self>, instance: &str) -> Result<(), String> {
        match self.game_current() {
            Some(cur) if cur == instance => {
                let title = self.instances.get(instance).map(|i| i.name).unwrap_or_default();
                self.close_game_for_switch(&title).await
            }
            Some(_) => Err("un autre jeu est déjà ouvert : ferme-le d'abord".into()),
            None => Ok(()),
        }
    }

    async fn hearth_play_inner(self: &Arc<Self>, g: GroupId, instance: &str) -> Result<(), String> {
        let rt = self.hearth.clone();
        for _ in 0..3 {
            // Ronde `Hello` parallèle à tous les membres (4 s chacun) : on décide sur leurs réponses et la liste des
            // injoignables, pas sur le cache.
            let round = rt.probe_round(g).await;
            if rt.is_fork(&g) {
                return Err("Deux versions du monde existent : choisis laquelle garder depuis la page du serveur.".into());
            }
            match rt.decision_for(&g, &round).ok_or("ce groupe n'existe plus")? {
                // Personne d'autre n'est joignable : on héberge quand même (docs/hearth.md § 1). Les amis rejoignent mon nœud à leur
                // arrivée ; le monde leur parvient par la file d'envoi. Le bail « au mieux » demande une seconde confirmation
                // (`best_effort_pending`) : le clic sur Jouer vaut pour les deux, on ne fait pas cliquer deux fois.
                Decision::Blocked => {
                    let r = self.hearth_host(g, instance, None).await;
                    if r.is_err() && rt.best_effort_pending(&g) {
                        return self.hearth_host(g, instance, None).await;
                    }
                    return r;
                }
                Decision::Unclean { holder, .. } => {
                    let name = rt.name_of(&holder);
                    return Err(format!("La partie de {name} ne s'est pas terminée proprement : choisis comment reprendre depuis la carte du serveur."));
                }
                Decision::Sync(peer) => {
                    rt.pull_from(g, peer).await?;
                    // On récupère ce que ce membre annonce en entier : sa dernière tête n'a pas forcément son contenu
                    // (un en-tête circule avant son monde), et la demander lui échouerait.
                    let versions: Vec<_> = rt.announced_by(&g, &peer).iter().filter_map(|id| rt.version(&g, id)).collect();
                    let versions = if versions.is_empty() { rt.heads_of(&g).unwrap_or_default() } else { versions };
                    for v in versions {
                        rt.fetch_version_user(g, &v, Some(peer)).await?;
                    }
                }
                Decision::Join(holder) => return self.hearth_join(g, instance, holder).await,
                Decision::Host => return self.hearth_host(g, instance, None).await,
            }
        }
        Err("le groupe n'a pas fini de se synchroniser : réessaie dans un instant".into())
    }

    /// Rejoint la partie de `holder` : tunnel vers son serveur, puis le jeu en `--quickPlayMultiplayer`.
    async fn hearth_join(self: &Arc<Self>, g: GroupId, instance: &str, holder: PeerKey) -> Result<(), String> {
        let rt = self.hearth.clone();
        rt.set_play(&g, Some(Play::Launching { steps: JOIN_STEPS, index: 0 }));
        self.hearth_check_pack(&g, instance).await?;
        self.close_same_instance_game(instance).await?;
        // Le titulaire prépare encore son serveur (installation, démarrage d'un gros pack) : on attend qu'il soit prêt.
        rt.wait_ready(&g, &holder).await?;
        let port = match self.backend().hearth_open_tunnel(key_hex(&holder)).await {
            Ok(port) => {
                rt.service_ok(&g, &holder);
                port
            }
            Err(e) => {
                // Preuve de service : un bail annoncé sans serveur derrière ne bloque pas le groupe plus de quelques minutes.
                rt.service_failed(&g, &holder);
                return Err(sanitize(&e));
            }
        };
        rt.set_play(&g, Some(Play::Launching { steps: JOIN_STEPS, index: 1 }));
        if let Err(e) = rt.launcher().launch(self, instance, port).await {
            self.backend().join_close_tunnel().await;
            return Err(e);
        }
        rt.set_play(&g, Some(Play::Joined { holder }));
        self.hearth_watch(g, instance.to_string(), false);
        Ok(())
    }

    /// Démarre le serveur sur ce PC et y connecte le jeu. `base` : version de départ imposée (reprise après une partie
    /// mal terminée), sinon la tête.
    async fn hearth_host(self: &Arc<Self>, g: GroupId, instance: &str, base: Option<VersionId>) -> Result<(), String> {
        let rt = self.hearth.clone();
        if !rt.eula_accepted() {
            return Err(EULA_REQUIRED.into());
        }
        let inst: Instance = self.instances.get(instance).map_err(|e| user_msg(&e))?;
        let game_dir = self.instances.game_dir(instance).map_err(|e| user_msg(&e))?;
        rt.set_play(&g, Some(Play::Launching { steps: HOST_STEPS, index: 0 }));
        self.hearth_check_pack(&g, instance).await?;
        let head = match base {
            Some(id) => Some(rt.version(&g, &id).ok_or("la version de départ est introuvable")?),
            None => rt.head(&g),
        };
        // La tête est connue (en-tête) mais personne de joignable n'en a le contenu : on le dit avant de prendre le bail, au
        // lieu d'annoncer « le serveur démarre » puis d'échouer. L'écran propose d'attendre l'auteur ou de repartir de la
        // dernière version complète (`hearth_play_from_previous`).
        if base.is_none() {
            if let Some(h) = head.as_ref().filter(|h| !rt.world_complete(h)) {
                if !rt.online_member_has_world(&g, h).await {
                    return Err(rt.head_unavailable_error(&g, h));
                }
            }
        }
        let tip = head.as_ref().map(|h| (h.id(), h.header.number));
        // Premier temps du bail : les autres membres voient « le serveur démarre » dès maintenant, et non seulement à la
        // fin de l'installation (de 2 à 6 minutes pour un gros pack). Un membre déjà en train de démarrer, ou dont le
        // serveur est prêt, garde la main.
        // Le bail se prend en deux phases (claim puis annonce) : un bail seulement « au mieux » (pas de majorité) demande une
        // confirmation (un second « Jouer »), et un concurrent qui a la main se rejoint.
        match rt.begin_starting(g, tip).await? {
            Begin::Hosting => {}
            Begin::Join(holder) => return self.hearth_join(g, instance, holder).await,
        }
        let started = self.hearth_host_start(g, instance, &inst, &game_dir, head.as_ref(), tip).await;
        match started {
            Ok(Some(holder)) => self.hearth_join(g, instance, holder).await,
            Ok(None) => Ok(()),
            Err(e) => {
                // Le serveur n'a pas démarré : le bail `Starting` est relâché pour que personne n'attende un serveur qui ne vient pas.
                rt.end_starting(g).await;
                Err(e)
            }
        }
    }

    /// Prépare le monde, démarre le serveur sous le bail `Starting`, le passe en `Ready` et y ouvre le jeu. `Some(holder)` :
    /// un autre membre a gardé la main, ce serveur-ci est abandonné et il faut rejoindre `holder`.
    async fn hearth_host_start(
        self: &Arc<Self>,
        g: GroupId,
        instance: &str,
        inst: &Instance,
        game_dir: &Path,
        head: Option<&p2p_lib::hearth::version::SignedVersion>,
        tip: Option<(VersionId, u64)>,
    ) -> Result<Option<PeerKey>, String> {
        let rt = self.hearth.clone();
        rt.prepare_world(g, head).await?;
        rt.set_play(&g, Some(Play::Launching { steps: HOST_STEPS, index: 1 }));
        let settings = self.settings_get();
        let params = HostParams {
            instance_id: instance.to_string(),
            minecraft: inst.minecraft.clone(),
            loader: inst.loader.clone(),
            mods_src: game_dir.join("mods"),
            ram_mb: core_lib::instance::effective_ram_mb(inst.ram_mb, settings.default_ram_mb).clamp(1024, 8192),
            java_override: settings.java_path.as_deref().map(PathBuf::from),
            java_root: self.paths.java.clone(),
        };
        let active = rt.start_server(g, params, tip).await?;
        // L'accès des membres s'ouvre **avant** d'annoncer le bail `Ready` : un membre qui rejoint dès l'annonce (cas du
        // candidat qui a cédé) ne doit pas trouver le tunnel fermé. `abort_hosting` le referme si on perd le départage.
        let st = match rt.require_state(&g) {
            Ok(st) => st,
            Err(e) => {
                // Groupe supprimé entre-temps : on ne laisse pas de serveur orphelin.
                rt.abort_hosting(g).await;
                return Err(e);
            }
        };
        self.access.allow_members(g, st.members.iter().copied().filter(|k| *k != rt.me()));
        let port = active.port();
        self.backend().host_set_world_port(port);
        if let Some(holder) = rt.announce_lease(&active).await {
            // Un autre bail l'emporte (serveur prêt avant le nôtre, version de départ plus avancée, ou plus petite clé).
            self.backend().host_set_world_port(0);
            rt.abort_hosting(g).await;
            return Ok(Some(holder));
        }
        rt.run_hosting(&active);
        // Les membres atteignent le serveur par le tunnel, et le manifeste du modpack par le même accès.
        if let Err(e) = self.backend().host_start_sharing(instance.to_string(), port).await {
            eprintln!("[cubelink] partage du modpack du groupe : {e}");
        }
        rt.set_play(&g, Some(Play::Launching { steps: HOST_STEPS, index: 2 }));
        if let Err(e) = rt.launcher().launch(self, instance, port).await {
            self.hearth_stop_inner(g).await;
            return Err(e);
        }
        rt.set_play(&g, None);
        self.hearth_watch(g, instance.to_string(), true);
        Ok(None)
    }

    /// Ouvre mon jeu sur mon propre serveur (qui tourne déjà).
    async fn hearth_launch_client(self: &Arc<Self>, g: GroupId, instance: &str, port: u16, hosting: bool) -> Result<(), String> {
        self.hearth.launcher().launch(self, instance, port).await?;
        self.hearth_watch(g, instance.to_string(), hosting);
        Ok(())
    }

    /// Surveille la fin du jeu lancé pour ce groupe : l'état « lancement » tombe, et l'hôte décide du serveur.
    fn hearth_watch(self: &Arc<Self>, g: GroupId, instance: String, hosting: bool) {
        let st = self.clone();
        crate::hearth::spawn(async move {
            // Le verrou de jeu est pris avant le retour de `launch_with` : une disparition est une vraie fin de partie.
            while st.game_current().as_deref() == Some(instance.as_str()) {
                tokio::time::sleep(Duration::from_millis(500)).await;
            }
            if matches!(st.hearth.play_of(&g), Some(Play::Joined { .. })) {
                st.hearth.set_play(&g, None);
            }
            if hosting {
                st.hearth_host_client_exited(g).await;
            }
        });
    }

    /// Mon jeu s'est fermé : le serveur continue seul (réglage), passe la main (d'autres jouent) ou s'arrête.
    async fn hearth_host_client_exited(self: &Arc<Self>, g: GroupId) {
        let rt = self.hearth.clone();
        let Some(active) = rt.active(&g) else { return };
        if rt.keep_background(&g) {
            rt.emit_state();
            return;
        }
        // Le serveur écrit « left the game » un instant après la fermeture du jeu.
        tokio::time::sleep(Duration::from_secs(2)).await;
        let players = active.players();
        if players > 0 {
            let name = rt.state(&g).map(|s| s.name).unwrap_or_default();
            rt.emit_json("hearth://host_prompt", &HearthHostPrompt { group_id: g.to_hex(), name, players });
            rt.emit_state();
            return;
        }
        self.hearth_stop_inner(g).await;
    }

    /// Arrête le serveur du groupe : publie la version, relâche le bail (les joueurs encore là reçoivent la passation).
    pub async fn hearth_stop(self: &Arc<Self>, id: &str) -> Result<(), String> {
        let g = gid(id)?;
        let rt = self.hearth.clone();
        let host = rt.active(&g).ok_or("ce serveur n'est pas lancé sur ce PC")?;
        if self.game_current().as_deref() == Some(host.instance_id.as_str()) {
            self.game_close().await?;
            let _ = self.stop_game_and_wait(Duration::from_secs(30)).await;
        }
        self.hearth_stop_inner(g).await;
        Ok(())
    }

    /// Arrête tous les serveurs de groupe de ce PC (fermeture de Cubelink) : publication, bail relâché, partage fermé. Le
    /// second appelant attend la fin du premier.
    pub async fn hearth_stop_all(self: &Arc<Self>) {
        let _one_at_a_time = self.hearth.stop_all_guard().await;
        for g in self.hearth.active_groups() {
            if tokio::time::timeout(Duration::from_secs(150), self.hearth_stop_inner(g)).await.is_err() {
                eprintln!("[cubelink] l'arrêt du serveur du groupe {g} a pris trop de temps à la fermeture");
            }
        }
    }

    /// Arrête le serveur du groupe `g` et ferme tout ce qui s'y rattache (partage, accès, état de lancement). Appelé aussi
    /// quand le serveur s'arrête tout seul.
    pub(crate) async fn hearth_stop_inner(self: &Arc<Self>, g: GroupId) {
        // Groupe oublié pendant la partie : le serveur est déjà arrêté, il ne reste que le partage et l'état à fermer.
        if self.hearth.active(&g).is_some() {
            match self.hearth.finish_hosting(g).await {
                Ok(_) => {}
                Err(e) => eprintln!("[cubelink] arrêt du serveur du groupe {g} : {e}"),
            }
        }
        self.backend().host_stop_sharing();
        self.access.close_group(&g);
        self.hearth.set_play(&g, None);
    }

    /// Choix du groupe après une partie mal terminée : `wait` (ne rien faire), `checkpoint` (reprendre depuis son
    /// dernier point de reprise) ou `last` (depuis la dernière version connue).
    pub async fn hearth_unclean_choice(self: &Arc<Self>, id: &str, choice: &str) -> Result<(), String> {
        self.hearth.require_enabled()?;
        let g = gid(id)?;
        let rt = self.hearth.clone();
        if choice == "wait" {
            return Ok(());
        }
        let Some(Decision::Unclean { holder, last_checkpoint }) = rt.decision(&g) else {
            return Err("il n'y a pas de partie mal terminée à reprendre".into());
        };
        let base = match choice {
            "checkpoint" => Some(last_checkpoint.ok_or("aucun point de reprise n'est connu")?),
            "last" => None,
            _ => return Err("choix inconnu".into()),
        };
        let instance = rt.instance_of(&g).ok_or("Installe d'abord le modpack de ce groupe.")?;
        if !rt.try_begin_play(&g, HOST_STEPS) {
            return Err("un lancement de ce serveur est déjà en cours".into());
        }
        rt.revoke_lease(&g, &holder);
        let r = async {
            if let Some(b) = base {
                // Le point de reprise doit être rapatrié avant d'en repartir.
                let v = rt.version(&g, &b).ok_or("le point de reprise est introuvable")?;
                rt.fetch_version_user(g, &v, None).await?;
            }
            self.hearth_host(g, &instance, base).await
        }
        .await;
        if r.is_err() {
            rt.set_play(&g, None);
        }
        r
    }
}

fn dir_bytes(root: &std::path::Path) -> u64 {
    let mut total = 0;
    let mut stack = vec![root.to_path_buf()];
    while let Some(d) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&d) else { continue };
        for e in rd.flatten() {
            match e.file_type() {
                Ok(t) if t.is_dir() => stack.push(e.path()),
                Ok(t) if t.is_file() => total += e.metadata().map_or(0, |m| m.len()),
                _ => {}
            }
        }
    }
    total
}

// ---------------------------------------------------------------------------------------------------------
// Commandes Tauri
// ---------------------------------------------------------------------------------------------------------

type S<'a> = State<'a, Arc<AppState>>;

/// Serveurs de groupe de ce PC (cartes de l'accueil).
#[tauri::command(rename_all = "snake_case")]
#[specta::specta(rename_all = "snake_case")]
pub fn hearth_list(state: S<'_>) -> Vec<HearthCard> {
    state.hearth_list()
}

/// Page d'un serveur de groupe : membres, versions du monde, votes, réglages.
#[tauri::command(rename_all = "snake_case")]
#[specta::specta(rename_all = "snake_case")]
pub async fn hearth_detail(state: S<'_>, id: String) -> Result<HearthDetail, String> {
    state.hearth_detail(&id).await
}

/// Mondes d'une instance, proposés à la création d'un groupe.
#[tauri::command(rename_all = "snake_case")]
#[specta::specta(rename_all = "snake_case")]
pub async fn hearth_worlds(state: S<'_>, instance_id: String) -> Result<Vec<HearthWorld>, String> {
    state.hearth_worlds(&instance_id).await
}

/// Crée un serveur de groupe à partir d'une instance (et d'un de ses mondes) et invite les amis donnés.
#[tauri::command(rename_all = "snake_case")]
#[specta::specta(rename_all = "snake_case")]
pub async fn hearth_create(
    state: S<'_>,
    name: String,
    instance_id: String,
    world: Option<String>,
    members: Vec<String>,
) -> Result<HearthCard, String> {
    state.inner().hearth_create(&name, &instance_id, world, members).await
}

/// Invitations à des groupes reçues d'amis.
#[tauri::command(rename_all = "snake_case")]
#[specta::specta(rename_all = "snake_case")]
pub fn hearth_invites(state: S<'_>) -> Vec<HearthInvite> {
    state.hearth_invites()
}

/// Accepte une invitation à un groupe.
#[tauri::command(rename_all = "snake_case")]
#[specta::specta(rename_all = "snake_case")]
pub fn hearth_invite_accept(state: S<'_>, id: String) -> Result<(), String> {
    state.hearth_invite_accept(&id)
}

/// Écarte une invitation à un groupe.
#[tauri::command(rename_all = "snake_case")]
#[specta::specta(rename_all = "snake_case")]
pub fn hearth_invite_dismiss(state: S<'_>, id: String) -> Result<(), String> {
    state.hearth_invite_dismiss(&id)
}

/// Joue sur le serveur du groupe : le rejoint s'il est hébergé, sinon l'héberge sur ce PC.
#[tauri::command(rename_all = "snake_case")]
#[specta::specta(rename_all = "snake_case")]
pub async fn hearth_play(state: S<'_>, id: String) -> Result<(), String> {
    state.inner().hearth_play(&id).await
}

/// Arrête le serveur du groupe hébergé sur ce PC (monde publié, bail relâché).
#[tauri::command(rename_all = "snake_case")]
#[specta::specta(rename_all = "snake_case")]
pub async fn hearth_stop(state: S<'_>, id: String) -> Result<(), String> {
    state.inner().hearth_stop(&id).await
}

/// Jouer depuis la dernière version complète quand le contenu de la dernière version n'est chez personne de joignable.
#[tauri::command(rename_all = "snake_case")]
#[specta::specta(rename_all = "snake_case")]
pub async fn hearth_play_from_previous(state: S<'_>, id: String) -> Result<(), String> {
    state.inner().hearth_play_from_previous(&id).await
}

/// Choix du groupe après une partie mal terminée.
#[tauri::command(rename_all = "snake_case")]
#[specta::specta(rename_all = "snake_case")]
pub async fn hearth_unclean_choice(state: S<'_>, id: String, choice: String) -> Result<(), String> {
    state.inner().hearth_unclean_choice(&id, &choice).await
}

/// Associe l'instance locale du modpack au groupe.
#[tauri::command(rename_all = "snake_case")]
#[specta::specta(rename_all = "snake_case")]
pub async fn hearth_set_instance(state: S<'_>, id: String, instance_id: String) -> Result<(), String> {
    state.hearth_set_instance(&id, &instance_id).await
}

/// Propose d'ajouter un ami au groupe.
#[tauri::command(rename_all = "snake_case")]
#[specta::specta(rename_all = "snake_case")]
pub async fn hearth_add_member(state: S<'_>, id: String, public_key: String) -> Result<(), String> {
    state.hearth_add_member(&id, &public_key).await
}

/// Propose de retirer un membre du groupe.
#[tauri::command(rename_all = "snake_case")]
#[specta::specta(rename_all = "snake_case")]
pub async fn hearth_remove_member(state: S<'_>, id: String, public_key: String) -> Result<(), String> {
    state.hearth_remove_member(&id, &public_key).await
}

/// Quitte le groupe, après avoir attendu (jusqu'à 5 min) que ma dernière partie soit remise à un membre. Rejette `{ code: "last_member" | "unreachable" | "outbox_pending" | "other", message? }` : rien n'est supprimé dans ces cas. `force: true` saute l'attente de la file d'envoi (la dernière partie est perdue pour le groupe).
#[tauri::command(rename_all = "snake_case")]
#[specta::specta(rename_all = "snake_case")]
pub async fn hearth_leave(state: S<'_>, id: String, force: Option<bool>) -> Result<(), LeaveError> {
    state.inner().hearth_leave(&id, force.unwrap_or(false)).await
}

/// Supprime le serveur de ce PC seulement, sans prévenir le groupe : arrête mon serveur, libère les blobs et efface le dossier (choix explicite de l'utilisateur quand personne n'est joignable, jamais un repli automatique).
#[tauri::command(rename_all = "snake_case")]
#[specta::specta(rename_all = "snake_case")]
pub async fn hearth_forget(state: S<'_>, id: String) -> Result<(), String> {
    state.inner().hearth_forget(&id).await
}

/// Approuve une décision du groupe en attente.
#[tauri::command(rename_all = "snake_case")]
#[specta::specta(rename_all = "snake_case")]
pub async fn hearth_vote(state: S<'_>, id: String, proposal_id: String) -> Result<(), String> {
    state.hearth_vote(&id, &proposal_id).await
}

/// Propose de continuer sans la version d'un membre absent.
#[tauri::command(rename_all = "snake_case")]
#[specta::specta(rename_all = "snake_case")]
pub async fn hearth_continue_without(state: S<'_>, id: String, public_key: String) -> Result<(), String> {
    state.hearth_continue_without(&id, &public_key).await
}

/// Propose de passer le groupe à une nouvelle version du modpack.
#[tauri::command(rename_all = "snake_case")]
#[specta::specta(rename_all = "snake_case")]
pub async fn hearth_pack_update(state: S<'_>, id: String, instance_id: String) -> Result<(), String> {
    state.hearth_pack_update(&id, &instance_id).await
}

/// Restaure une version du monde.
#[tauri::command(rename_all = "snake_case")]
#[specta::specta(rename_all = "snake_case")]
pub async fn hearth_restore(state: S<'_>, id: String, version_id: String) -> Result<(), String> {
    state.hearth_restore(&id, &version_id).await
}

/// Choisit la version gardée quand le monde a deux versions.
#[tauri::command(rename_all = "snake_case")]
#[specta::specta(rename_all = "snake_case")]
pub async fn hearth_resolve_fork(state: S<'_>, id: String, version_id: String) -> Result<(), String> {
    state.hearth_resolve_fork(&id, &version_id).await
}

/// Garde le serveur ouvert quand le jeu de ce PC se ferme.
#[tauri::command(rename_all = "snake_case")]
#[specta::specta(rename_all = "snake_case")]
pub fn hearth_set_background(state: S<'_>, id: String, on: bool) -> Result<(), String> {
    state.hearth_set_background(&id, on)
}

/// État de l'acceptation du CLUF Minecraft (serveur dédié) et son adresse.
#[tauri::command(rename_all = "snake_case")]
#[specta::specta(rename_all = "snake_case")]
pub fn hearth_eula(state: S<'_>) -> HearthEula {
    state.hearth_eula()
}

/// Serveurs de groupe activés (fonction expérimentale).
#[tauri::command(rename_all = "snake_case")]
#[specta::specta(rename_all = "snake_case")]
pub fn hearth_enabled(state: S<'_>) -> bool {
    state.hearth_enabled()
}

/// Active ou coupe les serveurs de groupe.
#[tauri::command(rename_all = "snake_case")]
#[specta::specta(rename_all = "snake_case")]
pub fn hearth_set_enabled(state: S<'_>, enabled: bool) {
    state.hearth_set_enabled(enabled);
}

/// Accepte le CLUF Minecraft pour le serveur dédié.
#[tauri::command(rename_all = "snake_case")]
#[specta::specta(rename_all = "snake_case")]
pub fn hearth_eula_accept(state: S<'_>) {
    state.hearth_eula_accept();
}

# P2m : téléchargement Modrinth via le magasin (core/src/providers/modrinth_dl.rs)
- API : `fetch_into_store(file, store_root, &StoreIndex, &DlOptions) -> StoredObject{sha256, rel_path, size, from_cache}` ; `install_to_instance(object, store_root, index, game_dir, filename) -> LinkKind` (valide le nom, `link_into` vers `mods/<filename>`, `add_ref`).
- Flux : alias -> cache sans réseau ; sinon flux vers `store_root/.tmp/<uuid>.part`, sha1+sha256+sha512 en une passe, vérif taille + sha512 (sinon sha1), rename atomique vers `object_rel_path`, `insert`. Mismatch : temporaire supprimé, `Error::Integrity`.
- Retries 3x sur réseau/5xx (délai injectable), 4xx sans retry ; annulation `Arc<AtomicBool>` ; callback de progression.
- Limite 1 : `StoreIndex` n'a pas de table d'alias -> `store_root/aliases.json` (clés `sha512:`/`sha1:` -> sha256), écriture atomique (verrou de processus uniquement).
- Limite 2 : pas de crate `sha1` dans les dépendances -> SHA-1 minimal interne (testé sur vecteurs connus).
- Limite 3 : « présent et valide » = entrée d'index + fichier existant + taille identique (pas de re-hash).
- Au moins un hash Modrinth est exigé (sinon `Error::Invalid`). 10 tests wiremock/unitaires OK, clippy -D warnings OK.

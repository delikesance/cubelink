# Rapport P1m : client API Modrinth v2

- Fichier : `core/src/providers/modrinth_api.rs` (`ModrinthClient`, base URL et User-Agent injectables, aucun contact codé en dur).
- `search` : `facets` JSON `[["versions:..."],["categories:..."],["project_type:..."]]`, limit/offset ; `SearchResult{hits,total_hits}`.
- `project_versions` : `game_versions` et `loaders` en tableaux JSON ; `Version`/`VersionFile`/`FileHashes`/`Dependency` typés.
- `version_from_hash` (404 -> `Ok(None)`) et `versions_from_hashes` (POST /version_files, map hash -> Version, lot vide sans appel).
- Erreurs : 429 -> `Network` avec le Retry-After ; 404 -> `NotFound` ; 5xx et JSON invalide -> `Network`. `Loader::Vanilla` n'ajoute aucun filtre.
- Tests : 10 wiremock/unitaires OK. Clippy bloqué par un import inutilisé dans `loader/quilt.rs` (hors périmètre).

# Faux Modrinth (E0.3)

Node >= 18, aucune dépendance, écoute 127.0.0.1 uniquement, contenus fictifs.

- Lancer : `node tests-fixtures/modrinth/server.mjs --port 0` -> ligne JSON `{"ready":true,"port":N,"base":"http://127.0.0.1:N"}`.
- API : base `<base>/v2` (donc `ModrinthClient::with_base_url("<base>/v2", ua)`) ; jars et `cubelink-test-pack-1.0.0.mrpack` sous `<base>/cdn/<fichier>`.
- Fautes : `POST /__control` avec `{"rateLimit":{"count":1,"retryAfter":5}}`, `{"error":{"count":1,"status":500}}`, `{"slowMs":500}`, `{"tamper":true}` (octets du CDN altérés, l'API annonce toujours le bon hash), `{"reset":true}` (efface fautes et journal). Sans `count` : permanent. `GET /__state` : journal des requêtes, fautes, catalogue.
- Mod maison : `node build-home-mod.mjs [dossier]` (imprime le sha256, écrit le jar si dossier) ; absent du catalogue, 404 sur `/version_file/<sha1>`.
- Instance hôte : `node make-instance-fixture.mjs <dir>` crée `mods/` (4 mods catalogue + mod maison), `config/cubelink-test.toml`, et `options.txt`, `saves/`, `servers.dat` (à ne pas partager) ; imprime le manifeste attendu (chemins, tailles, sha256).
- Tests : `node --test "tests-fixtures/modrinth/*.test.mjs"` (Node >= 21 n’accepte plus un dossier nu ; sur Node 18/20, `node --test tests-fixtures/modrinth/` fonctionne).
- Pointer l'app : avec `CUBELINK_TEST_MODE=1`, `CUBELINK_MODRINTH_API_URL` (127.0.0.1/localhost seulement) ; voir `docs/contrats.md` (historique : docs/rapports/E0.3.md). Pour un `.mrpack`, ajouter `127.0.0.1` à `ImportOptions.allowed_hosts`.

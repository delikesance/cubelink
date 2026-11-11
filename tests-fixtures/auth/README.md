# Faux serveurs d'authentification (E2E)

Lancement : `node tests-fixtures/auth/server.mjs --port 0` (Node >= 18, sans dépendance, 127.0.0.1 seulement).
Écrit sur stdout `{"ready":true,"port":N,"base":"http://127.0.0.1:N"}`. Tous les chemins sont servis sur la même base : Microsoft `/devicecode` et `/token` (préfixe `/consumers/oauth2/v2.0` accepté), Xbox `/user/authenticate`, XSTS `/xsts/authorize`, Minecraft `/authentication/login_with_xbox`, `/entitlements/mcstore`, `/minecraft/profile`.

Scénarios : `POST /__control {"scenario": "..."}` (remet l'état à zéro) ; `GET /__state` renvoie les requêtes reçues.
`ok`, `authorization_pending_then_ok` (2 attentes), `declined`, `expired_code`, `no_license`, `xbox_child_account`, `token_expired_then_refresh` (jetons Minecraft initiaux refusés en 401, valides après refresh), `server_error` (500 partout).
Profil : `TestPlayer`, UUID `069a79f4-44e9-4726-a5be-fca90e38aaf5`. Jetons préfixés `fake-`.

Variables d'environnement : avec `CUBELINK_TEST_MODE=1`, l'app accepte `CUBELINK_AUTH_{MS,XBOX,XSTS,MC}_URL` (limitées à 127.0.0.1/localhost) pour pointer vers ce faux serveur ; sans le mode test elles sont ignorées. Liste complète dans `docs/contrats.md`. (À l'origine, aucune surcharge n'existait : voir `docs/rapports/E0.1.md`.)

Test : `node --test tests-fixtures/auth/server.test.mjs` (Node 24 : `node --test <dossier>` ne marche plus)

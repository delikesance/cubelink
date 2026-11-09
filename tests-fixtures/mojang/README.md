# tests-fixtures/mojang

Faux miroir Mojang/Fabric/Adoptium et faux `java` pour les E2E. Node >= 18, sans dépendance, 127.0.0.1 uniquement.

## Tests

    node --test "tests-fixtures/mojang/*.test.mjs"

(Node >= 22 n'accepte plus un dossier nu comme argument de `--test`, d'où le glob.)

## Faux miroir

    node tests-fixtures/mojang/server.mjs --port 0
    -> {"ready":true,"port":N,"base":"http://127.0.0.1:N"}

Versions `1.21.1-test` (dernière) et `1.20.4-test`. Routes (`<base>` = URL ci-dessus) :

| Rôle | URL |
|---|---|
| Manifeste (`DEFAULT_MANIFEST_URL`) | `<base>/mc/game/version_manifest_v2.json` |
| Resources (`DEFAULT_RESOURCES_URL`) | `<base>/resources` |
| Fabric meta (`meta_url`) | `<base>` (routes `/v2/versions/loader/...`, Maven sous `/maven/`) |
| Adoptium (`DEFAULT_ADOPTIUM_URL`) | `<base>` (routes `/v3/assets/latest/{8,17,21,25}/hotspot`) |

Les URLs de version.json, client.jar, libraries, natives, index d'assets et JRE sont des URLs absolues vers le miroir
(sha1/sha256/tailles calculés au démarrage).

Fautes : `POST /__control` avec
`{"reset":true,"faults":[{"match":"<sous-chaîne du chemin>","mode":"bad_hash|404|500|slow","times":N,"delayMs":1500}]}`.
`times` omis = illimité ; `500` avec `times:2` = deux erreurs puis succès. `GET /__state` : requêtes reçues, compteurs, fautes restantes.

## Faux java

`fake-java.cmd` (Windows) / `fake-java.sh` (POSIX) / `node fake-java.mjs`. À utiliser comme `java_path` (réglage `java_path`).

- `-version` : `openjdk version "21.0.0"` sur stderr (`FAKE_JAVA_VERSION` pour changer).
- Sinon : écrit `<gameDir>/.cubelink-fake-launch.json` (args avec jeton masqué, cwd, version, classpath, `classpath_missing`, `lan_port`),
  échoue avec le code 1 si un fichier du classpath manque (`FAKE_JAVA_LENIENT=1` pour l'ignorer), écoute en TCP sur
  127.0.0.1:`FAKE_JAVA_LAN_PORT` (0 = libre ; port réel dans le témoin), journalise `Done`, reste vivant.
- Handshake : le client envoie l'octet `0x42`, reçoit `0x24` ; tout autre octet est renvoyé en écho.
- Arrêt : SIGTERM/SIGINT ou création de `<gameDir>/.cubelink-fake-stop` (`FAKE_JAVA_STOP_FILE`). Code de sortie : `FAKE_JAVA_EXIT_CODE`.
  `FAKE_JAVA_CRASH_AFTER_MS` : sortie spontanée.
- `FAKE_JAVA_NODE` : chemin de node si absent du PATH.

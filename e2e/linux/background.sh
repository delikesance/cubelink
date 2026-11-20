#!/bin/bash
# Usage : `bash e2e/linux/background.sh` (autonome). Il demarre lui-meme Xvfb, openbox (un vrai gestionnaire de fenetres :
# `wmctrl -c` lui demande de fermer la fenetre) et Vite sur 5173 (le binaire debug charge l'interface depuis devUrl),
# et se relance sous `dbus-run-session` si aucun bus de session n'existe. Prerequis : xvfb, openbox, xdotool, wmctrl,
# x11-utils (xwininfo), dbus-x11, un binaire `cargo build -p cubelink` et `npm ci`.
# Variables : CUBELINK_BIN (binaire), CARGO_TARGET_DIR (sinon target/), E2E_DISPLAY (defaut :99), E2E_LOG_DIR (journaux
# de l'app et de Xvfb/openbox/Vite ; defaut : dossier temporaire /tmp/cl-bg-logs.*, conserve).
# Verifie : fenetre montree, entree de demarrage avec --background, fermer = masquer (processus vivant), avis unique,
# instance unique (relancer remontre la fenetre), --background sans fenetre, start_with_os=false retire l entree.
# Essai reel de la phase 1 sous Linux (Xvfb + openbox + D-Bus) : vraie app Tauri, vrai disque, HOME isole.
# Code de sortie 0 seulement si toutes les verifications passent.
set -u
ROOT=$(cd "$(dirname "$0")/../.." && pwd)
if [ -z "${DBUS_SESSION_BUS_ADDRESS:-}" ]; then exec dbus-run-session -- bash "$0" "$@"; fi
BIN=${CUBELINK_BIN:-${CARGO_TARGET_DIR:-$ROOT/target}/debug/cubelink}
[ -x "$BIN" ] || { echo "FAIL binaire introuvable : $BIN (cargo build -p cubelink)"; exit 2; }
for t in Xvfb openbox xdotool wmctrl xwininfo; do command -v $t >/dev/null || { echo "FAIL outil manquant : $t"; exit 2; }; done
LOGS=${E2E_LOG_DIR:-$(mktemp -d /tmp/cl-bg-logs.XXXX)}; mkdir -p "$LOGS"
export HOME=$(mktemp -d /tmp/clhome.XXXX)
export DISPLAY=${E2E_DISPLAY:-:99}
EXTRA_PIDS=""
cleanup(){ for p in $EXTRA_PIDS; do kill "$p" 2>/dev/null; done; rm -rf "$HOME"; }
trap cleanup EXIT

Xvfb "$DISPLAY" -screen 0 1280x800x24 >"$LOGS/xvfb.log" 2>&1 & EXTRA_PIDS="$EXTRA_PIDS $!"
for _ in $(seq 1 40); do xdotool getdisplaygeometry >/dev/null 2>&1 && break; sleep 0.25; done
xdotool getdisplaygeometry >/dev/null 2>&1 || { echo "FAIL Xvfb ne repond pas ($DISPLAY)"; cat "$LOGS/xvfb.log"; exit 2; }
openbox >"$LOGS/openbox.log" 2>&1 & EXTRA_PIDS="$EXTRA_PIDS $!"
sleep 1
if ! (exec 3<>/dev/tcp/127.0.0.1/5173) 2>/dev/null; then
  (cd "$ROOT" && exec node_modules/.bin/vite --port 5173 --strictPort >"$LOGS/vite.log" 2>&1) & EXTRA_PIDS="$EXTRA_PIDS $!"
  for _ in $(seq 1 120); do (exec 3<>/dev/tcp/127.0.0.1/5173) 2>/dev/null && break; sleep 0.5; done
fi
DATA="$HOME/.local/share/app.cubelink.launcher"
AUTO="$HOME/.config/autostart"
ok(){ echo "OK   $*"; }; ko(){ echo "FAIL $*"; FAILS=$((FAILS+1)); }
FAILS=0
mapped(){ for id in $(xdotool search --name Cubelink 2>/dev/null); do xwininfo -id "$id" | grep -q '"Cubelink"' && xwininfo -id "$id" | grep -q IsViewable && return 0; done; return 1; }
wait_for(){ for _ in $(seq 1 60); do eval "$1" && return 0; sleep 0.5; done; return 1; }

# 1. Premier lancement normal
$BIN > "$LOGS/app1.log" 2>&1 & P1=$!
wait_for mapped && ok "fenetre montree au lancement" || ko "fenetre non montree"
wait_for "ls $AUTO/*.desktop >/dev/null 2>&1" && ok "entree de demarrage creee: $(grep -h '^Exec' $AUTO/*.desktop)" || ko "pas d'entree autostart"
grep -q -- "--background" $AUTO/*.desktop 2>/dev/null && ok "argument --background present" || ko "argument --background absent"

# 2. Fermer la fenetre (requete de fermeture du gestionnaire de fenetres)
wmctrl -c Cubelink; sleep 2
kill -0 $P1 2>/dev/null && ok "processus toujours vivant apres fermeture" || ko "processus termine"
mapped && ko "fenetre encore visible" || ok "fenetre masquee"
[ -f "$DATA/background-notice-shown" ] && ok "avis du premier masquage note" || ko "pas de temoin d'avis"

# 3. Relancer : instance unique, la fenetre existante revient
$BIN > "$LOGS/app2.log" 2>&1 & P2=$!
wait_for "! kill -0 $P2 2>/dev/null" && ok "seconde instance sortie" || { ko "seconde instance encore vivante"; kill $P2; }
wait_for mapped && ok "fenetre existante remontree" || ko "fenetre pas remontree"
[ "$(pgrep -fc "^$BIN$")" = "1" ] && ok "un seul processus Cubelink" || ko "plusieurs processus: $(pgrep -fa cubelink)"

# 3 bis. Masquer puis relancer en --background (demarrage de session) : la fenetre reste masquee
wmctrl -c Cubelink; sleep 2
$BIN --background > "$LOGS/app2b.log" 2>&1 & P2B=$!
wait_for "! kill -0 $P2B 2>/dev/null" && ok "seconde instance --background sortie" || { ko "seconde instance --background vivante"; kill $P2B; }
sleep 1; mapped && ko "fenetre remontree par un demarrage de session" || ok "fenetre toujours masquee apres demarrage de session"

# 4. Arret, reglage start_with_os=false, relance en --background
kill $P1; wait $P1 2>/dev/null
python3 - "$DATA/settings.json" <<'PY'
import json,sys
p=sys.argv[1]; s=json.load(open(p)) if __import__('os').path.exists(p) else None
if s is None:
    print("pas de settings.json, cree"); s={"language":"fr","default_ram_mb":4096,"java_path":None,"trust_default":"ask","close_on_launch":False,"share_mods":True}
s["start_with_os"]=False; json.dump(s,open(p,"w"))
PY
$BIN --background > "$LOGS/app3.log" 2>&1 & P3=$!
sleep 6
kill -0 $P3 2>/dev/null && ok "lancement --background vivant" || ko "lancement --background mort"
mapped && ko "fenetre visible en --background" || ok "fenetre non montree en --background"
ls $AUTO/*.desktop >/dev/null 2>&1 && ko "entree autostart encore la" || ok "entree autostart retiree (start_with_os=false)"
kill $P3; wait $P3 2>/dev/null
# 5. Mode test (E2E Windows : fermeture par WM_CLOSE) : fermer quitte, comme avant
TD=$(mktemp -d)
CUBELINK_TEST_MODE=1 CUBELINK_DATA_DIR=$TD $BIN > "$LOGS/app4.log" 2>&1 & P4=$!
wait_for mapped && ok "mode test : fenetre montree" || ko "mode test : fenetre non montree"
wmctrl -c Cubelink
wait_for "! kill -0 $P4 2>/dev/null" && ok "mode test : fermer quitte" || { ko "mode test : processus encore vivant"; kill $P4; }
rm -rf "$TD"
echo "ECHECS: $FAILS"
[ "$FAILS" = 0 ]

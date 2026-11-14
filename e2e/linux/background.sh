#!/bin/bash
# Usage : Xvfb :99 + un gestionnaire de fenetres (openbox) + `npx vite` sur 5173, puis
#   DISPLAY=:99 dbus-run-session -- bash e2e/linux/background.sh
# Verifie : fenetre montree, entree de demarrage avec --background, fermer = masquer (processus vivant), avis unique,
# instance unique (relancer remontre la fenetre), --background sans fenetre, start_with_os=false retire l entree.
# Essai reel de la phase 1 sous Linux (Xvfb + openbox + D-Bus) : vraie app Tauri, vrai disque, HOME isole.
set -u
BIN=${CUBELINK_BIN:-$(cd "$(dirname "$0")/../.." && pwd)/target/debug/cubelink}
export HOME=$(mktemp -d /tmp/clhome.XXXX)
export DISPLAY=:99
DATA="$HOME/.local/share/app.cubelink.launcher"
AUTO="$HOME/.config/autostart"
ok(){ echo "OK   $*"; }; ko(){ echo "FAIL $*"; FAILS=$((FAILS+1)); }
FAILS=0
mapped(){ for id in $(xdotool search --name Cubelink 2>/dev/null); do xwininfo -id "$id" | grep -q '"Cubelink"' && xwininfo -id "$id" | grep -q IsViewable && return 0; done; return 1; }
wait_for(){ for _ in $(seq 1 60); do eval "$1" && return 0; sleep 0.5; done; return 1; }

# 1. Premier lancement normal
$BIN > /tmp/app1.log 2>&1 & P1=$!
wait_for mapped && ok "fenetre montree au lancement" || ko "fenetre non montree"
wait_for "ls $AUTO/*.desktop >/dev/null 2>&1" && ok "entree de demarrage creee: $(grep -h '^Exec' $AUTO/*.desktop)" || ko "pas d'entree autostart"
grep -q -- "--background" $AUTO/*.desktop 2>/dev/null && ok "argument --background present" || ko "argument --background absent"

# 2. Fermer la fenetre (requete de fermeture du gestionnaire de fenetres)
wmctrl -c Cubelink; sleep 2
kill -0 $P1 2>/dev/null && ok "processus toujours vivant apres fermeture" || ko "processus termine"
mapped && ko "fenetre encore visible" || ok "fenetre masquee"
[ -f "$DATA/background-notice-shown" ] && ok "avis du premier masquage note" || ko "pas de temoin d'avis"

# 3. Relancer : instance unique, la fenetre existante revient
$BIN > /tmp/app2.log 2>&1 & P2=$!
wait_for "! kill -0 $P2 2>/dev/null" && ok "seconde instance sortie" || { ko "seconde instance encore vivante"; kill $P2; }
wait_for mapped && ok "fenetre existante remontree" || ko "fenetre pas remontree"
[ "$(pgrep -fc "^$BIN$")" = "1" ] && ok "un seul processus Cubelink" || ko "plusieurs processus: $(pgrep -fa cubelink)"

# 3 bis. Masquer puis relancer en --background (demarrage de session) : la fenetre reste masquee
wmctrl -c Cubelink; sleep 2
$BIN --background > /tmp/app2b.log 2>&1 & P2B=$!
wait_for "! kill -0 $P2B 2>/dev/null" && ok "seconde instance --background sortie" || { ko "seconde instance --background vivante"; kill $P2B; }
sleep 1; mapped && ko "fenetre remontree par un demarrage de session" || ok "fenetre toujours masquee apres demarrage de session"

# 4. Arret, reglage start_with_os=false, relance en --background
kill $P1; wait $P1 2>/dev/null
python3 - "$DATA/settings.json" <<'PY'
import json,sys
p=sys.argv[1]; s=json.load(open(p)) if __import__('os').path.exists(p) else None
if s is None:
    print("pas de settings.json, cree"); s={"language":"fr","default_ram_mb":4096,"java_path":None,"trust_default":"ask","allow_relay":True,"close_on_launch":False,"relay_only":False,"share_mods":True}
s["start_with_os"]=False; json.dump(s,open(p,"w"))
PY
$BIN --background > /tmp/app3.log 2>&1 & P3=$!
sleep 6
kill -0 $P3 2>/dev/null && ok "lancement --background vivant" || ko "lancement --background mort"
mapped && ko "fenetre visible en --background" || ok "fenetre non montree en --background"
ls $AUTO/*.desktop >/dev/null 2>&1 && ko "entree autostart encore la" || ok "entree autostart retiree (start_with_os=false)"
kill $P3; wait $P3 2>/dev/null
# 5. Mode test (E2E Windows : fermeture par WM_CLOSE) : fermer quitte, comme avant
TD=$(mktemp -d)
CUBELINK_TEST_MODE=1 CUBELINK_DATA_DIR=$TD $BIN > /tmp/app4.log 2>&1 & P4=$!
wait_for mapped && ok "mode test : fenetre montree" || ko "mode test : fenetre non montree"
wmctrl -c Cubelink
wait_for "! kill -0 $P4 2>/dev/null" && ok "mode test : fermer quitte" || { ko "mode test : processus encore vivant"; kill $P4; }
rm -rf "$TD"
echo "ECHECS: $FAILS"; rm -rf "$HOME"

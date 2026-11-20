#!/usr/bin/env bash
# Vérifie le paquet .deb sur un vrai Ubuntu (conteneur) : `apt install` résout les dépendances, puis le launcher
# démarre sous Xvfb et tient 20 s sans planter (pendant de rpm-smoke.sh).
# Usage : bash scripts/deb-smoke.sh <dossier contenant le .deb> [image, défaut ubuntu:22.04]
set -euo pipefail
dir=$(cd "$1" && pwd)
image=${2:-ubuntu:22.04}
docker run --rm -v "$dir":/deb:ro "$image" bash -euo pipefail -c '
  export DEBIAN_FRONTEND=noninteractive
  apt-get update -qq
  apt-get install -y -qq /deb/*.deb xvfb >/dev/null
  dpkg -l | grep -i cubelink
  bin=$(command -v cubelink)
  if ldd "$bin" | grep "not found"; then echo "bibliothèque manquante" >&2; exit 1; fi
  test -f /usr/lib/Cubelink/cubelink-lan-agent.jar
  Xvfb :99 >/dev/null 2>&1 &
  sleep 2
  set +e
  DISPLAY=:99 timeout 20 cubelink >/tmp/app.log 2>&1
  code=$?
  set -e
  cat /tmp/app.log
  # 124 = toujours en vie au bout de 20 s (arrêté par timeout) : c'"'"'est le succès attendu.
  if [ "$code" != 124 ]; then echo "le launcher s'"'"'est arrêté (code $code)" >&2; exit 1; fi
  echo "DEB OK sur $(. /etc/os-release; echo "$PRETTY_NAME")"
'

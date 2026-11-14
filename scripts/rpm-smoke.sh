#!/usr/bin/env bash
# Vérifie le paquet .rpm sur un vrai Fedora (conteneur) : `dnf install` résout les dépendances, puis le launcher
# démarre sous Xvfb et tient 20 s sans planter.
# Usage : bash scripts/rpm-smoke.sh <dossier contenant le .rpm> [image, défaut fedora:latest]
set -euo pipefail
dir=$(cd "$1" && pwd)
image=${2:-fedora:latest}
docker run --rm -v "$dir":/rpm:ro "$image" bash -euo pipefail -c '
  dnf install -y -q /rpm/*.rpm xorg-x11-server-Xvfb
  rpm -q cubelink
  if ldd /usr/bin/cubelink | grep "not found"; then echo "bibliothèque manquante" >&2; exit 1; fi
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
  echo "RPM OK sur $(. /etc/os-release; echo "$PRETTY_NAME")"
'

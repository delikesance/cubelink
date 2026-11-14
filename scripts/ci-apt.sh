#!/usr/bin/env bash
# Dépendances système du build Linux (Tauri / WebKitGTK) sur les runners Ubuntu 22.04 de GitHub.
# Usage : bash scripts/ci-apt.sh [paquets en plus...]   (ex. `xvfb` pour les scénarios X11 de la Scène)
# Miroir kernel.org : les noms ubuntu.com ne résolvent plus toujours depuis les runners.
set -euo pipefail

# Machine jetable : pas de réindexation des pages de manuel, pas de doc installée, pas de fsync à chaque fichier.
sudo rm -f /var/lib/man-db/auto-update
for l in force-unsafe-io 'path-exclude /usr/share/doc/*' 'path-exclude /usr/share/man/*' 'path-exclude /usr/share/locale/*'; do
  echo "$l"
done | sudo tee /etc/dpkg/dpkg.cfg.d/99-ci >/dev/null
sudo tee /etc/apt/sources.list >/dev/null <<'LIST'
deb http://mirrors.edge.kernel.org/ubuntu jammy main restricted universe multiverse
deb http://mirrors.edge.kernel.org/ubuntu jammy-updates main restricted universe multiverse
deb http://mirrors.edge.kernel.org/ubuntu jammy-security main restricted universe multiverse
LIST

pkgs="libwebkit2gtk-4.1-dev libayatana-appindicator3-dev librsvg2-dev libssl-dev patchelf xdg-utils $*"
for i in 1 2 3; do
  # shellcheck disable=SC2086
  sudo timeout 240 apt-get update && sudo timeout 600 apt-get install -y --no-install-recommends $pkgs && exit 0
  echo "apt : essai $i échoué, nouvel essai"; sleep 5
done
exit 1

#!/usr/bin/env bash
# Dépendances système du build Linux (Tauri / WebKitGTK) sur les runners Ubuntu 22.04 de GitHub.
# Usage : bash scripts/ci-apt.sh [paquets en plus...]   (ex. `xvfb` pour les scénarios X11 de la Scène)
# Sources : celles du runner d'abord, puis le miroir kernel.org (les noms ubuntu.com ne résolvent pas toujours depuis les
# runners, et kernel.org est parfois injoignable lui aussi : aucun des deux ne suffit seul).
set -euo pipefail

# Machine jetable : pas de réindexation des pages de manuel, pas de doc installée, pas de fsync à chaque fichier.
sudo rm -f /var/lib/man-db/auto-update
for l in force-unsafe-io 'path-exclude /usr/share/doc/*' 'path-exclude /usr/share/man/*' 'path-exclude /usr/share/locale/*'; do
  echo "$l"
done | sudo tee /etc/dpkg/dpkg.cfg.d/99-ci >/dev/null
KERNEL_ORG='deb http://mirrors.edge.kernel.org/ubuntu jammy main restricted universe multiverse
deb http://mirrors.edge.kernel.org/ubuntu jammy-updates main restricted universe multiverse
deb http://mirrors.edge.kernel.org/ubuntu jammy-security main restricted universe multiverse'

pkgs="libwebkit2gtk-4.1-dev libayatana-appindicator3-dev librsvg2-dev libssl-dev patchelf xdg-utils $*"
install() {
  # shellcheck disable=SC2086
  sudo timeout 240 apt-get update && sudo timeout 600 apt-get install -y --no-install-recommends $pkgs
}
for i in 1 2; do
  install && exit 0
  echo "apt (sources du runner) : essai $i échoué"; sleep 5
done
echo "apt : bascule sur le miroir kernel.org"
sudo mv /etc/apt/sources.list.d /etc/apt/sources.list.d.runner 2>/dev/null || true
sudo mkdir -p /etc/apt/sources.list.d
echo "$KERNEL_ORG" | sudo tee /etc/apt/sources.list >/dev/null
for i in 1 2; do
  install && exit 0
  echo "apt (kernel.org) : essai $i échoué"; sleep 5
done
exit 1

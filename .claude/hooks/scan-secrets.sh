#!/usr/bin/env bash
# Hook PreToolUse (matcher Bash) : bloque `git commit` si le contenu à committer ressemble à un secret.
# Entrée : JSON du hook sur stdin (tool_input.command, cwd).
# Sortie : sur stderr, uniquement le chemin du fichier et la catégorie, JAMAIS la valeur.
# Code 0 = autorisé, 2 = bloqué (aussi si jq est absent ou si le JSON est invalide : échec fermé).
# Portée : l'index (git diff --cached) ; avec `git commit -a/--all`, l'arbre suivi complet (git diff HEAD).
# Limite : un `git add x && git commit` dans la MÊME commande n'est pas vu, car l'ajout n'a pas encore eu lieu
# quand le hook s'exécute. Les fichiers non suivis ne sont scannés qu'une fois ajoutés à l'index.
set -u

block() {
  echo "BLOQUÉ (scan-secrets.sh) : $1" >&2
  exit 2
}

command -v jq >/dev/null 2>&1 || block "jq introuvable, la commande ne peut pas être analysée."
input=$(cat)
jq -e . >/dev/null 2>&1 <<<"$input" || block "entrée du hook illisible (JSON invalide)."
cmd=$(jq -r '.tool_input.command // ""' <<<"$input")
[ -n "$cmd" ] || exit 0
cwd=$(jq -r '.cwd // ""' <<<"$input")
[ -n "$cwd" ] || cwd=$PWD

# Seulement les commandes qui contiennent `git commit` (options globales -C/-c tolérées).
GITCOMMIT='(^|[[:space:];&|(])git([[:space:]]+(-C|-c)[[:space:]]+[^[:space:]]+)*[[:space:]]+commit([[:space:]]|$)'
grep -Eq -- "$GITCOMMIT" <<<"$cmd" || exit 0

# `commit -a` / `--all` / `-am` : git ajoutera les modifications des fichiers suivis au moment du commit.
after=$(sed -E 's/^.*[[:space:]]commit([[:space:]]|$)//' <<<"$cmd")
use_head=0
if grep -Eq -- '(^|[[:space:]])(--all|-[a-zA-Z]*a[a-zA-Z]*)([[:space:]]|$)' <<<"$after"; then
  use_head=1
fi

git -C "$cwd" rev-parse --git-dir >/dev/null 2>&1 || exit 0

if [ "$use_head" = 1 ] && git -C "$cwd" rev-parse --verify -q HEAD >/dev/null 2>&1; then
  base=(diff HEAD)
else
  base=(diff --cached)
fi

found=0
report() { # $1 = catégorie, $2 = chemin
  echo "BLOQUÉ (scan-secrets.sh) : $1 dans $2" >&2
  found=1
}

while IFS= read -r -d '' f; do
  name=${f##*/}
  # Noms de fichiers sensibles (.env.example est explicitement autorisé).
  if [ "$name" != ".env.example" ] && grep -Eq -- '^\.env(\..+)?$' <<<"$name"; then
    report "fichier d'environnement (.env)" "$f"
  fi
  grep -Eq -- '\.pem$' <<<"$name" && report "certificat ou clé (.pem)" "$f"
  grep -Eq -- '^id_rsa' <<<"$name" && report "clé SSH (id_rsa)" "$f"

  # Lignes ajoutées uniquement ; les valeurs ne sont jamais affichées.
  added=$(git -C "$cwd" "${base[@]}" -U0 --no-color -- "$f" 2>/dev/null | grep -E '^\+' | grep -vE '^\+\+\+ ')
  [ -n "$added" ] || continue
  grep -Eq -- '-----BEGIN [A-Z ]*PRIVATE KEY-----' <<<"$added" && report "en-tête de clé privée" "$f"
  grep -Eq -- 'ghp_[A-Za-z0-9]{30,}' <<<"$added" && report "jeton GitHub (ghp_)" "$f"
  grep -Eq -- 'sk-[A-Za-z0-9_-]{20,}' <<<"$added" && report "clé de type OpenAI (sk-)" "$f"
  grep -Eq -- 'AKIA[0-9A-Z]{16}' <<<"$added" && report "clé d'accès AWS (AKIA)" "$f"
done < <(git -C "$cwd" "${base[@]}" --name-only -z --diff-filter=ACMR 2>/dev/null)

if [ "$found" = 1 ]; then
  echo "Commit bloqué : retirer ces éléments de l'index (git restore --staged <fichier>) ou les sortir du dépôt." >&2
  exit 2
fi
exit 0

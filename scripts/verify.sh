#!/usr/bin/env bash
# Vérification Cubelink : une seule commande, un tampon qui prouve qu'elle a réussi sur CET arbre.
# Usage : bash scripts/verify.sh --quick | --full | --quick-changed | --help
# Aucun test réel (host/tests, E2E_REAL*) : ils ouvrent des fenêtres et exigent l'accord de l'utilisateur.
set -u

usage() {
  cat <<'EOF'
Usage : bash scripts/verify.sh <mode>

  --quick          npx tsc --noEmit ; npx vitest run ; cargo check -p <crate> pour les crates dont un
                   fichier Rust a changé (sinon : étape Rust ignorée).
  --full           cargo fmt --all --check ; cargo clippy --workspace --all-targets -- -D warnings ;
                   cargo test --workspace ; npx tsc --noEmit ; npx vitest run ;
                   cargo deny check (seulement si cargo-deny est installé).
  --quick-changed  Hook Stop : avertit seulement (code 0 toujours). tsc si src/ a changé,
                   cargo check -p <crate> pour les crates Rust modifiées. Lit stop_hook_active sur stdin.
  --help           Cette aide.

Tampon : $(git rev-parse --git-path cubelink-verify.stamp), écrit seulement si tout est vert.
Signature : sha256 de HEAD, `git status --porcelain`, `git diff HEAD`, `git diff --cached`.
Les fichiers non suivis ne comptent que par leur nom (via status), pas par leur contenu.
Un tampon --full satisfait --quick ; un tampon --quick ne satisfait pas --full.
EOF
}

mode=${1:-}
case "$mode" in
  --quick|--full|--quick-changed) ;;
  --help|-h) usage; exit 0 ;;
  *) usage >&2; exit 2 ;;
esac

# --- Hook Stop : stop_hook_active d'abord, avertissement seulement ---------------------------------------
if [ "$mode" = "--quick-changed" ]; then
  input=""
  if [ ! -t 0 ]; then input=$(timeout 2 cat 2>/dev/null || true); fi
  if grep -Eq '"stop_hook_active"[[:space:]]*:[[:space:]]*true' <<<"$input"; then
    exit 0
  fi
fi

root=$(git rev-parse --show-toplevel 2>/dev/null) || { echo "Pas dans un dépôt git." >&2; [ "$mode" = "--quick-changed" ] && exit 0; exit 2; }
cd "$root" || exit 2

# --- Fichiers modifiés et crates touchées ------------------------------------------------------------------
changed_files() {
  { git diff HEAD --name-only; git ls-files --others --exclude-standard; } 2>/dev/null | sort -u
}

# Imprime le nom de chaque crate du workspace dont un fichier .rs ou Cargo.toml a changé.
touched_crates() {
  command -v jq >/dev/null 2>&1 || return 0
  local meta files
  files=$(changed_files | grep -E '(\.rs|(^|/)Cargo\.toml)$' || true)
  [ -n "$files" ] || return 0
  meta=$(cargo metadata --no-deps --format-version 1 2>/dev/null | jq -r '.packages[] | "\(.name)\t\(.manifest_path)"' 2>/dev/null) || return 0
  local f name mp dir best bestlen
  local -A hit=()
  while IFS= read -r f; do
    best=""; bestlen=0
    while IFS=$'\t' read -r name mp; do
      dir=${mp%/Cargo.toml}; dir=${dir#"$root"/}
      case "$f/" in
        "$dir"/*) if [ ${#dir} -gt "$bestlen" ]; then best=$name; bestlen=${#dir}; fi ;;
      esac
    done <<<"$meta"
    [ -n "$best" ] && hit[$best]=1
  done <<<"$files"
  printf '%s\n' "${!hit[@]}" | sort
}

if [ "$mode" = "--quick-changed" ]; then
  # Avertissements seulement : jamais de blocage, jamais de test, de clippy ni de vitest.
  changed=$(changed_files)
  [ -n "$changed" ] || exit 0
  if grep -q '^src/' <<<"$changed"; then
    out=$(npx tsc --noEmit 2>&1) \
      || { echo "AVERTISSEMENT (verify.sh --quick-changed) : npx tsc --noEmit a échoué :" >&2; head -n 15 <<<"$out" >&2; }
  fi
  for c in $(touched_crates); do
    out=$(cargo check -p "$c" 2>&1) \
      || { echo "AVERTISSEMENT (verify.sh --quick-changed) : cargo check -p $c a échoué :" >&2; tail -n 15 <<<"$out" >&2; }
  done
  exit 0
fi

# --- Tampon ------------------------------------------------------------------------------------------------
stamp=$(git rev-parse --git-path cubelink-verify.stamp)

signature() {
  {
    echo "HEAD"; git rev-parse HEAD
    echo "STATUS"; git status --porcelain
    echo "DIFF-HEAD"; git diff HEAD
    echo "DIFF-CACHED"; git diff --cached
  } | sha256sum | cut -d' ' -f1
}

report_untracked() {
  local n
  n=$(git ls-files --others --exclude-standard | wc -l)
  if [ "$n" -gt 0 ]; then
    echo "ATTENTION : $n fichier(s) non suivi(s), non couverts par la signature (seul leur nom l'est) :"
    git ls-files --others --exclude-standard | head -n 10 | sed 's/^/  - /'
    [ "$n" -gt 10 ] && echo "  ... et $((n - 10)) autre(s)"
  else
    echo "Aucun fichier non suivi."
  fi
}

sig_start=$(signature)
report_untracked

if [ -f "$stamp" ]; then
  st_mode=$(sed -n 's/^mode=//p' "$stamp")
  st_sig=$(sed -n 's/^sig=//p' "$stamp")
  if [ "$st_sig" = "$sig_start" ]; then
    if [ "$st_mode" = "full" ] || [ "$mode" = "--quick" ]; then
      echo "déjà vert pour cet arbre (tampon $st_mode, signature ${sig_start:0:12})"
      exit 0
    fi
    echo "Tampon présent en mode $st_mode : insuffisant pour $mode, on relance."
  fi
fi

# --- Exécution ---------------------------------------------------------------------------------------------
ran=()
run() {
  echo
  echo "\$ $*"
  "$@"
  local rc=$?
  if [ $rc -ne 0 ]; then
    echo "ÉCHEC (code $rc) : $*"
    echo "Tampon non écrit."
    exit $rc
  fi
  echo "OK : $*"
  ran+=("$*")
}

if [ "$mode" = "--full" ]; then
  run cargo fmt --all --check
  run cargo clippy --workspace --all-targets -- -D warnings
  run cargo test --workspace
  run npx tsc --noEmit
  run npx vitest run
  if command -v cargo-deny >/dev/null 2>&1; then
    run cargo deny check
  else
    echo
    echo "cargo-deny absent : vérifié par la CI seulement"
  fi
  stamp_mode=full
else
  run npx tsc --noEmit
  run npx vitest run
  crates=$(touched_crates)
  if [ -n "$crates" ]; then
    for c in $crates; do run cargo check -p "$c"; done
  else
    echo
    echo "Aucun fichier Rust modifié : cargo check ignoré."
  fi
  stamp_mode=quick
fi

# L'arbre ne doit pas avoir bougé pendant la vérification (sinon le vert ne vaut pas pour l'arbre final).
sig_end=$(signature)
if [ "$sig_end" != "$sig_start" ]; then
  echo
  echo "L'arbre a changé pendant la vérification : tampon non écrit, relancer."
  exit 1
fi

tmp="$stamp.tmp.$$"
{
  echo "mode=$stamp_mode"
  echo "sig=$sig_end"
  echo "date=$(date -u +%Y-%m-%dT%H:%M:%SZ)"
  echo "head=$(git rev-parse HEAD)"
} >"$tmp" && mv "$tmp" "$stamp"

echo
echo "VERT ($stamp_mode) : ${#ran[@]} commande(s) exécutée(s)."
echo "Tampon écrit : $stamp (mode=$stamp_mode, signature ${sig_end:0:12})"
exit 0

#!/usr/bin/env bash
# Hook PreToolUse (matcher Bash) : bloque les commandes que CLAUDE.md réserve à l'utilisateur.
# Entrée : JSON du hook sur stdin (tool_input.command, cwd).
# Sortie : message sur stderr si bloqué. Code 0 = autorisé, 2 = bloqué.
# Limites : analyse textuelle, pas un shell. La commande est découpée sur ; && || | et les retours
# à la ligne ; un texte cité qui ressemble à une commande interdite est bloqué par prudence.
# `git -C <autre dossier> commit` n'est pas comparé à la branche de cet autre dossier.
set -u

cmd=""
block() {
  echo "BLOQUÉ (block-unsafe.sh) : $1" >&2
  [ -n "$cmd" ] && echo "Commande : $cmd" >&2
  exit 2
}
m()  { grep -Eq -- "$1" <<<"$2"; }   # motif ERE, sensible à la casse
mi() { grep -Eiq -- "$1" <<<"$2"; }  # motif ERE, insensible à la casse

command -v jq >/dev/null 2>&1 || block "jq introuvable, la commande ne peut pas être analysée."
input=$(cat)
jq -e . >/dev/null 2>&1 <<<"$input" || block "entrée du hook illisible (JSON invalide)."
cmd=$(jq -r '.tool_input.command // ""' <<<"$input")
[ -n "$cmd" ] || exit 0
cwd=$(jq -r '.cwd // ""' <<<"$input")
[ -n "$cwd" ] || cwd=$PWD

# Contrôle explicite de la branche courante : main et canary sont protégées.
branch=$(git -C "$cwd" symbolic-ref --short -q HEAD 2>/dev/null || true)
on_protected=0
if [ "$branch" = "main" ] || [ "$branch" = "canary" ]; then
  on_protected=1
fi

# Java : seulement par PID explicite. Pas de liste de PID construite à la volée.
if mi 'java' "$cmd"; then
  m 'xargs[^|;&]*kill' "$cmd" \
    && block "arrêt de java : donner un PID explicite (kill <pid>), pas de liste construite par xargs."
  m '(^|[[:space:]])kill[^|;&]*(\$\(|`)' "$cmd" \
    && block "arrêt de java : donner un PID explicite (kill <pid>), pas de substitution de commande."
fi

GIT='(^|[[:space:]])git([[:space:]]+(-C|-c)[[:space:]]+[^[:space:]]+)*[[:space:]]+'
REFSPEC='(^|[[:space:]])\+?([^[:space:]]*:)?(refs/heads/)?(main|canary)([[:space:]]|$)'

check_segment() {
  local s=$1

  if m "${GIT}push([[:space:]]|\$)" "$s"; then
    m '(^|[[:space:]])(--force[a-z-]*|-[a-zA-Z]*f[a-zA-Z]*)([[:space:]=]|$)' "$s" \
      && block "push forcé interdit."
    m '(^|[[:space:]])\+[^[:space:]]' "$s" \
      && block "push forcé (refspec +) interdit."
    m '(^|[[:space:]])--(all|mirror)([[:space:]]|$)' "$s" \
      && block "push --all/--mirror interdit : il pousserait main et canary."
    m "$REFSPEC" "$s" \
      && block "push vers main/canary interdit : passer par une PR vers canary."
    [ "$on_protected" = 1 ] \
      && block "push depuis $branch interdit : travailler sur une branche <type>/<sujet>."
  fi

  if m "${GIT}commit([[:space:]]|\$)" "$s" && [ "$on_protected" = 1 ]; then
    block "commit sur $branch interdit : créer une branche <type>/<sujet> depuis origin/canary."
  fi

  m '(^|[[:space:]])gh[[:space:]]+pr[[:space:]]+merge([[:space:]]|$)' "$s" \
    && block "gh pr merge interdit : seul l'utilisateur fusionne."
  m '(^|[[:space:]])gh[[:space:]]+release[[:space:]]+(create|delete)([[:space:]]|$)' "$s" \
    && block "gh release create/delete interdit : publication réservée à l'utilisateur."
  m '(^|[[:space:]])cargo([[:space:]]+\+[^[:space:]]+)?[[:space:]]+publish([[:space:]]|$)' "$s" \
    && block "cargo publish interdit : publication réservée à l'utilisateur."
  m '(^|[[:space:]])npm[[:space:]]+publish([[:space:]]|$)' "$s" \
    && block "npm publish interdit : publication réservée à l'utilisateur."

  if mi '(^|[[:space:]])taskkill([[:space:]]|$)' "$s" && mi '[[:space:]][/-]{1,2}im([[:space:]]|$)' "$s"; then
    block "taskkill /IM interdit : utiliser taskkill /PID <pid> sur le processus lancé par le test."
  fi
  if mi '(^|[[:space:]])(pkill|killall)([[:space:]]|$)' "$s" && mi 'java' "$s"; then
    block "pkill/killall java interdit : retrouver le PID par sa ligne de commande puis kill <pid>."
  fi
}

while IFS= read -r seg; do
  [ -n "$seg" ] && check_segment "$seg"
done < <(printf '%s\n' "$cmd" | sed -E 's/(&&|\|\||[;|&])/\n/g')

exit 0

#!/usr/bin/env bash
# Test des hooks PreToolUse : bash .claude/hooks/hooks.test.sh (sortie 0 = tout passe).
# Rejoue des entrées JSON d'outils Bash et REPL et compare le code de sortie attendu (0 autorisé, 2 bloqué).
set -u
dir=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
root=$(git -C "$dir" rev-parse --show-toplevel)
fails=0

run_case() { # $1 = hook, $2 = code attendu, $3 = clé (command|code), $4 = contenu, $5 = libellé
  local json got
  json=$(jq -n --arg k "$3" --arg v "$4" --arg cwd "$root" '{tool_input: {($k): $v}, cwd: $cwd}')
  bash "$dir/$1" <<<"$json" >/dev/null 2>&1
  got=$?
  if [ "$got" != "$2" ]; then
    echo "ÉCHEC $1 : $5 (attendu $2, obtenu $got)"
    fails=$((fails + 1))
  fi
}

run_case block-unsafe.sh 2 command 'git push origin main' "Bash : push vers main"
run_case block-unsafe.sh 2 code 'const r = await $`git push --dry-run origin HEAD:main`.nothrow();' "REPL : push vers main en template"
run_case block-unsafe.sh 2 code "await \$('git push --force origin chore/x');" "REPL : push forcé en chaîne"
run_case block-unsafe.sh 2 code 'await tools.Bash({command: "gh release create v1"});' "REPL : release via tools.Bash"
run_case block-unsafe.sh 2 code 'await $`pkill -f java`;' "REPL : pkill java"
run_case block-unsafe.sh 0 code 'await $`git push -u origin chore/code-mode`;' "REPL : push de branche autorisé"
run_case block-unsafe.sh 0 code 'const n = 1 + 2; ({ n })' "REPL : script sans commande"
run_case block-unsafe.sh 0 command 'git status' "Bash : commande anodine"
run_case scan-secrets.sh 0 code 'await $`git status`;' "REPL : pas de commit, pas de scan"

[ "$fails" = 0 ] && echo "hooks : tous les cas passent" || echo "hooks : $fails échec(s)"
[ "$fails" = 0 ]

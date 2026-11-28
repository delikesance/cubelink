# Sourcé par block-unsafe.sh et scan-secrets.sh : texte shell à analyser, tiré du JSON d'un hook PreToolUse.
# Bash : tool_input.command tel quel. REPL : le code JS (tool_input.code) ; chaque délimiteur de chaîne
# (` " ') devient un retour à la ligne et ${ } un espace, si bien que les commandes de $`...`, $("...") ou
# tools.Bash({command: "..."}) deviennent chacune un segment analysé comme une ligne de shell.
# Analyse textuelle : un texte cité qui ressemble à une commande interdite est bloqué par prudence.
hook_command() {
  local cmd
  cmd=$(jq -r '.tool_input.command // ""' <<<"$1")
  if [ -z "$cmd" ]; then
    cmd=$(jq -r '.tool_input.code // ""' <<<"$1" | sed -E 's/\$\{/ /g; s/\}/ /g' | tr "\`\"'" '\n\n\n')
  fi
  printf '%s' "$cmd"
}

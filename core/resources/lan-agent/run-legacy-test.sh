#!/usr/bin/env bash
# Teste l'agent sur les stubs legacy (SRG) et modernes, sous Java 8 reel (si trouve) et Java 17+.
# Usage : run-legacy-test.sh [chemin_java ...]   (sinon : detection automatique)
set -o pipefail
cd "$(dirname "$0")"
HERE="$(pwd)"
JAR="$HERE/../cubelink-lan-agent.jar"
W="$(mktemp -d)"; trap 'rm -rf "$W"' EXIT
JAVAC="${JAVA_HOME:+$JAVA_HOME/bin/}javac"
SEP=":"; command -v cygpath >/dev/null 2>&1 && SEP=";"
win() { if command -v cygpath >/dev/null 2>&1; then cygpath -m "$1"; else echo "$1"; fi; }
[ -f "$JAR" ] || { echo "jar absent : lance build.sh"; exit 2; }

# --- Compilation des stubs en classfile 8 (valables sur toutes les JVM) ---
for v in mc112 mc1710; do
  mkdir -p "$W/$v"; "$JAVAC" --release 8 -Xlint:-options -d "$W/$v" $(find "test-legacy/$v" -name '*.java') || exit 2
done
mkdir -p "$W/modern"; "$JAVAC" --release 8 -Xlint:-options -d "$W/modern" $(find test -name '*.java') || exit 2

# --- JVM a tester ---
JVMS=("$@")
if [ ${#JVMS[@]} -eq 0 ]; then
  for c in "$JAVA8_HOME/bin/java" "$APPDATA/app.cubelink.launcher/java"/*/bin/java.exe "$APPDATA/paladium-games/runtime/bin/java.exe" \
           "$LOCALAPPDATA/Programs/Paladium Games Launcher/jre/bin/java.exe" "/c/Program Files/Java"/*8*/bin/java.exe \
           "/usr/lib/jvm"/*8*/bin/java "$JAVA_HOME/bin/java" "$(command -v java 2>/dev/null)"; do
    [ -x "$c" ] && JVMS+=("$c")
  done
fi
has8=0; fail=0; run=0
for J in "${JVMS[@]}"; do
  ver="$("$J" -version 2>&1 | head -1)"
  echo "=== JVM : $J  ($ver)"
  case "$ver" in *'"1.8'*) has8=1;; esac
  jar="$(win "$JAR")"
  check() { # nom, attendu, sortie
    run=$((run+1))
    if grep -q "$2" <<<"$3"; then echo "  OK   $1"; else echo "  FAIL $1 (attendu : $2)"; echo "$3" | sed 's/^/       | /'; fail=$((fail+1)); fi
  }
  for v in mc112 mc1710; do
    out="$("$J" "-javaagent:$jar=map=$(win "$HERE/test-legacy/$v.map")" -cp "$(win "$W/$v")" net.minecraft.client.Minecraft 2>&1)"
    check "$v publie (port)" "\[Cubelink\] LAN: Published LAN server on port 54321" "$out"
    ! grep -q "UnsupportedClassVersionError" <<<"$out" || { echo "  FAIL $v UnsupportedClassVersionError"; fail=$((fail+1)); }
    out="$("$J" "-javaagent:$jar=probe,map=$(win "$HERE/test-legacy/$v.map")" -cp "$(win "$W/$v")$SEP$jar" app.cubelink.agent.LanAgent 2>&1)"
    check "$v probe" "TOUT SE RESOUT" "$out"
    out="$("$J" "-javaagent:$jar=join=127.0.0.1:25565,map=$(win "$HERE/test-legacy/$v.map")" -cp "$(win "$W/$v")" net.minecraft.client.Minecraft 2>&1)"
    check "$v join ignore" "join legacy" "$out"
  done
  out="$("$J" "-javaagent:$jar" -cp "$(win "$W/modern")" net.minecraft.client.Minecraft 2>&1)"
  check "stubs modernes (Mojang) publient" "Started serving on" "$out"
done
echo
[ $has8 -eq 1 ] && echo "Java 8 reel : TESTE" || echo "Java 8 reel : ABSENT (non teste)"
echo "$run verifications, $fail echec(s)"
[ $fail -eq 0 ]

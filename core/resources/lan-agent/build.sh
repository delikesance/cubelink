#!/usr/bin/env bash
# Construit ../cubelink-lan-agent.jar (agent Java, aucune dependance). Necessite un JDK >= 17 (cible Java 17).
set -euo pipefail
cd "$(dirname "$0")"
JAVA_HOME_BIN="${JAVA_HOME:+$JAVA_HOME/bin/}"
out=build; rm -rf "$out"; mkdir -p "$out/classes"
"${JAVA_HOME_BIN}javac" --release 17 -Xlint:all -d "$out/classes" src/app/cubelink/agent/LanAgent.java
printf 'Premain-Class: app.cubelink.agent.LanAgent\nAgent-Class: app.cubelink.agent.LanAgent\nCan-Redefine-Classes: false\nCan-Retransform-Classes: false\nLauncher-Agent-Class: app.cubelink.agent.LanAgent\n' > "$out/MANIFEST.MF"
"${JAVA_HOME_BIN}jar" --create --file ../cubelink-lan-agent.jar --manifest "$out/MANIFEST.MF" -C "$out/classes" .
rm -rf "$out"
echo "OK : $(cd .. && pwd)/cubelink-lan-agent.jar"

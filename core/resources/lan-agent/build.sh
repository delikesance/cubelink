#!/usr/bin/env bash
# Construit ../cubelink-lan-agent.jar (agent Java, aucune dependance). Necessite un JDK >= 17.
# LanAgent : classfile Java 8 (--release 8, charge par les JVM 8 de Forge 1.7.10-1.12.2).
# Modules9 : tout le code Java 9+ (modules), classfile 53, charge uniquement si java.specification.version >= 9.
set -euo pipefail
cd "$(dirname "$0")"
JAVA_HOME_BIN="${JAVA_HOME:+$JAVA_HOME/bin/}"
out=build; rm -rf "$out"; mkdir -p "$out/classes"
"${JAVA_HOME_BIN}javac" --release 9 -Xlint:all -d "$out/classes" src/app/cubelink/agent/Modules9.java
"${JAVA_HOME_BIN}javac" --release 8 -Xlint:all -Xlint:-options -d "$out/classes8" -cp "$out/classes" src/app/cubelink/agent/LanAgent.java
# LanAgent (classfile 52) rejoint Modules9 (classfile 53).
cp "$out/classes8/app/cubelink/agent/"*.class "$out/classes/app/cubelink/agent/"
printf 'Premain-Class: app.cubelink.agent.LanAgent\nAgent-Class: app.cubelink.agent.LanAgent\nCan-Redefine-Classes: false\nCan-Retransform-Classes: false\nLauncher-Agent-Class: app.cubelink.agent.LanAgent\n' > "$out/MANIFEST.MF"
"${JAVA_HOME_BIN}jar" --create --file ../cubelink-lan-agent.jar --manifest "$out/MANIFEST.MF" -C "$out/classes" .
rm -rf "$out"
echo "OK : $(cd .. && pwd)/cubelink-lan-agent.jar"

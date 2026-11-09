#!/bin/sh
DIR=$(cd "$(dirname "$0")" && pwd)
exec "${FAKE_JAVA_NODE:-node}" "$DIR/fake-java.mjs" "$@"

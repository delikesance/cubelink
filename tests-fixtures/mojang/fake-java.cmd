@echo off
setlocal
if defined FAKE_JAVA_NODE (set "NODE_BIN=%FAKE_JAVA_NODE%") else (set "NODE_BIN=node")
"%NODE_BIN%" "%~dp0fake-java.mjs" %*
exit /b %ERRORLEVEL%

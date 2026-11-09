#!/usr/bin/env node
// Faux `java` pour les E2E de lancement. Node >= 18, aucune dépendance, 127.0.0.1 uniquement.
//
//   fake-java -version            -> « openjdk version "21.0.0" » sur stderr, code 0
//   fake-java <args JVM + jeu>    -> écrit <gameDir>/.cubelink-fake-launch.json, ouvre un « monde LAN »
//                                    (serveur TCP local), journalise « Done », reste vivant jusqu'à
//                                    SIGTERM/SIGINT ou jusqu'à l'apparition du fichier d'arrêt.
//
// Variables d'environnement :
//   FAKE_JAVA_VERSION        version affichée par -version (défaut 21.0.0)
//   FAKE_JAVA_LAN_PORT       port TCP du faux monde LAN (défaut 0 = port libre, écrit dans le témoin)
//   FAKE_JAVA_EXIT_CODE      code de sortie à l'arrêt (défaut 0)
//   FAKE_JAVA_STOP_FILE      fichier d'arrêt (défaut <gameDir>/.cubelink-fake-stop)
//   FAKE_JAVA_LENIENT=1      ne pas échouer si un fichier du classpath manque (défaut : code 1)
//   FAKE_JAVA_CRASH_AFTER_MS si défini : sort tout seul après ce délai avec FAKE_JAVA_EXIT_CODE (défaut 1)
//
// Protocole du faux monde LAN : le client envoie l'octet HANDSHAKE_REQ (0x42), le serveur répond
// HANDSHAKE_REPLY (0x24) ; tout autre octet reçu est renvoyé tel quel (écho).
import fs from 'node:fs';
import net from 'node:net';
import path from 'node:path';
import { pathToFileURL } from 'node:url';

export const HANDSHAKE_REQ = 0x42;
export const HANDSHAKE_REPLY = 0x24;
export const WITNESS_NAME = '.cubelink-fake-launch.json';
export const STOP_NAME = '.cubelink-fake-stop';

const SECRET_FLAGS = new Set(['--accessToken', '--access-token']);

export function maskArgs(args) {
  const out = [];
  for (let i = 0; i < args.length; i++) {
    const a = args[i];
    const eq = a.indexOf('=');
    if (eq > 0 && SECRET_FLAGS.has(a.slice(0, eq))) out.push(`${a.slice(0, eq)}=***`);
    else if (SECRET_FLAGS.has(a) && i + 1 < args.length) { out.push(a, '***'); i++; }
    else out.push(a);
  }
  return out;
}

function valueOf(args, flag) {
  const i = args.indexOf(flag);
  if (i >= 0 && i + 1 < args.length) return args[i + 1];
  const pre = args.find((a) => a.startsWith(flag + '='));
  return pre ? pre.slice(flag.length + 1) : undefined;
}

function main() {
  const args = process.argv.slice(2);
  const version = process.env.FAKE_JAVA_VERSION || '21.0.0';

  if (args.includes('-version')) {
    process.stderr.write(`openjdk version "${version}" 2024-01-01\nOpenJDK Runtime Environment (fake build ${version})\nOpenJDK 64-Bit Server VM (fake)\n`);
    process.exit(0);
  }

  const cwd = process.cwd();
  const gameDir = path.resolve(valueOf(args, '--gameDir') ?? cwd);
  const cpIdx = args.findIndex((a) => a === '-cp' || a === '-classpath' || a === '--class-path');
  const classpath = cpIdx >= 0 && args[cpIdx + 1]
    ? args[cpIdx + 1].split(path.delimiter).filter(Boolean)
    : [];
  const classpathMissing = classpath.filter((p) => !fs.existsSync(path.resolve(cwd, p)));
  const mainClass = cpIdx >= 0 ? args[cpIdx + 2] : undefined;
  const exitCode = Number(process.env.FAKE_JAVA_EXIT_CODE ?? 0);
  const stopFile = process.env.FAKE_JAVA_STOP_FILE || path.join(gameDir, STOP_NAME);

  if (classpathMissing.length && process.env.FAKE_JAVA_LENIENT !== '1') {
    process.stderr.write(`Error: could not find or load main class ${mainClass ?? '?'}\nMissing classpath entries: ${classpathMissing.join(', ')}\n`);
    process.exit(1);
  }

  fs.mkdirSync(gameDir, { recursive: true });
  try { fs.rmSync(stopFile, { force: true }); } catch { /* ignoré */ }

  const server = net.createServer((sock) => {
    sock.on('error', () => {});
    sock.on('data', (buf) => {
      const out = Buffer.from(buf);
      if (out[0] === HANDSHAKE_REQ) out[0] = HANDSHAKE_REPLY;
      sock.write(out);
    });
  });
  server.on('error', (e) => {
    process.stderr.write(`LAN port error: ${e.message}\n`);
    process.exit(2);
  });

  let shuttingDown = false;
  const shutdown = () => {
    if (shuttingDown) return;
    shuttingDown = true;
    server.close();
    process.stdout.write('[Server thread/INFO]: Stopping the server\n', () => process.exit(exitCode));
  };
  process.on('SIGTERM', shutdown);
  process.on('SIGINT', shutdown);

  const port = Number(process.env.FAKE_JAVA_LAN_PORT ?? 0);
  server.listen(port, '127.0.0.1', () => {
    const lanPort = server.address().port;
    const witness = {
      args: maskArgs(args),
      cwd,
      game_dir: gameDir,
      version: valueOf(args, '--version') ?? null,
      java_version: version,
      main_class: mainClass ?? null,
      classpath,
      classpath_missing: classpathMissing,
      lan_port: lanPort,
      pid: process.pid,
      done: true,
    };
    const dest = path.join(gameDir, WITNESS_NAME);
    fs.writeFileSync(dest + '.tmp', JSON.stringify(witness, null, 2));
    fs.renameSync(dest + '.tmp', dest);
    process.stdout.write(`[Server thread/INFO]: Started serving on 127.0.0.1:${lanPort}\n`);
    process.stdout.write('[Server thread/INFO]: Done (0.123s)! For help, type "help"\n');
  });

  setInterval(() => { if (fs.existsSync(stopFile)) shutdown(); }, 100);

  const crash = process.env.FAKE_JAVA_CRASH_AFTER_MS;
  if (crash !== undefined) {
    setTimeout(() => process.exit(process.env.FAKE_JAVA_EXIT_CODE ? exitCode : 1), Number(crash));
  }
}

const isMain = process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href;
if (isMain) main();

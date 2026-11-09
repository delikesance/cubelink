import test from 'node:test';
import assert from 'node:assert/strict';
import fs from 'node:fs';
import os from 'node:os';
import net from 'node:net';
import path from 'node:path';
import { spawn, spawnSync } from 'node:child_process';
import { fileURLToPath } from 'node:url';
import { HANDSHAKE_REQ, HANDSHAKE_REPLY, WITNESS_NAME, STOP_NAME, maskArgs } from './fake-java.mjs';

const SCRIPT = fileURLToPath(new URL('./fake-java.mjs', import.meta.url));
const tmp = () => fs.mkdtempSync(path.join(os.tmpdir(), 'fakejava-'));
const run = (args, env = {}, cwd) =>
  spawn(process.execPath, [SCRIPT, ...args], { cwd, env: { ...process.env, ...env }, stdio: ['ignore', 'pipe', 'pipe'] });
const until = async (fn, ms = 8000) => {
  const t0 = Date.now();
  while (Date.now() - t0 < ms) { const v = fn(); if (v) return v; await new Promise((r) => setTimeout(r, 50)); }
  throw new Error('timeout');
};
const exited = (c) => new Promise((r) => c.on('exit', (code, sig) => r({ code, sig })));
const readWitness = (dir) => {
  const f = path.join(dir, WITNESS_NAME);
  return fs.existsSync(f) ? JSON.parse(fs.readFileSync(f, 'utf8')) : null;
};
const tcp = (port, bytes) => new Promise((res, rej) => {
  const s = net.connect(port, '127.0.0.1', () => s.write(Buffer.from(bytes)));
  s.once('data', (d) => { s.destroy(); res(d); });
  s.on('error', rej);
});

test('-version : version sur stderr, code 0', () => {
  const r = spawnSync(process.execPath, [SCRIPT, '-version'], { encoding: 'utf8' });
  assert.equal(r.status, 0);
  assert.match(r.stderr, /version "21\.0\.0"/);
});

test('-version via le lanceur de la plateforme', () => {
  const win = process.platform === 'win32';
  const launcher = fileURLToPath(new URL(win ? './fake-java.cmd' : './fake-java.sh', import.meta.url));
  const r = spawnSync(win ? `"${launcher}"` : launcher, ['-version'], { encoding: 'utf8', shell: win });
  assert.equal(r.status, 0);
  assert.match(r.stderr, /21\.0\.0/);
});

test('maskArgs masque le jeton', () => {
  assert.deepEqual(maskArgs(['--accessToken', 'SECRET', '--x', '1']), ['--accessToken', '***', '--x', '1']);
  assert.deepEqual(maskArgs(['--accessToken=SECRET']), ['--accessToken=***']);
});

test('lancement : témoin, classpath, port LAN, handshake, arrêt par fichier, code de sortie', async () => {
  const dir = tmp();
  const jar = path.join(dir, 'a.jar');
  fs.writeFileSync(jar, 'x');
  const cp = [jar, jar].join(path.delimiter);
  const c = run(
    ['-Xmx1G', '-cp', cp, 'net.minecraft.client.main.Main', '--username', 'Steve', '--version', '1.21.1-test', '--gameDir', dir, '--accessToken', 'SECRET-TOKEN'],
    { FAKE_JAVA_EXIT_CODE: '7' }, dir,
  );
  let out = '';
  c.stdout.on('data', (d) => (out += d));
  const w = await until(() => readWitness(dir));
  assert.equal(w.version, '1.21.1-test');
  assert.equal(fs.realpathSync(w.cwd), fs.realpathSync(dir));
  assert.deepEqual(w.classpath, [jar, jar]);
  assert.deepEqual(w.classpath_missing, []);
  assert.equal(w.main_class, 'net.minecraft.client.main.Main');
  assert.ok(!JSON.stringify(w).includes('SECRET-TOKEN'));
  assert.equal(w.args[w.args.indexOf('--accessToken') + 1], '***');
  assert.ok(w.lan_port > 0);

  assert.deepEqual([...(await tcp(w.lan_port, [HANDSHAKE_REQ]))], [HANDSHAKE_REPLY]);
  assert.deepEqual([...(await tcp(w.lan_port, [1, 2, 3]))], [1, 2, 3]);
  await until(() => /Done/.test(out));
  assert.equal(c.exitCode, null, 'toujours vivant');

  fs.writeFileSync(path.join(dir, STOP_NAME), '');
  const e = await exited(c);
  assert.equal(e.code, 7);
  fs.rmSync(dir, { recursive: true, force: true });
});

test('port LAN paramétrable', async () => {
  const dir = tmp();
  const probe = net.createServer().listen(0, '127.0.0.1');
  await new Promise((r) => probe.once('listening', r));
  const port = probe.address().port;
  await new Promise((r) => probe.close(r));
  const c = run(['--gameDir', dir], { FAKE_JAVA_LAN_PORT: String(port) });
  const w = await until(() => readWitness(dir));
  assert.equal(w.lan_port, port);
  assert.deepEqual([...(await tcp(port, [HANDSHAKE_REQ]))], [HANDSHAKE_REPLY]);
  fs.writeFileSync(path.join(dir, STOP_NAME), '');
  assert.equal((await exited(c)).code, 0);
  fs.rmSync(dir, { recursive: true, force: true });
});

test('classpath avec fichier manquant : code 1 (sauf FAKE_JAVA_LENIENT=1)', async () => {
  const dir = tmp();
  const args = ['-cp', path.join(dir, 'absent.jar'), 'Main', '--gameDir', dir];
  const r = spawnSync(process.execPath, [SCRIPT, ...args], { encoding: 'utf8' });
  assert.equal(r.status, 1);
  assert.match(r.stderr, /absent\.jar/);
  assert.equal(readWitness(dir), null);

  const c = run(args, { FAKE_JAVA_LENIENT: '1' });
  const w = await until(() => readWitness(dir));
  assert.equal(w.classpath_missing.length, 1);
  fs.writeFileSync(path.join(dir, STOP_NAME), '');
  await exited(c);
  fs.rmSync(dir, { recursive: true, force: true });
});

test('arrêt par SIGTERM (hors Windows)', { skip: process.platform === 'win32' }, async () => {
  const dir = tmp();
  const c = run(['--gameDir', dir], { FAKE_JAVA_EXIT_CODE: '3' });
  await until(() => readWitness(dir));
  c.kill('SIGTERM');
  assert.equal((await exited(c)).code, 3);
  fs.rmSync(dir, { recursive: true, force: true });
});

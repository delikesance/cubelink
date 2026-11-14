// Phase INVITE avec VRAI Minecraft (Fabric 26.3, JDK 25) : A (hote, vrai jeu + agent LAN) et B (invite, data-dir vide hormis
// versions/loaders/libraries copies). Amitie, join_start (transfert P2P des mods), join_play : B lance un vrai Minecraft en
// --quickPlayMultiplayer 127.0.0.1:<port du tunnel>. Profils HORS LIGNE : on rapporte ce que dit latest.log de B, sans
// contourner l'authentification du serveur LAN.   E2E_MC=1 npx playwright test -c e2e 15-real-guest --workers=1
import fs from 'node:fs';
import net from 'node:net';
import os from 'node:os';
import path from 'node:path';
import { execFileSync } from 'node:child_process';
import { test, expect, invoke, joinStart, testState } from './harness/fixtures';
import { AGENT_JAR, findJava25, killProcsOfDir, prepareDataDir, readLog, sleep, startRealApp, tmpRoot, waitLog, type RealApp } from './helpers/e2e14-real';
import { makeFriends, friendKeyOnB, guestInstance, recordEvents, waitProgress, lastProgress } from './helpers/e2e2-common';

test.skip(process.env.E2E_MC !== '1', 'Reserve a E2E_MC=1 : vrai Minecraft, Internet, JDK 25.');

const ID = 'fabulously-optimized';

/** Etat du java de l'invite quand le quickPlay ne part pas : pile du Render thread (jcmd), fenetre reduite/au premier plan. Lecture seule. */
function stallDiag(dirNeedle: string): string {
  const out: string[] = [];
  try {
    const pids = execFileSync('powershell', ['-NoProfile', '-Command', `Get-CimInstance Win32_Process -Filter "Name='java.exe'" | Where-Object { $_.CommandLine -like '*${dirNeedle.replace(/'/g, "''")}*' } | ForEach-Object { $_.ProcessId }`], { encoding: 'utf8', timeout: 30_000 }).split(/\s+/).filter(Boolean);
    for (const pid of pids) {
      try {
        const dump = execFileSync(path.join(path.dirname(findJava25()), 'jcmd.exe'), [pid, 'Thread.print'], { encoding: 'utf8', timeout: 30_000, maxBuffer: 20_000_000 });
        const i = dump.indexOf('"Render thread"');
        out.push(`pid ${pid} Render thread :\n` + (i >= 0 ? dump.slice(i, i + 3000).split(/\r?\n\r?\n/)[0] : '(introuvable)'));
      } catch (e) { out.push(`pid ${pid} jcmd echec : ${String(e).slice(0, 200)}`); }
    }
    out.push(execFileSync('powershell', ['-NoProfile', '-Command', `Add-Type 'using System;using System.Runtime.InteropServices;public class W{[DllImport("user32.dll")]public static extern bool IsIconic(IntPtr h);[DllImport("user32.dll")]public static extern IntPtr GetForegroundWindow();}'; Get-Process java | Where-Object { $_.MainWindowHandle -ne 0 } | ForEach-Object { "fenetre '" + $_.MainWindowTitle + "' pid " + $_.Id + " reduite=" + [W]::IsIconic($_.MainWindowHandle) + " premier-plan=" + ([W]::GetForegroundWindow() -eq $_.MainWindowHandle) }`], { encoding: 'utf8', timeout: 30_000 }));
  } catch (e) { out.push('diagnostic impossible : ' + String(e).slice(0, 200)); }
  return out.join('\n');
}

function javaCmd(needle: string): string {
  try {
    return execFileSync('powershell', ['-NoProfile', '-Command', `Get-CimInstance Win32_Process -Filter "Name='java.exe'" | Where-Object { $_.CommandLine -like '*${needle.replace(/'/g, "''")}*' } | ForEach-Object { $_.CommandLine }`], { encoding: 'utf8', timeout: 30_000 });
  } catch { return ''; }
}

async function boot(binary: string, root: string, name: string, instanceId: string | null, env: Record<string, string>) {
  const dataDir = path.join(root, 'data-' + name);
  fs.mkdirSync(path.join(root, name), { recursive: true });
  prepareDataDir(dataDir, instanceId);
  const app = await startRealApp(binary, path.join(root, name), dataDir, { CUBELINK_JAVA_PATH: findJava25(), ...env });
  await app.page.waitForSelector('main[data-route="login"]', { timeout: 30_000 });
  await invoke(app.page, 'auth_offline_login', { name });
  const s = await invoke<Record<string, unknown>>(app.page, 'settings_get');
  await invoke(app.page, 'settings_set', { settings: { ...s, close_on_launch: false } });
  return app;
}

/** Ping de statut Minecraft (handshake + status request) a travers le tunnel : prouve que le tunnel atteint le serveur de A. */
function statusPing(port: number): Promise<string> {
  const varint = (n: number) => { const b: number[] = []; do { let x = n & 0x7f; n >>>= 7; if (n) x |= 0x80; b.push(x); } while (n); return Buffer.from(b); };
  const pkt = (body: Buffer) => Buffer.concat([varint(body.length), body]);
  return new Promise((resolve, reject) => {
    const s = net.connect(port, '127.0.0.1');
    const host = Buffer.from('127.0.0.1');
    const hs = Buffer.concat([varint(0), varint(767), varint(host.length), host, Buffer.from([port >> 8, port & 255]), varint(1)]);
    s.on('connect', () => { s.write(pkt(hs)); s.write(pkt(varint(0))); });
    let buf = Buffer.alloc(0);
    s.on('data', (d) => {
      buf = Buffer.concat([buf, d]);
      const t = buf.toString('utf8');
      const i = t.indexOf('{"');
      if (i >= 0 && t.includes('"version"')) { s.destroy(); resolve(t.slice(i, i + 300)); }
    });
    s.setTimeout(30_000, () => { s.destroy(); reject(new Error('pas de reponse en 30 s (octets recus : ' + buf.length + ')')); });
    s.on('error', reject);
  });
}

test('invite reel : amitie, transfert P2P des mods, join_play avec vrai Minecraft', async ({ binary }) => {
  test.setTimeout(1_500_000);
  const root = tmpRoot('cubelink-mc2-');
  let a: RealApp | undefined;
  let b: RealApp | undefined;
  try {
    a = await boot(binary, root, 'Hote', ID, { CUBELINK_LAN_AGENT: AGENT_JAR, CUBELINK_LAN_NOAUTH: '1', CUBELINK_QUICKPLAY_WORLD: 'Nouveau monde', CUBELINK_LAN_NOAUTH: '1' /* TEST uniquement : serveur LAN sans verification de session (l'invite de test est hors ligne) */ });
    b = await boot(binary, root, 'Invite', null, {});
    const A = a;
    const B = b;
    await invoke(A.page, 'instances_launch', { id: ID });
    await waitLog(A.dataDir, ID, /Published LAN server on port|Started serving on|Local game hosted on port/, 600_000, () => (A.proc.exitCode !== null ? 'app A arretee' : null));
    await expect.poll(async () => (await invoke<{ world_port: number | null }>(A.page, 'host_state')).world_port, { timeout: 60_000 }).toBeGreaterThan(1023);
    console.log('[mc2] hote : ' + JSON.stringify(await invoke(A.page, 'host_state')));

    await makeFriends(A as any, B as any);
    const keyA = await friendKeyOnB(B as any);
    await recordEvents(B.page, 'transfer://progress');
    const plan = await invoke<{ to_receive: number; manifest: { files: unknown[] } }>(B.page, 'join_plan', { friend_public_key: keyA });
    console.log(`[mc2] plan : ${plan.manifest.files.length} fichiers, ${plan.to_receive} a recevoir`);
    await joinStart(B.page, keyA);
    await waitProgress(B.page, (p) => p.phase === 'done' || p.phase === 'failed', 900_000, 'fin du transfert');
    const last = await lastProgress(B.page);
    console.log('[mc2] fin du transfert : ' + JSON.stringify(last));
    expect(last.phase, last.error).toBe('done');
    const guest = await guestInstance(B as any);
    expect(guest).not.toBeNull();
    const modsDir = path.join(guest!.gameDir, 'mods');
    console.log(`[mc2] mods installes chez B : ${fs.existsSync(modsDir) ? fs.readdirSync(modsDir).length : 0}`);

    // Preuve du tunnel : ping de statut Minecraft a travers un tunnel ouvert vers A.
    const tp = await invoke<number>(B.page, 'join_open_tunnel', { friend_public_key: keyA });
    try { console.log(`[mc2] ping de statut via tunnel ${tp} : ` + await statusPing(tp)); }
    catch (e) { console.log('[mc2] ping de statut echec : ' + String(e)); }
    await invoke(B.page, 'join_close_tunnel');

    await invoke(B.page, 'join_play', { friend_public_key: keyA, instance_id: guest!.id });
    console.log('[mc2] join_play : tunnel invite sur 127.0.0.1:' + (await testState(B.page)).guest_tunnel_port);
    const okRe = /Invite.*(logged in|joined the game)/;
    const koRe = /Invalid session|Failed to verify username|Lost connection|Disconnecting|multiplayer\.disconnect|Failed to connect|Couldn't connect|Connection refused|Connection reset/i;
    const end = Date.now() + 420_000;
    let verdict = 'delai depasse';
    let stallSince: number | undefined;
    let stallDumped = false;
    while (Date.now() < end) {
      if (okRe.test(readLog(A.dataDir, ID))) { verdict = 'ENTRE dans le monde'; break; }
      if (koRe.test(readLog(B.dataDir, guest!.id))) { await sleep(5000); verdict = okRe.test(readLog(A.dataDir, ID)) ? 'ENTRE dans le monde' : 'REFUSE/DECONNECTE'; break; }
      if (B.proc.exitCode !== null) { verdict = 'app B arretee'; break; }
      if (!stallDumped && /Overlay is removed/.test(readLog(B.dataDir, guest!.id)) && !/Connecting to/.test(readLog(B.dataDir, guest!.id))) {
        stallSince ??= Date.now();
        if (Date.now() - stallSince > 60_000) { stallDumped = true; console.log('[mc2] DIAGNOSTIC quickPlay bloque depuis 60 s :\n' + stallDiag(root + path.sep + 'data-Invite')); }
      }
      await sleep(2000);
    }
    console.log('[mc2] verdict invite : ' + verdict);
    console.log('[mc2] sortie launcher B (filtree) : ' + B.log.join('').split(/\r?\n/).filter((l) => /tunnel|quickPlay|erreur|error|launch|premier plan/i.test(l)).slice(-15).join(' | '));
    console.log('[mc2] ligne de commande java de B : ' + (javaCmd(root + path.sep + 'data-Invite').match(/--quickPlayMultiplayer\s+\S+/) ?? ['(absent)'])[0]);
    const tail = (s: string) => s.split(/\r?\n/).slice(-25).join('\n');
    console.log('[mc2] fin de latest.log de B :\n' + tail(readLog(B.dataDir, guest!.id)));
    console.log('[mc2] fin de latest.log de A :\n' + tail(readLog(A.dataDir, ID)));
    const sel = (s: string) => s.split(/\r?\n/).filter((l) => /quickPlay|Setting user|Invalid session|Disconnect|Connecting to|Failed to|Authentication|multiplayer\.|Lost connection|joined the game|logged in|Online mode|verify/i.test(l)).join('\n');
    console.log('[mc2] latest.log de B :\n' + sel(readLog(B.dataDir, guest!.id)));
    console.log('[mc2] latest.log de A :\n' + sel(readLog(A.dataDir, ID)));
    expect(readLog(A.dataDir, ID)).toMatch(/TEST authentification serveur DESACTIVEE/);
    expect(verdict, 'l\'invite doit entrer dans le monde de A').toBe('ENTRE dans le monde');
    expect(verdict, "l'invite doit entrer dans le monde de l'hote").toBe('ENTRE dans le monde');
    expect(readLog(A.dataDir, ID)).toMatch(/TEST authentification serveur DESACTIVEE/);
  } finally {
    await a?.close();
    await b?.close();
    killProcsOfDir(root);
    fs.rmSync(root, { recursive: true, force: true, maxRetries: 10, retryDelay: 300 });
  }
});

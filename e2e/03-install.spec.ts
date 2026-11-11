// T3 : installation du monde d'un ami (B rejoint A) : progression, pause/reprise, annulation, coupure/reprise, empreintes.
import fs from 'node:fs';
import path from 'node:path';
import { test, expect, invoke, testState } from './harness/fixtures';
import {
  setupHostAndGuest, startJoinViaUi, sleep, recordEvents, events, waitProgress, lastProgress,
  suspendProcess, resumeProcess, guestInstance, sha256File, walk, type Progress,
} from './helpers/e2e2-common';

test.describe.configure({ mode: 'serial' });
test.setTimeout(240_000);

// Fichiers volumineux (5 x 24 Mo) : ~3 s de transfert en local ; A est gele (suspendu) pour avoir le temps d'agir.
const BIG = { count: 5, sizeMb: 24 };

async function setup(world: any) {
  const ctx = await setupHostAndGuest(world, BIG);
  await recordEvents(world.appB.page, 'transfer://progress');
  await startJoinViaUi(world.appB.page);
  return ctx;
}

async function expectGuestHasHostFiles(appB: any, host: { files: { path: string; sha256: string }[] }) {
  const g = await guestInstance(appB);
  expect(g, 'instance invitee creee chez B').not.toBeNull();
  for (const f of host.files) {
    const p = path.join(g!.gameDir, ...f.path.split('/'));
    expect(fs.existsSync(p), `${f.path} present chez B`).toBe(true);
    expect(sha256File(p), `sha256 de ${f.path}`).toBe(f.sha256);
  }
  expect(host.files).toHaveLength(5);
  return g!;
}

test('installation : progression (barre, fichiers, octets), pause, reprise, empreintes identiques', async ({ appA, appB, servers, world }) => {
  const { host } = await setup({ appA, appB, servers, binary: world.binary });
  const page = appB.page;
  const bar = page.getByRole('progressbar');
  await expect(bar).toBeVisible();
  // gele A des que les octets arrivent : le transfert reste en cours le temps des assertions
  await waitProgress(page, (p) => p.phase === 'receiving' && p.bytes_done > 0);
  suspendProcess(appA.proc.pid!);
  await expect(bar).toHaveAttribute('aria-valuetext', /\d+\s*%\s*·\s*\d+\s*\/\s*5\s*fichiers\s*·\s*[\d,.\s]+\S*\s*\/\s*[\d,.\s]+\s*Mo/);
  const now = Number(await bar.getAttribute('aria-valuenow'));
  expect(now).toBeGreaterThanOrEqual(0);
  expect(now).toBeLessThan(100);
  await expect(page.getByText('Réception des fichiers').first()).toBeVisible();

  // Pause (en plein transfert), A repart : rien ne doit avancer tant que c'est en pause
  await page.getByRole('button', { name: 'Pause' }).click();
  await expect(page.getByText('Transfert en pause').first()).toBeVisible();
  await expect(page.getByRole('button', { name: 'Reprendre' })).toBeVisible();
  await sleep(1000);
  const frozen = (await lastProgress(page)).bytes_done;
  resumeProcess(appA.proc.pid!);
  await sleep(3500);
  const still = await lastProgress(page);
  expect(still.bytes_done, 'aucun octet recu pendant la pause').toBe(frozen);
  expect(still.phase).toBe('paused');

  // Reprendre -> fin
  await page.getByRole('button', { name: 'Reprendre' }).click();
  await expect.poll(async () => ((await events(page, 'transfer://progress')) as any[]).some((p) => p.phase === 'done'), { timeout: 90000 }).toBe(true);
  const last = await lastProgress(page);
  expect(last.files_done).toBe(5);
  expect(last.bytes_done).toBe(host.total);
  await expectGuestHasHostFiles(appB, host);
  // hote intact
  for (const f of host.files) expect(sha256File(path.join(host.gameDir, ...f.path.split('/')))).toBe(f.sha256);
});

test("installation : annuler demande confirmation, ne laisse rien d'incomplet chez B, hote intact", async ({ appA, appB, servers, world }) => {
  const { host, keyA } = await setup({ appA, appB, servers, binary: world.binary });
  const page = appB.page;
  await waitProgress(page, (p) => p.phase === 'receiving' && p.bytes_done > 0);
  suspendProcess(appA.proc.pid!);

  await page.getByRole('button', { name: 'Annuler' }).click();
  const group = page.getByRole('group', { name: "Confirmer l'annulation" });
  await expect(group).toBeVisible();
  // Continuer : la confirmation disparait, le transfert reste affiche
  await group.getByRole('button', { name: "Continuer l'installation" }).click();
  await expect(group).toBeHidden();
  await expect(page.getByRole('progressbar')).toBeVisible();

  await page.getByRole('button', { name: 'Annuler' }).click();
  await group.getByRole('button', { name: 'Oui, annuler' }).click();
  await expect(page.locator('main[data-route="main"]')).toBeVisible({ timeout: 20_000 });
  resumeProcess(appA.proc.pid!);
  await sleep(2500);

  // B : aucune instance invitee, aucun partiel ni journal de reprise
  expect(await guestInstance(appB)).toBeNull();
  const st = await testState(page);
  expect(fs.existsSync(st.paths.instances) ? fs.readdirSync(st.paths.instances) : [], 'dossier des instances de B').toEqual([]);
  const partials = walk(appB.dataDir, (p) => /\.part$|\.resume\.part$|\.tmp$/.test(p) || p.includes(`${path.sep}.transfers${path.sep}`));
  expect(partials, 'partiels / journaux chez B').toEqual([]);

  // Hote intact : fichiers inchanges, instance toujours la, partage toujours servi (le plan fonctionne encore)
  for (const f of host.files) expect(sha256File(path.join(host.gameDir, ...f.path.split('/')))).toBe(f.sha256);
  expect((await invoke<{ id: string }[]>(appA.page, 'instances_list')).map((i) => i.id)).toContain(host.id);
  const plan = await invoke<{ manifest: { files: unknown[] } }>(page, 'join_plan', { friend_public_key: keyA });
  expect(plan.manifest.files).toHaveLength(5);
});

// Corrige (FIXX) : compteur monotone (high-water par fichier), fichiers en cache comptes des le depart ; la reprise intra-fichier reste impossible (iroh).
test('installation : coupure en cours de route puis reprise automatique sans retransfert', async ({ appA, appB, servers, world }) => {
  const { host } = await setup({ appA, appB, servers, binary: world.binary });
  const page = appB.page;
  // coupe des les premiers octets (le lancement de PowerShell prend ~1 s : on ne peut pas viser plus fin)
  const cut = await waitProgress(page, (p) => p.phase === 'receiving' && p.bytes_done > 0);
  suspendProcess(appA.proc.pid!);
  await sleep(1500);
  const atCut = await lastProgress(page);
  expect(atCut.bytes_done, 'coupe en plein transfert').toBeLessThan(atCut.bytes_total);
  // pendant la coupure : toujours en transfert (pas d'echec), la barre reste visible
  await sleep(18_000);
  const during = await lastProgress(page);
  expect(['receiving', 'verifying']).toContain(during.phase);
  expect(during.bytes_done).toBeGreaterThanOrEqual(atCut.bytes_done);
  await expect(page.getByRole('progressbar')).toBeVisible();
  resumeProcess(appA.proc.pid!);

  await expect.poll(async () => ((await events(page, 'transfer://progress')) as any[]).some((p) => p.phase === 'done'), { timeout: 150000 }).toBe(true);
  const all = await events<Progress & { error?: string }>(page, 'transfer://progress');
  expect(all.some((p) => p.phase === 'failed'), 'aucun echec visible').toBe(false);
  // reprise sans retransfert : octets et fichiers termines ne reculent jamais
  let maxBytes = 0, maxFiles = 0;
  for (const p of all.filter((e) => e.phase === 'receiving' || e.phase === 'verifying')) {
    expect(p.bytes_done, 'octets recus monotones').toBeGreaterThanOrEqual(maxBytes - 1);
    expect(p.files_done, 'fichiers termines monotones').toBeGreaterThanOrEqual(maxFiles);
    maxBytes = Math.max(maxBytes, p.bytes_done); maxFiles = Math.max(maxFiles, p.files_done);
  }
  expect(cut.bytes_total).toBe(host.total);
  expect(maxBytes).toBe(host.total);
  await expectGuestHasHostFiles(appB, host);
});

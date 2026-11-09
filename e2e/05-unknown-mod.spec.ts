// T5 : mod maison (inconnu de Modrinth) transfere de A vers B ; avertissement avant telechargement,
// confiance requise, re-verification locale apres reception, cas hostile (fichier altere cote hote).
import fs from 'node:fs';
import path from 'node:path';
import { test, expect, invoke, testState } from './harness/fixtures';
import { HOME_MOD, befriend, captureEvents, events, gotoHash, hostShare, sha256File } from './helpers/e2e3-common';

interface Plan { manifest: { name: string; files: { path: string; sha256: string; sha1?: string }[] }; cached: number; to_receive: number; unknown_mods: string[]; unknown_unchecked: string[] }

test('T5 : mod inconnu signale avant telechargement, confiance requise, sha256 identique, re-verification locale', async ({ appA, appB, servers, binary }) => {
  const { keyA } = await befriend({ appA, appB, servers, binary });
  const { host, world } = await hostShare(appA);
  expect(world.lanPort).toBeGreaterThan(0);
  await captureEvents(appB, ['transfer://unknown_mods', 'transfer://progress']);

  // Avant tout : rien chez B (pas d'instance).
  expect(await invoke<unknown[]>(appB.page, 'instances_list')).toHaveLength(0);

  // Le plan (commande) : seul le mod maison est inconnu, les 4 mods du catalogue ne le sont pas.
  const plan = await invoke<Plan>(appB.page, 'join_plan', { friend_public_key: keyA });
  expect(plan.unknown_mods).toEqual([HOME_MOD]);
  expect(plan.unknown_unchecked).toEqual([]);
  expect(plan.manifest.files.filter((f) => f.path.startsWith('mods/'))).toHaveLength(5);
  expect(plan.to_receive).toBe(plan.manifest.files.length);

  // L'ecran Rejoindre : avertissement, case de confiance requise.
  await gotoHash(appB, `#/join?friend=${keyA}`, 'join');
  const page = appB.page;
  await expect(page.getByRole('heading', { name: /Rejoindre « Monde de test »/ })).toBeVisible({ timeout: 30_000 });
  const warn = page.getByRole('note');
  await expect(warn.getByRole('heading', { name: "Un mod inconnu va s'exécuter sur ton ordinateur" })).toBeVisible();
  await expect(warn.getByText('homebrew-mod-0.0.1.jar')).toBeVisible();
  await expect(warn.getByText(/L'intégrité est vérifiée, pas la sécurité/)).toBeVisible();
  expect(await warn.locator('li').count()).toBe(1); // les 4 mods connus ne sont pas signales
  const join = page.getByRole('button', { name: 'Rejoindre' });
  await expect(join).toBeDisabled();
  await expect(page.getByText('Coche la case de confiance pour activer « Rejoindre ».')).toBeVisible();
  const trust = page.getByLabel('Je fais confiance à mon ami pour ce mod');
  await trust.check();
  await expect(join).toBeEnabled();
  await trust.uncheck();
  await expect(join).toBeDisabled();
  expect(await invoke<unknown[]>(appB.page, 'instances_list')).toHaveLength(0); // rien telecharge sans clic
  await trust.check();

  // Telechargement.
  await join.click();
  await expect(page.getByRole('heading', { name: 'Installation en cours' })).toBeVisible();
  await expect.poll(async () => (await events(appB, 'transfer://unknown_mods')).length, { timeout: 90_000 }).toBeGreaterThan(0);
  await expect(page.getByRole('button', { name: 'Jouer' })).toBeVisible({ timeout: 90_000 });

  // Re-verification locale : evenement transfer://unknown_mods.
  const ev = (await events(appB, 'transfer://unknown_mods')) as { instance_id: string; unknown_mods: string[] }[];
  expect(ev).toHaveLength(1);
  expect(ev[0].unknown_mods).toEqual([HOME_MOD]);

  // Instance invitee : sha256 identiques, jar dans le game_dir invite.
  const st = await testState(appB.page);
  const list = await invoke<{ id: string; name: string }[]>(appB.page, 'instances_list');
  const guest = list.find((i) => i.id === ev[0].instance_id);
  expect(guest?.name).toMatch(/\(invité\)$/);
  const meta = JSON.parse(fs.readFileSync(path.join(st.paths.instances, ev[0].instance_id, 'instance.json'), 'utf8'));
  expect(meta.kind).toBe('guest');
  const guestDir = path.join(st.paths.instances, ev[0].instance_id, 'minecraft');
  for (const f of plan.manifest.files) {
    const mine = path.join(guestDir, ...f.path.split('/'));
    expect(fs.existsSync(mine), f.path).toBe(true);
    expect(sha256File(mine), f.path).toBe(f.sha256);
    expect(sha256File(path.join(host.gameDir, ...f.path.split('/'))), f.path).toBe(f.sha256);
  }
  // Les fichiers exclus de l'hote ne sont pas copies.
  for (const ex of ['options.txt', 'servers.dat', 'saves']) expect(fs.existsSync(path.join(guestDir, ex)), ex).toBe(false);
});

test('T5 hostile : fichier altere cote hote (meme taille) -> Integrity, hors cache, ecran bad-hash', async ({ appA, appB, servers, binary }) => {
  const { keyA } = await befriend({ appA, appB, servers, binary });
  const { host } = await hostShare(appA);
  await captureEvents(appB, ['transfer://unknown_mods', 'transfer://progress']);
  const plan = await invoke<Plan>(appB.page, 'join_plan', { friend_public_key: keyA });

  // L'hote a annonce (manifeste fige au partage) un mod connu ; il remplace ensuite le jar par un autre de meme taille.
  const known = plan.manifest.files.find((f) => f.path.startsWith('mods/') && f.path !== HOME_MOD)!;
  const target = path.join(host.gameDir, ...known.path.split('/'));
  const evil = Buffer.from(fs.readFileSync(target));
  for (let i = evil.length - 64; i < evil.length; i++) evil[i] ^= 0xff; // altere sans changer la taille
  fs.writeFileSync(target, evil);
  const bad = sha256File(target);
  expect(bad).not.toBe(known.sha256);

  await gotoHash(appB, `#/join?friend=${keyA}`, 'join');
  await appB.page.getByLabel('Je fais confiance à mon ami pour ce mod').check({ timeout: 30_000 });
  await appB.page.getByRole('button', { name: 'Rejoindre' }).click();

  // Ecran Erreur bad-hash.
  await expect(appB.page.getByRole('heading', { name: /ne correspond pas à ce qu'il avait annoncé/ })).toBeVisible({ timeout: 90_000 });
  await expect(appB.page.getByText('Installation · échec')).toBeVisible();
  await expect(appB.page.getByText(/Cubelink l'a supprimé et ne l'a pas exécuté/)).toBeVisible();
  await expect(appB.page.getByRole('button', { name: 'Réessayer' })).toBeVisible();
  await expect(appB.page.getByRole('button', { name: 'Annuler' })).toBeVisible();
  const failed = (await events(appB, 'transfer://progress')).filter((p: any) => p.phase === 'failed');
  expect(failed.at(-1).error).toMatch(/fichier corrompu/);

  // Ni dans le cache ni dans une instance ; aucune instance invitee ; pas de re-verification.
  expect(await events(appB, 'transfer://unknown_mods')).toHaveLength(0);
  expect(await invoke<unknown[]>(appB.page, 'instances_list')).toHaveLength(0);
  const hits: string[] = [];
  const walk = (d: string) => {
    for (const e of fs.readdirSync(d, { withFileTypes: true })) {
      const p = path.join(d, e.name);
      if (e.isDirectory()) walk(p);
      else if (fs.statSync(p).size === evil.length && sha256File(p) === bad) hits.push(p);
    }
  };
  walk(appB.dataDir);
  expect(hits, 'contenu altere retrouve dans le dossier de donnees de B').toEqual([]);

  // Annuler ramene a l'accueil.
  await appB.page.getByRole('button', { name: 'Annuler' }).click();
  await appB.page.waitForSelector('main[data-route="main"]');
});

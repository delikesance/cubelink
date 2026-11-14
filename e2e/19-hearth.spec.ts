// E2E-19 : serveurs de groupe (Hearth) entre deux vraies applications (A cree, B est invite), sans lancer de serveur Minecraft.
// Couvre : creation depuis un monde de l'instance, invitation recue (evenement, file, boite de dialogue), acceptation,
// cartes de l'accueil, page du serveur, contrat Minecraft exige avant le premier hebergement.
// Le serveur dedie et le jeu ne sont pas lances ici (telechargement Mojang + Java) : voir src-tauri/tests/hearth_flow.rs
// pour le cycle complet (hote, rejoindre, passation, partie mal terminee, fourche) avec un faux serveur.
import fs from 'node:fs';
import path from 'node:path';
import { test, expect, invoke, waitForRoute } from './harness/fixtures';
import { setupPair, invokeErr, record, recorded, until } from './helpers/e2e18-invites';

test.setTimeout(240_000);

interface Card { id: string; name: string; state: string; members_total: number; is_creator: boolean; can_host: boolean }
interface Detail { card: Card; members: { name: string; is_me: boolean }[]; versions: { number: number; is_head: boolean }[] }
interface HInvite { group_id: string; name: string; from: { public_key: string; name: string }; members: number }

test('serveur de groupe : A le cree depuis un monde, B recoit l\'invitation et l\'accepte ; cartes et page du serveur ; contrat Minecraft exige', async ({ appA, appB }) => {
  const { host, keyA, keyB } = await setupPair({ appA, appB });
  // Hearth est coupé par défaut (docs/hearth.md § 12.1 n° 63) : refus clair, puis on l'active des deux côtés.
  expect(await invokeErr(appA.page, 'hearth_create', { name: 'X', instance_id: host.id, world: 'Monde Hearth', members: [keyB] })).toContain('désactivés');
  await invoke(appA.page, 'hearth_set_enabled', { enabled: true });
  await invoke(appB.page, 'hearth_set_enabled', { enabled: true });
  await record(appB.page, 'hearth://invites');
  await record(appA.page, 'hearth://state');

  // Un monde dans l'instance de A.
  const save = path.join(host.gameDir, 'saves', 'Monde Hearth');
  fs.mkdirSync(path.join(save, 'region'), { recursive: true });
  fs.writeFileSync(path.join(save, 'level.dat'), Buffer.from('niveau-hearth'));
  fs.writeFileSync(path.join(save, 'region', 'r.0.0.mca'), Buffer.alloc(8192, 7));

  const worlds = await invoke<{ name: string; size_bytes: number }[]>(appA.page, 'hearth_worlds', { instance_id: host.id });
  expect(worlds.map((w) => w.name)).toContain('Monde Hearth');

  // Refus : un inconnu, un nom vide.
  expect(await invokeErr(appA.page, 'hearth_create', { name: 'X', instance_id: host.id, world: 'Monde Hearth', members: ['00'.repeat(32)] })).toBeTruthy();
  expect(await invokeErr(appA.page, 'hearth_create', { name: '  ', instance_id: host.id, world: 'Monde Hearth', members: [keyB] })).toBeTruthy();
  expect(await invoke<Card[]>(appA.page, 'hearth_list')).toEqual([]);

  // Creation.
  const card = await invoke<Card>(appA.page, 'hearth_create', { name: 'Les copains', instance_id: host.id, world: 'Monde Hearth', members: [keyB] });
  expect(card).toMatchObject({ name: 'Les copains', is_creator: true, can_host: true, members_total: 2 });
  expect(card.state).toBe('idle');
  const detail = await invoke<Detail>(appA.page, 'hearth_detail', { id: card.id });
  expect(detail.members.map((m) => m.name).sort()).toHaveLength(2);
  expect(detail.versions).toHaveLength(1);
  expect(detail.versions[0]).toMatchObject({ number: 1, is_head: true });
  expect((await recorded<Card[]>(appA.page, 'hearth://state')).length).toBeGreaterThan(0);

  // B : invitation (evenement + file), puis boite de dialogue sur l'accueil.
  const inv = await until('invitation de groupe chez B', async () => (await invoke<HInvite[]>(appB.page, 'hearth_invites'))[0]);
  expect(inv).toMatchObject({ group_id: card.id, name: 'Les copains', members: 2 });
  expect(inv.from.public_key).toBe(keyA);
  expect((await recorded<HInvite[]>(appB.page, 'hearth://invites')).at(-1)).toHaveLength(1);
  await appB.page.reload();
  await waitForRoute(appB.page, 'main');
  const dialog = appB.page.getByRole('dialog', { name: 'Invitation à un serveur de groupe' });
  await expect(dialog).toBeVisible();
  await dialog.getByRole('button', { name: 'Plus tard' }).click();
  await expect(dialog).toBeHidden();
  expect(await invoke<HInvite[]>(appB.page, 'hearth_invites')).toHaveLength(1);
  await appB.page.reload();
  await waitForRoute(appB.page, 'main');
  await appB.page.getByRole('dialog', { name: 'Invitation à un serveur de groupe' }).getByRole('button', { name: 'Rejoindre le groupe' }).click();
  await expect(appB.page.getByRole('listitem', { name: 'Serveur Les copains' })).toBeVisible();
  expect(await invoke<HInvite[]>(appB.page, 'hearth_invites')).toEqual([]);

  // B n'a pas le modpack : la carte le dit, au lieu de proposer Jouer.
  const cardB = await until('carte du groupe chez B', async () => (await invoke<Card[]>(appB.page, 'hearth_list')).find((c) => c.id === card.id));
  expect(cardB).toMatchObject({ is_creator: false });

  // A : carte sur l'accueil et page du serveur.
  await appA.page.reload();
  await waitForRoute(appA.page, 'main');
  const cardA = appA.page.getByRole('listitem', { name: 'Serveur Les copains' });
  await expect(cardA).toBeVisible();
  await expect(cardA.getByText('Personne ne joue')).toBeVisible();
  await cardA.getByRole('button', { name: 'Page du serveur Les copains' }).click();
  await waitForRoute(appA.page, 'hearth');
  await expect(appA.page.getByRole('heading', { level: 1, name: 'Les copains' })).toBeVisible();
  await expect(appA.page.getByText('2 membres', { exact: true }).first()).toBeVisible();
  await appA.page.getByRole('tab', { name: 'Historique' }).click();
  await expect(appA.page.getByText(/Version 1 · Partie · actuelle/)).toBeVisible();
  await appA.page.getByRole('tab', { name: 'Réglages' }).click();
  await expect(appA.page.getByRole('checkbox', { name: /Garder le serveur ouvert/ })).not.toBeChecked();

  // Contrat Minecraft : exige avant le premier hebergement, jamais accepte automatiquement.
  expect((await invoke<{ accepted: boolean }>(appA.page, 'hearth_eula')).accepted).toBe(false);
  expect(await invokeErr(appA.page, 'hearth_play', { id: card.id })).toContain('eula_required');
  expect((await invoke<{ accepted: boolean }>(appA.page, 'hearth_eula')).accepted).toBe(false);
  expect((await invoke<Card[]>(appA.page, 'hearth_list'))[0]!.state).toBe('idle');
});

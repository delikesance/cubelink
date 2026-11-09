// T2 : B (ami accepte) ouvre « Rejoindre » depuis l'Accueil ; plan, mod inconnu, case de confiance, annulation.
import { test, expect, invoke, makeHostInstance, startFakeWorld, testState } from './harness/fixtures';
import { NAME_A, closeOpened, countFiles, gotoRoute, keyOfCode, loginBoth, makeFriends, readMyCode } from './helpers/e2e1-common';

const HOME_MOD = 'mods/homebrew-mod-0.0.1.jar';
test.afterEach(closeOpened);

test('T2 : plan affiche, mod maison inconnu, confiance requise, non-ami refuse, annuler ne telecharge rien', async ({ world }) => {
  const { appA, appB } = world;
  await loginBoth(world);

  // A possede l'instance hote (4 mods catalogue + 1 mod maison) et la partage.
  const host = await makeHostInstance(appA);
  expect(host.manifest.files.filter((f) => f.path.startsWith('mods/'))).toHaveLength(5);
  const fake = await startFakeWorld(appA);
  await invoke(appA.page, 'host_start_sharing', { instance_id: host.id, world_port: fake.lanPort });
  const keyA = keyOfCode(await readMyCode(appA));

  // Non-ami : B ne peut pas obtenir le plan (commande et ecran).
  await expect(invoke(appB.page, 'join_plan', { friend_public_key: keyA })).rejects.toThrow(/inconnu/);
  await gotoRoute(appB, `join?friend=${keyA}`);
  await expect(appB.page.getByRole('heading', { name: "Impossible de rejoindre pour l'instant" })).toBeVisible({ timeout: 20_000 });
  await expect(appB.page.getByRole('button', { name: 'Rejoindre', exact: true })).toHaveCount(0);
  await appB.page.getByRole('button', { name: 'Annuler' }).click();
  await appB.page.waitForSelector('main[data-route="main"]');

  // Amitie, puis presence en ligne cote B.
  await makeFriends(appA, appB);
  await expect.poll(async () => (await invoke<{ presence: string }[]>(appB.page, 'friend_list'))[0]?.presence, { timeout: 60_000 }).toBe('online');

  // Etat du disque de B avant : aucune instance, rien dans le magasin.
  const st = await testState(appB.page);
  const before = { instances: countFiles(st.paths.instances), store: countFiles(st.paths.store) };
  expect(await invoke<unknown[]>(appB.page, 'instances_list')).toHaveLength(0);

  // B : Accueil (rechargee pour lire la presence) puis « Rejoindre Alice ».
  await gotoRoute(appB, 'friends');
  await gotoRoute(appB, 'main');
  await appB.page.getByRole('button', { name: `Rejoindre ${NAME_A}` }).click();
  await appB.page.waitForSelector('main[data-route="join"]');
  await expect(appB.page.getByRole('heading', { name: /Rejoindre « Monde de test »/ })).toBeVisible({ timeout: 30_000 });

  // Le plan : version, loader, fichiers a recevoir.
  const dialog = appB.page.getByRole('region', { name: 'Rejoindre un ami' });
  await expect(dialog.getByText('1.21.1-test')).toBeVisible();
  await expect(dialog.getByText(/vanilla/i)).toBeVisible();
  const n = host.manifest.files.length;
  await expect(dialog.getByText(new RegExp(n + '[\\s\\u00a0]+fichiers')).first()).toBeVisible();
  await dialog.getByText(`Liste des fichiers (${n})`).click();
  for (const f of host.manifest.files) await expect(dialog.getByRole("group").getByText(f.path, { exact: true })).toBeVisible();

  // Mod maison inconnu : signale, bouton desactive tant que la case n'est pas cochee.
  const warn = dialog.getByRole('note');
  await expect(warn.getByRole('heading', { name: "Un mod inconnu va s'exécuter sur ton ordinateur" })).toBeVisible();
  await expect(warn.getByText(HOME_MOD, { exact: true })).toBeVisible();
  const join = dialog.getByRole('button', { name: 'Rejoindre', exact: true });
  await expect(join).toBeDisabled();
  const trust = dialog.getByRole('checkbox', { name: /Je fais confiance/ });
  await expect(trust).not.toBeChecked();
  await trust.check();
  await expect(join).toBeEnabled();
  await trust.uncheck();
  await expect(join).toBeDisabled();

  // Annuler : retour a l'Accueil, rien telecharge ni installe.
  await dialog.getByRole('button', { name: 'Annuler' }).click();
  await appB.page.waitForSelector('main[data-route="main"]');
  await new Promise((r) => setTimeout(r, 2000));
  expect(await invoke<unknown[]>(appB.page, 'instances_list')).toHaveLength(0);
  expect(countFiles(st.paths.instances)).toBe(before.instances);
  expect(countFiles(st.paths.store)).toBe(before.store);
});

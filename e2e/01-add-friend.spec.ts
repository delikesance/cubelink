// T1 : ajout d'ami entre deux instances reelles (A et B), de la demande a l'acceptation.
import { test, expect, invoke } from './harness/fixtures';
import { NAME_A, NAME_B, closeOpened, gotoRoute, loginBoth, readMyCode, sendRequest } from './helpers/e2e1-common';

test.afterEach(closeOpened);

test('T1 : B ajoute A, codes de verification identiques, A accepte, presence en ligne', async ({ world }) => {
  const { appA, appB } = world;
  await loginBoth(world);
  const codeA = await readMyCode(appA);

  // B colle le code de A et envoie la demande.
  await sendRequest(appB, codeA);
  const verifyB = appB.page.getByLabel(/^Code de vérification : /);
  await expect(verifyB).toBeVisible({ timeout: 30_000 });
  const codeB = ((await verifyB.textContent()) ?? '').trim();
  expect(codeB).not.toBe('');

  // A voit le badge (Accueil), puis la demande dans l'onglet Demandes.
  await gotoRoute(appA, 'main');
  await expect(appA.page.getByRole('button', { name: /^Amis \(1 demande\)/ })).toBeVisible({ timeout: 30_000 });
  await gotoRoute(appA, 'friends');
  await expect(appA.page.getByRole('tab', { name: /Demandes/ }).getByLabel('1 demande en attente')).toBeVisible({ timeout: 30_000 });
  await appA.page.getByRole('tab', { name: /Demandes/ }).click();
  await expect(appA.page.getByRole('heading', { name: `Demande de ${NAME_B}` })).toBeVisible();
  const verifyA = appA.page.getByLabel('Code de vérification', { exact: true });
  await expect(verifyA).toBeVisible();
  expect(((await verifyA.textContent()) ?? '').trim()).toBe(codeB);

  // A accepte : Accepter reste desactive tant que le code n'est pas coche comme compare.
  await expect(appA.page.getByRole('button', { name: `Accepter ${NAME_B}` })).toBeDisabled();
  await appA.page.getByLabel(`J'ai comparé ce code avec ${NAME_B}`).check();
  await appA.page.getByRole('button', { name: `Accepter ${NAME_B}` }).click();
  await expect(appA.page.getByText(`${NAME_B} est maintenant ton ami.`)).toBeVisible();

  // La presence est etablie cote backend des deux cotes.
  for (const app of [appA, appB]) {
    await expect.poll(async () => (await invoke<{ presence: string }[]>(app.page, 'friend_list'))[0]?.presence, { timeout: 60_000 }).toBe('online');
  }

  // Chacun voit l'autre dans sa liste d'amis (Accueil) avec la presence en ligne.
  for (const [app, other] of [[appA, NAME_B], [appB, NAME_A]] as const) {
    await gotoRoute(app, 'main');
    await app.page.getByRole('button', { name: /^Amis/ }).click();
    const friends = app.page.getByRole('dialog', { name: 'Amis · 1' });
    await expect(friends.getByText(other).first()).toBeVisible({ timeout: 20_000 });
    await expect(friends.getByRole('list', { name: 'Amis' })).toBeVisible();
    await expect(friends.getByText('En ligne')).toBeVisible({ timeout: 30_000 });
  }
  const la = await invoke<{ friend: { name: string }; presence: string }[]>(appA.page, 'friend_list');
  expect(la[0].friend.name).toBe(NAME_B);
  expect(la[0].presence).toBe('online');
});

test('T1 negatif : code invalide, soi-meme, refus', async ({ world }) => {
  const { appA, appB } = world;
  await loginBoth(world);
  const codeA = await readMyCode(appA);

  // Code invalide : erreur de validation, aucune demande chez A.
  await gotoRoute(appB, 'friends');
  const input = appB.page.getByLabel('Code ami de ton ami');
  await input.fill('pas-un-code');
  await appB.page.getByRole('button', { name: 'Envoyer la demande' }).click();
  await expect(appB.page.getByText(/Ce code n'est pas reconnu/)).toBeVisible();
  await expect(appB.page.getByLabel(/^Code de vérification : /)).toHaveCount(0);
  await input.fill('mcl1-court');
  await appB.page.getByRole('button', { name: 'Envoyer la demande' }).click();
  await expect(appB.page.getByText(/Ce code n'est pas reconnu/)).toBeVisible();
  expect((await invoke<{ total: number }>(appA.page, 'friend_requests', { offset: 0, limit: 5 })).total).toBe(0);

  // Demande a soi-meme : refusee, aucune demande ni ami cree.
  await gotoRoute(appA, 'friends');
  await appA.page.getByLabel('Code ami de ton ami').fill(codeA);
  await appA.page.getByRole('button', { name: 'Envoyer la demande' }).click();
  await expect(appA.page.getByRole('alert').filter({ hasText: /\S/ })).toBeVisible({ timeout: 20_000 });
  await expect(appA.page.getByLabel(/^Code de vérification : /)).toHaveCount(0);
  expect((await invoke<{ total: number }>(appA.page, 'friend_requests', { offset: 0, limit: 5 })).total).toBe(0);
  expect((await invoke<unknown[]>(appA.page, 'friend_list')).length).toBe(0);

  // B envoie une demande valide, A refuse : pas d'ami des deux cotes.
  await sendRequest(appB, codeA);
  await expect(appB.page.getByLabel(/^Code de vérification : /)).toBeVisible({ timeout: 30_000 });
  await gotoRoute(appA, 'friends');
  await appA.page.getByRole('tab', { name: /Demandes/ }).click();
  await appA.page.getByRole('button', { name: `Refuser ${NAME_B}` }).click({ timeout: 30_000 });
  await expect(appA.page.getByText(`Demande de ${NAME_B} refusée.`)).toBeVisible();
  await expect(appA.page.getByText('Aucune demande en attente.')).toBeVisible();
  await new Promise((r) => setTimeout(r, 1500));
  expect((await invoke<unknown[]>(appA.page, 'friend_list')).length).toBe(0);
  expect((await invoke<unknown[]>(appB.page, 'friend_list')).length).toBe(0);
  await gotoRoute(appA, 'main');
  await appA.page.getByRole('button', { name: /^Amis/ }).click();
  await expect(appA.page.getByRole('dialog', { name: 'Amis · 0' })).toBeVisible();
});

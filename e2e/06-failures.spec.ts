// T6 : echecs. Hote hors ligne, coupure puis retour, fichier corrompu, disque plein (non simulable),
// jeton Microsoft refuse / code expire / sans licence ; messages en francais et actions (Reessayer, Annuler).
import fs from 'node:fs';
import path from 'node:path';
import { test, expect, invoke, testState } from './harness/fixtures';
import {
  befriend, captureEvents, events, gotoHash, hostShare, killApp, resumeApp, sha256File, sleep, suspendApp,
} from './helpers/e2e3-common';

const control = (base: string, body: unknown) =>
  fetch(`${base}/__control`, { method: 'POST', headers: { 'content-type': 'application/json' }, body: JSON.stringify(body) });

// ---------------------------------------------------------------- hote hors ligne
test('T6 hote hors ligne pendant le join : message francais, Reessayer, Annuler', async ({ appA, appB, servers, binary }) => {
  const { keyA } = await befriend({ appA, appB, servers, binary });
  await hostShare(appA);
  const page = appB.page;

  // Plan charge (hote en ligne), puis l'hote disparait avant le clic « Rejoindre ».
  await gotoHash(appB, `#/join?friend=${keyA}`, 'join');
  await expect(page.getByRole('heading', { name: /Rejoindre « Monde de test »/ })).toBeVisible({ timeout: 30_000 });
  await page.getByLabel('Je fais confiance à mon ami pour ce mod').check();
  killApp(appA);
  await page.getByRole('button', { name: 'Rejoindre' }).click();
  // join_start rejoue la demande de manifeste : echec affiche sur l'ecran, aucun transfert demarre.
  const startErr = page.getByRole('alert').filter({ hasText: /\S/ });
  await expect(startErr).toBeVisible({ timeout: 60_000 });
  await expect(page.locator('main[data-route="join"]')).toBeVisible();
  expect(await invoke<unknown[]>(page, 'instances_list')).toHaveLength(0);

  // Nouvelle ouverture de l'ecran : erreur de plan, en francais, avec Reessayer / Annuler.
  await gotoHash(appB, `#/main`, 'main');
  await gotoHash(appB, `#/join?friend=${keyA}`, 'join');
  const title = page.getByRole('heading', { name: "Impossible de rejoindre pour l'instant" });
  await expect(title).toBeVisible({ timeout: 60_000 });
  await expect(page.getByText("Vérifie que ton ami est en ligne avec Cubelink ouvert, puis réessaie.")).toBeVisible();
  const retry = page.getByRole('button', { name: 'Réessayer' });
  await retry.click(); // hote toujours absent : l'erreur revient
  await expect(title).toBeVisible({ timeout: 60_000 });
  await page.getByRole('button', { name: 'Annuler' }).click();
  await page.waitForSelector('main[data-route="main"]');
});

test('T6 hote injoignable puis de retour : Reessayer recharge le plan', async ({ appA, appB, servers, binary }) => {
  test.setTimeout(240_000);
  const { keyA } = await befriend({ appA, appB, servers, binary });
  await hostShare(appA);
  const page = appB.page;
  // Coupure : le processus de l'hote est gele (injoignable, adresse conservee) ; un redemarrage changerait son
  // port et, sans relais ni decouverte d'adresses (mode test), B ne le retrouverait plus.
  suspendApp(appA);
  await gotoHash(appB, `#/join?friend=${keyA}`, 'join');
  await expect(page.getByRole('heading', { name: "Impossible de rejoindre pour l'instant" })).toBeVisible({ timeout: 90_000 });
  resumeApp(appA);
  await expect(async () => {
    await page.getByRole('button', { name: 'Réessayer' }).click({ timeout: 2000 });
    await expect(page.getByRole('heading', { name: /Rejoindre « Monde de test »/ })).toBeVisible({ timeout: 45_000 });
  }).toPass({ timeout: 120_000 });
});

// ---------------------------------------------------------------- coupure au milieu puis retour
test('T6 coupure au milieu du transfert puis retour de l hote : reprise, sha256 identiques', async ({ appA, appB, servers, binary }) => {
  test.setTimeout(300_000);
  const { keyA } = await befriend({ appA, appB, servers, binary });
  const { host } = await hostShare(appA, { bigMb: 300 });
  await captureEvents(appB, ['transfer://progress', 'transfer://unknown_mods']);
  const page = appB.page;
  await gotoHash(appB, `#/join?friend=${keyA}`, 'join');
  await page.getByLabel('Je fais confiance à mon ami pour ce mod').check({ timeout: 30_000 });
  await page.getByRole('button', { name: 'Rejoindre' }).click();
  await expect(page.getByRole('heading', { name: 'Installation en cours' })).toBeVisible();

  // Des octets sont arrives mais pas tout : on gele l'hote.
  await expect.poll(async () => {
    const p = ((await events(appB, 'transfer://progress')) as any[]).filter((e) => e.phase === 'receiving');
    return p.length ? p.at(-1).bytes_done : 0;
  }, { timeout: 60_000, intervals: [30] }).toBeGreaterThan(0);
  suspendApp(appA);
  const cut = ((await events(appB, 'transfer://progress')) as any[]).filter((e) => e.phase === 'receiving').at(-1);
  expect(cut.bytes_done, 'coupure avant la fin').toBeLessThan(cut.bytes_total);
  await sleep(8000); // pas d'echec immediat : B retente
  expect(((await events(appB, 'transfer://progress')) as any[]).some((e) => e.phase === 'failed')).toBe(false);
  await expect(page.getByRole('heading', { name: 'Installation en cours' })).toBeVisible();

  // L'hote revient : le transfert reprend (reprise automatique) jusqu'au bout.
  resumeApp(appA);
  await expect(page.getByRole('button', { name: 'Jouer' })).toBeVisible({ timeout: 180_000 });
  const ev = (await events(appB, 'transfer://unknown_mods')) as { instance_id: string }[];
  expect(ev).toHaveLength(1);
  const st = await testState(appB.page);
  const guestDir = path.join(st.paths.instances, ev[0].instance_id, 'minecraft');
  for (const rel of ['config/big.bin', 'mods/homebrew-mod-0.0.1.jar', 'config/cubelink-test.toml']) {
    expect(sha256File(path.join(guestDir, rel)), rel).toBe(sha256File(path.join(host.gameDir, rel)));
  }
});

test('T6 coupure prolongee : ecran friend-left, Annuler ramene a l accueil', async ({ appA, appB, servers, binary }) => {
  test.skip(!process.env.E2E_SLOW, 'lent (~6,5 min : les 8 reprises de B s epuisent) : E2E_SLOW=1 pour l executer');
  test.setTimeout(600_000);
  const { keyA } = await befriend({ appA, appB, servers, binary });
  await hostShare(appA, { bigMb: 300 });
  await captureEvents(appB, ['transfer://progress']);
  const page = appB.page;
  await gotoHash(appB, `#/join?friend=${keyA}`, 'join');
  await page.getByLabel('Je fais confiance à mon ami pour ce mod').check({ timeout: 30_000 });
  await page.getByRole('button', { name: 'Rejoindre' }).click();
  await expect.poll(async () => {
    const p = ((await events(appB, 'transfer://progress')) as any[]).filter((e) => e.phase === 'receiving');
    return p.length ? p.at(-1).bytes_done : 0;
  }, { timeout: 60_000, intervals: [30] }).toBeGreaterThan(0);
  killApp(appA); // l'hote ne revient pas : les tentatives de reprise s'epuisent
  await expect(page.getByRole('heading', { name: "Alex s'est déconnecté" })).toBeVisible({ timeout: 540_000 });
  await expect(page.getByText('Installation · en pause')).toBeVisible();
  await expect(page.getByText(/La réception reprend dès qu'Alex revient en ligne/)).toBeVisible();
  const failed = ((await events(appB, 'transfer://progress')) as any[]).filter((p) => p.phase === 'failed');
  expect(failed.at(-1).error).toBe("l'ami s'est déconnecté : réessayez quand il est en ligne");
  await page.getByRole('button', { name: 'Annuler' }).click();
  await page.waitForSelector('main[data-route="main"]');
});

// ---------------------------------------------------------------- fichier corrompu : Reessayer / Annuler
test('T6 fichier corrompu cote hote (config) : ecran bad-hash, Reessayer et Annuler', async ({ appA, appB, servers, binary }) => {
  const { keyA } = await befriend({ appA, appB, servers, binary });
  const { host } = await hostShare(appA);
  await captureEvents(appB, ['transfer://progress']);
  const target = path.join(host.gameDir, 'config', 'cubelink-test.toml');
  const evil = Buffer.from(fs.readFileSync(target));
  evil[0] ^= 0x01; // meme taille, contenu different
  fs.writeFileSync(target, evil);

  const page = appB.page;
  await gotoHash(appB, `#/join?friend=${keyA}`, 'join');
  await page.getByLabel('Je fais confiance à mon ami pour ce mod').check({ timeout: 30_000 });
  await page.getByRole('button', { name: 'Rejoindre' }).click();
  await expect(page.getByRole('heading', { name: /ne correspond pas à ce qu'il avait annoncé/ })).toBeVisible({ timeout: 90_000 });
  const failed = ((await events(appB, 'transfer://progress')) as any[]).filter((p) => p.phase === 'failed');
  expect(failed.at(-1).error).toBe("fichier corrompu : le contenu reçu ne correspond pas à l'empreinte annoncée");
  expect(await invoke<unknown[]>(page, 'instances_list')).toHaveLength(0);

  // Reessayer : ramene a l'ecran d'installation (le design ne prevoit pas plus).
  await page.getByRole('button', { name: 'Réessayer' }).click();
  await page.waitForSelector('main[data-route="install"]');
  await gotoHash(appB, '#/main', 'main');
});

// Corrige (FIXX) : le cache `known` de l'hote est invalide si le fichier change (taille + mtime).
test('T6 Reessayer (bad-hash / friend-left) relance reellement le transfert', async ({ appA, appB, servers, binary }) => {
  const { keyA } = await befriend({ appA, appB, servers, binary });
  const { host } = await hostShare(appA);
  const target = path.join(host.gameDir, 'config', 'cubelink-test.toml');
  const good = fs.readFileSync(target);
  const evil = Buffer.from(good); evil[0] ^= 1; fs.writeFileSync(target, evil);
  const page = appB.page;
  await gotoHash(appB, `#/join?friend=${keyA}`, 'join');
  await page.getByLabel('Je fais confiance à mon ami pour ce mod').check({ timeout: 30_000 });
  await page.getByRole('button', { name: 'Rejoindre' }).click();
  await expect(page.getByRole('heading', { name: /ne correspond pas/ })).toBeVisible({ timeout: 90_000 });
  fs.writeFileSync(target, good); // l'hote repare, (manifeste fige : il faut aussi repartager)
  await invoke(appA.page, 'host_start_sharing', { instance_id: host.id, world_port: 25590 });
  await page.getByRole('button', { name: 'Réessayer' }).click();
  await expect(page.getByRole('button', { name: 'Jouer' })).toBeVisible({ timeout: 60_000 });
});

// ---------------------------------------------------------------- disque plein
test.fixme('T6 disque plein reel : l\'ecran disk-full s\'affiche', async () => {
  // Non simulable de facon fiable ici : aucun crochet de test (CUBELINK_* / test_state) n'injecte ENOSPC ; saturer le
  // disque reel (ou un VHD/quota) est destructif et non portable. Le message du coeur (failure_message : « disque plein : ... »)
  // et le mapping classifyFailure -> disk-full sont couverts par les tests unitaires (commands_join.rs, format.test.ts) ;
  // le rendu de l'ecran est verifie ci-dessous (« ecrans Erreur »).
});

// ---------------------------------------------------------------- authentification Microsoft
// `screen` : l'echec est route vers l'ecran Erreur (classifyLoginFailure) ; sinon message en ligne avec Reessayer.
const AUTH_CASES = [
  { scenario: 'declined', message: /connexion refusée par l'utilisateur/, screen: { title: 'La connexion a été annulée', retry: 'Relancer la connexion' } },
  { scenario: 'expired_code', message: /code d'appareil expiré/ },
  { scenario: 'no_license', message: /ce compte ne possède pas Minecraft Java/, screen: { title: 'Ce compte Microsoft ne possède pas Minecraft : Java Edition', retry: 'Réessayer' } },
  { scenario: 'token_expired_then_refresh', message: /jeton refusé/ },
] as const;

test.describe('authentification', () => {
// Sans identifiant d'application Azure, l'app refuse la connexion (« identifiant d'application manquant ») : on en fournit un faux.
test.use({ appOptions: { env: { CUBELINK_MS_CLIENT_ID: '00000000-0000-0000-0000-00000000e2e3' } } });

for (const c of AUTH_CASES) {
  test(`T6 connexion Microsoft « ${c.scenario} » : message francais, Reessayer puis succes`, async ({ appA, servers }) => {
    const page = appA.page;
    await page.waitForSelector('main[data-route="login"]');
    await control(servers.auth.base, { scenario: c.scenario });
    await page.getByRole('button', { name: 'Se connecter avec Microsoft' }).click();
    if ('screen' in c) {
      await expect(page.getByRole('heading', { name: c.screen.title })).toBeVisible({ timeout: 30_000 });
      expect(await invoke(page, 'auth_status')).toBeNull();
      // Reessayer ramene a l'ecran de connexion ; on relance puis la connexion aboutit.
      await control(servers.auth.base, { scenario: 'ok' });
      await page.getByRole('button', { name: c.screen.retry }).click();
      await page.waitForSelector('main[data-route="login"]');
      await page.getByRole('button', { name: 'Se connecter avec Microsoft' }).click();
    } else {
      const alert = page.getByRole('alert').filter({ hasText: c.message });
      await expect(alert).toBeVisible({ timeout: 30_000 });
      expect(await invoke(page, 'auth_status')).toBeNull();
      // Reessayer : meme ecran, nouveau code, et la connexion aboutit quand le serveur est retabli.
      await control(servers.auth.base, { scenario: 'ok' });
      await page.getByRole('button', { name: 'Réessayer' }).click();
    }
    await page.waitForSelector('main[data-route="main"]', { timeout: 30_000 });
    expect(((await invoke<any>(page, 'auth_status')) ?? {}).name).toBe('TestPlayer');
  });
}

test('T6 echec de connexion Microsoft : l\'ecran Erreur (cancelled / no-license / offline) est affiche', async ({ appA, servers }) => {
  const page = appA.page;
  await page.waitForSelector('main[data-route="login"]');
  await control(servers.auth.base, { scenario: 'no_license' });
  await page.getByRole('button', { name: 'Se connecter avec Microsoft' }).click();
  await expect(page.getByRole('heading', { name: 'Ce compte Microsoft ne possède pas Minecraft : Java Edition' })).toBeVisible({ timeout: 30_000 });
});

});

// ---------------------------------------------------------------- ecrans Erreur (textes et actions)
const ERR = [
  { state: 'no-license', kicker: 'Connexion Microsoft · échec', title: 'Ce compte Microsoft ne possède pas Minecraft : Java Edition', actions: { 'Réessayer': 'login' }, link: 'Obtenir Minecraft' },
  { state: 'offline', kicker: 'Connexion Microsoft · échec', title: "Cubelink n'arrive pas à joindre Microsoft", actions: { 'Réessayer': 'login', 'Annuler': 'login' } },
  { state: 'cancelled', kicker: 'Connexion Microsoft · annulée', title: 'La connexion a été annulée', actions: { 'Relancer la connexion': 'login' } },
  { state: 'bad-hash', kicker: 'Installation · échec', title: "Un fichier reçu ne correspond pas à ce que ton ami avait annoncé", actions: { 'Réessayer': 'main', 'Annuler': 'main' } },
  { state: 'friend-left', kicker: 'Installation · en pause', title: "Ton ami s'est déconnecté", actions: { 'Réessayer': 'main', 'Annuler': 'main' } },
  { state: 'disk-full', kicker: 'Installation · échec', title: 'Espace disque insuffisant', actions: { 'Réessayer': 'main', 'Annuler': 'main' } },
  { state: 'java-missing', kicker: 'Installation · échec', title: 'Java 21 est introuvable sur cet ordinateur', actions: { 'Installer Java 21': 'install', 'Annuler': 'main' } },
] as const;

test('T6 ecrans Erreur : textes francais et actions de chaque etat', async ({ appA }) => {
  const page = appA.page;
  await page.waitForSelector('main[data-route="login"]');
  for (const e of ERR) {
    for (const [label, route] of Object.entries(e.actions)) {
      await gotoHash(appA, `#/error?state=${e.state}`, 'error');
      await expect(page.getByRole('heading', { name: e.title }), e.state).toBeVisible();
      await expect(page.getByText(e.kicker, { exact: true }), e.state).toBeVisible();
      await expect(page.getByRole('alert'), e.state).toBeVisible();
      if ('link' in e) await expect(page.getByRole('link', { name: e.link })).toHaveAttribute('href', 'https://www.minecraft.net/');
      await page.getByRole('button', { name: label, exact: true }).click();
      await page.waitForSelector(`main[data-route="${route}"]`);
    }
  }
  // Etat « waiting » : Annuler -> connexion ; Rouvrir la fenetre relance auth_begin_login.
  await gotoHash(appA, '#/error?state=waiting', 'error');
  await expect(page.getByRole('status').getByRole('heading', { name: 'Ouverture de Microsoft dans ton navigateur…' })).toBeVisible();
  await page.getByRole('button', { name: 'Annuler' }).click();
  await page.waitForSelector('main[data-route="login"]');
  // Etat inconnu : retombe sur « cancelled » (non alarmant).
  await gotoHash(appA, '#/error?state=n-importe-quoi', 'error');
  await expect(page.getByRole('heading', { name: 'La connexion a été annulée' })).toBeVisible();
  await sleep(0);
});

test("T6 les ecrans Erreur affichent le nom de l'ami et le fichier fournis (libelles neutres sinon)", async ({ appA }) => {
  await appA.page.waitForSelector('main[data-route="login"]');
  await gotoHash(appA, '#/error?state=friend-left&name=Alice', 'error');
  await expect(appA.page.getByRole('heading', { name: "Alice s'est déconnecté" })).toBeVisible();
  await gotoHash(appA, '#/error?state=bad-hash&name=Alice&file=mods%2Fa.jar', 'error');
  await expect(appA.page.getByRole('heading', { name: /Un fichier reçu de Alice ne correspond pas/ })).toBeVisible();
  await expect(appA.page.getByText('mods/a.jar')).toBeVisible();
});

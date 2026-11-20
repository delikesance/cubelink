// « Vider le cache » (#115) sur la VRAIE application : les commandes cache_size / cache_clear lisent le vrai disque.
// Profil neuf = cache vide : on verifie la commande, l'ecran Stockage (« Rien a liberer », bouton desactive) et
// l'absence de l'ancien message « pas encore disponible ». Le cas rempli (objets lies / non lies, octets exacts)
// est prouve sur disque par les tests Rust (`core/src/store/purge.rs`, `commands_settings.rs`).
import { test, expect } from './harness/fixtures';
import { SCREENS, prepare, gotoScreen } from './helpers/e2e4-lib';

const settings = SCREENS.find((s) => s.id === 'settings')!;

test.describe('#115 cache de mods', () => {
  test('profil neuf : taille 0, vidage 0, Stockage affiche « Rien a liberer »', async ({ appA }) => {
    const page = appA.page;
    await prepare(appA);
    const inv = (cmd: string) => page.evaluate((c) => (window as any).__TAURI_INTERNALS__.invoke(c, {}), cmd);
    expect(await inv('cache_size')).toBe(0);
    expect(await inv('cache_clear')).toBe(0);

    await gotoScreen(page, settings);
    await page.getByRole('tab', { name: 'Stockage' }).click();
    await expect(page.getByText(/Rien à libérer/)).toBeVisible();
    await expect(page.getByRole('button', { name: /Vider le cache…/ })).toBeDisabled();
    await expect(page.getByText(/pas encore disponible/)).toHaveCount(0);
  });
});

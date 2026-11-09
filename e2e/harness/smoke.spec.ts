import { test, expect, testState, waitForRoute } from './fixtures';

test('smoke : A et B demarrent sur la connexion, test_state repond', async ({ appA, appB, servers }) => {
  for (const app of [appA, appB]) {
    await waitForRoute(app.page, 'login');
    await expect(app.page.locator('main[data-route="login"]')).toBeVisible();
    const st = await testState(app.page);
    expect(st.test_mode).toBe(true);
    expect(st.data_dir.toLowerCase()).toBe(app.dataDir.toLowerCase());
    expect(st.relay_mode).toBeTruthy();
    expect(st.skip_java_probe).toBe(true);
    expect(st.url_overrides.CUBELINK_AUTH_MS_URL).toBe(servers.auth.base);
  }
  expect((await testState(appA.page)).data_dir).not.toBe((await testState(appB.page)).data_dir);
});

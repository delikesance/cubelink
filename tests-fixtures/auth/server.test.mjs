import test, { before, after } from 'node:test';
import assert from 'node:assert/strict';
import { spawn } from 'node:child_process';
import { fileURLToPath } from 'node:url';

let proc, base;
before(async () => {
  proc = spawn(process.execPath, [fileURLToPath(new URL('./server.mjs', import.meta.url)), '--port', '0'], { stdio: ['ignore', 'pipe', 'inherit'] });
  const line = await new Promise((resolve) => proc.stdout.once('data', (d) => resolve(String(d).split('\n')[0])));
  const r = JSON.parse(line);
  assert.equal(r.ready, true);
  base = r.base;
  assert.match(base, /^http:\/\/127\.0\.0\.1:\d+$/);
});
after(() => proc.kill());

const call = async (method, path, body, { json = true, headers = {} } = {}) => {
  const res = await fetch(base + path, {
    method, headers: { ...(body ? { 'Content-Type': json ? 'application/json' : 'application/x-www-form-urlencoded' } : {}), ...headers },
    body: body ? (json ? JSON.stringify(body) : new URLSearchParams(body).toString()) : undefined,
  });
  return { status: res.status, body: await res.json() };
};
const scenario = (s) => call('POST', '/__control', { scenario: s });
const poll = () => call('POST', '/token', { grant_type: 'urn:ietf:params:oauth:grant-type:device_code', client_id: 'x', device_code: 'fake-device-code' }, { json: false });
const mc = (p, tok) => call('GET', p, null, { headers: { Authorization: `Bearer ${tok}` } });
const login = () => call('POST', '/authentication/login_with_xbox', { identityToken: 'XBL3.0 x=fake-uhs;fake-xsts-token' });

test('contrôle: scénario inconnu rejeté', async () => {
  assert.equal((await scenario('nope')).status, 400);
});

test('ok: chaîne complète', async () => {
  await scenario('ok');
  const dc = await call('POST', '/devicecode', { client_id: 'x', scope: 's' }, { json: false });
  assert.equal(dc.body.device_code, 'fake-device-code');
  assert.equal(typeof dc.body.expires_in, 'number');
  const t = await poll();
  assert.equal(t.status, 200);
  assert.match(t.body.access_token, /^fake-/);
  const xbl = await call('POST', '/user/authenticate', { Properties: { RpsTicket: 'd=' + t.body.access_token } });
  assert.equal(xbl.body.DisplayClaims.xui[0].uhs, 'fake-uhs');
  const xsts = await call('POST', '/xsts/authorize', { Properties: { UserTokens: [xbl.body.Token] } });
  assert.equal(xsts.status, 200);
  const l = await login();
  assert.match(l.body.access_token, /^fake-mc-/);
  const ent = await mc('/entitlements/mcstore', l.body.access_token);
  assert.ok(ent.body.items.some((i) => i.name === 'game_minecraft'));
  const prof = await mc('/minecraft/profile', l.body.access_token);
  assert.deepEqual([prof.body.id, prof.body.name], ['069a79f444e94726a5befca90e38aaf5', 'TestPlayer']);
  assert.equal((await call('POST', '/consumers/oauth2/v2.0/devicecode', { client_id: 'x' }, { json: false })).status, 200);
  const st = await call('GET', '/__state');
  assert.ok(st.body.requests.length >= 7);
});

test('authorization_pending_then_ok', async () => {
  await scenario('authorization_pending_then_ok');
  for (let i = 0; i < 2; i++) {
    const r = await poll();
    assert.equal(r.status, 400);
    assert.equal(r.body.error, 'authorization_pending');
  }
  assert.equal((await poll()).status, 200);
});

test('declined', async () => {
  await scenario('declined');
  const r = await poll();
  assert.equal(r.status, 400);
  assert.equal(r.body.error, 'authorization_declined');
});

test('expired_code', async () => {
  await scenario('expired_code');
  assert.equal((await poll()).body.error, 'expired_token');
});

test('no_license', async () => {
  await scenario('no_license');
  const l = await login();
  const ent = await mc('/entitlements/mcstore', l.body.access_token);
  assert.equal(ent.status, 200);
  assert.deepEqual(ent.body.items, []);
});

test('xbox_child_account', async () => {
  await scenario('xbox_child_account');
  const r = await call('POST', '/xsts/authorize', { Properties: { UserTokens: ['fake-xbl-token'] } });
  assert.equal(r.status, 401);
  assert.equal(r.body.XErr, 2148916238);
});

test('token_expired_then_refresh', async () => {
  await scenario('token_expired_then_refresh');
  const t = await poll();
  const l = await login();
  assert.equal((await mc('/minecraft/profile', l.body.access_token)).status, 401);
  const r = await call('POST', '/token', { grant_type: 'refresh_token', refresh_token: t.body.refresh_token, client_id: 'x' }, { json: false });
  assert.equal(r.status, 200);
  assert.notEqual(r.body.access_token, t.body.access_token);
  const l2 = await login();
  assert.equal((await mc('/minecraft/profile', l2.body.access_token)).status, 200);
  const bad = await call('POST', '/token', { grant_type: 'refresh_token', refresh_token: 'bad' }, { json: false });
  assert.equal(bad.body.error, 'invalid_grant');
});

test('server_error', async () => {
  await scenario('server_error');
  for (const p of ['/devicecode', '/user/authenticate', '/xsts/authorize', '/authentication/login_with_xbox']) {
    assert.equal((await call('POST', p, { a: 1 })).status, 500);
  }
  assert.equal((await mc('/minecraft/profile', 'fake-mc-access-1')).status, 500);
  await scenario('ok');
});

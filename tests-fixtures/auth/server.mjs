// Faux serveurs d'authentification (Microsoft, Xbox Live, XSTS, Minecraft) pour les E2E.
// Node >= 18, aucune dépendance. Écoute uniquement sur 127.0.0.1. Jetons factices (préfixe fake-).
import http from 'node:http';
import { fileURLToPath } from 'node:url';

export const PROFILE = { id: '069a79f444e94726a5befca90e38aaf5', name: 'TestPlayer' };
export const SCENARIOS = [
  'ok', 'authorization_pending_then_ok', 'declined', 'expired_code',
  'no_license', 'xbox_child_account', 'token_expired_then_refresh', 'server_error',
];
const MS_PREFIX = '/consumers/oauth2/v2.0';

export function createServer() {
  let scenario, requests, pollCount, generation, staleGen;
  function reset(s) {
    scenario = s; requests = []; pollCount = 0; generation = 1;
    // Jetons Minecraft de génération <= staleGen refusés (401) : le refresh les rend valides.
    staleGen = s === 'token_expired_then_refresh' ? 1 : 0;
  }
  reset('ok');

  const send = (res, status, body) => {
    const text = JSON.stringify(body);
    res.writeHead(status, { 'Content-Type': 'application/json', 'Content-Length': Buffer.byteLength(text) });
    res.end(text);
  };
  const parseBody = (raw, ct) => {
    if (!raw) return {};
    try {
      if ((ct || '').includes('json')) return JSON.parse(raw);
      return Object.fromEntries(new URLSearchParams(raw));
    } catch { return { _raw: raw }; }
  };
  const msToken = () => ({
    access_token: `fake-ms-access-${generation}`, refresh_token: `fake-ms-refresh-${generation}`,
    expires_in: 3600, token_type: 'Bearer', scope: 'XboxLive.signin offline_access',
  });
  const mcGen = (auth) => {
    const m = /^Bearer fake-mc-access-(\d+)$/.exec(auth || '');
    return m ? Number(m[1]) : null;
  };

  function route(method, path, body, headers, res) {
    if (scenario === 'server_error') return send(res, 500, { error: 'server_error' });
    if (path.startsWith(MS_PREFIX)) path = path.slice(MS_PREFIX.length);

    if (method === 'POST' && path === '/devicecode') {
      return send(res, 200, {
        user_code: 'FAKE1234', device_code: 'fake-device-code',
        verification_uri: 'https://microsoft.invalid/link', // .invalid : jamais résolu
        expires_in: 900, interval: 1, message: 'Fake device code (test)',
      });
    }
    if (method === 'POST' && path === '/token') {
      if (body.grant_type === 'refresh_token') {
        if (!String(body.refresh_token || '').startsWith('fake-ms-refresh-')) {
          return send(res, 400, { error: 'invalid_grant', error_description: 'fake refresh token invalide' });
        }
        generation += 1;
        return send(res, 200, msToken());
      }
      pollCount += 1;
      if (scenario === 'declined') return send(res, 400, { error: 'authorization_declined' });
      if (scenario === 'expired_code') return send(res, 400, { error: 'expired_token' });
      if (scenario === 'authorization_pending_then_ok' && pollCount <= 2) {
        return send(res, 400, { error: 'authorization_pending' });
      }
      return send(res, 200, msToken());
    }
    if (method === 'POST' && path === '/user/authenticate') {
      return send(res, 200, { IssueInstant: '2026-01-01T00:00:00Z', NotAfter: '2099-01-01T00:00:00Z',
        Token: 'fake-xbl-token', DisplayClaims: { xui: [{ uhs: 'fake-uhs' }] } });
    }
    if (method === 'POST' && path === '/xsts/authorize') {
      if (scenario === 'xbox_child_account') {
        return send(res, 401, { Identity: '0', XErr: 2148916238, Message: '', Redirect: 'https://xbox.invalid/family' });
      }
      return send(res, 200, { IssueInstant: '2026-01-01T00:00:00Z', NotAfter: '2099-01-01T00:00:00Z',
        Token: 'fake-xsts-token', DisplayClaims: { xui: [{ uhs: 'fake-uhs' }] } });
    }
    if (method === 'POST' && path === '/authentication/login_with_xbox') {
      if (!/^XBL3\.0 x=[^;]+;.+/.test(String(body.identityToken || ''))) return send(res, 400, { error: 'BAD_REQUEST' });
      return send(res, 200, { username: 'fake-user', roles: [], access_token: `fake-mc-access-${generation}`,
        token_type: 'Bearer', expires_in: 86400 });
    }
    if (method === 'GET' && (path === '/entitlements/mcstore' || path === '/minecraft/profile')) {
      const g = mcGen(headers.authorization);
      if (g === null || g <= staleGen) return send(res, 401, { error: 'UNAUTHORIZED' });
      if (path === '/entitlements/mcstore') {
        return send(res, 200, { items: scenario === 'no_license' ? []
          : [{ name: 'product_minecraft', signature: 'fake' }, { name: 'game_minecraft', signature: 'fake' }], signature: 'fake', keyId: '1' });
      }
      return send(res, 200, { ...PROFILE, skins: [], capes: [] });
    }
    return send(res, 404, { error: 'NOT_FOUND', path });
  }

  return http.createServer((req, res) => {
    const chunks = [];
    req.on('data', (c) => chunks.push(c));
    req.on('end', () => {
      const raw = Buffer.concat(chunks).toString('utf8');
      const path = new URL(req.url, 'http://127.0.0.1').pathname;
      const body = parseBody(raw, req.headers['content-type']);
      if (path === '/__control' && req.method === 'POST') {
        if (!SCENARIOS.includes(body.scenario)) return send(res, 400, { error: 'scénario inconnu', scenarios: SCENARIOS });
        reset(body.scenario);
        return send(res, 200, { scenario });
      }
      if (path === '/__state' && req.method === 'GET') return send(res, 200, { scenario, requests });
      requests.push({ method: req.method, path, headers: { authorization: req.headers.authorization }, body });
      route(req.method, path, body, req.headers, res);
    });
  });
}

if (process.argv[1] === fileURLToPath(import.meta.url)) {
  const i = process.argv.indexOf('--port');
  const port = i > -1 ? Number(process.argv[i + 1]) : 0;
  const server = createServer();
  server.listen(port, '127.0.0.1', () => {
    const p = server.address().port;
    console.log(JSON.stringify({ ready: true, port: p, base: `http://127.0.0.1:${p}` }));
  });
  for (const s of ['SIGINT', 'SIGTERM']) process.on(s, () => server.close(() => process.exit(0)));
}

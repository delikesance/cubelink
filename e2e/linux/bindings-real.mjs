// Essai réel sous Linux : l'application Tauri (build debug) tourne vraiment, pilotée par WebDriver (tauri-driver +
// WebKitWebDriver), et appelle ses commandes Rust par `createTauriServices()`, donc par les liaisons générées
// (`src/services/bindings.ts`). Vérifie que chaque appel atteint Rust avec les bons noms d'arguments : une clé mal
// nommée donnerait « missing required key », un type faux « invalid args ».
//
// Prérequis : `cargo build -p cubelink`, `cargo install tauri-driver`, paquet `webkit2gtk-driver`, `npm ci`.
// Lancer : `xvfb-run -a node e2e/linux/bindings-real.mjs` (sans Xvfb sur un bureau Linux). Code de sortie 0 si tout passe.

import { spawn } from "node:child_process";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { fileURLToPath } from "node:url";

const ROOT = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "../..");
const APP = path.join(ROOT, "target/debug/cubelink");
const DRIVER = process.env.TAURI_DRIVER ?? path.join(os.homedir(), ".cargo/bin/tauri-driver");
const WD = "http://127.0.0.1:4444";
const VITE_PORT = 5173; // devUrl de tauri.conf.json : le build debug charge l'interface depuis Vite.

const procs = [];
const dataDir = fs.mkdtempSync(path.join(os.tmpdir(), "cubelink-bindings-"));

function start(cmd, args, env = {}) {
  const p = spawn(cmd, args, { cwd: ROOT, env: { ...process.env, ...env }, stdio: ["ignore", "pipe", "pipe"], detached: true });
  const log = [];
  p.stdout.on("data", (d) => log.push(String(d)));
  p.stderr.on("data", (d) => log.push(String(d)));
  procs.push(p);
  return { p, log };
}

function cleanup() {
  // Groupe de processus entier : WebKitWebDriver et l'application lancés par tauri-driver partent avec lui.
  for (const p of procs) { try { process.kill(-p.pid, "SIGTERM"); } catch { /* déjà parti */ } }
  fs.rmSync(dataDir, { recursive: true, force: true });
}

async function waitHttp(url, ms) {
  const end = Date.now() + ms;
  for (;;) {
    try { await fetch(url); return; } catch { /* pas encore */ }
    if (Date.now() > end) throw new Error(`${url} injoignable après ${ms} ms`);
    await new Promise((r) => setTimeout(r, 250));
  }
}

async function wd(method, url, body) {
  const r = await fetch(`${WD}${url}`, { method, headers: { "content-type": "application/json" }, body: body ? JSON.stringify(body) : undefined });
  const j = await r.json();
  if (!r.ok) throw new Error(`WebDriver ${method} ${url} : ${JSON.stringify(j.value)}`);
  return j.value;
}

// Scénario exécuté dans la page de l'application : vrais appels IPC par les services Tauri (liaisons générées).
const SCENARIO = async () => {
  const { createTauriServices } = await import("/src/services/tauri.ts");
  const svc = createTauriServices();
  const out = [];
  const step = async (name, fn, expectError = false) => {
    try {
      const v = await fn();
      out.push({ name, ok: !expectError, value: v ?? null });
      return v;
    } catch (e) {
      const msg = String(e?.message ?? e);
      // Erreur attendue : elle doit venir de Rust (message en français), pas de la désérialisation des arguments.
      const ipc = /missing required key|invalid args|invalid type|unknown variant|not found/i.test(msg);
      out.push({ name, ok: expectError && !ipc, error: msg });
      return null;
    }
  };

  await step("authOfflineLogin", () => svc.authOfflineLogin("Testeur"));
  await step("authStatus", () => svc.authStatus());
  await step("authAccounts", () => svc.authAccounts());
  const s = await step("settingsGet", () => svc.settingsGet());
  if (s) {
    await step("settingsSet", () => svc.settingsSet({ ...s, close_on_launch: !s.close_on_launch, overlay_hotkey: "Ctrl+Alt+F9" }));
    await step("settingsGet (relu)", () => svc.settingsGet());
    await step("settingsSet (refus français)", () => svc.settingsSet({ ...s, allow_relay: false, relay_only: true }), true);
  }
  await step("instancesList", () => svc.instancesList());
  await step("minecraftVersions (hors réseau)", () => svc.minecraftVersions(), true);
  await step("instancesCreate (nom vide refusé)", () => svc.instancesCreate({ name: " ", minecraft: "1.21.1", loader: { loader: "vanilla", version: "" }, ram_mb: 2048 }), true);
  await step("instancesUpdate (inconnue)", () => svc.instancesUpdate("absente", { name: "X", ram_mb: 3072 }), true);
  await step("instancesArtImage (inconnue)", () => svc.instancesArtImage("absente", "icon.png", 128), true);
  await step("modsList (inconnue)", () => svc.modsList("absente"), true);
  await step("modsToggle (inconnue)", () => svc.modsToggle("absente", "a.jar", false), true);
  await step("modsFixApply (inconnue)", () => svc.modsFixApply("absente", "plan", ["a"]), true);
  await step("instancesImportMrpack (fichier absent)", () => svc.instancesImportMrpack("/nulle/part.mrpack", { name: "Pack", ram_mb: 2048 }), true);
  await step("friendMyCode", () => svc.friendMyCode());
  await step("friendList", () => svc.friendList());
  await step("friendRequests", () => svc.friendRequests({ offset: 0, limit: 10, query: "a" }));
  await step("friendSendRequest (code invalide)", () => svc.friendSendRequest("pas-un-code"), true);
  await step("friendInvites", () => svc.friendInvites());
  await step("friendInviteDismiss", () => svc.friendInviteDismiss("aucune"));
  await step("joinPlan (ami inconnu)", () => svc.joinPlan("00".repeat(32)), true);
  await step("joinStart (sans plan)", () => svc.joinStart("00".repeat(32), "f".repeat(64), false, 2048), true);
  await step("gameJoinFriend (ami inconnu)", () => svc.gameJoinFriend("00".repeat(32), "absente"), true);
  await step("hostState", () => svc.hostState());
  await step("gameCurrent", () => svc.gameCurrent());
  await step("browseCheck (inconnue)", () => svc.browseCheck("modrinth", "sodium", "mod", "absente", "v1"), true);
  await step("stageSupported", () => svc.stageSupported());
  await step("stageState", () => svc.stageState());
  await step("stageLayout", () => svc.stageLayout({ x: 0, y: 36, w: 1100, h: 684, panel_w: 380 }));
  await step("stagePanel", () => svc.stagePanel(false));
  await step("stageView", () => svc.stageView(false));
  await step("authLogout", () => svc.authLogout());
  return out;
};

let failed = false;
try {
  const vite = start("npx", ["vite", "--port", String(VITE_PORT), "--strictPort"]);
  await waitHttp(`http://localhost:${VITE_PORT}/`, 60_000).catch((e) => { throw new Error(`${e.message}\n${vite.log.join("")}`); });
  const driver = start(DRIVER, [], {
    CUBELINK_TEST_MODE: "1",
    CUBELINK_DATA_DIR: dataDir,
    CUBELINK_RELAY_MODE: "disabled",
    CUBELINK_SKIP_JAVA_PROBE: "1",
  });
  await waitHttp(`${WD}/status`, 30_000).catch((e) => { throw new Error(`${e.message}\n${driver.log.join("")}`); });
  const session = await wd("POST", "/session", { capabilities: { alwaysMatch: { "tauri:options": { application: APP } } } });
  const sid = session.sessionId;
  try {
    // L'interface est montée : l'écran de connexion (ou l'accueil) est là.
    const end = Date.now() + 60_000;
    for (;;) {
      const ready = await wd("POST", `/session/${sid}/execute/sync`, { script: "return !!document.querySelector('main[data-route]') && '__TAURI_INTERNALS__' in window;", args: [] });
      if (ready) break;
      if (Date.now() > end) throw new Error("interface non montée après 60 s");
      await new Promise((r) => setTimeout(r, 500));
    }
    await wd("POST", `/session/${sid}/timeouts`, { script: 120_000 });
    const results = await wd("POST", `/session/${sid}/execute/async`, {
      script: `const done = arguments[arguments.length - 1]; (${SCENARIO.toString()})().then(done, (e) => done([{ name: "scénario", ok: false, error: String(e) }]));`,
      args: [],
    });
    for (const r of results) {
      const detail = r.error ? `erreur : ${r.error}` : `valeur : ${JSON.stringify(r.value).slice(0, 160)}`;
      console.log(`${r.ok ? "OK  " : "ÉCHEC"} ${r.name} — ${detail}`);
      if (!r.ok) failed = true;
    }
    const settings = JSON.parse(fs.readFileSync(path.join(dataDir, "settings.json"), "utf8"));
    const disk = settings.overlay_hotkey === "Ctrl+Alt+F9";
    console.log(`${disk ? "OK  " : "ÉCHEC"} settings.json sur le disque — overlay_hotkey = ${settings.overlay_hotkey}`);
    if (!disk) failed = true;
  } finally {
    await wd("DELETE", `/session/${sid}`).catch(() => {});
  }
} catch (e) {
  console.error(String(e?.stack ?? e));
  failed = true;
} finally {
  cleanup();
}
console.log(failed ? "Essai réel des liaisons : ÉCHEC" : "Essai réel des liaisons : tout passe");
process.exit(failed ? 1 : 0);

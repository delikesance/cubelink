// Essai réel sous Linux du lien ami `cubelink://add/<code>` : la vraie application Tauri (build debug) tourne, reçoit
// le lien comme le système le lui donnerait (argument de lancement, ou second lancement pendant que la première
// instance tourne) et le rend à l'interface par `friend_take_pending_link` / `friend://link`. Pas de WebDriver : la
// « page » est une petite sonde servie sur le devUrl (localhost:5173) qui appelle les vraies commandes IPC et rapporte.
// Ne touche ni souris, ni clavier, ni bureau : Xvfb, D-Bus de session et dossiers temporaires.
//
// Prérequis : `cargo build -p cubelink`. Lancer :
//   xvfb-run -a dbus-run-session -- node e2e/linux/friend-link-real.mjs
// Variable : CUBELINK_BIN (binaire, défaut target/debug/cubelink). Code de sortie 0 si tout passe.
//
// Non couvert ici (voir docs/release.md) : l'enregistrement du schéma par les paquets (MSI, MSIX, .rpm, AppImage) et
// le clic d'un navigateur ; seul ce que fait l'application une fois lancée avec le lien est exercé.

import { spawn } from "node:child_process";
import fs from "node:fs";
import http from "node:http";
import os from "node:os";
import path from "node:path";
import { fileURLToPath } from "node:url";

const ROOT = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "../..");
const BIN = process.env.CUBELINK_BIN ?? path.join(process.env.CARGO_TARGET_DIR ?? path.join(ROOT, "target"), "debug/cubelink");
const PORT = 5173; // devUrl de tauri.conf.json : le build debug y charge l'interface.
const CODE = "mcl1-gaydamrqgayc2mbrgm2tqmjvgq4dambqgayd";
const LINK = `cubelink://add/${CODE}`;

// Sonde : reprend le lien au montage, écoute friend://link, rapporte tout au serveur.
const PROBE = `<!doctype html><meta charset="utf-8"><title>sonde</title><script type="module">
const inv = (c, a) => window.__TAURI_INTERNALS__.invoke(c, a);
const post = (o) => fetch("/report", { method: "POST", body: JSON.stringify(o) });
const listen = (ev, cb) => inv("plugin:event|listen", { event: ev, target: { kind: "Any" }, handler: window.__TAURI_INTERNALS__.transformCallback(cb) });
try {
  await listen("friend://link", async () => post({ kind: "event", link: await inv("friend_take_pending_link") }));
  const first = await inv("friend_take_pending_link");
  const again = await inv("friend_take_pending_link");
  await post({ kind: "mount", link: first, again, mine: await inv("friend_my_link"), code: await inv("friend_my_code") });
} catch (e) { await post({ kind: "error", error: String(e) }); }
</script>`;

const reports = [];
const waiters = [];
const server = http.createServer((req, res) => {
  if (req.method === "POST" && req.url === "/report") {
    let b = "";
    req.on("data", (d) => { b += d; });
    req.on("end", () => {
      const r = JSON.parse(b);
      reports.push(r);
      waiters.splice(0).forEach((w) => w());
      res.end("ok");
    });
    return;
  }
  res.setHeader("content-type", "text/html");
  res.end(PROBE);
});
await new Promise((ok) => server.listen(PORT, "127.0.0.1", ok));

const procs = [];
const dirs = [];
function launch(args, env) {
  const log = [];
  const p = spawn(BIN, args, { cwd: ROOT, env: { ...process.env, ...env }, stdio: ["ignore", "pipe", "pipe"] });
  p.stdout.on("data", (d) => log.push(String(d)));
  p.stderr.on("data", (d) => log.push(String(d)));
  procs.push(p);
  return { p, log };
}
const tmp = (prefix) => { const d = fs.mkdtempSync(path.join(os.tmpdir(), prefix)); dirs.push(d); return d; };
async function waitReport(pred, what, ms = 90_000) {
  const end = Date.now() + ms;
  for (;;) {
    const r = reports.find(pred);
    if (r) return r;
    if (Date.now() > end) throw new Error(`rien reçu : ${what}`);
    await new Promise((ok) => { waiters.push(ok); setTimeout(ok, 500); });
  }
}
const exited = (p, ms) => new Promise((ok) => { if (p.exitCode !== null) return ok(true); const t = setTimeout(() => ok(false), ms); p.once("exit", () => { clearTimeout(t); ok(true); }); });
const stop = (p) => { try { p.kill("SIGTERM"); } catch { /* déjà parti */ } };

let fails = 0;
const check = (cond, msg) => { console.log(`${cond ? "OK   " : "ÉCHEC"} ${msg}`); if (!cond) fails += 1; };

try {
  // 1. Démarrage à froid avec le lien en argument (mode test : pas d'instance unique, dossier de données isolé).
  {
    const n = reports.length;
    const { p } = launch([LINK], { CUBELINK_TEST_MODE: "1", CUBELINK_DATA_DIR: tmp("cl-link-a-"), CUBELINK_RELAY_MODE: "disabled" });
    const r = await waitReport((x, i) => i >= n && x.kind === "mount", "montage (lien au lancement)");
    check(r.link === LINK, `démarrage à froid : le lien du lancement est repris (${r.link})`);
    check(r.again === null, "le lien n'est repris qu'une fois (second appel : null)");
    check(r.mine === `cubelink://add/${r.code}`, `friend_my_link = cubelink://add/ + friend_my_code (${r.mine?.slice(0, 40)}…)`);
    stop(p); await exited(p, 10_000);
  }
  // 2. Démarrage à froid sans lien, avec un argument qui n'en est pas un, puis avec un lien démesuré : rien n'est repris.
  for (const [what, arg] of [["argument ordinaire", "--background"], ["autre schéma", "https://example.org/add/x"], ["lien démesuré", `cubelink://add/${"a".repeat(6000)}`]]) {
    const n = reports.length;
    const { p } = launch([arg], { CUBELINK_TEST_MODE: "1", CUBELINK_DATA_DIR: tmp("cl-link-b-"), CUBELINK_RELAY_MODE: "disabled" });
    const r = await waitReport((x, i) => i >= n && x.kind === "mount", `montage (${what})`);
    check(r.link === null, `démarrage à froid, ${what} : aucun lien repris`);
    stop(p); await exited(p, 10_000);
  }
  // 3. Cubelink déjà ouvert (instance unique, comme en usage normal) : un second lancement avec le lien le transmet à la
  //    première instance (événement friend://link), puis sort sans ouvrir de second noeud.
  {
    const home = tmp("cl-link-home-");
    const env = { HOME: home, XDG_DATA_HOME: path.join(home, ".local/share"), XDG_CONFIG_HOME: path.join(home, ".config"), CUBELINK_RELAY_MODE: "disabled" };
    const first = launch([], env);
    const n = reports.length;
    const m = await waitReport((x, i) => i >= n && x.kind === "mount", "montage (première instance)");
    check(m.link === null, "première instance lancée sans lien : rien en attente");
    const second = launch([LINK], env);
    const e = await waitReport((x, i) => i >= n && x.kind === "event", "événement friend://link");
    check(e.link === LINK, `instance déjà ouverte : le lien du second lancement arrive par friend://link (${e.link})`);
    check(await exited(second.p, 20_000), "le second lancement sort (instance unique)");
    check(first.p.exitCode === null, "la première instance tourne toujours");
    stop(first.p); await exited(first.p, 10_000);
  }
  check(!reports.some((r) => r.kind === "error"), `aucune erreur IPC de la sonde${reports.filter((r) => r.kind === "error").map((r) => ` : ${r.error}`).join("")}`);
} catch (e) {
  console.error(String(e?.stack ?? e));
  fails += 1;
} finally {
  procs.forEach(stop);
  server.close();
  dirs.forEach((d) => fs.rmSync(d, { recursive: true, force: true }));
}
console.log(fails ? `Essai réel du lien ami : ${fails} ÉCHEC(S)` : "Essai réel du lien ami : tout passe");
process.exit(fails ? 1 : 0);

import { describe, expect, it } from "vitest";
import { createFakeServices } from "./fake";
import type { TransferProgress } from "./types";

const svc = () => createFakeServices({ tickMs: 0, loginDelayMs: 0 });

describe("faux service", () => {
  it("instance d'exemple Create: Aeronautics, 94 mods", async () => {
    const [i] = await svc().instancesList();
    expect(i.name).toBe("Create: Aeronautics");
    expect(i.mod_count).toBe(94);
    expect(i.loader.loader).toBe("neoforge");
  });

  it("login simulé et événement auth", async () => {
    const s = svc();
    const seen: unknown[] = [];
    s.onAuthState((p) => seen.push(p));
    expect(await s.authStatus()).toBeNull();
    const d = await s.authBeginLogin();
    expect(d.user_code).toBeTruthy();
    const p = await s.authPollLogin();
    expect(p.kind).toBe("online");
    expect(await s.authStatus()).toEqual(p);
    await s.authLogout();
    expect(seen.length).toBe(2);
  });

  it("1000 demandes, pagination et recherche", async () => {
    const s = svc();
    const p1 = await s.friendRequests({ offset: 0, limit: 50 });
    expect(p1.total).toBe(1000);
    expect(p1.items).toHaveLength(50);
    const p2 = await s.friendRequests({ offset: 50, limit: 50 });
    expect(p2.items[0].public_key).not.toBe(p1.items[0].public_key);
    expect((await s.friendRequests({ offset: 990, limit: 50 })).items).toHaveLength(10);
    expect(p1.items[0].verify_code).toMatch(/^[0-9A-F]{4}( · [0-9A-F]{4}){3}$/);
    const q = await s.friendRequests({ offset: 0, limit: 5, query: "alex0" });
    expect(q.items.every((r) => r.name.toLowerCase().includes("alex0"))).toBe(true);
  });

  it("accepter, refuser, tout refuser", async () => {
    const s = svc();
    const [first, second] = (await s.friendRequests({ offset: 0, limit: 2 })).items;
    await s.friendAccept(first.public_key);
    await s.friendRefuse(second.public_key);
    expect((await s.friendRequests({ offset: 0, limit: 1 })).total).toBe(998);
    expect(await s.friendRefuseAll()).toBe(998);
    expect((await s.friendRequests({ offset: 0, limit: 1 })).total).toBe(0);
    await expect(s.friendRefuse("zzz")).rejects.toThrow();
  });

  it("code ami : régénération et envoi", async () => {
    const s = svc();
    const a = await s.friendMyCode();
    expect(a.startsWith("mcl1-")).toBe(true);
    expect(await s.friendRegenerateCode()).not.toBe(a);
    await expect(s.friendSendRequest("nimporte")).rejects.toThrow("Code ami invalide.");
    expect(await s.friendSendRequest(a)).toMatch(/^[0-9A-F]{4}( · [0-9A-F]{4}){3}$/);
  });

  it("plan de jonction : 94 fichiers, 3 à recevoir", async () => {
    const plan = await svc().joinPlan("k");
    expect(plan.manifest.files).toHaveLength(94);
    expect(plan.cached + plan.to_receive).toBe(94);
    expect(plan.unknown_mods.length).toBe(1);
  });

  it("transfert simulé de 0 à 100 % puis pause/annulation", async () => {
    const s = createFakeServices({ tickMs: 5 });
    const events: TransferProgress[] = [];
    s.onTransferProgress((e) => events.push(e));
    await s.joinStart("k");
    for (let n = 0; n < 200 && events[events.length - 1]?.phase !== "done"; n++) {
      await new Promise((r) => setTimeout(r, 10));
    }
    const last = events[events.length - 1];
    expect(events[0].phase).toBe("preparing");
    expect(last.phase).toBe("done");
    expect(last.bytes_done).toBe(last.bytes_total);

    events.length = 0;
    await s.joinStart("k");
    await s.joinPause();
    await s.joinCancel();
    expect(events.map((e) => e.phase)).toEqual(["preparing", "paused", "failed"]);
  });

  it("paramètres : aller-retour", async () => {
    const s = svc();
    const cur = await s.settingsGet();
    const out = await s.settingsSet({ ...cur, default_ram_mb: 8192 });
    expect(out.default_ram_mb).toBe(8192);
    expect((await s.settingsGet()).default_ram_mb).toBe(8192);
  });

  it("création d'instance et erreurs en français", async () => {
    const s = svc();
    const i = await s.instancesCreate({
      name: "Test", minecraft: "1.21.1", loader: { loader: "vanilla", version: "" }, ram_mb: 2048,
    });
    expect((await s.instancesList()).map((x) => x.id)).toContain(i.id);
    await expect(s.instancesLaunch("nope")).rejects.toThrow("Instance introuvable.");
  });
});

describe("fake : gestion des instances", () => {
  it("kill, update, logs, delete et flux de logs au lancement", async () => {
    const s = createFakeServices({ tickMs: 0 });
    const lines: string[] = [];
    const states: string[] = [];
    s.onLaunchLog((e) => lines.push(e.line));
    s.onLaunchState((e) => states.push(e.state));
    await s.instancesLaunch("inst-aeronautics");
    expect(lines.length).toBeGreaterThan(0);
    expect(await s.instancesLogs("inst-aeronautics")).toContain(lines[0]);
    await s.instancesKill("inst-aeronautics");
    expect(states).toEqual(["preparing", "exited"]);
    const up = await s.instancesUpdate("inst-aeronautics", { name: "Neuf", ram_mb: 2048, java_path: null, jvm_args: "-Xss2m" });
    expect(up).toMatchObject({ name: "Neuf", ram_mb: 2048, jvm_args: "-Xss2m" });
    await s.instancesDelete("inst-aeronautics");
    expect((await s.instancesList()).map((x) => x.id)).not.toContain("inst-aeronautics");
    await expect(s.instancesKill("inst-aeronautics")).rejects.toThrow("Instance introuvable.");
  });

  it("signale les mods manquants à l'import", async () => {
    const s = createFakeServices({ tickMs: 0 });
    const got: string[][] = [];
    s.onImportWarning((e) => got.push(e.missing));
    await s.instancesImportMrpack("pack-manquant.mrpack");
    expect(got).toEqual([["sodium", "iris"]]);
  });

  it("parcourir : pagination, filtre, non téléchargeables, versions", async () => {
    const s = svc();
    for (const source of ["curseforge", "modrinth"] as const) {
      for (const kind of ["modpack", "mod", "resourcepack", "shader"] as const) {
        const p = await s.browseSearch({ source, kind, query: "", offset: 0, limit: 10 });
        expect(p.total).toBeGreaterThanOrEqual(30);
        expect(p.hits).toHaveLength(10);
        const p2 = await s.browseSearch({ source, kind, query: "", offset: 10, limit: 10 });
        expect(p2.hits[0].id).not.toBe(p.hits[0].id);
      }
    }
    const f = await s.browseSearch({ source: "modrinth", kind: "mod", query: "sodium", offset: 0, limit: 50 });
    expect(f.total).toBeGreaterThan(0);
    expect(f.hits.every((h) => h.name.toLowerCase().includes("sodium"))).toBe(true);
    const all = await s.browseSearch({ source: "curseforge", kind: "mod", query: "", offset: 0, limit: 100 });
    expect(all.hits.some((h) => !h.downloadable)).toBe(true);
    const v = await s.browseVersions("modrinth", all.hits[0].id.replace("curseforge", "modrinth"), "mod", "1.21.1", "neoforge");
    expect(v.length).toBeGreaterThan(0);
  });

  it("parcourir : installer un mod l'ajoute à l'instance et émet la progression", async () => {
    const s = svc();
    const ev: number[] = [];
    s.onBrowseProgress((e) => ev.push(e.done));
    const before = (await s.modsList("inst-aeronautics")).length;
    const r = await s.browseInstall({ source: "modrinth", id: "modrinth-mod-0", kind: "mod", instance_id: "inst-aeronautics" });
    expect(r.installed).toHaveLength(1);
    expect((await s.modsList("inst-aeronautics")).length).toBe(before + 1);
    expect(ev.length).toBeGreaterThan(1);
    await expect(s.browseInstall({ source: "curseforge", id: "curseforge-mod-5", kind: "mod" })).rejects.toThrow();
  });

  it("parcourir : un modpack crée une nouvelle instance", async () => {
    const s = svc();
    const n = (await s.instancesList()).length;
    const r = await s.browseInstall({ source: "modrinth", id: "modrinth-modpack-1", kind: "modpack" });
    expect((await s.instancesList()).length).toBe(n + 1);
    expect(r.instance.id).toBeTruthy();
  });

  it("mods : liste, bascule, suppression, ajout", async () => {
    const s = svc();
    const id = "inst-aeronautics";
    const list = await s.modsList(id);
    expect(list.some((m) => m.custom)).toBe(true);
    await s.modsToggle(id, list[0].file, !list[0].enabled);
    expect((await s.modsList(id))[0].enabled).toBe(!list[0].enabled);
    await s.modsRemove(id, list[0].file);
    expect(await s.modsList(id)).toHaveLength(list.length - 1);
    const m = await s.modsAdd(id, "C:\\mods\\nouveau.jar");
    expect(m).toMatchObject({ file: "nouveau.jar", custom: true, enabled: true });
    await expect(s.modsAdd(id, "x.txt")).rejects.toThrow();
    await expect(s.modsToggle(id, "absent.jar", true)).rejects.toThrow("Mod introuvable.");
  });

  it("pickFiles et glisser-déposer", async () => {
    const s = svc();
    expect(await s.pickFiles({ extensions: ["jar"] })).toEqual([]);
    s.setPickedFiles(["a.jar", "b.txt", "c.jar"]);
    expect(await s.pickFiles({ extensions: ["jar"], multiple: true })).toEqual(["a.jar", "c.jar"]);
    const got: string[][] = [];
    const off = s.onFilesDropped((p) => got.push(p));
    s.dropFiles(["x.jar"]);
    off();
    s.dropFiles(["y.jar"]);
    expect(got).toEqual([["x.jar"]]);
  });
});

describe("fake : illustrations d'instance", () => {
  it("instances de démo avec et sans art, images distinctes", async () => {
    const s = createFakeServices({ tickMs: 0 });
    const list = await s.instancesList();
    const atm = list.find((i) => i.id === "inst-atm10")!;
    const pro = list.find((i) => i.id === "inst-prominence")!;
    expect(atm.art).toMatchObject({ color: "#2f6fed", icon: "icon.png" });
    expect(pro.art?.color).toBe("#c0392b");
    expect(atm.art!.screenshots).toHaveLength(4);
    expect(list.find((i) => i.id === "inst-aeronautics")!.art).toBeFalsy();
    const imgs = await Promise.all([atm.art!.icon!, ...atm.art!.screenshots].map((f) => s.instancesArtImage(atm.id, f)));
    expect(new Set(imgs).size).toBe(5);
    expect(imgs.every((u) => u.startsWith("data:image/svg+xml"))).toBe(true);
    expect(await s.instancesArtImage(pro.id, "icon.png")).not.toBe(imgs[0]);
    await expect(s.instancesArtImage(atm.id, "absent.png")).rejects.toThrow();
    await expect(s.instancesArtImage("inst-aeronautics", "icon.png")).rejects.toThrow();
  });

  it("lookup : trouve par le nom, sinon rejette", async () => {
    const s = createFakeServices({ tickMs: 0 });
    await expect(s.instancesArtLookup("inst-aeronautics")).rejects.toThrow(
      "Aucun modpack correspondant à « Create: Aeronautics » trouvé. Choisis une image à la main.");
    await s.instancesUpdate("inst-aeronautics", { name: "All the Mods 10 perso" });
    const r = await s.instancesArtLookup("inst-aeronautics");
    expect(r.art?.title).toBe("All the Mods 10");
    expect((await s.instancesList())[0].art).toBeTruthy();
  });

  it("set : crée un art custom puis le met à jour", async () => {
    const s = createFakeServices({ tickMs: 0 });
    const a = await s.instancesArtSet("inst-aeronautics", "C:\\img\\bg.png", "banner");
    expect(a.art).toMatchObject({ source: "custom", title: "Create: Aeronautics" });
    expect(a.art!.screenshots[0]).toBe("bg.png");
    const b = await s.instancesArtSet("inst-aeronautics", "/x/ico.jpg", "icon");
    expect(b.art!.icon).toBe("ico.jpg");
    await expect(s.instancesArtSet("inst-aeronautics", "x.txt", "icon")).rejects.toThrow();
    await expect(s.instancesArtSet("nope", "x.png", "icon")).rejects.toThrow("Instance introuvable.");
  });

  it("installer un modpack attache un art à la nouvelle instance", async () => {
    const s = createFakeServices({ tickMs: 0 });
    const r = await s.browseInstall({ source: "modrinth", id: "modrinth-modpack-1", kind: "modpack" });
    expect(r.instance.art?.screenshots.length).toBe(4);
  });
});

describe("comptes multiples", () => {
  it("ajout, bascule, retrait et événements", async () => {
    const s = svc();
    const seen: Array<string | null> = [];
    s.onAccountsState((st) => seen.push(st.active));
    await s.authPollLogin();
    await s.authOfflineLogin("Alex_");
    let st = await s.authAccounts();
    expect(st.accounts.map((a) => a.kind)).toEqual(["online", "offline"]);
    expect(st.active).toBe(st.accounts[1].id);
    await s.authSwitch(st.accounts[0].id);
    expect((await s.authStatus())?.kind).toBe("online");
    await s.authRemove(st.accounts[0].id);
    st = await s.authAccounts();
    expect(st.accounts).toHaveLength(1);
    expect(st.active).toBe(st.accounts[0].id);
    expect(seen.length).toBe(4);
    await expect(s.authOfflineLogin("ab")).rejects.toThrow(/3 à 16/);
    await expect(s.authSwitch("nope")).rejects.toThrow();
  });
});

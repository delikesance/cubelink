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

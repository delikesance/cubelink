import { afterEach, describe, expect, it, vi } from "vitest";
import { act, cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { createFakeServices } from "../../services/fake";
import type { ModCheck, Services } from "../../services";
import { ModsPanel, UNDO_MS } from "./ModsPanel";

afterEach(cleanup);

const setup = (check: ModCheck | Error) => {
  const services: Services = {
    ...createFakeServices({ tickMs: 0 }),
    modsCheck: async () => { if (check instanceof Error) throw check; return check; },
  };
  render(<ModsPanel services={services} instanceId="inst-aeronautics" onCount={() => {}} onBrowse={() => {}} />);
};

describe("ModsPanel compatibilité", () => {
  it("affiche le bandeau, la liste dépliable et la pastille sur la ligne", async () => {
    setup({ checked: 8, issues: [
      { file: "iris-1.2.0.jar", mod_name: "Iris", kind: "missing_dep", severity: "error", message: "Il manque Sodium." },
      { file: "create-1.0.0.jar", mod_name: "Create", kind: "incompatible", severity: "warning", message: "Gêne Sodium.", related: "sodium-1.1.0.jar" },
    ] });
    const head = await screen.findByRole("button", { name: /2 problèmes de compatibilité/ });
    expect(head.getAttribute("aria-expanded")).toBe("false");
    fireEvent.click(head);
    expect(head.getAttribute("aria-expanded")).toBe("true");
    const list = screen.getByRole("list", { name: "Problèmes de compatibilité" });
    expect(list.textContent).toContain("Iris : Il manque Sodium.");
    const row = (await screen.findByText("iris-1.2.0.jar")).closest("li")!;
    expect(within(row).getByText("Il manque Sodium.")).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: "Voir sodium-1.1.0.jar" }));
    expect((screen.getByRole("searchbox") as HTMLInputElement).value).toBe("sodium-1.1.0.jar");
  });

  it("rien quand la liste est vide ou si l'appel échoue", async () => {
    setup({ checked: 8, issues: [] });
    await screen.findByText("iris-1.2.0.jar");
    expect(screen.queryByText(/problème/)).toBeNull();
    cleanup();
    setup(new Error("absent"));
    await screen.findByText("iris-1.2.0.jar");
    expect(screen.queryByText(/problème/)).toBeNull();
  });
});

describe("ModsPanel retraits en attente", () => {
  const FILE = "mod-maison.jar";
  const mount = (over: Partial<Services> = {}) => {
    const services: Services = { ...createFakeServices({ tickMs: 0 }), modsCheck: async () => ({ issues: [], checked: 0 }), ...over };
    let flush: (() => Promise<string | null>) | null = null;
    const onFlushError = vi.fn();
    const view = render(
      <ModsPanel
        services={services} instanceId="inst-aeronautics" onCount={() => {}} onBrowse={() => {}}
        registerFlush={(f) => { flush = f; }} onFlushError={onFlushError}
      />,
    );
    return { services, onFlushError, view, flush: () => flush };
  };
  const remove = async () => {
    fireEvent.click(await screen.findByRole("button", { name: `Supprimer ${FILE}` }));
  };

  it("flush avant le délai : un seul modsRemove, pas de second appel au terme du délai", async () => {
    const rm = vi.fn(async () => {});
    const { flush } = mount({ modsRemove: rm });
    await remove();
    vi.useFakeTimers({ shouldAdvanceTime: true });
    try {
      let res: string | null = "x";
      await act(async () => { res = await flush()!(); });
      expect(res).toBeNull();
      expect(rm).toHaveBeenCalledTimes(1);
      expect(rm).toHaveBeenCalledWith("inst-aeronautics", FILE);
      await act(async () => { await vi.advanceTimersByTimeAsync(UNDO_MS + 50); });
      expect(rm).toHaveBeenCalledTimes(1);
    } finally { vi.useRealTimers(); }
    await waitFor(() => expect(screen.queryByText(FILE)).toBeNull());
  });

  it("flush : un échec est renvoyé avec le nom du fichier et le mod reste listé", async () => {
    const { flush } = mount({ modsRemove: async () => { throw new Error("fichier verrouillé"); } });
    await remove();
    let res: string | null = null;
    await act(async () => { res = await flush()!(); });
    expect(res).toMatch(/mod-maison\.jar.*fichier verrouillé/);
    expect(await screen.findByText(FILE)).toBeTruthy();
  });

  it("flush attend un retrait déjà parti", async () => {
    let release: () => void = () => {};
    const rm = vi.fn(() => new Promise<void>((r) => { release = r; }));
    const { flush } = mount({ modsRemove: rm });
    const btn = await screen.findByRole("button", { name: `Supprimer ${FILE}` });
    vi.useFakeTimers({ shouldAdvanceTime: true });
    try {
      fireEvent.click(btn);
      await act(async () => { await vi.advanceTimersByTimeAsync(UNDO_MS + 50); });
    } finally { vi.useRealTimers(); }
    expect(rm).toHaveBeenCalledTimes(1);
    let done = false;
    const p = flush()!().then((r) => { done = true; return r; });
    await act(async () => { await Promise.resolve(); });
    expect(done).toBe(false);
    await act(async () => { release(); await p; });
    expect(done).toBe(true);
    expect(rm).toHaveBeenCalledTimes(1);
  });

  it("démontage : le retrait part tout de suite, un échec est remonté au parent", async () => {
    const rm = vi.fn(async () => { throw new Error("fichier verrouillé"); });
    const { view, onFlushError } = mount({ modsRemove: rm });
    await remove();
    view.unmount();
    expect(rm).toHaveBeenCalledTimes(1);
    await waitFor(() => expect(onFlushError).toHaveBeenCalledTimes(1));
    expect(onFlushError.mock.calls[0][0]).toMatch(/mod-maison\.jar.*fichier verrouillé/);
  });

  it("démontage sans échec : aucune erreur remontée", async () => {
    const rm = vi.fn(async () => {});
    const { view, onFlushError } = mount({ modsRemove: rm });
    await remove();
    view.unmount();
    await waitFor(() => expect(rm).toHaveBeenCalledTimes(1));
    await act(async () => { await Promise.resolve(); });
    expect(onFlushError).not.toHaveBeenCalled();
  });
});

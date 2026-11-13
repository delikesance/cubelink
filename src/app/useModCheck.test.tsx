import { afterEach, describe, expect, it, vi } from "vitest";
import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import type { ModCheck, ModIssue } from "../services";
import { useModCheckGate } from "./useModCheck";

afterEach(cleanup);

const issue = (n: number, severity: ModIssue["severity"] = "error"): ModIssue =>
  ({ file: `m${n}.jar`, mod_name: `Mod${n}`, kind: "missing_dep", severity, message: `souci ${n}` });

function Harness({ check, onPlay, onSee, onFix }: { check: () => Promise<ModCheck>; onPlay: () => void; onSee: () => void; onFix?: () => void }) {
  const gate = useModCheckGate({ modsCheck: check });
  return (
    <>
      <button onClick={() => gate.guard({ id: "i", name: "Pack" }, onPlay, onSee, onFix)}>go</button>
      {gate.modal}
    </>
  );
}
const run = (check: () => Promise<ModCheck>) => {
  const onPlay = vi.fn(), onSee = vi.fn(), onFix = vi.fn();
  render(<Harness check={check} onPlay={onPlay} onSee={onSee} onFix={onFix} />);
  fireEvent.click(screen.getByText("go"));
  return { onPlay, onSee, onFix };
};

describe("useModCheckGate", () => {
  it("erreur : ouvre la modale, « Jouer quand même » lance", async () => {
    const { onPlay } = run(async () => ({ issues: [1, 2, 3, 4, 5].map((n) => issue(n)), checked: 5 }));
    const dlg = await screen.findByRole("alertdialog", { name: "Ce pack risque de planter" });
    expect(dlg.textContent).toContain("5 problèmes de compatibilité détectés");
    expect(dlg.textContent).toContain("Mod3 : souci 3");
    expect(dlg.textContent).not.toContain("Mod4");
    expect(dlg.textContent).toContain("… et 2 autres");
    expect(onPlay).not.toHaveBeenCalled();
    fireEvent.click(screen.getByRole("button", { name: "Jouer quand même" }));
    expect(onPlay).toHaveBeenCalledTimes(1);
  });

  it("« Voir les mods » appelle le callback sans lancer", async () => {
    const { onPlay, onSee } = run(async () => ({ issues: [issue(1)], checked: 1 }));
    fireEvent.click(await screen.findByRole("button", { name: "Voir les mods" }));
    expect(onSee).toHaveBeenCalledTimes(1);
    expect(onPlay).not.toHaveBeenCalled();
    expect(screen.queryByRole("alertdialog")).toBeNull();
  });

  it("« Corriger » appelle onFix sans lancer", async () => {
    const { onPlay, onFix } = run(async () => ({ issues: [issue(1)], checked: 1 }));
    fireEvent.click(await screen.findByRole("button", { name: "Corriger" }));
    expect(onFix).toHaveBeenCalledTimes(1);
    expect(onPlay).not.toHaveBeenCalled();
    expect(screen.queryByRole("alertdialog")).toBeNull();
  });

  it("avertissements seuls : lancement direct", async () => {
    const { onPlay } = run(async () => ({ issues: [issue(1, "warning")], checked: 1 }));
    await waitFor(() => expect(onPlay).toHaveBeenCalledTimes(1));
    expect(screen.queryByRole("alertdialog")).toBeNull();
  });

  it("appel rejeté : lancement direct", async () => {
    const { onPlay } = run(() => Promise.reject(new Error("absent")));
    await waitFor(() => expect(onPlay).toHaveBeenCalledTimes(1));
  });
});

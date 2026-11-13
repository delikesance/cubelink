import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { keepsNativeMenu, useContextMenu, useSuppressNativeMenu } from "./useContextMenu";

function Demo({ onPlay }: { onPlay: () => void }) {
  const ctx = useContextMenu();
  useSuppressNativeMenu();
  return (
    <div>
      <div data-testid="tile" tabIndex={0} onContextMenu={ctx.bind("Actions de Pack", [
        { key: "play", label: "Jouer", onSelect: onPlay },
        { key: "del", label: "Supprimer", danger: true, disabled: true, onSelect: () => {} },
      ])}>Pack</div>
      <input aria-label="champ" />
      <p data-testid="plain">texte</p>
      {ctx.menu}
    </div>
  );
}

describe("clic droit", () => {
  afterEach(cleanup);
  it("ouvre le menu de l'élément et lance l'action", () => {
    const onPlay = vi.fn();
    render(<Demo onPlay={onPlay} />);
    const ev = fireEvent.contextMenu(screen.getByTestId("tile"), { clientX: 40, clientY: 50 });
    expect(ev).toBe(false); // menu natif annulé
    expect(screen.getByRole("menu", { name: "Actions de Pack" })).toBeTruthy();
    expect((screen.getByRole("menuitem", { name: "Supprimer" }) as HTMLButtonElement).disabled).toBe(true);
    fireEvent.click(screen.getByRole("menuitem", { name: "Jouer" }));
    expect(onPlay).toHaveBeenCalledOnce();
    expect(screen.queryByRole("menu")).toBeNull();
  });

  it("supprime le menu natif ailleurs, sauf dans un champ de saisie", () => {
    render(<Demo onPlay={() => {}} />);
    expect(fireEvent.contextMenu(screen.getByTestId("plain"))).toBe(false);
    expect(fireEvent.contextMenu(screen.getByLabelText("champ"))).toBe(true);
    expect(screen.queryByRole("menu")).toBeNull();
  });

  it("garde le menu natif dans les champs", () => {
    const input = document.createElement("input");
    expect(keepsNativeMenu(input)).toBe(true);
    expect(keepsNativeMenu(document.createElement("div"))).toBe(false);
  });
});

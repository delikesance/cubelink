import { act, cleanup, fireEvent, render, screen } from "@testing-library/react";
import { useState } from "react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { Modal } from "./Modal";

afterEach(cleanup);

function Nested({ a, b }: { a: () => void; b: () => void }) {
  const [inner, setInner] = useState(false);
  return (
    <Modal title="Dessous" onClose={a}>
      <button onClick={() => setInner(true)}>Ouvrir</button>
      {inner && <Modal title="Dessus" onClose={() => { b(); setInner(false); }}>x</Modal>}
    </Modal>
  );
}

describe("Modal : Échap avec deux modales empilées", () => {
  it("ne ferme que celle du dessus, puis celle du dessous au second Échap", () => {
    const a = vi.fn();
    const b = vi.fn();
    render(<Nested a={a} b={b} />);
    fireEvent.click(screen.getByText("Ouvrir"));
    expect(screen.getAllByRole("dialog")).toHaveLength(2);
    fireEvent.keyDown(document, { key: "Escape" });
    expect(b).toHaveBeenCalledTimes(1);
    expect(a).not.toHaveBeenCalled();
    expect(screen.getAllByRole("dialog")).toHaveLength(1);
    fireEvent.keyDown(document, { key: "Escape" });
    expect(a).toHaveBeenCalledTimes(1);
    expect(b).toHaveBeenCalledTimes(1);
  });

  it("une modale démontée ne bloque pas Échap pour les autres", () => {
    const a = vi.fn();
    const { rerender } = render(<><Modal title="A" onClose={a}>x</Modal><Modal title="B" onClose={() => {}}>y</Modal></>);
    // Le dessous est démonté dans le désordre : la pile doit rester saine.
    rerender(<Modal title="B" onClose={a}>y</Modal>);
    act(() => { fireEvent.keyDown(document, { key: "Escape" }); });
    expect(a).toHaveBeenCalledTimes(1);
  });
});

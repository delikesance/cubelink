import { afterEach, describe, expect, it, vi } from "vitest";
import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { useState } from "react";
import { RAM_DEFAULT, RamPicker, ramLabel } from "./RamPicker";

function Harness({ initial, onChange }: { initial: number; onChange: (mb: number) => void }) {
  const [v, setV] = useState(initial);
  return <RamPicker value={v} defaultMb={8192} onChange={(mb) => { setV(mb); onChange(mb); }} />;
}

describe("RamPicker", () => {
  afterEach(cleanup);
  it("par défaut : suit la valeur des paramètres, sans curseur", () => {
    render(<Harness initial={RAM_DEFAULT} onChange={() => {}} />);
    expect((screen.getByRole("checkbox", { name: /Valeur des paramètres \(8 Go\)/ }) as HTMLInputElement).checked).toBe(true);
    expect(screen.queryByRole("slider")).toBeNull();
    expect(screen.getByText("8 Go", { selector: "output" })).toBeTruthy();
  });

  it("décocher : valeur propre (partant du défaut), puis retour au défaut", () => {
    const changed = vi.fn();
    render(<Harness initial={RAM_DEFAULT} onChange={changed} />);
    fireEvent.click(screen.getByRole("checkbox"));
    expect(changed).toHaveBeenLastCalledWith(8192);
    fireEvent.change(screen.getByRole("slider", { name: "Mémoire allouée (Go)" }), { target: { value: "12" } });
    expect(changed).toHaveBeenLastCalledWith(12288);
    expect(screen.getByText("12 Go", { selector: "output" })).toBeTruthy();
    fireEvent.click(screen.getByRole("checkbox"));
    expect(changed).toHaveBeenLastCalledWith(RAM_DEFAULT);
    expect(screen.queryByRole("slider")).toBeNull();
  });

  it("libellés en Go", () => {
    expect(ramLabel(8192)).toBe("8 Go");
    expect(ramLabel(1536)).toBe("1,5 Go");
  });
});

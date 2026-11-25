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

  it("réglage global Automatique : « Automatique » au lieu de 0 Mo, et la valeur propre part de 4 Go", () => {
    const changed = vi.fn();
    render(<RamPicker value={RAM_DEFAULT} defaultMb={0} onChange={changed} />);
    expect(screen.getByRole("checkbox", { name: /Valeur des paramètres \(Automatique\)/ })).toBeTruthy();
    expect(screen.getByText("Automatique", { selector: "output" })).toBeTruthy();
    expect(screen.queryByText(/0 Go|0 Mo/)).toBeNull();
    fireEvent.click(screen.getByRole("checkbox"));
    expect(changed).toHaveBeenLastCalledWith(4096);
  });

  it("avertit (sans bloquer) quand la valeur manuelle dépasse 75 % de la RAM du PC", () => {
    const { rerender } = render(<RamPicker value={24576} defaultMb={0} systemMb={16384} onChange={() => {}} />);
    expect(screen.getByText(/Attention/).textContent).toMatch(/24 Go dépassent 75\s%\sde la mémoire de ce PC \(16 Go\)/);
    expect(screen.getByRole("slider")).toBeTruthy();
    rerender(<RamPicker value={8192} defaultMb={0} systemMb={16384} onChange={() => {}} />);
    expect(screen.queryByText(/Attention/)).toBeNull();
    rerender(<RamPicker value={RAM_DEFAULT} defaultMb={0} systemMb={16384} onChange={() => {}} />);
    expect(screen.queryByText(/Attention/)).toBeNull();
    rerender(<RamPicker value={24576} defaultMb={0} systemMb={null} onChange={() => {}} />);
    expect(screen.queryByText(/Attention/)).toBeNull();
  });

  it("libellés en Go", () => {
    expect(ramLabel(8192)).toBe("8 Go");
    expect(ramLabel(1536)).toBe("1,5 Go");
  });
});

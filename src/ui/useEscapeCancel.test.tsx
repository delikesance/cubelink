import { cleanup, fireEvent, render } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { useEscapeCancel } from "./useEscapeCancel";

afterEach(cleanup);

function Probe({ active, onCancel }: { active: boolean; onCancel: () => void }) {
  useEscapeCancel(active, onCancel);
  return <button type="button">x</button>;
}

describe("useEscapeCancel", () => {
  it("annule sur Échap et empêche la propagation vers les écouteurs du document", () => {
    const onCancel = vi.fn();
    const bubble = vi.fn();
    document.addEventListener("keydown", bubble);
    const { getByRole } = render(<Probe active onCancel={onCancel} />);
    fireEvent.keyDown(getByRole("button"), { key: "Escape" });
    document.removeEventListener("keydown", bubble);
    expect(onCancel).toHaveBeenCalledTimes(1);
    expect(bubble).not.toHaveBeenCalled();
  });

  it("ignore les autres touches et ne fait rien tant qu'inactif", () => {
    const onCancel = vi.fn();
    const bubble = vi.fn();
    document.addEventListener("keydown", bubble);
    const { rerender } = render(<Probe active onCancel={onCancel} />);
    fireEvent.keyDown(document, { key: "Enter" });
    rerender(<Probe active={false} onCancel={onCancel} />);
    fireEvent.keyDown(document, { key: "Escape" });
    document.removeEventListener("keydown", bubble);
    expect(onCancel).not.toHaveBeenCalled();
    expect(bubble).toHaveBeenCalledTimes(2);
  });

  it("utilise le dernier onCancel sans se réabonner", () => {
    const first = vi.fn();
    const second = vi.fn();
    const { rerender } = render(<Probe active onCancel={first} />);
    rerender(<Probe active onCancel={second} />);
    fireEvent.keyDown(document, { key: "Escape" });
    expect(first).not.toHaveBeenCalled();
    expect(second).toHaveBeenCalledTimes(1);
  });
});

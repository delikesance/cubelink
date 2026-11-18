import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, expect, it, vi } from "vitest";
import { BackButton } from "./BackButton";

afterEach(cleanup);

it("bouton retour : même bouton secondaire de taille standard partout, libellé court par défaut", () => {
  const onClick = vi.fn();
  render(<><BackButton onClick={onClick} /><BackButton label="Accueil" onClick={onClick} /></>);
  const [a, b] = [screen.getByRole("button", { name: "Retour" }), screen.getByRole("button", { name: "Accueil" })];
  for (const el of [a, b]) {
    expect(el.className).toContain("cl-btn-secondary");
    expect(el.className).not.toContain("cl-btn-lg");
    expect(el.querySelector("svg")).toBeTruthy();
  }
  fireEvent.click(b);
  expect(onClick).toHaveBeenCalledTimes(1);
});

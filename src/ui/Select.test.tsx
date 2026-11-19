import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, expect, it, vi } from "vitest";
import { Select } from "./Select";

afterEach(cleanup);
const options = [{ value: "a", label: "Alpha" }, { value: "b", label: "Bravo" }, { value: "c", label: "Charlie" }];

function setup(onChange = vi.fn(), value = "a") {
  render(<><label htmlFor="s">Choix</label><Select id="s" value={value} options={options} onChange={onChange} /></>);
  return { onChange, box: screen.getByLabelText("Choix") };
}

it("affiche le libellé de la valeur, liste fermée, relié à son label", () => {
  const { box } = setup();
  expect(box.textContent).toBe("Alpha");
  expect(box.getAttribute("role")).toBe("combobox");
  expect(box.getAttribute("aria-expanded")).toBe("false");
  expect(screen.queryByRole("listbox")).toBeNull();
});

it("souris : ouvre, marque l'option courante, choisit et referme", () => {
  const { onChange, box } = setup();
  fireEvent.click(box);
  expect(screen.getByRole("option", { name: "Alpha" }).getAttribute("aria-selected")).toBe("true");
  fireEvent.click(screen.getByRole("option", { name: "Charlie" }));
  expect(onChange).toHaveBeenCalledWith("c");
  expect(screen.queryByRole("listbox")).toBeNull();
});

it("clavier : flèches + Entrée, Échap annule, une lettre saute à l'option", () => {
  const { onChange, box } = setup();
  fireEvent.keyDown(box, { key: "ArrowDown" });
  fireEvent.keyDown(box, { key: "ArrowDown" });
  fireEvent.keyDown(box, { key: "Enter" });
  expect(onChange).toHaveBeenCalledWith("b");
  fireEvent.keyDown(box, { key: "Enter" });
  fireEvent.keyDown(box, { key: "c" });
  fireEvent.keyDown(box, { key: "Escape" });
  expect(screen.queryByRole("listbox")).toBeNull();
  expect(onChange).toHaveBeenCalledTimes(1);
  fireEvent.keyDown(box, { key: "Enter" });
  fireEvent.keyDown(box, { key: "c" });
  fireEvent.keyDown(box, { key: "Enter" });
  expect(onChange).toHaveBeenLastCalledWith("c");
});

it("clic à l'extérieur ferme ; désactivé n'ouvre pas", () => {
  const { box } = setup();
  fireEvent.click(box);
  fireEvent.mouseDown(document.body);
  expect(screen.queryByRole("listbox")).toBeNull();
  cleanup();
  render(<Select aria-label="x" value="a" options={options} onChange={() => {}} disabled />);
  fireEvent.click(screen.getByLabelText("x"));
  expect(screen.queryByRole("listbox")).toBeNull();
});

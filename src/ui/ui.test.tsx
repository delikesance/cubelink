import { act, cleanup, fireEvent, render, screen } from "@testing-library/react";
import { useState } from "react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { Badge, Button, Card, Drawer, Field, IconButton, Input, ProgressBar, StatusDot, Tooltip } from "./index";

afterEach(cleanup);

describe("Button", () => {
  it("variantes et taille lg", () => {
    render(<><Button variant="danger">Suppr</Button><Button size="lg">Jouer</Button></>);
    expect(screen.getByRole("button", { name: "Suppr" }).className).toContain("cl-btn-danger");
    expect(screen.getByRole("button", { name: "Jouer" }).className).toContain("cl-btn-lg");
  });
  it("loading : aria-busy, clic ignore, reste focalisable", () => {
    const f = vi.fn();
    render(<Button loading onClick={f}>Envoi</Button>);
    const b = screen.getByRole("button", { name: "Envoi" });
    expect(b.getAttribute("aria-busy")).toBe("true");
    fireEvent.click(b);
    expect(f).not.toHaveBeenCalled();
    b.focus();
    expect(document.activeElement).toBe(b);
  });
  it("disabled natif", () => {
    const f = vi.fn();
    render(<Button disabled onClick={f}>X</Button>);
    fireEvent.click(screen.getByRole("button", { name: "X" }));
    expect(f).not.toHaveBeenCalled();
  });
});

describe("IconButton", () => {
  it("aria-label et title", () => {
    render(<IconButton aria-label="Copier" icon={<svg />} />);
    expect(screen.getByRole("button", { name: "Copier" }).getAttribute("title")).toBe("Copier");
  });
});

describe("Field", () => {
  it("relie label, aide et erreur", () => {
    render(<Field label="Nom" hint="3 lettres" error="Trop court">{(p) => <Input {...p} />}</Field>);
    const i = screen.getByLabelText("Nom");
    expect(i.getAttribute("aria-invalid")).toBe("true");
    expect(i.getAttribute("aria-describedby")!.split(" ")).toHaveLength(2);
    expect(screen.getByRole("alert").textContent).toBe("Trop court");
  });
});

describe("Card", () => {
  it("rend l'element demande", () => {
    render(<Card as="section" aria-label="c">x</Card>);
    expect(screen.getByRole("region", { name: "c" })).toBeTruthy();
  });
});

describe("Drawer", () => {
  function Host() {
    const [o, setO] = useState(false);
    return (
      <>
        <button onClick={() => setO(true)}>Ouvrir</button>
        <Drawer open={o} onClose={() => setO(false)} title="Amis"><button>A</button><button>B</button></Drawer>
      </>
    );
  }
  it("ouvre, piege le focus, ferme avec Echap et rend le focus", () => {
    vi.useFakeTimers();
    render(<Host />);
    const opener = screen.getByText("Ouvrir");
    opener.focus();
    fireEvent.click(opener);
    expect(screen.getByRole("dialog", { name: "Amis" }).getAttribute("aria-modal")).toBe("true");
    expect(document.activeElement).toBe(screen.getByText("A"));
    screen.getByText("B").focus();
    fireEvent.keyDown(screen.getByText("B"), { key: "Tab" });
    expect(document.activeElement).toBe(screen.getByText("A"));
    fireEvent.keyDown(screen.getByText("A"), { key: "Tab", shiftKey: true });
    expect(document.activeElement).toBe(screen.getByText("B"));
    fireEvent.keyDown(screen.getByText("B"), { key: "Escape" });
    act(() => { vi.advanceTimersByTime(400); });
    expect(screen.queryByRole("dialog")).toBeNull();
    expect(document.activeElement).toBe(opener);
    vi.useRealTimers();
  });
  it("clic sur le voile ferme", () => {
    const f = vi.fn();
    render(<Drawer open onClose={f} title="T">x</Drawer>);
    fireEvent.click(screen.getByTestId("drawer-veil"));
    expect(f).toHaveBeenCalled();
  });
});

describe("ProgressBar", () => {
  it("determine et indetermine", () => {
    render(<><ProgressBar label="Install" value={42} showValue /><ProgressBar label="Attente" /></>);
    expect(screen.getByRole("progressbar", { name: "Install" }).getAttribute("aria-valuenow")).toBe("42");
    expect(screen.getByText("42 %")).toBeTruthy();
    expect(screen.getByRole("progressbar", { name: "Attente" }).hasAttribute("aria-valuenow")).toBe(false);
  });
  it("borne la valeur", () => {
    render(<ProgressBar label="x" value={250} />);
    expect(screen.getByRole("progressbar").getAttribute("aria-valuenow")).toBe("100");
  });
});

describe("Tooltip", () => {
  it("apparait au focus, disparait a Echap", () => {
    render(<Tooltip text="Copier le code"><button>Btn</button></Tooltip>);
    const b = screen.getByRole("button", { name: "Btn" });
    fireEvent.focus(b);
    expect(screen.getByRole("tooltip").textContent).toBe("Copier le code");
    expect(b.getAttribute("aria-describedby")).toBe(screen.getByRole("tooltip").id);
    fireEvent.keyDown(b, { key: "Escape" });
    expect(screen.queryByRole("tooltip")).toBeNull();
  });
});

describe("Status", () => {
  it("libelle texte toujours present", () => {
    render(<><StatusDot tone="ok" label="En ligne" hideLabel /><Badge tone="warn">Bientôt</Badge></>);
    expect(screen.getByText("En ligne")).toBeTruthy();
    expect(screen.getByText("Bientôt").className).toContain("cl-badge-warn");
  });
});

import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it } from "vitest";
import { Shell } from "../../app/shell";
import { createFakeServices } from "../../services/fake";

afterEach(() => { cleanup(); window.location.hash = ""; });

describe("Amis : onglets", () => {
  it("pattern WAI-ARIA : flèches, Home, End, tabIndex itinérant", () => {
    window.location.hash = "#/friends";
    render(<Shell services={createFakeServices({ tickMs: 0 })} />);
    const [t1, t2] = screen.getAllByRole("tab");
    expect(t1.getAttribute("tabindex")).toBe("0");
    expect(t2.getAttribute("tabindex")).toBe("-1");
    t1.focus();
    fireEvent.keyDown(t1, { key: "ArrowRight" });
    expect(t2.getAttribute("aria-selected")).toBe("true");
    expect(document.activeElement).toBe(t2);
    expect(t2.getAttribute("tabindex")).toBe("0");
    fireEvent.keyDown(t2, { key: "Home" });
    expect(document.activeElement).toBe(t1);
    fireEvent.keyDown(t1, { key: "End" });
    expect(document.activeElement).toBe(t2);
  });
});

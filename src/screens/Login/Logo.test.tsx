import { describe, expect, it } from "vitest";
import { render } from "@testing-library/react";
import { Logo } from "./Logo";

describe("Logo", () => {
  it("rend un svg décoratif (aria-hidden) avec corps, morceau et tranches", () => {
    const { container } = render(<Logo width={44} />);
    const svg = container.querySelector("svg")!;
    expect(svg.getAttribute("aria-hidden")).toBe("true");
    expect(svg.querySelectorAll("path").length).toBe(4);
  });
  it("variante petite taille sous 48 px : trait plus fin", () => {
    const small = render(<Logo width={32} />).container.querySelector("path")!;
    const large = render(<Logo width={160} />).container.querySelector("path")!;
    expect(small.getAttribute("stroke-width")).toBe("10");
    expect(large.getAttribute("stroke-width")).toBe("12");
  });
  it("morceau éloigné et ombre agrandissent le cadre", () => {
    const base = render(<Logo width={100} />).container.querySelector("svg")!;
    const far = render(<Logo width={100} far shadow />).container.querySelector("svg")!;
    expect(Number(far.getAttribute("height"))).toBeGreaterThan(Number(base.getAttribute("height")));
    expect(far.querySelector("ellipse")).not.toBeNull();
  });
});

import { useEffect, useRef } from "react";
import "./legal.css";
import { useBack } from "../../app/context";
import { SECTIONS } from "./sections";

const TABS = [
  { id: "compte", label: "Compte" },
  { id: "reseau", label: "Réseau" },
  { id: "java", label: "Java" },
  { id: "stockage", label: "Stockage" },
  { id: "apropos", label: "À propos" },
] as const;

export default function Legal() {
  const back = useBack("settings");
  const backRef = useRef(back);
  backRef.current = back;
  const title = useRef<HTMLHeadingElement>(null);
  useEffect(() => { title.current?.focus(); }, []);
  useEffect(() => {
    const onEsc = (e: globalThis.KeyboardEvent) => {
      if (e.key === "Escape" && !e.defaultPrevented) { e.preventDefault(); backRef.current(); }
    };
    document.addEventListener("keydown", onEsc);
    return () => document.removeEventListener("keydown", onEsc);
  }, []);

  return (
    <div className="legal">
      <h1 className="enter" ref={title} tabIndex={-1}>Réglages</h1>
      <nav className="legal-nav enter" aria-label="Sections des réglages" style={{ "--i": 1 } as React.CSSProperties}>
        {TABS.map((t) => (
          <a key={t.id} className="legal-tab" href={`#/settings?tab=${t.id}`} aria-current={t.id === "apropos" ? "page" : undefined}>{t.label}</a>
        ))}
        <div className="legal-bar" aria-hidden="true" />
      </nav>
      <article className="legal-article enter" style={{ "--i": 2 } as React.CSSProperties} aria-labelledby="leg-title">
        <h2 id="leg-title">Mentions légales et confidentialité</h2>
        {SECTIONS.map((s) => (
          <section key={s.id} className="legal-section" id={`leg-${s.id}`}>{s.content}</section>
        ))}
      </article>
    </div>
  );
}

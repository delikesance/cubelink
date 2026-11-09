import { useRef, useState, type KeyboardEvent } from "react";
import "./legal.css";
import { useShell } from "../../app/context";
import { SECTIONS } from "./sections";

const Chevron = () => (
  <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true"><path d="M15 5l-7 7 7 7" /></svg>
);
const Cross = () => (
  <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" aria-hidden="true"><path d="M6 6l12 12M18 6L6 18" /></svg>
);

export default function Legal() {
  const { navigate } = useShell();
  const [active, setActive] = useState(0);
  const tabs = useRef<(HTMLButtonElement | null)[]>([]);

  const select = (i: number) => {
    const n = (i + SECTIONS.length) % SECTIONS.length;
    setActive(n);
    tabs.current[n]?.focus();
  };
  const onKey = (e: KeyboardEvent) => {
    const keys: Record<string, number> = {
      ArrowDown: active + 1, ArrowRight: active + 1, ArrowUp: active - 1, ArrowLeft: active - 1,
      Home: 0, End: SECTIONS.length - 1,
    };
    if (e.key in keys) { e.preventDefault(); select(keys[e.key]); }
  };
  const current = SECTIONS[active];

  return (
    <div className="legal">
      <div className="legal-card" role="region" aria-labelledby="leg-title">
        <header className="legal-head">
          <div className="legal-head-l">
            <button type="button" className="legal-btn" aria-label="Retour aux paramètres" onClick={() => navigate("settings")}>
              <Chevron />Retour
            </button>
            <h1 id="leg-title">Mentions légales et confidentialité</h1>
          </div>
          <button type="button" className="legal-btn" aria-label="Fermer" onClick={() => navigate("main")}>
            <Cross />
          </button>
        </header>
        <div className="legal-body">
          <div className="legal-nav" role="tablist" aria-label="Sommaire" aria-orientation="vertical" onKeyDown={onKey}>
            {SECTIONS.map((s, i) => (
              <button
                key={s.id}
                ref={(el) => { tabs.current[i] = el; }}
                type="button"
                role="tab"
                id={`leg-tab-${s.id}`}
                aria-selected={i === active}
                aria-controls={`leg-panel-${s.id}`}
                tabIndex={i === active ? 0 : -1}
                className="legal-tab"
                onClick={() => setActive(i)}
              >
                {s.label}
              </button>
            ))}
          </div>
          <section
            key={current.id}
            className="legal-pane"
            role="tabpanel"
            id={`leg-panel-${current.id}`}
            aria-labelledby={`leg-tab-${current.id}`}
          >
            {current.content}
          </section>
        </div>
      </div>
    </div>
  );
}

import { BackButton } from "../../ui";
import { useEffect, useRef } from "react";
import "./legal.css";
import { useBack } from "../../app/context";
import { SECTIONS } from "./sections";

/** Mentions légales, dans l'appli : bouton Retour (ou Échap) vers l'écran d'origine, Paramètres par défaut. */
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
    <div className="legal screen">
      <div className="legal-top enter">
        <BackButton onClick={() => back()} />
      </div>
      <article className="legal-article enter" style={{ "--i": 1 } as React.CSSProperties} aria-labelledby="leg-title">
        <h1 id="leg-title" ref={title} tabIndex={-1}>Mentions légales et confidentialité</h1>
        {SECTIONS.map((s) => (
          <section key={s.id} className="legal-section" id={`leg-${s.id}`}>{s.content}</section>
        ))}
      </article>
    </div>
  );
}

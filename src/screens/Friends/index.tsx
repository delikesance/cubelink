// Écran Amis (C4 à C6) : titre + pastille, deux cartes « Ajouter un ami » / « Mon code », puis onglets Tes amis / Demandes.
import { useEffect, useRef, useState, type CSSProperties, type KeyboardEvent } from "react";
import { useBack, useServices } from "../../app/context";
import { Card } from "../../ui";
import "./friends.css";
import MyCode from "./MyCode";
import Requests, { RequestsBadge, useRequestsTotal } from "./Requests";
import Add from "./Add";
import FriendsList from "./FriendsList";
import Invites from "./Invites";

type Tab = "friends" | "requests";

const stagger = (i: number) => ({ "--i": i }) as CSSProperties;

export default function Friends() {
  const services = useServices();
  const [tab, setTab] = useState<Tab>("friends");
  const [online, setOnline] = useState(0);
  const total = useRequestsTotal();
  const h1 = useRef<HTMLHeadingElement>(null);
  const back = useBack("main");
  const backRef = useRef(back);
  backRef.current = back;

  useEffect(() => { h1.current?.focus(); }, []);

  useEffect(() => {
    let alive = true;
    const refresh = () => services.friendList().then(
      (l) => { if (alive) setOnline(l.filter((f) => f.presence === "online").length); },
      () => {},
    );
    void refresh();
    const off = services.onFriendPresence(() => { void refresh(); });
    return () => { alive = false; off(); };
  }, [services]);

  // Échap = retour, sauf si un champ ou un autre composant a déjà traité la touche.
  useEffect(() => {
    const onEsc = (e: globalThis.KeyboardEvent) => {
      if (e.key !== "Escape" || e.defaultPrevented) return;
      const t = e.target as HTMLElement | null;
      if (t && /^(INPUT|TEXTAREA|SELECT)$/.test(t.tagName)) return;
      e.preventDefault();
      backRef.current();
    };
    document.addEventListener("keydown", onEsc);
    return () => document.removeEventListener("keydown", onEsc);
  }, []);

  const tabs: { id: Tab; label: React.ReactNode }[] = [
    { id: "friends", label: "Tes amis" },
    { id: "requests", label: <>Demandes <RequestsBadge total={total} /></> },
  ];
  const refs = useRef<Record<string, HTMLButtonElement | null>>({});
  // Pattern WAI-ARIA tabs : flèches, Home, End ; tabIndex itinérant.
  const onKey = (e: KeyboardEvent<HTMLButtonElement>, i: number) => {
    let n = i;
    if (e.key === "ArrowRight" || e.key === "ArrowDown") n = (i + 1) % tabs.length;
    else if (e.key === "ArrowLeft" || e.key === "ArrowUp") n = (i + tabs.length - 1) % tabs.length;
    else if (e.key === "Home") n = 0;
    else if (e.key === "End") n = tabs.length - 1;
    else return;
    e.preventDefault();
    setTab(tabs[n].id);
    refs.current[tabs[n].id]?.focus();
  };

  return (
    <div className="fr">
      <div className="fr-page">
        <header className="fr-head enter">
          <button type="button" className="fr-back" aria-label="Retour à l'accueil" onClick={back}>
            <svg width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true"><path d="M15 5l-7 7 7 7" /></svg>
            Retour
          </button>
          <h1 className="fr-title" ref={h1} tabIndex={-1}>Amis</h1>
          <span className="fr-pill"><span className={online ? "fr-dot on" : "fr-dot"} aria-hidden="true" />Amis · {online} en ligne</span>
        </header>
        <div className="fr-cards">
          <Card as="div" className="fr-card enter" style={stagger(1)}><Add /></Card>
          <Card as="div" className="fr-card enter" style={stagger(2)}><MyCode /></Card>
        </div>
        <Invites />
        <div role="tablist" aria-label="Amis" className="fr-tabs enter" style={stagger(3)}>
          {tabs.map((t, i) => (
            <button
              key={t.id}
              role="tab"
              type="button"
              id={`friends-tab-${t.id}`}
              aria-selected={tab === t.id}
              aria-controls={`friends-panel-${t.id}`}
              tabIndex={tab === t.id ? 0 : -1}
              className="fr-tab"
              ref={(el) => { refs.current[t.id] = el; }}
              onClick={() => setTab(t.id)}
              onKeyDown={(e) => onKey(e, i)}
            >
              {t.label}
            </button>
          ))}
        </div>
        <div role="tabpanel" id={`friends-panel-${tab}`} aria-labelledby={`friends-tab-${tab}`} className="fr-panel">
          {tab === "friends" ? <FriendsList /> : <Requests />}
        </div>
      </div>
    </div>
  );
}

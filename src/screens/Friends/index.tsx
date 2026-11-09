// Écran Amis : bandeau fixe « Mon code ami » au-dessus d'onglets (propriété du team lead ; blocs U8, U9, U10).
import { useRef, useState, type KeyboardEvent } from "react";
import "./friends.css";
import MyCode from "./MyCode";
import Requests, { RequestsBadge, useRequestsTotal } from "./Requests";
import Add from "./Add";

type Tab = "requests" | "add";

export default function Friends() {
  const [tab, setTab] = useState<Tab>("requests");
  const total = useRequestsTotal();
  const tabs: { id: Tab; label: React.ReactNode }[] = [
    { id: "requests", label: <>Demandes <RequestsBadge total={total} /></> },
    { id: "add", label: "Ajouter un ami" },
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
      <MyCode />
      <div role="tablist" aria-label="Amis" className="fr-tabs">
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
      <div
        role="tabpanel"
        id={`friends-panel-${tab}`}
        aria-labelledby={`friends-tab-${tab}`}
        className="fr-panel"
      >
        {tab === "requests" ? <Requests /> : <Add />}
      </div>
    </div>
  );
}

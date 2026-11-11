import { useCallback, useEffect, useRef, useState } from "react";
import "./requests.css";
import { useServices } from "../../app/context";
import { Button } from "../../ui";
import type { FriendRequest, Services } from "../../services/types";

const PAGE = 20;
const DEBOUNCE_MS = 250;

function messageOf(e: unknown): string {
  if (e instanceof Error && e.message) return e.message;
  if (typeof e === "string" && e) return e;
  return "Une erreur est survenue. Réessaie dans un instant.";
}

/** Texte du badge de l'onglet : « 99+ » au-delà de 99, rien à 0. */
export function badgeLabel(total: number): string {
  return total > 99 ? "99+" : total > 0 ? String(total) : "";
}

/** Nombre total de demandes en attente, tenu à jour par l'événement friend://request. */
export function useRequestsTotal(): number {
  const services = useServices();
  const [total, setTotal] = useState(0);
  useEffect(() => {
    let alive = true;
    services.friendRequests({ offset: 0, limit: 1 })
      .then((p) => { if (alive) setTotal(p.total); })
      .catch(() => {});
    const off = services.onFriendRequest(() => setTotal((t) => t + 1));
    return () => { alive = false; off(); };
  }, [services]);
  return total;
}

/** Pastille de l'onglet « Demandes » (à placer dans le libellé de l'onglet). */
export function RequestsBadge({ total }: { total: number }) {
  const label = badgeLabel(total);
  if (!label) return null;
  return (
    <span className="rq-badge" aria-label={`${total} demande${total > 1 ? "s" : ""} en attente`}>{label}</span>
  );
}

function Avatar({ name }: { name: string }) {
  return <span className="rq-avatar" aria-hidden="true">{name.slice(0, 1).toUpperCase()}</span>;
}

export default function Requests() {
  const services: Services = useServices();
  const [items, setItems] = useState<FriendRequest[]>([]);
  const [total, setTotal] = useState(0);
  const [grand, setGrand] = useState(0);
  const [loading, setLoading] = useState(true);
  const [more, setMore] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [input, setInput] = useState("");
  const [query, setQuery] = useState("");
  const [openKey, setOpenKey] = useState<string | null>(null);
  const [busyKey, setBusyKey] = useState<string | null>(null);
  const [confirming, setConfirming] = useState(false);
  const [bulkBusy, setBulkBusy] = useState(false);
  const [notice, setNotice] = useState("");
  const [compared, setCompared] = useState<string | null>(null);
  const seq = useRef(0);
  const queryRef = useRef("");
  const cancelRef = useRef<HTMLButtonElement>(null);
  queryRef.current = query;

  useEffect(() => {
    const t = setTimeout(() => setQuery(input.trim()), DEBOUNCE_MS);
    return () => clearTimeout(t);
  }, [input]);

  const load = useCallback(async () => {
    const id = ++seq.current;
    setLoading(true);
    setError(null);
    try {
      const page = await services.friendRequests({ offset: 0, limit: PAGE, query: query || undefined });
      if (id !== seq.current) return;
      setItems(page.items);
      setTotal(page.total);
      if (!query) setGrand(page.total);
      setOpenKey(page.items[0]?.public_key ?? null);
    } catch (e) {
      if (id === seq.current) setError(messageOf(e));
    } finally {
      if (id === seq.current) setLoading(false);
    }
  }, [services, query]);

  useEffect(() => { void load(); }, [load]);

  useEffect(() => services.onFriendRequest((r) => {
    setGrand((g) => g + 1);
    const q = queryRef.current.toLowerCase();
    if (!q || r.name.toLowerCase().includes(q)) {
      setTotal((t) => t + 1);
      setItems((list) => (list.some((x) => x.public_key === r.public_key) ? list : [r, ...list]));
      setOpenKey((k) => k ?? r.public_key);
    }
    setNotice(`Nouvelle demande de ${r.name}.`);
  }), [services]);

  useEffect(() => { if (confirming) cancelRef.current?.focus(); }, [confirming]);

  const loadMore = async () => {
    const id = seq.current;
    setMore(true);
    setError(null);
    try {
      const page = await services.friendRequests({ offset: items.length, limit: PAGE, query: query || undefined });
      if (id !== seq.current) return;
      setItems((list) => {
        const seen = new Set(list.map((x) => x.public_key));
        return [...list, ...page.items.filter((x) => !seen.has(x.public_key))];
      });
      setTotal(page.total);
      setNotice(`${page.items.length} demandes de plus affichées.`);
    } catch (e) {
      setError(messageOf(e));
    } finally {
      setMore(false);
    }
  };

  const answer = async (r: FriendRequest, accept: boolean) => {
    setBusyKey(r.public_key);
    setError(null);
    try {
      if (accept) await services.friendAccept(r.public_key);
      else await services.friendRefuse(r.public_key);
      const idx = items.findIndex((x) => x.public_key === r.public_key);
      const rest = items.filter((x) => x.public_key !== r.public_key);
      setItems(rest);
      setTotal((t) => Math.max(0, t - 1));
      setGrand((g) => Math.max(0, g - 1));
      setOpenKey((k) => (k === r.public_key ? (rest[idx] ?? rest[idx - 1] ?? null)?.public_key ?? null : k));
      setNotice(accept ? `${r.name} est maintenant ton ami.` : `Demande de ${r.name} refusée.`);
    } catch (e) {
      setError(messageOf(e));
    } finally {
      setBusyKey(null);
    }
  };

  const refuseAll = async () => {
    setBulkBusy(true);
    setError(null);
    try {
      const n = await services.friendRefuseAll();
      seq.current++;
      setItems([]);
      setTotal(0);
      setGrand(0);
      setOpenKey(null);
      setConfirming(false);
      setNotice(`${n} demande${n > 1 ? "s" : ""} refusée${n > 1 ? "s" : ""}.`);
    } catch (e) {
      setError(messageOf(e));
    } finally {
      setBulkBusy(false);
    }
  };

  const isCompared = compared !== null && compared === openKey;
  const remaining = Math.max(0, total - items.length);
  const current = items.find((x) => x.public_key === openKey) ?? null;
  const others = items.filter((x) => x.public_key !== openKey);
  const empty = !loading && items.length === 0 && !error;

  return (
    <section className="rq" aria-labelledby="rq-title">
      <h2 id="rq-title" className="rq-title">
        Demandes reçues <RequestsBadge total={grand} />
      </h2>

      <div className="rq-bar">
        <input
          type="search"
          className="rq-search"
          aria-label="Chercher une demande"
          placeholder="Chercher un nom"
          value={input}
          onChange={(e) => setInput(e.target.value)}
        />
        {grand > 0 && !confirming && (
          <button type="button" className="rq-link" onClick={() => setConfirming(true)}>Tout refuser</button>
        )}
      </div>

      {confirming && (
        <div className="rq-confirm" role="group" aria-label="Confirmer le refus de toutes les demandes">
          <p>Refuser les {grand} demandes&nbsp;? Cette action est définitive.</p>
          <div className="rq-actions">
            <button type="button" className="rq-btn" disabled={bulkBusy} onClick={() => { void refuseAll(); }}>
              {bulkBusy ? "Refus…" : `Refuser les ${grand} demandes`}
            </button>
            <button type="button" className="rq-ghost" ref={cancelRef} disabled={bulkBusy} onClick={() => setConfirming(false)}>
              Annuler
            </button>
          </div>
        </div>
      )}

      {error && (
        <div className="rq-error" role="alert">
          <span>{error}</span>
          <button type="button" className="rq-ghost" onClick={() => { void load(); }}>Réessayer</button>
        </div>
      )}

      {loading && <p className="rq-info" role="status">Chargement des demandes…</p>}

      {empty && (
        <p className="rq-info">{query ? "Aucune demande ne correspond à cette recherche." : "Aucune demande en attente."}</p>
      )}

      {current && (
        <section className="rq-open" aria-labelledby="rq-open-title">
          <div className="rq-head">
            <Avatar name={current.name} />
            <h3 id="rq-open-title">Demande de {current.name}</h3>
          </div>
          <p className="rq-text">
            {current.name} veut t&apos;ajouter. Cubelink ne peut pas prouver son identité&nbsp;: compare ce code avec lui par
            un autre canal (Discord, SMS) avant d&apos;accepter.
          </p>
          <div className="rq-row">
            <code className="rq-verify" aria-label="Code de vérification">{current.verify_code}</code>
          </div>
          <label className="rq-compare">
            <input type="checkbox" checked={isCompared} onChange={(e) => setCompared(e.target.checked ? current.public_key : null)} />
            <span>J&apos;ai comparé ce code avec {current.name}</span>
          </label>
          <div className="rq-actions">
            <Button disabled={!isCompared || busyKey === current.public_key} loading={busyKey === current.public_key}
              aria-label={`Accepter ${current.name}`} onClick={() => { void answer(current, true); }}>Accepter</Button>
            <Button variant="secondary" disabled={busyKey === current.public_key}
              aria-label={`Refuser ${current.name}`} onClick={() => { void answer(current, false); }}>Refuser</Button>
          </div>
          {!isCompared && <p className="rq-hint">Coche la case une fois le code comparé pour pouvoir accepter.</p>}
        </section>
      )}

      {others.length > 0 && (
        <ul className="rq-list">
          {others.map((r) => (
            <li key={r.public_key} className="rq-item">
              <Avatar name={r.name} />
              <div className="rq-who">
                <span className="rq-name">{r.name}</span>
                <span className="rq-sub">veut t&apos;ajouter</span>
              </div>
              <button type="button" className="rq-ghost" aria-label={`Voir la demande de ${r.name}`}
                onClick={() => setOpenKey(r.public_key)}>Voir</button>
            </li>
          ))}
        </ul>
      )}

      {remaining > 0 && (
        <button type="button" className="rq-link rq-more" disabled={more} onClick={() => { void loadMore(); }}>
          {more ? "Chargement…" : `Voir les ${remaining} autres demandes`}
        </button>
      )}

      <div className="rq-live" aria-live="polite" aria-atomic="true">{notice}</div>
    </section>
  );
}

import { useEffect, useId, useRef, useState, type KeyboardEvent } from "react";
import { PixelHead } from "../screens/Main/Pixel";
import type { AccountInfo, Services } from "../services/types";
import { useShell } from "./context";
import { useAccounts } from "./useAccounts";

/** Tête du skin d'un compte Microsoft (8×8, agrandie sans lissage) ; tête générée pour un compte hors ligne ou sans réseau. */
export function SkinHead({ services, account, size }: { services: Services; account: AccountInfo; size: number }) {
  const [src, setSrc] = useState<string | null>(null);
  useEffect(() => {
    setSrc(null);
    if (account.kind !== "online") return;
    let alive = true;
    services.authSkinHead(account.uuid).then((s) => { if (alive) setSrc(s); }, () => {});
    return () => { alive = false; };
  }, [services, account.kind, account.uuid]);
  return src
    ? <img className="acc-head px-img" src={src} width={size} height={size} alt="" />
    : <PixelHead className="acc-head" seed={account.uuid || account.name} size={size} />;
}

/** Carte de profil (bas du panneau de l'accueil) : compte actif, changer de compte en deux clics, menu ouvert vers le haut. */
export function AccountSwitcher() {
  const { services, route, navigate } = useShell();
  const { state, refresh } = useAccounts(services);
  const [open, setOpen] = useState(false);
  const [error, setError] = useState("");
  const uid = useId();
  const root = useRef<HTMLDivElement>(null);
  const trigger = useRef<HTMLButtonElement>(null);
  const items = useRef<Array<HTMLButtonElement | null>>([]);

  const close = (refocus: boolean) => {
    setOpen(false);
    if (refocus) trigger.current?.focus();
  };

  // Ferme sur clic extérieur et à chaque changement de route.
  useEffect(() => { setOpen(false); setError(""); }, [route]);
  useEffect(() => {
    if (!open) return;
    const onDown = (e: MouseEvent) => { if (!root.current?.contains(e.target as Node)) setOpen(false); };
    document.addEventListener("mousedown", onDown);
    return () => document.removeEventListener("mousedown", onDown);
  }, [open]);
  useEffect(() => {
    if (!open) return;
    const cur = items.current.findIndex((el) => el?.getAttribute("aria-checked") === "true");
    items.current[cur >= 0 ? cur : 0]?.focus();
  }, [open]);

  if (route === "login" || !state || state.accounts.length === 0) return null;
  const active = state.accounts.find((a) => a.id === state.active) ?? null;

  const pick = async (id: string) => {
    if (id === state.active) { close(true); return; }
    try {
      await services.authSwitch(id);
      setError("");
      close(true);
      void refresh();
    } catch (e) {
      setError(e instanceof Error && e.message ? e.message : "Impossible de changer de compte.");
    }
  };

  const onMenuKey = (e: KeyboardEvent<HTMLDivElement>) => {
    const list = items.current.filter((x): x is HTMLButtonElement => !!x);
    const i = list.indexOf(document.activeElement as HTMLButtonElement);
    let n = i;
    if (e.key === "ArrowDown") n = (i + 1) % list.length;
    else if (e.key === "ArrowUp") n = (i + list.length - 1) % list.length;
    else if (e.key === "Home") n = 0;
    else if (e.key === "End") n = list.length - 1;
    else if (e.key === "Escape") { e.preventDefault(); e.stopPropagation(); close(true); return; }
    else if (e.key === "Tab") { setOpen(false); return; }
    else return;
    e.preventDefault();
    list[n]?.focus();
  };

  return (
    <div className="acc-sw" ref={root}>
      <button
        ref={trigger} type="button" className="acc-trigger"
        aria-haspopup="menu" aria-expanded={open} aria-controls={`${uid}-menu`}
        aria-label={active ? `Compte actif : ${active.name}. Changer de compte` : "Choisir un compte"}
        onClick={() => setOpen((o) => !o)}
      >
        {active ? <SkinHead services={services} account={active} size={40} /> : <PixelHead className="acc-head" seed="?" size={40} />}
        <span className="acc-txt">
          <span className="acc-name">{active?.name ?? "Aucun compte"}</span>
          <span className="acc-kind">{active?.kind === "offline" ? "Hors ligne" : "Compte Microsoft"} · changer</span>
        </span>
      </button>
      {open && (
        <div id={`${uid}-menu`} className="acc-menu" role="menu" aria-label="Changer de compte" onKeyDown={onMenuKey}>
          {state.accounts.map((a, i) => (
            <button
              key={a.id} type="button" role="menuitemradio" aria-checked={a.id === state.active}
              ref={(el) => { items.current[i] = el; }} className="acc-item" onClick={() => { void pick(a.id); }}
            >
              <SkinHead services={services} account={a} size={24} />
              <span className="acc-name">{a.name}</span>
              {a.kind === "offline" && <span className="acc-tag">Hors ligne</span>}
              {a.id === state.active && (
                <svg className="acc-ok" width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true"><path d="M5 13l4 4L19 7" /></svg>
              )}
            </button>
          ))}
          <button
            type="button" role="menuitem" className="acc-item acc-manage"
            ref={(el) => { items.current[state.accounts.length] = el; }}
            onClick={() => { setOpen(false); navigate("settings", { tab: "compte" }); }}
          >Gérer les comptes…</button>
          {error && <p className="acc-err" role="alert">{error}</p>}
        </div>
      )}
    </div>
  );
}

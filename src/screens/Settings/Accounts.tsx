import { useEffect, useRef, useState } from "react";
import { useServices, useShell } from "../../app/context";
import { useAccounts } from "../../app/useAccounts";
import { Button, Card } from "../../ui";
import { SkinHead } from "../../app/AccountSwitcher";
import { ICONS, Icon } from "../Main/Panel";
import { OfflineForm } from "../Login/OfflineForm";
import { messageOf, useDeviceLogin } from "../Login/useDeviceLogin";
import { formatCountdown } from "../Login";

type Adding = "none" | "microsoft" | "offline";

/** Réglages > Compte : liste des comptes, changement de compte actif, retrait et ajout (Microsoft ou hors ligne). */
export function Accounts({ onError }: { onError: (message: string | null) => void }) {
  const services = useServices();
  const { navigate } = useShell();
  const { state, refresh } = useAccounts(services);
  const [adding, setAdding] = useState<Adding>("none");
  const [confirming, setConfirming] = useState<string | null>(null);
  const [busy, setBusy] = useState<string | null>(null);
  const [notice, setNotice] = useState("");
  const cancelRef = useRef<HTMLButtonElement>(null);
  const alive = useRef(true);
  useEffect(() => { alive.current = true; return () => { alive.current = false; }; }, []);

  const ms = useDeviceLogin(services);
  const msStep = ms.state.step;
  const userCode = ms.state.step === "code" ? ms.state.code.user_code : "";

  // Compte Microsoft ajouté : on referme le panneau et on l'annonce.
  useEffect(() => {
    if (msStep !== "connected" || ms.state.step !== "connected") return;
    setNotice(`Compte ${ms.state.profile.name} ajouté.`);
    setAdding("none");
    ms.cancel();
    void refresh();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [msStep]);

  useEffect(() => {
    if (confirming) cancelRef.current?.focus();
  }, [confirming]);

  if (!state) return <Card as="section" aria-label="Comptes" className="acc-card"><p className="set-hint" role="status">Chargement des comptes…</p></Card>;

  const act = async (id: string, fn: () => Promise<unknown>, done: string) => {
    setBusy(id);
    onError(null);
    setNotice("");
    try {
      await fn();
      if (!alive.current) return;
      setNotice(done);
      await refresh();
    } catch (e) {
      if (alive.current) onError(messageOf(e));
    }
    if (alive.current) setBusy(null);
  };

  const remove = async (id: string, name: string) => {
    setConfirming(null);
    setBusy(id);
    onError(null);
    setNotice("");
    try {
      await services.authRemove(id);
      const next = await services.authAccounts();
      if (!alive.current) return;
      if (next.accounts.length === 0) { navigate("login"); return; }
      setNotice(`Compte ${name} retiré.`);
      await refresh();
    } catch (e) {
      if (alive.current) onError(messageOf(e));
    }
    if (alive.current) setBusy(null);
  };

  const toggleAdding = (what: Adding) => {
    ms.cancel();
    setAdding((cur) => (cur === what ? "none" : what));
    setNotice("");
    if (what === "microsoft" && adding !== "microsoft") void ms.start();
  };

  return (
    <Card as="section" aria-label="Comptes" className="acc-card">
      {state.accounts.length === 0 ? (
        <p className="set-warn">Aucun compte connecté</p>
      ) : (
        <ul className="acc-list">
          {state.accounts.map((a) => {
            const isActive = a.id === state.active;
            const ask = confirming === a.id;
            return (
              <li key={a.id} className="acc-row" data-active={isActive || undefined}>
                <SkinHead services={services} account={a} size={40} />
                <div className="acc-who">
                  <div className="name"><span className="acc-nm">{a.name}</span>{isActive && <span className="acc-active">Actif</span>}</div>
                  <div className="acc-meta">{a.kind === "offline" ? "Hors ligne" : "Compte Microsoft"}</div>
                </div>
                {ask ? (
                  <div className="acc-confirm" role="group" aria-label={`Retirer ${a.name}`}>
                    <span>Retirer ce compte&nbsp;?</span>
                    <Button variant="danger" aria-label={`Confirmer le retrait de ${a.name}`} onClick={() => { void remove(a.id, a.name); }}>Retirer</Button>
                    <Button variant="secondary" ref={cancelRef} onClick={() => setConfirming(null)}>Annuler</Button>
                  </div>
                ) : (
                  <div className="acc-acts">
                    {!isActive && (
                      <Button variant="secondary" aria-label={`Utiliser ${a.name}`} loading={busy === a.id}
                        onClick={() => { void act(a.id, () => services.authSwitch(a.id), `Compte actif : ${a.name}.`); }}>Utiliser</Button>
                    )}
                    <button type="button" className="acc-remove" aria-label={`Retirer ${a.name}`} title="Retirer" disabled={busy === a.id} onClick={() => setConfirming(a.id)}>
                      <Icon d={ICONS.trash} size={18} />
                    </button>
                  </div>
                )}
              </li>
            );
          })}
        </ul>
      )}
      <div role="status" aria-live="polite" className="set-hint acc-notice">{notice}</div>

      <div className="acc-add">
        <button type="button" className="acc-addbtn" aria-expanded={adding === "microsoft"} onClick={() => toggleAdding("microsoft")}><Icon d={ICONS.plus} size={18} />Ajouter un compte Microsoft</button>
        <button type="button" className="acc-addbtn" aria-expanded={adding === "offline"} aria-controls="acc-offline" onClick={() => toggleAdding("offline")}><Icon d={ICONS.plus} size={18} />Ajouter un compte hors ligne</button>
      </div>

      {adding === "microsoft" && (
        <div className="acc-panel enter" aria-label="Connexion Microsoft" role="group">
          {(msStep === "starting") && <p className="set-hint" role="status">Demande du code…</p>}
          {ms.state.step === "code" && (
            <>
              <p className="set-hint">Entre ce code sur la page Microsoft. Cubelink se connecte tout seul une fois validé.</p>
              <p className="acc-code" aria-label={`Code : ${ms.state.code.user_code.split("").join(" ")}`}>{ms.state.code.user_code}</p>
              <p className="set-hint" data-testid="acc-countdown">Expire dans {formatCountdown(ms.left)}</p>
              <div className="set-acts">
                <Button variant="secondary" onClick={() => { void ms.copy(userCode); }}>{ms.copied ? "Copié" : "Copier"}</Button>
                <a className="cl-btn cl-btn-primary" href={ms.state.code.verification_uri} target="_blank" rel="noopener noreferrer">
                  Ouvrir {ms.state.code.verification_uri.replace(/^https?:\/\//, "")}
                </a>
                <Button variant="secondary" onClick={() => toggleAdding("microsoft")}>Annuler</Button>
              </div>
            </>
          )}
          {ms.state.step === "expired" && (
            <>
              <p className="set-error" role="alert">Le code a expiré. Demande un nouveau code pour te connecter.</p>
              <div className="set-acts"><Button onClick={() => { void ms.start(); }}>Réessayer</Button></div>
            </>
          )}
          {ms.state.step === "error" && (
            <>
              <p className="set-error" role="alert">{ms.state.message}</p>
              <div className="set-acts"><Button onClick={() => { void ms.start(); }}>Réessayer</Button></div>
            </>
          )}
        </div>
      )}

      {adding === "offline" && (
        <OfflineForm
          id="acc-offline" submitLabel="Ajouter"
          onCancel={() => setAdding("none")}
          onSubmit={async (name) => {
            const p = await services.authOfflineLogin(name);
            if (!alive.current) return;
            setNotice(`Compte ${p.name} ajouté.`);
            setAdding("none");
            await refresh();
          }}
        />
      )}
    </Card>
  );
}

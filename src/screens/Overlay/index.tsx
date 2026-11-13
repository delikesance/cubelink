// Panneau « en jeu » : bande à droite du jeu (la fenêtre du jeu y est découpée, le launcher apparaît dessous).
// Invitations reçues, amis (inviter / rejoindre) et changement de modpack.
import { useCallback, useEffect, useRef, useState } from "react";
import "./overlay.css";
import "../Friends/avatar.css";
import { useServices } from "../../app/context";
import { canInviteFrom, hostingKeys, useHostState, useInvites } from "../../app/useInvites";
import { Badge, Button } from "../../ui";
import { Avatar } from "../Friends/Avatar";
import type { FriendWithPresence, Instance, Invite } from "../../services/types";
import { setStagePanel } from "../../app/stage";

type Confirm =
  | { kind: "join"; publicKey: string; name: string }
  | { kind: "switch"; instance: Instance }
  | { kind: "close"; instance: Instance };

function messageOf(e: unknown): string {
  if (e instanceof Error && e.message) return e.message;
  if (typeof e === "string" && e) return e;
  return "Une erreur est survenue. Réessaie dans un instant.";
}

/**
 * `open` : le panneau reste monté (déjà dessiné) sous la fenêtre du jeu tant que le jeu est affiché ; la découpe de la fenêtre
 * du jeu le révèle alors sans flash. Fermé, il ne charge rien, ne prend ni le focus ni la touche Échap.
 */
export default function InGamePanel({ open = true }: { open?: boolean }) {
  const services = useServices();
  const [friends, setFriends] = useState<FriendWithPresence[]>([]);
  const [instances, setInstances] = useState<Instance[]>([]);
  const [running, setRunning] = useState(false);
  /** Instance du jeu en cours : son bouton ferme le jeu au lieu de le lancer. */
  const [current, setCurrent] = useState<string | null>(null);
  const [confirm, setConfirm] = useState<Confirm | null>(null);
  const [busy, setBusy] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [notice, setNotice] = useState("");
  const invites = useInvites(services);
  const host = useHostState(services);
  const canInvite = canInviteFrom(host);
  const hosting = hostingKeys(invites);
  const titleRef = useRef<HTMLHeadingElement>(null);
  const cancelRef = useRef<HTMLButtonElement>(null);
  const confirmRef = useRef<Confirm | null>(null);
  confirmRef.current = confirm;

  const close = useCallback(() => { void setStagePanel(false).catch(() => {}); }, []);

  // Listes rechargées à chaque ouverture.
  useEffect(() => {
    if (!open) return;
    let alive = true;
    services.friendList().then((l) => { if (alive) setFriends(l); }, () => {});
    services.instancesList().then((l) => { if (alive) setInstances(l); }, () => {});
    services.gameCurrent().then((id) => { if (alive) setCurrent(id); }, () => {});
    return () => { alive = false; };
  }, [services, open]);

  // Fermé : une confirmation ou une erreur en cours ne survit pas jusqu'à la prochaine ouverture.
  useEffect(() => {
    if (!open) { setConfirm(null); setError(null); }
  }, [open]);

  useEffect(() => {
    setRunning(true); // le panneau n'existe que pendant une partie
    const offs = [
      services.onFriendPresence((e) => setFriends((l) => l.map((f) => (f.friend.public_key === e.public_key ? { ...f, presence: e.presence } : f)))),
      services.onLaunchState((s) => {
        if (s.state === "running" || s.state === "preparing") {
          if (s.state === "running") setRunning(true);
          setCurrent(s.id);
        } else if (s.state === "exited" || s.state === "failed") {
          setRunning(false);
          setCurrent((c) => (c === s.id ? null : c));
        }
      }),
    ];
    return () => { offs.forEach((o) => o()); };
  }, [services]);

  // Le panneau prend le focus clavier à l'ouverture (ouverture et retour de focus de la fenêtre).
  useEffect(() => {
    if (!open) return;
    titleRef.current?.focus();
    const onFocus = () => {
      const a = document.activeElement;
      if (!a || a === document.body) titleRef.current?.focus();
    };
    window.addEventListener("focus", onFocus);
    return () => window.removeEventListener("focus", onFocus);
  }, [open]);

  // Échap : annule une confirmation en cours, sinon ferme le panneau.
  useEffect(() => {
    if (!open) return;
    const onKey = (e: KeyboardEvent) => {
      if (e.key !== "Escape" || e.defaultPrevented) return;
      e.preventDefault();
      if (confirmRef.current) { setConfirm(null); titleRef.current?.focus(); } else close();
    };
    document.addEventListener("keydown", onKey);
    return () => document.removeEventListener("keydown", onKey);
  }, [close, open]);

  useEffect(() => { if (confirm) cancelRef.current?.focus(); }, [confirm]);

  const run = async (key: string, fn: () => Promise<void>, ok?: string) => {
    setBusy(key);
    setError(null);
    try {
      await fn();
      if (ok) setNotice(ok);
    } catch (e) {
      setError(messageOf(e));
    } finally {
      setBusy(null);
    }
  };

  const join = (publicKey: string, name: string) => {
    if (running) setConfirm({ kind: "join", publicKey, name });
    else void run(`join-${publicKey}`, () => services.gameJoinFriend(publicKey));
  };
  const dismiss = (i: Invite) => run(`dismiss-${i.id}`, () => services.friendInviteDismiss(i.id), `Invitation de ${i.name} ignorée.`);
  const invite = (f: FriendWithPresence) =>
    run(`invite-${f.friend.public_key}`, () => services.friendInvite(f.friend.public_key), `Invitation envoyée à ${f.friend.name}.`);
  const launch = (inst: Instance) => {
    if (running) setConfirm({ kind: "switch", instance: inst });
    else void run(`switch-${inst.id}`, () => services.gameSwitch(inst.id));
  };

  const doConfirm = () => {
    const c = confirm;
    if (!c) return;
    setConfirm(null);
    if (c.kind === "join") void run(`join-${c.publicKey}`, () => services.gameJoinFriend(c.publicKey));
    else if (c.kind === "close") void run(`close-${c.instance.id}`, () => services.gameClose());
    else void run(`switch-${c.instance.id}`, () => services.gameSwitch(c.instance.id));
    titleRef.current?.focus();
  };

  const online = friends.filter((f) => f.presence === "online");
  const offline = friends.filter((f) => f.presence !== "online");

  return (
    <section className="ov" aria-labelledby="ov-title">
      <header className="ov-head">
        <h1 id="ov-title" className="ov-title" ref={titleRef} tabIndex={-1}>Cubelink</h1>
        <button type="button" className="ov-close" aria-label="Fermer le panneau" onClick={close}>
          <svg width="12" height="12" viewBox="0 0 10 10" aria-hidden="true"><path d="M0 0l10 10M10 0L0 10" stroke="currentColor" fill="none" /></svg>
        </button>
      </header>

      <div className="ov-body">
        {error && <p className="ov-error" role="alert">{error}</p>}

        <section className="ov-sec" aria-labelledby="ov-inv">
          <h2 id="ov-inv">Invitations{invites.length > 0 ? ` · ${invites.length}` : ""}</h2>
          {invites.length === 0 ? <p className="ov-empty">Aucune invitation en attente.</p> : (
            <ul className="ov-list" aria-label="Invitations">
              {invites.map((i) => (
                <li key={i.id} className="ov-row ov-row-inv">
                  <Avatar name={i.name} size={36} services={services} />
                  <div className="ov-who">
                    <span className="ov-name"><span>{i.name}</span></span>
                    <span className="ov-sub">{i.world_name} · {i.minecraft}</span>
                  </div>
                  <div className="ov-actions">
                    <Button aria-label={`Rejoindre ${i.name}`} loading={busy === `join-${i.public_key}`} onClick={() => join(i.public_key, i.name)}>Rejoindre</Button>
                    <Button variant="secondary" aria-label={`Ignorer l'invitation de ${i.name}`} disabled={busy === `dismiss-${i.id}`} onClick={() => { void dismiss(i); }}>Ignorer</Button>
                  </div>
                </li>
              ))}
            </ul>
          )}
        </section>

        <section className="ov-sec" aria-labelledby="ov-fr">
          <h2 id="ov-fr">Amis{friends.length > 0 ? ` · ${online.length} en ligne` : ""}</h2>
          {friends.length === 0 ? <p className="ov-empty">Aucun ami pour l'instant.</p> : (
            <ul className="ov-list" aria-label="Amis">
              {[...online, ...offline].map((f) => {
                const on = f.presence === "online";
                const pk = f.friend.public_key;
                return (
                  <li key={pk} className="ov-row">
                    <Avatar name={f.friend.name} size={36} services={services} presence={on ? "online" : "offline"} />
                    <div className="ov-who">
                      <span className="ov-name">
                        <span>{f.friend.name}</span>
                        {hosting.has(pk) && on && <Badge tone="accent">Héberge</Badge>}
                      </span>
                      <span className="ov-sub">{on ? "En ligne" : "Hors ligne"}</span>
                    </div>
                    <div className="ov-actions">
                      {on && hosting.has(pk) && (
                        <Button aria-label={`Rejoindre ${f.friend.name}`} loading={busy === `join-${pk}`} onClick={() => join(pk, f.friend.name)}>Rejoindre</Button>
                      )}
                      {on && canInvite && (
                        <Button variant="secondary" aria-label={`Inviter ${f.friend.name}`} loading={busy === `invite-${pk}`} onClick={() => { void invite(f); }}>Inviter</Button>
                      )}
                    </div>
                  </li>
                );
              })}
            </ul>
          )}
        </section>

        <section className="ov-sec" aria-labelledby="ov-mp">
          <h2 id="ov-mp">Modpacks</h2>
          {instances.length === 0 ? <p className="ov-empty">Aucune instance.</p> : (
            <ul className="ov-list" aria-label="Modpacks">
              {instances.map((inst) => (
                <li key={inst.id} className="ov-row">
                  <div className="ov-who">
                    <span className="ov-name"><span>{inst.name}</span></span>
                    <span className="ov-sub">{inst.minecraft} · {inst.loader.loader}</span>
                  </div>
                  <div className="ov-actions">
                    {inst.id === current ? (
                      <Button variant="danger" aria-label={`Fermer ${inst.name}`} loading={busy === `close-${inst.id}`} onClick={() => setConfirm({ kind: "close", instance: inst })}>Fermer</Button>
                    ) : (
                      <Button variant="secondary" aria-label={`Lancer ${inst.name}`} loading={busy === `switch-${inst.id}`} onClick={() => launch(inst)}>Lancer</Button>
                    )}
                  </div>
                </li>
              ))}
            </ul>
          )}
        </section>
      </div>

      {confirm && (
        <div className="ov-confirm" role="group" aria-label="Confirmation">
          <p>
            {confirm.kind === "join"
              ? `Fermer le jeu en cours et rejoindre ${confirm.name} ? Les changements non sauvegardés seront perdus.`
              : confirm.kind === "close"
                ? `Fermer ${confirm.instance.name} ? Le jeu enregistre le monde avant de se fermer.`
                : `Fermer le jeu et lancer ${confirm.instance.name} ? Les changements non sauvegardés seront perdus.`}
          </p>
          <div className="ov-actions">
            <Button variant={confirm.kind === "close" ? "danger" : undefined} onClick={doConfirm}>
              {confirm.kind === "join" ? "Fermer et rejoindre" : confirm.kind === "close" ? "Fermer le jeu" : "Fermer et lancer"}
            </Button>
            <Button variant="secondary" ref={cancelRef} onClick={() => { setConfirm(null); titleRef.current?.focus(); }}>Annuler</Button>
          </div>
        </div>
      )}
      <footer className="ov-foot">Échap pour retourner au jeu</footer>
      <div className="ov-live" role="status" aria-live="polite">{notice}</div>
    </section>
  );
}

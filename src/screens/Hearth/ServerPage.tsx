import { useEffect, useRef, useState } from "react";
import "./hearth.css";
import { useShell } from "../../app/context";
import { useHearthDetail } from "../../app/useHearth";
import { useAccounts } from "../../app/useAccounts";
import { HearthLeaveError } from "../../services/types";
import type { FriendWithPresence, HearthDetail, HearthProposal, Instance, Services } from "../../services/types";
import { Select, BackButton, Badge, Button, Card, Icon, ICONS, StatusDot } from "../../ui";
import { Avatar } from "../Friends/Avatar";
import { Modal } from "../Main/Modal";
import { crashText, voiceText, errText, fmtDate, fmtPlayed, fmtSize, mainAction, plural, resumeLabel, stateHint, stateText } from "./format";
import { useHearthRun } from "./usePlay";

const TABS = [["membres", "Membres"], ["historique", "Historique"], ["reglages", "Réglages"]] as const;
type Tab = (typeof TABS)[number][0];

const PROPOSAL_LABEL = (p: HearthProposal): string => {
  const who = p.subject?.name ?? "un membre";
  switch (p.kind) {
    case "continue_without": return `Continuer sans ${who}`;
    case "add_member": return `Ajouter ${who} au groupe`;
    case "remove_member": return `Retirer ${who} du groupe`;
    case "pack_update": return "Mettre à jour le modpack du groupe";
    case "set_governance": return "Passer les décisions à la majorité";
    case "leave": return `${who} quitte le groupe`;
  }
};

const SYNC_LABEL = { ok: "À jour", behind: "En retard", unknown: "Pas encore vérifié" } as const;

/** `run` peut rendre une seconde confirmation à enchaîner (elle remplace celle-ci au lieu de la fermer). */
interface Confirm { title: string; text: string; action: string; danger?: boolean; busyText?: string; run: () => Promise<void | Confirm> }

/** Boîte de confirmation d'une action qu'on ne peut pas défaire d'un clic. */
function ConfirmDialog({ c, onClose }: { c: Confirm; onClose: (next?: Confirm) => void }) {
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const go = async () => {
    setBusy(true);
    try { const next = await c.run(); setBusy(false); setError(null); onClose(next ?? undefined); }
    catch (e) { setError(errText(e)); setBusy(false); }
  };
  return (
    <Modal title={c.title} role="alertdialog" onClose={() => onClose()}>
      <p className="home-modal-text" role={busy && c.busyText ? "status" : undefined}>{busy && c.busyText ? c.busyText : c.text}</p>
      {error && <p className="hearth-err" role="alert">{error}</p>}
      <div className="home-modal-foot">
        <Button variant="secondary" data-autofocus onClick={() => onClose()}>Annuler</Button>
        <Button variant={c.danger ? "danger" : "primary"} loading={busy} onClick={() => { void go(); }}>{c.action}</Button>
      </div>
    </Modal>
  );
}

function Proposals({ d, services, onError }: { d: HearthDetail; services: Services; onError: (m: string) => void }) {
  if (d.proposals.length === 0) return null;
  return (
    <section aria-label="Décisions du groupe" className="hearth-block">
      <h2>Décisions en attente</h2>
      <ul className="hearth-rows">
        {d.proposals.map((p) => (
          <li key={p.id} className="hearth-row">
            <span className="hearth-row-txt"><strong>{PROPOSAL_LABEL(p)}</strong><span className="hearth-meta">{p.approvals} sur {p.needed} accords nécessaires</span></span>
            {p.i_voted ? <Badge tone="ok">Tu as voté</Badge> : (
              <Button variant="secondary" disabled={!p.can_vote} onClick={() => { services.hearthVote(d.card.id, p.id).catch((e: unknown) => onError(errText(e))); }}>Approuver</Button>
            )}
          </li>
        ))}
      </ul>
    </section>
  );
}

function Members({ d, services, friends, ask, onError, leave }: { d: HearthDetail; services: Services; friends: FriendWithPresence[]; ask: (c: Confirm) => void; onError: (m: string) => void; leave: () => void }) {
  const id = d.card.id;
  // Ma tête : le skin de MON compte actif (le libellé « Toi » n'est pas mon pseudo). Compte hors ligne : tête générée.
  const { state: accounts } = useAccounts(services);
  const active = accounts?.accounts.find((a) => a.id === accounts.active);
  const myskin: { by: "uuid" | null; id: string } = active ? { by: active.kind === "online" ? "uuid" : null, id: active.uuid } : { by: null, id: "" };
  const addable = friends.filter((f) => !d.members.some((m) => m.public_key === f.friend.public_key));
  const [toAdd, setToAdd] = useState("");
  const [adding, setAdding] = useState(false);
  const add = async () => {
    const pk = toAdd || addable[0]?.friend.public_key;
    if (!pk) return;
    setAdding(true);
    try { await services.hearthAddMember(id, pk); setToAdd(""); }
    catch (e) { onError(errText(e)); }
    finally { setAdding(false); }
  };
  return (
    <>
      {d.blocker && (
        <div className="hearth-banner" role="status">
          <Icon d={ICONS.alert} size={20} />
          <span className="hearth-row-txt">
            <strong>{d.blocker.member.name} avait la version la plus récente et n'est pas revenu depuis {plural(d.blocker.days, "jour", "jours")}.</strong>
            <span className="hearth-meta">Le groupe peut continuer sans lui avec la version qu'il a : la majorité doit être d'accord.</span>
          </span>
          <Button variant="secondary" onClick={() => { services.hearthContinueWithout(id, d.blocker!.member.public_key).catch((e: unknown) => onError(errText(e))); }}>
            Continuer sans {d.blocker.member.name}
          </Button>
        </div>
      )}
      <Proposals d={d} services={services} onError={onError} />
      <section aria-label="Membres" className="hearth-block">
        <h2 className="hearth-sr">{plural(d.members.length, "membre", "membres")}</h2>
        <ul className="hearth-rows">
          {d.members.map((m) => (
            <li key={m.public_key} className="hearth-row">
              <Avatar name={m.name} size={36} presence={m.online ? "online" : "offline"} services={services} skin={m.is_me ? myskin : undefined} />
              <span className="hearth-row-txt">
                <strong>{m.name}{m.is_me ? " (toi)" : ""}{m.is_creator ? " · créateur" : ""}</strong>
                <span className="hearth-meta">
                  {m.online ? "En ligne" : m.unseen_days != null ? `Pas vu depuis ${plural(m.unseen_days, "jour", "jours")}` : "Hors ligne"}
                  {" · "}{SYNC_LABEL[m.sync]}{m.version != null ? ` (version ${m.version})` : ""}
                </span>
              </span>
              {!m.is_me && (
                <Button
                  variant="danger-quiet" aria-label={`Retirer ${m.name}`}
                  onClick={() => ask({
                    title: `Retirer ${m.name} ?`, action: "Retirer", danger: true,
                    text: `${m.name} ne pourra plus rejoindre ce serveur. Selon les règles du groupe, le retrait peut demander l'accord de la majorité.`,
                    run: () => services.hearthRemoveMember(id, m.public_key),
                  })}
                >Retirer</Button>
              )}
              {m.is_me && <Button variant="danger-quiet" onClick={leave}>Quitter</Button>}
            </li>
          ))}
        </ul>
        {addable.length > 0 && (
          <div className="hearth-add">
            <div className="cl-field">
              <Select aria-label="Ami à ajouter" value={toAdd || addable[0]!.friend.public_key} onChange={setToAdd}
                options={addable.map((f) => ({ value: f.friend.public_key, label: f.friend.name }))} />
            </div>
            <Button variant="secondary" loading={adding} icon={<Icon d={ICONS.userPlus} />} onClick={() => { void add(); }}>Ajouter au groupe</Button>
          </div>
        )}
      </section>
    </>
  );
}

function History({ d, services, ask }: { d: HearthDetail; services: Services; ask: (c: Confirm) => void }) {
  const KIND = { session: "Partie", checkpoint: "Point de reprise", restore: "Restauration" } as const;
  if (d.versions.length === 0) return <p className="hearth-empty">Aucune version pour l'instant : la première apparaîtra à la fin d'une partie.</p>;
  return (
    <ul className="hearth-rows" aria-label="Versions du monde">
      {d.versions.map((v) => (
        <li key={v.id} className="hearth-row">
          <span className="hearth-row-txt">
            <strong>Version {v.number} · {KIND[v.kind]}{v.is_head ? " · actuelle" : ""}</strong>
            <span className="hearth-meta">
              {v.author_name} · {fmtDate(v.created_at)} · {fmtPlayed(v.played_secs)} de jeu · {fmtSize(v.total_size)}
              {v.side_branch ? " · mise de côté" : ""}
              {!v.is_head && !v.available ? " · contenu libéré de ce PC" : ""}
            </span>
          </span>
          {!v.is_head && v.available && (
            <Button
              variant="secondary" aria-label={`Restaurer la version ${v.number}`}
              onClick={() => ask({
                title: `Restaurer la version ${v.number} ?`, action: "Restaurer",
                text: "Le monde du groupe redevient celui de cette version. Les versions plus récentes restent dans l'historique.",
                run: () => services.hearthRestore(d.card.id, v.id),
              })}
            >Restaurer</Button>
          )}
        </li>
      ))}
    </ul>
  );
}

function Settings({ d, services, instances, ask, onError }: { d: HearthDetail; services: Services; instances: Instance[]; ask: (c: Confirm) => void; onError: (m: string) => void }) {
  const id = d.card.id;
  const [inst, setInst] = useState("");
  const pick = inst || d.card.instance_id || instances[0]?.id || "";
  return (
    <div className="hearth-settings">
      <section className="hearth-block" aria-label="Modpack">
        <div className="hearth-block-head">
          <h2>Modpack</h2>
          <p className="hearth-meta">Instance utilisée sur ce PC : {d.settings.instance_name ?? "aucune"}. Les mods sont figés : une mise à jour doit être acceptée par le groupe.</p>
        </div>
        {instances.length > 0 && (
          <div className="hearth-add">
            <div className="cl-field">
              <Select aria-label="Instance à proposer" value={pick} onChange={setInst} options={instances.map((i) => ({ value: i.id, label: i.name }))} />
            </div>
            <Button
              variant="secondary"
              onClick={() => ask({
                title: "Proposer cette mise à jour ?", action: "Proposer au groupe",
                text: "La mise à jour du modpack n'est appliquée que si la majorité du groupe l'accepte.",
                run: () => services.hearthPackUpdate(id, pick),
              })}
            >Proposer la mise à jour au groupe</Button>
          </div>
        )}
      </section>
      <section className="hearth-block" aria-label="Serveur">
        <div className="hearth-block-head">
          <h2>Serveur</h2>
          <p className="hearth-meta">
            Place prise sur ce disque : {fmtSize(d.settings.disk_bytes)}.
            {d.settings.outbox_pending > 0 && !d.settings.outbox_failed && ` ${plural(d.settings.outbox_pending, "version attend", "versions attendent")} d'être envoyée aux autres membres.`}
          </p>
          {d.settings.outbox_failed && <p className="hearth-meta" role="status">{d.settings.outbox_failed}</p>}
        </div>
        <label className="hearth-check">
          <input type="checkbox" checked={d.settings.keep_background} onChange={(e) => { services.hearthSetBackground(id, e.target.checked).catch((x: unknown) => onError(errText(x))); }} />
          <span>Garder le serveur ouvert quand mon jeu se ferme</span>
        </label>
        <p className="hearth-hint">Sans joueur pendant 30 minutes, le serveur s'arrête tout seul, même ouvert.</p>
      </section>
    </div>
  );
}

/** Page d'un serveur de groupe : état et action principale en haut, puis Membres, Historique et Réglages. */
export default function ServerPage({ id }: { id: string }) {
  const { services, navigate } = useShell();
  const { detail, error: loadError, reload } = useHearthDetail(services, id);
  const [tab, setTab] = useState<Tab>("membres");
  const [instances, setInstances] = useState<Instance[]>([]);
  const [friends, setFriends] = useState<FriendWithPresence[]>([]);
  const [confirm, setConfirm] = useState<Confirm | null>(null);
  const [error, setError] = useState<string | null>(null);
  const { run, error: runError, busy, eula } = useHearthRun(services);
  const title = useRef<HTMLHeadingElement>(null);

  useEffect(() => {
    let alive = true;
    services.instancesList().then((l) => { if (alive) setInstances(l); }, () => {});
    services.friendList().then((l) => { if (alive) setFriends(l); }, () => {});
    return () => { alive = false; };
  }, [services]);
  useEffect(() => { if (detail) title.current?.focus({ preventScroll: true }); }, [detail?.card.id]);

  if (loadError) {
    return (
      <div className="hearth-page">
        <h1 ref={title} tabIndex={-1}>Serveur introuvable</h1>
        <p role="alert" className="hearth-err">{loadError}</p>
        <Button variant="secondary" onClick={() => navigate("main")}>Retour à l'accueil</Button>
      </div>
    );
  }
  if (!detail) return <div className="hearth-page"><p role="status">Chargement du serveur…</p></div>;

  const c = detail.card;
  const act = mainAction(c);
  const hint = stateHint(c, detail.members.filter((m) => !m.is_me && !m.online).map((m) => m.name));
  const wrap = (x: Confirm): Confirm => ({ ...x, run: async () => { const next = await x.run(); if (!next) reload(); return next; } });
  const ask = (x: Confirm) => setConfirm(wrap(x));
  // Suppression locale : seul choix explicite de l'utilisateur, jamais un repli automatique (#57). Rien n'est annoncé au groupe.
  const deleteWorld = (title: string, text: string, action: string): Confirm => wrap({
    title, text, action, danger: true,
    run: async () => { await services.hearthForget(c.id); navigate("main"); },
  });
  const lastMember = () => deleteWorld(
    `Tu es le dernier membre de « ${c.name} »`,
    "Personne d'autre n'a une copie du monde : quitter est impossible, et le supprimer efface la seule copie, sans retour possible.",
    "Supprimer le monde",
  );
  const unreachable = () => deleteWorld(
    "Personne n'est joignable",
    "Aucun ami du groupe n'est connecté : ton départ ne leur sera pas annoncé, et ils te compteront encore dans le groupe. "
      + (detail.settings.outbox_pending > 0 ? `${plural(detail.settings.outbox_pending, "partie n'a pas été envoyée et sera perdue", "parties n'ont pas été envoyées et seront perdues")}. ` : "")
      + "Supprimer quand même le monde de ce PC ? Tu pourras être réinvité plus tard.",
    "Supprimer quand même",
  );
  // Ma dernière partie n'a été remise à personne : quitter maintenant la perd pour le groupe (#56). Le serveur est déjà arrêté.
  const notSent = (): Confirm => wrap({
    title: "Ta dernière partie n'est pas encore partie",
    text: "Aucun ami ne l'a reçue : si tu quittes maintenant, elle sera perdue pour le groupe. Tu peux attendre qu'un ami soit connecté et réessayer.",
    action: "Quitter quand même", danger: true,
    busyText: "Quitter le groupe…",
    run: () => leaveNow(true),
  });
  // Une fin de partie non publiée, gardée de côté, part avec le monde : on le dit avant (#61). Le serveur est déjà arrêté.
  const unpublished = (): Confirm => wrap({
    title: "Une partie n'a pas été publiée",
    text: "Tu as une fin de partie gardée de côté sur ce PC, que personne d'autre n'a reçue. Si tu quittes maintenant, elle est supprimée avec le monde. Tu peux d'abord la publier ou la supprimer depuis cette page.",
    action: "Quitter quand même", danger: true,
    busyText: "Quitter le groupe…",
    run: () => leaveNow(true),
  });
  const leaveNow = async (force: boolean): Promise<void | Confirm> => {
    try { await services.hearthLeave(c.id, force); } catch (e) {
      // Seuls ces codes mènent à autre chose qu'une erreur affichée, et la suppression n'arrive qu'après une nouvelle
      // confirmation. Toute autre erreur est relancée : la boîte l'affiche et le monde reste en place.
      if (e instanceof HearthLeaveError && e.code === "last_member") return lastMember();
      if (e instanceof HearthLeaveError && e.code === "unreachable") return unreachable();
      if (e instanceof HearthLeaveError && e.code === "outbox_pending" && !force) return notSent();
      if (e instanceof HearthLeaveError && e.code === "orphan_pending" && !force) return unpublished();
      throw e;
    }
    navigate("main");
  };
  const leave = () => {
    // Seul dans le groupe : le texte « le serveur continue pour les autres » serait faux, et Quitter serait refusé.
    if (detail.members.length <= 1) { setConfirm(lastMember()); return; }
    ask({
      title: `Quitter « ${c.name} » ?`, action: "Quitter", danger: true,
      text: "Le monde copié sur ce PC est supprimé. Le serveur continue pour les autres membres, même si c'est toi qui l'as créé. Tu pourras être réinvité plus tard.",
      busyText: "Envoi de ta dernière partie aux autres membres avant de quitter… Ça peut prendre quelques minutes.",
      run: () => leaveNow(false),
    });
  };
  const shownError = error ?? runError;

  return (
    <div className="hearth-page">
      <header className="hearth-page-head">
        <BackButton label="Accueil" onClick={() => navigate("main")} />
        <div className="hearth-hero">
          <div className="hearth-hero-id">
            <h1 ref={title} tabIndex={-1}>{c.name}</h1>
            <p className="hearth-meta">{c.minecraft} · {c.loader}</p>
            <StatusDot tone={c.state === "hosting" ? "ok" : c.state === "idle" ? "neutral" : c.state === "hosted" ? "accent" : "warn"} label={stateText(c)} />
          </div>
          <div className="hearth-hero-cta">
            {act.kind === "play" && <Button size="lg" loading={busy} icon={<Icon d={ICONS.play} fill />} onClick={() => { void run(() => services.hearthPlay(c.id), c.id); }}>{act.label}</Button>}
            {act.kind === "stop" && <Button variant="secondary" loading={busy} onClick={() => { void run(() => services.hearthStop(c.id)); }}>{act.label}</Button>}
          </div>
          {hint && <p className="hearth-hint">{hint}</p>}
        </div>
        {shownError && <p className="hearth-err" role="alert">{shownError}</p>}
      </header>

      {c.state === "unclean" && c.unclean && (
        <div className="hearth-banner" role="status">
          <Icon d={ICONS.alert} size={20} />
          <span className="hearth-row-txt">
            <strong>La partie de {c.unclean.holder.name} s'est mal terminée.</strong>
            <span className="hearth-meta">{c.unclean.checkpoint_at != null ? `Dernier point de reprise : ${fmtDate(c.unclean.checkpoint_at)}.` : "Aucun point de reprise connu."}</span>
          </span>
          <Button loading={busy} onClick={() => { const r = resumeLabel(c); void run(() => services.hearthUncleanChoice(c.id, r.choice), c.id); }}>{resumeLabel(c).label}</Button>
          <Button variant="secondary" onClick={() => { void run(() => services.hearthUncleanChoice(c.id, "wait")); }}>Attendre</Button>
        </div>
      )}

      {c.crash && (
        <div className="hearth-banner" role="status">
          <Icon d={ICONS.alert} size={20} />
          <span className="hearth-row-txt">
            <strong>{crashText(c)}</strong>
            <span className="hearth-meta">
              {c.crash.mine
                ? "Le monde de cette partie n'a pas été publié : il est gardé de côté sur ce PC, à toi de choisir ce que tu en fais."
                : "Son monde n'a pas été publié : le groupe repart du dernier point de reprise."}
            </span>
          </span>
        </div>
      )}

      {voiceText(c) && (
        <div className="hearth-banner" role="status">
          <Icon d={ICONS.alert} size={20} />
          <span className="hearth-row-txt">
            <strong>Chat vocal non pris en charge</strong>
            <span className="hearth-meta">Le serveur du groupe ne passe que par le tunnel de Cubelink, qui ne transporte pas la voix. Le mod reste chez toi mais ne sera pas chargé sur le serveur : jouez sans voix, ou utilisez une application de discussion à part.</span>
          </span>
        </div>
      )}

      {detail.orphans.length > 0 && (
        <section className="hearth-block hearth-fork hearth-orphans" aria-label="Fin de partie non publiée">
          <h2>Une fin de partie n'a pas été publiée</h2>
          <p className="hearth-meta">
            Une de tes parties ne s'est pas terminée proprement (plantage, arrêt du PC ou fermeture trop longue). Elle est gardée de côté sur ce PC : personne d'autre ne l'a reçue, et rien n'est publié sans ton accord.
            Si le groupe a joué depuis, la publier crée deux versions du monde et tu choisiras laquelle garder.
          </p>
          <ul className="hearth-rows">
            {detail.orphans.map((o) => (
              <li key={o.id} className="hearth-row">
                <span className="hearth-row-txt">
                  <strong>Partie du {fmtDate(o.at)}</strong>
                  <span className="hearth-meta">
                    {o.played_secs > 0 ? `${fmtPlayed(o.played_secs)} de jeu · ` : ""}{fmtSize(o.size_bytes)}
                    {o.base_number != null ? ` · partie de la version ${o.base_number}` : ""}
                  </span>
                </span>
                <Button aria-label={`Publier la fin de partie du ${fmtDate(o.at)}`} onClick={() => { services.hearthOrphanPublish(c.id, o.id).then(reload, (e: unknown) => setError(errText(e))); }}>Publier la fin de partie</Button>
                <Button
                  variant="secondary"
                  aria-label={`Supprimer la fin de partie du ${fmtDate(o.at)}`}
                  onClick={() => ask({
                    title: "Supprimer cette fin de partie ?", action: "Supprimer", danger: true,
                    text: "Ces minutes de jeu ne sont plus nulle part ailleurs : une fois supprimées, il n'y a pas de retour possible.",
                    run: () => services.hearthOrphanDiscard(c.id, o.id),
                  })}
                >Supprimer</Button>
              </li>
            ))}
          </ul>
          <p className="hearth-meta">« La mettre de côté » ne demande rien : laisse-la là, elle reste gardée tant que tu ne choisis pas.</p>
        </section>
      )}

      {c.state === "pack_missing" && (
        <div className="hearth-banner" role="status">
          <Icon d={ICONS.alert} size={20} />
          <span className="hearth-row-txt">
            <strong>Le modpack du groupe n'est pas installé sur ce PC.</strong>
            <span className="hearth-meta">Choisis l'instance qui contient le même modpack que {detail.members.find((m) => m.is_creator)?.name ?? "le créateur"}.</span>
          </span>
          {instances.filter((i) => i.minecraft === c.minecraft).map((i) => (
            <Button key={i.id} variant="secondary" onClick={() => { services.hearthSetInstance(c.id, i.id).then(reload, (e: unknown) => setError(errText(e))); }}>Utiliser {i.name}</Button>
          ))}
        </div>
      )}

      {(c.state === "fork" || detail.fork.length > 0) && (
        <section className="hearth-block hearth-fork" aria-label="Deux versions du monde">
          <h2>Deux versions du monde existent</h2>
          <p className="hearth-meta">Deux membres ont joué en même temps. Choisis la version à garder : l'autre reste dans l'historique et peut être restaurée.</p>
          <ul className="hearth-rows">
            {detail.fork.map((b) => (
              <li key={b.version_id} className="hearth-row">
                <span className="hearth-row-txt">
                  <strong>Version de {b.author_name}</strong>
                  <span className="hearth-meta">Version {b.number} · {fmtDate(b.created_at)} · {fmtPlayed(b.played_secs)} de jeu · {fmtSize(b.total_size)}</span>
                </span>
                <Button aria-label={`Garder la version de ${b.author_name}`} onClick={() => { services.hearthResolveFork(c.id, b.version_id).then(reload, (e: unknown) => setError(errText(e))); }}>Garder celle-ci</Button>
              </li>
            ))}
          </ul>
        </section>
      )}

      <Card className="hearth-tabs-card">
        <div className="hearth-tabs" role="tablist" aria-label="Sections du serveur">
          {TABS.map(([k, label]) => (
            <button key={k} type="button" role="tab" id={`hearth-tab-${k}`} aria-selected={tab === k} aria-controls="hearth-panel" onClick={() => setTab(k)}>{label}</button>
          ))}
        </div>
        <div id="hearth-panel" role="tabpanel" aria-labelledby={`hearth-tab-${tab}`} className="hearth-panel">
          {tab === "membres" && <Members d={detail} services={services} friends={friends} ask={ask} onError={setError} leave={leave} />}
          {tab === "historique" && <History d={detail} services={services} ask={ask} />}
          {tab === "reglages" && <Settings d={detail} services={services} instances={instances} ask={ask} onError={setError} />}
        </div>
      </Card>
      {confirm && <ConfirmDialog c={confirm} onClose={(next) => setConfirm(next ?? null)} />}
      {eula}
    </div>
  );
}

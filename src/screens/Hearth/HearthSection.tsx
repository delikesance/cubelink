import "./hearth.css";
import type { HearthCard, Services } from "../../services/types";
import { Badge, Button, Card, Icon, ICONS, ProgressBar } from "../../ui";
import { Avatar } from "../Friends/Avatar";
import { useHearthCards } from "../../app/useHearth";
import type { navigate as Navigate } from "../../app/router";
import { mainAction, plural, resumeLabel, stateHint, stateText } from "./format";
import { useHearthRun } from "./usePlay";
import { useHearthEnabled } from "./useEnabled";

/** Une carte de serveur de groupe : état, avatars des membres en ligne, un seul bouton principal, lien vers la page. */
function GroupCard({ card, services, navigate }: { card: HearthCard; services: Services; navigate: typeof Navigate }) {
  const { run, error, busy, eula } = useHearthRun(services);
  const act = mainAction(card);
  const hint = stateHint(card);
  const online = card.members_online;
  const open = () => navigate("hearth", { id: card.id });
  const tone = card.state === "hosting" ? "ok" : card.state === "hosted" ? "accent" : card.state === "idle" ? "neutral" : "warn";
  return (
    <Card as="li" className={`hearth-card is-${card.state}`} aria-label={`Serveur ${card.name}`}>
      <div className="hearth-head">
        <div className="hearth-title">
          <h3>{card.name}</h3>
          <span className="hearth-meta">{card.minecraft} · {card.loader}</span>
        </div>
        <Badge tone={tone}>{stateText(card)}</Badge>
      </div>

      {card.state === "syncing" && <ProgressBar value={card.progress} label={`Mise à jour du monde de ${card.name}`} showValue />}
      {card.state === "launching" && (
        <div className="hearth-steps" role="status">
          <ProgressBar value={card.step_index != null ? ((card.step_index + 1) / Math.max(1, card.step_count)) * 100 : null} label={`Lancement de ${card.name}`} />
          <span>Étape {(card.step_index ?? 0) + 1} sur {card.step_count}{card.step ? ` : ${card.step}` : ""}</span>
        </div>
      )}
      {hint && <p className="hearth-hint">{hint}</p>}

      <div className="hearth-foot">
        <ul className="hearth-avatars" aria-label={online.length ? `En ligne : ${online.map((m) => m.name).join(", ")}` : "Aucun membre en ligne"}>
          {online.slice(0, 5).map((m) => (
            <li key={m.public_key} title={`${m.name} (en ligne)`}><Avatar name={m.name} size={28} presence="online" services={services} /></li>
          ))}
          {online.length > 5 && <li className="hearth-more">+{online.length - 5}</li>}
          {online.length === 0 && <li className="hearth-none">{plural(card.members_total, "membre", "membres")}</li>}
        </ul>
        <div className="hearth-acts">
          {act.kind === "play" && (
            <Button loading={busy} aria-label={`${act.label} sur ${card.name}`} icon={<Icon d={ICONS.play} fill />} onClick={() => { void run(() => services.hearthPlay(card.id)); }}>{act.label}</Button>
          )}
          {act.kind === "stop" && (
            <Button variant="secondary" loading={busy} aria-label={`Arrêter le serveur ${card.name}`} icon={<Icon d={ICONS.stop} fill />} onClick={() => { void run(() => services.hearthStop(card.id)); }}>{act.label}</Button>
          )}
          {card.state === "unclean" && (() => {
            const r = resumeLabel(card);
            return (
              <>
                <Button loading={busy} aria-label={`${r.label} sur ${card.name}`} onClick={() => { void run(() => services.hearthUncleanChoice(card.id, r.choice)); }}>{r.label}</Button>
                <Button variant="secondary" onClick={() => { void run(() => services.hearthUncleanChoice(card.id, "wait")); }}>Attendre</Button>
              </>
            );
          })()}
          {card.state === "fork" && <Button onClick={open}>Choisir une version</Button>}
          {card.state === "pack_missing" && <Button onClick={open}>Installer le modpack</Button>}
          <button type="button" className="hearth-link" onClick={open} aria-label={`Page du serveur ${card.name}`}>Page du serveur</button>
        </div>
      </div>
      {error && <p className="hearth-err" role="alert">{error}</p>}
      {eula}
    </Card>
  );
}

/** Section « Serveurs de groupe » de l'accueil : une carte par groupe et le bouton de création. */
export function HearthSection({ services, navigate }: { services: Services; navigate: typeof Navigate }) {
  const cards = useHearthCards(services);
  const enabled = useHearthEnabled(services);
  // Interrupteur coupé (réglage « Serveurs de groupe (expérimental) ») : la section n'existe pas.
  if (!enabled) return null;
  return (
    <section className="home-hearth" aria-label="Serveurs de groupe">
      <div className="home-head is-small">
        <h2>Serveurs de groupe {cards && cards.length > 0 && <span className="home-count">{cards.length}</span>}</h2>
        <div className="home-head-act">
          <Button variant="secondary" icon={<Icon d={ICONS.plus} />} onClick={() => navigate("hearth", { new: "1" })}>Créer un serveur</Button>
        </div>
      </div>
      {cards === null ? (
        <p role="status" className="home-loading">Chargement des serveurs…</p>
      ) : cards.length === 0 ? (
        <p className="hearth-empty">Un monde partagé avec tes amis, qui vit même quand tu es hors ligne. Crée un serveur de groupe pour commencer.</p>
      ) : (
        <ul className="hearth-list" aria-label="Serveurs de groupe">
          {cards.map((c) => <GroupCard key={c.id} card={c} services={services} navigate={navigate} />)}
        </ul>
      )}
    </section>
  );
}

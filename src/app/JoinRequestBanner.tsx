import { useState } from "react";
import type { Services, StageState } from "../services";
import type { JoinRequest } from "../services/types";
import { JoinRequestRow } from "./JoinRequestRow";
import { useJoinRequests } from "./useJoinRequests";

/**
 * Le panneau « en jeu » affiche déjà les demandes quand il est ouvert : le bandeau s'efface alors pour ne pas les doubler.
 * Partout ailleurs (Scène absente, Jeu intégré coupé, repli, panneau fermé) il est la seule trace d'une demande.
 */
export function joinBannerVisible(stage: StageState, count: number): boolean {
  return count > 0 && !(stage.attached && stage.game && stage.panel);
}

function messageOf(e: unknown): string {
  if (e instanceof Error && e.message) return e.message;
  if (typeof e === "string" && e) return e;
  return "Une erreur est survenue. Réessaie dans un instant.";
}

/**
 * Hôte : carte flottante des demandes à rejoindre reçues, hors du panneau « en jeu » (absent sans Scène : Wayland, macOS,
 * Jeu intégré coupé). La liste se met à jour toute seule par friend://join_requests (décision, expiration).
 */
export function JoinRequestBanner({ services, stage }: { services: Services; stage: StageState }) {
  const requests = useJoinRequests(services);
  const [busy, setBusy] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  if (!joinBannerVisible(stage, requests.length)) return null;
  const answer = async (r: JoinRequest, accept: boolean) => {
    if (busy) return;
    setBusy(r.public_key);
    setError(null);
    try {
      await services.friendJoinRequestAnswer(r.public_key, accept);
    } catch (e) {
      setError(messageOf(e));
    } finally {
      setBusy(null);
    }
  };
  return (
    <section className="join-banner" role="region" aria-label="Demandes à rejoindre">
      <ul className="ov-list">
        {requests.map((r) => (
          <JoinRequestRow key={r.public_key} request={r} services={services} busy={busy === r.public_key} onAnswer={(q, a) => { void answer(q, a); }} />
        ))}
      </ul>
      {error && <p className="join-banner-err" role="alert">{error}</p>}
    </section>
  );
}

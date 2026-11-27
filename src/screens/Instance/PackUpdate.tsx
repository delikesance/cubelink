// Mise à jour d'un modpack installé depuis le catalogue (ticket #137) : bandeau « Mise à jour X disponible », application
// (les fichiers du joueur sont gardés, l'ancien état est sauvegardé), résumé avec « Annuler », puis proposition au groupe
// Hearth quand l'instance en est la base.
import { useCallback, useEffect, useRef, useState } from "react";
import { errMsg } from "../../app/useLaunch";
import type { HearthCard, Instance, ModpackUpdate, ModpackUpdateResult, Services } from "../../services";
import { Button, ICONS, Icon } from "../../ui";

type Svc = Pick<
  Services,
  "modpackUpdateCheck" | "modpackUpdateApply" | "modpackUpdateUndo" | "onBrowseProgress" | "hearthEnabled" | "hearthList" | "hearthPackUpdate"
>;

const plural = (n: number, one: string, many: string) => `${n} ${n > 1 ? many : one}`;

/** « 3 ajoutés · 1 retiré · 12 remplacés » (les compteurs à zéro sont omis). */
export function summaryOf(r: ModpackUpdateResult): string {
  const parts = [
    r.added > 0 && plural(r.added, "fichier ajouté", "fichiers ajoutés"),
    r.removed > 0 && plural(r.removed, "retiré", "retirés"),
    r.replaced > 0 && plural(r.replaced, "remplacé", "remplacés"),
  ].filter(Boolean);
  return parts.length ? parts.join(" · ") : "Aucun fichier à changer";
}

export function PackUpdate({ services, inst, locked, onChanged }: { services: Svc; inst: Instance; locked: boolean; onChanged: () => void }) {
  const [update, setUpdate] = useState<ModpackUpdate | null>(null);
  const [busy, setBusy] = useState<{ done: number; total: number } | null>(null);
  const [done, setDone] = useState<{ r: ModpackUpdateResult; name: string } | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [notice, setNotice] = useState<string | null>(null);
  const [group, setGroup] = useState<HearthCard | null>(null);
  const [proposed, setProposed] = useState(false);
  const alive = useRef(true);
  useEffect(() => { alive.current = true; return () => { alive.current = false; }; }, []);

  const supported = inst.pack?.source === "modrinth" || inst.pack?.source === "curseforge";
  const versionId = inst.pack?.version_id ?? "";

  // Détection : meilleur effort, silencieuse hors ligne (la commande garde son résultat 30 minutes).
  const check = useCallback(() => {
    if (!supported) { setUpdate(null); return; }
    services.modpackUpdateCheck(inst.id).then(
      (u) => { if (alive.current) setUpdate(u); },
      () => { if (alive.current) setUpdate(null); },
    );
  }, [services, inst.id, supported]);
  useEffect(() => { check(); }, [check, versionId]);

  // Progression des téléchargements pendant la mise à jour.
  useEffect(() => {
    if (!busy) return;
    return services.onBrowseProgress((e) => { if (alive.current) setBusy({ done: e.done, total: e.total }); });
    // Abonné tant qu'une mise à jour est en cours (la valeur de `busy` change sans se réabonner).
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [services, busy === null]);

  // Groupe Hearth dont cette instance est la base : on propose d'y pousser la mise à jour.
  useEffect(() => {
    if (!done) { setGroup(null); return; }
    let on = true;
    services.hearthEnabled().then((e) => (e ? services.hearthList() : [])).then(
      (l) => { if (on) setGroup(l.find((c) => c.instance_id === inst.id) ?? null); },
      () => {},
    );
    return () => { on = false; };
  }, [services, done, inst.id]);

  const apply = async () => {
    if (!update || busy) return;
    setError(null);
    setNotice(null);
    setProposed(false);
    setBusy({ done: 0, total: 0 });
    try {
      const r = await services.modpackUpdateApply(inst.id, update.version_id);
      if (!alive.current) return;
      setDone({ r, name: update.name });
      setUpdate(null);
      onChanged();
    } catch (e) {
      if (alive.current) setError(errMsg(e));
    } finally {
      if (alive.current) setBusy(null);
    }
  };

  const undo = async () => {
    if (!done || busy) return;
    setError(null);
    setBusy({ done: 0, total: 0 });
    try {
      await services.modpackUpdateUndo(inst.id, done.r.backup_id);
      if (!alive.current) return;
      setDone(null);
      setNotice("Mise à jour annulée : ton modpack est revenu à son état d'avant.");
      onChanged();
      check();
    } catch (e) {
      if (alive.current) setError(errMsg(e));
    } finally {
      if (alive.current) setBusy(null);
    }
  };

  const propose = async () => {
    if (!group) return;
    setError(null);
    try {
      await services.hearthPackUpdate(group.id, inst.id);
      if (alive.current) setProposed(true);
    } catch (e) {
      if (alive.current) setError(errMsg(e));
    }
  };

  if (!update && !done && !busy && !notice && !error) return null;

  return (
    <section className="ipg-card ipg-upd" aria-label="Mise à jour du modpack">
      {busy ? (
        <p className="ipg-upd-line" role="status">
          <Icon d={ICONS.download} size={16} />
          {busy.total > 0 ? `Mise à jour en cours… ${busy.done}/${busy.total}` : "Mise à jour en cours…"}
        </p>
      ) : done ? (
        <>
          <div className="ipg-card-h"><span><Icon d={ICONS.check} size={16} />Modpack mis à jour</span><span className="c">{done.name}</span></div>
          <p className="ipg-upd-line" role="status">{summaryOf(done.r)}.</p>
          {done.r.kept.length > 0 && (
            <p className="ipg-note">
              {plural(done.r.kept.length, "fichier à toi a été conservé", "fichiers à toi ont été conservés")} tel{done.r.kept.length > 1 ? "s" : ""} quel{done.r.kept.length > 1 ? "s" : ""} :{" "}
              {done.r.kept.slice(0, 3).join(", ")}{done.r.kept.length > 3 ? `, … (+${done.r.kept.length - 3})` : ""}.
            </p>
          )}
          {done.r.missing.length > 0 && (
            <p className="ipg-note">
              {plural(done.r.missing.length, "mod à télécharger", "mods à télécharger")} toi-même :{" "}
              {done.r.missing.slice(0, 5).join(", ")}{done.r.missing.length > 5 ? `, … (+${done.r.missing.length - 5})` : ""}.
            </p>
          )}
          <div className="ipg-upd-act">
            <Button variant="secondary" disabled={locked} onClick={() => { void undo(); }}>Annuler la mise à jour</Button>
            {group && !proposed && <Button disabled={locked} onClick={() => { void propose(); }}>Proposer la mise à jour au groupe</Button>}
          </div>
          {group && proposed && <p className="ipg-note" role="status">Proposition envoyée au groupe « {group.name} » : elle s'applique si la majorité l'accepte.</p>}
        </>
      ) : update ? (
        <>
          <div className="ipg-card-h"><span><Icon d={ICONS.download} size={16} />Mise à jour {update.version_number} disponible</span></div>
          <p className="ipg-note">
            {update.name}. Tes mondes, tes options et les fichiers que tu as modifiés ou ajoutés sont gardés. L'ancien état est sauvegardé : tu pourras annuler.
          </p>
          <div className="ipg-upd-act">
            <Button disabled={locked} onClick={() => { void apply(); }}>Mettre à jour</Button>
            {locked && <span className="ipg-note">Ferme le jeu pour mettre à jour.</span>}
          </div>
        </>
      ) : null}
      {notice && !done && !busy && <p className="ipg-note" role="status">{notice}</p>}
      {error && <p className="ipg-note is-error" role="alert">{error}</p>}
    </section>
  );
}

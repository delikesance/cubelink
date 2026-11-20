import { useCallback, useEffect, useState } from "react";
import type { Instance, PlayerShot, Services } from "../../services";
import { Button, ICONS, Icon } from "../../ui";
import { errMsg } from "../../app/useLaunch";
import { clearArtCache, useArtImage } from "../Main/artImage";
import { Viewer } from "../Main/Viewer";

type ShotServices = Pick<Services, "instancesArtImage" | "instancesScreenshotsList" | "instancesScreenshotImage" | "instancesScreenshotSetBanner" | "instancesOpenScreenshotsFolder">;

export interface ScreenshotsPanelProps {
  services: ShotServices;
  instance: Instance;
  /** Appelé avec l'instance mise à jour quand une capture devient la bannière. */
  onInstance: (i: Instance) => void;
}

/** « 20 nov., 21:14 » ; le nom du fichier seul quand la date est inconnue. */
export function shotLabel(s: PlayerShot): string {
  if (!s.modified_ms) return s.file;
  return new Date(s.modified_ms).toLocaleString("fr-FR", { day: "numeric", month: "short", hour: "2-digit", minute: "2-digit" });
}

const sameList = (a: PlayerShot[] | null, b: PlayerShot[]) =>
  a !== null && a.length === b.length && a.every((x, i) => x.file === b[i].file && x.modified_ms === b[i].modified_ms && x.size === b[i].size);

function ShotThumb({ services, instanceId, shot, onOpen }: { services: ShotServices; instanceId: string; shot: PlayerShot; onOpen: () => void }) {
  const img = useArtImage(services, instanceId, shot.file, true, undefined, "shot");
  return (
    <button type="button" ref={img.ref} className="ipg-cap-thumb" aria-label={`Agrandir la capture du ${shotLabel(shot)}`} onClick={onOpen}>
      {img.status === "ready" && img.src ? <img src={img.src} alt="" onError={img.markError} />
        : img.status === "error" ? <span className="home-thumb-err">Indisponible</span>
        : <span className="home-skel" />}
    </button>
  );
}

/**
 * Onglet « Captures » : les captures F2 du joueur (`screenshots/` du dossier de jeu), en lecture seule.
 * Se recharge à l'ouverture et quand la fenêtre reprend le focus (le joueur prend ses captures en jeu).
 */
export function ScreenshotsPanel({ services, instance, onInstance }: ScreenshotsPanelProps) {
  const id = instance.id;
  const [shots, setShots] = useState<PlayerShot[] | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [note, setNote] = useState<{ kind: "ok" | "error"; text: string } | null>(null);
  const [busy, setBusy] = useState<string | null>(null);
  const [viewer, setViewer] = useState<number | null>(null);

  const reload = useCallback(() => {
    let alive = true;
    services.instancesScreenshotsList(id).then(
      (l) => { if (alive) { setError(null); setShots((cur) => (sameList(cur, l) ? cur : l)); } },
      (e: unknown) => { if (alive) { setError(errMsg(e)); setShots((cur) => cur ?? []); } },
    );
    return () => { alive = false; };
  }, [services, id]);

  useEffect(() => {
    setShots(null);
    const stop = reload();
    const onFocus = () => { if (document.visibilityState !== "hidden") reload(); };
    window.addEventListener("focus", onFocus);
    document.addEventListener("visibilitychange", onFocus);
    return () => { stop(); window.removeEventListener("focus", onFocus); document.removeEventListener("visibilitychange", onFocus); };
  }, [reload]);

  const openFolder = async () => {
    setNote(null);
    try { await services.instancesOpenScreenshotsFolder(id); } catch (e) { setNote({ kind: "error", text: errMsg(e) }); }
  };

  const useAsBanner = async (file: string) => {
    if (busy) return;
    setNote(null);
    setBusy(file);
    try {
      const u = await services.instancesScreenshotSetBanner(id, file);
      clearArtCache();
      onInstance(u);
      setNote({ kind: "ok", text: "Bannière définie." });
    } catch (e) {
      setNote({ kind: "error", text: errMsg(e) });
    } finally {
      setBusy(null);
    }
  };

  const list = shots ?? [];
  return (
    <section className="ipg-card ipg-caps" aria-label="Captures d'écran du joueur">
      <div className="ipg-card-h">
        <span><Icon d={ICONS.image} size={16} />Mes captures{shots !== null && shots.length > 0 ? <span className="c">{shots.length}</span> : null}</span>
        <Button variant="secondary" icon={<Icon d={ICONS.folder} size={18} />} onClick={() => { void openFolder(); }}>Ouvrir le dossier</Button>
      </div>
      {error && <p className="ipg-note is-error" role="alert">{error}</p>}
      {note && (note.kind === "error"
        ? <p className="ipg-note is-error" role="alert">{note.text}</p>
        : <p className="ipg-note" role="status">{note.text}</p>)}
      {shots === null ? (
        <p className="ipg-note" role="status">Chargement des captures…</p>
      ) : list.length === 0 ? (
        <p className="ipg-note">Aucune capture pour le moment. Appuie sur F2 en jeu : elle apparaîtra ici.</p>
      ) : (
        <ul className="ipg-cap-grid" aria-label="Captures">
          {list.map((s, i) => (
            <li key={s.file} className="ipg-cap">
              <ShotThumb services={services} instanceId={id} shot={s} onOpen={() => setViewer(i)} />
              <div className="ipg-cap-foot">
                <span className="ipg-cap-date" title={s.file}>{shotLabel(s)}</span>
                <Button
                  variant="secondary" disabled={busy !== null} loading={busy === s.file}
                  aria-label={`Utiliser la capture du ${shotLabel(s)} comme bannière`}
                  onClick={() => { void useAsBanner(s.file); }}
                >Comme bannière</Button>
              </div>
            </li>
          ))}
        </ul>
      )}
      {viewer !== null && list.length > 0 && (
        <Viewer services={services} source="shot" instanceId={id} title={instance.name} files={list.map((s) => s.file)} start={Math.min(viewer, list.length - 1)} onClose={() => setViewer(null)} />
      )}
    </section>
  );
}

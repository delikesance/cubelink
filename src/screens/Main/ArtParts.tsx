import "./shared.css";
import type { CSSProperties } from "react";
import type { Instance, Services } from "../../services";
import { avatarColors } from "./accent";
import { useArtImage } from "./artImage";

type ImgServices = Pick<Services, "instancesArtImage">;

/** Couleurs de la pastille d'une instance (rail, cartes). */
export function avatarStyle(i: Instance): CSSProperties {
  const c = avatarColors(i.art?.color, i.name);
  return { background: c.bg, color: c.ink };
}

/** Initiales (1 ou 2 lettres) pour la pastille sans icône. */
export function initials(name: string): string {
  const words = name.trim().split(/[\s:_-]+/).filter(Boolean);
  if (words.length >= 2 && /^[\p{L}\p{N}]/u.test(words[1])) return (words[0][0] + words[1][0]).toUpperCase();
  return (words[0] ?? "?").slice(0, 1).toUpperCase();
}

export const sourceLabel = (s: string) =>
  s === "curseforge" ? "CurseForge" : s === "modrinth" ? "Modrinth" : null;

/** Icône du modpack, sinon initiales sur fond de la couleur de l'instance. */
export function Avatar({ services, inst, className }: { services: ImgServices; inst: Instance; className: string }) {
  const img = useArtImage(services, inst.id, inst.art?.icon, true);
  const showImg = img.status === "ready" && img.src;
  return (
    <span ref={img.ref} className={`${className}${showImg ? " has-img" : ""}`} style={avatarStyle(inst)} aria-hidden="true">
      {showImg ? <img src={img.src!} alt="" onError={img.markError} /> : initials(inst.name)}
    </span>
  );
}

function Thumb({ services, instanceId, file, index, active, onPick }: { services: ImgServices; instanceId: string; file: string; index: number; active: boolean; onPick: (i: number) => void }) {
  const img = useArtImage(services, instanceId, file, true);
  return (
    <button
      type="button" ref={img.ref} className={`home-thumb${active ? " is-active" : ""}`}
      aria-label={`Capture ${index + 1}`} aria-current={active || undefined} onClick={() => onPick(index)}
    >
      {img.status === "ready" && img.src ? <img src={img.src} alt="" onError={img.markError} />
        : img.status === "error" ? <span className="home-thumb-err">Indisponible</span>
        : <span className="home-skel" />}
    </button>
  );
}

/** Bande de miniatures de la visionneuse. */
export function ThumbStrip({ services, instanceId, files, current, onPick }: { services: ImgServices; instanceId: string; files: string[]; current: number; onPick: (i: number) => void }) {
  return (
    <div className="home-thumbs" role="group" aria-label="Captures d'écran">
      {files.map((f, i) => <Thumb key={f} services={services} instanceId={instanceId} file={f} index={i} active={i === current} onPick={onPick} />)}
    </div>
  );
}

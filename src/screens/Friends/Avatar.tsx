import "./avatar.css";
import type { CSSProperties } from "react";

const PALETTE = ["#8EC5E8", "#F0A080", "#C8A8E0", "#B8DC8C", "#8CB4F0", "#F2C98A", "#E8A0C0", "#8ED8C4"];

/** Couleur d'avatar stable pour un nom (aplat pastel, texte sombre). */
export function avatarColor(name: string): string {
  let h = 0;
  for (const ch of name) h = (h * 31 + ch.codePointAt(0)!) >>> 0;
  return PALETTE[h % PALETTE.length]!;
}

export type AvatarPresence = "online" | "offline" | "none";

/** Pastille ronde avec l'initiale ; `presence` ajoute le point d'état (le libellé d'état doit être lisible à côté). */
export function Avatar({ name, size = 44, presence = "none", tone }: { name: string; size?: number; presence?: AvatarPresence; tone?: string }) {
  const style: CSSProperties = { width: size, height: size, background: tone ?? avatarColor(name), fontSize: Math.round(size * 0.36) };
  return (
    <span className="av" style={style} aria-hidden="true">
      {name.trim().slice(0, 1).toUpperCase() || "?"}
      {presence !== "none" && <span className={`av-dot av-dot-${presence}`} />}
    </span>
  );
}

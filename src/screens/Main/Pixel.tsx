/** Vignettes en pixels : tête d'un joueur (8×8) et icône de monde, nettes à toutes les tailles. */

const SKINS = ["#e0b48f", "#c98e66", "#f0c9a8", "#8d5a3b", "#d8a47a", "#6e432a", "#f2d3b3"];
const HAIRS = ["#2b1d14", "#b8332c", "#d9b44a", "#111111", "#6b4a2b", "#e8e1d0", "#3a5ba8", "#7a3fa0"];
const EYES = ["#3f8f5a", "#3a5ba8", "#6b4aa8", "#5a3a20", "#2a6f7a"];

/** Empreinte stable d'une chaîne (FNV-1a 32 bits). */
export function hash(s: string): number {
  let h = 0x811c9dc5;
  for (let i = 0; i < s.length; i++) h = Math.imul(h ^ s.charCodeAt(i), 0x01000193) >>> 0;
  return h;
}

/** Tête générée depuis une graine (clé publique d'un ami, nom d'un compte hors ligne) : les skins des amis ne sont pas transmis. */
export function PixelHead({ seed, size, className }: { seed: string; size: number; className?: string }) {
  const h = hash(seed);
  const skin = SKINS[h % SKINS.length];
  const hair = HAIRS[(h >>> 8) % HAIRS.length];
  const eye = EYES[(h >>> 16) % EYES.length];
  return (
    <svg className={className} width={size} height={size} viewBox="0 0 8 8" shapeRendering="crispEdges" aria-hidden="true">
      <rect width="8" height="8" fill={skin} />
      <rect width="8" height="2" fill={hair} />
      <rect y="2" width="1" height="1" fill={hair} /><rect x="7" y="2" width="1" height="1" fill={hair} />
      <rect x="1" y="4" width="2" height="1" fill="#f4f1ec" /><rect x="5" y="4" width="2" height="1" fill="#f4f1ec" />
      <rect x="2" y="4" width="1" height="1" fill={eye} /><rect x="5" y="4" width="1" height="1" fill={eye} />
      <rect x="3" y="5" width="2" height="1" fill="rgba(0,0,0,.18)" />
      <rect x="2" y="6" width="4" height="1" fill="rgba(0,0,0,.3)" />
    </svg>
  );
}

/** Icône du monde (icon.png de la sauvegarde de l'hôte), sinon un paysage d'herbe par défaut. */
export function WorldIcon({ src, size, className }: { src?: string | null; size: number; className?: string }) {
  if (src && src.startsWith("data:image/png;base64,")) {
    return <img className={`px-img ${className ?? ""}`} src={src} width={size} height={size} alt="" />;
  }
  return (
    <svg className={className} width={size} height={size} viewBox="0 0 8 8" shapeRendering="crispEdges" aria-hidden="true">
      <rect width="8" height="8" fill="#8fc7f0" />
      <rect x="6" y="1" width="1" height="1" fill="#fff2a8" />
      <rect y="4" width="8" height="4" fill="#6fbf5a" />
      <rect x="1" y="3" width="3" height="1" fill="#6fbf5a" /><rect x="2" y="2" width="1" height="1" fill="#6fbf5a" />
      <rect x="5" y="3" width="2" height="1" fill="#6fbf5a" />
      <rect y="6" width="8" height="2" fill="#8a5a3a" />
    </svg>
  );
}

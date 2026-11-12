/** Logo Cubelink : un bloc dont un coin est retiré, le morceau (plus clair) flotte en diagonale.
 *  Aplats via les tokens (--accent corps, --accent-hover morceau, --accent-edge tranche), bouts arrondis par un trait de même couleur.
 *  `small` (< 48 px) : tranche et morceau plus gros pour que l'idée tienne. `far` : morceau éloigné (états d'erreur / vides). */
const BODY = "M24 104Q24 84 44 84L104 84Q118 84 118 98L118 118Q118 140 140 140L160 140Q174 140 174 154L174 214Q174 234 154 234L44 234Q24 234 24 214Z";
const PIECE = "M172 56Q172 36 192 36L208 36Q228 36 228 56L228 72Q228 92 208 92L192 92Q172 92 172 72Z";
const BODY_SLAB = "M24 124Q24 104 44 104L104 104Q118 104 118 118L118 138Q118 160 140 160L160 160Q174 160 174 174L174 234Q174 254 154 254L44 254Q24 254 24 234Z";
const PIECE_SLAB = "M172 76Q172 56 192 56L208 56Q228 56 228 76L228 92Q228 112 208 112L192 112Q172 112 172 92Z";
const BODY_SMALL = "M24 104Q24 84 44 84L104 84Q118 84 118 98L118 118Q118 140 140 140L160 140Q174 140 174 154L174 214Q174 234 154 234L44 234Q24 234 24 214Z";
const BODY_SLAB_SMALL = "M24 128Q24 108 44 108L104 108Q118 108 118 122L118 142Q118 164 140 164L160 164Q174 164 174 178L174 238Q174 258 154 258L44 258Q24 258 24 238Z";
const PIECE_SMALL = "M168 48Q168 28 188 28L212 28Q232 28 232 48L232 72Q232 92 212 92L188 92Q168 92 168 72Z";
const PIECE_SLAB_SMALL = "M168 72Q168 52 188 52L212 52Q232 52 232 72L232 96Q232 116 212 116L188 116Q168 116 168 96Z";

export function Logo({ width = 44, small, far = false, shadow = false, className }: { width?: number; small?: boolean; far?: boolean; shadow?: boolean; className?: string }) {
  const s = small ?? width < 48;
  const stroke = s ? 10 : 12;
  const dx = far ? 24 : 0;
  const dy = far ? -22 : 0;
  // cadre serré autour du dessin (marge pour le morceau éloigné et l'ombre)
  const minY = far ? -14 : 10;
  const vbW = far ? 258 : 232;
  const vbH = (shadow ? 274 : 258) - minY;
  const body = s ? BODY_SMALL : BODY;
  const bodySlab = s ? BODY_SLAB_SMALL : BODY_SLAB;
  const piece = s ? PIECE_SMALL : PIECE;
  const pieceSlab = s ? PIECE_SLAB_SMALL : PIECE_SLAB;
  const g = { strokeWidth: stroke, strokeLinejoin: "round" as const };
  return (
    <svg className={className} width={width} height={(width * vbH) / vbW} viewBox={`16 ${minY} ${vbW} ${vbH}`} aria-hidden="true" focusable="false">
      {shadow && <ellipse cx="108" cy="262" rx="80" ry="9" fill="var(--edge-2)" />}
      <g transform="translate(6 -12)">
        <path d={bodySlab} fill="var(--accent-edge)" stroke="var(--accent-edge)" {...g} />
        <g transform={`translate(${dx} ${dy})`}>
          <path d={pieceSlab} fill="var(--accent-edge)" stroke="var(--accent-edge)" {...g} />
        </g>
        <path d={body} fill="var(--accent)" stroke="var(--accent)" {...g} />
        <g transform={`translate(${dx} ${dy})`}>
          <path d={piece} fill="var(--accent-hover)" stroke="var(--accent-hover)" {...g} />
        </g>
      </g>
    </svg>
  );
}

/** Motif de blocs (carrés arrondis à 4 % d'opacité) : Connexion, états vides, Erreur. */
export function BlockPattern() {
  const blocks: [number, number, number][] = [[8, 18, 160], [18, 34, 100], [76, 14, 140], [82, 62, 180], [11, 72, 120], [68, 78, 90]];
  return (
    <div className="blocks" aria-hidden="true">
      {blocks.map(([x, y, s]) => <span key={`${x}-${y}`} style={{ left: `${x}%`, top: `${y}%`, width: s, height: s }} />)}
    </div>
  );
}

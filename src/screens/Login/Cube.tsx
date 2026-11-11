import { useId } from "react";

/** Cube isométrique (3 aplats) : logo et illustration d'état vide / d'erreur. */
export function Cube({ width = 44, crack = false, shadow = false, className }: { width?: number; crack?: boolean; shadow?: boolean; className?: string }) {
  const id = useId();
  const h = shadow ? 152 : 140;
  return (
    <svg className={className} width={width} height={(width * h) / 160} viewBox={`0 0 160 ${h}`} aria-hidden="true" focusable="false">
      {shadow && <ellipse cx="80" cy="141" rx="54" ry="8" fill="var(--edge-2)" />}
      <clipPath id={id}>
        <path d="M73.94,9.50 Q80.00,6.00 86.06,9.50 L129.36,34.50 Q135.43,38.00 135.43,45.00 L135.43,95.00 Q135.43,102.00 129.36,105.50 L86.06,130.50 Q80.00,134.00 73.94,130.50 L30.64,105.50 Q24.57,102.00 24.57,95.00 L24.57,45.00 Q24.57,38.00 30.64,34.50 Z" />
      </clipPath>
      <g clipPath={`url(#${id})`}>
        <polygon points="80,6 135.43,38 80,70 24.57,38" fill="var(--accent-hover)" />
        <polygon points="24.57,38 80,70 80,134 24.57,102" fill="var(--accent)" />
        <polygon points="80,70 135.43,38 135.43,102 80,134" fill="var(--accent-edge)" />
      </g>
      {crack && <polyline points="50,53 59,68 46,83 57,98 51,112" fill="none" stroke="var(--edge-2)" strokeWidth="3.5" strokeLinejoin="miter" />}
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

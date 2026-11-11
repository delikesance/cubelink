import { useEffect, useState } from "react";
import type { Services } from "../../services";
import { Button, IconButton } from "../../ui";
import { Modal } from "./Modal";
import { ThumbStrip } from "./ArtParts";
import { useArtImage } from "./artImage";

export interface ViewerProps {
  services: Pick<Services, "instancesArtImage">;
  instanceId: string;
  title: string;
  files: string[];
  start: number;
  onClose: () => void;
}

const Chevron = ({ dir }: { dir: "l" | "r" }) => (
  <svg width="24" height="24" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.75" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true">
    <path d={dir === "l" ? "M15 5l-7 7 7 7" : "M9 5l7 7-7 7"} />
  </svg>
);

/** Visionneuse de captures (A8) : plein écran, flèches gauche/droite, miniatures, Échap pour fermer. */
export function Viewer({ services, instanceId, title, files, start, onClose }: ViewerProps) {
  const n = files.length;
  const [i, setI] = useState(Math.min(Math.max(start, 0), n - 1));
  const go = (d: number) => setI((c) => (c + d + n) % n);
  const img = useArtImage(services, instanceId, files[i], false, 2048);

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "ArrowLeft") { e.preventDefault(); setI((c) => (c + n - 1) % n); }
      else if (e.key === "ArrowRight") { e.preventDefault(); setI((c) => (c + 1) % n); }
    };
    document.addEventListener("keydown", onKey);
    return () => document.removeEventListener("keydown", onKey);
  }, [n]);

  return (
    <Modal title={`${title} : capture ${i + 1} sur ${n}`} onClose={onClose} fullscreen>
      <div className="home-viewer">
        <p className="home-viewer-count" aria-hidden="true">{i + 1} / {n}</p>
        <Button variant="secondary" className="home-viewer-close" aria-label="Fermer (Échap)" onClick={onClose}>
          Échap
          <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.75" strokeLinecap="round" aria-hidden="true"><path d="M6 6l12 12M18 6L6 18" /></svg>
        </Button>
        <IconButton variant="secondary" className="home-viewer-nav is-prev" aria-label="Capture précédente" disabled={n < 2} icon={<Chevron dir="l" />} onClick={() => go(-1)} />
        <div className="home-viewer-stage">
          {img.status === "ready" && img.src ? (
            <img key={i} className="enter" src={img.src} alt={`${title}, capture ${i + 1}`} onError={img.markError} />
          ) : img.status === "error" ? (
            <p className="home-err" role="alert">Image indisponible.</p>
          ) : (
            <span className="home-skel" role="status" aria-label="Chargement de l'image" />
          )}
        </div>
        <IconButton variant="secondary" className="home-viewer-nav is-next" aria-label="Capture suivante" disabled={n < 2} icon={<Chevron dir="r" />} onClick={() => go(1)} />
        <p className="home-viewer-file">{files[i].split(/[\/]/).pop()}</p>
        <ThumbStrip services={services} instanceId={instanceId} files={files} current={i} onPick={setI} />
      </div>
    </Modal>
  );
}

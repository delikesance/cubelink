import { useEffect, useRef } from "react";
import "./install.css";
import { useScreenTitle, useShell } from "../../app/context";
import { Button } from "../../ui";
import { DefaultArt } from "../../ui";
import { TransferPanel } from "./TransferPanel";
import { useTransfer } from "./useTransfer";

/**
 * Écran d'installation hors parcours ami (le parcours ami vit dans l'écran Rejoindre).
 * Les échecs identifiables mènent à l'écran Erreur.
 */
export default function Install() {
  const { services, params, navigate } = useShell();
  const t = useTransfer(services, (kind, file) => {
    navigate("error", { state: kind, ...(file ? { file } : {}) });
    return true;
  });
  const title = useRef<HTMLHeadingElement>(null);

  useScreenTitle("Installation");
  useEffect(() => { title.current?.focus(); }, []);

  const play = () => t.run(async () => {
    const id = params.instance ?? (await services.instancesList())[0]?.id;
    if (!id) throw new Error("no instance");
    await services.instancesLaunch(id);
    navigate("main");
  }, "Le lancement a échoué. Réessaie.");

  return (
    <div className="inst">
      <div className="inst-bg" aria-hidden="true"><DefaultArt className="inst-art" /><div className="inst-veil" /></div>
      <section className="inst-main" aria-labelledby="inst-title">
        <span className="inst-badge enter">Monde d'un ami</span>
        <h1 id="inst-title" ref={title} tabIndex={-1} className="enter" style={{ "--i": 1 } as React.CSSProperties}>Installation en cours</h1>
        <TransferPanel
          t={t}
          style={{ "--i": 2 } as React.CSSProperties}
          onCancelled={() => navigate("main")}
          onHome={() => navigate("main")}
          onPause={() => services.joinPause()}
          onResume={() => services.joinResume()}
          extra={t.phase === "done" ? <Button size="lg" disabled={t.busy} onClick={() => { void play(); }}>Jouer</Button> : null}
        />
      </section>
    </div>
  );
}

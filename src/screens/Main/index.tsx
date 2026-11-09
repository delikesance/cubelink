import { useCallback, useEffect, useState } from "react";
import "./main.css";
import heroImg from "../../assets/hero-1.jpg";
import { useScreenTitle, useShell } from "../../app/context";
import type { FriendWithPresence, Instance, LaunchState } from "../../services";

const LOADERS: Record<string, string> = {
  vanilla: "Vanilla", neoforge: "NeoForge", fabric: "Fabric", quilt: "Quilt", forge: "Forge",
};
const loaderLabel = (i: Instance) => LOADERS[i.loader.loader] ?? i.loader.loader;
const modsLabel = (n: number) => `${n} mod${n > 1 ? "s" : ""}`;

export function badgeText(total: number): string {
  return total > 99 ? "99+" : String(total);
}

const Plus = () => (
  <svg width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" aria-hidden="true"><path d="M12 5v14M5 12h14" /></svg>
);
const Gear = () => (
  <svg width="20" height="20" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.6" strokeLinecap="round" aria-hidden="true"><circle cx="12" cy="12" r="3" /><path d="M12 2v3M12 19v3M2 12h3M19 12h3M4.9 4.9l2.1 2.1M17 17l2.1 2.1M4.9 19.1L7 17M17 7l2.1-2.1" /></svg>
);
const Play = () => (
  <svg width="18" height="18" viewBox="0 0 24 24" fill="currentColor" aria-hidden="true"><path d="M7 4.5v15a1 1 0 001.5.9l12-7.5a1 1 0 000-1.7l-12-7.5A1 1 0 007 4.5z" /></svg>
);

export default function Main() {
  const { services, navigate } = useShell();
  const [instances, setInstances] = useState<Instance[] | null>(null);
  const [selected, setSelected] = useState<string | null>(null);
  const [launch, setLaunch] = useState<LaunchState | null>(null);
  const [launchError, setLaunchError] = useState<string | null>(null);
  const [requests, setRequests] = useState<number | null>(null);
  const [friends, setFriends] = useState<FriendWithPresence[]>([]);
  const [loadError, setLoadError] = useState<string | null>(null);

  useScreenTitle(null);

  useEffect(() => {
    let alive = true;
    services.instancesList().then(
      (list) => {
        if (!alive) return;
        setInstances(list);
        setSelected((cur) => cur ?? list[0]?.id ?? null);
      },
      (e: unknown) => { if (alive) { setInstances([]); setLoadError(e instanceof Error ? e.message : String(e)); } },
    );
    services.friendRequests({ offset: 0, limit: 1 }).then(
      (p) => { if (alive) setRequests(p.total); },
      () => {},
    );
    services.friendList().then((l) => { if (alive) setFriends(l); }, () => {});
    return () => { alive = false; };
  }, [services]);

  useEffect(() => services.onLaunchState((s) => { setLaunch(s); setLaunchError(null); }), [services]);
  useEffect(() => services.onFriendPresence((e) => {
    setFriends((l) => l.map((f) => (f.friend.public_key === e.public_key ? { ...f, presence: e.presence } : f)));
  }), [services]);
  useEffect(() => services.onFriendRequest(() => setRequests((n) => (n ?? 0) + 1)), [services]);

  const current = instances?.find((i) => i.id === selected) ?? null;
  const state = launch && launch.id === selected ? launch : null;
  const busy = state?.state === "preparing" || state?.state === "running";

  const play = useCallback(async () => {
    if (!current) return;
    setLaunchError(null);
    setLaunch({ id: current.id, state: "preparing" });
    try {
      await services.instancesLaunch(current.id);
    } catch (e) {
      const msg = e instanceof Error ? e.message : String(e);
      setLaunch(null);
      setLaunchError(msg);
    }
  }, [current, services]);

  let status = "";
  let isError = false;
  if (launchError) { status = `Échec du lancement : ${launchError}`; isError = true; }
  else if (state?.state === "preparing") status = "Préparation du jeu…";
  else if (state?.state === "running") status = "Minecraft est en cours d'exécution.";
  else if (state?.state === "exited") status = "Minecraft est fermé.";
  else if (state?.state === "failed") {
    status = `Échec du lancement${state.message ? ` : ${state.message}` : "."}`;
    isError = true;
  }

  const reqLabel = requests ? ` (${badgeText(requests)} demande${requests > 1 ? "s" : ""})` : "";
  const online = friends.filter((f) => f.presence === "online").length;

  return (
    <div className="home">
      <nav className="home-rail" aria-label="Instances">
        {(instances ?? []).map((i) => (
          <button
            key={i.id} type="button" className="home-rail-btn" title={i.name}
            aria-label={i.name} aria-pressed={i.id === selected} onClick={() => setSelected(i.id)}
          >
            {i.name.charAt(0).toUpperCase()}
          </button>
        ))}
        <button type="button" className="home-rail-btn home-rail-add" aria-label="Nouvelle instance" title="Nouvelle instance" onClick={() => navigate("create")}>
          <Plus />
        </button>
        <div style={{ marginTop: "auto" }}>
          <button type="button" className="home-ibtn" aria-label="Paramètres de Cubelink" title="Paramètres de Cubelink" onClick={() => navigate("settings")}>
            <Gear />
          </button>
        </div>
      </nav>

      <section className="home-main" aria-label="Accueil">
        <img className="home-bg" src={heroImg} alt="" aria-hidden="true" />
        <div className="home-shade" aria-hidden="true" />

        {instances === null ? (
          <p role="status" className="home-status" style={{ position: "relative" }}>Chargement des instances…</p>
        ) : current ? (
          <>
            <div className="home-top">
              <span className="home-pill"><i aria-hidden="true" />{online}&nbsp;ami{online > 1 ? "s" : ""} en ligne</span>
            </div>
            <div className="home-hero">
              <h1>{current.name}</h1>
              <p>{modsLabel(current.mod_count)}, installés une seule fois et partagés entre tes instances.</p>
              <div className="home-actions">
                <button type="button" className="home-play" disabled={busy} onClick={play}>
                  <Play />{busy ? "En cours…" : "Jouer"}
                </button>
              </div>
              <div className={`home-status${isError ? " is-error" : ""}`} role="status" aria-live="polite">{status}</div>
              <span className="home-meta">
                Minecraft&nbsp;{current.minecraft} · {loaderLabel(current)}
                {current.loader.loader !== "vanilla" ? ` ${current.loader.version}` : ""}
              </span>
            </div>
            <section className="home-list" aria-label="Mes instances">
              <h2>Mes instances</h2>
              <div className="home-grid">
                {instances.map((i) => (
                  <button key={i.id} type="button" className="home-card" aria-pressed={i.id === selected} onClick={() => setSelected(i.id)}>
                    <span className="t">{loaderLabel(i)} · {modsLabel(i.mod_count)}</span>
                    <span className="n">{i.name}</span>
                  </button>
                ))}
              </div>
            </section>
          </>
        ) : (
          <div className="home-empty">
            <h1>Aucune instance</h1>
            <p>Crée ta première instance pour commencer à jouer, seul ou avec tes amis.</p>
            {loadError && <div className="home-status is-error" role="alert">{loadError}</div>}
            <button type="button" className="home-play" onClick={() => navigate("create")}>
              <Plus />Créer une instance
            </button>
          </div>
        )}
      </section>

      <aside className="home-friends" aria-label="Amis">
        <h2>Amis · {friends.length}</h2>
        <ul>
          {friends.map((f) => (
            <li key={f.friend.public_key} className="home-friend">
              <span className={`home-dot${f.presence === "online" ? " on" : ""}`} aria-hidden="true" />
              <span className="nm">{f.friend.name}<span className="st">{f.presence === "online" ? "En ligne" : "Hors ligne"}</span></span>
              {f.presence === "online" && (
                <button type="button" className="home-btn" aria-label={`Rejoindre ${f.friend.name}`} onClick={() => navigate("join", { friend: f.friend.public_key })}>
                  Rejoindre
                </button>
              )}
            </li>
          ))}
        </ul>
        <div className="home-friends-foot">
          <button type="button" className="home-btn" style={{ width: "100%" }} aria-label={`Amis${reqLabel}`} onClick={() => navigate("friends")}>
            Amis
            {requests ? <span className="home-badge" aria-hidden="true">{badgeText(requests)}</span> : null}
          </button>
        </div>
      </aside>
    </div>
  );
}

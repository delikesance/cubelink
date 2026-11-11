import { useCallback, useEffect, useRef, useState, type KeyboardEvent, type MouseEvent } from "react";
import "./main.css";
import { useScreenTitle, useShell } from "../../app/context";
import { peekImportWarning, setImportWarning } from "../../app/importWarning";
import type { FriendWithPresence, HostState, Instance, InstancePatch, LaunchState } from "../../services";
import { Button, Drawer, IconButton, StatusDot } from "../../ui";
import { Avatar, Backdrop, safeUrl, sourceLabel } from "./ArtParts";
import { clearArtCache } from "./artImage";
import { ContextMenu, type MenuItem } from "./ContextMenu";
import { LogsPanel } from "./LogsPanel";
import { ModsPanel } from "./ModsPanel";
import { Modal } from "./Modal";
import { ICONS, Icon, Panel } from "./Panel";
import { SettingsDialog } from "./SettingsDialog";
import { Viewer } from "./Viewer";

const LOADERS: Record<string, string> = {
  vanilla: "Vanilla", neoforge: "NeoForge", fabric: "Fabric", quilt: "Quilt", forge: "Forge",
};
const loaderLabel = (i: Instance) => LOADERS[i.loader.loader] ?? i.loader.loader;
const modsLabel = (n: number) => `${n} mod${n > 1 ? "s" : ""}`;
/** « Minecraft 1.21.1 · NeoForge 21.1.250 · 329 mods ». */
const metaLine = (i: Instance) =>
  [`Minecraft ${i.minecraft}`, `${loaderLabel(i)}${i.loader.loader !== "vanilla" && i.loader.version ? ` ${i.loader.version}` : ""}`, modsLabel(i.mod_count)].join(" · ");

export function badgeText(total: number): string {
  return total > 99 ? "99+" : String(total);
}

const clock = (ms: number) => {
  const s = Math.max(0, Math.floor(ms / 1000));
  const p = (n: number) => String(n).padStart(2, "0");
  return `${p(Math.floor(s / 3600))}:${p(Math.floor((s % 3600) / 60))}:${p(s % 60)}`;
};

interface MenuState { id: string; x: number; y: number; trigger: HTMLElement }
type DialogKind = "settings" | "logs" | "delete" | "mods" | "friends";
type DialogState = { kind: DialogKind; id: string } | null;

type ArtBusy = { id: string; kind: "lookup" | "banner" | "icon" } | null;
type ArtMsg = { id: string; kind: "ok" | "error"; text: string } | null;

// Retire le préfixe technique du backend (« introuvable : », « entrée invalide : »…).
const errMsg = (e: unknown) => (e instanceof Error ? e.message : String(e)).replace(/^[a-zéèêàçùîô' -]{3,24}\s:\s+/, "");

export default function Main() {
  const { services, navigate } = useShell();
  const [instances, setInstances] = useState<Instance[] | null>(null);
  const [selected, setSelected] = useState<string | null>(null);
  const [launch, setLaunch] = useState<LaunchState | null>(null);
  const [launchError, setLaunchError] = useState<string | null>(null);
  const [requests, setRequests] = useState<number | null>(null);
  const [friends, setFriends] = useState<FriendWithPresence[]>([]);
  const [loadError, setLoadError] = useState<string | null>(null);
  const [host, setHost] = useState<HostState | null>(null);

  const [menu, setMenu] = useState<MenuState | null>(null);
  const [dialog, setDialog] = useState<DialogState>(null);
  const [actionError, setActionError] = useState<string | null>(null);
  const [artBusy, setArtBusy] = useState<ArtBusy>(null);
  const [artMsg, setArtMsg] = useState<ArtMsg>(null);
  const [viewer, setViewer] = useState<number | null>(null);
  const [modsTotal, setModsTotal] = useState<number | null>(null);
  const [warning, setWarning] = useState<string[] | null>(() => peekImportWarning());
  const [now, setNow] = useState(() => Date.now());
  const runningSince = useRef(new Map<string, number>());
  /** Dernier tiroir ouvert : son contenu reste affiché pendant l'animation de sortie (280 ms). */
  const lastDialog = useRef<DialogState>(null);
  if (dialog) lastDialog.current = dialog;

  useScreenTitle(null);

  useEffect(() => { setImportWarning(null); }, []);
  useEffect(() => services.onImportWarning((e) => { if (e.missing.length) setWarning(e.missing); }), [services]);

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

  const track = useCallback((s: LaunchState) => {
    if (s.state === "running") { if (!runningSince.current.has(s.id)) runningSince.current.set(s.id, Date.now()); }
    else runningSince.current.delete(s.id);
  }, []);
  useEffect(() => services.onLaunchState((s) => { track(s); setLaunch(s); setLaunchError(null); }), [services, track]);
  useEffect(() => services.onFriendPresence((e) => {
    setFriends((l) => l.map((f) => (f.friend.public_key === e.public_key ? { ...f, presence: e.presence } : f)));
  }), [services]);
  useEffect(() => {
    let alive = true;
    services.hostState().then((h) => { if (alive) setHost(h); }, () => {});
    const off = services.onHostState((h) => setHost(h));
    return () => { alive = false; off(); };
  }, [services]);
  useEffect(() => services.onFriendRequest(() => setRequests((n) => (n ?? 0) + 1)), [services]);

  const homeRef = useRef<HTMLDivElement>(null);
  // Focus cohérent au retour sur l'accueil : bouton Jouer une fois chargé, sauf si le focus est déjà ailleurs.
  const ready = instances !== null;
  useEffect(() => {
    if (!ready) return;
    const a = document.activeElement;
    if (a && a !== document.body && homeRef.current?.contains(a)) return;
    (homeRef.current?.querySelector<HTMLElement>(".home-play") ?? homeRef.current?.querySelector<HTMLElement>(".home-rail-btn"))?.focus();
  }, [ready]);

  /** Flèches haut/bas, Home et Fin dans la barre latérale des instances. */
  const onRailKey = (e: KeyboardEvent<HTMLElement>) => {
    if (!["ArrowUp", "ArrowDown", "Home", "End"].includes(e.key)) return;
    const items = Array.from(e.currentTarget.querySelectorAll<HTMLElement>(".home-rail-btn"));
    const i = items.indexOf(document.activeElement as HTMLElement);
    if (i < 0) return;
    e.preventDefault();
    const n = e.key === "Home" ? 0 : e.key === "End" ? items.length - 1
      : (i + (e.key === "ArrowDown" ? 1 : items.length - 1)) % items.length;
    items[n].focus();
  };

  const current = instances?.find((i) => i.id === selected) ?? null;
  const state = launch && launch.id === selected ? launch : null;
  const busy = state?.state === "preparing" || state?.state === "running";
  const running = state?.state === "running";

  // Chronomètre de session : une seconde de précision, uniquement pendant le jeu.
  useEffect(() => {
    if (!running) return;
    setNow(Date.now());
    const t = setInterval(() => setNow(Date.now()), 1000);
    return () => clearInterval(t);
  }, [running]);

  const play = useCallback(async (id: string) => {
    setSelected(id);
    setLaunchError(null);
    setActionError(null);
    setLaunch({ id, state: "preparing" });
    try {
      await services.instancesLaunch(id);
    } catch (e) {
      setLaunch(null);
      setLaunchError(errMsg(e));
    }
  }, [services]);

  const run = useCallback(async (fn: () => Promise<unknown>) => {
    setActionError(null);
    try { await fn(); } catch (e) { setActionError(errMsg(e)); }
  }, []);

  /** Remplace l'instance mise à jour (illustration) dans la liste : décor, rail et titre suivent. */
  const applyInstance = (u: Instance) => setInstances((l) => (l ?? []).map((i) => (i.id === u.id ? u : i)));

  const artRun = async (id: string, kind: "lookup" | "banner" | "icon") => {
    if (artBusy) return;
    setArtMsg(null);
    setArtBusy({ id, kind });
    try {
      if (kind === "lookup") {
        const u = await services.instancesArtLookup(id);
        clearArtCache();
        applyInstance(u);
        setArtMsg(u.art ? null : { id, kind: "ok", text: "Aucune illustration trouvée." });
      } else {
        const files = await services.pickFiles({ extensions: ["png", "jpg", "jpeg", "webp"] });
        if (!files.length) return;
        const u = await services.instancesArtSet(id, files[0], kind);
        clearArtCache();
        applyInstance(u);
        setArtMsg({ id, kind: "ok", text: kind === "icon" ? "Icône définie." : "Illustration définie." });
      }
    } catch (e) {
      setArtMsg({ id, kind: "error", text: errMsg(e) });
    } finally {
      setArtBusy(null);
    }
  };

  // Illustration par défaut : une recherche silencieuse par instance et par session pour celles qui n'en ont pas.
  // Sans résultat, l'image par défaut (blocs) reste en place ; l'utilisateur peut toujours choisir une image.
  const autoTried = useRef(new Set<string>());
  useEffect(() => {
    if (!instances) return;
    const todo = instances.filter((i) => !i.art && !autoTried.current.has(i.id));
    if (!todo.length) return;
    todo.forEach((i) => autoTried.current.add(i.id));
    void (async () => {
      for (const i of todo) {
        try {
          const u = await services.instancesArtLookup(i.id);
          if (u.art) { clearArtCache(); applyInstance(u); }
        } catch { /* silencieux : pas de correspondance ou hors ligne */ }
      }
    })();
  }, [instances, services]);

  const kill = (id: string) => run(() => services.instancesKill(id));

  const closeDialog = () => setDialog(null);

  const remove = async (id: string) => {
    setDialog(null);
    await run(async () => {
      await services.instancesDelete(id);
      const next = (instances ?? []).filter((i) => i.id !== id);
      setInstances(next);
      setSelected((cur) => (cur === id ? next[0]?.id ?? null : cur));
      setLaunch((l) => (l?.id === id ? null : l));
    });
  };

  const save = async (id: string, patch: InstancePatch) => {
    const updated = await services.instancesUpdate(id, patch);
    setInstances((l) => (l ?? []).map((i) => (i.id === id ? updated : i)));
    setDialog(null);
  };

  const isBusy = (id: string) => launch?.id === id && (launch.state === "preparing" || launch.state === "running");

  const openMenuAt = (id: string, trigger: HTMLElement, x: number, y: number) => setMenu({ id, x, y, trigger });
  /** Clic droit, touche Menu et Maj+F10 sur une instance. */
  const menuProps = (id: string) => ({
    onContextMenu: (e: MouseEvent<HTMLElement>) => {
      e.preventDefault();
      openMenuAt(id, e.currentTarget, e.clientX, e.clientY);
    },
    onKeyDown: (e: KeyboardEvent<HTMLElement>) => {
      if (e.key === "ContextMenu" || (e.shiftKey && e.key === "F10")) {
        e.preventDefault();
        const r = e.currentTarget.getBoundingClientRect();
        openMenuAt(id, e.currentTarget, r.left + 8, r.bottom);
      }
    },
  });

  const menuItems = (id: string): MenuItem[] => {
    const act = (fn: () => void) => () => { menu?.trigger.focus(); fn(); };
    const inst = instances?.find((i) => i.id === id);
    const shots = inst?.art?.screenshots.length ?? 0;
    return [
      isBusy(id)
        ? { key: "stop", label: "Arrêter", icon: <Icon d={ICONS.stop} />, onSelect: act(() => { void kill(id); }) }
        : { key: "play", label: "Jouer", icon: <Icon d={ICONS.play} />, onSelect: act(() => { void play(id); }) },
      { key: "settings", label: "Paramètres", icon: <Icon d={ICONS.gear} />, onSelect: act(() => setDialog({ kind: "settings", id })) },
      { key: "logs", label: "Logs", icon: <Icon d={ICONS.logs} />, onSelect: act(() => setDialog({ kind: "logs", id })) },
      { key: "mods", label: "Mods", icon: <Icon d={ICONS.mods} />, onSelect: act(() => setDialog({ kind: "mods", id })) },
      { key: "browse", label: "Parcourir des mods", icon: <Icon d={ICONS.search} />, onSelect: act(() => navigate("browse", { kind: "mod", instance: id })) },
      ...(shots > 0 ? [{ key: "shots", label: `Voir les captures (${shots})`, icon: <Icon d={ICONS.film} />, separator: true, onSelect: act(() => { setSelected(id); setViewer(0); }) }] : []),
      { key: "art", label: inst?.art ? "Changer l'illustration" : "Choisir une illustration", icon: <Icon d={ICONS.image} />, separator: shots === 0, onSelect: act(() => { void artRun(id, "banner"); }) },
      { key: "icon", label: "Définir l'icône", icon: <Icon d={ICONS.image} />, onSelect: act(() => { void artRun(id, "icon"); }) },
      { key: "lookup", label: "Chercher automatiquement", icon: <Icon d={ICONS.wand} />, onSelect: act(() => { void artRun(id, "lookup"); }) },
      { key: "folder", label: "Ouvrir le dossier", icon: <Icon d={ICONS.folder} />, separator: true, onSelect: act(() => { void run(() => services.instancesOpenFolder(id)); }) },
      { key: "delete", label: "Supprimer", icon: <Icon d={ICONS.trash} />, danger: true, onSelect: act(() => setDialog({ kind: "delete", id })) },
    ];
  };

  const reqLabel = requests ? ` (${badgeText(requests)} demande${requests > 1 ? "s" : ""})` : "";
  const online = friends.filter((f) => f.presence === "online").length;

  const dr = dialog ?? lastDialog.current;
  const target = dr ? instances?.find((i) => i.id === dr.id) ?? null : null;
  const delTarget = dialog?.kind === "delete" ? instances?.find((i) => i.id === dialog.id) ?? null : null;
  const since = current ? runningSince.current.get(current.id) : undefined;

  const failTitle = "Échec du lancement";
  const failMsg = launchError ?? (state?.state === "failed" ? state.message ?? "" : null);
  const failed = launchError !== null || state?.state === "failed";

  return (
    <div className="home" ref={homeRef}>
      <Backdrop services={services} inst={current} />

      <nav className="home-rail" aria-label="Instances" onKeyDown={onRailKey}>
        {(instances ?? []).map((i) => (
          <div key={i.id} className="home-rail-item">
            {i.id === selected && <span className="home-rail-mark" aria-hidden="true" />}
            <button
              type="button" className="home-rail-btn" title={i.name}
              aria-label={i.name} aria-pressed={i.id === selected} onClick={() => setSelected(i.id)}
              {...menuProps(i.id)}
            >
              <Avatar services={services} inst={i} className="home-av" />
            </button>
          </div>
        ))}
        <button type="button" className="home-rail-btn home-rail-add" aria-label="Nouvelle instance" title="Nouvelle instance" onClick={() => navigate("create")}>
          <Icon d={ICONS.plus} size={20} />
        </button>
        <div className="home-rail-end">
          <button type="button" className="home-rail-btn home-rail-set" aria-label="Paramètres de Cubelink" title="Paramètres de Cubelink" onClick={() => navigate("settings")}>
            <Icon d={ICONS.gear} size={22} />
          </button>
        </div>
      </nav>

      <button
        type="button" className="home-pill" aria-haspopup="dialog" aria-expanded={dialog?.kind === "friends"}
        aria-label={`Amis${reqLabel}, ${online} en ligne`}
        onClick={() => setDialog({ kind: "friends", id: "" })}
      >
        <span className={`home-pill-dot${online ? " on" : ""}`} aria-hidden="true" />
        Amis · {online} en ligne
        {requests ? <span className="home-badge" aria-hidden="true">{badgeText(requests)}</span> : null}
      </button>

      <div className="home-stack">
        {warning && current && (
          <div className="home-warn enter" role="status">
            <span>
              Import terminé, mais {warning.length} mod{warning.length > 1 ? "s" : ""} introuvable{warning.length > 1 ? "s" : ""} :{" "}
              {warning.slice(0, 5).join(", ")}{warning.length > 5 ? `… (+${warning.length - 5})` : ""}.
            </span>
            <Button variant="secondary" onClick={() => setWarning(null)}>Ignorer</Button>
          </div>
        )}

        {instances === null ? (
          <div className="home-card" aria-label="Accueil" role="region">
            <p role="status" className="home-loading">Chargement des instances…</p>
          </div>
        ) : current ? (
          <section className="home-card" aria-label={current.name} key={current.id}>
            <h1 className="enter" style={{ "--i": 0 } as React.CSSProperties}>{current.name}</h1>
            <p className="home-meta enter" style={{ "--i": 1 } as React.CSSProperties}>{metaLine(current)}</p>

            <div className="home-play-zone enter" style={{ "--i": 3 } as React.CSSProperties}>
              {state?.state === "preparing" ? (
                <>
                  <div className="home-prep" role="progressbar" aria-label="Préparation du jeu" aria-busy="true">
                    <span className="home-prep-fill" aria-hidden="true" />
                    <span className="home-prep-label" role="status">Préparation du jeu…</span>
                  </div>
                  <IconButton variant="secondary" aria-label="Annuler le lancement" icon={<Icon d={ICONS.close} />} onClick={() => { void kill(current.id); }} />
                </>
              ) : running ? (
                <Button variant="danger" size="lg" className="home-play is-danger" icon={<Icon d={ICONS.stop} size={20} fill />} onClick={() => { void kill(current.id); }}>
                  Arrêter
                </Button>
              ) : (
                <Button size="lg" className="home-play" icon={<Icon d={ICONS.play} size={20} fill />} onClick={() => { void play(current.id); }}>
                  Jouer
                </Button>
              )}
            </div>

            {running && (
              <p className="home-state enter">
                <StatusDot tone="ok" label="En jeu" />
                {since !== undefined && <span className="home-session"> · session <time>{clock(now - since)}</time></span>}
              </p>
            )}
            {running && (
              <p className="home-hint" role="status">
                {host && host.instance_id === current.id && host.sharing && host.world_port
                  ? "Tes amis peuvent te rejoindre"
                  : "Ouvre ton monde au réseau local pour que tes amis te rejoignent"}
              </p>
            )}
            {failed && (
              <div className="home-fail enter" role="alert">
                <span className="home-fail-ic" aria-hidden="true"><Icon d={ICONS.alert} size={20} /></span>
                <div className="home-fail-txt">
                  <div className="t">{failTitle}</div>
                  {failMsg && <div className="m">{failMsg}</div>}
                </div>
                <Button variant="secondary" onClick={() => setDialog({ kind: "logs", id: current.id })}>Voir les logs</Button>
              </div>
            )}
            {actionError && <p className="home-err" role="alert">{actionError}</p>}
            {artBusy?.id === current.id && <p className="home-artmsg" role="status">{artBusy.kind === "lookup" ? "Recherche du modpack en cours…" : "Enregistrement de l'image…"}</p>}
            {!artBusy && artMsg?.id === current.id && (artMsg.kind === "error"
              ? <p className="home-artmsg is-error" role="alert">{artMsg.text}</p>
              : <p className="home-artmsg" role="status">{artMsg.text}</p>)}

            <div className="home-row enter" style={{ "--i": 4 } as React.CSSProperties}>
              <Button variant="secondary" icon={<Icon d={ICONS.mods} />} onClick={() => setDialog({ kind: "mods", id: current.id })}>Mods</Button>
              <Button variant="secondary" icon={<Icon d={ICONS.gear} />} onClick={() => setDialog({ kind: "settings", id: current.id })}>Paramètres</Button>
              <Button variant="secondary" icon={<Icon d={ICONS.logs} />} onClick={() => setDialog({ kind: "logs", id: current.id })}>Logs</Button>
              <IconButton
                variant="secondary" aria-label={`Actions de ${current.name}`} aria-haspopup="menu" aria-expanded={menu?.id === current.id}
                icon={<Icon d={ICONS.dots} size={20} />}
                onClick={(e) => { const r = e.currentTarget.getBoundingClientRect(); openMenuAt(current.id, e.currentTarget, r.left, r.bottom + 6); }}
                onKeyDown={menuProps(current.id).onKeyDown}
              />
            </div>
          </section>
        ) : (
          <section className="home-card" aria-label="Accueil">
            <h1 className="enter">Aucune instance</h1>
            <p className="home-meta enter" style={{ "--i": 1 } as React.CSSProperties}>Crée ta première instance pour commencer à jouer, seul ou avec tes amis.</p>
            {loadError && <p className="home-err" role="alert">{loadError}</p>}
            <div className="home-play-zone enter" style={{ "--i": 2 } as React.CSSProperties}>
              <Button icon={<Icon d={ICONS.plus} />} onClick={() => navigate("create")}>Créer une instance</Button>
            </div>
          </section>
        )}
      </div>

      {current?.art && (
        <p className="home-credit">
          <span>{current.art.author ? `par ${current.art.author}` : "Illustration du modpack"}</span>
          <span>{sourceLabel(current.art.source) ? `Source : ${sourceLabel(current.art.source)}` : "Illustration personnalisée"}</span>
          {safeUrl(current.art.page_url) && (
            <a href={safeUrl(current.art.page_url)!} target="_blank" rel="noopener noreferrer">Page du modpack</a>
          )}
        </p>
      )}

      {menu && instances?.some((i) => i.id === menu.id) && (
        <ContextMenu
          label={`Actions de ${instances.find((i) => i.id === menu.id)!.name}`}
          x={menu.x} y={menu.y} items={menuItems(menu.id)}
          onClose={(restore) => { if (restore) menu.trigger.focus(); setMenu(null); }}
        />
      )}

      <Drawer open={dialog?.kind === "mods"} onClose={closeDialog} title={modsTotal === null ? "Mods" : `Mods · ${modsTotal}`} width={520}>
        {dr?.kind === "mods" && target && (
          <ModsPanel
            services={services} instanceId={target.id} onClose={closeDialog} onTotal={setModsTotal}
            onCount={(n) => setInstances((l) => (l ?? []).map((i) => (i.id === target.id && i.mod_count !== n ? { ...i, mod_count: n } : i)))}
            onBrowse={() => navigate("browse", { kind: "mod", instance: target.id })}
          />
        )}
      </Drawer>
      <Drawer open={dialog?.kind === "settings"} onClose={closeDialog} title={`Paramètres de ${target?.name ?? ""}`} width={520}>
        {dr?.kind === "settings" && target && (
          <SettingsDialog
            key={target.id} instance={target} onSave={(patch) => save(target.id, patch)} onClose={closeDialog}
            onOpenFolder={() => { void run(() => services.instancesOpenFolder(target.id)); }}
            onDelete={() => { void remove(target.id); }}
          />
        )}
      </Drawer>
      <Drawer open={dialog?.kind === "logs"} onClose={closeDialog} title="Logs" width={860}>
        {dr?.kind === "logs" && target && <LogsPanel services={services} instanceId={target.id} onClose={closeDialog} />}
      </Drawer>
      <Drawer open={dialog?.kind === "friends"} onClose={closeDialog} title={`Amis · ${friends.length}`} width={360}>
        <Panel onClose={closeDialog}>
          <div className="dr-body">
            {friends.length === 0 ? <p className="dr-empty">Aucun ami pour l'instant.</p> : (
              <ul className="dr-list dr-friends" aria-label="Amis">
                {friends.map((f) => (
                  <li key={f.friend.public_key} className="dr-row">
                    <span className="dr-nm">
                      <span className="n">{f.friend.name}</span>
                      <StatusDot tone={f.presence === "online" ? "ok" : "neutral"} label={f.presence === "online" ? "En ligne" : "Hors ligne"} />
                    </span>
                    {f.presence === "online" && (
                      <Button variant="secondary" aria-label={`Rejoindre ${f.friend.name}`} onClick={() => navigate("join", { friend: f.friend.public_key })}>
                        Rejoindre
                      </Button>
                    )}
                  </li>
                ))}
              </ul>
            )}
          </div>
          <footer className="dr-foot">
            <Button icon={<Icon d={ICONS.friends} />} aria-label={`Amis${reqLabel}`} onClick={() => navigate("friends")}>
              Amis{requests ? <span className="home-badge" aria-hidden="true">{badgeText(requests)}</span> : null}
            </Button>
          </footer>
        </Panel>
      </Drawer>

      {viewer !== null && current?.art && current.art.screenshots.length > 0 && (
        <Viewer
          services={services} instanceId={current.id} title={current.name}
          files={current.art.screenshots} start={viewer} onClose={() => setViewer(null)}
        />
      )}
      {delTarget && (
        <Modal title="Supprimer l'instance ?" role="alertdialog" onClose={closeDialog}>
          <p className="home-modal-text">
            L'instance « {delTarget.name} » et ses fichiers (mondes, configuration) seront supprimés définitivement.
          </p>
          <div className="home-modal-foot">
            <Button variant="secondary" data-autofocus onClick={closeDialog}>Annuler</Button>
            <Button variant="danger" onClick={() => { void remove(delTarget.id); }}>Supprimer</Button>
          </div>
        </Modal>
      )}
    </div>
  );
}

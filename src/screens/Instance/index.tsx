// Page d'un modpack (route `instance?id=…&tab=…`) : en-tête (jouer, amis sur le pack), onglets Aperçu · Mods · Réglages · Logs.
import { useEffect, useId, useRef, useState, type KeyboardEvent, type ReactNode } from "react";
import "./instance.css";
import { useScreenTitle, useShell } from "../../app/context";
import { errMsg, useLaunch } from "../../app/useLaunch";
import { canInviteFrom, useHostState, useInvites } from "../../app/useInvites";
import type { Instance, InstancePatch, Services } from "../../services";
import { Button, IconButton, StatusDot } from "../../ui";
import { Avatar, ThumbStrip, sourceLabel } from "../Main/ArtParts";
import { clearArtCache, useArtImage } from "../Main/artImage";
import { ContextMenu, type MenuItem } from "../Main/ContextMenu";
import { useContextMenu } from "../../app/useContextMenu";
import { useModCheckGate } from "../../app/useModCheck";
import { sessionsFrom } from "../Main/index";
import { LogsPanel } from "../Main/LogsPanel";
import { Modal } from "../Main/Modal";
import { ModsPanel } from "../Main/ModsPanel";
import { ICONS, Icon } from "../../ui";
import { SettingsDialog } from "../Main/SettingsDialog";
import { Viewer } from "../Main/Viewer";

const LOADERS: Record<string, string> = { vanilla: "Vanilla", neoforge: "NeoForge", fabric: "Fabric", quilt: "Quilt", forge: "Forge" };
const loaderLabel = (i: Instance) => LOADERS[i.loader.loader] ?? i.loader.loader;
const modsLabel = (n: number) => `${n} mod${n > 1 ? "s" : ""}`;
/** « Minecraft 1.21.1 · NeoForge 21.1.250 · 329 mods ». */
const metaLine = (i: Instance) =>
  [`Minecraft ${i.minecraft}`, `${loaderLabel(i)}${i.loader.loader !== "vanilla" && i.loader.version ? ` ${i.loader.version}` : ""}`, modsLabel(i.mod_count)].join(" · ");

const clock = (ms: number) => {
  const s = Math.max(0, Math.floor(ms / 1000));
  const p = (n: number) => String(n).padStart(2, "0");
  return `${p(Math.floor(s / 3600))}:${p(Math.floor((s % 3600) / 60))}:${p(s % 60)}`;
};

const TABS = [
  { id: "apercu", label: "Aperçu" },
  { id: "mods", label: "Mods" },
  { id: "reglages", label: "Réglages" },
  { id: "logs", label: "Logs" },
] as const;
type TabId = (typeof TABS)[number]["id"];

/**
 * Bannière d'ambiance en haut de la page : première capture du pack (sinon son icône), floutée et estompée vers le fond.
 * Sans image : rien (le fond reste uni).
 */
function Hero({ services, inst }: { services: Parameters<typeof useArtImage>[0]; inst: Instance }) {
  const shot = useArtImage(services, inst.id, inst.art?.screenshots[0] ?? null, false, 1280);
  const icon = useArtImage(services, inst.id, shot.status === "error" || !inst.art?.screenshots[0] ? inst.art?.icon : null, false);
  const src = shot.status === "ready" ? shot.src : icon.status === "ready" ? icon.src : null;
  if (!src) return null;
  return (
    <div className={`ipg-hero${shot.status === "ready" ? "" : " is-icon"}`} aria-hidden="true">
      <img src={src} alt="" onError={shot.status === "ready" ? shot.markError : icon.markError} />
    </div>
  );
}

/**
 * Description complète du modpack (page Modrinth / CurseForge), HTML nettoyé côté Rust (ammonia) :
 * pas de script ni d'attribut d'événement, liens en target=_blank ouverts dans le navigateur par externalLinks.
 */
function LongDescription({ services, id, project }: { services: Pick<Services, "instancesDescription">; id: string; project: string }) {
  const [state, setState] = useState<{ html: string | null; error: boolean } | null>(null);
  // Images de l'hébergeur introuvables (lien vers une page au lieu d'une image…) : masquées plutôt qu'une icône cassée.
  const hideBroken = (e: React.SyntheticEvent<HTMLDivElement>) => { if (e.target instanceof HTMLImageElement) e.target.hidden = true; };
  useEffect(() => {
    let alive = true;
    setState(null);
    services.instancesDescription(id).then(
      (html) => { if (alive) setState({ html, error: false }); },
      () => { if (alive) setState({ html: null, error: true }); },
    );
    return () => { alive = false; };
  }, [services, id, project]);
  if (state && !state.html && !state.error) return null;
  return state === null ? <p className="ipg-meta" role="status">Chargement de la description…</p>
    : state.error ? <p className="ipg-meta" role="status">Description complète indisponible pour le moment (hors ligne ?).</p>
    : <div className="ipg-prose" onErrorCapture={hideBroken} dangerouslySetInnerHTML={{ __html: state.html! }} />;
}

type ArtKind = "lookup" | "banner" | "icon";
type ArtMsg = { kind: "ok" | "error"; text: string } | null;

export default function InstancePage() {
  const { services, params, navigate } = useShell();
  const id = params.id ?? "";
  const uid = useId();
  const { launch, error: launchError, since: sinceMap, play, kill, forget, isBusy } = useLaunch(services);
  const invites = useInvites(services);
  const host = useHostState(services);

  const [instances, setInstances] = useState<Instance[] | null>(null);
  const [loadError, setLoadError] = useState<string | null>(null);
  const [tab, setTab] = useState<TabId>(() => TABS.find((t) => t.id === params.tab)?.id ?? (params.fix === "1" ? "mods" : "apercu"));
  const [menu, setMenu] = useState<{ x: number; y: number; trigger: HTMLElement } | null>(null);
  const [confirmDel, setConfirmDel] = useState(false);
  const ctx = useContextMenu();
  const gate = useModCheckGate(services);
  const [autoFix, setAutoFix] = useState(params.fix === "1");
  const [viewer, setViewer] = useState<number | null>(null);
  const [actionError, setActionError] = useState<string | null>(null);
  const [artBusy, setArtBusy] = useState<ArtKind | null>(null);
  const [artMsg, setArtMsg] = useState<ArtMsg>(null);
  const [now, setNow] = useState(() => Date.now());
  const tabRefs = useRef<Record<string, HTMLButtonElement | null>>({});
  const h1 = useRef<HTMLHeadingElement>(null);

  const inst = instances?.find((i) => i.id === id) ?? null;
  useScreenTitle(inst?.name ?? null);

  useEffect(() => {
    let alive = true;
    services.instancesList().then(
      (l) => { if (alive) setInstances(l); },
      (e: unknown) => { if (alive) { setInstances([]); setLoadError(errMsg(e)); } },
    );
    return () => { alive = false; };
  }, [services]);

  // Focus sur le titre une fois la page chargée (lecteurs d'écran, retour clavier cohérent).
  const ready = inst !== null;
  useEffect(() => { if (ready) h1.current?.focus(); }, [ready]);

  const state = launch && launch.id === id ? launch : null;
  const running = state?.state === "running";
  const preparing = state?.state === "preparing";
  const since = sinceMap[id];

  // Chronomètre de session : une seconde de précision, uniquement pendant le jeu.
  useEffect(() => {
    if (!running) return;
    setNow(Date.now());
    const t = setInterval(() => setNow(Date.now()), 1000);
    return () => clearInterval(t);
  }, [running]);

  const patchInstance = (u: Instance) => setInstances((l) => (l ?? []).map((i) => (i.id === u.id ? u : i)));
  const run = async (fn: () => Promise<unknown>) => {
    setActionError(null);
    try { await fn(); } catch (e) { setActionError(errMsg(e)); }
  };

  const artRun = async (kind: ArtKind) => {
    if (artBusy || !inst) return;
    setArtMsg(null);
    setArtBusy(kind);
    try {
      if (kind === "lookup") {
        const u = await services.instancesArtLookup(inst.id);
        clearArtCache();
        patchInstance(u);
        setArtMsg(u.art ? null : { kind: "ok", text: "Aucune illustration trouvée." });
      } else {
        const files = await services.pickFiles({ extensions: ["png", "jpg", "jpeg", "webp"] });
        if (!files.length) return;
        const u = await services.instancesArtSet(inst.id, files[0], kind);
        clearArtCache();
        patchInstance(u);
        setArtMsg({ kind: "ok", text: kind === "icon" ? "Icône définie." : "Illustration définie." });
      }
    } catch (e) {
      setArtMsg({ kind: "error", text: errMsg(e) });
    } finally {
      setArtBusy(null);
    }
  };

  const save = async (patch: InstancePatch) => {
    const updated = await services.instancesUpdate(id, patch);
    patchInstance(updated);
  };

  const remove = async () => {
    setConfirmDel(false);
    await run(async () => {
      await services.instancesDelete(id);
      forget(id);
      navigate("main");
    });
  };

  const guardedPlay = () => {
    setActionError(null);
    if (inst) gate.guard(inst, () => { void play(id); }, () => goTab("mods"), () => { goTab("mods"); setAutoFix(true); });
  };
  const goTab = (t: TabId, focus = false) => {
    setTab(t);
    if (focus) tabRefs.current[t]?.focus();
  };
  const onTabKey = (e: KeyboardEvent<HTMLElement>) => {
    const i = TABS.findIndex((t) => t.id === tab);
    const last = TABS.length - 1;
    const n = e.key === "ArrowRight" ? (i + 1) % TABS.length : e.key === "ArrowLeft" ? (i + last) % TABS.length : e.key === "Home" ? 0 : e.key === "End" ? last : -1;
    if (n < 0) return;
    e.preventDefault();
    goTab(TABS[n].id, true);
  };

  if (instances === null) return <main className="ipg"><p role="status" className="ipg-loading">Chargement du modpack…</p></main>;

  if (!inst) {
    return (
      <main className="ipg">
        <div className="ipg-missing">
          <h1 ref={h1} tabIndex={-1}>Modpack introuvable</h1>
          <p className="ipg-meta">{loadError ?? "Ce modpack n'existe plus ou a été supprimé."}</p>
          <Button onClick={() => navigate("main")}>Retour à l'accueil</Button>
        </div>
      </main>
    );
  }

  const shots = inst.art?.screenshots ?? [];
  const packFriends = sessionsFrom(invites).filter((g) => g.pack === inst.name);
  const failed = launchError !== null || state?.state === "failed";
  const failMsg = launchError ?? (state?.state === "failed" ? state.message ?? "" : null);
  const otherBusy = !!launch && launch.id !== id && isBusy(launch.id);
  const tabId = (t: string) => `${uid}-tab-${t}`;
  const panelId = (t: string) => `${uid}-panel-${t}`;

  const menuItems: MenuItem[] = [
    { key: "art", label: inst.art ? "Changer l'illustration" : "Choisir une illustration", icon: <Icon d={ICONS.image} />, onSelect: () => { void artRun("banner"); } },
    { key: "icon", label: "Définir l'icône", icon: <Icon d={ICONS.image} />, onSelect: () => { void artRun("icon"); } },
    { key: "lookup", label: "Chercher automatiquement", icon: <Icon d={ICONS.wand} />, onSelect: () => { void artRun("lookup"); } },
    { key: "browse", label: "Parcourir des mods", icon: <Icon d={ICONS.search} />, separator: true, onSelect: () => navigate("browse", { kind: "mod", instance: id }) },
    { key: "folder", label: "Ouvrir le dossier", icon: <Icon d={ICONS.folder} />, onSelect: () => { void run(() => services.instancesOpenFolder(id)); } },
    { key: "delete", label: "Supprimer", icon: <Icon d={ICONS.trash} />, danger: true, separator: true, onSelect: () => setConfirmDel(true) },
  ];


  return (
    <main className="ipg">
      <Hero services={services} inst={inst} />
      <header
        className="ipg-head"
        onContextMenu={ctx.bind(`Actions de ${inst.name}`, () => [
          running || preparing
            ? { key: "stop", label: preparing ? "Annuler le lancement" : "Arrêter", icon: <Icon d={ICONS.stop} />, onSelect: () => { void kill(id); } }
            : { key: "play", label: "Jouer", icon: <Icon d={ICONS.play} />, disabled: otherBusy || gate.checking, onSelect: guardedPlay },
          ...menuItems.map((m, k) => (k === 0 ? { ...m, separator: true } : m)),
        ])}
      >
        <Avatar services={services} inst={inst} className="home-av ipg-av" />
        <div className="ipg-id">
          <h1 ref={h1} tabIndex={-1}>{inst.name}</h1>
          <p className="ipg-meta">{metaLine(inst)}</p>
        </div>
        <div className="ipg-act">
          {preparing ? (
            <>
              <div className="home-prep ipg-prep" role="progressbar" aria-label="Préparation du jeu" aria-busy="true">
                <span className="home-prep-fill" aria-hidden="true" />
                <span className="home-prep-label" role="status">Préparation du jeu…</span>
              </div>
              <IconButton variant="secondary" aria-label="Annuler le lancement" icon={<Icon d={ICONS.close} />} onClick={() => { void kill(id); }} />
            </>
          ) : running ? (
            <Button variant="danger" size="lg" icon={<Icon d={ICONS.stop} size={20} fill />} onClick={() => { void kill(id).then((m) => { if (m) setActionError(m); }); }}>Arrêter</Button>
          ) : (
            <Button size="lg" disabled={otherBusy || gate.checking} icon={<Icon d={ICONS.play} size={20} fill />} onClick={guardedPlay}>Jouer</Button>
          )}
          <IconButton
            variant="secondary" aria-label={`Actions de ${inst.name}`} aria-haspopup="menu" aria-expanded={menu !== null}
            icon={<Icon d={ICONS.dots} size={20} />}
            onClick={(e) => { const r = e.currentTarget.getBoundingClientRect(); setMenu({ x: Math.max(8, r.right - 240), y: r.bottom + 6, trigger: e.currentTarget }); }}
          />
        </div>
      </header>

      {running && (
        <p className="ipg-state">
          <StatusDot tone="ok" label="En jeu" />
          {since !== undefined && <span className="home-session"> · session <time>{clock(now - since)}</time></span>}
        </p>
      )}
      {failed && (
        <div className="ipg-fail enter" role="alert">
          <span className="ipg-fail-ic" aria-hidden="true"><Icon d={ICONS.alert} size={20} /></span>
          <div className="ipg-fail-txt">
            <div className="t">Échec du lancement</div>
            {failMsg && <div className="m">{failMsg}</div>}
          </div>
          <Button variant="secondary" onClick={() => goTab("logs")}>Voir les logs</Button>
        </div>
      )}
      {actionError && <p className="home-err" role="alert">{actionError}</p>}
      {artBusy && <p className="ipg-note" role="status">{artBusy === "lookup" ? "Recherche du modpack en cours…" : "Enregistrement de l'image…"}</p>}
      {!artBusy && artMsg && (artMsg.kind === "error"
        ? <p className="ipg-note is-error" role="alert">{artMsg.text}</p>
        : <p className="ipg-note" role="status">{artMsg.text}</p>)}

      <section className="ipg-card ipg-friends" aria-label="Amis sur ce pack">
        <div className="ipg-card-h">
          <span><Icon d={ICONS.friends} size={16} />Amis sur ce pack</span>
          {packFriends.length > 0 && <span className="c">{packFriends.length} en partie</span>}
        </div>
        {packFriends.length === 0 ? (
          <p className="ipg-friends-empty">
            Personne n'y joue.{" "}
            {canInviteFrom(host) ? "Invite un ami depuis la liste d'amis de l'accueil." : "Lance une partie et ouvre ton monde pour que tes amis te rejoignent."}
          </p>
        ) : (
          <ul className="ipg-friends-list">
            {packFriends.map((g) => (
              <li key={g.id} className="ipg-friend">
                <span className="ipg-friend-txt">
                  <span className="n">{g.world_name}</span>
                  <span className="s">{g.name}</span>
                </span>
                <Button aria-label={`Rejoindre ${g.name}`} onClick={() => navigate("join", { friend: g.public_key })}>Rejoindre</Button>
              </li>
            ))}
          </ul>
        )}
      </section>

      <div className="ipg-tabs" role="tablist" aria-label="Sections du modpack" onKeyDown={onTabKey}>
        {TABS.map((t) => (
          <button
            key={t.id} type="button" role="tab" id={tabId(t.id)} aria-selected={tab === t.id} aria-controls={panelId(t.id)}
            tabIndex={tab === t.id ? 0 : -1} ref={(el) => { tabRefs.current[t.id] = el; }}
            onClick={() => goTab(t.id)}
          >{t.label}</button>
        ))}
      </div>

      <div className="ipg-panel" role="tabpanel" id={panelId(tab)} aria-labelledby={tabId(tab)}>
        {tab === "apercu" && (
          <div className="ipg-overview">
            {shots.length > 0 && (
              <section className="ipg-card ipg-shots" aria-label="Captures d'écran">
                <div className="ipg-card-h"><span><Icon d={ICONS.film} size={16} />Captures</span><span className="c">{shots.length}</span></div>
                <ThumbStrip services={services} instanceId={inst.id} files={shots} current={-1} onPick={setViewer} />
              </section>
            )}
            {inst.art?.summary || (inst.art && inst.art.source !== "custom") ? (
              <section className="ipg-card ipg-about" aria-label="À propos du modpack">
                <div className="ipg-card-h">
                  <span><Icon d={ICONS.info} size={16} />À propos</span>
                  {inst.art?.author && <span className="c">Par {inst.art.author}{sourceLabel(inst.art.source) ? ` · ${sourceLabel(inst.art.source)}` : ""}</span>}
                </div>
                {inst.art?.summary && <p className="ipg-lead">{inst.art.summary}</p>}
                {inst.art && inst.art.source !== "custom" && <LongDescription services={services} id={inst.id} project={`${inst.art.source}:${inst.art.project_id}`} />}
              </section>
            ) : (
              shots.length === 0 && <p className="ipg-meta">Pas encore d'illustration. Utilise le menu ⋯ pour en choisir une ou lancer la recherche automatique.</p>
            )}
          </div>
        )}
        {tab === "mods" && (
          <ModsPanel
            services={services} instanceId={inst.id}
            onCount={(n) => setInstances((l) => (l ?? []).map((i) => (i.id === inst.id && i.mod_count !== n ? { ...i, mod_count: n } : i)))}
            onBrowse={() => navigate("browse", { kind: "mod", instance: inst.id })}
            autoFix={autoFix} onAutoFixed={() => setAutoFix(false)}
          />
        )}
        {tab === "reglages" && (
          <SettingsDialog
            key={inst.id} instance={inst} onSave={save}
            onOpenFolder={() => { void run(() => services.instancesOpenFolder(id)); }}
            onDelete={() => { void remove(); }}
          />
        )}
        {tab === "logs" && <LogsPanel services={services} instanceId={inst.id} />}
      </div>

      {ctx.menu}
      {gate.modal}
      {menu && (
        <ContextMenu
          label={`Actions de ${inst.name}`} x={menu.x} y={menu.y}
          items={menuItems.map((m) => ({ ...m, onSelect: () => { menu.trigger.focus(); m.onSelect(); } }))}
          onClose={(restore) => { if (restore) menu.trigger.focus(); setMenu(null); }}
        />
      )}
      {viewer !== null && shots.length > 0 && (
        <Viewer services={services} instanceId={inst.id} title={inst.name} files={shots} start={viewer} onClose={() => setViewer(null)} />
      )}
      {confirmDel && (
        <Modal title="Supprimer l'instance ?" role="alertdialog" onClose={() => setConfirmDel(false)}>
          <p className="home-modal-text">L'instance « {inst.name} » et ses fichiers (mondes, configuration) seront supprimés définitivement.</p>
          <div className="home-modal-foot">
            <Button variant="secondary" data-autofocus onClick={() => setConfirmDel(false)}>Annuler</Button>
            <Button variant="danger" onClick={() => { void remove(); }}>Supprimer</Button>
          </div>
        </Modal>
      )}
    </main>
  );
}

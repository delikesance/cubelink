import { useEffect, useRef, useState, type KeyboardEvent } from "react";
import "./main.css";
import { useScreenTitle, useShell } from "../../app/context";
import { peekImportWarning, setImportWarning } from "../../app/importWarning";
import type { FriendWithPresence, HostState, Instance, Invite } from "../../services";
import { errMsg, useLaunch } from "../../app/useLaunch";
import { Button, IconButton } from "../../ui";
import { Avatar } from "./ArtParts";
import { clearArtCache } from "./artImage";
import { ICONS, Icon } from "./Panel";
import { PixelHead, WorldIcon } from "./Pixel";
import { lastSeenText } from "../Friends/lastSeen";
import { canInviteFrom, useInvites } from "../../app/useInvites";
import { AccountSwitcher } from "../../app/AccountSwitcher";
import { useContextMenu, type MenuItem } from "../../app/useContextMenu";
import { useModCheckGate } from "../../app/useModCheck";
import { Modal } from "./Modal";

const LOADERS: Record<string, string> = {
  vanilla: "Vanilla", neoforge: "NeoForge", fabric: "Fabric", quilt: "Quilt", forge: "Forge",
};
const loaderLabel = (i: Instance) => LOADERS[i.loader.loader] ?? i.loader.loader;

export function badgeText(total: number): string {
  return total > 99 ? "99+" : String(total);
}

/** Parties en cours : une par ami qui héberge (invitation non expirée la plus récente). */
export function sessionsFrom(invites: Invite[], nowSec = Math.floor(Date.now() / 1000)): Invite[] {
  const byFriend = new Map<string, Invite>();
  for (const i of invites) {
    if (i.expires_at <= nowSec) continue;
    const cur = byFriend.get(i.public_key);
    if (!cur || i.received_at > cur.received_at) byFriend.set(i.public_key, i);
  }
  return [...byFriend.values()].sort((a, b) => b.received_at - a.received_at);
}

const shortMeta = (i: Instance) => `${i.minecraft} · ${loaderLabel(i)}`;
const friendsHere = (n: number) => (n === 1 ? "1 ami y joue" : `${badgeText(n)} amis y jouent`);
const TABS = ["En partie", "En ligne", "Tous"] as const;

export default function Main() {
  const { services, navigate } = useShell();
  const [instances, setInstances] = useState<Instance[] | null>(null);
  const { launch, error: launchError, play: startGame, kill: stopGame, clearError } = useLaunch(services);
  /** Dernier modpack lancé depuis l'accueil : nomme l'échec quand l'appel de lancement lui-même est rejeté. */
  const [lastPlayed, setLastPlayed] = useState<string | null>(null);
  /** Événement d'échec que l'utilisateur a fermé (comparé par identité). */
  const [dismissed, setDismissed] = useState<unknown>(null);
  const [requests, setRequests] = useState<number | null>(null);
  const [friends, setFriends] = useState<FriendWithPresence[]>([]);
  const [loadError, setLoadError] = useState<string | null>(null);
  const [host, setHost] = useState<HostState | null>(null);
  const [actionError, setActionError] = useState<string | null>(null);
  /** Onglet de la colonne d'amis : 0 En partie, 1 En ligne, 2 Tous. */
  const [tab, setTab] = useState(2);
  const [query, setQuery] = useState("");
  const invites = useInvites(services);
  const ctx = useContextMenu();
  /** Modpack dont la suppression attend confirmation (clic droit > Supprimer). */
  const [toDelete, setToDelete] = useState<Instance | null>(null);
  const [warning, setWarning] = useState<string[] | null>(() => peekImportWarning());

  useScreenTitle(null);

  useEffect(() => { setImportWarning(null); }, []);
  useEffect(() => services.onImportWarning((e) => { if (e.missing.length) setWarning(e.missing); }), [services]);

  useEffect(() => {
    let alive = true;
    services.instancesList().then(
      (list) => { if (alive) setInstances(list); },
      (e: unknown) => { if (alive) { setInstances([]); setLoadError(e instanceof Error ? e.message : String(e)); } },
    );
    services.friendRequests({ offset: 0, limit: 1 }).then(
      (p) => { if (alive) setRequests(p.total); },
      () => {},
    );
    services.friendList().then((l) => { if (alive) setFriends(l); }, () => {});
    return () => { alive = false; };
  }, [services]);

  useEffect(() => services.onFriendPresence((e) => {
    const now = Math.floor(Date.now() / 1000);
    setFriends((l) => l.map((f) => (f.friend.public_key === e.public_key ? { ...f, presence: e.presence, last_seen: now } : f)));
  }), [services]);
  useEffect(() => {
    let alive = true;
    services.hostState().then((h) => { if (alive) setHost(h); }, () => {});
    const off = services.onHostState((h) => setHost(h));
    return () => { alive = false; off(); };
  }, [services]);
  useEffect(() => services.onFriendRequest(() => setRequests((n) => (n ?? 0) + 1)), [services]);

  const homeRef = useRef<HTMLDivElement>(null);
  // Focus cohérent au retour sur l'accueil : première tuile une fois chargé, sauf si le focus est déjà ailleurs.
  const ready = instances !== null;
  useEffect(() => {
    if (!ready) return;
    const a = document.activeElement;
    if (a && a !== document.body && homeRef.current?.contains(a)) return;
    homeRef.current?.querySelector<HTMLElement>(".home-tile")?.focus();
  }, [ready]);

  /** Flèches (grille de deux colonnes), Home et Fin dans la grille des modpacks. */
  const onGridKey = (e: KeyboardEvent<HTMLElement>) => {
    const step: Record<string, number> = { ArrowLeft: -1, ArrowRight: 1, ArrowUp: -2, ArrowDown: 2 };
    if (!(e.key in step) && e.key !== "Home" && e.key !== "End") return;
    const items = Array.from(e.currentTarget.querySelectorAll<HTMLElement>(".home-tile"));
    const i = items.indexOf(document.activeElement as HTMLElement);
    if (i < 0) return;
    e.preventDefault();
    const n = e.key === "Home" ? 0 : e.key === "End" ? items.length - 1 : Math.min(items.length - 1, Math.max(0, i + step[e.key]));
    items[n].focus();
  };

  const gate = useModCheckGate(services);
  const play = (id: string) => {
    setActionError(null);
    const inst = instances?.find((x) => x.id === id);
    gate.guard(inst ?? { id, name: id }, () => { setLastPlayed(id); void startGame(id); }, () => navigate("instance", { id, tab: "mods" }), () => navigate("instance", { id, tab: "mods", fix: "1" }));
  };
  const kill = async (id: string) => {
    setActionError(null);
    const msg = await stopGame(id);
    if (msg) setActionError(msg);
  };

  /** Remplace l'instance mise à jour (illustration) dans la liste : les tuiles suivent. */
  const applyInstance = (u: Instance) => setInstances((l) => (l ?? []).map((i) => (i.id === u.id ? u : i)));

  // Illustration par défaut : une recherche silencieuse par instance et par session pour celles qui n'en ont pas.
  // Sans résultat, l'image par défaut (blocs) reste en place ; la page du modpack permet de choisir une image.
  const autoTried = useRef(new Set<string>());
  useEffect(() => {
    if (!instances) return;
    const todo = instances.filter((i) => !i.art && !autoTried.current.has(i.id));
    if (!todo.length) return;
    todo.forEach((i) => autoTried.current.add(i.id));
    void (async () => {
      for (const i of todo) {
        try {
          const u = await services.instancesArtLookup(i.id, true);
          if (u.art) { clearArtCache(); applyInstance(u); }
        } catch { /* silencieux : pas de correspondance ou hors ligne */ }
      }
    })();
  }, [instances, services]);

  const isBusy = (id: string) => launch?.id === id && (launch.state === "preparing" || launch.state === "running");

  const onlineList = friends.filter((f) => f.presence === "online");
  const sessions = sessionsFrom(invites);
  const playingKeys = new Set(sessions.map((g) => g.public_key));
  const sessionOf = (key: string) => sessions.find((g) => g.public_key === key);
  const onPack = (inst: Instance) => sessions.filter((g) => g.pack === inst.name);
  const join = (key: string) => navigate("join", { friend: key });
  const inviteOk = canInviteFrom(host);
  const showSessions = sessions.length > 4 ? sessions.slice(0, 3) : sessions;
  const openFriendTab = (k: number) => { setQuery(""); setTab(k); };
  const copy = (text: string) => { void navigator.clipboard?.writeText(text).catch(() => {}); };
  const invite = (key: string) => { setActionError(null); services.friendInvite(key).catch((e: unknown) => setActionError(errMsg(e))); };

  // Clic droit : mêmes actions que les boutons et la page du modpack, au plus près.
  const tileMenu = (i: Instance): MenuItem[] => {
    const st = launch?.id === i.id ? launch.state : null;
    const busy = st === "running" || st === "preparing";
    const blocked = !!launch && launch.id !== i.id && isBusy(launch.id);
    const open = (tab?: string) => navigate("instance", tab ? { id: i.id, tab } : { id: i.id });
    return [
      busy
        ? { key: "stop", label: st === "preparing" ? "Annuler le lancement" : "Arrêter", icon: <Icon d={ICONS.stop} />, onSelect: () => { void kill(i.id); } }
        : { key: "play", label: "Jouer", icon: <Icon d={ICONS.play} />, disabled: blocked || gate.checking, onSelect: () => play(i.id) },
      { key: "open", label: "Ouvrir", icon: <Icon d={ICONS.image} />, separator: true, onSelect: () => open() },
      { key: "mods", label: "Mods", icon: <Icon d={ICONS.mods} />, onSelect: () => open("mods") },
      { key: "settings", label: "Réglages", icon: <Icon d={ICONS.gear} />, onSelect: () => open("reglages") },
      { key: "logs", label: "Logs", icon: <Icon d={ICONS.logs} />, onSelect: () => open("logs") },
      { key: "folder", label: "Ouvrir le dossier", icon: <Icon d={ICONS.folder} />, separator: true, onSelect: () => { services.instancesOpenFolder(i.id).catch((e: unknown) => setActionError(errMsg(e))); } },
      { key: "delete", label: "Supprimer", icon: <Icon d={ICONS.trash} />, danger: true, separator: true, disabled: busy, onSelect: () => setToDelete(i) },
    ];
  };
  const friendMenu = (f: FriendWithPresence): MenuItem[] => {
    const key = f.friend.public_key;
    const g = sessionOf(key);
    const on = f.presence === "online" || !!g;
    const items: MenuItem[] = [];
    if (on) items.push({ key: "join", label: g ? `Rejoindre ${g.world_name}` : "Rejoindre", icon: <Icon d={ICONS.play} />, onSelect: () => join(key) });
    if (on && inviteOk && !g) items.push({ key: "invite", label: "Inviter dans ma partie", icon: <Icon d={ICONS.userPlus} />, onSelect: () => invite(key) });
    items.push({ key: "copy", label: "Copier le pseudo", icon: <Icon d={ICONS.copy} />, separator: items.length > 0, onSelect: () => copy(f.friend.name) });
    items.push({ key: "manage", label: "Gérer mes amis", icon: <Icon d={ICONS.friends} />, onSelect: () => navigate("settings", { tab: "amis" }) });
    return items;
  };
  const sessionMenu = (g: Invite): MenuItem[] => [
    { key: "join", label: `Rejoindre ${g.name}`, icon: <Icon d={ICONS.play} />, onSelect: () => join(g.public_key) },
    { key: "copy", label: "Copier le pseudo", icon: <Icon d={ICONS.copy} />, separator: true, onSelect: () => copy(g.name) },
  ];
  const removeInstance = async (i: Instance) => {
    setToDelete(null);
    setActionError(null);
    try {
      await services.instancesDelete(i.id);
      setInstances((l) => (l ?? []).filter((x) => x.id !== i.id));
    } catch (e) { setActionError(errMsg(e)); }
  };

  // Échec de lancement rapide : bandeau en haut, avec accès aux logs du modpack concerné.
  const failedId = launch?.state === "failed" && launch !== dismissed ? launch.id : launchError !== null ? lastPlayed : null;
  const failMsg = launchError ?? (launch?.state === "failed" ? launch.message ?? "" : "");
  const failInst = failedId ? instances?.find((i) => i.id === failedId) : undefined;

  // Colonne d'amis : en partie, en ligne, tous ; filtre par pseudo.
  const tabLists = [
    friends.filter((f) => playingKeys.has(f.friend.public_key)),
    friends.filter((f) => f.presence === "online" || playingKeys.has(f.friend.public_key)),
    friends,
  ];
  const q = query.trim().toLowerCase();
  const rows = tabLists[tab].filter((f) => !q || f.friend.name.toLowerCase().includes(q));
  const nowSec = Math.floor(Date.now() / 1000);

  return (
    <div className="home" ref={homeRef}>
      <main className="home-main">
        {failedId && (
          <div className="home-fail enter" role="alert">
            <span className="home-fail-ic" aria-hidden="true"><Icon d={ICONS.alert} size={20} /></span>
            <div className="home-fail-txt">
              <div className="t">Échec du lancement{failInst ? ` de ${failInst.name}` : ""}</div>
              {failMsg && <div className="m">{failMsg}</div>}
            </div>
            <Button variant="secondary" onClick={() => navigate("instance", { id: failedId, tab: "logs" })}>Voir les logs</Button>
            <Button variant="secondary" onClick={() => { clearError(); setDismissed(launch); }}>Ignorer</Button>
          </div>
        )}
        {actionError && <p className="home-err" role="alert">{actionError}</p>}
        {warning && instances && instances.length > 0 && (
          <div className="home-warn enter" role="status">
            <span>
              Import terminé, mais {warning.length} mod{warning.length > 1 ? "s" : ""} introuvable{warning.length > 1 ? "s" : ""} :{" "}
              {warning.slice(0, 5).join(", ")}{warning.length > 5 ? `… (+${warning.length - 5})` : ""}.
            </span>
            <Button variant="secondary" onClick={() => setWarning(null)}>Ignorer</Button>
          </div>
        )}

        <section className="home-friends" aria-label="Parties">
          <div className="home-head">
            <div className="home-head-txt">
              <h1>Jouer avec tes amis</h1>
              <p className="home-stats">
                <span className="dot is-play" aria-hidden="true">●</span> {badgeText(sessions.length)} en partie ·{" "}
                <span className="dot is-on" aria-hidden="true">●</span> {badgeText(onlineList.length)} en ligne
                {requests ? <> · <button type="button" className="home-link" onClick={() => navigate("settings", { tab: "amis" })}>{badgeText(requests)} demande{requests > 1 ? "s" : ""}</button></> : null}
              </p>
            </div>
          </div>
          {sessions.length === 0 ? (
            <p className="home-nosession">Aucune partie en cours. Les mondes de tes amis apparaissent ici dès qu'ils jouent.</p>
          ) : (
            <div className="home-sessions">
              <span className="home-sessions-t">Parties en cours · {badgeText(sessions.length)}</span>
              <ul className="home-sessions-grid" aria-label="Parties en cours">
                {showSessions.map((g) => (
                  <li key={g.id} className="home-session-card" onContextMenu={ctx.bind(`Partie de ${g.name}`, () => sessionMenu(g))} title={g.pack ? `${g.pack} · Minecraft ${g.minecraft}` : `Minecraft ${g.minecraft}`}>
                    <WorldIcon className="home-wicon" src={g.world_icon} size={40} />
                    <span className="home-session-txt">
                      <span className="w">{g.world_name}</span>
                      <span className="who">{g.name}</span>
                    </span>
                    <button type="button" className="home-join" aria-label={`Rejoindre ${g.name}`} onClick={() => join(g.public_key)}>
                      <Icon d={ICONS.play} fill />
                    </button>
                  </li>
                ))}
                {sessions.length > 4 && (
                  <li className="home-session-more">
                    <button type="button" onClick={() => openFriendTab(0)}>+{badgeText(sessions.length - 3)} autres parties</button>
                  </li>
                )}
              </ul>
            </div>
          )}
        </section>

        <section className="home-packs" aria-label="Mes modpacks">
          <div className="home-head is-small">
            <h2>Mes modpacks {instances && <span className="home-count">{instances.length}</span>}</h2>
            <div className="home-head-act">
              <Button variant="secondary" icon={<Icon d={ICONS.plus} />} onClick={() => navigate("create")}>Ajouter un modpack</Button>
            </div>
          </div>
          {instances === null ? (
            <p role="status" className="home-loading">Chargement des instances…</p>
          ) : instances.length === 0 ? (
            <div className="home-empty">
              <h2>Aucune instance</h2>
              <p className="home-meta">Crée ta première instance pour commencer à jouer, seul ou avec tes amis.</p>
              {loadError && <p className="home-err" role="alert">{loadError}</p>}
              <Button icon={<Icon d={ICONS.plus} />} onClick={() => navigate("create")}>Créer une instance</Button>
            </div>
          ) : (
            <div className="home-grid" role="group" aria-label="Instances" onKeyDown={onGridKey}>
              {instances.map((i) => {
                const n = onPack(i).length;
                const st = launch?.id === i.id ? launch.state : null;
                return (
                  <div key={i.id} className="home-tile-wrap" onContextMenu={ctx.bind(`Actions de ${i.name}`, () => tileMenu(i))}>
                    <button type="button" className="home-tile" aria-label={i.name} onClick={() => navigate("instance", { id: i.id })}>
                      <Avatar services={services} inst={i} className="home-av home-tile-av" />
                      <span className="home-tile-txt">
                        <span className="n">{i.name}</span>
                        <span className="m">{shortMeta(i)}{n > 0 && <span className="there"> · {friendsHere(n)}</span>}</span>
                      </span>
                    </button>
                    {st === "running" || st === "preparing" ? (
                      <button
                        type="button" className="home-quick is-stop" aria-label={`Arrêter ${i.name}`}
                        title={st === "preparing" ? "Annuler le lancement" : "Arrêter"} onClick={() => { void kill(i.id); }}
                      >
                        <Icon d={ICONS.stop} size={18} fill />
                      </button>
                    ) : (
                      <button
                        type="button" className="home-quick" aria-label={`Jouer à ${i.name}`} title="Jouer"
                        disabled={gate.checking || (!!launch && launch.id !== i.id && isBusy(launch.id))} onClick={() => play(i.id)}
                      >
                        <Icon d={ICONS.play} size={18} fill />
                      </button>
                    )}
                  </div>
                );
              })}
            </div>
          )}
        </section>
      </main>

      <aside className="home-side" aria-label="Amis">
        <div className="side-friends">
          <div className="side-friends-head">
            <h2>Amis <span className="home-count">{badgeText(friends.length)}</span></h2>
          </div>
          <label className="side-search">
            <Icon d={ICONS.search} size={16} />
            <span className="home-sr">Chercher un ami</span>
            <input type="search" placeholder="Pseudo…" value={query} onChange={(e) => setQuery(e.target.value)} />
          </label>
          <div className="side-tabs" role="tablist" aria-label="Filtrer les amis">
            {TABS.map((t, k) => (
              <button key={t} type="button" role="tab" aria-selected={k === tab} onClick={() => setTab(k)}>
                {t} · {badgeText(tabLists[k].length)}
              </button>
            ))}
          </div>
          <ul className="side-list" aria-label="Amis">
            {rows.map((f) => {
              const g = sessionOf(f.friend.public_key);
              const on = f.presence === "online" || !!g;
              const seen = on ? null : lastSeenText(f.last_seen, nowSec);
              return (
                <li key={f.friend.public_key} className={`side-row${on ? "" : " is-off"}`} onContextMenu={ctx.bind(`Actions pour ${f.friend.name}`, () => friendMenu(f))}>
                  <span className="side-head">
                    <PixelHead seed={f.friend.public_key} size={32} />
                    <span className={`side-dot${g ? " is-play" : on ? " is-on" : ""}`} aria-hidden="true" />
                  </span>
                  <span className="side-txt">
                    <span className="n">{f.friend.name}</span>
                    <span className={`s${g ? " is-play" : on ? " is-on" : ""}`}>
                      {g ? `Joue à ${g.pack ?? g.world_name}` : on ? "En ligne" : "Hors ligne"}
                    </span>
                    {seen && <span className="s">{seen}</span>}
                  </span>
                  {on && inviteOk && !g && (
                    <Button
                      variant="secondary" aria-label={`Inviter ${f.friend.name}`}
                      onClick={() => invite(f.friend.public_key)}
                    >Inviter</Button>
                  )}
                  {on && (
                    <Button variant={g ? "primary" : "secondary"} aria-label={`Rejoindre ${f.friend.name}`} onClick={() => join(f.friend.public_key)}>Rejoindre</Button>
                  )}
                </li>
              );
            })}
            {rows.length === 0 && <li className="side-none">{friends.length ? "Aucun ami ne correspond." : "Aucun ami pour l'instant."}</li>}
          </ul>
          <Button className="side-add" icon={<Icon d={ICONS.userPlus} />} onClick={() => navigate("settings", { tab: "amis" })}>Ajouter un ami</Button>
        </div>

        <div className="side-profile" onContextMenu={ctx.bind("Compte et paramètres", [
          { key: "account", label: "Gérer les comptes", icon: <Icon d={ICONS.friends} />, onSelect: () => navigate("settings", { tab: "compte" }) },
          { key: "settings", label: "Paramètres de Cubelink", icon: <Icon d={ICONS.gear} />, onSelect: () => navigate("settings") },
        ])}>
          <AccountSwitcher />
          <IconButton variant="secondary" aria-label="Paramètres de Cubelink" title="Paramètres de Cubelink" icon={<Icon d={ICONS.gear} size={18} />} onClick={() => navigate("settings")} />
        </div>
      </aside>
      {ctx.menu}
      {gate.modal}
      {toDelete && (
        <Modal title="Supprimer l'instance ?" role="alertdialog" onClose={() => setToDelete(null)}>
          <p className="home-modal-text">L'instance « {toDelete.name} » et ses fichiers (mondes, configuration) seront supprimés définitivement.</p>
          <div className="home-modal-foot">
            <Button variant="secondary" data-autofocus onClick={() => setToDelete(null)}>Annuler</Button>
            <Button variant="danger" onClick={() => { void removeInstance(toDelete); }}>Supprimer</Button>
          </div>
        </Modal>
      )}
    </div>
  );
}

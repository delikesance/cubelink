import { useCallback, useEffect, useId, useMemo, useRef, useState, type KeyboardEvent } from "react";
import "./browse.css";
import { useBack, useScreenTitle, useShell } from "../../app/context";
import type { BrowseHit, BrowseKind, BrowseQuery, BrowseSource, Instance } from "../../services/types";
import { Detail, SOURCE_LABEL, CF_UNAVAILABLE, isCurseForgeDown, messageOf } from "./Detail";
import { Button } from "../../ui";
import { useContextMenu, type MenuItem } from "../../app/useContextMenu";
import { openExternal } from "../../app/externalLinks";
import { ICONS, Icon } from "../Main/Panel";

export const KINDS: { value: BrowseKind; label: string }[] = [
  { value: "modpack", label: "Modpacks" },
  { value: "mod", label: "Mods" },
  { value: "resourcepack", label: "Packs de ressources" },
  { value: "shader", label: "Shaders" },
];
const LOADERS = [
  { value: "", label: "Tous" },
  { value: "neoforge", label: "NeoForge" },
  { value: "fabric", label: "Fabric" },
  { value: "quilt", label: "Quilt" },
  { value: "forge", label: "Forge" },
];
type Sort = NonNullable<BrowseQuery["sort"]>;
const SORTS: { value: Sort; label: string }[] = [
  { value: "relevance", label: "Pertinence" },
  { value: "downloads", label: "Téléchargements" },
  { value: "updated", label: "Récents" },
];
export const PAGE_SIZE = 24;
export const DEBOUNCE_MS = 300;

export function formatDownloads(n: number): string {
  if (n >= 1_000_000) return `${(n / 1_000_000).toFixed(n >= 10_000_000 ? 0 : 1).replace(".", ",")} M`;
  if (n >= 1_000) return `${(n / 1_000).toFixed(n >= 10_000 ? 0 : 1).replace(".", ",")} k`;
  return String(n);
}

function asKind(v: string | undefined): BrowseKind {
  return KINDS.some((k) => k.value === v) ? (v as BrowseKind) : "modpack";
}

type Status = "loading" | "ready" | "error";

export default function Browse() {
  useScreenTitle("Parcourir");
  const { services, params, navigate: go, goBack } = useShell();
  const uid = useId();
  const searchRef = useRef<HTMLInputElement>(null);
  const tabRefs = useRef<(HTMLButtonElement | null)[]>([]);

  const [kind, setKind] = useState<BrowseKind>(() => asKind(params.kind));
  const [source, setSource] = useState<BrowseSource>("modrinth");
  const [cfDown, setCfDown] = useState(false);
  const [text, setText] = useState("");
  const [query, setQuery] = useState("");
  const [minecraft, setMinecraft] = useState("");
  const [loader, setLoader] = useState("");
  const [sort, setSort] = useState<Sort>("relevance");
  const [instanceId, setInstanceId] = useState(params.instance ?? "");
  const [instances, setInstances] = useState<Instance[]>([]);
  const [mcVersions, setMcVersions] = useState<string[]>([]);
  const [hits, setHits] = useState<BrowseHit[]>([]);
  const [total, setTotal] = useState(0);
  const [status, setStatus] = useState<Status>("loading");
  const [error, setError] = useState<string | null>(null);
  const [more, setMore] = useState(false);
  const [moreError, setMoreError] = useState<string | null>(null);
  const [retry, setRetry] = useState(0);
  // La fiche est une page : son état vit dans l'URL (`detail=<source>:<id>`).
  const detailKey = params.detail ?? "";
  const [opened, setOpened] = useState<BrowseHit | null>(null);
  const [instancesReady, setInstancesReady] = useState(false);

  const instance = useMemo(() => instances.find((i) => i.id === instanceId) ?? null, [instances, instanceId]);
  const needsInstance = kind !== "modpack";

  // Données de référence.
  useEffect(() => {
    let off = false;
    services.minecraftVersions().then((l) => { if (!off) setMcVersions(l); }, () => {});
    services.instancesList().then((l) => { if (!off) { setInstances(l); setInstancesReady(true); } }, () => { if (!off) setInstancesReady(true); });
    return () => { off = true; };
  }, [services]);

  const applyInstance = useCallback((i: Instance | null) => {
    if (!i) return;
    setMinecraft(i.minecraft);
    setLoader(i.loader.loader === "vanilla" ? "" : i.loader.loader);
  }, []);

  // Instance pré-sélectionnée par la route : préremplit les filtres une fois la liste connue.
  const preApplied = useRef(false);
  useEffect(() => {
    if (preApplied.current || !instance) return;
    preApplied.current = true;
    if (kind !== "modpack") applyInstance(instance);
  }, [instance, kind, applyInstance]);

  // Debounce de la saisie.
  useEffect(() => {
    if (text === query) return;
    const t = setTimeout(() => setQuery(text), DEBOUNCE_MS);
    return () => clearTimeout(t);
  }, [text, query]);

  const effLoader = kind === "mod" || kind === "modpack" ? loader : "";
  const buildQuery = useCallback((offset: number): BrowseQuery => ({
    source, kind, query: query.trim(),
    minecraft: minecraft || undefined,
    loader: effLoader || undefined,
    sort, offset, limit: PAGE_SIZE,
  }), [source, kind, query, minecraft, effLoader, sort]);

  const seq = useRef(0);
  useEffect(() => {
    const id = ++seq.current;
    setStatus("loading");
    setError(null);
    setMoreError(null);
    setMore(false);
    services.browseSearch(buildQuery(0)).then(
      (p) => { if (id === seq.current) { setHits(p.hits); setTotal(p.total); setStatus("ready"); } },
      (e) => { if (id === seq.current) { if (isCurseForgeDown(e)) setCfDown(true); setHits([]); setTotal(0); setError(messageOf(e)); setStatus("error"); } },
    );
    return () => { seq.current++; };
  }, [services, buildQuery, retry]);

  const loadMore = async () => {
    if (more || status !== "ready") return;
    const id = seq.current;
    setMore(true);
    setMoreError(null);
    try {
      const p = await services.browseSearch(buildQuery(hits.length));
      if (id !== seq.current) return;
      setHits((h) => [...h, ...p.hits]);
      setTotal(p.total);
      setMore(false);
    } catch (e) {
      if (id === seq.current) { setMoreError(messageOf(e)); setMore(false); }
    }
  };

  // Focus initial : champ de recherche.
  useEffect(() => { searchRef.current?.focus(); }, []);

  // Échap : retour (de la fiche vers la liste, ou de la liste vers l'écran précédent).
  const back = useBack("main");
  const backRef = useRef(back);
  backRef.current = back;
  useEffect(() => {
    const onEsc = (e: globalThis.KeyboardEvent) => {
      if (e.key === "Escape" && !e.defaultPrevented) { e.preventDefault(); backRef.current(); }
    };
    document.addEventListener("keydown", onEsc);
    return () => document.removeEventListener("keydown", onEsc);
  }, []);

  const selectKind = (k: BrowseKind) => {
    setKind(k);
    if (k !== "modpack" && instance) applyInstance(instance);
  };
  const onTabKey = (e: KeyboardEvent<HTMLButtonElement>, i: number) => {
    let n = -1;
    if (e.key === "ArrowRight") n = (i + 1) % KINDS.length;
    else if (e.key === "ArrowLeft") n = (i - 1 + KINDS.length) % KINDS.length;
    else if (e.key === "Home") n = 0;
    else if (e.key === "End") n = KINDS.length - 1;
    if (n < 0) return;
    e.preventDefault();
    selectKind(KINDS[n].value);
    tabRefs.current[n]?.focus();
  };

  const onInstance = (id: string) => {
    setInstanceId(id);
    applyInstance(instances.find((i) => i.id === id) ?? null);
  };

  const openHit = (h: BrowseHit) => {
    setOpened(h);
    go("browse", { ...params, detail: `${h.source}:${h.id}` });
  };
  // Retour à la liste : on dépile l'historique ; sans historique (ouverture directe), on retire `detail` de l'URL.
  const closeDetail = () => goBack("browse");

  // Résolution de la fiche demandée par l'URL : résultat connu, sinon repli sur l'illustration d'une instance installée.
  const detailHit = useMemo<BrowseHit | null>(() => {
    if (!detailKey) return null;
    const cut = detailKey.indexOf(":");
    const src = detailKey.slice(0, cut);
    const id = detailKey.slice(cut + 1);
    if (cut < 0 || (src !== "modrinth" && src !== "curseforge") || !id) return null;
    const known = [opened, ...hits].find((h) => h && h.source === src && h.id === id);
    if (known) return known;
    const art = instances.find((i) => i.art && i.art.source === src && i.art.project_id === id)?.art;
    if (!art || art.source === "custom") return null;
    return {
      source: src, id, kind: "modpack", name: art.title, summary: art.summary, author: art.author,
      icon_url: art.icon && /^https?:/.test(art.icon) ? art.icon : null, downloads: 0,
      page_url: art.page_url ?? "", downloadable: true,
    };
  }, [detailKey, opened, hits, instances]);

  const installedFor = (h: BrowseHit): Instance | null =>
    h.kind !== "modpack" ? null
      : instances.find((i) => i.art && i.art.source === h.source && i.art.project_id === h.id) ?? null;
  const ctx = useContextMenu();
  const hitMenu = (h: BrowseHit): MenuItem[] => {
    const inst = installedFor(h);
    const items: MenuItem[] = [{ key: "detail", label: "Voir la fiche", icon: <Icon d={ICONS.search} />, onSelect: () => openHit(h) }];
    if (inst) items.push({ key: "open", label: "Ouvrir le modpack", icon: <Icon d={ICONS.play} />, onSelect: () => go("instance", { id: inst.id }) });
    if (h.page_url) {
      items.push({ key: "site", label: `Ouvrir sur ${SOURCE_LABEL[h.source]}`, icon: <Icon d={ICONS.folder} />, separator: true, onSelect: () => openExternal(h.page_url) });
      items.push({ key: "copy", label: "Copier le lien", icon: <Icon d={ICONS.copy} />, onSelect: () => { void navigator.clipboard?.writeText(h.page_url).catch(() => {}); } });
    }
    return items;
  };

  const panelId = `${uid}-panel`;
  const canMore = status === "ready" && hits.length < total && hits.length > 0;

  if (detailKey) {
    return (
      <div className="browse screen">
        {detailHit ? (
          <Detail
            key={detailKey}
            hit={detailHit}
            services={services}
            minecraft={minecraft}
            loader={effLoader}
            instance={instance}
            installed={installedFor(detailHit)}
            onBack={closeDetail}
          />
        ) : status === "loading" || !instancesReady ? (
          <p className="br-muted" role="status">Chargement de la fiche…</p>
        ) : (
          <div className="br-state" role="alert">
            <p className="br-error">Cette fiche est introuvable.</p>
            <Button variant="secondary" onClick={closeDetail}>Retour aux résultats</Button>
          </div>
        )}
      </div>
    );
  }

  return (
    <div className="browse screen">
      <header className="br-head">
        <h1>Parcourir</h1>
        <div className="br-source-sel" role="group" aria-label="Source">
          {(["modrinth", "curseforge"] as BrowseSource[]).map((s) => (
            <button key={s} type="button" className="br-seg" aria-pressed={source === s} onClick={() => setSource(s)}>
              {SOURCE_LABEL[s]}{s === "curseforge" && cfDown ? <span className="br-seg-note"> · indisponible</span> : null}
            </button>
          ))}
        </div>
      </header>

      <div role="tablist" aria-label="Type de contenu" className="br-tabs">
        {KINDS.map((k, i) => (
          <button
            key={k.value}
            ref={(el) => { tabRefs.current[i] = el; }}
            id={`${uid}-tab-${k.value}`}
            type="button"
            role="tab"
            aria-selected={kind === k.value}
            aria-controls={panelId}
            tabIndex={kind === k.value ? 0 : -1}
            className="br-tab"
            onClick={() => selectKind(k.value)}
            onKeyDown={(e) => onTabKey(e, i)}
          >{k.label}</button>
        ))}
      </div>

      <div id={panelId} role="tabpanel" aria-labelledby={`${uid}-tab-${kind}`} className="br-panel">
        <form className="br-filters" role="search" onSubmit={(e) => { e.preventDefault(); setQuery(text); setRetry((n) => n + 1); }}>
          <div className="br-field br-grow">
            <label htmlFor={`${uid}-q`}>Recherche</label>
            <input
              id={`${uid}-q`} ref={searchRef} className="br-input" type="search" value={text} autoComplete="off"
              placeholder={`Rechercher sur ${SOURCE_LABEL[source]}…`}
              onChange={(e) => setText(e.target.value)}
            />
          </div>
          <div className="br-field">
            <label htmlFor={`${uid}-mc`}>Version de Minecraft</label>
            <select id={`${uid}-mc`} className="br-input" value={minecraft} onChange={(e) => setMinecraft(e.target.value)}>
              <option value="">Toutes</option>
              {minecraft && !mcVersions.includes(minecraft) && <option value={minecraft}>{minecraft}</option>}
              {mcVersions.map((v) => <option key={v} value={v}>{v}</option>)}
            </select>
          </div>
          {(kind === "mod" || kind === "modpack") && (
            <div className="br-field">
              <label htmlFor={`${uid}-ld`}>Loader</label>
              <select id={`${uid}-ld`} className="br-input" value={loader} onChange={(e) => setLoader(e.target.value)}>
                {LOADERS.map((l) => <option key={l.value} value={l.value}>{l.label}</option>)}
              </select>
            </div>
          )}
          <div className="br-field">
            <label htmlFor={`${uid}-sort`}>Tri</label>
            <select id={`${uid}-sort`} className="br-input" value={sort} onChange={(e) => setSort(e.target.value as Sort)}>
              {SORTS.map((s) => <option key={s.value} value={s.value}>{s.label}</option>)}
            </select>
          </div>
          {needsInstance && (
            <div className="br-field">
              <label htmlFor={`${uid}-inst`}>Instance</label>
              <select id={`${uid}-inst`} className="br-input" value={instanceId} onChange={(e) => onInstance(e.target.value)}>
                <option value="">Aucune</option>
                {instances.map((i) => <option key={i.id} value={i.id}>{i.name}</option>)}
              </select>
            </div>
          )}
        </form>

        <div className="br-live" aria-live="polite">
          {status === "ready" && <p className="br-muted br-count">{total} résultat{total > 1 ? "s" : ""}</p>}
        </div>

        {status === "loading" && (
          <ul className="br-grid" aria-busy="true" aria-label="Chargement des résultats">
            {Array.from({ length: 8 }, (_, i) => <li key={i} className="br-card br-skel" aria-hidden="true" />)}
          </ul>
        )}

        {status === "error" && (
          <div className="br-state" role="alert">
            <p className="br-error">{error}</p>
            {source === "curseforge" && (cfDown || error === CF_UNAVAILABLE)
              ? <Button onClick={() => setSource("modrinth")}>Utiliser Modrinth</Button>
              : <Button variant="secondary" onClick={() => setRetry((n) => n + 1)}>Réessayer</Button>}
          </div>
        )}

        {status === "ready" && hits.length === 0 && (
          <div className="br-state">
            <p>Aucun résultat{query.trim() ? ` pour « ${query.trim()} »` : ""}. Essaie d'autres mots-clés ou de retirer un filtre.</p>
          </div>
        )}

        {status === "ready" && hits.length > 0 && (
          <ul className="br-grid">
            {hits.map((h) => (
              <li key={`${h.source}:${h.id}`} className="br-card" onContextMenu={ctx.bind(h.name, () => hitMenu(h))}>
                <button type="button" className="br-card-btn" onClick={() => openHit(h)} aria-label={`${h.name}, par ${h.author}. Ouvrir la fiche`}>
                  <span className="br-card-top">
                    {h.icon_url ? <img className="br-icon" src={h.icon_url} alt="" loading="lazy" /> : <span className="br-icon br-icon-ph" aria-hidden="true" />}
                    <span className="br-card-titles">
                      <span className="br-card-name">{h.name}</span>
                      <span className="br-muted">{h.author}</span>
                    </span>
                  </span>
                  <span className="br-card-sum">{h.summary}</span>
                  <span className="br-card-foot">
                    <span className="br-muted">{formatDownloads(h.downloads)} téléchargements</span>
                    <span className="br-badge">{SOURCE_LABEL[h.source]}</span>
                  </span>
                </button>
              </li>
            ))}
          </ul>
        )}

        {ctx.menu}
        {canMore && (
          <div className="br-more">
            {moreError && <p className="br-error" role="alert">{moreError}</p>}
            <button type="button" className="cl-btn cl-btn-secondary" disabled={more} onClick={() => { void loadMore(); }}>
              {more ? "Chargement…" : "Charger plus"}
            </button>
          </div>
        )}
      </div>

    </div>
  );
}

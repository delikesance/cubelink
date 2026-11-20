import { afterEach, describe, expect, it, vi } from "vitest";
import { act, cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { ShellContext, type ShellContextValue } from "../../app/context";
import { createFakeServices } from "../../services/fake";
import { HearthLeaveError } from "../../services/types";
import { HearthSection } from "./HearthSection";
import { HearthMoments } from "./Moments";
import Hearth from "./index";
import { parseHeadUnavailable } from "./format";

afterEach(cleanup);

function shell(services: ReturnType<typeof createFakeServices>, params: Record<string, string>) {
  const navigate = vi.fn();
  const goBack = vi.fn();
  const value = { services, route: "hearth", params, navigate, goBack, canGoBack: false, setTitle: () => {} } as ShellContextValue;
  return { navigate, goBack, value };
}

function home(g?: Parameters<ReturnType<typeof createFakeServices>["seedHearth"]>[0]) {
  const services = createFakeServices({ tickMs: 0 });
  if (g) services.seedHearth(g);
  const navigate = vi.fn();
  render(<HearthSection services={services} navigate={navigate} />);
  return { services, navigate };
}

function page(params: Record<string, string>, seed?: Parameters<ReturnType<typeof createFakeServices>["seedHearth"]>[0], detail?: Parameters<ReturnType<typeof createFakeServices>["seedHearth"]>[1]) {
  const services = createFakeServices({ tickMs: 0 });
  if (seed) services.seedHearth(seed, detail);
  const s = shell(services, params);
  render(<ShellContext.Provider value={s.value}><Hearth /></ShellContext.Provider>);
  return { services, ...s };
}

describe("Accueil : serveurs de groupe", () => {
  it("sans groupe : texte d'accueil et bouton de création", async () => {
    const { navigate } = home();
    await screen.findByText(/Un monde partagé avec tes amis/);
    fireEvent.click(screen.getByRole("button", { name: "Créer un serveur" }));
    expect(navigate).toHaveBeenCalledWith("hearth", { new: "1" });
  });

  it("interrupteur coupé : la section est masquée, et revient une fois activée", async () => {
    const services = createFakeServices({ tickMs: 0 });
    await services.hearthSetEnabled(false);
    render(<HearthSection services={services} navigate={vi.fn()} />);
    await waitFor(() => expect(services.hearthCalls.some((c) => c.kind === "SetEnabled")).toBe(true));
    await act(async () => { await Promise.resolve(); });
    expect(screen.queryByRole("region", { name: "Serveurs de groupe" })).toBeNull();
    expect(screen.queryByRole("button", { name: "Créer un serveur" })).toBeNull();
    await act(async () => { await services.hearthSetEnabled(true); });
    await screen.findByRole("button", { name: "Créer un serveur" });
  });

  it("idle : « Personne ne joue » et un seul bouton Jouer ; la page du serveur s'ouvre par le lien", async () => {
    const { navigate } = home({ id: "g1", name: "Les copains" });
    const card = await screen.findByRole("listitem", { name: "Serveur Les copains" });
    expect(within(card).getByText("Personne ne joue")).toBeTruthy();
    expect(within(card).getAllByRole("button").map((b) => b.textContent)).toEqual(["Jouer", "Page du serveur"]);
    fireEvent.click(within(card).getByRole("button", { name: "Page du serveur Les copains" }));
    expect(navigate).toHaveBeenCalledWith("hearth", { id: "g1" });
  });

  it("hosted : « X héberge · n joueurs » et Rejoindre appelle hearthPlay", async () => {
    const { services } = home({ id: "g1", name: "Les copains", state: "hosted", host: { public_key: "k", name: "Alex", online: true }, players: 2 });
    await screen.findByText("Alex héberge · 2 joueurs");
    fireEvent.click(screen.getByRole("button", { name: "Rejoindre sur Les copains" }));
    await waitFor(() => expect(services.hearthCalls.map((c) => c.kind)).toEqual(["Play"]));
  });

  it("états : mise à jour sans bouton, personne en ligne (on peut lancer), deux versions", async () => {
    const services = createFakeServices({ tickMs: 0 });
    services.seedHearth({ id: "a", name: "Sync", state: "syncing", progress: 42 });
    services.seedHearth({ id: "b", name: "Hors ligne", state: "blocked" });
    services.seedHearth({ id: "c", name: "Fourche", state: "fork" });
    render(<HearthSection services={services} navigate={vi.fn()} />);
    await screen.findByText("Mise à jour du monde 42 %");
    expect(screen.getByRole("progressbar", { name: /Sync/ }).getAttribute("aria-valuenow")).toBe("42");
    const blocked = screen.getByRole("listitem", { name: "Serveur Hors ligne" });
    expect(within(blocked).getByText("Aucun membre en ligne")).toBeTruthy();
    expect(within(blocked).getByText(/Tu peux lancer le serveur : ils le rejoindront à leur arrivée/)).toBeTruthy();
    expect(within(blocked).getByRole("button", { name: "Jouer sur Hors ligne" })).toBeTruthy();
    const fork = screen.getByRole("listitem", { name: "Serveur Fourche" });
    expect(within(fork).getByText("Deux versions existent")).toBeTruthy();
    expect(within(fork).getByRole("button", { name: "Choisir une version" })).toBeTruthy();
  });

  it("lancement : étape en cours sur le nombre d'étapes", async () => {
    home({ id: "g1", name: "Les copains", state: "launching", step: "Démarrage du serveur", step_index: 1, step_count: 3 });
    await screen.findByText("Étape 2 sur 3 : Démarrage du serveur");
  });

  it("partie mal terminée : reprise au point de reprise ou attendre", async () => {
    const { services } = home({
      id: "g1", name: "Les copains", state: "unclean",
      unclean: { holder: { public_key: "k", name: "Alex", online: false }, checkpoint_at: 1_760_000_000 },
    });
    services.setHearthEula(true);
    await screen.findByText("Partie mal terminée");
    fireEvent.click(screen.getByRole("button", { name: "Attendre" }));
    await waitFor(() => expect(services.hearthCalls).toEqual([{ kind: "UncleanChoice", args: ["g1", "wait"] }]));
    fireEvent.click(screen.getByRole("button", { name: /Jouer depuis le point de reprise/ }));
    await waitFor(() => expect(services.hearthCalls[1]).toEqual({ kind: "UncleanChoice", args: ["g1", "checkpoint"] }));
    await screen.findByText("Tu héberges · 1 joueur");
  });

  it("sans point de reprise : reprise à la dernière version", async () => {
    home({ id: "g1", name: "Les copains", state: "unclean", unclean: { holder: { public_key: "k", name: "Alex", online: false }, checkpoint_at: null } });
    await screen.findByRole("button", { name: /Jouer depuis la dernière version/ });
  });

  it("dernière version sans contenu : Attendre ferme la boîte, Jouer depuis la n° précédente repart de celle-ci", async () => {
    const { services } = home({ id: "g1", name: "Les copains" });
    services.setHearthEula(true);
    services.setHearthHeadUnavailable({ author: "Alice", head: 12, previous: 11 });
    fireEvent.click(await screen.findByRole("button", { name: "Jouer sur Les copains" }));
    const dlg = await screen.findByRole("dialog", { name: "La partie de Alice n'a pas fini d'être envoyée" });
    expect(within(dlg).getByText(/Personne de connecté n'a encore le monde de la n° 12/)).toBeTruthy();
    fireEvent.click(within(dlg).getByRole("button", { name: "Attendre Alice" }));
    expect(screen.queryByRole("dialog")).toBeNull();
    expect(services.hearthCalls.map((c) => c.kind)).toEqual(["Play"]);
    fireEvent.click(screen.getByRole("button", { name: "Jouer sur Les copains" }));
    fireEvent.click(await screen.findByRole("button", { name: "Jouer depuis la n° 11" }));
    await screen.findByText("Tu héberges · 1 joueur");
    expect(services.hearthCalls.map((c) => c.kind)).toEqual(["Play", "Play", "PlayFromPrevious"]);
  });

  it("dernière version sans contenu et aucune version complète : seulement Attendre", async () => {
    const { services } = home({ id: "g1", name: "Les copains" });
    services.setHearthEula(true);
    services.setHearthHeadUnavailable({ author: "Alice", head: 2, previous: null });
    fireEvent.click(await screen.findByRole("button", { name: "Jouer sur Les copains" }));
    const dlg = await screen.findByRole("dialog");
    expect(within(dlg).getByRole("button", { name: "Attendre Alice" })).toBeTruthy();
    expect(within(dlg).queryByRole("button", { name: /Jouer depuis/ })).toBeNull();
  });

  it("parseHeadUnavailable : lit le format stable, ignore les autres erreurs", () => {
    expect(parseHeadUnavailable("head_unavailable|12|11|Alice")).toEqual({ author: "Alice", head: 12, previous: 11 });
    expect(parseHeadUnavailable(new Error("head_unavailable|3||Alice B"))).toEqual({ author: "Alice B", head: 3, previous: null });
    expect(parseHeadUnavailable("aucun membre du groupe n'est joignable")).toBeNull();
    expect(parseHeadUnavailable("head_unavailable|x|1|A")).toBeNull();
  });

  it("premier hébergement : la boîte du contrat Minecraft s'ouvre, J'accepte relance Jouer", async () => {
    const { services } = home({ id: "g1", name: "Les copains" });
    fireEvent.click(await screen.findByRole("button", { name: "Jouer sur Les copains" }));
    const dlg = await screen.findByRole("dialog", { name: "Contrat d'utilisation de Minecraft" });
    expect(within(dlg).getByRole("link").getAttribute("href")).toBe("https://aka.ms/MinecraftEULA");
    fireEvent.click(within(dlg).getByRole("button", { name: "J'accepte" }));
    await screen.findByText("Tu héberges · 1 joueur");
    expect(services.hearthCalls.map((c) => c.kind)).toEqual(["Play", "EulaAccept", "Play"]);
    expect(screen.queryByRole("dialog")).toBeNull();
    // Les fois suivantes, plus de question.
    fireEvent.click(screen.getByRole("button", { name: "Arrêter le serveur Les copains" }));
    await screen.findByText("Personne ne joue");
    fireEvent.click(screen.getByRole("button", { name: "Jouer sur Les copains" }));
    await screen.findByText("Tu héberges · 1 joueur");
    expect(screen.queryByRole("dialog")).toBeNull();
  });

  it("refuser le contrat : rien n'est lancé", async () => {
    const { services } = home({ id: "g1", name: "Les copains" });
    fireEvent.click(await screen.findByRole("button", { name: "Jouer sur Les copains" }));
    fireEvent.click(await screen.findByRole("button", { name: "Annuler" }));
    expect(screen.queryByRole("dialog")).toBeNull();
    expect(services.hearthCalls.map((c) => c.kind)).toEqual(["Play"]);
    expect(screen.getByText("Personne ne joue")).toBeTruthy();
  });

  it("une erreur de lancement s'affiche sur la carte", async () => {
    const services = createFakeServices({ tickMs: 0 });
    services.seedHearth({ id: "g1", name: "Les copains" });
    services.hearthPlay = async () => { throw new Error("Java introuvable."); };
    render(<HearthSection services={services} navigate={vi.fn()} />);
    fireEvent.click(await screen.findByRole("button", { name: "Jouer sur Les copains" }));
    expect((await screen.findByRole("alert")).textContent).toBe("Java introuvable.");
  });
});

describe("Moments du groupe", () => {
  it("invitation : Rejoindre le groupe l'accepte et la carte apparaît ; Plus tard la masque sans la supprimer", async () => {
    const services = createFakeServices({ tickMs: 0 });
    render(<><HearthSection services={services} navigate={vi.fn()} /><HearthMoments services={services} /></>);
    await screen.findByText(/Un monde partagé/);
    act(() => { services.emitHearthInvite({ group_id: "g9", name: "Les potes" }); });
    const dlg = await screen.findByRole("dialog", { name: "Invitation à un serveur de groupe" });
    expect(within(dlg).getByText(/t'invite dans le groupe/)).toBeTruthy();
    fireEvent.click(within(dlg).getByRole("button", { name: "Plus tard" }));
    expect(screen.queryByRole("dialog")).toBeNull();
    expect((await services.hearthInvites()).length).toBe(1);
    act(() => { services.emitHearthInvite({ group_id: "g10", name: "Les autres" }); });
    fireEvent.click(await screen.findByRole("button", { name: "Rejoindre le groupe" }));
    await screen.findByRole("listitem", { name: "Serveur Les autres" });
    expect(services.hearthCalls).toEqual([{ kind: "InviteAccept", args: ["g10"] }]);
  });

  it("passation : l'hôte s'arrête, Reprendre le serveur lance hearthPlay", async () => {
    const services = createFakeServices({ tickMs: 0 });
    services.seedHearth({ id: "g1", name: "Les copains" });
    services.setHearthEula(true);
    render(<HearthMoments services={services} />);
    await act(async () => {});
    act(() => { services.emitHearthHandover({ group_id: "g1", name: "Les copains", from_name: "Alex" }); });
    const dlg = await screen.findByRole("dialog", { name: "L'hôte a arrêté le serveur" });
    expect(within(dlg).getByText(/Alex/)).toBeTruthy();
    fireEvent.click(within(dlg).getByRole("button", { name: "Reprendre le serveur" }));
    await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());
    expect(services.hearthCalls.map((c) => c.kind)).toEqual(["Play"]);
  });

  it("serveur resté ouvert : arrêter, ou garder (et toujours garder)", async () => {
    const services = createFakeServices({ tickMs: 0 });
    services.seedHearth({ id: "g1", name: "Les copains", state: "hosting" });
    render(<HearthMoments services={services} />);
    await act(async () => {});
    act(() => { services.emitHearthHostPrompt({ group_id: "g1", name: "Les copains", players: 2 }); });
    let dlg = await screen.findByRole("alertdialog", { name: "Ton serveur est encore ouvert" });
    expect(within(dlg).getByText(/2 joueurs sont encore/)).toBeTruthy();
    fireEvent.click(within(dlg).getByRole("checkbox"));
    fireEvent.click(within(dlg).getByRole("button", { name: "Garder le serveur ouvert" }));
    await waitFor(() => expect(screen.queryByRole("alertdialog")).toBeNull());
    expect(services.hearthCalls).toEqual([{ kind: "SetBackground", args: ["g1", true] }]);
    act(() => { services.emitHearthHostPrompt({ group_id: "g1", name: "Les copains", players: 1 }); });
    dlg = await screen.findByRole("alertdialog", { name: "Ton serveur est encore ouvert" });
    fireEvent.click(within(dlg).getByRole("button", { name: "Arrêter le serveur" }));
    await waitFor(() => expect(services.hearthCalls[1]).toEqual({ kind: "Stop", args: ["g1"] }));
  });
});

describe("Mémoire du PC", () => {
  it("avertissement de mémoire : message affiché, non bloquant, « Compris » le ferme", async () => {
    const services = createFakeServices({ tickMs: 0 });
    render(<HearthMoments services={services} />);
    await act(async () => {});
    act(() => { services.emitHearthRamWarning({ group_id: "g1", name: "Les copains", message: "Ton PC a 16 Go de mémoire : le serveur et le jeu risquent d'en manquer." }); });
    const dlg = await screen.findByRole("dialog", { name: "Ton PC risque de manquer de mémoire" });
    expect(within(dlg).getByText(/Ton PC a 16 Go de mémoire/)).toBeTruthy();
    expect(within(dlg).getByText(/démarre quand même/)).toBeTruthy();
    fireEvent.click(within(dlg).getByRole("button", { name: "Compris" }));
    expect(screen.queryByRole("dialog")).toBeNull();
    expect(services.hearthCalls).toEqual([]);
  });
});

describe("Création d'un serveur", () => {
  it("trois étapes : monde, membres, ce qui change ; puis création et ouverture de la page", async () => {
    const services = createFakeServices({ tickMs: 0 });
    const s = shell(services, { new: "1" });
    render(<ShellContext.Provider value={s.value}><Hearth /></ShellContext.Provider>);
    expect(await screen.findByRole("heading", { name: "Choisis le monde du groupe" })).toBeTruthy();
    await screen.findByRole("radio", { name: /Monde de survie/ });
    fireEvent.click(screen.getByRole("radio", { name: /Monde de survie/ }));
    fireEvent.click(screen.getByRole("button", { name: "Suivant" }));

    expect(await screen.findByRole("heading", { name: "Qui joue sur ce serveur ?" })).toBeTruthy();
    expect((screen.getByRole("button", { name: "Suivant" }) as HTMLButtonElement).disabled).toBe(true);
    fireEvent.click(screen.getByRole("checkbox", { name: /Alex/ }));
    fireEvent.click(screen.getByRole("button", { name: "Suivant" }));

    expect(await screen.findByRole("heading", { name: "Ce qui change pour le groupe" })).toBeTruthy();
    expect(screen.getByText(/412 Mo à télécharger chez 1 membre/)).toBeTruthy();
    expect(screen.getByText("Connexion requise.")).toBeTruthy();
    expect(screen.getByText("Mods figés.")).toBeTruthy();
    expect(screen.getByText("Un seul bouton Jouer.")).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: "Créer et inviter" }));
    await waitFor(() => expect(s.navigate).toHaveBeenCalledWith("hearth", { id: "grp-1" }));
    const call = services.hearthCalls[0]!.args[0] as { name: string; instanceId: string; world: string | null; members: string[] };
    expect(call).toMatchObject({ name: "Create: Aeronautics", instanceId: "inst-aeronautics", world: "Monde de survie" });
    expect(call.members).toHaveLength(1);
  });

  it("Retour revient à l'étape précédente ; Annuler quitte la création", async () => {
    const services = createFakeServices({ tickMs: 0 });
    const s = shell(services, { new: "1" });
    render(<ShellContext.Provider value={s.value}><Hearth /></ShellContext.Provider>);
    await screen.findByRole("radio", { name: /Nouveau monde/ });
    fireEvent.click(screen.getByRole("button", { name: "Suivant" }));
    await screen.findByRole("heading", { name: "Qui joue sur ce serveur ?" });
    fireEvent.click(screen.getByRole("button", { name: "Retour" }));
    await screen.findByRole("heading", { name: "Choisis le monde du groupe" });
    fireEvent.click(screen.getByRole("button", { name: "Annuler" }));
    expect(s.goBack).toHaveBeenCalledWith("main");
  });
});

describe("Page du serveur", () => {
  async function withBlocker() {
    const services = createFakeServices({ tickMs: 0 });
    const noor = (await services.friendList()).find((f) => f.friend.name === "Noor")!.friend.public_key;
    services.seedHearth({ id: "g1", name: "Les copains" }, {
      blocker: { member: { public_key: noor, name: "Noor", online: false }, days: 9 },
      members: [
        { public_key: "me", name: "Moi", online: true, is_me: true, is_creator: true, sync: "ok", version: 2, unseen_days: null },
        { public_key: noor, name: "Noor", online: false, is_me: false, is_creator: false, sync: "ok", version: 3, unseen_days: 9 },
      ],
    });
    const s = shell(services, { id: "g1" });
    render(<ShellContext.Provider value={s.value}><Hearth /></ShellContext.Provider>);
    return { services, pk: noor };
  }

  const me = { public_key: "me", name: "Toi", online: true, is_me: true, is_creator: true, sync: "ok", version: 1, unseen_days: null } as const;
  const noor = { public_key: "noor", name: "Noor", online: false, is_me: false, is_creator: false, sync: "ok", version: 1, unseen_days: null } as const;

  /** Groupe à deux membres, ouvert sur sa page ; `leave` remplace hearthLeave. */
  function twoMembers(id: string, leave?: () => Promise<void>) {
    const services = createFakeServices({ tickMs: 0 });
    services.seedHearth({ id, name: "Les copains" }, { members: [{ ...me }, { ...noor }] });
    if (leave) services.hearthLeave = async (gid) => { services.hearthCalls.push({ kind: "Leave", args: [gid] }); await leave(); };
    const s = shell(services, { id });
    render(<ShellContext.Provider value={s.value}><Hearth /></ShellContext.Provider>);
    return { services, s };
  }
  const clickQuitter = async () => {
    // sur ma ligne de l'onglet Membres, sans changer d'onglet
    fireEvent.click(await screen.findByRole("button", { name: "Quitter" }));
    return await screen.findByRole("alertdialog");
  };

  it("Quitter (ma ligne de Membres) : départ annoncé au groupe, retour à l'accueil", async () => {
    const { services, s } = twoMembers("g3");
    const dlg = await clickQuitter();
    expect(within(dlg).getByText(/Le serveur continue pour les autres membres/)).toBeTruthy();
    expect(within(dlg).getByText(/même si c'est toi qui l'as créé/)).toBeTruthy();
    fireEvent.click(within(dlg).getByRole("button", { name: "Quitter" }));
    await waitFor(() => expect(services.hearthCalls).toEqual([{ kind: "Leave", args: ["g3"] }]));
    await waitFor(() => expect(s.navigate).toHaveBeenCalledWith("main"));
  });

  it("Quitter seul dans le groupe : boîte « dernier membre » véridique, hearthLeave non appelé, Supprimer efface", async () => {
    const services = createFakeServices({ tickMs: 0 });
    services.seedHearth({ id: "g5", name: "Les copains" }, { members: [{ ...me }] });
    const s = shell(services, { id: "g5" });
    render(<ShellContext.Provider value={s.value}><Hearth /></ShellContext.Provider>);
    const dlg = await clickQuitter();
    expect(within(dlg).getByText(/dernier membre/)).toBeTruthy();
    expect(within(dlg).queryByText(/continue pour les autres/)).toBeNull();
    expect(services.hearthCalls).toEqual([]);
    fireEvent.click(within(dlg).getByRole("button", { name: "Supprimer le monde" }));
    await waitFor(() => expect(services.hearthCalls).toEqual([{ kind: "Forget", args: ["g5"] }]));
    await waitFor(() => expect(s.navigate).toHaveBeenCalledWith("main"));
  });

  it("Quitter seul dans le groupe, puis Annuler : aucun appel, le monde reste", async () => {
    const services = createFakeServices({ tickMs: 0 });
    services.seedHearth({ id: "g6", name: "Les copains" }, { members: [{ ...me }] });
    const s = shell(services, { id: "g6" });
    render(<ShellContext.Provider value={s.value}><Hearth /></ShellContext.Provider>);
    const dlg = await clickQuitter();
    fireEvent.click(within(dlg).getByRole("button", { name: "Annuler" }));
    await waitFor(() => expect(screen.queryByRole("alertdialog")).toBeNull());
    expect(services.hearthCalls).toEqual([]);
    expect(s.navigate).not.toHaveBeenCalled();
  });

  it("Quitter : le dernier membre l'apprend du serveur (les autres sont partis entre-temps) : même boîte, rien supprimé avant le clic", async () => {
    const { services, s } = twoMembers("g7", async () => { throw new HearthLeaveError("last_member"); });
    fireEvent.click(within(await clickQuitter()).getByRole("button", { name: "Quitter" }));
    const dlg = await screen.findByText(/dernier membre/).then(() => screen.getByRole("alertdialog"));
    expect(within(dlg).getByRole("button", { name: "Supprimer le monde" })).toBeTruthy();
    expect(services.hearthCalls).toEqual([{ kind: "Leave", args: ["g7"] }]);
    expect(s.navigate).not.toHaveBeenCalled();
    fireEvent.click(within(dlg).getByRole("button", { name: "Supprimer le monde" }));
    await waitFor(() => expect(services.hearthCalls).toEqual([{ kind: "Leave", args: ["g7"] }, { kind: "Forget", args: ["g7"] }]));
    await waitFor(() => expect(s.navigate).toHaveBeenCalledWith("main"));
  });

  it("Quitter quand personne n'est joignable : seconde confirmation, Forget seulement après elle", async () => {
    const { services, s } = twoMembers("g4", async () => { throw new HearthLeaveError("unreachable"); });
    fireEvent.click(within(await clickQuitter()).getByRole("button", { name: "Quitter" }));
    const dlg = await screen.findByText(/ne leur sera pas annoncé/).then(() => screen.getByRole("alertdialog"));
    expect(services.hearthCalls).toEqual([{ kind: "Leave", args: ["g4"] }]);
    expect(s.navigate).not.toHaveBeenCalled();
    fireEvent.click(within(dlg).getByRole("button", { name: "Supprimer quand même" }));
    await waitFor(() => expect(services.hearthCalls).toEqual([{ kind: "Leave", args: ["g4"] }, { kind: "Forget", args: ["g4"] }]));
    await waitFor(() => expect(s.navigate).toHaveBeenCalledWith("main"));
  });

  it("Quitter alors que ma dernière partie n'est pas partie (#56) : boîte « Quitter quand même », hearthLeave rappelé avec force, rien n'est jeté avant", async () => {
    const services = createFakeServices({ tickMs: 0 });
    services.seedHearth({ id: "g10", name: "Les copains" }, { members: [{ ...me }, { ...noor }], settings: { keep_background: false, disk_bytes: 1, outbox_pending: 1, outbox_failed: null, instance_name: "x" } });
    const s = shell(services, { id: "g10" });
    render(<ShellContext.Provider value={s.value}><Hearth /></ShellContext.Provider>);
    fireEvent.click(within(await clickQuitter()).getByRole("button", { name: "Quitter" }));
    const dlg = await screen.findByText(/elle sera perdue pour le groupe/).then(() => screen.getByRole("alertdialog"));
    expect(within(dlg).getByText("Ta dernière partie n'est pas encore partie")).toBeTruthy();
    expect(services.hearthCalls).toEqual([{ kind: "Leave", args: ["g10"] }]);
    expect(s.navigate).not.toHaveBeenCalled();
    fireEvent.click(within(dlg).getByRole("button", { name: "Quitter quand même" }));
    await waitFor(() => expect(services.hearthCalls).toEqual([{ kind: "Leave", args: ["g10"] }, { kind: "Leave", args: ["g10", true] }]));
    await waitFor(() => expect(s.navigate).toHaveBeenCalledWith("main"));
  });

  it("Quitter alors que ma dernière partie n'est pas partie, puis Annuler : le monde reste", async () => {
    const { services, s } = twoMembers("g11", async () => { throw new HearthLeaveError("outbox_pending"); });
    fireEvent.click(within(await clickQuitter()).getByRole("button", { name: "Quitter" }));
    const dlg = await screen.findByText(/elle sera perdue pour le groupe/).then(() => screen.getByRole("alertdialog"));
    fireEvent.click(within(dlg).getByRole("button", { name: "Annuler" }));
    await waitFor(() => expect(screen.queryByRole("alertdialog")).toBeNull());
    expect(services.hearthCalls).toEqual([{ kind: "Leave", args: ["g11"] }]);
    expect(s.navigate).not.toHaveBeenCalled();
  });

  it("Quitter quand même, puis personne n'est joignable : on retombe sur la confirmation de suppression avec la partie perdue", async () => {
    const services = createFakeServices({ tickMs: 0 });
    services.seedHearth({ id: "g12", name: "Les copains" }, { members: [{ ...me }, { ...noor }], settings: { keep_background: false, disk_bytes: 1, outbox_pending: 2, outbox_failed: null, instance_name: "x" } });
    services.hearthLeave = async (gid, force) => {
      services.hearthCalls.push({ kind: "Leave", args: force ? [gid, true] : [gid] });
      throw new HearthLeaveError(force ? "unreachable" : "outbox_pending");
    };
    const s = shell(services, { id: "g12" });
    render(<ShellContext.Provider value={s.value}><Hearth /></ShellContext.Provider>);
    fireEvent.click(within(await clickQuitter()).getByRole("button", { name: "Quitter" }));
    fireEvent.click(within(await screen.findByText(/elle sera perdue pour le groupe/).then(() => screen.getByRole("alertdialog"))).getByRole("button", { name: "Quitter quand même" }));
    const dlg = await screen.findByText(/ne leur sera pas annoncé/).then(() => screen.getByRole("alertdialog"));
    expect(within(dlg).getByText(/2 parties n'ont pas été envoyées et seront perdues/)).toBeTruthy();
    expect(services.hearthCalls).toEqual([{ kind: "Leave", args: ["g12"] }, { kind: "Leave", args: ["g12", true] }]);
    expect(s.navigate).not.toHaveBeenCalled();
  });

  it("Quitter quand personne n'est joignable, puis Annuler : le monde reste", async () => {
    const { services, s } = twoMembers("g8", async () => { throw new HearthLeaveError("unreachable"); });
    fireEvent.click(within(await clickQuitter()).getByRole("button", { name: "Quitter" }));
    const dlg = await screen.findByText(/ne leur sera pas annoncé/).then(() => screen.getByRole("alertdialog"));
    fireEvent.click(within(dlg).getByRole("button", { name: "Annuler" }));
    await waitFor(() => expect(screen.queryByRole("alertdialog")).toBeNull());
    expect(services.hearthCalls).toEqual([{ kind: "Leave", args: ["g8"] }]);
    expect(s.navigate).not.toHaveBeenCalled();
  });

  it.each([
    ["erreur à code « other »", () => new HearthLeaveError("other", "Trop de propositions en attente.")],
    ["erreur ordinaire", () => new Error("Trop de propositions en attente.")],
    ["chaîne brute", () => "Trop de propositions en attente."],
  ])("Quitter échoue pour une autre raison (%s) : l'erreur est affichée, rien n'est supprimé, pas de retour à l'accueil", async (_n, mk) => {
    const { services, s } = twoMembers("g9", async () => { throw mk(); });
    fireEvent.click(within(await clickQuitter()).getByRole("button", { name: "Quitter" }));
    expect(await screen.findByText("Trop de propositions en attente.")).toBeTruthy();
    expect(services.hearthCalls).toEqual([{ kind: "Leave", args: ["g9"] }]);
    expect(s.navigate).not.toHaveBeenCalled();
    expect(screen.getByRole("alertdialog")).toBeTruthy();
  });

  it("Membres : ma tête vient de mon compte actif, jamais du libellé « Toi » (un autre joueur porte ce pseudo)", async () => {
    const services = createFakeServices({ tickMs: 0 });
    const byName = vi.fn(async () => null);
    const byUuid = vi.fn(async () => null);
    services.skinHeadByName = byName;
    services.authSkinHead = byUuid;
    const acc = await services.authAccounts();
    const active = acc.accounts.find((a) => a.id === acc.active);
    services.seedHearth({ id: "g2", name: "Les copains" }, {
      members: [{ public_key: "me", name: "Toi", online: true, is_me: true, is_creator: true, sync: "ok", version: 1, unseen_days: null }],
    });
    const s = shell(services, { id: "g2" });
    render(<ShellContext.Provider value={s.value}><Hearth /></ShellContext.Provider>);
    await screen.findByText(/Toi \(toi\)/);
    await new Promise((r) => setTimeout(r, 50));
    expect(byName).not.toHaveBeenCalledWith("Toi");
    if (active?.kind === "online") expect(byUuid).toHaveBeenCalledWith(active.uuid);
  });

  it("Membres : bannière « n'est pas revenu depuis N jours » et vote par majorité", async () => {
    const { services, pk } = await withBlocker();
    await screen.findByText("Noor avait la version la plus récente et n'est pas revenu depuis 9 jours.");
    fireEvent.click(screen.getByRole("button", { name: "Continuer sans Noor" }));
    await waitFor(() => expect(services.hearthCalls).toEqual([{ kind: "ContinueWithout", args: ["g1", pk] }]));
    const list = await screen.findByRole("region", { name: "Décisions du groupe" });
    expect(within(list).getByText("Continuer sans Noor")).toBeTruthy();
    expect(within(list).getByText("Tu as voté")).toBeTruthy();
  });

  it("Membres : état de synchro, retrait confirmé", async () => {
    const { services, pk } = await withBlocker();
    await screen.findByText(/Pas vu depuis 9 jours/);
    fireEvent.click(screen.getByRole("button", { name: "Retirer Noor" }));
    const dlg = await screen.findByRole("alertdialog", { name: "Retirer Noor ?" });
    fireEvent.click(within(dlg).getByRole("button", { name: "Retirer" }));
    await waitFor(() => expect(services.hearthCalls).toEqual([{ kind: "RemoveMember", args: ["g1", pk] }]));
    await waitFor(() => expect(screen.queryByRole("button", { name: "Retirer Noor" })).toBeNull());
  });

  it("Membres : ajouter un ami du réseau", async () => {
    const { services } = page({ id: "g1" }, { id: "g1", name: "Les copains" }, {
      members: [{ public_key: "me", name: "Moi", online: true, is_me: true, is_creator: true, sync: "ok", version: 1, unseen_days: null }],
    });
    fireEvent.click(await screen.findByRole("button", { name: "Ajouter au groupe" }));
    await waitFor(() => expect(services.hearthCalls.map((c) => c.kind)).toEqual(["AddMember"]));
    await screen.findByText(/Alex/);
  });

  it("Historique : Restaurer demande confirmation puis ajoute une version", async () => {
    const { services } = page({ id: "g1" }, { id: "g1", name: "Les copains" });
    services.setHearthDetail("g1", {
      versions: [
        { id: "v3", number: 3, kind: "session", author: "k", author_name: "Alex", created_at: 1_760_000_100, played_secs: 3600, total_size: 400_000_000, is_head: true, side_branch: false, available: true },
        { id: "v2", number: 2, kind: "checkpoint", author: "k", author_name: "Noor", created_at: 1_760_000_000, played_secs: 900, total_size: 390_000_000, is_head: false, side_branch: false, available: true },
      ],
    });
    fireEvent.click(await screen.findByRole("tab", { name: "Historique" }));
    expect(await screen.findByText(/Version 3 · Partie · actuelle/)).toBeTruthy();
    expect(screen.queryByRole("button", { name: "Restaurer la version 3" })).toBeNull();
    fireEvent.click(screen.getByRole("button", { name: "Restaurer la version 2" }));
    fireEvent.click(within(await screen.findByRole("alertdialog")).getByRole("button", { name: "Restaurer" }));
    await waitFor(() => expect(services.hearthCalls).toEqual([{ kind: "Restore", args: ["g1", "v2"] }]));
    await screen.findByText(/Version 4 · Restauration · actuelle/);
  });

  it("Historique : une version dont le contenu est libéré n'a pas de bouton Restaurer (#96)", async () => {
    const { services } = page({ id: "g1" }, { id: "g1", name: "Les copains" });
    services.setHearthDetail("g1", {
      versions: [
        { id: "v3", number: 3, kind: "session", author: "k", author_name: "Alex", created_at: 1_760_000_100, played_secs: 3600, total_size: 400_000_000, is_head: true, side_branch: false, available: true },
        { id: "v2", number: 2, kind: "checkpoint", author: "k", author_name: "Noor", created_at: 1_760_000_000, played_secs: 900, total_size: 390_000_000, is_head: false, side_branch: false, available: true },
        { id: "v1", number: 1, kind: "session", author: "k", author_name: "Noor", created_at: 1_750_000_000, played_secs: 900, total_size: 380_000_000, is_head: false, side_branch: false, available: false },
      ],
    });
    fireEvent.click(await screen.findByRole("tab", { name: "Historique" }));
    expect(await screen.findByRole("button", { name: "Restaurer la version 2" })).toBeTruthy();
    expect(screen.queryByRole("button", { name: "Restaurer la version 1" })).toBeNull();
    expect(screen.getAllByText(/contenu libéré de ce PC/)).toHaveLength(1);
  });

  it("Réglages : garder le serveur, proposer la mise à jour du modpack (pas de bouton Quitter ici)", async () => {
    const { services, navigate } = page({ id: "g1" }, { id: "g1", name: "Les copains" });
    fireEvent.click(await screen.findByRole("tab", { name: "Réglages" }));
    fireEvent.click(await screen.findByRole("checkbox", { name: /Garder le serveur ouvert/ }));
    await waitFor(() => expect(services.hearthCalls[0]).toEqual({ kind: "SetBackground", args: ["g1", true] }));
    expect(screen.getByText(/Place prise sur ce disque : 380 Mo/)).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: "Proposer la mise à jour au groupe" }));
    fireEvent.click(within(await screen.findByRole("alertdialog")).getByRole("button", { name: "Proposer au groupe" }));
    await waitFor(() => expect(services.hearthCalls[1]).toEqual({ kind: "PackUpdate", args: ["g1", "inst-aeronautics"] }));
    // un seul « Quitter » sur la page : celui de ma ligne dans Membres, pas de doublon dans Réglages
    expect(screen.queryByRole("button", { name: /^Quitter/ })).toBeNull();
    expect(navigate).not.toHaveBeenCalled();
  });

  it("Réglages : une partie refusée par les amis (#99) est expliquée au lieu du simple compteur", async () => {
    page({ id: "g20" }, { id: "g20", name: "Les copains" }, { settings: { keep_background: false, disk_bytes: 1, outbox_pending: 1, outbox_failed: "Ta dernière partie (2,5 Go) n'a pas été envoyée : elle est beaucoup plus grosse que la précédente.", instance_name: "x" } });
    fireEvent.click(await screen.findByRole("tab", { name: "Réglages" }));
    expect(await screen.findByText(/Ta dernière partie \(2,5 Go\) n'a pas été envoyée/)).toBeTruthy();
    expect(screen.queryByText(/attend d'être envoyée/)).toBeNull();
  });

  it("deux versions : choisir celle à garder résout la fourche", async () => {
    const { services } = page({ id: "g1" }, { id: "g1", name: "Les copains", state: "fork" }, {
      fork: [
        { version_id: "va", number: 4, author_name: "Alex", created_at: 1_760_000_000, played_secs: 3600, total_size: 400_000_000 },
        { version_id: "vb", number: 4, author_name: "Noor", created_at: 1_760_000_050, played_secs: 1800, total_size: 395_000_000 },
      ],
    });
    await screen.findByRole("heading", { name: "Deux versions du monde existent" });
    fireEvent.click(screen.getByRole("button", { name: "Garder la version de Noor" }));
    await waitFor(() => expect(services.hearthCalls).toEqual([{ kind: "ResolveFork", args: ["g1", "vb"] }]));
    await waitFor(() => expect(screen.queryByRole("heading", { name: "Deux versions du monde existent" })).toBeNull());
  });

  const orphan = { id: "1763600000", at: 1_763_600_000, played_secs: 480, base_number: 11, size_bytes: 52_000_000 };

  it("fin de partie non publiée (#61) : bandeau, Publier appelle hearthOrphanPublish et le bandeau disparaît", async () => {
    const { services } = page({ id: "g1" }, { id: "g1", name: "Les copains" }, { orphans: [orphan] });
    await screen.findByRole("heading", { name: "Une fin de partie n'a pas été publiée" });
    expect(screen.getByText(/partie de la version 11/)).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: /^Publier la fin de partie/ }));
    await waitFor(() => expect(services.hearthCalls).toEqual([{ kind: "OrphanPublish", args: ["g1", "1763600000"] }]));
    await waitFor(() => expect(screen.queryByRole("heading", { name: "Une fin de partie n'a pas été publiée" })).toBeNull());
  });

  it("fin de partie non publiée : Supprimer demande confirmation avant d'appeler hearthOrphanDiscard", async () => {
    const { services } = page({ id: "g1" }, { id: "g1", name: "Les copains" }, { orphans: [orphan] });
    fireEvent.click(await screen.findByRole("button", { name: /^Supprimer la fin de partie/ }));
    const dlg = await screen.findByRole("alertdialog");
    expect(services.hearthCalls).toEqual([]);
    fireEvent.click(within(dlg).getByRole("button", { name: "Annuler" }));
    await waitFor(() => expect(screen.queryByRole("alertdialog")).toBeNull());
    expect(services.hearthCalls).toEqual([]);
    fireEvent.click(screen.getByRole("button", { name: /^Supprimer la fin de partie/ }));
    fireEvent.click(within(await screen.findByRole("alertdialog")).getByRole("button", { name: "Supprimer" }));
    await waitFor(() => expect(services.hearthCalls).toEqual([{ kind: "OrphanDiscard", args: ["g1", "1763600000"] }]));
    await waitFor(() => expect(screen.queryByRole("heading", { name: "Une fin de partie n'a pas été publiée" })).toBeNull());
  });

  it("Quitter avec une fin de partie non publiée : boîte « Quitter quand même », rien n'est supprimé avant", async () => {
    const { services, navigate } = page({ id: "g1" }, { id: "g1", name: "Les copains" }, { orphans: [orphan], members: [{ ...me }, { ...noor }] });
    fireEvent.click(within(await clickQuitter()).getByRole("button", { name: "Quitter" }));
    const dlg = await screen.findByText(/supprimée avec le monde/).then(() => screen.getByRole("alertdialog"));
    expect(services.hearthCalls).toEqual([{ kind: "Leave", args: ["g1"] }]);
    expect(navigate).not.toHaveBeenCalled();
    fireEvent.click(within(dlg).getByRole("button", { name: "Quitter quand même" }));
    await waitFor(() => expect(services.hearthCalls).toEqual([{ kind: "Leave", args: ["g1"] }, { kind: "Leave", args: ["g1", true] }]));
    await waitFor(() => expect(navigate).toHaveBeenCalledWith("main"));
  });

  it("accueil : la carte signale une fin de partie non publiée et ouvre la page du serveur", async () => {
    const { navigate } = home({ id: "g1", name: "Les copains", orphans: 1 });
    const card = await screen.findByRole("listitem", { name: "Serveur Les copains" });
    expect(within(card).getByText(/Une fin de partie n'a pas été publiée/)).toBeTruthy();
    fireEvent.click(within(card).getByRole("button", { name: /Voir la fin de partie non publiée/ }));
    expect(navigate).toHaveBeenCalledWith("hearth", { id: "g1" });
  });

  const alexCrash = { public_key: "k", name: "Alex", online: false };

  it("plantage du serveur d'un ami (#98) : la carte et la page disent le mod probable", async () => {
    home({ id: "g1", name: "Les copains", crash: { host: alexCrash, mine: false, mod_hint: "Sodium (sodium)" } });
    const card = await screen.findByRole("listitem", { name: "Serveur Les copains" });
    expect(within(card).getByText("Le serveur de Alex a planté (mod probable : Sodium (sodium)).")).toBeTruthy();
  });

  it("plantage sans mod connu : cause inconnue ; chez l'hôte, le monde est gardé de côté", async () => {
    page({ id: "g1" }, { id: "g1", name: "Les copains", crash: { host: { ...alexCrash, name: "Toi" }, mine: true, mod_hint: null }, orphans: 1 }, { orphans: [orphan] });
    await screen.findByText("Ton serveur a planté (cause inconnue).");
    expect(screen.getByText(/gardé de côté sur ce PC/)).toBeTruthy();
    expect(screen.getByRole("heading", { name: "Une fin de partie n'a pas été publiée" })).toBeTruthy();
  });

  it("sans plantage connu, aucun message", async () => {
    home({ id: "g1", name: "Les copains" });
    await screen.findByRole("listitem", { name: "Serveur Les copains" });
    expect(screen.queryByText(/a planté/)).toBeNull();
  });

  it("chat vocal (§ 6.7) : la carte et la page disent que le mod n'est pas pris en charge", async () => {
    home({ id: "g1", name: "Les copains", voice_unsupported: true });
    const card = await screen.findByRole("listitem", { name: "Serveur Les copains" });
    expect(within(card).getByText(/Chat vocal non pris en charge/)).toBeTruthy();
    cleanup();
    page({ id: "g1" }, { id: "g1", name: "Les copains", voice_unsupported: true });
    expect(await screen.findByText("Chat vocal non pris en charge")).toBeTruthy();
  });

  it("sans chat vocal dans le modpack, aucun message de voix", async () => {
    home({ id: "g1", name: "Les copains" });
    await screen.findByRole("listitem", { name: "Serveur Les copains" });
    expect(screen.queryByText(/Chat vocal/)).toBeNull();
  });

  it("modpack absent : associer une instance locale", async () => {
    const { services } = page({ id: "g1" }, { id: "g1", name: "Les copains", state: "pack_missing", can_host: false, instance_id: null });
    fireEvent.click(await screen.findByRole("button", { name: /Utiliser Create: Aeronautics/ }));
    await waitFor(() => expect(services.hearthCalls).toEqual([{ kind: "SetInstance", args: ["g1", "inst-aeronautics"] }]));
    await screen.findByText("Personne ne joue");
  });

  it("groupe inconnu : message et retour à l'accueil", async () => {
    const { navigate } = page({ id: "absent" });
    expect((await screen.findByRole("alert")).textContent).toMatch(/introuvable/);
    fireEvent.click(screen.getByRole("button", { name: "Retour à l'accueil" }));
    expect(navigate).toHaveBeenCalledWith("main");
  });

  it("sans paramètre : retour à l'accueil", async () => {
    const { navigate } = page({});
    await waitFor(() => expect(navigate).toHaveBeenCalledWith("main"));
  });
});

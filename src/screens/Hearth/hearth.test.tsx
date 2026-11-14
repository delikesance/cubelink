import { afterEach, describe, expect, it, vi } from "vitest";
import { act, cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { ShellContext, type ShellContextValue } from "../../app/context";
import { createFakeServices } from "../../services/fake";
import { HearthSection } from "./HearthSection";
import { HearthMoments } from "./Moments";
import Hearth from "./index";

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

  it("états sans bouton de jeu : mise à jour, connexion requise, deux versions", async () => {
    const services = createFakeServices({ tickMs: 0 });
    services.seedHearth({ id: "a", name: "Sync", state: "syncing", progress: 42 });
    services.seedHearth({ id: "b", name: "Hors ligne", state: "blocked" });
    services.seedHearth({ id: "c", name: "Fourche", state: "fork" });
    render(<HearthSection services={services} navigate={vi.fn()} />);
    await screen.findByText("Mise à jour du monde 42 %");
    expect(screen.getByRole("progressbar", { name: /Sync/ }).getAttribute("aria-valuenow")).toBe("42");
    const blocked = screen.getByRole("listitem", { name: "Serveur Hors ligne" });
    expect(within(blocked).getByText("Connexion requise")).toBeTruthy();
    expect(within(blocked).queryByRole("button", { name: /^Jouer/ })).toBeNull();
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
        { id: "v3", number: 3, kind: "session", author: "k", author_name: "Alex", created_at: 1_760_000_100, played_secs: 3600, total_size: 400_000_000, is_head: true, side_branch: false },
        { id: "v2", number: 2, kind: "checkpoint", author: "k", author_name: "Noor", created_at: 1_760_000_000, played_secs: 900, total_size: 390_000_000, is_head: false, side_branch: false },
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

  it("Réglages : garder le serveur, proposer la mise à jour du modpack, quitter le groupe", async () => {
    const { services, navigate } = page({ id: "g1" }, { id: "g1", name: "Les copains" });
    fireEvent.click(await screen.findByRole("tab", { name: "Réglages" }));
    fireEvent.click(await screen.findByRole("checkbox", { name: /Garder le serveur ouvert/ }));
    await waitFor(() => expect(services.hearthCalls[0]).toEqual({ kind: "SetBackground", args: ["g1", true] }));
    expect(screen.getByText(/Place prise sur ce disque : 380 Mo/)).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: "Proposer la mise à jour au groupe" }));
    fireEvent.click(within(await screen.findByRole("alertdialog")).getByRole("button", { name: "Proposer au groupe" }));
    await waitFor(() => expect(services.hearthCalls[1]).toEqual({ kind: "PackUpdate", args: ["g1", "inst-aeronautics"] }));
    fireEvent.click(screen.getByRole("button", { name: "Quitter le groupe" }));
    fireEvent.click(within(await screen.findByRole("alertdialog")).getByRole("button", { name: "Quitter" }));
    await waitFor(() => expect(navigate).toHaveBeenCalledWith("main"));
    expect(services.hearthCalls[2]).toEqual({ kind: "Leave", args: ["g1"] });
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

import { useEffect, useRef, useState } from "react";
import "./hearth.css";
import { useShell } from "../../app/context";
import type { FriendWithPresence, HearthWorld, Instance } from "../../services/types";
import { Button, Card, Field, Input } from "../../ui";
import { Avatar } from "../Friends/Avatar";
import { errText, fmtSize, plural } from "./format";

const STEPS = ["Le monde", "Les membres", "Ce qui change"] as const;
/** Valeur du bouton radio « nouveau monde » (jamais un nom de monde : les noms de dossier ne peuvent pas la contenir). */
const NEW_WORLD = "\u0000new";

/** Création d'un serveur de groupe en trois étapes : monde, membres, récapitulatif ; puis les invitations partent. */
export default function Wizard() {
  const { services, navigate, goBack } = useShell();
  const [step, setStep] = useState(0);
  const [instances, setInstances] = useState<Instance[] | null>(null);
  const [friends, setFriends] = useState<FriendWithPresence[]>([]);
  const [instanceId, setInstanceId] = useState("");
  const [worlds, setWorlds] = useState<HearthWorld[] | null>(null);
  const [world, setWorld] = useState<string>(NEW_WORLD);
  const [name, setName] = useState("");
  const [nameTouched, setNameTouched] = useState(false);
  const [members, setMembers] = useState<Set<string>>(() => new Set());
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const title = useRef<HTMLHeadingElement>(null);

  useEffect(() => {
    let alive = true;
    services.instancesList().then((l) => {
      if (!alive) return;
      setInstances(l);
      if (l[0]) setInstanceId((cur) => cur || l[0]!.id);
    }, (e: unknown) => { if (alive) { setInstances([]); setError(errText(e)); } });
    services.friendList().then((l) => { if (alive) setFriends(l); }, () => {});
    return () => { alive = false; };
  }, [services]);

  const inst = instances?.find((i) => i.id === instanceId);
  useEffect(() => { if (inst && !nameTouched) setName(inst.name); }, [inst, nameTouched]);

  useEffect(() => {
    if (!instanceId) return;
    let alive = true;
    setWorlds(null);
    setWorld(NEW_WORLD);
    services.hearthWorlds(instanceId).then(
      (l) => { if (alive) setWorlds(l); },
      (e: unknown) => { if (alive) { setWorlds([]); setError(errText(e)); } },
    );
    return () => { alive = false; };
  }, [services, instanceId]);

  useEffect(() => { title.current?.focus(); }, [step]);

  const chosen = worlds?.find((w) => w.name === world);
  const picked = friends.filter((f) => members.has(f.friend.public_key));
  const canNext = step === 0 ? !!inst && name.trim().length > 0 && worlds !== null : step === 1 ? members.size > 0 : true;
  const toggle = (pk: string) => setMembers((s) => { const n = new Set(s); if (n.has(pk)) n.delete(pk); else n.add(pk); return n; });

  const create = async () => {
    setError(null);
    setBusy(true);
    try {
      const card = await services.hearthCreate({ name: name.trim(), instanceId, world: world === NEW_WORLD ? null : world, members: [...members] });
      navigate("hearth", { id: card.id });
    } catch (e) {
      setError(errText(e));
      setBusy(false);
    }
  };

  return (
    <div className="hearth-page">
      <header className="hearth-page-head">
        <p className="hearth-step">Étape {step + 1} sur {STEPS.length}</p>
        <h1 ref={title} tabIndex={-1}>{step === 0 ? "Choisis le monde du groupe" : step === 1 ? "Qui joue sur ce serveur ?" : "Ce qui change pour le groupe"}</h1>
      </header>

      <Card className="hearth-wiz">
        {step === 0 && (
          <div className="hearth-form">
            {instances === null ? <p role="status">Chargement…</p> : instances.length === 0 ? (
              <p>Tu n'as pas encore de modpack. <button type="button" className="hearth-link" onClick={() => navigate("create")}>Crée-en un d'abord</button>.</p>
            ) : (
              <>
                <Field label="Modpack du groupe" hint="Tout le monde jouera avec ce modpack, mods compris.">
                  {({ id, "aria-describedby": described }) => (
                    <select id={id} aria-describedby={described} className="cl-input" value={instanceId} onChange={(e) => setInstanceId(e.target.value)}>
                      {instances.map((i) => <option key={i.id} value={i.id}>{i.name} ({i.minecraft})</option>)}
                    </select>
                  )}
                </Field>
                <Field label="Nom du serveur">
                  {(p) => <Input {...p} value={name} maxLength={40} onChange={(e) => { setNameTouched(true); setName(e.target.value); }} />}
                </Field>
                <fieldset className="hearth-radios">
                  <legend>Monde</legend>
                  {worlds === null ? <p role="status">Recherche des mondes…</p> : (
                    <>
                      {worlds.map((w) => (
                        <label key={w.name} className="hearth-radio">
                          <input type="radio" name="world" checked={world === w.name} onChange={() => setWorld(w.name)} />
                          <span><strong>{w.name}</strong> <span className="hearth-meta">{fmtSize(w.size_bytes)}</span></span>
                        </label>
                      ))}
                      <label className="hearth-radio">
                        <input type="radio" name="world" checked={world === NEW_WORLD} onChange={() => setWorld(NEW_WORLD)} />
                        <span><strong>Nouveau monde</strong> <span className="hearth-meta">créé au premier lancement</span></span>
                      </label>
                    </>
                  )}
                </fieldset>
              </>
            )}
          </div>
        )}

        {step === 1 && (
          <fieldset className="hearth-radios">
            <legend>Amis invités</legend>
            {friends.length === 0 ? <p>Aucun ami pour l'instant. Ajoute des amis dans les paramètres pour les inviter.</p> : friends.map((f) => (
              <label key={f.friend.public_key} className="hearth-radio">
                <input type="checkbox" checked={members.has(f.friend.public_key)} onChange={() => toggle(f.friend.public_key)} />
                <Avatar name={f.friend.name} size={28} presence={f.presence === "online" ? "online" : "offline"} services={services} />
                <span><strong>{f.friend.name}</strong> <span className="hearth-meta">{f.presence === "online" ? "en ligne" : "hors ligne"}</span></span>
              </label>
            ))}
          </fieldset>
        )}

        {step === 2 && (
          <ul className="hearth-changes">
            <li>
              <strong>Une copie du monde chez chacun.</strong>{" "}
              {chosen ? `${fmtSize(chosen.size_bytes)} à télécharger chez ${plural(picked.length, "membre", "membres")} (${picked.map((f) => f.friend.name).join(", ")}).` : `Le nouveau monde sera copié chez ${plural(picked.length, "membre", "membres")} (${picked.map((f) => f.friend.name).join(", ")}) après le premier lancement.`}
            </li>
            <li><strong>Connexion requise.</strong> Jouer hors ligne est interdit sur un serveur de groupe : sinon deux versions du monde pourraient apparaître.</li>
            <li><strong>Mods figés.</strong> Le modpack « {inst?.name} » ne change que si le groupe vote la mise à jour.</li>
            <li><strong>Un seul bouton Jouer.</strong> Le serveur démarre chez le premier qui joue, les autres le rejoignent.</li>
          </ul>
        )}
        {error && <p className="hearth-err" role="alert">{error}</p>}
      </Card>

      <div className="hearth-wiz-foot">
        <Button variant="secondary" onClick={() => (step === 0 ? goBack("main") : setStep(step - 1))}>{step === 0 ? "Annuler" : "Retour"}</Button>
        {step < STEPS.length - 1
          ? <Button disabled={!canNext} onClick={() => setStep(step + 1)}>Suivant</Button>
          : <Button loading={busy} onClick={() => { void create(); }}>Créer et inviter</Button>}
      </div>
    </div>
  );
}

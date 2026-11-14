import { useEffect } from "react";
import "./hearth.css";
import { useScreenTitle, useShell } from "../../app/context";
import { HearthMoments } from "./Moments";
import ServerPage from "./ServerPage";
import Wizard from "./Wizard";

/** Serveurs de groupe : `#/hearth?new=1` ouvre la création, `#/hearth?id=<groupe>` la page d'un serveur. */
export default function Hearth() {
  const { services, params, navigate } = useShell();
  const id = params.id;
  const creating = params.new === "1";
  useScreenTitle(creating ? "Nouveau serveur de groupe" : null);
  useEffect(() => { if (!creating && !id) navigate("main"); }, [creating, id, navigate]);
  return (
    <>
      {creating ? <Wizard /> : id ? <ServerPage key={id} id={id} /> : null}
      <HearthMoments services={services} />
    </>
  );
}

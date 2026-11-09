import type { ReactNode } from "react";

export interface LegalSection { id: string; label: string; content: ReactNode }

const Todo = () => <span className="legal-todo">à compléter</span>;
const Ext = ({ href, children }: { href: string; children: ReactNode }) => (
  <a className="legal-lnk" href={href} target="_blank" rel="noopener noreferrer">{children}</a>
);

export const SECTIONS: LegalSection[] = [
  {
    id: "editeur", label: "Éditeur", content: (
      <>
        <h2>Éditeur</h2>
        <p>Nom ou raison sociale&nbsp;: <Todo /></p>
        <p>Adresse et statut juridique&nbsp;: <Todo /></p>
      </>
    ),
  },
  {
    id: "non-affiliation", label: "Non-affiliation", content: (
      <>
        <h2>Non-affiliation</h2>
        <p className="legal-strong">Cubelink est un projet indépendant. Il n'est pas un produit Minecraft officiel, n'est pas approuvé par Mojang ou Microsoft et n'est pas associé à eux.</p>
      </>
    ),
  },
  {
    id: "donnees", label: "Données traitées", content: (
      <>
        <h2>Données traitées</h2>
        <ul>
          <li>Ton pseudo Minecraft, affiché à tes amis. Cubelink ne le vérifie pas auprès d'eux.</li>
          <li>Une clé publique générée au premier lancement, qui identifie ton installation.</li>
          <li>Ton adresse IP, visible de tes amis en connexion directe et du relais quand il transmet ton trafic.</li>
          <li>Un cache local de mods, sur ton disque.</li>
        </ul>
        <p>Cubelink ne stocke ni ton compte ni ta liste d'amis sur un serveur. Un relais peut transmettre ton trafic quand la connexion directe échoue.</p>
        <details className="legal-more">
          <summary>En savoir plus</summary>
          <p>Ta liste d'amis reste sur ton ordinateur, et aucune donnée de compte n'est conservée sur un serveur du projet. La connexion à ton compte Microsoft se fait directement entre ton ordinateur et Microsoft.</p>
        </details>
        <p>Durée de conservation et base légale du traitement&nbsp;: <Todo /></p>
      </>
    ),
  },
  {
    id: "relais", label: "Le relais", content: (
      <>
        <h2>Le relais</h2>
        <p>Un relais est un serveur intermédiaire. Il transmet ton trafic quand la connexion directe échoue, ou si tu as choisi «&nbsp;Passer toujours par le relais&nbsp;». Il voit alors ton adresse IP.</p>
        <p>Ce que le relais peut lire du contenu transmis n'est pas encore établi&nbsp;: ne pars pas du principe qu'il ne voit rien. Cette partie sera précisée une fois le fonctionnement du relais validé.</p>
      </>
    ),
  },
  {
    id: "licences", label: "Licences", content: (
      <>
        <h2>Licences des images et des polices</h2>
        <details className="legal-more">
          <summary>Images (licence OGA-BY&nbsp;3.0 et CC0)</summary>
          <p>«&nbsp;Moon and Sea Pixel Art Background&nbsp;» par CraftPix.net 2D Game Assets, <Ext href="https://opengameart.org/content/moon-and-sea-pixel-art-background">https://opengameart.org/content/moon-and-sea-pixel-art-background</Ext>, licence <Ext href="https://opengameart.org/content/oga-by-30-faq">OGA-BY&nbsp;3.0</Ext>. Modifié&nbsp;: converti en JPEG ; recadré pour la vignette «&nbsp;Créer un monde&nbsp;».</p>
          <p>Autres illustrations (CC0), via OpenGameArt.org&nbsp;: «&nbsp;City Parallax pixel art&nbsp;» par Gustavo Saraiva, «&nbsp;City Scrollable Background Pixel Art&nbsp;» par Quantiset, «&nbsp;Desert Forest&nbsp;» par LLGD, «&nbsp;RPG portraits&nbsp;» par Buch. Aucune ressource Minecraft ou Mojang n'est utilisée.</p>
        </details>
        <details className="legal-more">
          <summary>Polices (SIL Open Font License&nbsp;1.1)</summary>
          <p>Polices Inter (The Inter Project Authors) et Montserrat (The Montserrat Project Authors), sous licence <Ext href="https://openfontlicense.org/">SIL Open Font License&nbsp;1.1</Ext>.</p>
        </details>
      </>
    ),
  },
  {
    id: "contact", label: "Contact", content: (
      <>
        <h2>Contact</h2>
        <p>Pour toute question sur tes données ou sur ces mentions&nbsp;: <Todo /> (adresse e-mail de contact).</p>
      </>
    ),
  },
];

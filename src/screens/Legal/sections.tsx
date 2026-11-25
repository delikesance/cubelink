import type { ReactNode } from "react";

export interface LegalSection { id: string; label: string; content: ReactNode }

const Ext = ({ href, children }: { href: string; children: ReactNode }) => (
  <a className="legal-lnk" href={href} target="_blank" rel="noopener noreferrer">{children}</a>
);

export const SECTIONS: LegalSection[] = [
  {
    id: "projet", label: "Le projet", content: (
      <>
        <h3>Le projet</h3>
        <p>Cubelink est un projet personnel, gratuit et sans but commercial&nbsp;: pas de publicité, pas d'abonnement, aucune donnée revendue.</p>
      </>
    ),
  },
  {
    id: "non-affiliation", label: "Non-affiliation", content: (
      <>
        <h3>Non-affiliation</h3>
        <p className="legal-strong">Cubelink est un projet indépendant. Il n'est pas un produit Minecraft officiel, n'est pas approuvé par Mojang ou Microsoft et n'est pas associé à eux.</p>
      </>
    ),
  },
  {
    id: "donnees", label: "Données traitées", content: (
      <>
        <h3>Données traitées</h3>
        <ul>
          <li>Ton pseudo Minecraft, affiché à tes amis. Cubelink ne le vérifie pas auprès d'eux.</li>
          <li>Une clé publique générée au premier lancement, qui identifie ton installation.</li>
          <li>Ton adresse IP, visible de tes amis en connexion directe, et écrite dans ton code ami (jusqu'à 8 adresses). Aucun serveur intermédiaire ne la voit.</li>
          <li>Le fait qu'un jeu tourne chez toi (statut «&nbsp;en jeu&nbsp;»), visible de tes amis.</li>
          <li>Un cache local de mods, sur ton disque. Pour un monde de groupe Hearth, une copie du monde est conservée chez les autres membres du groupe.</li>
        </ul>
        <p>Cubelink ne stocke ni ton compte ni ta liste d'amis sur un serveur, et ne publie ni ta clé ni ton adresse dans un annuaire public. Aucun relais ni service tiers ne relie tes amis à toi. Cubelink peut démarrer avec ta session (réglage modifiable) pour que tes amis te retrouvent. Les images des descriptions de mods viennent de sites tiers, qui voient ton adresse IP.</p>
        <details className="legal-more">
          <summary>En savoir plus</summary>
          <p>Ta liste d'amis reste sur ton ordinateur, et aucune donnée de compte n'est conservée sur un serveur du projet. La connexion à ton compte Microsoft se fait directement entre ton ordinateur et Microsoft.</p>
        </details>
        <p>Ces données restent sur ton ordinateur tant que Cubelink y est installé&nbsp;; les désinstaller avec ses données les efface.</p>
      </>
    ),
  },
  {
    id: "relais", label: "Pas de relais", content: (
      <>
        <h3>Pas de relais</h3>
        <p>Un relais est un serveur intermédiaire qui transmet du trafic quand deux appareils ne peuvent pas se joindre. Cubelink n'en utilise aucun&nbsp;: ni les relais publics du projet Iroh (société n0), ni un relais de Cubelink. Tes connexions avec tes amis sont directes, et aucun annuaire public ne connaît ta clé ni ton adresse.</p>
        <p>Pour que ça marche, au moins l'un de vous deux doit être joignable&nbsp;: adresse IPv6 ou IPv4 publique, ou port ouvert automatiquement sur ta box (UPnP). En attendant la découverte par DHT Mainline et des relais Cubelink, deux amis tous deux derrière un réseau très fermé (NAT strict ou CGNAT, sans IPv6 ni UPnP) peuvent ne pas se joindre. Réglages, onglet Réseau, «&nbsp;Tester ma connexion&nbsp;» indique où tu en es.</p>
      </>
    ),
  },
  {
    id: "licences", label: "Licences", content: (
      <>
        <h3>Licences des images et des polices</h3>
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
];

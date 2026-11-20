// Table unique des routes de l'interface pour les E2E : hash, route affichee (`<main data-route=...>`) et sélecteur « prêt ».
// A adapter ICI (et nulle part ailleurs) quand `src/app/router.ts` ou un écran change.
//
// Alias : `friends` n'est plus une route. L'ancien écran Amis vit dans Paramètres > Amis (router.ts : `friends` ->
// `settings?tab=amis`), donc `data-route` vaut `settings` : attendre `data-route="friends"` ne réussit jamais.
export interface RouteDef {
  /** Hash à écrire pour y aller. */
  hash: string;
  /** Valeur de `data-route` une fois l'écran affiché. */
  route: string;
  /** Sélecteur CSS visible quand le contenu est chargé (au-delà de `data-route`, posé dès le premier rendu). */
  ready?: string;
}

/** Noms d'écran des tests -> définition. Les routes absentes se déduisent du nom (voir `routeOf`). */
export const ROUTE_ALIASES: Record<string, RouteDef> = {
  // Accueil : une tuile par modpack, ou l'état vide ; `.home-loading` n'est que transitoire.
  main: { hash: '#/main', route: 'main', ready: '.home-tile, .home-empty' },
  // Amis : onglet « Amis » des Paramètres (demandes reçues, ajouter un ami, mon code).
  friends: { hash: '#/settings?tab=amis', route: 'settings', ready: '.add-input' },
};

/** `join?friend=ab` -> { hash: '#/join?friend=ab', route: 'join' } ; `friends` -> l'alias ci-dessus. */
export function routeOf(name: string): RouteDef {
  const alias = ROUTE_ALIASES[name];
  if (alias) return alias;
  const clean = name.replace(/^#?\/?/, '');
  return { hash: `#/${clean}`, route: clean.split('?')[0] };
}

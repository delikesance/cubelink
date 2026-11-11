# Signature du code (SignPath Foundation)

L'installeur Windows est signe gratuitement par **SignPath Foundation** (programme open source).
Le depot doit etre **public**, sous licence OSS (MIT, fichier `LICENSE`), et l'installeur doit etre construit par la CI
(`.github/workflows/release.yml`) : SignPath refuse un binaire construit sur un poste personnel.

## Mise en place (une fois, par le mainteneur)
1. Rendre le depot GitHub public et verifier que `.env` n'y est pas (il est ignore par git).
2. Demander l'inscription sur https://signpath.org/ (formulaire « Open Source », rubrique Foundation) avec l'URL du depot.
3. Une fois accepte : creer dans SignPath un projet `cubelink` (slug), la configuration d'artefact `default` (coller le
   contenu de `.signpath/artifact-configuration.xml`) et la politique de signature `release-signing`
   (origine : GitHub Actions, depot de ce projet). Relier le depot via l'app « SignPath » de GitHub (trusted build system).
4. Dans GitHub -> Settings : variable `SIGNPATH_ORGANIZATION_ID` et secret `SIGNPATH_API_TOKEN`.
5. Publier : `git tag v0.1.0 && git push origin v0.1.0`. Le job `sign` joint l'installeur signe a la release GitHub.

## Politique (texte exige par SignPath, a garder dans le README)
Free code signing provided by [SignPath.io](https://about.signpath.io/), certificate by
[SignPath Foundation](https://signpath.org/). Seuls les binaires construits par la CI de ce depot sont signes.
Cubelink ne collecte aucune donnee personnelle sur un serveur : les echanges entre amis sont directs (P2P chiffre).

## Limites connues
- SmartScreen peut encore avertir quelques jours : la reputation se construit avec les telechargements.
- Seul l'installeur est signe, pas le `cubelink.exe` embarque dans l'installeur.
- Ne jamais definir `CUBELINK_CF_API_KEY` dans la CI publique : `core/build.rs` l'integre au binaire.

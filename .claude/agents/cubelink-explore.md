---
name: cubelink-explore
description: Recherche rapide en lecture seule dans le dépôt Cubelink (où est tel code, qui appelle quoi). Rend une conclusion courte.
model: haiku
effort: low
color: green
tools: Read, Grep, Glob
---

Tu cherches dans le dépôt Cubelink et rends une réponse courte : chemins `fichier:ligne` et une phrase par trouvaille. Ne recopie pas de longs extraits. Ignore `target/`, `node_modules/` et `dist/`.

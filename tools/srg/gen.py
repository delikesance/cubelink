"""Genere core/resources/lan-map-srg/<mc>.map depuis mcp-<mc>-srg (joined.srg) et verifie chaque entree
contre le vrai client.jar de Mojang (javap sur les noms obfusques).

Usage : python -I gen.py <dossier_donnees> <dossier_sortie> <version>...
  <dossier_donnees>/<v>/joined.srg   (de de.oceanlabs.mcp:mcp:<v>:srg@zip, Maven Forge)
  <dossier_donnees>/<v>/client.jar   (piston-data, SHA1 a verifier par l'appelant)
Les fichiers telecharges sont des DONNEES : on ne les execute jamais (javap lit seulement les classes).
Aucun nom n'est invente : une entree non resolue/non verifiee => pas de fichier (code retour 1).
"""
import os, re, subprocess, sys

# cle -> (type, classe declarante MCP, nom SRG attendu, nbParams, retour attendu (binaire) ou None, statique)
# Les noms SRG ci-dessous sont ceux de joined.srg ; le script verifie nom + nbParams + retour + presence dans le jar.
SPEC = [
    ("mc.getInstance", "M", "net/minecraft/client/Minecraft", "func_71410_x", 0, "net.minecraft.client.Minecraft", True),
    ("mc.getSingleplayerServer", "M", "net/minecraft/client/Minecraft", "func_71401_C", 0, "net.minecraft.server.integrated.IntegratedServer", False),
    ("mc.player", "F", "net/minecraft/client/Minecraft", "field_71439_g", -1, None, False),
    ("is.shareToLan", "M", "net/minecraft/server/integrated/IntegratedServer", "func_71206_a", 2, "java.lang.String", False),
    ("is.isPublished", "M", "net/minecraft/server/integrated/IntegratedServer", "func_71344_c", 0, "boolean", False),
    ("server.getDefaultGameType", "M", "net/minecraft/server/integrated/IntegratedServer", "func_71265_f", 0, "GAMETYPE", False),
    ("mc.schedule", "M", "net/minecraft/client/Minecraft", "func_152344_a", 1, "com.google.common.util.concurrent.ListenableFuture", False),
]
CLASSES = [("minecraft", "net/minecraft/client/Minecraft"), ("integrated", "net/minecraft/server/integrated/IntegratedServer")]
PRIM = {"Z": "boolean", "B": "byte", "C": "char", "S": "short", "I": "int", "J": "long", "F": "float", "D": "double", "V": "void"}

def parse_desc(d):
    """'(Lx/Y;I)Lz/W;' -> (nb_params, retour binaire)."""
    m = re.match(r"\((.*)\)(.+)$", d)
    params, ret = m.group(1), m.group(2)
    n, i = 0, 0
    while i < len(params):
        while params[i] == "[": i += 1
        if params[i] == "L": i = params.index(";", i)
        i += 1; n += 1
    arr = ret.count("[") ; ret = ret.lstrip("[")
    r = PRIM[ret] if ret in PRIM else ret[1:-1].replace("/", ".")
    return n, r + "[]" * arr

def javap(jar, cls):
    out = subprocess.run(["javap", "-p", "-s", "-cp", jar, cls], capture_output=True, text=True, encoding="utf-8", errors="replace")
    if out.returncode != 0: return None
    members, cur = [], None
    for line in out.stdout.splitlines():
        s = line.strip()
        if s.startswith("descriptor:") and cur: members.append((cur, s.split(None, 1)[1])); cur = None
        elif line.startswith("  ") and not s.startswith(("descriptor", "flags", "Code", "Signature", "Exceptions")) and s.endswith(";"): cur = s
    return out.stdout.splitlines()[1] if len(out.stdout.splitlines()) > 1 else "", members

def gen(data, outdir, v):
    srg = os.path.join(data, v, "joined.srg"); jar = os.path.join(data, v, "client.jar")
    cl, md, fd = {}, {}, {}
    for l in open(srg, encoding="utf-8"):
        p = l.split()
        if p[0] == "CL:": cl[p[2]] = p[1]
        elif p[0] == "MD:":
            o, _, n = p[3].rpartition("/"); md[(o, n)] = (p[1], p[2], p[4])   # obf_cls/obf_name, obf_desc, deobf_desc
        elif p[0] == "FD:":
            o, _, n = p[2].rpartition("/"); fd[(o, n)] = p[1]
    lines, errs = [f"# cubelink-lanmap srg  (MC {v} ; genere par tools/srg/gen.py depuis mcp-{v}-srg, verifie sur client.jar)"], []
    cache = {}
    def members(obfcls):
        if obfcls not in cache: cache[obfcls] = javap(jar, obfcls.replace("/", "."))
        return cache[obfcls]
    for key, cls in CLASSES:
        if cls not in cl: errs.append(f"classe {cls} absente de joined.srg"); continue
        info = members(cl[cls])
        if info is None or "public" not in info[0] and True and not info[0].startswith("public"): errs.append(f"classe {cls} ({cl[cls]}) absente ou non publique dans client.jar")
        lines.append(f"C {key} {cls.replace('/', '.')}")
    for key, kind, owner, srgname, nparams, ret, static in SPEC:
        if kind == "F":
            if (owner, srgname) not in fd: errs.append(f"{key}: champ {owner}.{srgname} absent de joined.srg"); continue
            obf = fd[(owner, srgname)]; ocls, _, oname = obf.rpartition("/")
            info = members(ocls); ok = info and any(d.startswith("public ") and re.search(r"\b" + re.escape(oname) + r";$", d) and "(" not in d for d, _ in info[1])
            if not ok: errs.append(f"{key}: champ public {ocls}.{oname} absent de client.jar"); continue
            lines.append(f"F {key} {srgname}")
            continue
        if (owner, srgname) not in md: errs.append(f"{key}: methode {owner}.{srgname} absente de joined.srg"); continue
        obf, odesc, ddesc = md[(owner, srgname)]
        n, r = parse_desc(ddesc)
        r = r.replace("$", "$")
        if ret == "GAMETYPE":
            if r not in ("net.minecraft.world.WorldSettings$GameType", "net.minecraft.world.GameType"): errs.append(f"{key}: retour inattendu {r}"); continue
        elif ret and r != ret: errs.append(f"{key}: retour {r} != {ret}"); continue
        if n != nparams: errs.append(f"{key}: {n} parametres != {nparams}"); continue
        ocls, _, oname = obf.rpartition("/")
        info = members(ocls)
        hit = info and [d for d, dd in info[1] if dd == odesc and re.search(r"\b" + re.escape(oname) + r"\(", d)]
        if not hit: errs.append(f"{key}: {ocls}.{oname}{odesc} absente de client.jar"); continue
        decl = hit[0]
        if not decl.startswith("public "): errs.append(f"{key}: non publique ({decl})"); continue
        if (" static " in " " + decl + " ") != static: errs.append(f"{key}: statique attendu={static} ({decl})"); continue
        lines.append(f"M {key} {srgname} {n} {r}")
    if errs:
        print(f"[{v}] ECHEC", *errs, sep="\n  "); return False
    os.makedirs(outdir, exist_ok=True)
    with open(os.path.join(outdir, v + ".map"), "w", encoding="utf-8", newline="\n") as f: f.write("\n".join(lines) + "\n")
    print(f"[{v}] ok ({len(lines)-1} lignes)"); return True

if __name__ == "__main__":
    data, out, vs = sys.argv[1], sys.argv[2], sys.argv[3:]
    sys.exit(0 if all([gen(data, out, v) for v in vs]) else 1)

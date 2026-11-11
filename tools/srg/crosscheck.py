"""Verifie un .map contre les donnees reelles de Forge : deobfuscation_data-<mc>.lzma du jar universel (ce que FML applique au runtime).
Usage : python -I crosscheck.py <forge-universal.jar> <mc> <fichier.map>   (le jar n'est qu'ouvert comme zip, jamais execute)"""
import sys, zipfile, lzma, re
jar, mc, mp = sys.argv[1:4]
z = zipfile.ZipFile(jar)
raw = lzma.LZMADecompressor(format=lzma.FORMAT_ALONE).decompress(z.read(f"deobfuscation_data-{mc}.lzma")).decode("utf-8", "replace")
md, fd, cl = {}, set(), set()
for l in raw.splitlines():
    p = l.split()
    if not p: continue
    if p[0] == "CL:": cl.add(p[2])
    elif p[0] == "MD:": md[p[3]] = p[4]
    elif p[0] == "FD:": fd.add(p[2])
cls = {}; bad = 0
for l in open(mp, encoding="utf-8"):
    p = l.split()
    if not p or p[0] == "#": continue
    if p[0] == "C":
        ok = p[2].replace(".", "/") in cl; cls[p[1]] = p[2].replace(".", "/")
    elif p[0] == "F":
        ok = any(x.endswith("/" + p[2]) and x.startswith("net/minecraft/client/Minecraft/") for x in fd)
    else:
        hits = [(k, v) for k, v in md.items() if k.endswith("/" + p[2]) and k.startswith(("net/minecraft/client/Minecraft/", "net/minecraft/server/integrated/IntegratedServer/"))]
        ok = bool(hits)
        if ok:
            k, v = hits[0]; n = len(re.findall(r"\[*(?:L[^;]+;|[ZBCSIJFD])", v[1:v.index(")")]))
            ret = v[v.index(")")+1:]
            ok = n == int(p[3])
            print("   ", p[1], p[2], v)
    print(("ok  " if ok else "KO  ") + l.strip()); bad += not ok
print("echecs:", bad); sys.exit(1 if bad else 0)

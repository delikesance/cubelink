"""Liste les membres candidats des classes utiles depuis joined.srg + methods.csv (MCP stable).
Usage : python -I explore.py <joined.srg> <dossier csv ou ''>"""
import sys, csv, os
srg, csvd = sys.argv[1], sys.argv[2]
names = {}
if csvd:
    for f in ("methods.csv", "fields.csv"):
        with open(os.path.join(csvd, f), encoding="utf-8", errors="replace") as fh:
            for r in csv.DictReader(fh):
                names[r["searge"]] = r["name"]
cls = {}; md = []; fd = []
for l in open(srg, encoding="utf-8"):
    p = l.split()
    if p[0] == "CL:": cls[p[2]] = p[1]
    elif p[0] == "MD:": md.append(p)
    elif p[0] == "FD:": fd.append(p)
TARGET = {"net/minecraft/client/Minecraft", "net/minecraft/server/integrated/IntegratedServer", "net/minecraft/server/MinecraftServer"}
WANT = ("getMinecraft", "getIntegratedServer", "addScheduledTask", "shareToLAN", "getPublic", "isLanServer", "getGameType", "getDefaultGameType", "isCallingFromMinecraftThread", "getSingleplayerServer", "isSinglePlayer", "isSingleplayer", "thePlayer", "player", "getServer")
for p in md:
    owner, _, sn = p[3].rpartition("/")
    if owner in TARGET and (names.get(sn) in WANT or sn in ("func_71410_x","func_71206_a","func_152343_a","func_152344_a")):
        print("M", owner.split("/")[-1], sn, names.get(sn), p[4])
for p in fd:
    owner, _, sn = p[2].rpartition("/")
    if owner in TARGET and (names.get(sn) in WANT or sn in ("field_71439_g",)):
        print("F", owner.split("/")[-1], sn, names.get(sn))

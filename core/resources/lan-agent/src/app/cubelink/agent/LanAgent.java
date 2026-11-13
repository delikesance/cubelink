package app.cubelink.agent;

import java.lang.instrument.Instrumentation;
import java.lang.reflect.Field;
import java.lang.reflect.InvocationTargetException;
import java.lang.reflect.Method;
import java.lang.reflect.Modifier;
import java.net.ServerSocket;
import java.nio.charset.StandardCharsets;
import java.nio.file.Files;
import java.nio.file.Paths;
import java.util.HashMap;
import java.util.Map;
import java.util.concurrent.Callable;

/**
 * Agent Java de Cubelink : ouvre automatiquement le monde solo au reseau local.
 *
 * Aucune injection de bytecode : un thread demon guette `Minecraft.getInstance().getSingleplayerServer()` par
 * reflexion, puis appelle `IntegratedServer.publishServer(GameType, false, portLibre)` sur le thread du serveur.
 * Minecraft journalise alors `Started serving on <port>`, que le launcher detecte pour ouvrir le tunnel P2P.
 *
 * Noms : par defaut les noms Mojang (NeoForge, Minecraft 26.x non obfusque). Pour les jeux obfusques (vanilla),
 * intermediary (Fabric, Quilt) ou autres, Cubelink fournit un fichier de correspondance `map=<fichier>`
 * (voir `core/src/lan_map.rs`) : lignes `C cle classe`, `M cle nom nbParams retour`, `F cle nom`.
 * Si le nom traduit est introuvable, le nom Mojang direct est essaye aussi.
 *
 * Regle d'or : l'agent ne doit JAMAIS empecher le jeu de demarrer ni le perturber. Tout est dans des try/catch
 * Throwable. Diagnostic : lignes `[Cubelink] LAN: ...` sur stdout (latest.log).
 *
 * Arguments : `probe` (verification a blanc sur stderr) et `map=<fichier>` (en dernier).
 */
public final class LanAgent {
    private static final long POLL_MS = 1000;
    /** Nombre de passages consecutifs avec monde charge avant de publier (laisse le joueur apparaitre). */
    private static final int SETTLE_POLLS = 3;

    // Cles (noms Mojang) : classe `cle` -> nom Mojang complet ; membre `cle` -> nom Mojang du membre.
    static final String K_MC = "minecraft";
    static final String K_INTEGRATED = "integrated";
    static final Map<String, String> MOJANG_CLASS = new HashMap<>();
    static final Map<String, String> MOJANG_MEMBER = new HashMap<>();

    static {
        MOJANG_CLASS.put(K_MC, "net.minecraft.client.Minecraft");
        MOJANG_CLASS.put(K_INTEGRATED, "net.minecraft.client.server.IntegratedServer");
        MOJANG_MEMBER.put("mc.getInstance", "getInstance");
        MOJANG_MEMBER.put("mc.getSingleplayerServer", "getSingleplayerServer");
        MOJANG_MEMBER.put("mc.player", "player");
        MOJANG_MEMBER.put("is.publishServer", "publishServer");
        MOJANG_MEMBER.put("is.isPublished", "isPublished");
        MOJANG_MEMBER.put("server.getDefaultGameType", "getDefaultGameType");
        MOJANG_MEMBER.put("loop.execute", "execute");
        // Branche legacy (Forge 1.7.10-1.12.2) : noms SRG fournis uniquement par le fichier map=.
        MOJANG_MEMBER.put("is.shareToLan", "shareToLAN");
        MOJANG_MEMBER.put("mc.schedule", "addScheduledTask");
    }

    /** Correspondance fournie par Cubelink (vide : noms Mojang). */
    static final Map<String, String> MAP_CLASS = new HashMap<>();
    /** cle -> {nom, nbParams, retour}. */
    static final Map<String, String[]> MAP_MEMBER = new HashMap<>();
    static String mapNamespace = "mojang";

    private LanAgent() {}

    /** Point d'entree factice pour les verifications (`java -javaagent:...=probe -cp ... app.cubelink.agent.LanAgent`). */
    public static void main(String[] args) throws Exception {
        // `wait` : laisse le temps au thread de l'agent de journaliser (tests avec une vraie JVM).
        if (args.length > 0 && "wait".equals(args[0])) Thread.sleep(1500);
    }

    /** TEST UNIQUEMENT (`noauth`, ajoute par Cubelink seulement si CUBELINK_TEST_MODE=1 et CUBELINK_LAN_NOAUTH=1) : coupe la verification de session du serveur LAN. */
    private static volatile boolean NOAUTH = false;

    public static void premain(String args, Instrumentation inst) {
        try {
            String mapFile = null;
            boolean probe = false;
            if (args != null) {
                NOAUTH = args.contains("noauth");
                int i = args.indexOf("map=");
                if (i >= 0) {
                    mapFile = args.substring(i + 4);
                    args = args.substring(0, i);
                }
                probe = args.contains("probe");
            }
            if (mapFile != null) {
                try {
                    loadMap(mapFile);
                } catch (Throwable t) {
                    log("fichier de correspondance illisible (" + mapFile + ") : " + cause(t) + " ; noms Mojang utilises");
                }
            }
            if (probe) {
                try {
                    System.err.println(probe(inst));
                } catch (Throwable t) {
                    System.err.println("cubelink-lan-agent probe : ECHEC " + cause(t) + " (noms " + mapNamespace + ")"
                        + " ; classpath incomplet (bibliotheques du jeu manquantes) ?");
                }
                return;
            }
            String join = null;
            if (args != null) {
                java.util.regex.Matcher jm = java.util.regex.Pattern.compile("join=([A-Za-z0-9.-]+:[0-9]{1,5})").matcher(args);
                if (jm.find()) join = jm.group(1);
            }
            final String target = join;
            if (target != null && isLegacy()) {
                log("join legacy : gere par --server/--port");
                return;
            }
            Thread t = new Thread(() -> {
                if (target != null) joinLoop(inst, target);
                else loop(inst);
            }, "cubelink-lan-agent");
            t.setDaemon(true);
            t.start();
        } catch (Throwable t) {
            // jamais d'impact sur le jeu, mais jamais silencieux non plus
            log("agent non demarre : " + cause(t));
        }
    }

    public static void agentmain(String args, Instrumentation inst) {
        premain(args, inst);
    }

    /** Mode legacy (Forge 1.7.10-1.12.2, noms SRG) : la table fournit `is.shareToLan` mais pas `is.publishServer`. */
    static boolean isLegacy() {
        return MAP_MEMBER.containsKey("is.shareToLan") && !MAP_MEMBER.containsKey("is.publishServer");
    }

    static void loadMap(String file) throws Exception {
        for (String line : Files.readAllLines(Paths.get(file), StandardCharsets.UTF_8)) {
            String[] p = line.trim().split(" ");
            if (p.length == 0 || p[0].isEmpty()) continue;
            if (p[0].equals("#") && p.length >= 3) mapNamespace = p[2];
            else if (p[0].equals("C") && p.length >= 3) MAP_CLASS.put(p[1], p[2]);
            else if (p[0].equals("M") && p.length >= 5) MAP_MEMBER.put(p[1], new String[] {p[2], p[3], p[4]});
            else if (p[0].equals("F") && p.length >= 3) MAP_MEMBER.put(p[1], new String[] {p[2], "-1", "-"});
        }
    }

    /** Journal de diagnostic : stdout du jeu, donc latest.log (`[Cubelink] LAN: ...`). Ne leve jamais. */
    static void log(String msg) {
        try {
            System.out.println("[Cubelink] LAN: " + msg);
            System.out.flush();
        } catch (Throwable ignored) {
            // rien
        }
    }

    static String cause(Throwable t) {
        Throwable c = t;
        while ((c instanceof InvocationTargetException || c instanceof java.util.concurrent.ExecutionException) && c.getCause() != null) {
            c = c.getCause();
        }
        return c.getClass().getName() + (c.getMessage() == null ? "" : " : " + c.getMessage());
    }

    static Class<?> find(Instrumentation inst, String name) {
        for (Class<?> c : inst.getAllLoadedClasses()) {
            if (c.getName().equals(name)) return c;
        }
        return null;
    }

    /** Classe `cle` : nom traduit d'abord, puis nom Mojang direct. */
    static Class<?> findClass(Instrumentation inst, String key) {
        String mapped = MAP_CLASS.get(key);
        if (mapped != null) {
            Class<?> c = find(inst, mapped);
            if (c != null) return c;
        }
        return find(inst, MOJANG_CLASS.get(key));
    }

    /** Candidats de nom pour un membre : traduit puis Mojang. */
    static String[][] memberNames(String key) {
        String[] mapped = MAP_MEMBER.get(key);
        String moj = MOJANG_MEMBER.get(key);
        if (mapped != null && !mapped[0].equals(moj)) return new String[][] {mapped, {moj, "-1", "-"}};
        return new String[][] {mapped != null ? mapped : new String[] {moj, "-1", "-"}};
    }

    /** Methode publique `cle` de `type` (nom, nbParams et type de retour verifies quand ils sont connus). */
    static Method findMethod(Class<?> type, String key, int nParams, Boolean wantStatic) {
        for (String[] n : memberNames(key)) {
            Method best = null;
            for (Method m : type.getMethods()) {
                if (!m.getName().equals(n[0]) || m.getParameterCount() != nParams) continue;
                if (wantStatic != null && Modifier.isStatic(m.getModifiers()) != wantStatic) continue;
                if (!"-".equals(n[2]) && n.length > 2 && !m.getReturnType().getName().equals(n[2])) {
                    if (best == null) best = m; // repli si le retour attendu ne correspond a rien
                    continue;
                }
                return m;
            }
            if (best != null) return best;
        }
        return null;
    }

    static Field findField(Class<?> type, String key) {
        for (String[] n : memberNames(key)) {
            try {
                return type.getField(n[0]);
            } catch (NoSuchFieldException ignored) {
                // suivant
            }
        }
        return null;
    }

    /** Java 9+ (modules) ? Sur Java 8, tout le code modules (classe Modules9) est ignore. */
    static final boolean JAVA9 = javaMajor() >= 9;

    static int javaMajor() {
        try {
            String v = System.getProperty("java.specification.version", "8");
            if (v.startsWith("1.")) v = v.substring(2);
            int dot = v.indexOf('.');
            return Integer.parseInt(dot > 0 ? v.substring(0, dot) : v);
        } catch (Throwable t) {
            return 8;
        }
    }

    /** Ouvre le paquet a l'agent si le module le protege (sans effet sinon, et sans effet sur Java 8). */
    static void open(Instrumentation inst, Class<?> c) {
        if (!JAVA9) return;
        try {
            Modules9.open(inst, c);
        } catch (Throwable ignored) {
            // tente quand meme
        }
    }

    static String moduleOf(Class<?> c) {
        if (!JAVA9) return "(Java 8, sans modules)";
        try {
            return Modules9.describe(c);
        } catch (Throwable t) {
            return "?";
        }
    }

    /** Delai (ms) a l'ecran-titre sans connexion avant de lancer nous-memes la connexion de l'invite. */
    static final long JOIN_GRACE_MS = 20_000;

    /**
     * Mode invite (`join=<hote>:<port>`) : le jeu est lance avec `--quickPlayMultiplayer`, mais ce quickPlay ne part pas
     * toujours (observe en reel). Si le jeu reste a l'ecran-titre JOIN_GRACE_MS sans qu'aucune connexion n'ait demarre,
     * on appelle `ConnectScreen.startConnecting` sur le thread de rendu. Ne remplace jamais un quickPlay qui fonctionne :
     * des qu'un ecran de connexion ou un monde est vu, on s'arrete. Noms Mojang (26.x) ; autre version : rien n'est fait.
     */
    private static void joinLoop(Instrumentation inst, String target) {
        log("connexion invite : agent charge, repli du quickPlay vers " + target + " apres " + (JOIN_GRACE_MS / 1000) + " s d'ecran-titre");
        Object mc = null;
        Method getInstance = null;
        Method screenM = null;
        Field screenF = null;
        Field levelF = null;
        long idleSince = 0;
        int polls = 0;
        while (true) {
            try {
                Thread.sleep(POLL_MS);
                polls++;
                if (getInstance == null) {
                    if (polls % 2 == 0) continue;
                    Class<?> mcClass = findClass(inst, K_MC);
                    if (mcClass == null) continue;
                    open(inst, mcClass);
                    getInstance = findMethod(mcClass, "mc.getInstance", 0, Boolean.TRUE);
                    if (getInstance == null) {
                        log("connexion invite : Minecraft.getInstance introuvable, repli non disponible sur cette version");
                        return;
                    }
                    try {
                        screenM = mcClass.getMethod("screen"); // 26.3+ : methode ; avant : champ
                    } catch (NoSuchMethodException e) {
                        try {
                            screenF = mcClass.getField("screen");
                        } catch (NoSuchFieldException e2) {
                            // rien
                        }
                    }
                    try {
                        levelF = mcClass.getField("level");
                    } catch (NoSuchFieldException e) {
                        // rien
                    }
                    if (screenM == null && screenF == null) {
                        log("connexion invite : acces a l'ecran courant introuvable, repli non disponible sur cette version");
                        return;
                    }
                }
                mc = getInstance.invoke(null);
                if (mc == null) continue;
                if (levelF != null && levelF.get(mc) != null) {
                    log("connexion invite : monde deja charge, rien a faire");
                    return;
                }
                Object screen = screenM != null ? screenM.invoke(mc) : screenF.get(mc);
                String sn = screen == null ? "" : screen.getClass().getName();
                if (sn.contains("ConnectScreen") || sn.contains("DisconnectedScreen") || sn.contains("JoinMultiplayerScreen")) {
                    log("connexion invite : quickPlay en cours (ecran " + sn + "), rien a faire");
                    return;
                }
                boolean title = false;
                for (Class<?> k = screen == null ? null : screen.getClass(); k != null; k = k.getSuperclass()) {
                    if (k.getName().equals("net.minecraft.client.gui.screens.TitleScreen")) title = true;
                }
                if (!title) {
                    idleSince = 0;
                    continue;
                }
                long now = System.currentTimeMillis();
                if (idleSince == 0) {
                    idleSince = now;
                    log("connexion invite : ecran-titre atteint, attente du quickPlay");
                }
                if (now - idleSince < JOIN_GRACE_MS) continue;
                log("connexion invite : quickPlay non parti apres " + (JOIN_GRACE_MS / 1000) + " s, connexion lancee par l'agent vers " + target);
                startConnecting(inst, mc, screen, target);
                return;
            } catch (InterruptedException e) {
                return;
            } catch (Throwable t) {
                log("connexion invite : echec " + cause(t));
                return;
            }
        }
    }

    /** `ConnectScreen.startConnecting(parent, mc, ServerAddress, ServerData, quickPlay, null)` sur le thread de rendu (26.x). */
    private static void startConnecting(Instrumentation inst, Object mc, Object parent, String target) throws Exception {
        ClassLoader cl = mc.getClass().getClassLoader();
        Class<?> connect = Class.forName("net.minecraft.client.gui.screens.ConnectScreen", true, cl);
        Class<?> addrC = Class.forName("net.minecraft.client.multiplayer.resolver.ServerAddress", true, cl);
        Class<?> dataC = Class.forName("net.minecraft.client.multiplayer.ServerData", true, cl);
        Class<?> typeC = Class.forName("net.minecraft.client.multiplayer.ServerData$Type", true, cl);
        for (Class<?> c : new Class<?>[] {connect, addrC, dataC}) open(inst, c);
        Object other = null;
        for (Object c : typeC.getEnumConstants()) {
            if (((Enum<?>) c).name().equals("OTHER")) other = c;
        }
        if (other == null) throw new IllegalStateException("ServerData.Type.OTHER introuvable");
        final Object addr = addrC.getMethod("parseString", String.class).invoke(null, target);
        final Object data = dataC.getConstructor(String.class, String.class, typeC).newInstance("Cubelink", target, other);
        Method start = null;
        for (Method m : connect.getMethods()) {
            if (m.getName().equals("startConnecting") && m.getParameterCount() == 6) start = m;
        }
        if (start == null) throw new IllegalStateException("ConnectScreen.startConnecting a 6 parametres introuvable");
        final Method st = start;
        Method exec = findMethod(mc.getClass(), "loop.execute", 1, Boolean.FALSE);
        if (exec == null) throw new IllegalStateException("Minecraft.execute introuvable");
        open(inst, exec.getDeclaringClass());
        Runnable job = () -> {
            try {
                st.invoke(null, parent, mc, addr, data, true, null);
                log("connexion invite : startConnecting appele");
            } catch (Throwable t) {
                log("connexion invite : echec de startConnecting : " + cause(t));
            }
        };
        exec.invoke(mc, job);
    }

    private static void loop(Instrumentation inst) {
        log("agent charge (java " + System.getProperty("java.version") + ", noms " + mapNamespace + "), attente de Minecraft");
        Object lastServer = null;
        int settled = 0;
        int polls = 0;
        boolean warnedMissing = false;
        boolean loggedClass = false;
        boolean loggedServer = false;
        Class<?> mcClass = null;
        Method getInstance = null;
        Method getServer = null;
        Field player = null;
        while (true) {
            try {
                Thread.sleep(POLL_MS);
                polls++;
                if (mcClass == null) {
                    if (polls % 2 == 0) continue; // getAllLoadedClasses est couteux : un passage sur deux
                    mcClass = findClass(inst, K_MC);
                    if (mcClass == null) {
                        if (!warnedMissing && polls >= 120) {
                            warnedMissing = true;
                            log("classe Minecraft (" + MAP_CLASS.getOrDefault(K_MC, MOJANG_CLASS.get(K_MC))
                                + ") toujours introuvable apres " + polls + " s : noms non geres par cette version/ce loader, partage a ouvrir a la main");
                        }
                        continue;
                    }
                    log("classe Minecraft trouvee : " + mcClass.getName() + " (module " + moduleOf(mcClass) + ", chargeur " + mcClass.getClassLoader() + ")");
                    loggedClass = true;
                    open(inst, mcClass);
                    getInstance = findMethod(mcClass, "mc.getInstance", 0, Boolean.TRUE);
                    getServer = findMethod(mcClass, "mc.getSingleplayerServer", 0, Boolean.FALSE);
                    player = findField(mcClass, "mc.player");
                    if (getInstance == null || getServer == null || player == null) {
                        log("membres de Minecraft introuvables (getInstance=" + (getInstance != null) + ", getSingleplayerServer="
                            + (getServer != null) + ", player=" + (player != null) + ") : noms non geres, partage a ouvrir a la main");
                        return;
                    }
                }
                Object mc = getInstance.invoke(null);
                if (mc == null) continue;
                Object server = getServer.invoke(mc);
                if (server == null) {
                    if (loggedServer) log("monde solo ferme");
                    loggedServer = false;
                    lastServer = null;
                    settled = 0;
                    continue;
                }
                if (!loggedServer) {
                    loggedServer = true;
                    log("serveur solo trouve (" + server.getClass().getName() + "), attente du joueur");
                }
                if (server == lastServer) continue; // deja traite (publie ou impossible)
                if (player.get(mc) == null) {
                    settled = 0;
                    continue;
                }
                if (++settled < SETTLE_POLLS) continue;
                lastServer = server;
                settled = 0;
                publishLater(inst, mc, server);
            } catch (InterruptedException e) {
                return;
            } catch (Throwable t) {
                log((loggedClass ? "echec, nouvelle tentative : " : "echec avant d'avoir trouve Minecraft : ") + cause(t));
                try {
                    Thread.sleep(5000);
                } catch (InterruptedException e) {
                    return;
                }
            }
        }
    }

    /**
     * Branche legacy (Forge 1.7.10-1.12.2) : `IntegratedServer.shareToLAN(GameType, boolean)` renvoie le port (String).
     * Appel sur le thread client via le planificateur de Minecraft (addScheduledTask / func_152343_a), sinon direct.
     */
    private static void publishLegacy(Instrumentation inst, Object mc, Object server) throws Exception {
        Class<?> sc = server.getClass();
        final Method share = findMethod(sc, "is.shareToLan", 2, Boolean.FALSE);
        final Method isPublished = findMethod(sc, "is.isPublished", 0, Boolean.FALSE);
        Method getGameType = findMethod(sc, "server.getDefaultGameType", 0, Boolean.FALSE);
        if (share == null || getGameType == null) {
            log("methodes legacy introuvables (shareToLAN=" + (share != null) + ", getGameType=" + (getGameType != null)
                + ") : version non geree, partage a ouvrir a la main");
            return;
        }
        open(inst, share.getDeclaringClass());
        open(inst, getGameType.getDeclaringClass());
        log("signature de shareToLAN : " + share.toGenericString());
        if (isPublished != null && Boolean.TRUE.equals(isPublished.invoke(server))) {
            log("monde deja ouvert au reseau local, rien a faire");
            return;
        }
        final Method sched = findScheduler(mc.getClass());
        final boolean callable = sched != null && sched.getParameterTypes()[0] == Callable.class;
        final Object gameType = getGameType.invoke(server);
        log("ouverture du monde au reseau local (legacy, planificateur " + (sched == null ? "aucun, appel direct" : sched.getName()) + ")");
        final Runnable job = () -> {
            try {
                if (isPublished != null && Boolean.TRUE.equals(isPublished.invoke(server))) return;
                Object r = share.invoke(server, gameType, false);
                String port = r == null ? "" : r.toString().trim();
                if (port.matches("[0-9]{1,5}")) log("Published LAN server on port " + Integer.parseInt(port));
                else log("shareToLAN n'a pas renvoye de port (" + r + ") : partage non confirme");
            } catch (Throwable t) {
                log("echec de shareToLAN : " + cause(t));
            }
        };
        if (sched == null) {
            job.run();
        } else {
            open(inst, sched.getDeclaringClass());
            Object arg;
            if (callable) {
                arg = (Callable<Object>) () -> {
                    job.run();
                    return null;
                };
            } else {
                arg = job;
            }
            sched.invoke(mc, arg);
        }
    }

    /** Planificateur du thread client : methode `mc.schedule` a 1 parametre Runnable (1.8+) ou Callable (1.7.10). */
    static Method findScheduler(Class<?> mcClass) {
        for (String[] n : memberNames("mc.schedule")) {
            Method callableM = null;
            for (Method m : mcClass.getMethods()) {
                if (!m.getName().equals(n[0]) || m.getParameterCount() != 1 || Modifier.isStatic(m.getModifiers())) continue;
                Class<?> p = m.getParameterTypes()[0];
                if (p == Runnable.class) return m;
                if (p == Callable.class) callableM = m;
            }
            if (callableM != null) return callableM;
        }
        return null;
    }

    private static void publishLater(Instrumentation inst, Object mc, Object server) throws Exception {
        Class<?> sc = server.getClass();
        open(inst, sc);
        if (isLegacy()) {
            publishLegacy(inst, mc, server);
            return;
        }
        Method isPublished = findMethod(sc, "is.isPublished", 0, Boolean.FALSE);
        Method getGameType = findMethod(sc, "server.getDefaultGameType", 0, Boolean.FALSE);
        Method publish = findMethod(sc, "is.publishServer", 3, Boolean.FALSE);
        Method exec = findMethod(sc, "loop.execute", 1, Boolean.FALSE);
        if (isPublished == null || publish == null || exec == null) {
            log("methodes introuvables (isPublished=" + (isPublished != null) + ", publishServer=" + (publish != null)
                + ", execute=" + (exec != null) + ") : version non geree, partage a ouvrir a la main");
            return;
        }
        for (Method m : new Method[] {isPublished, publish, exec}) open(inst, m.getDeclaringClass());
        log("signature de publishServer : " + publish.toGenericString());
        if (Boolean.TRUE.equals(isPublished.invoke(server))) {
            log("monde deja ouvert au reseau local, rien a faire");
            return;
        }
        // Premier parametre : GameType (jusqu'a 1.21.x / 26.1) ou enum MultiplayerScope (26.3+, on passe LAN).
        Class<?> first = publish.getParameterTypes()[0];
        // GameType est AUSSI une enum : seule une enum avec LAN est MultiplayerScope ; sinon, mode de jeu du monde.
        Object firstArg = null;
        if (first.isEnum()) {
            for (Object c : first.getEnumConstants()) {
                if (((Enum<?>) c).name().equals("LAN")) firstArg = c;
            }
        }
        if (firstArg == null) {
            if (getGameType == null) {
                log("getDefaultGameType introuvable : version non geree, partage a ouvrir a la main");
                return;
            }
            if (!first.isAssignableFrom(getGameType.getReturnType())) {
                log("getDefaultGameType renvoie " + getGameType.getReturnType().getName() + ", publishServer attend "
                    + first.getName() + " : version non geree, partage a ouvrir a la main");
                return;
            }
            open(inst, getGameType.getDeclaringClass());
            firstArg = getGameType.invoke(server);
        }
        int port;
        try (ServerSocket s = new ServerSocket(0)) {
            port = s.getLocalPort();
        }
        final int chosen = port;
        final Object firstFinal = firstArg;
        log("ouverture du monde au reseau local sur le port " + chosen);
        Runnable job = () -> {
            try {
                if (Boolean.TRUE.equals(isPublished.invoke(server))) return;
                Object ok = publish.invoke(server, firstFinal, false, chosen);
                log("publishServer a repondu " + ok + " (port " + chosen + ")");
                if (NOAUTH) {
                    for (Class<?> c = server.getClass(); c != null; c = c.getSuperclass()) {
                        for (Method m : c.getDeclaredMethods()) {
                            if (m.getName().equals("setUsesAuthentication") && m.getParameterCount() == 1 && m.getParameterTypes()[0] == boolean.class) {
                                open(inst, c);
                                m.setAccessible(true);
                                m.invoke(server, false);
                                log("TEST authentification serveur DESACTIVEE");
                                return;
                            }
                        }
                    }
                    log("TEST noauth demande mais setUsesAuthentication introuvable");
                }
            } catch (Throwable t) {
                log("echec de publishServer : " + cause(t));
            }
        };
        // Sur le thread du serveur (BlockableEventLoop.execute).
        exec.invoke(server, job);
    }

    /** Verification a blanc : resout classes, methodes et champ sur le classpath, sans lancer le jeu. */
    public static String probe(Instrumentation inst) {
        StringBuilder sb = new StringBuilder("cubelink-lan-agent probe, java ").append(System.getProperty("java.version"))
            .append(", noms ").append(mapNamespace).append('\n');
        int missing = 0;
        Class<?> mc = probeClass(inst, K_MC, sb);
        Class<?> is = probeClass(inst, K_INTEGRATED, sb);
        if (mc == null || is == null) missing++;
        if (mc != null) {
            missing += probeMember(sb, "mc.getInstance", findMethod(mc, "mc.getInstance", 0, Boolean.TRUE));
            missing += probeMember(sb, "mc.getSingleplayerServer", findMethod(mc, "mc.getSingleplayerServer", 0, Boolean.FALSE));
            Field f = findField(mc, "mc.player");
            sb.append(f == null ? "  MANQUE mc.player\n" : "  OK mc.player : " + f + "\n");
            if (f == null) missing++;
        }
        if (is != null && isLegacy()) {
            missing += probeMember(sb, "is.shareToLan", findMethod(is, "is.shareToLan", 2, Boolean.FALSE));
            missing += probeMember(sb, "is.isPublished", findMethod(is, "is.isPublished", 0, Boolean.FALSE));
            missing += probeMember(sb, "server.getDefaultGameType", findMethod(is, "server.getDefaultGameType", 0, Boolean.FALSE));
            missing += probeMember(sb, "mc.schedule", mc == null ? null : findScheduler(mc));
        } else if (is != null) {
            missing += probeMember(sb, "is.publishServer", findMethod(is, "is.publishServer", 3, Boolean.FALSE));
            missing += probeMember(sb, "is.isPublished", findMethod(is, "is.isPublished", 0, Boolean.FALSE));
            missing += probeMember(sb, "server.getDefaultGameType", findMethod(is, "server.getDefaultGameType", 0, Boolean.FALSE));
            missing += probeMember(sb, "loop.execute", findMethod(is, "loop.execute", 1, Boolean.FALSE));
        }
        sb.append(missing == 0 ? "RESULTAT : TOUT SE RESOUT\n" : "RESULTAT : " + missing + " ELEMENT(S) MANQUANT(S)\n");
        return sb.toString();
    }

    private static Class<?> probeClass(Instrumentation inst, String key, StringBuilder sb) {
        String[] names = {MAP_CLASS.get(key), MOJANG_CLASS.get(key)};
        for (String n : names) {
            if (n == null) continue;
            Class<?> c = find(inst, n);
            if (c == null) {
                try {
                    c = Class.forName(n, false, ClassLoader.getSystemClassLoader());
                } catch (Throwable ignored) {
                    // suivant
                }
            }
            if (c != null) {
                sb.append("OK classe ").append(key).append(" = ").append(c.getName()).append('\n');
                return c;
            }
        }
        sb.append("ABSENT classe ").append(key).append(" (essaye : ").append(names[0]).append(", ").append(names[1]).append(")\n");
        return null;
    }

    private static int probeMember(StringBuilder sb, String key, Method m) {
        sb.append(m == null ? "  MANQUE " : "  OK ").append(key).append(m == null ? "" : " : " + m.toGenericString()).append('\n');
        return m == null ? 1 : 0;
    }
}

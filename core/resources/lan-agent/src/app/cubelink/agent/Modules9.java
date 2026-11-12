package app.cubelink.agent;

import java.lang.instrument.Instrumentation;
import java.util.Map;
import java.util.Set;

/**
 * Tout le code Java 9+ (modules) de l'agent. Compile en classfile 53 ; n'est JAMAIS charge sur Java 8
 * (LanAgent ne l'appelle que si java.specification.version >= 9).
 */
final class Modules9 {
    private Modules9() {}

    /** Ouvre le paquet de `c` a l'agent si son module le protege (sans effet sinon). */
    static void open(Instrumentation inst, Class<?> c) {
        Module m = c.getModule();
        if (!m.isNamed()) return;
        Module me = Modules9.class.getModule();
        String pkg = c.getPackageName();
        if (m.isOpen(pkg, me)) return;
        if (inst.isModifiableModule(m)) {
            inst.redefineModule(m, Set.of(), Map.of(), Map.of(pkg, Set.of(me)), Set.of(), Map.of());
        }
    }

    static String describe(Class<?> c) {
        return String.valueOf(c.getModule());
    }
}

package net.minecraft.client;

import java.util.concurrent.Callable;

/** Stub Forge 1.7.10 (noms SRG) : pas d'addScheduledTask, seulement func_152343_a(Callable). */
public class Minecraft {
  private static final Minecraft I = new Minecraft();
  public Object field_71439_g; // player
  private net.minecraft.server.integrated.IntegratedServer srv;
  public static Minecraft func_71410_x() { return I; }
  public net.minecraft.server.integrated.IntegratedServer func_71401_C() { return srv; }
  public Object func_152343_a(Callable c) throws Exception {
    System.out.println("[Client thread] scheduled (Callable)"); return c.call(); }
  public static void main(String[] a) throws Exception {
    System.out.println("main: java " + System.getProperty("java.version") + ", menu principal (pas de monde)"); Thread.sleep(2500);
    I.srv = new net.minecraft.server.integrated.IntegratedServer(); System.out.println("main: serveur solo demarre, joueur absent"); Thread.sleep(1500);
    I.field_71439_g = new Object(); System.out.println("main: joueur dans le monde"); Thread.sleep(6000);
    System.out.println("main: fin, published=" + I.srv.func_71344_c());
  }
}

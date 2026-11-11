package net.minecraft.client;
public class Minecraft {
  private static final Minecraft I = new Minecraft();
  public Object player;
  private net.minecraft.client.server.IntegratedServer srv;
  public static Minecraft getInstance() { return I; }
  public net.minecraft.client.server.IntegratedServer getSingleplayerServer() { return srv; }
  public static void main(String[] a) throws Exception {
    System.out.println("main: menu principal (pas de monde)"); Thread.sleep(2500);
    I.srv = new net.minecraft.client.server.IntegratedServer(); System.out.println("main: serveur solo demarre, joueur absent"); Thread.sleep(1500);
    I.player = new Object(); System.out.println("main: joueur dans le monde"); Thread.sleep(6000);
    System.out.println("main: fin, published=" + I.srv.isPublished());
  }
}

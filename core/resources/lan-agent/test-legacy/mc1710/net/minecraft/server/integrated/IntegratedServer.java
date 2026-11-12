package net.minecraft.server.integrated;

public class IntegratedServer extends net.minecraft.server.MinecraftServer {
  private boolean published;
  public boolean func_71344_c() { return published; } // isPublished
  public String func_71206_a(net.minecraft.world.GameType t, boolean cheats) { // shareToLAN : renvoie le port
    published = true; return "54321"; }
}

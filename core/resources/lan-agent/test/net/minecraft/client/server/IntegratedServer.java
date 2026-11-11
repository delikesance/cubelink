package net.minecraft.client.server;
public class IntegratedServer extends net.minecraft.server.MinecraftServer {
  private boolean published;
  public boolean isPublished() { return published; }
  public boolean publishServer(net.minecraft.world.level.GameType t, boolean cheats, int port) {
    published = true; System.out.println("[Server thread/INFO] [minecraft/IntegratedServer]: Started serving on " + port); return true; }
}

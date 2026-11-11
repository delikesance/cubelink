package net.minecraft.server;
public class MinecraftServer extends net.minecraft.util.thread.BlockableEventLoop {
  public net.minecraft.world.level.GameType getDefaultGameType() { return net.minecraft.world.level.GameType.SURVIVAL; }
}

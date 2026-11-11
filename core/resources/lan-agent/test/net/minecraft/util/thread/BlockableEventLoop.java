package net.minecraft.util.thread;
public class BlockableEventLoop { public void execute(Runnable r) { System.out.println("[Server thread] execute"); r.run(); } }

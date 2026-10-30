package net.solarspire.ferrum.runtime;

import java.util.List;
import java.util.Set;
import java.util.concurrent.CopyOnWriteArrayList;
import net.neoforged.neoforgespi.transformation.ClassProcessor;
import net.neoforged.neoforgespi.transformation.ClassProcessorIds;
import net.neoforged.neoforgespi.transformation.ProcessorName;
import org.objectweb.asm.Opcodes;
import org.objectweb.asm.tree.InsnList;
import org.objectweb.asm.tree.LdcInsnNode;
import org.objectweb.asm.tree.MethodInsnNode;

/** Prevents mods from starting a second world authority during headless loading. */
public final class HeadlessGuard implements ClassProcessor {
    private static final Set<String> SERVER_METHODS = Set.of("<init>", "spin", "runServer", "tickServer", "saveAllChunks");
    private static final Set<String> LEVEL_METHODS = Set.of("<init>", "tick", "save");
    private static final List<String> BLOCKED = new CopyOnWriteArrayList<>();
    private static final Set<String> INSTALLED = java.util.concurrent.ConcurrentHashMap.newKeySet();

    public static void reject(String operation) {
        BLOCKED.add(operation);
        throw new IllegalStateException("Unavailable in FerrumMC headless bridge mode: " + operation);
    }

    public static List<String> blockedOperations() {
        return List.copyOf(BLOCKED);
    }

    public static List<String> installedGuards() {
        return INSTALLED.stream().sorted().toList();
    }

    @Override
    public ProcessorName name() {
        return new ProcessorName("ferrum", "headless_guard");
    }

    @Override
    public Set<ProcessorName> runsAfter() {
        return Set.of(ClassProcessorIds.MIXIN, ClassProcessorIds.COMPUTING_FRAMES);
    }

    @Override
    public boolean handlesClass(SelectionContext context) {
        String name = context.type().getClassName();
        return !context.empty() && (name.equals("net.minecraft.server.MinecraftServer") || name.equals("net.minecraft.server.level.ServerLevel"));
    }

    @Override
    public ComputeFlags processClass(TransformationContext context) {
        var methods = context.type().getClassName().equals("net.minecraft.server.MinecraftServer") ? SERVER_METHODS : LEVEL_METHODS;
        for (var method : context.node().methods) {
            if (!methods.contains(method.name)) continue;
            String operation = context.type().getClassName() + "." + method.name + method.desc;
            var instructions = new InsnList();
            instructions.add(new LdcInsnNode(operation));
            instructions.add(new MethodInsnNode(Opcodes.INVOKESTATIC,
                "net/solarspire/ferrum/runtime/HeadlessGuard", "reject", "(Ljava/lang/String;)V", false));
            method.instructions.insert(instructions);
            INSTALLED.add(operation);
            context.audit("deny", method.name, method.desc);
        }
        return ComputeFlags.COMPUTE_FRAMES;
    }
}

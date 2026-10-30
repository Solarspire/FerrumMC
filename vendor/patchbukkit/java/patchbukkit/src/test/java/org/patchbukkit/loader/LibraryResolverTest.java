package org.patchbukkit.loader;

import org.junit.jupiter.api.Test;
import org.junit.jupiter.api.io.TempDir;

import java.io.File;
import java.io.FileOutputStream;
import java.nio.charset.StandardCharsets;
import java.util.List;
import java.util.jar.JarEntry;
import java.util.jar.JarOutputStream;

import static org.junit.jupiter.api.Assertions.*;

public class LibraryResolverTest {

    @Test
    public void testLibraryResolverHandlesCustomReposAndCoords(@TempDir File tempDir) {
        String input = "repo:https://repo.miraculixx.de/snapshots\nde.miraculixx:kpaper:1.1.2";
        List<File> resolved = LibraryResolver.resolveLibraries(input, tempDir);
        assertNotNull(resolved);
    }

    @Test
    public void testNestedJarAndPaperLibrariesExtraction(@TempDir File tempDir) throws Exception {
        File dummyJar = new File(tempDir, "test-plugin.jar");
        try (ZipArchiveOutputStreamOrFileOutputStream fos = new ZipArchiveOutputStreamOrFileOutputStream(dummyJar);
             JarOutputStream jos = new JarOutputStream(fos)) {
            
            // Add paper-plugin.yml
            jos.putNextEntry(new JarEntry("paper-plugin.yml"));
            String paperYml = "name: TestPlugin\nversion: 1.0.0\nmain: org.test.TestMain\nlibraries:\n  - \"repo:https://repo.miraculixx.de\"\n  - \"de.miraculixx:kpaper:1.1.2\"\n";
            jos.write(paperYml.getBytes(StandardCharsets.UTF_8));
            jos.closeEntry();

            // Add paper-libraries.json
            jos.putNextEntry(new JarEntry("paper-libraries.json"));
            String paperJson = "{\n  \"repositories\": {\n    \"miraculixx\": \"https://repo.miraculixx.de\"\n  },\n  \"dependencies\": [\n    \"de.miraculixx:kpaper:1.1.2\"\n  ]\n}";
            jos.write(paperJson.getBytes(StandardCharsets.UTF_8));
            jos.closeEntry();

            // Add nested jar
            jos.putNextEntry(new JarEntry("META-INF/jars/nested-lib.jar"));
            jos.write(new byte[]{0x50, 0x4B, 0x03, 0x04}); // PK header
            jos.closeEntry();
        }

        PatchBukkitPluginClassLoader loader = new PatchBukkitPluginClassLoader(
            LibraryResolverTest.class.getClassLoader(),
            dummyJar
        );

        assertNotNull(loader.getDescription());
        assertEquals("TestPlugin", loader.getDescription().getName());
        loader.close();
    }

    private static class ZipArchiveOutputStreamOrFileOutputStream extends FileOutputStream {
        public ZipArchiveOutputStreamOrFileOutputStream(File file) throws Exception {
            super(file);
        }
    }
}

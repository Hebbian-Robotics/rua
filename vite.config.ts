import { resolve } from "node:path";
import tailwindcss from "@tailwindcss/vite";
import { loadEnv, normalizePath } from "vite";
import { defineConfig } from "vitest/config";

export default defineConfig(({ command, mode }) => {
  const backendEnvironment = loadEnv(mode, import.meta.dirname, "AGENT_");
  const viewerBackend = `http://${backendEnvironment["AGENT_HOST"] ?? "127.0.0.1"}:${backendEnvironment["AGENT_PORT"] ?? "8793"}`;
  const root = resolve(import.meta.dirname, "examples/typescript/viewer/web");
  const sourceDirectory = normalizePath(
    resolve(import.meta.dirname, "examples/typescript/viewer/src"),
  ).replace(/^\//, "");
  return {
    root,
    resolve: {
      conditions: ["development", "module", "browser"],
      alias: {
        "@": resolve(import.meta.dirname, "examples/typescript/viewer/src"),
      },
    },
    publicDir: resolve(
      import.meta.dirname,
      "examples/typescript/viewer/public",
    ),
    plugins: [
      tailwindcss(),
      ...(command === "serve"
        ? [
            {
              name: "viewer-source-entries",
              transformIndexHtml: {
                order: "pre",
                handler(html: string) {
                  return html.replaceAll(
                    'src="../src/',
                    `src="/@fs/${sourceDirectory}/`,
                  );
                },
              },
            },
          ]
        : []),
    ],
    base: command === "serve" ? "/viewer/" : "./",
    build: {
      outDir: resolve(import.meta.dirname, "dist/viewer"),
      emptyOutDir: true,
      target: "es2023",
      chunkSizeWarningLimit: 1500,
      rollupOptions: {
        // WHY: This app has no React server components, so all modules are client-side.
        onwarn(warning, warn) {
          if (
            warning.code === "MODULE_LEVEL_DIRECTIVE" &&
            warning.message.includes('"use client"')
          )
            return;
          warn(warning);
        },
        input: [resolve(root, "index.html"), resolve(root, "recording.html")],
      },
    },
    test: {
      root: import.meta.dirname,
      include: [
        "examples/typescript/viewer/tests/*.test.ts",
        "examples/typescript/viewer/server/tests/*.test.ts",
        "typescript-client/tests/*.test.ts",
        "examples/typescript/agent/tests/*.test.ts",
      ],
    },
    server: {
      proxy: {
        "/model": viewerBackend,
        "/api": viewerBackend,
      },
    },
  };
});

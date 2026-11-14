import assert from "node:assert/strict";
import { mkdir, mkdtemp, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { test } from "node:test";
import { createServer, mergeConfig, type ViteDevServer } from "vite";
import applicationConfig from "../vite.config.ts";

test(
  "development HTTP serving denies private workspace files even inside an allowed directory",
  { timeout: 30_000 },
  async (context) => {
    const root = await mkdtemp(join(tmpdir(), "apodect-dev-server-"));
    let server: ViteDevServer | undefined;
    context.after(async () => {
      await server?.close();
      await rm(root, { recursive: true, force: true });
    });

    const privateFiles = [
      ".localnet/wallets/agent.ethereum.json",
      ".secrets/postgres-app",
      ".agent-workspace/secrets/test-wallet.json",
      ".agent-workspace/checks/backups/test.json",
      "private-docs/demonstration.md",
      ".env",
      ".env.local",
      ".npmrc",
      ".yarnrc.yml",
      ".git/config",
      ...["crt", "pem", "key", "p12", "pfx", "cer", "der"].map(
        (extension) => `test.${extension}`,
      ),
    ];
    for (const file of [...privateFiles, "public/assets/public.txt"]) {
      const path = join(root, file);
      await mkdir(dirname(path), { recursive: true });
      await writeFile(path, "test fixture only; no real credentials\n");
    }

    server = await createServer(
      mergeConfig(applicationConfig, {
        configFile: false,
        root,
        cacheDir: join(root, ".vite-cache"),
        publicDir: join(root, "public"),
        logLevel: "silent",
        optimizeDeps: { noDiscovery: true, include: [] },
        server: {
          host: "127.0.0.1",
          port: 0,
          strictPort: true,
          fs: { allow: [root] },
        },
      }),
    );
    await server.listen();
    const address = server.httpServer?.address();
    assert.ok(address && typeof address === "object");
    const origin = `http://127.0.0.1:${address.port}`;

    for (const url of [
      "/assets/public.txt",
      `/@fs${join(root, "public/assets/public.txt")}`,
    ]) {
      const response = await fetch(new URL(url, origin), {
        method: "HEAD",
        signal: AbortSignal.timeout(3_000),
      });
      assert.equal(response.status, 200, "Public files must remain available");
    }

    for (const file of privateFiles) {
      for (const url of [`/${file}`, `/@fs${join(root, file)}`]) {
        for (const query of ["", "?raw", "?url"]) {
          const response = await fetch(new URL(`${url}${query}`, origin), {
            method: "HEAD",
            signal: AbortSignal.timeout(3_000),
          });
          assert.equal(
            response.status,
            403,
            `${file}${query} must be denied even when its parent directory is allowed`,
          );
        }
      }
    }
  },
);

import react from "@vitejs/plugin-react";
import { defineConfig } from "vite";

export default defineConfig({
  plugins: [react()],
  server: {
    fs: {
      deny: [
        ".env",
        ".env.*",
        "*.{crt,pem,key,p12,pfx,cer,der}",
        ".npmrc",
        ".yarnrc.yml",
        "**/.git/**",
        "**/.localnet/**",
        "**/.secrets/**",
        "**/.agent-workspace/**",
        "**/private-docs/**",
      ],
    },
    proxy: {
      "/api": {
        target: process.env.BACKEND_URL ?? "http://localhost:3001",
      },
    },
  },
});

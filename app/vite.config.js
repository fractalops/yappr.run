import { defineConfig } from "vite";
import { sveltekit } from "@sveltejs/kit/vite";
import process from "node:process";

export default defineConfig({
  plugins: [sveltekit()],
  define: {
    // The version of the WebAssembly reader that this page loads.
    __YAPPR_BUILD__: JSON.stringify(process.env.YAPPR_BUILD ?? String(Date.now())),
    // The path that the site lives under. It must match kit.paths.base.
    __YAPPR_BASE__: JSON.stringify(process.env.YAPPR_BASE ?? ""),
  },
});

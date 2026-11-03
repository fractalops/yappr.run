import { defineConfig } from "vite";
import { sveltekit } from "@sveltejs/kit/vite";
import adapter from "@sveltejs/adapter-static";
import { vitePreprocess } from "@sveltejs/vite-plugin-svelte";
import process from "node:process";

export default defineConfig({
  plugins: [
    // A static single-page site: the reader runs in the browser.
    sveltekit({
      preprocess: vitePreprocess(),
      // The path the site lives under, for example "/yappr.run" on a project
      // page. Empty for a site at the root of its domain.
      paths: { base: /** @type {"" | `/${string}`} */ (process.env.YAPPR_BASE ?? "") },
      adapter: adapter({ fallback: "index.html" }),
    }),
  ],
  define: {
    // The version of the WebAssembly reader that this page loads.
    __YAPPR_BUILD__: JSON.stringify(process.env.YAPPR_BUILD ?? String(Date.now())),
    // The path that the site lives under. It must match paths.base.
    __YAPPR_BASE__: JSON.stringify(process.env.YAPPR_BASE ?? ""),
  },
});

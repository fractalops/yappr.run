// A static single-page site: the reader runs in the browser.
import adapter from "@sveltejs/adapter-static";
import { vitePreprocess } from "@sveltejs/vite-plugin-svelte";

/** @type {import('@sveltejs/kit').Config} */
const config = {
  preprocess: vitePreprocess(),
  kit: {
    // The path the site lives under, for example "/yappr.run" on a project
    // page. Empty for a site at the root of its domain.
    paths: { base: process.env.YAPPR_BASE ?? "" },
    adapter: adapter({ fallback: "index.html" }),
  },
};

export default config;

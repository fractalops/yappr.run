import "svelte/elements";

// WebKit supports this attribute for exact text entry on iOS.
declare module "svelte/elements" {
  interface HTMLTextareaAttributes {
    autocorrect?: "on" | "off";
  }
}

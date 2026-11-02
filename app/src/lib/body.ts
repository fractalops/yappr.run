import MarkdownIt from "markdown-it";

// Bodies come from another machine. Raw HTML stays text, images do not load,
// and links open only http and https URLs.
const SAFE_LINK = /^https?:\/\//i;

const markdown = new MarkdownIt({ html: false, linkify: true, breaks: true });
markdown.disable("image");
// Only URLs with a scheme become links, not words such as "example.com".
markdown.linkify.set({ fuzzyLink: false, fuzzyEmail: false });
markdown.validateLink = (url) => SAFE_LINK.test(url.trim());
const defaultLinkOpen = markdown.renderer.rules.link_open
  ?? ((tokens, idx, options, _env, self) => self.renderToken(tokens, idx, options));
markdown.renderer.rules.link_open = (tokens, idx, options, env, self) => {
  tokens[idx].attrSet("target", "_blank");
  tokens[idx].attrSet("rel", "noopener noreferrer");
  return defaultLinkOpen(tokens, idx, options, env, self);
};

/** The HTML of a Markdown body. */
export function renderMarkdown(body: string): string {
  return markdown.render(body);
}

export type Segment = { text: string; href?: string };

/** A text body in parts, with the http and https URLs as links. */
export function linkSegments(body: string): Segment[] {
  const matches = markdown.linkify.match(body) ?? [];
  const segments: Segment[] = [];
  let at = 0;
  for (const match of matches) {
    if (!SAFE_LINK.test(match.url)) continue;
    if (match.index > at) segments.push({ text: body.slice(at, match.index) });
    segments.push({ text: match.raw, href: match.url });
    at = match.lastIndex;
  }
  if (at < body.length) segments.push({ text: body.slice(at) });
  return segments;
}

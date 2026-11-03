import assert from "node:assert/strict";
import { test } from "node:test";
import { linkSegments, renderMarkdown } from "../src/lib/body.ts";

test("Markdown keeps formatting and opens web links safely", () => {
  const html = renderMarkdown("**Ready**\n`cargo test`\n\n[Results](https://example.com/results)");
  assert.match(html, /<strong>Ready<\/strong><br>/);
  assert.match(html, /<code>cargo test<\/code>/);
  assert.match(html, /href="https:\/\/example.com\/results"/);
  assert.match(html, /target="_blank"/);
  assert.match(html, /rel="noopener noreferrer"/);
});

test("Markdown cannot load images, run HTML, or link to unsafe schemes", () => {
  const html = renderMarkdown([
    '<script>alert(1)</script><img src="https://example.com/tracker">',
    "![tracker](https://example.com/pixel)",
    "[script](javascript:alert(1))",
    "[data](data:text/html;base64,PHNjcmlwdD4=)",
    "[mail](mailto:someone@example.com)",
  ].join("\n"));
  assert.doesNotMatch(html, /<script|<img|href="(?:javascript:|data:|mailto:)/i);
  assert.match(html, /&lt;script&gt;/);
});

test("Plain text links require an explicit HTTP or HTTPS scheme", () => {
  const body = "example.com a@example.com javascript:alert(1) https://example.com/a http://example.org/b";
  const segments = linkSegments(body);
  assert.equal(segments.map((part) => part.text).join(""), body);
  assert.deepEqual(segments.filter((part) => part.href).map((part) => part.href), [
    "https://example.com/a", "http://example.org/b",
  ]);
});

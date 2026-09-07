/// Embed the exact project colour in a shipped provider glyph for both terminal tabs and sidebar rows.
export function accentGlyph(source: string, accent: string): string {
  if (!/^#[0-9a-f]{6}$/u.test(accent)) throw new Error("conversation accent must be a lowercase six-digit hex colour");
  let svg = source
    .replace(/<style\b[^>]*>[\s\S]*?<\/style>/gu, "")
    .replace(/\b(fill|stroke)="(?:currentColor|#[0-9a-fA-F]{3,8})"/gu, `$1="${accent}"`);
  if (!/<svg\b[^>]*\bfill=/u.test(svg)) svg = svg.replace(/<svg\b/u, `<svg fill="${accent}"`);
  return svg;
}

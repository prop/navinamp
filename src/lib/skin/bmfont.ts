/**
 * Bitmap glyph fonts (classic `.wsz` text rendering).
 *
 * Classic Winamp 2 skins draw text by blitting fixed cells out of two glyph
 * strips — no font files anywhere:
 *
 *  · `text.bmp` (155×74 in the base skin): 5×6 glyphs in a 31-column grid
 *    with NO cell padding (stride 5 on both axes). Only the first three rows
 *    (y=0/6/12) are the charmap; the rest of the strip repeats it on blue
 *    backgrounds (unused legacy variants). The font is single-case — Winamp
 *    lowercases all songticker/bitrate/khz text (there are no uppercase
 *    glyphs), which this module reproduces via `foldLower`.
 *  · `numbers.bmp` (99×13): 9×13 digit cells, stride 9 — `0123456789` plus
 *    one blank cell. There is no colon glyph: the classic time display's
 *    colon is baked into `main.bmp`, so the time renderer positions digits
 *    individually (see `params["digitoffsets"]` in the `.wsz` synthesizer).
 *
 * The charmaps below were derived empirically from the Winamp 2.91 base skin
 * (glyph strips decoded and identified cell by cell) and cross-checked
 * against Webamp's `FONT_LOOKUP` (packages/webamp/js/skinSprites.ts), which
 * agrees cell-for-cell.
 *
 * Glyph cells are blitted opaque (their background is black, matching the
 * classic display wells) exactly like Winamp and Webamp do.
 */

/** Geometry + charmap of one glyph strip. */
export interface BmFontSpec {
  /** Glyph cell size in pixels. */
  cellW: number;
  cellH: number;
  /** Pixels between cell origins (defaults to the cell size). */
  strideX?: number;
  strideY?: number;
  /** Cells per grid row. */
  cols: number;
  /**
   * Chars in row-major cell order; `"\u0000"` marks an empty cell (unused
   * cells are skipped when mapping chars, so their positions don't matter).
   */
  charmap: string;
  /** Chars rendered with another char's cell (e.g. `<` shares `[`). */
  aliases?: Record<string, string>;
  /** Map uppercase input to lowercase glyphs (the classic single-case font). */
  foldLower?: boolean;
  /** Pixels the pen advances per char (defaults to `cellW`). */
  advance?: number;
}

/** One glyph cell's source rectangle inside the strip. */
export interface GlyphCell {
  x: number;
  y: number;
  w: number;
  h: number;
}

/** Empty charmap cells are ignored when mapping characters. */
const EMPTY = "\u0000";

/** Classic `text.bmp`: 31 cells of 5×6, rows y=0/6/12 (see module docs). */
export const CLASSIC_TEXT_SPEC: BmFontSpec = {
  cellW: 5,
  cellH: 6,
  strideX: 5,
  strideY: 6,
  cols: 31,
  // Row 0: a–z, `"`, `@`, two empty cells, space.
  // Row 1: digits, `…` (U+2026), then punctuation.
  // Row 2: Å Ö Ä ? * (the only localized glyphs the classic font has).
  // `<` `>` `{` `}` share the bracket cells (Webamp does the same).
  charmap:
    'abcdefghijklmnopqrstuvwxyz"@' +
    EMPTY +
    EMPTY +
    " " +
    "0123456789\u2026.,():-'!_+\\/[]^&%,$#" +
    "\u00c5\u00d6\u00c4?*" +
    EMPTY.repeat(26),
  aliases: { "<": "[", ">": "]", "{": "[", "}": "]" },
  foldLower: true,
  advance: 5,
};

/** Classic `numbers.bmp`: 11 cells of 9×13 — `0123456789` + blank. */
export const CLASSIC_NUMBERS_SPEC: BmFontSpec = {
  cellW: 9,
  cellH: 13,
  strideX: 9,
  strideY: 13,
  cols: 11,
  charmap: "0123456789 ",
  advance: 9,
};

/**
 * Charset names the `.wsz` synthesizer puts in `params["charset"]` on text
 * objects. Unknown names → `null` (the caller falls back to HTML text).
 */
export function specForCharset(charset: string | undefined): BmFontSpec | null {
  switch (charset) {
    case "classic-text":
      return CLASSIC_TEXT_SPEC;
    case "classic-numbers":
      return CLASSIC_NUMBERS_SPEC;
    default:
      return null;
  }
}

/** Decode a data URL into an image the canvas can blit from. */
async function decodeImage(src: string): Promise<ImageBitmap | HTMLImageElement> {
  const img = new Image();
  img.src = src;
  await img.decode();
  return img;
}

/**
 * A decoded glyph strip. Instances are cheap to hold (one decoded image +
 * a spec); use [`loadBmFont`] to get the per-URL cached shared instances.
 */
export class BmFont {
  readonly spec: BmFontSpec;
  private img: ImageBitmap | HTMLImageElement;

  constructor(img: ImageBitmap | HTMLImageElement, spec: BmFontSpec) {
    this.img = img;
    this.spec = spec;
  }

  /**
   * Decode `src` and build a font for `spec` (rejects on undecodable images).
   * Prefer the cached [`loadBmFont`] when several objects share a strip.
   */
  static async load(src: string, spec: BmFontSpec): Promise<BmFont> {
    return new BmFont(await decodeImage(src), spec);
  }

  /** The char's cell in the strip, resolved through aliases; null = blank. */
  cell(ch: string): GlyphCell | null {
    let c = ch;
    const { aliases, foldLower } = this.spec;
    if (foldLower) c = c.toLowerCase();
    c = aliases?.[c] ?? c;
    const index = this.spec.charmap.indexOf(c);
    if (index === -1 || c === EMPTY) return null;
    const col = index % this.spec.cols;
    const row = Math.floor(index / this.spec.cols);
    return {
      x: col * (this.spec.strideX ?? this.spec.cellW),
      y: row * (this.spec.strideY ?? this.spec.cellH),
      w: this.spec.cellW,
      h: this.spec.cellH,
    };
  }

  /** Rendered width of `text` in pixels (blank cells still advance). */
  measure(text: string): number {
    const advance = this.spec.advance ?? this.spec.cellW;
    return [...text].length * advance;
  }

  /**
   * `text` as one tight canvas (`measure(text)` × `cellH`), glyphs blitted
   * opaque at 1:1 — crisp, pixelated, exactly like the classic engine.
   * Returns a zero-width canvas for empty text.
   */
  renderText(text: string): HTMLCanvasElement {
    const h = this.spec.cellH;
    const canvas = document.createElement("canvas");
    canvas.width = Math.max(0, this.measure(text));
    canvas.height = h;
    this.drawText(canvas.getContext("2d"), text, 0, 0);
    return canvas;
  }

  /**
   * Blit `text` into an existing context at `x,y` (glyph tops at `y` — the
   * classic displays are all top-aligned within their rects). Used by
   * renderText and by renderers that place glyphs individually (the time
   * display's fixed digit cells).
   */
  drawText(ctx: CanvasRenderingContext2D | null, text: string, x: number, y: number): void {
    if (!ctx) return;
    ctx.imageSmoothingEnabled = false;
    const advance = this.spec.advance ?? this.spec.cellW;
    let pen = x;
    for (const ch of text) {
      const cell = this.cell(ch);
      if (cell) {
        ctx.drawImage(
          this.img as CanvasImageSource,
          cell.x,
          cell.y,
          cell.w,
          cell.h,
          pen,
          y,
          cell.w,
          cell.h,
        );
      }
      pen += advance;
    }
  }
}

/**
 * Per-URL font cache: glyph strips arrive as data URLs from the skin
 * payload, and every classic text object shares the same two strips —
 * decode once per skin load. Entries are dropped wholesale when the cache
 * outgrows a couple of skins (data URLs change per load, so old entries are
 * unreachable anyway).
 */
const cache = new Map<string, Promise<BmFont>>();
const CACHE_CAP = 8;

/** Get (or decode) the shared `BmFont` for a glyph-strip URL. */
export function loadBmFont(src: string, spec: BmFontSpec): Promise<BmFont> {
  let entry = cache.get(src);
  if (!entry) {
    entry = BmFont.load(src, spec).catch((e) => {
      cache.delete(src); // don't cache failures
      throw e;
    });
    cache.set(src, entry);
    if (cache.size > CACHE_CAP) cache.clear();
  }
  return entry;
}

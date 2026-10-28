#!/usr/bin/env node
/**
 * NavAmp fallback skin generator.
 *
 * Draws every bitmap programmatically (original artwork in the spirit of the
 * Winamp3 default skin — no Nullsoft assets), assembles `skin.xml`, packs
 * `out/fallback.wal` (a ZIP with lowercase entry names) and verifies the
 * archive against the XML the same way crates/navamp-skin will: every
 * `file=` must exist in the archive, bitmap/groupdef ids must be unique,
 * every `image`/`downimage`/`hoverimage`/`thumbimage`/`background` must
 * resolve to a `<bitmap>` id, every `<group id=…>` instance must have a
 * matching `<groupdef>`, numerics must be well-formed and only known
 * (lowercase Wasabi) tags may appear — anything else fails the build.
 *
 * Deterministic: fixed palette, integer-hash noise (no Math.random, no
 * Date.now), fixed ZIP timestamps, sorted entries. The script builds the
 * whole skin twice and refuses to emit anything if the bytes differ.
 *
 * Usage: npm install && node build.mjs   (Node >= 20, no network at runtime)
 */

import { createHash } from "node:crypto";
import { mkdirSync, writeFileSync } from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { zipSync, unzipSync } from "fflate";
import { PNG } from "pngjs";

const HERE = path.dirname(fileURLToPath(import.meta.url));
const OUT_DIR = path.join(HERE, "out");
const OUT_FILE = path.join(OUT_DIR, "fallback.wal");

// ---------------------------------------------------------------------------
// Palette — one steel ramp + ink outline + the display-green accents.
// ---------------------------------------------------------------------------

const hex = (s) => [
  parseInt(s.slice(1, 3), 16),
  parseInt(s.slice(3, 5), 16),
  parseInt(s.slice(5, 7), 16),
];

const INK = hex("#101317"); // 1px outlines (windows, buttons, channels)
const SHADOW = hex("#2c3138"); // raised bevel: dark bottom/right
const HI = hex("#c3cad2"); // raised bevel: light top/left
const WALL_D = hex("#1c2126"); // recessed wall: dark top/left
const WALL_L = hex("#868f98"); // recessed wall: light bottom/right
const GLYPH_INK = hex("#23272d"); // dark steel glyph on buttons
const EMB = hex("#c9d0d8"); // glyph emboss highlight

const GREEN = hex("#00CC33");
const GREEN_DIM = hex("#00801F");
const GREEN_BRIGHT = hex("#66FF99");

/** Button face gradients per state (brushed steel; down = darker + sunken). */
const FACE = {
  normal: { top: hex("#7e868f"), bottom: hex("#5d646c") },
  hover: { top: hex("#8d959e"), bottom: hex("#6b727a") },
  down: { top: hex("#525960"), bottom: hex("#43494f") },
};

// ---------------------------------------------------------------------------
// Tiny pixel canvas on top of pngjs (BGRA buffer, deterministic encode).
// ---------------------------------------------------------------------------

const clamp255 = (v) => (v < 0 ? 0 : v > 255 ? 255 : v | 0);

function makeImg(w, h) {
  const png = new PNG({ width: w, height: h });
  png.data.fill(0);
  const data = png.data;
  const img = {
    w,
    h,
    set(x, y, c, a = 255) {
      if (x < 0 || y < 0 || x >= w || y >= h) return;
      const i = (y * w + x) * 4;
      data[i] = clamp255(c[0]);
      data[i + 1] = clamp255(c[1]);
      data[i + 2] = clamp255(c[2]);
      data[i + 3] = clamp255(a);
    },
    get(x, y) {
      const i = (y * w + x) * 4;
      return [data[i], data[i + 1], data[i + 2], data[i + 3]];
    },
    rect(x, y, rw, rh, c, a = 255) {
      for (let j = 0; j < rh; j++) for (let i = 0; i < rw; i++) img.set(x + i, y + j, c, a);
    },
    toBuffer() {
      return PNG.sync.write(png);
    },
  };
  return img;
}

/** Smooth vertical gradient (float lerp, rounded — no banding steps). */
function vgrad(im, x, y, w, h, top, bottom) {
  for (let j = 0; j < h; j++) {
    const t = h <= 1 ? 0 : j / (h - 1);
    const c = [
      top[0] + (bottom[0] - top[0]) * t,
      top[1] + (bottom[1] - top[1]) * t,
      top[2] + (bottom[2] - top[2]) * t,
    ];
    im.rect(x, y + j, w, 1, c);
  }
}

/** Deterministic integer hash (all noise in this skin comes from it). */
function hash2(a, b, s) {
  let h =
    Math.imul(a ^ 0x9e3779b9, 0x85ebca6b) ^
    Math.imul(b ^ 0xc2b2ae35, 0x27d4eb2f) ^
    Math.imul(s, 0x165667b1);
  h = Math.imul(h ^ (h >>> 15), 0x2c1b3c6d);
  h = Math.imul(h ^ (h >>> 12), 0x297a2d39);
  h ^= h >>> 15;
  return h >>> 0;
}

/**
 * Brushed metal: vertical gradient + per-row tone streaks + 4px horizontal
 * grain. Transparent pixels are left alone so it composites onto cutouts.
 */
function brushed(im, x, y, w, h, top, bottom, amp = 3) {
  vgrad(im, x, y, w, h, top, bottom);
  for (let j = 0; j < h; j++) {
    const rowJ = (hash2(11, y + j, 977) % (2 * amp + 1)) - amp;
    for (let i = 0; i < w; i++) {
      const c = im.get(x + i, y + j);
      if (c[3] === 0) continue;
      const n = (hash2((x + i) >> 2, y + j, 313) % (2 * amp - 1)) - (amp - 1);
      im.set(x + i, y + j, [c[0] + rowJ + n, c[1] + rowJ + n, c[2] + rowJ + n]);
    }
  }
}

// ---------------------------------------------------------------------------
// 5x7 pixel font (cell 6x8). ASCII 32..95; lowercase maps to uppercase.
// Every glyph is 7 rows of 5 chars, '#' = pixel. Digits + A-Z + punctuation.
// ---------------------------------------------------------------------------

const FONT = {
  " ": [".....", ".....", ".....", ".....", ".....", ".....", "....."],
  "!": ["..#..", "..#..", "..#..", "..#..", "..#..", ".....", "..#.."],
  '"': [".#.#.", ".#.#.", ".#.#.", ".....", ".....", ".....", "....."],
  "#": [".#.#.", ".#.#.", "#####", ".#.#.", "#####", ".#.#.", ".#.#."],
  $: ["..#..", ".####", "#....", ".###.", "....#", "####.", "..#.."],
  "%": ["##..#", "##..#", "...#.", "..#..", ".#...", "#..##", "#..##"],
  "&": [".##..", "#..#.", "#.#..", ".#...", "#.#.#", "#..#.", ".##.#"],
  "'": ["..#..", "..#..", ".....", ".....", ".....", ".....", "....."],
  "(": ["...#.", "..#..", ".#...", ".#...", ".#...", "..#..", "...#."],
  ")": [".#...", "..#..", "...#.", "...#.", "...#.", "..#..", ".#..."],
  "*": [".....", "..#..", ".#.#.", "..#..", ".#.#.", "..#..", "....."],
  "+": [".....", "..#..", "..#..", "#####", "..#..", "..#..", "....."],
  ",": [".....", ".....", ".....", ".....", "..#..", "..#..", ".#..."],
  "-": [".....", ".....", ".....", "#####", ".....", ".....", "....."],
  ".": [".....", ".....", ".....", ".....", ".....", ".....", "..#.."],
  "/": ["....#", "....#", "...#.", "..#..", ".#...", "#....", "#...."],
  "0": [".###.", "#...#", "#..##", "#.#.#", "##..#", "#...#", ".###."],
  "1": ["..#..", ".##..", "..#..", "..#..", "..#..", "..#..", ".###."],
  "2": [".###.", "#...#", "....#", "...#.", "..#..", ".#...", "#####"],
  "3": [".###.", "#...#", "....#", "..##.", "....#", "#...#", ".###."],
  "4": ["...#.", "..##.", ".#.#.", "#..#.", "#####", "...#.", "...#."],
  "5": ["#####", "#....", "####.", "....#", "....#", "#...#", ".###."],
  "6": ["..##.", ".#...", "#....", "####.", "#...#", "#...#", ".###."],
  "7": ["#####", "....#", "...#.", "..#..", "..#..", "..#..", "..#.."],
  "8": [".###.", "#...#", "#...#", ".###.", "#...#", "#...#", ".###."],
  "9": [".###.", "#...#", "#...#", ".####", "....#", "...#.", ".##.."],
  ":": [".....", "..#..", ".....", ".....", ".....", "..#..", "....."],
  ";": [".....", "..#..", ".....", ".....", "..#..", "..#..", ".#..."],
  "<": ["...#.", "..#..", ".#...", "#....", ".#...", "..#..", "...#."],
  "=": [".....", ".....", "#####", ".....", "#####", ".....", "....."],
  ">": [".#...", "..#..", "...#.", "....#", "...#.", "..#..", ".#..."],
  "?": [".###.", "#...#", "....#", "...#.", "..#..", ".....", "..#.."],
  "@": [".###.", "#...#", "#.###", "#.#.#", "#.###", "#....", ".###."],
  A: [".###.", "#...#", "#...#", "#####", "#...#", "#...#", "#...#"],
  B: ["####.", "#...#", "#...#", "####.", "#...#", "#...#", "####."],
  C: [".###.", "#...#", "#....", "#....", "#....", "#...#", ".###."],
  D: ["####.", "#...#", "#...#", "#...#", "#...#", "#...#", "####."],
  E: ["#####", "#....", "#....", "####.", "#....", "#....", "#####"],
  F: ["#####", "#....", "#....", "####.", "#....", "#....", "#...."],
  G: [".###.", "#...#", "#....", "#.###", "#...#", "#...#", ".###."],
  H: ["#...#", "#...#", "#...#", "#####", "#...#", "#...#", "#...#"],
  I: [".###.", "..#..", "..#..", "..#..", "..#..", "..#..", ".###."],
  J: ["...##", "....#", "....#", "....#", "....#", "#...#", ".###."],
  K: ["#...#", "#..#.", "#.#..", "##...", "#.#..", "#..#.", "#...#"],
  L: ["#....", "#....", "#....", "#....", "#....", "#....", "#####"],
  M: ["#...#", "##.##", "#.#.#", "#.#.#", "#...#", "#...#", "#...#"],
  N: ["#...#", "##..#", "#.#.#", "#..##", "#...#", "#...#", "#...#"],
  O: [".###.", "#...#", "#...#", "#...#", "#...#", "#...#", ".###."],
  P: ["####.", "#...#", "#...#", "####.", "#....", "#....", "#...."],
  Q: [".###.", "#...#", "#...#", "#...#", "#.#.#", "#..#.", ".##.#"],
  R: ["####.", "#...#", "#...#", "####.", "#.#..", "#..#.", "#...#"],
  S: [".####", "#....", "#....", ".###.", "....#", "....#", "####."],
  T: ["#####", "..#..", "..#..", "..#..", "..#..", "..#..", "..#.."],
  U: ["#...#", "#...#", "#...#", "#...#", "#...#", "#...#", ".###."],
  V: ["#...#", "#...#", "#...#", "#...#", "#...#", ".#.#.", "..#.."],
  W: ["#...#", "#...#", "#...#", "#.#.#", "#.#.#", "##.##", "#...#"],
  X: ["#...#", "#...#", ".#.#.", "..#..", ".#.#.", "#...#", "#...#"],
  Y: ["#...#", "#...#", ".#.#.", "..#..", "..#..", "..#..", "..#.."],
  Z: ["#####", "....#", "...#.", "..#..", ".#...", "#....", "#####"],
  "[": [".###.", "#....", "#....", "#....", "#....", "#....", ".###."],
  "\\": [".....", "#....", ".#...", "..#..", "...#.", "....#", "....."],
  "]": [".###.", "....#", "....#", "....#", "....#", "....#", ".###."],
  "^": ["..#..", ".#.#.", "#...#", ".....", ".....", ".....", "....."],
  _: [".....", ".....", ".....", ".....", ".....", ".....", "#####"],
};

function stamp(im, rows, x, y, color, scale = 1) {
  rows.forEach((row, j) => {
    for (let i = 0; i < row.length; i++) {
      if (row[i] !== "#") continue;
      if (scale === 1) im.set(x + i, y + j, color);
      else im.rect(x + i * scale, y + j * scale, scale, scale, color);
    }
  });
}

/** Ink width of a string (5px glyphs + 1px advance, no trailing space). */
const textWidth = (s, scale = 1) => (s.length * 6 - 1) * scale;

/**
 * Draw text from the pixel font. `parts` is a string (one color) or a list
 * of [string, color] segments (e.g. bright value + dim label).
 */
function drawText(im, x, y, parts, scale = 1) {
  const segs = typeof parts === "string" ? [[parts, GREEN]] : parts;
  let cx = x;
  for (const [s, color] of segs) {
    for (const ch of s) {
      const rows = FONT[ch] ?? FONT[ch.toUpperCase()] ?? FONT["?"];
      stamp(im, rows, cx, y, color, scale);
      cx += 6 * scale;
    }
  }
  return cx - x;
}

/** Bright text with a 1px drop shadow (title bars). */
function embossedText(im, x, y, s, main, shadow) {
  drawText(im, x + 1, y + 1, [[s, shadow]]);
  drawText(im, x, y, [[s, main]]);
}

// ---------------------------------------------------------------------------
// Transport glyphs: clean pixel shapes generated from triangles/bars.
// ---------------------------------------------------------------------------

const triLeft = (w, h) => {
  const rows = [];
  const c = (h - 1) / 2;
  for (let j = 0; j < h; j++) {
    const start = Math.round(Math.abs(j - c));
    rows.push(".".repeat(start) + "#".repeat(w - start));
  }
  return rows;
};
const triRight = (w, h) => {
  const rows = [];
  const c = (h - 1) / 2;
  for (let j = 0; j < h; j++) {
    const end = w - Math.round(Math.abs(j - c));
    rows.push("#".repeat(end) + ".".repeat(w - end));
  }
  return rows;
};

const GLYPH = {
  prev: triLeft(9, 9).map((r) => "##." + r), // ⏮ bar + left triangle
  next: triRight(9, 9).map((r) => r + ".##"), // ⏭ right triangle + bar
  play: triRight(9, 9), // ▶
  pause: Array.from({ length: 9 }, () => "###...###"), // ⏸
  stop: Array.from({ length: 9 }, () => "#########"), // ⏹
  eject: [
    ".....#.....",
    "....###....",
    "...#####...",
    "..#######..",
    ".#########.",
    "###########",
    "...........",
    "###########",
    "###########",
  ], // ⏏
};

// ---------------------------------------------------------------------------
// Component painters.
// ---------------------------------------------------------------------------

/** Raised/sunken beveled metal button face with 1px ink outline. */
function buttonFace(im, x, y, w, h, state) {
  const f = FACE[state];
  brushed(im, x + 2, y + 2, w - 4, h - 4, f.top, f.bottom, 3);
  const sunken = state === "down";
  const tl = sunken ? SHADOW : HI;
  const br = sunken ? HI : SHADOW;
  im.rect(x + 1, y + 1, w - 2, 1, tl);
  im.rect(x + 1, y + 1, 1, h - 2, tl);
  im.rect(x + 1, y + h - 2, w - 2, 1, br);
  im.rect(x + w - 2, y + 1, 1, h - 2, br);
  im.rect(x, y, w, 1, INK);
  im.rect(x, y + h - 1, w, 1, INK);
  im.rect(x, y, 1, h, INK);
  im.rect(x + w - 1, y, 1, h, INK);
}

/** Dark glyph with a light emboss below-right; pressed = nudged 1px. */
function glyphOn(im, rows, x, y, state) {
  const d = state === "down" ? 1 : 0;
  stamp(im, rows, x + d + 1, y + d + 1, EMB);
  stamp(im, rows, x + d, y + d, GLYPH_INK);
}

/** Font label on a button; pressed/active state glows display-green. */
function labelOn(im, text, x, y, state) {
  if (state === "down") {
    drawText(im, x + 1, y + 1, [[text, GREEN_BRIGHT]]);
  } else {
    drawText(im, x + 1, y + 1, [[text, EMB]]);
    drawText(im, x, y, [[text, GLYPH_INK]]);
  }
}

/** Decorative title-bar buttons: min / shade / close glyphs. */
const WIN_GLYPHS = {
  min: (im, x, y) => {
    im.rect(x + 5, y + 7, 6, 2, EMB);
    im.rect(x + 4, y + 6, 6, 2, GLYPH_INK);
  },
  shade: (im, x, y) => {
    im.rect(x + 4, y + 4, 8, 2, EMB);
    im.rect(x + 4, y + 9, 8, 1, EMB);
    im.rect(x + 3, y + 3, 8, 2, GLYPH_INK);
    im.rect(x + 3, y + 8, 8, 1, GLYPH_INK);
  },
  close: (im, x, y) => {
    for (let i = 0; i < 6; i++) {
      im.set(x + 4 + i, y + 3 + i, EMB);
      im.set(x + 9 - i, y + 3 + i, EMB);
    }
    for (let i = 0; i < 6; i++) {
      im.set(x + 3 + i, y + 2 + i, GLYPH_INK);
      im.set(x + 8 - i, y + 2 + i, GLYPH_INK);
    }
  },
};

/** Window chrome: brushed body, 1px ink outline, light/dark inner edges. */
function windowChrome(im, w, h, footerGrooveY = 0) {
  brushed(im, 1, 1, w - 2, h - 2, hex("#6a7179"), hex("#4d535a"), 3);
  im.rect(1, 1, w - 2, 1, hex("#9aa2ab"));
  im.rect(1, 1, 1, h - 2, hex("#9aa2ab"));
  im.rect(1, h - 2, w - 2, 1, hex("#262b31"));
  im.rect(w - 2, 1, 1, h - 2, hex("#262b31"));
  im.rect(0, 0, w, 1, INK);
  im.rect(0, h - 1, w, 1, INK);
  im.rect(0, 0, 1, h, INK);
  im.rect(w - 1, 0, 1, h, INK);
  if (footerGrooveY > 0) {
    im.rect(2, footerGrooveY, w - 4, 1, hex("#22262b"));
    im.rect(2, footerGrooveY + 1, w - 4, 1, hex("#7d858e"));
  }
}

/** Dark metallic title bar, 14px tall. */
function titlebarPaint(im, w) {
  vgrad(im, 0, 0, w, 13, hex("#7d858e"), hex("#2c3138"));
  for (let j = 0; j < 13; j++) {
    for (let i = 0; i < w; i++) {
      const n = (hash2(i >> 2, j, 601) % 5) - 2;
      const c = im.get(i, j);
      im.set(i, j, [c[0] + n, c[1] + n, c[2] + n]);
    }
  }
  im.rect(0, 13, w, 1, INK);
}

/** Recessed near-black panel (display / vis / list): inverted 1px bevel. */
function recessPanel(im, w, h, fillTop, fillBottom) {
  vgrad(im, 0, 0, w, h, fillTop, fillBottom);
  im.rect(0, 0, w, 1, WALL_D);
  im.rect(0, 0, 1, h, WALL_D);
  im.rect(0, h - 1, w, 1, WALL_L);
  im.rect(w - 1, 0, 1, h, WALL_L);
}

/** Slider channel: ink outline, recessed walls, tick marks. */
function channelPaint(im, w, { tickEvery = 12, centerTick = false, active = false }) {
  vgrad(im, 1, 1, w - 2, 11, active ? hex("#1a1e24") : hex("#15181d"), active ? hex("#141820") : hex("#101318"));
  const tick = active ? hex("#3d4750") : hex("#232a33");
  const strong = active ? hex("#55606b") : hex("#39424c");
  const mid = w >> 1;
  for (let x = tickEvery; x < w - 3; x += tickEvery) {
    if (centerTick && Math.abs(x - mid) < 3) continue; // keep the detent clear
    im.rect(x, 4, 1, 5, tick);
  }
  if (centerTick) im.rect(mid, 3, 1, 7, strong); // balance detent tick
  im.rect(0, 1, w, 1, WALL_D);
  im.rect(0, 1, 1, 11, WALL_D);
  im.rect(0, 11, w, 1, WALL_L);
  im.rect(w - 1, 1, 1, 11, WALL_L);
  im.rect(0, 0, w, 1, INK);
  im.rect(0, 12, w, 1, INK);
}

// ---------------------------------------------------------------------------
// Skin content: PNGs, bitmap defs, skin.xml.
// ---------------------------------------------------------------------------

const STATES = ["normal", "hover", "down"];
const TRANSPORTS = [
  ["prev", "PREV", "Previous"],
  ["play", "PLAY", "Play"],
  ["pause", "PAUSE", "Pause / resume"],
  ["stop", "STOP", "Stop"],
  ["next", "NEXT", "Next"],
  ["eject", "EJECT", "Eject / open files"],
];
const PLAYLIST_ACTIONS = [
  ["add", "PLAYLIST_ADD", "Add media"],
  ["rem", "PLAYLIST_REMOVE", "Remove selected"],
  ["sel", "PLAYLIST_SELECT", "Select all"],
  ["misc", "PLAYLIST_MISC", "Playlist options"],
];
const WIN_BUTTONS = [
  ["min", "Minimize"],
  ["shade", "Window shade"],
  ["close", "Close"],
];

function generate() {
  const files = new Map(); // lowercase zip entry name -> Uint8Array
  const bitmapDefs = []; // {id, file, x?, y?, w?, h?}

  const addPng = (name, w, h, paint) => {
    const im = makeImg(w, h);
    paint(im);
    files.set(name, new Uint8Array(im.toBuffer()));
  };
  const bitmap = (id, file, rect) => {
    bitmapDefs.push(rect ? { id, file, ...rect } : { id, file });
  };

  // ---- whole-file bitmaps: player ----------------------------------------

  addPng("player/window.png", 275, 280, (im) => windowChrome(im, 275, 280, 261));
  bitmap("player.window", "player/window.png");

  addPng("player/titlebar.png", 275, 14, (im) => titlebarPaint(im, 275));
  bitmap("titlebar.player.bg", "player/titlebar.png");

  addPng("player/title-player.png", 36, 8, (im) =>
    embossedText(im, 0, 0, "NAVAMP", hex("#dde2e7"), hex("#0d1013")),
  );
  bitmap("player.title.logo", "player/title-player.png");

  addPng("player/display-panel.png", 255, 60, (im) =>
    recessPanel(im, 255, 60, hex("#060a08"), hex("#0b110d")),
  );
  bitmap("player.display.panel", "player/display-panel.png");

  addPng("player/time-ghost.png", 72, 16, (im) => {
    // Ghost "88:88" at 2x — live digits paint over it in the engine.
    drawText(im, 7, 1, [["88:88", GREEN_DIM]], 2);
  });
  bitmap("player.time.ghost", "player/time-ghost.png");

  addPng("player/kbps.png", 48, 7, (im) =>
    drawText(im, 0, 0, [["320", GREEN], [" KBPS", GREEN_DIM]]),
  );
  bitmap("player.display.kbps", "player/kbps.png");

  addPng("player/khz.png", 36, 7, (im) =>
    drawText(im, 0, 0, [["44", GREEN], [" KHZ", GREEN_DIM]]),
  );
  bitmap("player.display.khz", "player/khz.png");

  addPng("player/mono-stereo.png", 36, 15, (im) => {
    drawText(im, 0, 0, [["MONO", GREEN_DIM]]);
    drawText(im, 0, 8, [["STEREO", GREEN]]);
  });
  bitmap("player.display.channels", "player/mono-stereo.png");

  addPng("player/logo.png", 36, 8, (im) =>
    embossedText(im, 0, 0, "NAVAMP", hex("#1f2429"), hex("#8b939c")),
  );
  bitmap("player.logo", "player/logo.png");

  addPng("player/seek-channel.png", 255, 13, (im) => channelPaint(im, 255, { tickEvery: 12 }));
  bitmap("player.seek.channel", "player/seek-channel.png");
  addPng("player/seek-channel-on.png", 255, 13, (im) =>
    channelPaint(im, 255, { tickEvery: 12, active: true }),
  );
  bitmap("player.seek.channel.on", "player/seek-channel-on.png");

  addPng("player/balance-channel.png", 110, 13, (im) =>
    channelPaint(im, 110, { tickEvery: 10, centerTick: true }),
  );
  bitmap("player.balance.channel", "player/balance-channel.png");
  addPng("player/balance-channel-on.png", 110, 13, (im) =>
    channelPaint(im, 110, { tickEvery: 10, centerTick: true, active: true }),
  );
  bitmap("player.balance.channel.on", "player/balance-channel-on.png");

  addPng("player/volume-channel.png", 110, 13, (im) => channelPaint(im, 110, { tickEvery: 10 }));
  bitmap("player.volume.channel", "player/volume-channel.png");
  addPng("player/volume-channel-on.png", 110, 13, (im) =>
    channelPaint(im, 110, { tickEvery: 10, active: true }),
  );
  bitmap("player.volume.channel.on", "player/volume-channel-on.png");

  addPng("player/thumb.png", 13, 13, (im) => {
    brushed(im, 2, 2, 9, 9, hex("#7e868f"), hex("#5a6169"), 3);
    im.rect(5, 3, 1, 7, hex("#3a4046"));
    im.rect(6, 3, 1, 7, hex("#b6bcc3"));
    im.rect(7, 3, 1, 7, hex("#3a4046"));
    im.rect(1, 1, 11, 1, HI);
    im.rect(1, 1, 1, 11, HI);
    im.rect(1, 11, 11, 1, SHADOW);
    im.rect(11, 1, 1, 11, SHADOW);
    im.rect(0, 0, 13, 1, INK);
    im.rect(0, 12, 13, 1, INK);
    im.rect(0, 0, 1, 13, INK);
    im.rect(12, 0, 1, 13, INK);
  });
  bitmap("player.slider.thumb", "player/thumb.png");

  // 4-frame playstate meter (animatedlayer; frame 1 shows while stopped).
  addPng("player/playstate.png", 64, 16, (im) => {
    const heights = [4, 7, 10, 13];
    for (let f = 0; f < 4; f++) {
      for (let b = 0; b < 4; b++) {
        const lit = b <= f;
        const bh = lit ? heights[b] : 2;
        const x = f * 16 + 3 + b * 3;
        if (lit) im.rect(x, 14 - bh, 2, 1, GREEN_BRIGHT);
        im.rect(x, 14 - bh + (lit ? 1 : 0), 2, bh - (lit ? 1 : 0), lit ? GREEN : hex("#0d3018"));
      }
    }
  });
  bitmap("player.playstate.anim", "player/playstate.png");

  addPng("player/vis-panel.png", 255, 96, (im) => {
    recessPanel(im, 255, 96, hex("#060a08"), hex("#0b110d"));
    for (let y = 7; y < 90; y += 8) for (let x = 7; x < 251; x += 8) im.set(x, y, hex("#121c15"));
    im.rect(3, 89, 249, 1, hex("#16241a"));
  });
  bitmap("player.vis.panel", "player/vis-panel.png");

  // ---- sprite sheets: player ---------------------------------------------

  // Title-bar window buttons: 3 buttons x 3 states, 14x10 cells.
  addPng("player/titlebar-buttons.png", 42, 30, (im) => {
    WIN_BUTTONS.forEach(([name], i) => {
      STATES.forEach((state, s) => {
        const x = i * 14;
        const y = s * 10;
        buttonFace(im, x, y, 14, 10, state);
        const d = state === "down" ? 1 : 0;
        WIN_GLYPHS[name](im, x + d, y + d); // emboss baked into the glyph fn
      });
    });
  });
  WIN_BUTTONS.forEach(([name], i) => {
    STATES.forEach((state, s) => {
      bitmap(`player.win.${name}.${state}`, "player/titlebar-buttons.png", {
        x: i * 14,
        y: s * 10,
        w: 14,
        h: 10,
      });
    });
  });

  // Transport buttons: 6 glyphs x 3 states, 26x22 cells.
  addPng("player/transport.png", 156, 66, (im) => {
    TRANSPORTS.forEach(([name], i) => {
      STATES.forEach((state, s) => {
        const x = i * 26;
        const y = s * 22;
        buttonFace(im, x, y, 26, 22, state);
        const rows = GLYPH[name];
        const gx = x + ((26 - rows[0].length - 1) >> 1);
        const gy = y + ((22 - rows.length - 1) >> 1);
        glyphOn(im, rows, gx, gy, state);
      });
    });
  });
  TRANSPORTS.forEach(([name], i) => {
    STATES.forEach((state, s) => {
      bitmap(`player.btn.${name}.${state}`, "player/transport.png", {
        x: i * 26,
        y: s * 22,
        w: 26,
        h: 22,
      });
    });
  });

  // Bottom-strip toggles (playlist / shade): 2 x 3 states, 54x18 cells.
  addPng("player/toggles.png", 108, 54, (im) => {
    const labels = ["PLAYLIST", "SHADE"];
    labels.forEach((label, i) => {
      STATES.forEach((state, s) => {
        const x = i * 54;
        const y = s * 18;
        buttonFace(im, x, y, 54, 18, state);
        labelOn(im, label, x + ((54 - textWidth(label) - 1) >> 1), y + ((18 - 7) >> 1), state);
      });
    });
  });
  ["playlist", "shade"].forEach((name, i) => {
    STATES.forEach((state, s) => {
      bitmap(`player.toggle.${name}.${state}`, "player/toggles.png", {
        x: i * 54,
        y: s * 18,
        w: 54,
        h: 18,
      });
    });
  });

  // ---- whole-file bitmaps: playlist ---------------------------------------

  addPng("playlist/window.png", 275, 232, (im) => windowChrome(im, 275, 232));
  bitmap("playlist.window", "playlist/window.png");

  addPng("playlist/titlebar.png", 275, 14, (im) => titlebarPaint(im, 275));
  bitmap("titlebar.playlist.bg", "playlist/titlebar.png");

  addPng("playlist/title-playlist.png", 48, 8, (im) =>
    embossedText(im, 0, 0, "PLAYLIST", hex("#dde2e7"), hex("#0d1013")),
  );
  bitmap("playlist.title.logo", "playlist/title-playlist.png");

  addPng("playlist/list-panel.png", 265, 174, (im) =>
    recessPanel(im, 265, 174, hex("#060907"), hex("#0a0f0c")),
  );
  bitmap("playlist.list.panel", "playlist/list-panel.png");

  // Rows the playlist renderer positions per track.
  addPng("playlist/row-selected.png", 257, 13, (im) => {
    vgrad(im, 0, 0, 257, 13, hex("#4e6d8a"), hex("#2b4359"));
    im.rect(0, 0, 257, 1, hex("#5f80a0"));
    im.rect(0, 12, 257, 1, hex("#223448"));
  });
  bitmap("playlist.row.selected", "playlist/row-selected.png");

  addPng("playlist/row-current.png", 257, 13, (im) => {
    im.rect(0, 0, 257, 12, GREEN, 26); // faint green wash
    im.rect(0, 12, 257, 1, GREEN); // bright underline
  });
  bitmap("playlist.row.current", "playlist/row-current.png");

  addPng("playlist/row-divider.png", 257, 13, (im) => {
    im.rect(0, 12, 257, 1, hex("#141a20"));
  });
  bitmap("playlist.row.divider", "playlist/row-divider.png");

  addPng("playlist/totals.png", 78, 7, (im) => {
    drawText(im, 0, 0, [["0 ITEMS  0:00", GREEN_DIM]]);
  });
  bitmap("playlist.totals.ghost", "playlist/totals.png");

  addPng("playlist/resize-grip.png", 9, 7, (im) => {
    for (let off = 0; off <= 6; off += 3) {
      for (let x = 0; x + off < 9; x++) {
        const px = x + off;
        const py = 6 - x;
        im.set(px, py, hex("#22262b"));
        im.set(px + 1, py + 1, hex("#7d858e"));
      }
    }
  });
  bitmap("playlist.resize.grip", "playlist/resize-grip.png");

  // Playlist action buttons: 4 labels x 3 states, 36x16 cells.
  addPng("playlist/actions.png", 144, 48, (im) => {
    PLAYLIST_ACTIONS.forEach(([name], i) => {
      STATES.forEach((state, s) => {
        const x = i * 36;
        const y = s * 16;
        buttonFace(im, x, y, 36, 16, state);
        const label = name.toUpperCase();
        labelOn(im, label, x + ((36 - textWidth(label) - 1) >> 1), y + ((16 - 7) >> 1), state);
      });
    });
  });
  PLAYLIST_ACTIONS.forEach(([name], i) => {
    STATES.forEach((state, s) => {
      bitmap(`playlist.btn.${name}.${state}`, "playlist/actions.png", {
        x: i * 36,
        y: s * 16,
        w: 36,
        h: 16,
      });
    });
  });

  // ---- bitmap font strip (ASCII 32..95, green on transparent) -------------

  addPng("player-fonts/font.png", 384, 8, (im) => {
    for (let c = 32; c <= 95; c++) {
      const ch = String.fromCharCode(c);
      const rows = FONT[ch] ?? FONT["?"];
      stamp(im, rows, (c - 32) * 6, 0, GREEN);
    }
  });

  // ---- skin.xml -----------------------------------------------------------

  const winButtons = (prefix) =>
    WIN_BUTTONS.map(
      ([name, tip], i) =>
        `    <togglebutton id="${prefix}.${name}" image="player.win.${name}.normal" hoverimage="player.win.${name}.hover" downimage="player.win.${name}.down" x="${227 + i * 16}" y="2" w="14" h="10" tooltip="${tip}"/>`,
    ).join("\n");

  const transportButtons = TRANSPORTS.map(
    ([name, action, tip], i) =>
      `    <button id="player.${name}" action="${action}" image="player.btn.${name}.normal" hoverimage="player.btn.${name}.hover" downimage="player.btn.${name}.down" x="${i * 29}" y="0" w="26" h="22" tooltip="${tip}"/>`,
  ).join("\n");

  const actionButtons = PLAYLIST_ACTIONS.map(
    ([name, action, tip], i) =>
      `    <button id="playlist.${name}" action="${action}" image="playlist.btn.${name}.normal" hoverimage="playlist.btn.${name}.hover" downimage="playlist.btn.${name}.down" x="${i * 39}" y="0" w="36" h="16" tooltip="${tip}"/>`,
  ).join("\n");

  const elementsXml =
    bitmapDefs
      .map((b) => {
        const rect = ["x", "y", "w", "h"]
          .filter((k) => b[k] !== undefined)
          .map((k) => ` ${k}="${b[k]}"`)
          .join("");
        return `    <bitmap id="${b.id}" file="${b.file}"${rect}/>`;
      })
      .join("\n") +
    `\n    <bitmapfont id="player.font" file="player-fonts/font.png" charwidth="6" charheight="8"/>` +
    [
      ["player.green", "#00CC33"],
      ["player.green.dim", "#00801F"],
      ["player.green.bright", "#66FF99"],
      ["player.vis.bars", "#00CC33"],
      ["player.vis.peaks", "#66FF99"],
      ["playlist.text", "#00CC33"],
      ["playlist.text.dim", "#00801F"],
      ["playlist.text.selected", "#DFE3E7"],
    ]
      .map(([id, value]) => `\n    <color id="${id}" value="${value}"/>`)
      .join("");

  const skinXml = `<?xml version="1.0" encoding="UTF-8"?>
<skin name="NavAmp Fallback" author="NavAmp Project" version="1.0">

  <elements>
${elementsXml}
  </elements>

  <!-- Player window chrome: gradient title bar + decorative window buttons. -->
  <groupdef id="player.titlebar" background="titlebar.player.bg" x="0" y="0" w="275" h="14">
    <layer id="player.title.logo" image="player.title.logo" x="4" y="3" w="36" h="8"/>
${winButtons("player")}
  </groupdef>

  <!-- Recessed display: ticker, play meter, big time, status, kbps/khz/stereo. -->
  <groupdef id="player.display" background="player.display.panel" x="0" y="0" w="255" h="60">
    <text id="player.songticker" display="songticker" x="8" y="6" w="150" h="9" font="Verdana, Tahoma, sans-serif" fontsize="8" color="#00CC33"/>
    <animatedlayer id="player.playstate" image="player.playstate.anim" x="162" y="5" w="16" h="16" length="4"/>
    <layer id="player.time.ghost" image="player.time.ghost" x="180" y="4" w="72" h="16"/>
    <text id="player.time" display="time" x="180" y="4" w="72" h="16" text=" " font="Verdana, Tahoma, sans-serif" fontsize="10" color="#00CC33"/>
    <text id="player.status" display="status" x="8" y="21" w="120" h="9" text="STOPPED" font="Verdana, Tahoma, sans-serif" fontsize="8" color="#00CC33"/>
    <layer id="player.kbps" image="player.display.kbps" x="8" y="36" w="48" h="7"/>
    <layer id="player.khz" image="player.display.khz" x="62" y="36" w="36" h="7"/>
    <layer id="player.channels" image="player.display.channels" x="206" y="32" w="36" h="15"/>
  </groupdef>

  <!-- Transport cluster: PREV PLAY PAUSE STOP NEXT EJECT. -->
  <groupdef id="player.transport" x="0" y="0" w="171" h="22">
${transportButtons}
  </groupdef>

  <!-- Playlist window chrome (reuses the window-button sheet). -->
  <groupdef id="playlist.titlebar" background="titlebar.playlist.bg" x="0" y="0" w="275" h="14">
    <layer id="playlist.title.logo" image="playlist.title.logo" x="4" y="3" w="48" h="8"/>
${winButtons("playlist")}
  </groupdef>

  <!-- Playlist bottom-bar action buttons. -->
  <groupdef id="playlist.actions" x="0" y="0" w="153" h="16">
${actionButtons}
  </groupdef>

  <container id="main" name="NavAmp Player">
    <layout id="normal" x="0" y="0" w="275" h="280" background="player.window">
      <layer id="player.background" image="player.window" x="0" y="0" w="275" h="280"/>
      <group id="player.titlebar" x="0" y="0" w="275" h="14"/>
      <group id="player.display" x="10" y="16" w="255" h="60"/>
      <slider id="player.seek" action="SEEK" image="player.seek.channel" downimage="player.seek.channel.on" thumbimage="player.slider.thumb" x="10" y="80" w="255" h="13" orientation="horizontal" tooltip="Seek"/>
      <slider id="player.balance" action="BALANCE" image="player.balance.channel" downimage="player.balance.channel.on" thumbimage="player.slider.thumb" x="10" y="96" w="110" h="13" orientation="horizontal" tooltip="Balance"/>
      <slider id="player.volume" action="VOLUME" image="player.volume.channel" downimage="player.volume.channel.on" thumbimage="player.slider.thumb" x="155" y="96" w="110" h="13" orientation="horizontal" tooltip="Volume"/>
      <group id="player.transport" x="52" y="114" w="171" h="22"/>
      <layer id="player.vis.bg" image="player.vis.panel" x="10" y="141" w="255" h="96"/>
      <vis id="player.vis" x="14" y="145" w="247" h="88"/>
      <togglebutton id="player.playlist.toggle" action="TOGGLE_PLAYLIST" image="player.toggle.playlist.normal" hoverimage="player.toggle.playlist.hover" downimage="player.toggle.playlist.down" x="10" y="243" w="54" h="18" tooltip="Playlist editor"/>
      <togglebutton id="player.shade.toggle" action="TOGGLE_SHADE" image="player.toggle.shade.normal" hoverimage="player.toggle.shade.hover" downimage="player.toggle.shade.down" x="68" y="243" w="54" h="18" tooltip="Shade mode"/>
      <layer id="player.footer.logo" image="player.logo" x="120" y="266" w="36" h="8"/>
    </layout>
  </container>

  <container id="playlist" name="Playlist Editor">
    <layout id="normal" x="0" y="0" w="275" h="232" background="playlist.window">
      <layer id="playlist.background" image="playlist.window" x="0" y="0" w="275" h="232"/>
      <group id="playlist.titlebar" x="0" y="0" w="275" h="14"/>
      <layer id="playlist.list.bg" image="playlist.list.panel" x="5" y="17" w="265" h="174"/>
      <layer id="playlist.row.divider" image="playlist.row.divider" x="9" y="21" w="257" h="13"/>
      <layer id="playlist.row.selected" image="playlist.row.selected" x="9" y="47" w="257" h="13"/>
      <layer id="playlist.row.current" image="playlist.row.current" x="9" y="21" w="257" h="13"/>
      <group id="playlist.actions" x="8" y="201" w="153" h="16"/>
      <layer id="playlist.totals.bg" image="playlist.totals.ghost" x="170" y="206" w="78" h="7"/>
      <status id="playlist.totals" x="170" y="203" w="97" h="12"/>
      <layer id="playlist.resize.grip" image="playlist.resize.grip" x="258" y="223" w="9" h="7"/>
    </layout>
  </container>

</skin>
`;

  const zip = assemble(skinXml, files);
  return { xml: skinXml, files, zip, bitmapDefs };
}

// ---------------------------------------------------------------------------
// ZIP assembly with fixed timestamps (deterministic bytes).
// ---------------------------------------------------------------------------

const FIXED_DATE = new Date(Date.UTC(2026, 0, 1, 0, 0, 0));

function assemble(xmlText, files) {
  const entries = { "skin.xml": new Uint8Array(Buffer.from(xmlText, "utf8")) };
  for (const name of [...files.keys()].sort()) entries[name] = files.get(name);
  return patchZipDates(zipSync(entries, { level: 6 }), FIXED_DATE);
}

/** Overwrite every local + central DOS timestamp with a fixed date. */
function patchZipDates(u8, date) {
  const time = (date.getUTCHours() << 11) | (date.getUTCMinutes() << 5) | (date.getUTCSeconds() >> 1);
  const day =
    ((date.getUTCFullYear() - 1980) << 9) | ((date.getUTCMonth() + 1) << 5) | date.getUTCDate();
  const dv = new DataView(u8.buffer, u8.byteOffset, u8.byteLength);
  let off = 0;
  while (off + 4 <= u8.length) {
    const sig = dv.getUint32(off, true);
    if (sig === 0x04034b50) {
      // local file header
      dv.setUint16(off + 10, time, true);
      dv.setUint16(off + 12, day, true);
      const csize = dv.getUint32(off + 18, true);
      const nlen = dv.getUint16(off + 26, true);
      const elen = dv.getUint16(off + 28, true);
      off += 30 + nlen + elen + csize;
    } else if (sig === 0x02014b50) {
      // central directory header
      dv.setUint16(off + 12, time, true);
      dv.setUint16(off + 14, day, true);
      const nlen = dv.getUint16(off + 28, true);
      const elen = dv.getUint16(off + 30, true);
      const clen = dv.getUint16(off + 32, true);
      off += 46 + nlen + elen + clen;
    } else {
      break; // EOCD or anything unexpected ends the walk
    }
  }
  return u8;
}

// ---------------------------------------------------------------------------
// Verification — mirrors the warnings crates/navamp-skin would emit, but
// fails the build instead of warning.
// ---------------------------------------------------------------------------

const KNOWN_TAGS = new Set([
  "skin",
  "elements",
  "bitmap",
  "bitmapfont",
  "truetypefont",
  "color",
  "gamma",
  "groupdef",
  "container",
  "layout",
  "layer",
  "animatedlayer",
  "button",
  "togglebutton",
  "slider",
  "text",
  "vis",
  "status",
  "grid",
  "group",
  "include",
]);

function dupCheck(ids, kind, errors) {
  const seen = new Set();
  for (const id of ids) {
    if (seen.has(id)) errors.push(`duplicate ${kind} id "${id}"`);
    seen.add(id);
  }
}

function verify(zipBytes, xml) {
  const errors = [];
  const entries = unzipSync(zipBytes); // also proves the ZIP is still readable
  const names = Object.keys(entries);

  for (const name of names) {
    if (name !== name.toLowerCase()) errors.push(`entry name not lowercase: "${name}"`);
  }
  if (!names.includes("skin.xml")) errors.push("skin.xml missing from archive");

  // Every file= referenced by skin.xml must exist in the archive.
  const fileRefs = [...xml.matchAll(/\bfile="([^"]*)"/g)].map((m) => m[1]);
  for (const f of fileRefs) {
    if (!names.includes(f)) errors.push(`file="${f}" referenced in skin.xml is not in the archive`);
  }
  // And no orphan assets.
  for (const name of names) {
    if (name.endsWith(".png") && !fileRefs.includes(name)) {
      errors.push(`archive file "${name}" is not referenced by any file=`);
    }
  }

  // Unique ids across element kinds + groupdefs.
  const bitmapIds = [...xml.matchAll(/<bitmap\s+id="([^"]+)"/g)].map((m) => m[1]);
  dupCheck(bitmapIds, "bitmap", errors);
  dupCheck(
    [...xml.matchAll(/<bitmapfont\s+id="([^"]+)"/g)].map((m) => m[1]),
    "bitmapfont",
    errors,
  );
  dupCheck([...xml.matchAll(/<color\s+id="([^"]+)"/g)].map((m) => m[1]), "color", errors);
  const groupIds = [...xml.matchAll(/<groupdef\s+id="([^"]+)"/g)].map((m) => m[1]);
  dupCheck(groupIds, "groupdef", errors);
  dupCheck(
    [...xml.matchAll(/<container\s+id="([^"]+)"/g)].map((m) => m[1]),
    "container",
    errors,
  );

  // Image references must resolve to bitmap ids.
  const idSet = new Set(bitmapIds);
  for (const m of xml.matchAll(/\b(image|downimage|hoverimage|thumbimage|background)="([^"]+)"/g)) {
    if (!idSet.has(m[2])) errors.push(`${m[1]}="${m[2]}" does not match any <bitmap> id`);
  }

  // Group instances must resolve to groupdefs.
  const groupSet = new Set(groupIds);
  for (const m of xml.matchAll(/<group\s+id="([^"]+)"/g)) {
    if (!groupSet.has(m[1])) errors.push(`group instance "${m[1]}" has no matching <groupdef>`);
  }

  // Numeric attributes must be well-formed integers.
  for (const m of xml.matchAll(/\s(x|y|w|h|charwidth|charheight)="([^"]*)"/g)) {
    if (!/^-?\d+$/.test(m[2])) errors.push(`non-numeric ${m[1]}="${m[2]}"`);
  }

  // Only known lowercase Wasabi tags (unknown tags would warn Rust-side).
  for (const m of xml.matchAll(/<\/?([a-zA-Z][a-zA-Z0-9_.-]*)/g)) {
    if (!KNOWN_TAGS.has(m[1])) errors.push(`unknown tag <${m[1]}> would produce a parser warning`);
  }

  return { errors, names };
}

// ---------------------------------------------------------------------------
// Summary + entry point.
// ---------------------------------------------------------------------------

function summarize(xml) {
  const rows = [];
  const parts = xml.split(/<container id="([^"]+)"[^>]*>/).slice(1);
  for (let i = 0; i < parts.length; i += 2) {
    const body = parts[i + 1];
    const lm = body.match(/<layout id="([^"]+)"[^>]*\sw="(\d+)"\sh="(\d+)"/);
    const counts = {};
    let total = 0;
    for (const t of [
      "layer",
      "animatedlayer",
      "button",
      "togglebutton",
      "slider",
      "text",
      "vis",
      "status",
      "group",
    ]) {
      counts[t] = [...body.matchAll(new RegExp(`<${t}(\\s|/)`, "g"))].length;
      total += counts[t];
    }
    rows.push({
      container: parts[i],
      layout: lm?.[1] ?? "?",
      size: `${lm?.[2]}x${lm?.[3]}`,
      counts,
      total,
    });
  }
  return rows;
}

function main() {
  // Determinism gate: two full builds must be byte-identical.
  const a = generate();
  const b = generate();
  if (Buffer.compare(Buffer.from(a.zip), Buffer.from(b.zip)) !== 0) {
    console.error("FAIL: two consecutive builds produced different bytes");
    process.exit(1);
  }

  const { errors, names } = verify(a.zip, a.xml);
  if (errors.length > 0) {
    console.error(`FAIL: ${errors.length} verification error(s):`);
    for (const e of errors) console.error(`  - ${e}`);
    process.exit(1);
  }

  mkdirSync(OUT_DIR, { recursive: true });
  writeFileSync(OUT_FILE, a.zip);

  const sha = createHash("sha256").update(a.zip).digest("hex");
  const pngCount = names.filter((n) => n.endsWith(".png")).length;

  console.log(`out/fallback.wal written (${a.zip.length} bytes, sha256 ${sha})`);
  console.log(`entries: ${names.length} (${pngCount} png + skin.xml), all lowercase, verified clean\n`);

  console.log("windows / objects (direct layout children, groups listed separately):");
  const head = ["container", "layout", "size", "layer", "anim", "btn", "tog", "sld", "txt", "vis", "sts", "grp", "total"];
  console.log(head.map((h, i) => (i < 3 ? h.padEnd(10) : h.padStart(6))).join(""));
  for (const r of summarize(a.xml)) {
    const c = r.counts;
    console.log(
      [
        r.container.padEnd(10),
        r.layout.padEnd(10),
        r.size.padEnd(10),
        String(c.layer).padStart(6),
        String(c.animatedlayer).padStart(6),
        String(c.button).padStart(6),
        String(c.togglebutton).padStart(6),
        String(c.slider).padStart(6),
        String(c.text).padStart(6),
        String(c.vis).padStart(6),
        String(c.status).padStart(6),
        String(c.group).padStart(6),
        String(r.total).padStart(6),
      ].join(""),
    );
  }

  const groupStats = [...a.xml.matchAll(/<groupdef id="([^"]+)"[\s\S]*?<\/groupdef>/g)].map((g) => {
    const objs = [...g[0].matchAll(/<(layer|animatedlayer|button|togglebutton|slider|text|vis|status)\b/g)].length;
    return `  ${g[1].padEnd(20)} ${String(objs).padStart(2)} objects`;
  });
  console.log(`\ngroupdefs (${groupStats.length}):`);
  for (const g of groupStats) console.log(g);

  console.log(`\nbitmaps: ${a.bitmapDefs.length} defs (${pngCount} files) + 1 bitmapfont (player-fonts/font.png, 6x8 cells)`);
  console.log("colors: 8 (3 display greens + vis + playlist text tones)");
  console.log("\nOK: every file= present, ids unique, image/group refs resolve, tags known, determinism checked.");
}

main();

/**
 * Built-in mock skin: a mini Winamp3-style player window (275x116).
 *
 * Hand-authored to exercise the whole renderer: background layer, title text,
 * songticker, an animatedlayer strip, six transport buttons (prev/play/pause/
 * stop/next/eject), a seek slider, and a `<groupdef>` instantiated via a
 * `<group>` containing a vis, a status field and the volume slider.
 *
 * Every bitmap is drawn at runtime on an offscreen canvas and shipped as a
 * data-URL PNG — no binary assets. When the real `skin_load` command lands,
 * this module keeps serving as the fallback-skin fixture.
 */

import type { Bitmap, GuiObject, Skin, SkinPayload } from "./types";

// --- window + control metrics (classic-Winamp-ish proportions) ------------

const W = 275;
const H = 116;
const BTN_W = 38;
const BTN_H = 20;
const BTN_Y = 92;
const SEEK_W = 258;
const TRACK_H = 12;
const THUMB = 12;
const GROUP_W = 68;
const GROUP_H = 48;

type Transport = "prev" | "play" | "pause" | "stop" | "next" | "eject";
type ButtonState = "normal" | "hover" | "down";

const TRANSPORTS: readonly Transport[] = ["prev", "play", "pause", "stop", "next", "eject"];
const STATES: readonly ButtonState[] = ["normal", "hover", "down"];
const BTN_X: Record<Transport, number> = {
  prev: 10,
  play: 52,
  pause: 94,
  stop: 136,
  next: 178,
  eject: 226,
};
const BTN_HUE: Record<Transport, number> = {
  prev: 210,
  play: 120,
  pause: 45,
  stop: 0,
  next: 270,
  eject: 175,
};
const TOOLTIP: Record<Transport, string> = {
  prev: "Previous",
  play: "Play",
  pause: "Pause / Resume",
  stop: "Stop",
  next: "Next",
  eject: "Eject",
};

// --- offscreen-canvas bitmap factory ---------------------------------------

function png(w: number, h: number, paint: (ctx: CanvasRenderingContext2D) => void): string {
  const canvas = document.createElement("canvas");
  canvas.width = w;
  canvas.height = h;
  const ctx = canvas.getContext("2d");
  if (!ctx) throw new Error("2D canvas context unavailable");
  paint(ctx);
  return canvas.toDataURL("image/png");
}

function triangle(
  ctx: CanvasRenderingContext2D,
  a: [number, number],
  b: [number, number],
  c: [number, number],
): void {
  ctx.beginPath();
  ctx.moveTo(a[0], a[1]);
  ctx.lineTo(b[0], b[1]);
  ctx.lineTo(c[0], c[1]);
  ctx.closePath();
  ctx.fill();
}

/** Transport glyphs drawn as paths (no font dependency, deterministic). */
function drawGlyph(
  ctx: CanvasRenderingContext2D,
  glyph: Transport,
  w: number,
  h: number,
  color: string,
  dy = 0,
): void {
  const cx = w / 2;
  const cy = h / 2 + dy;
  const s = Math.max(5, Math.min(w, h) * 0.42);
  ctx.fillStyle = color;
  const left = (x: number) =>
    triangle(ctx, [x + s / 2, cy - s / 2], [x + s / 2, cy + s / 2], [x - s / 2, cy]);
  const right = (x: number) =>
    triangle(ctx, [x - s / 2, cy - s / 2], [x - s / 2, cy + s / 2], [x + s / 2, cy]);
  switch (glyph) {
    case "prev":
      left(cx - s * 0.35);
      left(cx + s * 0.35);
      break;
    case "play":
      right(cx);
      break;
    case "pause":
      ctx.fillRect(cx - s * 0.45, cy - s / 2, s * 0.32, s);
      ctx.fillRect(cx + s * 0.13, cy - s / 2, s * 0.32, s);
      break;
    case "stop":
      ctx.fillRect(cx - s / 2, cy - s / 2, s, s);
      break;
    case "next":
      right(cx - s * 0.35);
      right(cx + s * 0.35);
      break;
    case "eject":
      triangle(ctx, [cx - s / 2, cy + s * 0.2], [cx + s / 2, cy + s * 0.2], [cx, cy - s * 0.55]);
      ctx.fillRect(cx - s / 2, cy + s * 0.45, s, s * 0.22);
      break;
  }
}

/** Gradient button face, one per transport × state; distinct hue per button. */
function buttonBitmap(t: Transport, state: ButtonState): string {
  return png(BTN_W, BTN_H, (ctx) => {
    const hue = BTN_HUE[t];
    const light = state === "hover" ? 46 : state === "down" ? 22 : 33;
    const grad = ctx.createLinearGradient(0, 0, 0, BTN_H);
    grad.addColorStop(0, `hsl(${hue}, 46%, ${light + 9}%)`);
    grad.addColorStop(1, `hsl(${hue}, 50%, ${light - 9}%)`);
    ctx.fillStyle = grad;
    ctx.fillRect(0, 0, BTN_W, BTN_H);
    ctx.strokeStyle = `hsl(${hue}, ${state === "down" ? 65 : 35}%, 58%)`;
    ctx.lineWidth = 1;
    ctx.strokeRect(0.5, 0.5, BTN_W - 1, BTN_H - 1);
    // Pressed look: glyph nudged down 1px.
    drawGlyph(ctx, t, BTN_W, BTN_H, state === "down" ? "#dfe3e6" : "#f2f4f5", state === "down" ? 1 : 0);
  });
}

/** Slider groove; `active` is the down/dragging variant. */
function trackBitmap(w: number, h: number, active: boolean): string {
  return png(w, h, (ctx) => {
    const grad = ctx.createLinearGradient(0, 0, 0, h);
    grad.addColorStop(0, active ? "#3a3a42" : "#2e2e36");
    grad.addColorStop(1, "#16161c");
    ctx.fillStyle = grad;
    ctx.fillRect(0, 0, w, h);
    ctx.strokeStyle = active ? "#6a6a76" : "#4a4a54";
    ctx.lineWidth = 1;
    ctx.strokeRect(0.5, 0.5, w - 1, h - 1);
    ctx.fillStyle = active ? "#5c5c68" : "#45454f";
    ctx.fillRect(1, Math.floor(h / 2), w - 2, 1);
  });
}

function thumbBitmap(size: number): string {
  return png(size, size, (ctx) => {
    const grad = ctx.createLinearGradient(0, 0, 0, size);
    grad.addColorStop(0, "#9a9aa6");
    grad.addColorStop(1, "#4c4c58");
    ctx.fillStyle = grad;
    ctx.fillRect(0, 0, size, size);
    ctx.strokeStyle = "#c8c8d0";
    ctx.lineWidth = 1;
    ctx.strokeRect(0.5, 0.5, size - 1, size - 1);
  });
}

function backgroundBitmap(): string {
  return png(W, H, (ctx) => {
    const grad = ctx.createLinearGradient(0, 0, 0, H);
    grad.addColorStop(0, "#34343e");
    grad.addColorStop(0.12, "#202028");
    grad.addColorStop(1, "#0b0b0f");
    ctx.fillStyle = grad;
    ctx.fillRect(0, 0, W, H);
    // Bevel highlight + title-bar divider.
    ctx.fillStyle = "rgba(255,255,255,0.07)";
    ctx.fillRect(1, 1, W - 2, 1);
    ctx.fillStyle = "rgba(0,0,0,0.55)";
    ctx.fillRect(1, 20, W - 2, 1);
    ctx.strokeStyle = "#4d4d58";
    ctx.lineWidth = 1;
    ctx.strokeRect(0.5, 0.5, W - 1, H - 1);
  });
}

/** Inset panel behind the vis/status/volume group. */
function panelBitmap(w: number, h: number): string {
  return png(w, h, (ctx) => {
    const grad = ctx.createLinearGradient(0, 0, 0, h);
    grad.addColorStop(0, "#17171d");
    grad.addColorStop(1, "#101015");
    ctx.fillStyle = grad;
    ctx.fillRect(0, 0, w, h);
    ctx.strokeStyle = "#3c3c46";
    ctx.lineWidth = 1;
    ctx.strokeRect(0.5, 0.5, w - 1, h - 1);
  });
}

/** 5-frame 16x16 strip: an extra bar lights up per frame. */
function animBitmap(): string {
  const fw = 16;
  const frames = 5;
  return png(fw * frames, 16, (ctx) => {
    for (let f = 0; f < frames; f++) {
      for (let b = 0; b < 5; b++) {
        ctx.fillStyle = b <= f ? "#3ccb4e" : "#1d3a22";
        const bh = 3 + b * 2.5;
        ctx.fillRect(f * fw + 2 + b * 3, 16 - 2 - bh, 2, bh);
      }
    }
  });
}

// --- the mock skin ----------------------------------------------------------

function transportButton(t: Transport): GuiObject {
  return {
    kind: "button",
    id: `button.${t}`,
    action: t.toUpperCase(),
    image: `cbutton.${t}`,
    hoverimage: `cbutton.${t}.hover`,
    downimage: `cbutton.${t}.down`,
    tooltip: TOOLTIP[t],
    rect: { x: BTN_X[t], y: BTN_Y, w: BTN_W, h: BTN_H },
  };
}

function buildImages(): Record<string, string> {
  const images: Record<string, string> = {
    "player.background": backgroundBitmap(),
    "player.status.bg": panelBitmap(GROUP_W, GROUP_H),
    "anim.playstate": animBitmap(),
    "seek.track": trackBitmap(SEEK_W, TRACK_H, false),
    "seek.track.down": trackBitmap(SEEK_W, TRACK_H, true),
    "seek.thumb": thumbBitmap(THUMB),
    "vol.track": trackBitmap(GROUP_W - 4, TRACK_H, false),
    "vol.track.down": trackBitmap(GROUP_W - 4, TRACK_H, true),
    "vol.thumb": thumbBitmap(THUMB),
  };
  for (const t of TRANSPORTS) {
    for (const state of STATES) {
      images[`cbutton.${t}${state === "normal" ? "" : `.${state}`}`] = buttonBitmap(t, state);
    }
  }
  return images;
}

export function mockSkinPayload(): SkinPayload {
  const images = buildImages();
  const bitmaps: Bitmap[] = Object.keys(images).map((id) => ({ id, file: `${id}.png` }));

  const skin: Skin = {
    params: { name: "NaVinaMP Fallback", author: "NaVinaMP", version: "0.1" },
    elements: {
      bitmaps,
      bitmap_fonts: [],
      truetype_fonts: [],
      colors: [{ id: "player.green", value: "#3ccb4e" }],
      gammas: [],
    },
    groups: {
      "status.group": {
        id: "status.group",
        background: "player.status.bg",
        rect: { w: GROUP_W, h: GROUP_H },
        objects: [
          {
            kind: "vis",
            id: "vis",
            rect: { x: 2, y: 2, w: GROUP_W - 4, h: 18 },
            params: { mode: "spectrum" },
          },
          {
            kind: "status",
            id: "status",
            rect: { x: 2, y: 22, w: GROUP_W - 4, h: 10 },
          },
          {
            kind: "slider",
            id: "volume",
            action: "VOLUME",
            image: "vol.track",
            downimage: "vol.track.down",
            thumbimage: "vol.thumb",
            rect: { x: 2, y: 34, w: GROUP_W - 4, h: TRACK_H },
            params: { orientation: "horizontal" },
          },
        ],
      },
    },
    containers: [
      {
        id: "main",
        name: "Player",
        layouts: [
          {
            id: "normal",
            rect: { x: 0, y: 0, w: W, h: H },
            objects: [
              {
                kind: "layer",
                id: "background",
                image: "player.background",
                rect: { x: 0, y: 0, w: W, h: H },
              },
              {
                kind: "text",
                id: "title",
                rect: { x: 10, y: 5, w: 96, h: 12 },
                params: {
                  text: "NAVINAMP",
                  color: "#3ccb4e",
                  font: "Verdana, Tahoma, sans-serif",
                  fontsize: "9",
                  bold: "1",
                },
              },
              {
                kind: "text",
                id: "songticker",
                display: "songticker",
                rect: { x: 10, y: 24, w: 180, h: 14 },
                params: {
                  color: "#9adcff",
                  font: "Verdana, Tahoma, sans-serif",
                  fontsize: "8",
                },
              },
              {
                kind: "animatedlayer",
                id: "playstate",
                image: "anim.playstate",
                rect: { x: W - 23, y: 2, w: 16, h: 16 },
                params: { length: "5", framewidth: "16", speed: "150" },
              },
              {
                kind: "group",
                id: "status.group",
                rect: { x: 200, y: 24, w: GROUP_W, h: GROUP_H },
              },
              {
                kind: "slider",
                id: "seeker",
                action: "SEEK",
                image: "seek.track",
                downimage: "seek.track.down",
                thumbimage: "seek.thumb",
                rect: { x: 10, y: 78, w: SEEK_W, h: TRACK_H },
                params: { orientation: "horizontal" },
              },
              ...TRANSPORTS.map(transportButton),
            ],
          },
        ],
      },
    ],
  };

  return { skin, images, warnings: [] };
}

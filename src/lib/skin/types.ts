/**
 * TypeScript mirror of `crates/navinamp-skin/src/model.rs` — the Rust model is
 * the SOURCE OF TRUTH; keep this file in sync with it.
 *
 * Field names are snake_case exactly as serde serializes them. Typed fields
 * are stripped from `params` on the Rust side, so `params` holds every other
 * attribute verbatim. Empty bags/vectors are *omitted* by serde
 * (`skip_serializing_if`), which is why they are optional here. `rect` is
 * always serialized (as `{}` when entirely absent), so it stays required.
 */

/** Attribute bag: every attribute that did not map onto a typed field. */
export type Params = Record<string, string>;

/** Absolute position/size in pixels; absent fields mean the attribute was absent. */
export interface Rect {
  x?: number;
  y?: number;
  w?: number;
  h?: number;
}

/** A rectangular sub-region of one file; absent coordinates = whole image. */
export interface Bitmap {
  id: string;
  file?: string;
  x?: number;
  y?: number;
  w?: number;
  h?: number;
  params?: Params;
}

/** A fixed-width bitmap font. */
export interface BitmapFont {
  id: string;
  file?: string;
  charwidth?: number;
  charheight?: number;
  params?: Params;
}

/** A TrueType font shipped inside the skin. */
export interface TrueTypeFont {
  id: string;
  file?: string;
  params?: Params;
}

/** A named color; `value` is kept raw (skins use several notations). */
export interface ColorElem {
  id: string;
  value?: string;
  params?: Params;
}

/** A `<gamma>` color-scheme group; children preserved generically. */
export interface GammaSet {
  id?: string;
  params?: Params;
  children?: GenericNode[];
}

/** The `<elements>` section: every reusable asset a skin defines. */
export interface Elements {
  bitmaps: Bitmap[];
  bitmap_fonts: BitmapFont[];
  truetype_fonts: TrueTypeFont[];
  colors: ColorElem[];
  gammas: GammaSet[];
  other?: GenericNode[];
}

/** A `<groupdef>`: a reusable group of objects, instantiated via `<group id=…>`. */
export interface GroupDef {
  id: string;
  /** Bitmap id for the group background. */
  background?: string;
  rect: Rect;
  params?: Params;
  objects: GuiObject[];
}

/** A `<container>`: one OS window's worth of layouts. */
export interface Container {
  id: string;
  name?: string;
  params?: Params;
  layouts: Layout[];
  other?: GenericNode[];
}

/** A `<layout>` inside a container: one window mode (normal, shade, ...). */
export interface Layout {
  id: string;
  rect: Rect;
  params?: Params;
  objects: GuiObject[];
}

/** A fully parsed skin: global elements, group definitions and containers. */
export interface Skin {
  params?: Params;
  elements: Elements;
  /** All `<groupdef id="...">` definitions, keyed by id. */
  groups: Record<string, GroupDef>;
  containers: Container[];
  other?: GenericNode[];
}

/** `<layer>`: a static image plane. */
export interface Layer {
  id?: string;
  /** Bitmap id (not a file path). */
  image?: string;
  rect: Rect;
  params?: Params;
}

/** `<animatedlayer>`: an image strip animated by the skin engine. */
export interface AnimatedLayer {
  id?: string;
  image?: string;
  rect: Rect;
  params?: Params;
}

/** `<button>`: a clickable object wired to a standard action (e.g. `PLAY`). */
export interface Button {
  id?: string;
  action?: string;
  image?: string;
  downimage?: string;
  hoverimage?: string;
  tooltip?: string;
  rect: Rect;
  params?: Params;
}

/** `<togglebutton>`: like `Button` with an on/off state. */
export interface ToggleButton {
  id?: string;
  action?: string;
  image?: string;
  downimage?: string;
  hoverimage?: string;
  tooltip?: string;
  rect: Rect;
  params?: Params;
}

/** `<slider>`: a draggable object for seek/volume/balance. */
export interface Slider {
  id?: string;
  action?: string;
  image?: string;
  downimage?: string;
  hoverimage?: string;
  thumbimage?: string;
  rect: Rect;
  params?: Params;
}

/** `<text>`: a text object, e.g. the song ticker (`display="songticker"`). */
export interface Text {
  id?: string;
  display?: string;
  rect: Rect;
  params?: Params;
}

/** `<vis>`: an audio visualizer. */
export interface Vis {
  id?: string;
  rect: Rect;
  params?: Params;
}

/** `<status>`: playback status field. */
export interface Status {
  id?: string;
  rect: Rect;
  params?: Params;
}

/** `<grid>`: a tiled image object. */
export interface Grid {
  id?: string;
  image?: string;
  rect: Rect;
  params?: Params;
}

/** A `<group .../>` instance: a reference to a `<groupdef>` by id. */
export interface GroupInstance {
  id?: string;
  rect: Rect;
  params?: Params;
  children?: GenericNode[];
}

/** A preserved verbatim node for unknown tags. */
export interface GenericNode {
  tag: string;
  text?: string;
  params?: Params;
  children?: GenericNode[];
}

/**
 * Any object that can appear inside a `<groupdef>` or `<layout>`, serialized
 * with a `"kind"` discriminator (serde internally-tagged enum, lowercase).
 */
export type GuiObject =
  | ({ kind: "layer" } & Layer)
  | ({ kind: "animatedlayer" } & AnimatedLayer)
  | ({ kind: "button" } & Button)
  | ({ kind: "togglebutton" } & ToggleButton)
  | ({ kind: "slider" } & Slider)
  | ({ kind: "text" } & Text)
  | ({ kind: "vis" } & Vis)
  | ({ kind: "status" } & Status)
  | ({ kind: "grid" } & Grid)
  | ({ kind: "group" } & GroupInstance)
  | ({ kind: "other" } & GenericNode);

/**
 * Non-fatal problems found while parsing a skin (mirror of `error::Warning`;
 * serde's default externally-tagged representation).
 */
export type Warning =
  | { UnknownTag: { tag: string; parent: string } }
  | { MalformedValue: { element: string; attribute: string; value: string } }
  | { MissingAttribute: { element: string; attribute: string } }
  | { MissingFile: { file: string; referenced_by: string } }
  | { MissingGroup: { id: string; referenced_by: string } }
  | { MissingBitmapId: { id: string; referenced_by: string } }
  | { DuplicateId: { kind: string; id: string } }
  | { GroupCycle: { path: string[] } }
  | { IncludeCycle: { file: string } }
  | { MalformedXml: { file: string; message: string } }
  | { DepthLimit: { file: string; message: string } }
  | { ImageDecode: { file: string; message: string } }
  | { ClassicMissingOptional: { file: string; purpose: string } }
  | {
      CropOutOfRange: {
        bitmap: string;
        file: string;
        x: number;
        y: number;
        w: number;
        h: number;
        image_width: number;
        image_height: number;
      };
    };

/** What the `skin_load` command (and the mock) returns over IPC. */
export interface SkinPayload {
  skin: Skin;
  /** Decoded images keyed by `<bitmap id>` (data URLs). */
  images: Record<string, string>;
  warnings: Warning[];
}

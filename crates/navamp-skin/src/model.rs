//! The normalized skin model that crosses IPC as JSON.
//!
//! Design rules:
//! - Known attributes get typed fields; **every** attribute (known or not)
//!   survives parsing. Typed fields are removed from `params`; anything left
//!   over (including unknown extras like `cfg`, `ghost`, `notify`,
//!   `relatx`/`relaty`, ...) lands in `params` verbatim.
//! - Values that were expected to be numeric but failed to parse stay in
//!   `params` as their raw string and produce a [`Warning::MalformedValue`].
//! - No floats are stored in typed fields (attributes like `alpha` stay in
//!   `params`), which keeps the model `Eq` and JSON round-trip exact.
//! - Unknown object tags are preserved as [`GuiObject::Other`] generic nodes.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::error::SkinError;

/// Attribute bag holding every attribute that did not map onto a typed field.
pub type Params = BTreeMap<String, String>;

/// A fully parsed skin: global elements, group definitions and containers.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Skin {
    /// Attributes on the root `<skin>` element, if any.
    #[serde(flatten, default, skip_serializing_if = "Params::is_empty")]
    pub params: Params,
    /// `<elements>` section: assets referenced by groups and objects.
    pub elements: Elements,
    /// All `<groupdef id="...">` definitions, keyed by id.
    pub groups: std::collections::HashMap<String, GroupDef>,
    /// `<container>`s in document order; windows are selected by id.
    pub containers: Vec<Container>,
    /// Top-level tags the parser does not know, preserved verbatim.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub other: Vec<GenericNode>,
}

impl Skin {
    /// Parses a skin out of an already-opened `.wal` archive.
    ///
    /// Returns the model plus non-fatal [`Warning`]s.
    pub fn from_archive(
        archive: &crate::archive::SkinArchive,
    ) -> Result<(Skin, Vec<crate::error::Warning>), SkinError> {
        crate::parse::SkinParser::parse(archive)
    }

    /// Serializes to JSON (pretty-printed) for IPC to the UI.
    pub fn to_json(&self) -> Result<String, SkinError> {
        Ok(serde_json::to_string_pretty(self)?)
    }
}

/// The `<elements>` section: every reusable asset a skin defines.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Elements {
    pub bitmaps: Vec<Bitmap>,
    pub bitmap_fonts: Vec<BitmapFont>,
    pub truetype_fonts: Vec<TrueTypeFont>,
    pub colors: Vec<ColorElem>,
    pub gammas: Vec<GammaSet>,
    /// Unknown element tags, preserved verbatim.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub other: Vec<GenericNode>,
}

/// A rectangular sub-region of one file. Absent coordinates mean the whole
/// image is used.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Bitmap {
    pub id: String,
    /// Path inside the archive. Checked against the archive at parse time
    /// (case-insensitively); missing files yield [`Warning::MissingFile`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub file: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub x: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub y: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub w: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub h: Option<u32>,
    #[serde(flatten, default, skip_serializing_if = "Params::is_empty")]
    pub params: Params,
}

impl Bitmap {
    /// True when no source rectangle is given (the whole file is the image).
    pub fn is_whole_image(&self) -> bool {
        self.x.is_none() && self.y.is_none() && self.w.is_none() && self.h.is_none()
    }
}

/// A fixed-width bitmap font.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct BitmapFont {
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub file: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub charwidth: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub charheight: Option<u32>,
    #[serde(flatten, default, skip_serializing_if = "Params::is_empty")]
    pub params: Params,
}

/// A TrueType font shipped inside the skin.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TrueTypeFont {
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub file: Option<String>,
    #[serde(flatten, default, skip_serializing_if = "Params::is_empty")]
    pub params: Params,
}

/// A named color; `value` is kept raw (skins use several notations).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ColorElem {
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub value: Option<String>,
    #[serde(flatten, default, skip_serializing_if = "Params::is_empty")]
    pub params: Params,
}

/// A `<gamma>` color-scheme group; children preserved generically.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct GammaSet {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    #[serde(flatten, default, skip_serializing_if = "Params::is_empty")]
    pub params: Params,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub children: Vec<GenericNode>,
}

/// A `<groupdef>`: a reusable group of objects, instantiated via
/// `<group id="..."/>` elsewhere. Instances are *not* resolved here — the
/// model stores references and the renderer resolves (and recursion-guards)
/// them. Cycles and dangling ids are reported as warnings.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct GroupDef {
    pub id: String,
    /// Bitmap id for the group background (checked at parse time).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub background: Option<String>,
    pub rect: Rect,
    #[serde(flatten, default, skip_serializing_if = "Params::is_empty")]
    pub params: Params,
    pub objects: Vec<GuiObject>,
}

/// A `<container>`: one OS window's worth of layouts.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Container {
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(flatten, default, skip_serializing_if = "Params::is_empty")]
    pub params: Params,
    pub layouts: Vec<Layout>,
    /// Unknown container-level children, preserved verbatim.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub other: Vec<GenericNode>,
}

/// A `<layout>` inside a container: one window mode (normal, shade, ...).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Layout {
    pub id: String,
    pub rect: Rect,
    #[serde(flatten, default, skip_serializing_if = "Params::is_empty")]
    pub params: Params,
    pub objects: Vec<GuiObject>,
}

/// Absolute position/size (pixels). `None` = attribute absent; relative
/// `relatx`/`relaty`/`relatw`/`relath` attributes stay in `params`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Rect {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub x: Option<i32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub y: Option<i32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub w: Option<i32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub h: Option<i32>,
}

/// Any object that can appear inside a `<groupdef>` or `<layout>`.
///
/// Known tags get typed variants; anything else is kept as
/// [`GuiObject::Other`] so rendering can skip it gracefully (and so nothing
/// is lost for future versions). Serialized with a `"kind"` discriminator.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum GuiObject {
    Layer(Layer),
    AnimatedLayer(AnimatedLayer),
    Button(Button),
    ToggleButton(ToggleButton),
    Slider(Slider),
    Text(Text),
    Vis(Vis),
    Status(Status),
    Grid(Grid),
    Group(GroupInstance),
    Other(GenericNode),
}

impl GuiObject {
    /// The Wasabi tag this object was parsed from (`"layer"`, `"button"`, ...
    /// or the original tag name for [`GuiObject::Other`]).
    pub fn tag_name(&self) -> &str {
        match self {
            GuiObject::Layer(_) => "layer",
            GuiObject::AnimatedLayer(_) => "animatedlayer",
            GuiObject::Button(_) => "button",
            GuiObject::ToggleButton(_) => "togglebutton",
            GuiObject::Slider(_) => "slider",
            GuiObject::Text(_) => "text",
            GuiObject::Vis(_) => "vis",
            GuiObject::Status(_) => "status",
            GuiObject::Grid(_) => "grid",
            GuiObject::Group(_) => "group",
            GuiObject::Other(node) => &node.tag,
        }
    }

    /// The object's position/size rectangle (absolute pixels; `None` fields
    /// mean the attribute was absent).
    pub fn rect(&self) -> &Rect {
        match self {
            GuiObject::Layer(o) => &o.rect,
            GuiObject::AnimatedLayer(o) => &o.rect,
            GuiObject::Button(o) => &o.rect,
            GuiObject::ToggleButton(o) => &o.rect,
            GuiObject::Slider(o) => &o.rect,
            GuiObject::Text(o) => &o.rect,
            GuiObject::Vis(o) => &o.rect,
            GuiObject::Status(o) => &o.rect,
            GuiObject::Grid(o) => &o.rect,
            GuiObject::Group(o) => &o.rect,
            GuiObject::Other(_) => &DEFAULT_RECT,
        }
    }

    /// The object's `id` attribute, when it carries one.
    pub fn id(&self) -> Option<&str> {
        match self {
            GuiObject::Layer(o) => o.id.as_deref(),
            GuiObject::AnimatedLayer(o) => o.id.as_deref(),
            GuiObject::Button(o) => o.id.as_deref(),
            GuiObject::ToggleButton(o) => o.id.as_deref(),
            GuiObject::Slider(o) => o.id.as_deref(),
            GuiObject::Text(o) => o.id.as_deref(),
            GuiObject::Vis(o) => o.id.as_deref(),
            GuiObject::Status(o) => o.id.as_deref(),
            GuiObject::Grid(o) => o.id.as_deref(),
            GuiObject::Group(o) => o.id.as_deref(),
            GuiObject::Other(o) => o.params.get("id").map(String::as_str),
        }
    }

    /// Image attribute references (bitmap ids) used by this object, in a
    /// stable order; used for reference validation.
    pub fn image_refs(&self) -> Vec<&str> {
        fn push<'a>(v: &mut Vec<&'a str>, s: &'a Option<String>) {
            if let Some(s) = s.as_deref().filter(|s| !s.is_empty()) {
                v.push(s);
            }
        }
        let mut refs = Vec::new();
        match self {
            GuiObject::Layer(o) => push(&mut refs, &o.image),
            GuiObject::AnimatedLayer(o) => push(&mut refs, &o.image),
            GuiObject::Grid(o) => push(&mut refs, &o.image),
            GuiObject::Button(o) => {
                push(&mut refs, &o.image);
                push(&mut refs, &o.downimage);
                push(&mut refs, &o.hoverimage);
            }
            GuiObject::ToggleButton(o) => {
                push(&mut refs, &o.image);
                push(&mut refs, &o.downimage);
                push(&mut refs, &o.hoverimage);
            }
            GuiObject::Slider(o) => {
                push(&mut refs, &o.image);
                push(&mut refs, &o.downimage);
                push(&mut refs, &o.hoverimage);
                push(&mut refs, &o.thumbimage);
            }
            _ => {}
        }
        refs
    }
}

/// `<layer>`: a static image plane.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Layer {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    /// Bitmap id (not a file path).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub image: Option<String>,
    pub rect: Rect,
    #[serde(flatten, default, skip_serializing_if = "Params::is_empty")]
    pub params: Params,
}

/// `<animatedlayer>`: an image strip animated by the skin engine.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct AnimatedLayer {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub image: Option<String>,
    pub rect: Rect,
    #[serde(flatten, default, skip_serializing_if = "Params::is_empty")]
    pub params: Params,
}

/// `<button>`: a clickable object wired to a standard action.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Button {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    /// e.g. `PLAY`, `NEXT`, `EJECT`; kept raw, see [`Button::action_kind`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub action: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub image: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub downimage: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hoverimage: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tooltip: Option<String>,
    pub rect: Rect,
    #[serde(flatten, default, skip_serializing_if = "Params::is_empty")]
    pub params: Params,
}

/// `<togglebutton>`: like [`Button`] with an on/off state.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToggleButton {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub action: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub image: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub downimage: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hoverimage: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tooltip: Option<String>,
    pub rect: Rect,
    #[serde(flatten, default, skip_serializing_if = "Params::is_empty")]
    pub params: Params,
}

/// `<slider>`: a draggable object for seek/volume/balance.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Slider {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub action: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub image: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub downimage: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hoverimage: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thumbimage: Option<String>,
    pub rect: Rect,
    #[serde(flatten, default, skip_serializing_if = "Params::is_empty")]
    pub params: Params,
}

impl Slider {
    /// The well-known slider actions, parsed case-insensitively from `action`.
    pub fn action_kind(&self) -> Option<SliderAction> {
        let action = self.action.as_deref()?.trim();
        let upper = action.to_ascii_uppercase();
        match upper.as_str() {
            "SEEK" => Some(SliderAction::Seek),
            "VOLUME" => Some(SliderAction::Volume),
            "BALANCE" => Some(SliderAction::Balance),
            _ => None,
        }
    }
}

/// The slider actions NavAmp's engine handles natively.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum SliderAction {
    Seek,
    Volume,
    Balance,
}

/// `<text>`: a text object, e.g. the song ticker.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Text {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    /// e.g. `songticker`; kept raw for the renderer to interpret.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub display: Option<String>,
    pub rect: Rect,
    #[serde(flatten, default, skip_serializing_if = "Params::is_empty")]
    pub params: Params,
}

/// `<vis>`: an audio visualizer.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Vis {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    pub rect: Rect,
    #[serde(flatten, default, skip_serializing_if = "Params::is_empty")]
    pub params: Params,
}

/// `<status>`: playback status field.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Status {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    pub rect: Rect,
    #[serde(flatten, default, skip_serializing_if = "Params::is_empty")]
    pub params: Params,
}

/// `<grid>`: a tiled image object.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Grid {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub image: Option<String>,
    pub rect: Rect,
    #[serde(flatten, default, skip_serializing_if = "Params::is_empty")]
    pub params: Params,
}

/// A `<group .../>` instance: a *reference* to a `<groupdef>` by id. Bodies
/// (rare, but legal in Winamp3) are preserved as generic children.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct GroupInstance {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    pub rect: Rect,
    #[serde(flatten, default, skip_serializing_if = "Params::is_empty")]
    pub params: Params,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub children: Vec<GenericNode>,
}

/// A preserved verbatim node for unknown tags: tag name, attributes, text
/// content and children.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct GenericNode {
    pub tag: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    #[serde(flatten, default, skip_serializing_if = "Params::is_empty")]
    pub params: Params,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub children: Vec<GenericNode>,
}

/// Shared empty [`Rect`] (unknown objects have no typed rectangle).
const DEFAULT_RECT: Rect = Rect {
    x: None,
    y: None,
    w: None,
    h: None,
};

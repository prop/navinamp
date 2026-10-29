//! NaVinaMP Wasabi skin engine — parsing + bitmap payloads.
//!
//! Loads Winamp3 `.wal` skin archives (ZIP + `skin.xml` and friends) *and*
//! classic Winamp 2 `.wsz` archives (ZIP of fixed-layout BMPs, synthesized
//! into the same model — see [`synthesize_classic`]), parses them into a
//! normalized, serde-serializable model that the UI renders with absolute
//! positioning, and materializes `<bitmap>` elements into PNG data-URLs.
//! This crate deliberately does no rendering, no MAKI interpretation, and
//! has no audio/tauri/symphonia dependencies or copyrighted assets.
//!
//! # Layout
//!
//! - [`SkinArchive`]: the ZIP layer with case-insensitive file lookup
//! - [`Skin`], [`Elements`], [`GroupDef`], [`Container`], [`GuiObject`], ...:
//!   the normalized model (every unknown attribute is preserved in `params`)
//! - [`bitmap_payloads`]: materializes `<bitmap>` elements into PNG
//!   data-URLs (crop + Wasabi color-key transparency), warnings never fatal
//! - [`is_classic`]/[`synthesize_classic`]: `.wsz` detection and synthesis
//! - [`SkinError`] vs [`Warning`]: only an unusable archive or a malformed
//!   `skin.xml` is fatal; everything else degrades to warnings
//!
//! # Example
//!
//! ```no_run
//! # use std::path::Path;
//! let archive = navinamp_skin::SkinArchive::open(Path::new("skin.wal"))?;
//! let (skin, warnings) = navinamp_skin::Skin::from_archive(&archive)?;
//! for warning in &warnings {
//!     eprintln!("skin warning: {warning}");
//! }
//! let json = skin.to_json()?; // crosses IPC to the UI
//! # Ok::<(), navinamp_skin::SkinError>(())
//! ```

mod archive;
mod error;
mod images;
mod model;
mod parse;
mod validate;
mod wsz;

pub use archive::SkinArchive;
pub use error::{SkinError, Warning};
pub use images::bitmap_payloads;
pub use model::{
    AnimatedLayer, Bitmap, BitmapFont, Button, ColorElem, Container, Elements, GammaSet,
    GenericNode, Grid, GroupDef, GroupInstance, GuiObject, Layer, Layout, Params, Rect, Skin,
    Slider, SliderAction, Status, Text, ToggleButton, TrueTypeFont, Vis,
};
pub use wsz::{is_classic, synthesize as synthesize_classic};

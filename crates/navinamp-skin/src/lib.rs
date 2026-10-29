//! NaVinaMP Wasabi skin engine — parsing + bitmap payloads.
//!
//! Loads Winamp3 `.wal` skin archives (ZIP + `skin.xml` and friends),
//! parses them into a normalized, serde-serializable model that the UI
//! renders with absolute positioning, and materializes `<bitmap>` elements
//! into PNG data-URLs. This crate deliberately does no rendering, no MAKI
//! interpretation, and has no audio/tauri/symphonia dependencies or
//! copyrighted assets.
//!
//! # Layout
//!
//! - [`SkinArchive`]: the ZIP layer with case-insensitive file lookup
//! - [`Skin`], [`Elements`], [`GroupDef`], [`Container`], [`GuiObject`], ...:
//!   the normalized model (every unknown attribute is preserved in `params`)
//! - [`bitmap_payloads`]: materializes `<bitmap>` elements into PNG
//!   data-URLs (crop + Wasabi color-key transparency), warnings never fatal
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

pub use archive::SkinArchive;
pub use error::{SkinError, Warning};
pub use images::bitmap_payloads;
pub use model::{
    AnimatedLayer, Bitmap, BitmapFont, Button, ColorElem, Container, Elements, GammaSet,
    GenericNode, Grid, GroupDef, GroupInstance, GuiObject, Layer, Layout, Params, Rect, Skin,
    Slider, SliderAction, Status, Text, ToggleButton, TrueTypeFont, Vis,
};

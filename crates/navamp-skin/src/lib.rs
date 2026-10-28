//! NavAmp Wasabi skin engine — parsing half.
//!
//! Loads Winamp3 `.wal` skin archives (ZIP + `skin.xml` and friends) and
//! parses them into a normalized, serde-serializable model that the UI
//! renders with absolute positioning. This crate is deliberately *only* a
//! parser: no rendering, no MAKI interpretation, no audio/tauri/symphonia
//! dependencies, and no copyrighted assets.
//!
//! # Layout
//!
//! - [`SkinArchive`]: the ZIP layer with case-insensitive file lookup
//! - [`Skin`], [`Elements`], [`GroupDef`], [`Container`], [`GuiObject`], ...:
//!   the normalized model (every unknown attribute is preserved in `params`)
//! - [`SkinError`] vs [`Warning`]: only an unusable archive or a malformed
//!   `skin.xml` is fatal; everything else degrades to warnings
//!
//! # Example
//!
//! ```no_run
//! # use std::path::Path;
//! let archive = navamp_skin::SkinArchive::open(Path::new("skin.wal"))?;
//! let (skin, warnings) = navamp_skin::Skin::from_archive(&archive)?;
//! for warning in &warnings {
//!     eprintln!("skin warning: {warning}");
//! }
//! let json = skin.to_json()?; // crosses IPC to the UI
//! # Ok::<(), navamp_skin::SkinError>(())
//! ```

mod archive;
mod error;
mod model;
mod parse;
mod validate;

pub use archive::SkinArchive;
pub use error::{SkinError, Warning};
pub use model::{
    AnimatedLayer, Bitmap, BitmapFont, Button, ColorElem, Container, Elements, GammaSet,
    GenericNode, Grid, GroupDef, GroupInstance, GuiObject, Layer, Layout, Params, Rect, Skin,
    Slider, SliderAction, Status, Text, ToggleButton, TrueTypeFont, Vis,
};

//! Error and warning types for the skin engine.
//!
//! [`SkinError`] is fatal: the archive itself or `skin.xml` is unusable.
//! [`Warning`] is non-fatal: something in the skin was unknown, dangling or
//! malformed, but parsing continued (Wasabi skins are frequently sloppy).

use std::fmt;

/// Fatal errors. Anything else the parser encounters is collected as a
/// [`Warning`] and parsing continues.
#[derive(Debug, thiserror::Error)]
pub enum SkinError {
    /// Reading the `.wal` file or an entry inside it failed.
    #[error("i/o error: {0}")]
    Io(#[from] std::io::Error),
    /// The file is not a readable ZIP archive.
    #[error("zip error: {0}")]
    Zip(#[from] zip::result::ZipError),
    /// `skin.xml` itself is not well-formed XML (fatal by design; malformed
    /// *included* XML files only produce a [`Warning::MalformedXml`]).
    #[error("xml parse error in skin.xml: {0}")]
    Xml(#[from] quick_xml::Error),
    /// JSON serialization failed (`Skin::to_json`).
    #[error("json error: {0}")]
    Json(#[from] serde_json::Error),
    /// The archive does not contain a `skin.xml` (searched case-insensitively).
    #[error("skin.xml not found in archive")]
    MissingSkinXml,
    /// A named entry was not found in the archive.
    #[error("file `{0}` not found in skin archive")]
    FileNotFound(String),
}

/// A non-fatal problem found while parsing a skin.
///
/// The skin model is still returned; warnings tell the renderer (and skin
/// authors) what was skipped, dangled or repaired.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum Warning {
    /// A tag (or an attribute key) this parser does not know about. The full
    /// node is still preserved in the model as a generic node.
    UnknownTag { tag: String, parent: String },
    /// An attribute value that should have been numeric (or otherwise
    /// well-formed) was not. The raw value is preserved in `params`.
    MalformedValue {
        element: String,
        attribute: String,
        value: String,
    },
    /// A required attribute was missing (`<bitmap>` without `id`, an
    /// `<include>` without `file`, ...).
    MissingAttribute { element: String, attribute: String },
    /// An element references a file that is not in the archive.
    MissingFile { file: String, referenced_by: String },
    /// A `<group .../>` instance references a group id with no matching
    /// `<groupdef id="...">`. System groups (`wasabi.*`) are exempt.
    MissingGroup { id: String, referenced_by: String },
    /// An object references a bitmap id with no matching `<bitmap id="...">`.
    MissingBitmapId { id: String, referenced_by: String },
    /// Two definitions share an id; the first one wins.
    DuplicateId { kind: String, id: String },
    /// A cycle in group-instances-of-groupdefs (`a -> b -> a`). Resolution is
    /// left to the renderer, which must not recurse.
    GroupCycle { path: Vec<String> },
    /// An `<include file="..."/>` chain loops back on itself.
    IncludeCycle { file: String },
    /// An included XML file was not well-formed (a malformed `skin.xml` is a
    /// fatal [`SkinError::Xml`] instead).
    MalformedXml { file: String, message: String },
    /// Nesting or include depth exceeded the parser's safety limit.
    DepthLimit { file: String, message: String },
    /// A bitmap file exists in the archive but its bytes could not be
    /// decoded as PNG/BMP (or the cropped result could not be re-encoded).
    /// The bitmap gets no image payload.
    ImageDecode { file: String, message: String },
    /// The `x`/`y`/`w`/`h` sub-rectangle of a bitmap lies (partly) outside
    /// the decoded image; the bitmap gets no image payload.
    CropOutOfRange {
        bitmap: String,
        file: String,
        x: u32,
        y: u32,
        w: u32,
        h: u32,
        image_width: u32,
        image_height: u32,
    },
}

impl fmt::Display for Warning {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Warning::UnknownTag { tag, parent } => {
                write!(
                    f,
                    "unknown tag <{tag}> inside <{parent}> (preserved as generic node)"
                )
            }
            Warning::MalformedValue {
                element,
                attribute,
                value,
            } => {
                write!(
                    f,
                    "malformed value `{value}` for attribute `{attribute}` on <{element}>"
                )
            }
            Warning::MissingAttribute { element, attribute } => {
                write!(f, "missing attribute `{attribute}` on <{element}>")
            }
            Warning::MissingFile {
                file,
                referenced_by,
            } => {
                write!(
                    f,
                    "file `{file}` referenced by {referenced_by} is not in the archive"
                )
            }
            Warning::MissingGroup { id, referenced_by } => {
                write!(
                    f,
                    "group instance `{id}` referenced by {referenced_by} has no matching groupdef"
                )
            }
            Warning::MissingBitmapId { id, referenced_by } => {
                write!(
                    f,
                    "bitmap id `{id}` referenced by {referenced_by} has no matching <bitmap>"
                )
            }
            Warning::DuplicateId { kind, id } => {
                write!(
                    f,
                    "duplicate {kind} id `{id}`; keeping the first definition"
                )
            }
            Warning::GroupCycle { path } => {
                write!(f, "group cycle: {}", path.join(" -> "))
            }
            Warning::IncludeCycle { file } => {
                write!(f, "include cycle through `{file}`; skipped")
            }
            Warning::MalformedXml { file, message } => {
                write!(f, "malformed xml in `{file}`: {message}")
            }
            Warning::DepthLimit { file, message } => {
                write!(f, "depth limit in `{file}`: {message}")
            }
            Warning::ImageDecode { file, message } => {
                write!(
                    f,
                    "file `{file}` could not be decoded as png/bmp: {message}"
                )
            }
            Warning::CropOutOfRange {
                bitmap,
                file,
                x,
                y,
                w,
                h,
                image_width,
                image_height,
            } => {
                write!(
                    f,
                    "crop rect {w}x{h}+{x}+{y} of bitmap `{bitmap}` (file `{file}`) \
                     is outside the {image_width}x{image_height} image"
                )
            }
        }
    }
}

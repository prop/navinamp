//! Streaming XML parser: `.wal` bytes in, [`Skin`](crate::Skin) out.
//!
//! Archives that look like classic Winamp 2 skins (no `skin.xml` plus
//! hallmark BMPs) never reach the XML path — they are delegated to the
//! `.wsz` synthesizer in [`crate::wsz`] before `skin.xml` is read, keeping
//! `.wal` behavior byte-for-byte unchanged.
//!
//! - Tag names are matched case-sensitively (the Wasabi convention is
//!   lowercase-first; mixed-case tags in the wild fall into the generic
//!   preserve-and-warn path).
//! - Attribute keys are matched case-insensitively for *typed* fields (so
//!   `File=` still populates `Bitmap::file`) but stored verbatim in `params`.
//! - Every attribute survives: typed fields are removed from the attribute
//!   bag, everything else stays in `params`.
//! - `<include file="..."/>` pulls in additional XML files; include cycles
//!   and malformed included files are warnings, never fatal.

use quick_xml::events::{BytesEnd, BytesStart, Event};
use quick_xml::{Reader, XmlVersion};

use crate::archive::{SKIN_XML, SkinArchive};
use crate::error::{SkinError, Warning};
use crate::model::*;
use crate::validate;

/// Safety limits so a hostile `.wal` can't blow the stack or loop forever.
const MAX_INCLUDE_DEPTH: u32 = 16;
const MAX_XML_DEPTH: u32 = 64;

type Xml<'r> = Reader<&'r [u8]>;

pub(crate) struct SkinParser<'a> {
    archive: &'a SkinArchive,
    skin: Skin,
    warnings: Vec<Warning>,
    /// lowercase archive paths currently being parsed (include-cycle guard).
    include_stack: Vec<String>,
}

/// An attribute bag collected from one XML tag.
#[derive(Debug, Default)]
struct Attrs {
    map: Params,
}

impl Attrs {
    fn from_start(e: &BytesStart<'_>) -> Self {
        let mut map = Params::new();
        for attr in e.attributes() {
            // A malformed single attribute must not kill the parse.
            let Ok(attr) = attr else { continue };
            let key = attr.key.as_ref().to_owned();
            let value = match attr.normalized_value(XmlVersion::Implicit1_0) {
                Ok(v) => v.into_owned(),
                Err(_) => attr.value.as_ref().to_owned(),
            };
            map.insert(key, value);
        }
        Self { map }
    }

    /// Removes an attribute by case-insensitive key; returns the original key
    /// casing with the value.
    fn take(&mut self, name: &str) -> Option<(String, String)> {
        let key = self
            .map
            .keys()
            .find(|k| k.eq_ignore_ascii_case(name))?
            .clone();
        let value = self.map.remove(&key)?;
        Some((key, value))
    }

    fn take_string(&mut self, name: &str) -> Option<String> {
        self.take(name).map(|(_, v)| v).filter(|v| !v.is_empty())
    }

    /// Parses an integer attribute; on failure returns the original key and
    /// the raw value so the caller can restore it into `params`.
    fn take_i32(&mut self, name: &str) -> Result<Option<i32>, (String, String)> {
        let Some((key, raw)) = self.take(name) else {
            return Ok(None);
        };
        raw.trim().parse::<i32>().map(Some).map_err(|_| (key, raw))
    }

    fn take_u32(&mut self, name: &str) -> Result<Option<u32>, (String, String)> {
        let Some((key, raw)) = self.take(name) else {
            return Ok(None);
        };
        raw.trim().parse::<u32>().map(Some).map_err(|_| (key, raw))
    }
}

fn tag_name(e: &BytesStart<'_>) -> String {
    e.name().as_ref().to_owned()
}

fn end_name(e: &BytesEnd<'_>) -> String {
    e.name().as_ref().to_owned()
}

impl<'a> SkinParser<'a> {
    /// Entry point: parse `skin.xml` (+ includes) and validate references.
    ///
    /// Archives without a `skin.xml` that carry classic Winamp 2 hallmark
    /// files are routed to the `.wsz` synthesizer instead, so both formats
    /// produce the same normalized model (see [`crate::wsz`]).
    pub(crate) fn parse(archive: &'a SkinArchive) -> Result<(Skin, Vec<Warning>), SkinError> {
        if crate::wsz::is_classic(archive) {
            return crate::wsz::synthesize(archive);
        }
        let xml = archive.read_file(SKIN_XML).map_err(|e| match e {
            SkinError::FileNotFound(_) => SkinError::MissingSkinXml,
            other => other,
        })?;
        let mut parser = Self {
            archive,
            skin: Skin::default(),
            warnings: Vec::new(),
            include_stack: vec![SKIN_XML.to_lowercase()],
        };
        parser.parse_document(&xml, SKIN_XML, 0)?;
        validate::validate(&parser.skin, archive, &mut parser.warnings);
        dedupe(&mut parser.warnings);
        Ok((parser.skin, parser.warnings))
    }

    fn warn(&mut self, w: Warning) {
        self.warnings.push(w);
    }

    // ---- typed-field helpers (emit MalformedValue and keep the raw value) ----

    fn num_i32(&mut self, attrs: &mut Attrs, name: &str, element: &str) -> Option<i32> {
        match attrs.take_i32(name) {
            Ok(v) => v,
            Err((key, raw)) => {
                attrs.map.insert(key, raw.clone());
                self.warn(Warning::MalformedValue {
                    element: element.to_owned(),
                    attribute: name.to_owned(),
                    value: raw,
                });
                None
            }
        }
    }

    fn num_u32(&mut self, attrs: &mut Attrs, name: &str, element: &str) -> Option<u32> {
        match attrs.take_u32(name) {
            Ok(v) => v,
            Err((key, raw)) => {
                attrs.map.insert(key, raw.clone());
                self.warn(Warning::MalformedValue {
                    element: element.to_owned(),
                    attribute: name.to_owned(),
                    value: raw,
                });
                None
            }
        }
    }

    fn required_id(&mut self, attrs: &mut Attrs, element: &str) -> Option<String> {
        match attrs.take_string("id") {
            Some(id) => Some(id),
            None => {
                self.warn(Warning::MissingAttribute {
                    element: element.to_owned(),
                    attribute: "id".to_owned(),
                });
                None
            }
        }
    }

    fn rect(&mut self, attrs: &mut Attrs, element: &str) -> Rect {
        Rect {
            x: self.num_i32(attrs, "x", element),
            y: self.num_i32(attrs, "y", element),
            w: self.num_i32(attrs, "w", element),
            h: self.num_i32(attrs, "h", element),
        }
    }

    // ---- document structure ----

    fn parse_document(&mut self, xml: &[u8], source: &str, depth: u32) -> Result<(), SkinError> {
        let mut reader = Reader::from_reader(xml);
        reader.config_mut().trim_text(true);
        let mut buf = Vec::new();
        loop {
            match reader.read_event_into(&mut buf)? {
                Event::Start(e) => self.top_tag(&mut reader, &e, false, source, depth)?,
                Event::Empty(e) => self.top_tag(&mut reader, &e, true, source, depth)?,
                Event::Eof => break,
                _ => {}
            }
            buf.clear();
        }
        Ok(())
    }

    fn top_tag(
        &mut self,
        reader: &mut Xml<'_>,
        e: &BytesStart<'_>,
        is_empty: bool,
        source: &str,
        depth: u32,
    ) -> Result<(), SkinError> {
        let tag = tag_name(e);
        match tag.as_str() {
            "skin" => {
                // Root element (also tolerated inside includes): keep its attrs.
                self.skin.params.extend(Attrs::from_start(e).map);
                Ok(())
            }
            "elements" => {
                if !is_empty {
                    self.parse_elements(reader, depth + 1, source)?;
                }
                Ok(())
            }
            "groupdef" => {
                let attrs = Attrs::from_start(e);
                self.parse_groupdef(reader, attrs, is_empty, depth + 1, source)
            }
            "container" => {
                let attrs = Attrs::from_start(e);
                self.parse_container(reader, attrs, is_empty, depth + 1, source)
            }
            "include" => self.handle_include(reader, e, is_empty, source, depth),
            _ => {
                let node = self.parse_generic_node(reader, e, tag.clone(), depth + 1, source)?;
                self.warn(Warning::UnknownTag {
                    tag,
                    parent: "skin".to_owned(),
                });
                self.skin.other.push(node);
                Ok(())
            }
        }
    }

    fn handle_include(
        &mut self,
        reader: &mut Xml<'_>,
        e: &BytesStart<'_>,
        is_empty: bool,
        source: &str,
        depth: u32,
    ) -> Result<(), SkinError> {
        let mut attrs = Attrs::from_start(e);
        if !is_empty {
            skip_to_end(reader, "include")?;
        }
        let Some(file) = attrs.take_string("file") else {
            self.warn(Warning::MissingAttribute {
                element: "include".to_owned(),
                attribute: "file".to_owned(),
            });
            return Ok(());
        };
        let key = file.to_lowercase();
        if self.include_stack.contains(&key) {
            self.warn(Warning::IncludeCycle { file });
            return Ok(());
        }
        if depth + 1 > MAX_INCLUDE_DEPTH {
            self.warn(Warning::DepthLimit {
                file: source.to_owned(),
                message: format!("include chain deeper than {MAX_INCLUDE_DEPTH}"),
            });
            return Ok(());
        }
        let bytes = match self.archive.read_file(&file) {
            Ok(bytes) => bytes,
            Err(SkinError::FileNotFound(_)) => {
                self.warn(Warning::MissingFile {
                    file,
                    referenced_by: "<include>".to_owned(),
                });
                return Ok(());
            }
            Err(e) => return Err(e),
        };
        self.include_stack.push(key);
        let result = self.parse_document(&bytes, &file, depth + 1);
        self.include_stack.pop();
        if let Err(e) = result {
            self.warn(Warning::MalformedXml {
                file,
                message: e.to_string(),
            });
        }
        Ok(())
    }

    fn parse_elements(
        &mut self,
        reader: &mut Xml<'_>,
        depth: u32,
        source: &str,
    ) -> Result<(), SkinError> {
        let mut buf = Vec::new();
        loop {
            match reader.read_event_into(&mut buf)? {
                Event::Start(e) => {
                    let tag = tag_name(&e);
                    let mut attrs = Attrs::from_start(&e);
                    match self.parse_element(&mut attrs, &tag) {
                        ElementParse::Bitmap(bitmap) => {
                            skip_to_end(reader, &tag)?;
                            self.skin.elements.bitmaps.push(bitmap);
                        }
                        ElementParse::BitmapFont(font) => {
                            skip_to_end(reader, &tag)?;
                            self.skin.elements.bitmap_fonts.push(font);
                        }
                        ElementParse::TrueTypeFont(font) => {
                            skip_to_end(reader, &tag)?;
                            self.skin.elements.truetype_fonts.push(font);
                        }
                        ElementParse::Color(color) => {
                            skip_to_end(reader, &tag)?;
                            self.skin.elements.colors.push(color);
                        }
                        ElementParse::Gamma(mut gamma) => {
                            // Start-form gamma: preserve child tags generically.
                            let node =
                                self.parse_generic_node(reader, &e, tag, depth + 1, source)?;
                            gamma.children = node.children;
                            self.skin.elements.gammas.push(gamma);
                        }
                        ElementParse::Unknown => {
                            let node = self.parse_generic_node(
                                reader,
                                &e,
                                tag.clone(),
                                depth + 1,
                                source,
                            )?;
                            self.warn(Warning::UnknownTag {
                                tag,
                                parent: "elements".to_owned(),
                            });
                            self.skin.elements.other.push(node);
                        }
                        ElementParse::Skipped => skip_to_end(reader, &tag)?,
                    }
                }
                Event::Empty(e) => {
                    let tag = tag_name(&e);
                    let mut attrs = Attrs::from_start(&e);
                    match self.parse_element(&mut attrs, &tag) {
                        ElementParse::Bitmap(bitmap) => self.skin.elements.bitmaps.push(bitmap),
                        ElementParse::BitmapFont(font) => {
                            self.skin.elements.bitmap_fonts.push(font)
                        }
                        ElementParse::TrueTypeFont(font) => {
                            self.skin.elements.truetype_fonts.push(font)
                        }
                        ElementParse::Color(color) => self.skin.elements.colors.push(color),
                        ElementParse::Gamma(gamma) => self.skin.elements.gammas.push(gamma),
                        ElementParse::Unknown => {
                            let node = leaf_node(&e, tag.clone());
                            self.warn(Warning::UnknownTag {
                                tag,
                                parent: "elements".to_owned(),
                            });
                            self.skin.elements.other.push(node);
                        }
                        ElementParse::Skipped => {}
                    }
                }
                Event::End(e) if end_name(&e) == "elements" => break,
                Event::Eof => break,
                _ => {}
            }
            buf.clear();
        }
        Ok(())
    }

    /// Builds a typed element from the attribute bag; `Skipped` marks a known
    /// tag missing its required `id` (warning already emitted).
    fn parse_element(&mut self, attrs: &mut Attrs, tag: &str) -> ElementParse {
        match tag {
            "bitmap" => {
                let Some(id) = self.required_id(attrs, "bitmap") else {
                    return ElementParse::Skipped;
                };
                ElementParse::Bitmap(Bitmap {
                    id,
                    file: attrs.take_string("file"),
                    x: self.num_u32(attrs, "x", "bitmap"),
                    y: self.num_u32(attrs, "y", "bitmap"),
                    w: self.num_u32(attrs, "w", "bitmap"),
                    h: self.num_u32(attrs, "h", "bitmap"),
                    params: std::mem::take(&mut attrs.map),
                })
            }
            "bitmapfont" => {
                let Some(id) = self.required_id(attrs, "bitmapfont") else {
                    return ElementParse::Skipped;
                };
                ElementParse::BitmapFont(BitmapFont {
                    id,
                    file: attrs.take_string("file"),
                    charwidth: self.num_u32(attrs, "charwidth", "bitmapfont"),
                    charheight: self.num_u32(attrs, "charheight", "bitmapfont"),
                    params: std::mem::take(&mut attrs.map),
                })
            }
            "truetypefont" => {
                let Some(id) = self.required_id(attrs, "truetypefont") else {
                    return ElementParse::Skipped;
                };
                ElementParse::TrueTypeFont(TrueTypeFont {
                    id,
                    file: attrs.take_string("file"),
                    params: std::mem::take(&mut attrs.map),
                })
            }
            "color" => {
                let Some(id) = self.required_id(attrs, "color") else {
                    return ElementParse::Skipped;
                };
                ElementParse::Color(ColorElem {
                    id,
                    value: attrs.take_string("value"),
                    params: std::mem::take(&mut attrs.map),
                })
            }
            "gamma" => ElementParse::Gamma(GammaSet {
                id: attrs.take_string("id"),
                params: std::mem::take(&mut attrs.map),
                children: Vec::new(),
            }),
            _ => ElementParse::Unknown,
        }
    }

    fn parse_groupdef(
        &mut self,
        reader: &mut Xml<'_>,
        mut attrs: Attrs,
        is_empty: bool,
        depth: u32,
        source: &str,
    ) -> Result<(), SkinError> {
        let id = self.required_id(&mut attrs, "groupdef").unwrap_or_default();
        let label = format!("groupdef `{id}`");
        let background = attrs.take_string("background");
        let rect = self.rect(&mut attrs, "groupdef");
        let objects = if is_empty {
            Vec::new()
        } else {
            self.parse_object_stream(reader, "groupdef", &label, depth, source)?
        };
        if id.is_empty() {
            return Ok(()); // unreferencable; warning already emitted
        }
        if self.skin.groups.contains_key(&id) {
            self.warn(Warning::DuplicateId {
                kind: "groupdef".to_owned(),
                id,
            });
        } else {
            self.skin.groups.insert(
                id.clone(),
                GroupDef {
                    id,
                    background,
                    rect,
                    params: attrs.map,
                    objects,
                },
            );
        }
        Ok(())
    }

    fn parse_container(
        &mut self,
        reader: &mut Xml<'_>,
        mut attrs: Attrs,
        is_empty: bool,
        depth: u32,
        source: &str,
    ) -> Result<(), SkinError> {
        let id = self
            .required_id(&mut attrs, "container")
            .unwrap_or_default();
        let name = attrs.take_string("name");
        let params = attrs.map;
        let mut container = Container {
            id: id.clone(),
            name,
            params,
            layouts: Vec::new(),
            other: Vec::new(),
        };
        if !is_empty {
            let mut buf = Vec::new();
            loop {
                match reader.read_event_into(&mut buf)? {
                    Event::Start(e) => {
                        let tag = tag_name(&e);
                        let mut cattrs = Attrs::from_start(&e);
                        match tag.as_str() {
                            "layout" => {
                                let layout = self.parse_layout(
                                    reader,
                                    &mut cattrs,
                                    false,
                                    depth + 1,
                                    source,
                                )?;
                                container.layouts.push(layout);
                            }
                            "groupdef" => {
                                self.parse_groupdef(reader, cattrs, false, depth + 1, source)?;
                            }
                            _ => {
                                let node = self.parse_generic_node(
                                    reader,
                                    &e,
                                    tag.clone(),
                                    depth + 1,
                                    source,
                                )?;
                                self.warn(Warning::UnknownTag {
                                    tag,
                                    parent: format!("container `{id}`"),
                                });
                                container.other.push(node);
                            }
                        }
                    }
                    Event::Empty(e) => {
                        let tag = tag_name(&e);
                        let mut cattrs = Attrs::from_start(&e);
                        match tag.as_str() {
                            "layout" => {
                                let layout = self.parse_layout(
                                    reader,
                                    &mut cattrs,
                                    true,
                                    depth + 1,
                                    source,
                                )?;
                                container.layouts.push(layout);
                            }
                            "groupdef" => {
                                self.parse_groupdef(reader, cattrs, true, depth + 1, source)?;
                            }
                            _ => {
                                let node = leaf_node(&e, tag.clone());
                                self.warn(Warning::UnknownTag {
                                    tag,
                                    parent: format!("container `{id}`"),
                                });
                                container.other.push(node);
                            }
                        }
                    }
                    Event::End(e) if end_name(&e) == "container" => break,
                    Event::Eof => break,
                    _ => {}
                }
                buf.clear();
            }
        }
        self.skin.containers.push(container);
        Ok(())
    }

    fn parse_layout(
        &mut self,
        reader: &mut Xml<'_>,
        attrs: &mut Attrs,
        is_empty: bool,
        depth: u32,
        source: &str,
    ) -> Result<Layout, SkinError> {
        let id = self.required_id(attrs, "layout").unwrap_or_default();
        let label = format!("layout `{id}`");
        let rect = self.rect(attrs, "layout");
        let params = std::mem::take(&mut attrs.map);
        let objects = if is_empty {
            Vec::new()
        } else {
            self.parse_object_stream(reader, "layout", &label, depth, source)?
        };
        Ok(Layout {
            id,
            rect,
            params,
            objects,
        })
    }

    // ---- objects ----

    fn parse_object_stream(
        &mut self,
        reader: &mut Xml<'_>,
        stop: &str,
        label: &str,
        depth: u32,
        source: &str,
    ) -> Result<Vec<GuiObject>, SkinError> {
        let mut objects = Vec::new();
        let mut buf = Vec::new();
        loop {
            match reader.read_event_into(&mut buf)? {
                Event::Start(e) => {
                    let tag = tag_name(&e);
                    let mut attrs = Attrs::from_start(&e);
                    self.push_object(
                        reader,
                        &e,
                        false,
                        &mut attrs,
                        tag,
                        &mut objects,
                        label,
                        depth,
                        source,
                    )?;
                }
                Event::Empty(e) => {
                    let tag = tag_name(&e);
                    let mut attrs = Attrs::from_start(&e);
                    self.push_object(
                        reader,
                        &e,
                        true,
                        &mut attrs,
                        tag,
                        &mut objects,
                        label,
                        depth,
                        source,
                    )?;
                }
                Event::End(e) if end_name(&e) == stop => break,
                Event::Eof => break,
                _ => {}
            }
            buf.clear();
        }
        Ok(objects)
    }

    /// Builds one object entry (shared by the `Start`/`Empty` arms of
    /// [`parse_object_stream`]). Known tags are typed; `group` becomes an
    /// instance reference; anything else is preserved generically.
    #[allow(clippy::too_many_arguments)]
    fn push_object(
        &mut self,
        reader: &mut Xml<'_>,
        e: &BytesStart<'_>,
        is_empty: bool,
        attrs: &mut Attrs,
        tag: String,
        objects: &mut Vec<GuiObject>,
        label: &str,
        depth: u32,
        source: &str,
    ) -> Result<(), SkinError> {
        match tag.as_str() {
            "layer" => {
                objects.push(GuiObject::Layer(self.build_layer(attrs, "layer")));
                skip_to_end_opt(reader, &tag, is_empty)?;
            }
            "animatedlayer" => {
                let animated = AnimatedLayer {
                    id: attrs.take_string("id"),
                    image: attrs.take_string("image"),
                    rect: self.rect(attrs, "animatedlayer"),
                    params: std::mem::take(&mut attrs.map),
                };
                objects.push(GuiObject::AnimatedLayer(animated));
                skip_to_end_opt(reader, &tag, is_empty)?;
            }
            "button" => {
                objects.push(GuiObject::Button(self.build_button(attrs, "button")));
                skip_to_end_opt(reader, &tag, is_empty)?;
            }
            "togglebutton" => {
                let toggle = ToggleButton {
                    id: attrs.take_string("id"),
                    action: attrs.take_string("action"),
                    image: attrs.take_string("image"),
                    downimage: attrs.take_string("downimage"),
                    hoverimage: attrs.take_string("hoverimage"),
                    tooltip: attrs.take_string("tooltip"),
                    rect: self.rect(attrs, "togglebutton"),
                    params: std::mem::take(&mut attrs.map),
                };
                objects.push(GuiObject::ToggleButton(toggle));
                skip_to_end_opt(reader, &tag, is_empty)?;
            }
            "slider" => {
                let slider = Slider {
                    id: attrs.take_string("id"),
                    action: attrs.take_string("action"),
                    image: attrs.take_string("image"),
                    downimage: attrs.take_string("downimage"),
                    hoverimage: attrs.take_string("hoverimage"),
                    thumbimage: attrs.take_string("thumbimage"),
                    rect: self.rect(attrs, "slider"),
                    params: std::mem::take(&mut attrs.map),
                };
                objects.push(GuiObject::Slider(slider));
                skip_to_end_opt(reader, &tag, is_empty)?;
            }
            "text" => {
                let text = Text {
                    id: attrs.take_string("id"),
                    display: attrs.take_string("display"),
                    rect: self.rect(attrs, "text"),
                    params: std::mem::take(&mut attrs.map),
                };
                objects.push(GuiObject::Text(text));
                skip_to_end_opt(reader, &tag, is_empty)?;
            }
            "vis" => {
                let vis = Vis {
                    id: attrs.take_string("id"),
                    rect: self.rect(attrs, "vis"),
                    params: std::mem::take(&mut attrs.map),
                };
                objects.push(GuiObject::Vis(vis));
                skip_to_end_opt(reader, &tag, is_empty)?;
            }
            "status" => {
                let status = Status {
                    id: attrs.take_string("id"),
                    rect: self.rect(attrs, "status"),
                    params: std::mem::take(&mut attrs.map),
                };
                objects.push(GuiObject::Status(status));
                skip_to_end_opt(reader, &tag, is_empty)?;
            }
            "grid" => {
                let grid = Grid {
                    id: attrs.take_string("id"),
                    image: attrs.take_string("image"),
                    rect: self.rect(attrs, "grid"),
                    params: std::mem::take(&mut attrs.map),
                };
                objects.push(GuiObject::Grid(grid));
                skip_to_end_opt(reader, &tag, is_empty)?;
            }
            "group" => {
                let children = if is_empty {
                    Vec::new()
                } else {
                    self.parse_generic_node(reader, e, tag.clone(), depth + 1, source)?
                        .children
                };
                let instance = GroupInstance {
                    id: attrs.take_string("id"),
                    rect: self.rect(attrs, "group"),
                    params: std::mem::take(&mut attrs.map),
                    children,
                };
                objects.push(GuiObject::Group(instance));
            }
            "groupdef" => {
                let taken = std::mem::take(attrs);
                self.parse_groupdef(reader, taken, is_empty, depth + 1, source)?;
            }
            _ => {
                let node = if is_empty {
                    leaf_node(e, tag.clone())
                } else {
                    self.parse_generic_node(reader, e, tag.clone(), depth + 1, source)?
                };
                self.warn(Warning::UnknownTag {
                    tag,
                    parent: label.to_owned(),
                });
                objects.push(GuiObject::Other(node));
            }
        }
        Ok(())
    }

    /// Builds a `Layer` or `AnimatedLayer` (identical attribute shape).
    fn build_layer(&mut self, attrs: &mut Attrs, element: &str) -> Layer {
        Layer {
            id: attrs.take_string("id"),
            image: attrs.take_string("image"),
            rect: self.rect(attrs, element),
            params: std::mem::take(&mut attrs.map),
        }
    }

    /// Builds a `Button` or `ToggleButton` (identical attribute shape).
    fn build_button(&mut self, attrs: &mut Attrs, element: &str) -> Button {
        Button {
            id: attrs.take_string("id"),
            action: attrs.take_string("action"),
            image: attrs.take_string("image"),
            downimage: attrs.take_string("downimage"),
            hoverimage: attrs.take_string("hoverimage"),
            tooltip: attrs.take_string("tooltip"),
            rect: self.rect(attrs, element),
            params: std::mem::take(&mut attrs.map),
        }
    }

    // ---- generic preservation ----

    /// Consumes a whole subtree (from the already-read `Start` event to its
    /// matching `End`) preserving tag, attributes, text and children.
    fn parse_generic_node(
        &mut self,
        reader: &mut Xml<'_>,
        e: &BytesStart<'_>,
        tag: String,
        depth: u32,
        source: &str,
    ) -> Result<GenericNode, SkinError> {
        let mut node = GenericNode {
            tag,
            text: None,
            params: Attrs::from_start(e).map,
            children: Vec::new(),
        };
        if depth > MAX_XML_DEPTH {
            self.warn(Warning::DepthLimit {
                file: source.to_owned(),
                message: format!(
                    "xml nesting deeper than {MAX_XML_DEPTH} below <{}>",
                    node.tag
                ),
            });
            skip_to_end(reader, &node.tag)?;
            return Ok(node);
        }
        let mut buf = Vec::new();
        loop {
            match reader.read_event_into(&mut buf)? {
                Event::Start(child) => {
                    let child_tag = tag_name(&child);
                    node.children.push(self.parse_generic_node(
                        reader,
                        &child,
                        child_tag,
                        depth + 1,
                        source,
                    )?);
                }
                Event::Empty(child) => {
                    let child_tag = tag_name(&child);
                    node.children.push(leaf_node(&child, child_tag));
                }
                Event::Text(t) => {
                    let raw = t.xml_content(XmlVersion::Implicit1_0);
                    let unescaped = quick_xml::escape::unescape(&raw).map(|c| c.into_owned());
                    let text = match unescaped {
                        Ok(text) => text,
                        Err(_) => raw.into_owned(),
                    };
                    let trimmed = text.trim();
                    if !trimmed.is_empty() {
                        node.text = Some(trimmed.to_owned());
                    }
                }
                Event::End(end) if end_name(&end) == node.tag => break,
                Event::Eof => break,
                _ => {}
            }
            buf.clear();
        }
        Ok(node)
    }
}

/// Result of building one `<elements>` child.
enum ElementParse {
    Bitmap(Bitmap),
    BitmapFont(BitmapFont),
    TrueTypeFont(TrueTypeFont),
    Color(ColorElem),
    Gamma(GammaSet),
    /// Unknown tag: preserve generically.
    Unknown,
    /// Known tag but unusable (e.g. missing `id`); consume and move on.
    Skipped,
}

/// Consumes events up to (and including) the matching `End` for `tag`,
/// accounting for nested tags of the same name.
fn skip_to_end(reader: &mut Xml<'_>, tag: &str) -> Result<(), SkinError> {
    let mut depth = 1usize;
    let mut buf = Vec::new();
    while depth > 0 {
        match reader.read_event_into(&mut buf)? {
            Event::Start(e) => {
                if tag_name(&e) == tag {
                    depth += 1;
                }
            }
            Event::End(e) => {
                if end_name(&e) == tag {
                    depth -= 1;
                }
            }
            Event::Eof => break,
            _ => {}
        }
        buf.clear();
    }
    Ok(())
}

/// Like [`skip_to_end`], but a no-op for already-closed (`Empty`) tags.
fn skip_to_end_opt(reader: &mut Xml<'_>, tag: &str, is_empty: bool) -> Result<(), SkinError> {
    if is_empty {
        Ok(())
    } else {
        skip_to_end(reader, tag)
    }
}

/// A preserved leaf node from an `Empty` tag.
fn leaf_node(e: &BytesStart<'_>, tag: String) -> GenericNode {
    GenericNode {
        tag,
        text: None,
        params: Attrs::from_start(e).map,
        children: Vec::new(),
    }
}

/// Removes exact duplicate warnings (e.g. one message per repeated unknown
/// tag) while keeping first-occurrence order.
fn dedupe(warnings: &mut Vec<Warning>) {
    let mut seen: Vec<Warning> = Vec::new();
    warnings.retain(|w| {
        if seen.contains(w) {
            false
        } else {
            seen.push(w.clone());
            true
        }
    });
}

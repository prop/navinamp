//! Post-parse reference validation: everything here produces
//! [`Warning`]s — never errors and never model changes.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use crate::archive::SkinArchive;
use crate::error::Warning;
use crate::model::{GuiObject, Skin};

/// Group ids resolved by the skin engine itself (system library groups), not
/// required to exist as `<groupdef>`s in this skin.
fn is_system_group(id: &str) -> bool {
    id.starts_with("wasabi.")
}

pub(crate) fn validate(skin: &Skin, archive: &SkinArchive, warnings: &mut Vec<Warning>) {
    check_element_files(skin, archive, warnings);

    let bitmap_ids: BTreeSet<&str> = skin
        .elements
        .bitmaps
        .iter()
        .map(|b| b.id.as_str())
        .collect();

    let mut group_ids: Vec<&String> = skin.groups.keys().collect();
    group_ids.sort();
    for id in group_ids {
        let def = &skin.groups[id];
        if let Some(bg) = def.background.as_deref().filter(|bg| !bg.is_empty())
            && !bitmap_ids.contains(bg)
            && !is_system_group(bg)
        {
            warnings.push(Warning::MissingBitmapId {
                id: bg.to_owned(),
                referenced_by: format!("groupdef `{id}`"),
            });
        }
        check_objects(
            &def.objects,
            &format!("groupdef `{id}`"),
            &bitmap_ids,
            &skin.groups,
            warnings,
        );
    }
    for container in &skin.containers {
        for layout in &container.layouts {
            let context = format!("container `{}` / layout `{}`", container.id, layout.id);
            check_objects(
                &layout.objects,
                &context,
                &bitmap_ids,
                &skin.groups,
                warnings,
            );
        }
    }

    detect_group_cycles(skin, warnings);
}

/// Element `file` attributes must exist in the archive (case-insensitive).
fn check_element_files(skin: &Skin, archive: &SkinArchive, warnings: &mut Vec<Warning>) {
    let check = |file: &Option<String>, element: String, warnings: &mut Vec<Warning>| {
        if let Some(file) = file.as_deref().filter(|f| !f.is_empty())
            && !archive.contains(file)
        {
            warnings.push(Warning::MissingFile {
                file: file.to_owned(),
                referenced_by: element,
            });
        }
    };
    for bitmap in &skin.elements.bitmaps {
        check(&bitmap.file, format!("bitmap `{}`", bitmap.id), warnings);
    }
    for font in &skin.elements.bitmap_fonts {
        check(&font.file, format!("bitmapfont `{}`", font.id), warnings);
    }
    for font in &skin.elements.truetype_fonts {
        check(&font.file, format!("truetypefont `{}`", font.id), warnings);
    }
}

fn check_objects(
    objects: &[GuiObject],
    context: &str,
    bitmap_ids: &BTreeSet<&str>,
    groups: &HashMap<String, crate::model::GroupDef>,
    warnings: &mut Vec<Warning>,
) {
    for object in objects {
        // bitmap id references on image attributes
        for image in object.image_refs() {
            if !bitmap_ids.contains(image) && !is_system_group(image) {
                warnings.push(Warning::MissingBitmapId {
                    id: image.to_owned(),
                    referenced_by: format!(
                        "{context}: {} `{}`",
                        object.tag_name(),
                        object.id().unwrap_or("<unnamed>")
                    ),
                });
            }
        }
        // group instance references
        if let GuiObject::Group(instance) = object
            && let Some(id) = instance.id.as_deref().filter(|id| !id.is_empty())
            && !groups.contains_key(id)
            && !is_system_group(id)
        {
            warnings.push(Warning::MissingGroup {
                id: id.to_owned(),
                referenced_by: context.to_owned(),
            });
        }
    }
}

/// Detects `group` -> `groupdef` -> `group` cycles among groupdefs.
fn detect_group_cycles(skin: &Skin, warnings: &mut Vec<Warning>) {
    let mut edges: BTreeMap<&str, BTreeSet<&str>> = BTreeMap::new();
    for (id, def) in &skin.groups {
        for object in &def.objects {
            if let GuiObject::Group(instance) = object
                && let Some(referenced) = instance.id.as_deref().filter(|r| !r.is_empty())
                && skin.groups.contains_key(referenced)
            {
                edges.entry(id.as_str()).or_default().insert(referenced);
            }
        }
    }

    let mut scan = CycleScan {
        edges: &edges,
        color: edges.keys().map(|&k| (k, WHITE)).collect(),
        path: Vec::new(),
        seen: BTreeSet::new(),
        depth_limited: false,
    };
    for root in edges.keys().copied().collect::<Vec<_>>() {
        if scan.color.get(root).copied().unwrap_or(BLACK) == WHITE {
            scan.visit(root, warnings, 0);
        }
    }
}

/// Three-color DFS state for the group reference graph.
struct CycleScan<'e> {
    edges: &'e BTreeMap<&'e str, BTreeSet<&'e str>>,
    /// WHITE = unvisited, GRAY = on the current path, BLACK = done.
    color: HashMap<&'e str, u8>,
    path: Vec<&'e str>,
    seen: BTreeSet<Vec<String>>,
    depth_limited: bool,
}

const WHITE: u8 = 0;
const GRAY: u8 = 1;
const BLACK: u8 = 2;

/// Recursion cap; real skins have shallow graphs, hostile ones don't get to
/// blow the stack.
const MAX_DFS_DEPTH: usize = 512;

impl<'e> CycleScan<'e> {
    fn visit(&mut self, node: &'e str, warnings: &mut Vec<Warning>, depth: usize) {
        if depth > MAX_DFS_DEPTH {
            if !self.depth_limited {
                self.depth_limited = true;
                warnings.push(Warning::DepthLimit {
                    file: "skin.xml".to_owned(),
                    message: "group reference graph too deep; cycle scan truncated".to_owned(),
                });
            }
            return;
        }
        self.color.insert(node, GRAY);
        self.path.push(node);
        if let Some(successors) = self.edges.get(node) {
            for &next in successors {
                match self.color.get(next).copied().unwrap_or(BLACK) {
                    GRAY => {
                        let cycle = self.closed_cycle(next);
                        if self.seen.insert(cycle.clone()) {
                            warnings.push(Warning::GroupCycle { path: cycle });
                        }
                    }
                    WHITE => self.visit(next, warnings, depth + 1),
                    _ => {}
                }
            }
        }
        self.path.pop();
        self.color.insert(node, BLACK);
    }

    /// Builds the canonical `next -> ... -> next` path for a found cycle,
    /// rotated so the smallest id leads (dedupes rotated duplicates).
    fn closed_cycle(&self, next: &str) -> Vec<String> {
        let start = self.path.iter().position(|p| *p == next).unwrap_or(0);
        let mut cycle: Vec<String> = self.path[start..].iter().map(|s| (*s).to_owned()).collect();
        cycle.push(next.to_owned());
        let min_pos = cycle[..cycle.len() - 1]
            .iter()
            .enumerate()
            .min_by(|a, b| a.1.cmp(b.1))
            .map(|(i, _)| i)
            .unwrap_or(0);
        cycle.rotate_left(min_pos);
        cycle
    }
}

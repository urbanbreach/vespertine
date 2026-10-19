//! The Metal shader header (`vespertine_apple/src/scene.h`) is generated once with cbindgen
//! (a build tool that is not a dependency of this repository, see the build script of `vespertine_apple`) and
//! committed. These tests check it against the Rust definitions on every platform: each struct
//! of the header has the size and alignment of its Rust counterpart, so a field added to or
//! changed in a scene type without regenerating the header fails here instead of silently
//! mis-rendering on macOS.

use crate::scene::PathVertex;
use crate::{
    AtlasTextureId, AtlasTile, Background, Bounds, ContentMask, Corners, DevicePixels, Edges, Hsla,
    ImageEffects, LinearColorStop, MonochromeSprite, Point, PolychromeSprite, Quad, ScaledPixels,
    ScopeParams, Shadow, Size, TileId, TransformationMatrix, Underline,
};
use std::{
    collections::HashMap,
    mem::{align_of, size_of},
};

const HEADER: &str = include_str!("../../vespertine_apple/src/scene.h");

/// `(size, alignment)` of every header type.
struct Layouts {
    scalars: HashMap<String, (usize, usize)>,
    structs: HashMap<String, (usize, usize)>,
}

fn scalar(name: &str) -> Option<(usize, usize)> {
    match name {
        "float" | "uint32_t" | "int32_t" => Some((4, 4)),
        "uint64_t" | "int64_t" | "double" => Some((8, 8)),
        "uint8_t" | "int8_t" => Some((1, 1)),
        "uint16_t" | "int16_t" => Some((2, 2)),
        _ => None,
    }
}

impl Layouts {
    fn type_layout(&self, spelled: &str) -> (usize, usize) {
        let spelled = spelled.trim();
        if let Some(name) = spelled.strip_prefix("struct ") {
            return *self
                .structs
                .get(name.trim())
                .unwrap_or_else(|| panic!("struct {name} is used before its definition"));
        }
        if spelled.starts_with("enum ") {
            return (4, 4);
        }
        scalar(spelled)
            .or_else(|| self.scalars.get(spelled).copied())
            .or_else(|| self.structs.get(spelled).copied())
            .unwrap_or_else(|| panic!("unknown header type `{spelled}`"))
    }
}

fn strip_comments(text: &str) -> String {
    let mut out = String::new();
    let mut rest = text;
    while let Some(start) = rest.find("/*") {
        out.push_str(&rest[..start]);
        let end = rest[start..].find("*/").expect("unterminated comment") + start + 2;
        rest = &rest[end..];
    }
    out.push_str(rest);
    out
}

fn parse(header: &str) -> Layouts {
    let text = strip_comments(header);
    let mut layouts = Layouts {
        scalars: HashMap::new(),
        structs: HashMap::new(),
    };
    let mut lines = text.lines().map(str::trim).filter(|line| !line.is_empty());
    while let Some(line) = lines.next() {
        if let Some(rest) = line.strip_prefix("typedef struct ") {
            if rest.ends_with('{') {
                let name = rest.trim_end_matches('{').trim().to_string();
                let (mut offset, mut align) = (0usize, 1usize);
                for field in lines.by_ref() {
                    if field.starts_with('}') {
                        break;
                    }
                    let declaration = field.trim_end_matches(';');
                    let (ty, field_name) = declaration
                        .rsplit_once(' ')
                        .unwrap_or_else(|| panic!("bad field `{field}`"));
                    let count: usize = field_name
                        .split('[')
                        .skip(1)
                        .map(|dim| {
                            dim.trim_end_matches(']')
                                .parse::<usize>()
                                .expect("array length")
                        })
                        .product();
                    let (size, field_align) = layouts.type_layout(ty);
                    offset = offset.next_multiple_of(field_align) + size * count;
                    align = align.max(field_align);
                }
                layouts
                    .structs
                    .insert(name, (offset.next_multiple_of(align), align));
            } else if let Some((target, alias)) = rest.rsplit_once(' ') {
                // `typedef struct Point_f32 PointF;` or an opaque `typedef struct Rems Rems;`
                if let Some(layout) = layouts.structs.get(target.trim()).copied() {
                    layouts
                        .structs
                        .insert(alias.trim_end_matches(';').to_string(), layout);
                }
            }
        } else if let Some(rest) = line.strip_prefix("typedef ")
            && !rest.starts_with("enum")
            && rest.ends_with(';')
        {
            let (ty, name) = rest
                .trim_end_matches(';')
                .rsplit_once(' ')
                .expect("typedef");
            let layout = layouts.type_layout(ty);
            layouts.scalars.insert(name.to_string(), layout);
        }
    }
    layouts
}

fn rust<T>() -> (usize, usize) {
    (size_of::<T>(), align_of::<T>())
}

#[test]
fn every_struct_of_the_shader_header_has_the_layout_of_its_rust_type() {
    let layouts = parse(HEADER);
    let cases: [(&str, (usize, usize)); 25] = [
        ("Quad", rust::<Quad>()),
        ("Shadow", rust::<Shadow>()),
        ("Underline", rust::<Underline>()),
        ("MonochromeSprite", rust::<MonochromeSprite>()),
        ("PolychromeSprite", rust::<PolychromeSprite>()),
        ("ImageEffects", rust::<ImageEffects>()),
        ("ScopeParams", rust::<ScopeParams>()),
        ("TransformationMatrix", rust::<TransformationMatrix>()),
        ("Background", rust::<Background>()),
        ("LinearColorStop", rust::<LinearColorStop>()),
        ("Hsla", rust::<Hsla>()),
        ("AtlasTile", rust::<AtlasTile>()),
        ("AtlasTextureId", rust::<AtlasTextureId>()),
        ("TileId", rust::<TileId>()),
        ("Point_f32", rust::<Point<f32>>()),
        ("Point_ScaledPixels", rust::<Point<ScaledPixels>>()),
        ("Size_ScaledPixels", rust::<Size<ScaledPixels>>()),
        ("Bounds_ScaledPixels", rust::<Bounds<ScaledPixels>>()),
        ("Corners_ScaledPixels", rust::<Corners<ScaledPixels>>()),
        ("Edges_ScaledPixels", rust::<Edges<ScaledPixels>>()),
        (
            "ContentMask_ScaledPixels",
            rust::<ContentMask<ScaledPixels>>(),
        ),
        ("Point_DevicePixels", rust::<Point<DevicePixels>>()),
        ("Bounds_DevicePixels", rust::<Bounds<DevicePixels>>()),
        ("Size_DevicePixels", rust::<Size<DevicePixels>>()),
        (
            "PathVertex_ScaledPixels",
            rust::<PathVertex<ScaledPixels>>(),
        ),
    ];
    for (name, expected) in cases {
        let header = layouts
            .structs
            .get(name)
            .unwrap_or_else(|| panic!("the header does not define {name}"));
        assert_eq!(
            *header, expected,
            "{name}: (size, alignment) in scene.h versus the Rust type; regenerate scene.h"
        );
    }
}

#[test]
fn the_header_parser_computes_c_layouts() {
    let layouts = parse(
        "typedef float ScaledPixels;\n\
         typedef struct A {\n  uint32_t a;\n  float b[2][2];\n  uint64_t c;\n} A;\n\
         typedef struct B {\n  uint8_t a;\n  struct A inner;\n} B;\n",
    );
    assert_eq!(layouts.structs["A"], (32, 8));
    assert_eq!(layouts.structs["B"], (40, 8));
}

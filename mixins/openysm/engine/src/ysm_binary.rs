//! The YSM binary model that an encrypted `.ysm` file carries (see `ysm::decrypt_file`),
//! formats 16 to 32. Ported from Sparkle-Morpher's `YSMBinaryDeserializer` (MIT,
//! Copyright (c) 2026 Marciken). Geometry is stored baked, animations as keyframes.
use crate::{Bone, Quad};
use serde_json::{Map, Value};

/// A keyframe component or a blend weight: a number or Molang text.
#[derive(Clone, Debug, PartialEq)]
pub enum Scalar {
    Number(f32),
    Text(String),
}
pub struct Face {
    pub normal: [f32; 3],
    pub positions: [[f32; 3]; 4],
    pub u: [f32; 4],
    pub v: [f32; 4],
}
pub struct RawBone {
    pub name: String,
    /// Empty for a root.
    pub parent: String,
    pub pivot: [f32; 3],
    pub rotation: [f32; 3],
    pub cubes: Vec<Vec<Face>>,
}
pub struct Geometry {
    pub identifier: String,
    pub texture_width: f32,
    pub texture_height: f32,
    pub bones: Vec<RawBone>,
}
pub struct Keyframe {
    /// Seconds.
    pub time: f32,
    /// 0 linear, 1 a keyframe object without a mode, 2 Catmull-Rom, 3 Bezier.
    pub mode: u32,
    pub pre: Option<[Scalar; 3]>,
    pub post: [Scalar; 3],
}
pub struct BoneAnimation {
    pub bone: String,
    /// Rotation, position, scale.
    pub channels: [Vec<Keyframe>; 3],
}
pub struct Animation {
    pub name: String,
    /// Seconds; infinite when the source gave no length.
    pub length: f32,
    /// 0 no, 1 loop, 2 not stated, 3 hold on the last frame.
    pub loop_mode: u32,
    pub blend_weight: Option<Scalar>,
    pub bones: Vec<BoneAnimation>,
    pub timeline: Vec<(f32, Vec<String>)>,
    pub sounds: Vec<(String, f32)>,
}
pub struct AnimationFile {
    /// 1 main, 2 arm, 3 extra, ...: see `role`.
    pub kind: u32,
    pub animations: Vec<Animation>,
}
pub struct Texture {
    pub name: String,
    pub width: u32,
    pub height: u32,
    /// The encoding of `data`: 1 BMP, 2 PNG, 3 JPEG, 4 WebP, 5 AVIF.
    pub format: u32,
    pub data: Vec<u8>,
}
pub struct Footer {
    pub version: u32,
    pub rand: String,
    pub time: u64,
    pub extra: String,
}
pub struct BinaryModel {
    pub format: u32,
    pub name: String,
    pub default_texture: String,
    pub main: Option<Geometry>,
    pub arm: Option<Geometry>,
    pub animations: Vec<AnimationFile>,
    pub textures: Vec<Texture>,
    pub footer: Option<Footer>,
    // Read past, not kept.
    pub vehicles: usize,
    pub projectiles: usize,
    pub controllers: usize,
    pub sounds: usize,
}
struct Reader<'a> {
    data: &'a [u8],
    at: usize,
    format: u32,
}
impl<'a> Reader<'a> {
    fn take(&mut self, length: usize) -> Result<&'a [u8], String> {
        let end = self.at.checked_add(length).filter(|end| *end <= self.data.len()).ok_or("truncated YSM binary data")?;
        let bytes = &self.data[self.at..end];
        self.at = end;
        Ok(bytes)
    }
    fn byte(&mut self) -> Result<u8, String> {
        Ok(self.take(1)?[0])
    }
    fn float(&mut self) -> Result<f32, String> {
        Ok(f32::from_le_bytes(self.take(4)?.try_into().unwrap()))
    }
    fn vector(&mut self) -> Result<[f32; 3], String> {
        Ok([self.float()?, self.float()?, self.float()?])
    }
    fn varint(&mut self) -> Result<u32, String> {
        let mut value = 0u32;
        for shift in [0, 7, 14, 21, 28] {
            let byte = self.byte()?;
            if shift == 28 && byte & 0xF0 != 0 {
                break;
            }
            value |= u32::from(byte & 0x7F) << shift;
            if byte & 0x80 == 0 {
                return Ok(value);
            }
        }
        Err("invalid integer in YSM binary data".into())
    }
    fn varlong(&mut self) -> Result<u64, String> {
        let mut value = 0u64;
        for shift in (0..64).step_by(7) {
            let byte = self.byte()?;
            value |= u64::from(byte & 0x7F) << shift;
            if byte & 0x80 == 0 {
                return Ok(value);
            }
        }
        Err("invalid integer in YSM binary data".into())
    }
    fn flag(&mut self) -> Result<bool, String> {
        Ok(self.varint()? != 0)
    }
    /// A count of items that each take at least `least` bytes: bounded by what is left.
    fn count(&mut self, least: usize) -> Result<usize, String> {
        let count = self.varint()? as usize;
        if count > (self.data.len() - self.at) / least {
            return Err("count exceeds the YSM binary data".into());
        }
        Ok(count)
    }
    fn bytes(&mut self) -> Result<&'a [u8], String> {
        let length = self.varint()? as usize;
        self.take(length)
    }
    fn string(&mut self) -> Result<String, String> {
        Ok(String::from_utf8_lossy(self.bytes()?).into_owned())
    }
    fn strings(&mut self) -> Result<Vec<String>, String> {
        (0..self.count(1)?).map(|_| self.string()).collect()
    }
    /// A count, then that many pairs of strings.
    fn skip_pairs(&mut self) -> Result<(), String> {
        for _ in 0..self.count(2)? {
            self.bytes()?;
            self.bytes()?;
        }
        Ok(())
    }
    fn scalar(&mut self) -> Result<Scalar, String> {
        match self.byte()? {
            1 => Ok(Scalar::Number(self.float()?)),
            2 => Ok(Scalar::Text(self.string()?)),
            kind => Err(format!("unknown value kind {kind} in YSM binary data")),
        }
    }
    fn scalars(&mut self) -> Result<[Scalar; 3], String> {
        Ok([self.scalar()?, self.scalar()?, self.scalar()?])
    }
    /// Encoded bytes, width, height, format and a flag.
    fn skip_image(&mut self) -> Result<(), String> {
        self.bytes()?;
        for _ in 0..4 {
            self.varint()?;
        }
        Ok(())
    }
    fn geometry(&mut self) -> Result<Geometry, String> {
        let mut bones = Vec::new();
        for _ in 0..self.count(34)? {
            let parent = self.string()?;
            let mut cubes = Vec::new();
            for _ in 0..self.count(4)? {
                let mut faces = Vec::new();
                for _ in 0..self.count(92)? {
                    let mut face = Face { normal: self.vector()?, positions: [[0.; 3]; 4], u: [0.; 4], v: [0.; 4] };
                    for corner in 0..4 {
                        face.positions[corner] = self.vector()?;
                        face.u[corner] = self.float()?;
                        face.v[corner] = self.float()?;
                    }
                    faces.push(face);
                }
                for _ in 0..3 {
                    self.varint()?;
                }
                cubes.push(faces);
            }
            let name = self.string()?;
            for _ in 0..5 {
                self.varint()?;
            }
            bones.push(RawBone { name, parent, pivot: self.vector()?, rotation: self.vector()?, cubes });
        }
        let identifier = self.string()?;
        let texture_height = self.float()?;
        let texture_width = self.float()?;
        // Visible bounds: height, width, an offset list; then two floats.
        self.float()?;
        self.float()?;
        for _ in 0..self.count(4)? {
            self.float()?;
        }
        self.float()?;
        self.float()?;
        if self.flag()? {
            // Model information of the oldest packs: name, tips, extra animations, authors, license, free.
            self.bytes()?;
            self.bytes()?;
            self.strings()?;
            self.strings()?;
            self.bytes()?;
            self.varint()?;
        }
        for _ in 0..3 {
            self.varint()?;
        }
        Ok(Geometry { identifier, texture_width, texture_height, bones })
    }
    fn channel(&mut self) -> Result<Vec<Keyframe>, String> {
        let mut keys = Vec::new();
        for _ in 0..self.count(11)? {
            let time = self.float()? / 20.;
            let mode = self.varint()?;
            let first = self.scalars()?;
            keys.push(if self.flag()? {
                Keyframe { time, mode, post: self.scalars()?, pre: Some(first) }
            } else {
                Keyframe { time, mode, pre: None, post: first }
            });
        }
        Ok(keys)
    }
    fn animations(&mut self) -> Result<Vec<Animation>, String> {
        let mut animations = Vec::new();
        for _ in 0..self.count(8)? {
            let name = self.string()?;
            let length = self.float()? / 20.;
            let loop_mode = self.varint()?;
            let mut blend_weight = None;
            if self.format > 9 {
                self.varint()?;
                self.varint()?;
                for _ in 0..self.count(2)? {
                    blend_weight = Some(self.scalar()?);
                }
                self.varint()?;
            }
            let mut bones = Vec::new();
            for _ in 0..self.count(4)? {
                bones.push(BoneAnimation { bone: self.string()?, channels: [self.channel()?, self.channel()?, self.channel()?] });
            }
            let mut timeline = Vec::new();
            for _ in 0..self.count(5)? {
                let events = self.strings()?;
                timeline.push((self.float()? / 20., events));
            }
            let mut sounds = Vec::new();
            if self.format > 9 {
                for _ in 0..self.count(5)? {
                    sounds.push((self.string()?, self.float()? / 20.));
                }
            }
            animations.push(Animation { name, length, loop_mode, blend_weight, bones, timeline, sounds });
        }
        Ok(animations)
    }
    /// Animation controller files: read past. The count of files.
    fn controllers(&mut self, named: bool) -> Result<usize, String> {
        let files = self.count(2)?;
        for _ in 0..files {
            if self.format <= 15 {
                self.varint()?;
            } else {
                if named {
                    self.bytes()?;
                }
                self.bytes()?;
            }
            for _ in 0..self.count(3)? {
                self.bytes()?;
                self.bytes()?;
                for _ in 0..self.count(8)? {
                    self.bytes()?;
                    // Animations, transitions, entry and exit scripts.
                    self.skip_pairs()?;
                    self.skip_pairs()?;
                    self.strings()?;
                    self.strings()?;
                    // One blend time, or a curve of them.
                    if self.flag()? {
                        self.float()?;
                    } else {
                        for _ in 0..self.count(8)? {
                            self.float()?;
                            self.float()?;
                        }
                    }
                    self.varint()?;
                    if self.format > 26 {
                        self.strings()?;
                    }
                }
            }
        }
        Ok(files)
    }
    /// A vehicle or projectile model: read past.
    fn sub_entity(&mut self) -> Result<(), String> {
        if self.format <= 26 {
            self.bytes()?;
        }
        for _ in 0..self.count(2)? {
            self.bytes()?;
            self.animations()?;
        }
        self.controllers(false)?;
        // The base texture, then its variants.
        self.bytes()?;
        self.skip_image()?;
        for _ in 0..self.count(7)? {
            self.varint()?;
            self.bytes()?;
            self.skip_image()?;
        }
        self.bytes()?;
        self.geometry()?;
        if self.format > 26 {
            let identifiers = self.count(1)?;
            if identifiers == 0 || identifiers > 128 {
                return Err("invalid identifier count in YSM binary data".into());
            }
            for _ in 0..identifiers {
                self.bytes()?;
            }
        }
        Ok(())
    }
    fn textures(&mut self) -> Result<Vec<Texture>, String> {
        let mut textures = Vec::new();
        for _ in 0..self.count(7)? {
            let name = self.string()?;
            self.bytes()?;
            let data = self.bytes()?.to_vec();
            let (width, height, format) = (self.varint()?, self.varint()?, self.varint()?);
            self.varint()?;
            for _ in 0..self.count(7)? {
                self.varint()?;
                self.bytes()?;
                self.skip_image()?;
            }
            textures.push(Texture { name, width, height, format, data });
        }
        Ok(textures)
    }
    /// The manifest's metadata and properties: the name and the default texture.
    fn properties(&mut self) -> Result<(String, String), String> {
        self.bytes()?;
        let mut name = String::new();
        if self.flag()? {
            name = self.string()?;
            // Tips, license type and description.
            for _ in 0..3 {
                self.bytes()?;
            }
            for _ in 0..self.count(4)? {
                self.bytes()?;
                self.bytes()?;
                self.skip_pairs()?;
                self.bytes()?;
            }
            self.skip_pairs()?;
        }
        // Width and height scale, then the extra animations.
        self.float()?;
        self.float()?;
        self.skip_pairs()?;
        // Buttons with their configuration forms, then the classification.
        for _ in 0..self.count(4)? {
            self.bytes()?;
            self.bytes()?;
            self.varint()?;
            for _ in 0..self.count(17)? {
                for _ in 0..4 {
                    self.bytes()?;
                }
                self.take(12)?;
                self.skip_pairs()?;
            }
        }
        for _ in 0..self.count(2)? {
            self.bytes()?;
            self.skip_pairs()?;
        }
        let default_texture = self.string()?;
        self.bytes()?;
        // Free, render layers first, all cutout, no preview rotation, no GUI lighting.
        for _ in 0..5 {
            self.varint()?;
        }
        if self.format >= 32 {
            self.varint()?;
        }
        self.bytes()?;
        self.bytes()?;
        // Author avatars, then background images.
        for _ in 0..2 {
            for _ in 0..self.count(6)? {
                self.bytes()?;
                self.skip_image()?;
            }
        }
        Ok((name, default_texture))
    }
    fn footer(&mut self) -> Result<Footer, String> {
        let version = if self.format > 26 { self.varint()? } else { self.format };
        self.varint()?;
        let rand = self.string()?;
        let time = self.varlong()?;
        let extra = self.string()?;
        if self.format >= 24 {
            self.varint()?;
        }
        Ok(Footer { version, rand, time, extra })
    }
}
/// Reads the model. `payload` starts with the four-byte format.
pub fn parse(payload: &[u8]) -> Result<BinaryModel, String> {
    let Some(format) = payload.get(..4) else {
        return Err("truncated YSM binary data".into());
    };
    let format = u32::from_le_bytes(format.try_into().unwrap());
    if !(16..=32).contains(&format) {
        return Err(format!("unsupported YSM binary format {format}: only 16 to 32 are read"));
    }
    let mut reader = Reader { data: payload, at: 4, format };
    // Sounds, function files and language files.
    let sounds = reader.count(3)?;
    for _ in 0..sounds {
        reader.bytes()?;
        reader.bytes()?;
        reader.bytes()?;
    }
    for _ in 0..reader.count(3)? {
        reader.bytes()?;
        reader.bytes()?;
        reader.bytes()?;
    }
    for _ in 0..reader.count(3)? {
        reader.bytes()?;
        reader.bytes()?;
        reader.skip_pairs()?;
    }
    let vehicles = reader.count(8)?;
    for _ in 0..vehicles {
        reader.sub_entity()?;
    }
    let mut projectiles = 0;
    if format < 26 {
        // One list of sub-entities, then a flag.
        reader.varint()?;
    } else {
        projectiles = reader.count(8)?;
        for _ in 0..projectiles {
            reader.sub_entity()?;
        }
    }
    if reader.varint()? != 1 {
        return Err("unexpected marker in YSM binary data".into());
    }
    let mut animations = Vec::new();
    for _ in 0..reader.count(3)? {
        let kind = reader.varint()?;
        reader.bytes()?;
        animations.push(AnimationFile { kind, animations: reader.animations()? });
    }
    let controllers = reader.controllers(true)?;
    let textures = reader.textures()?;
    let (mut main, mut arm) = (None, None);
    for _ in 0..reader.count(3)? {
        let kind = reader.varint()?;
        reader.bytes()?;
        let geometry = reader.geometry()?;
        match kind {
            1 => main = Some(geometry),
            2 => arm = Some(geometry),
            // An arrow model.
            3 => projectiles += 1,
            _ => return Err(format!("unknown model kind {kind} in YSM binary data")),
        }
    }
    let (name, default_texture) = reader.properties()?;
    // The export footer; cached models end differently, which is not an error.
    let footer = reader.footer().ok();
    Ok(BinaryModel { format, name, default_texture, main, arm, animations, textures, footer, vehicles, projectiles, controllers, sounds })
}
/// The manifest key of an animation file kind.
pub fn role(kind: u32) -> String {
    match kind {
        1 => "main".into(),
        2 => "arm".into(),
        3 => "extra".into(),
        4 => "tac".into(),
        5 => "arrow".into(),
        6 => "carryon".into(),
        7 => "parcool".into(),
        8 => "swem".into(),
        9 => "slashblade".into(),
        10 => "tlm".into(),
        11 => "fp_arm".into(),
        12 => "immersive_melodies".into(),
        13 => "irons_spell_books".into(),
        _ => format!("unk_{kind}"),
    }
}
impl Geometry {
    /// The engine's bones: parents first. The format keeps a bone's parent by
    /// name and its faces already baked into blocks.
    pub fn bones(&self) -> Result<Vec<Bone>, String> {
        if self.bones.is_empty() || self.bones.len() > 65535 {
            return Err("invalid bone count".into());
        }
        let mut ordered: Vec<Bone> = Vec::with_capacity(self.bones.len());
        let mut index = std::collections::HashMap::new();
        let mut pending: Vec<&RawBone> = self.bones.iter().collect();
        while !pending.is_empty() {
            let before = pending.len();
            let mut next = Vec::new();
            for raw in pending {
                let parent = if raw.parent.is_empty() {
                    -1
                } else if let Some(parent) = index.get(raw.parent.as_str()) {
                    *parent
                } else {
                    next.push(raw);
                    continue;
                };
                if index.insert(raw.name.as_str(), ordered.len() as i32).is_some() {
                    return Err("duplicate bone name".into());
                }
                ordered.push(Bone {
                    name: raw.name.clone(),
                    parent,
                    pivot: raw.pivot,
                    rotation: raw.rotation,
                    // The arm roots and Background name a part group; other bones inherit one.
                    mask: match raw.name.as_str() {
                        "LeftArm" => 1,
                        "RightArm" => 2,
                        "Background" => 3,
                        _ if parent >= 0 => ordered[parent as usize].mask,
                        _ => 0,
                    },
                    glow: false,
                    quads: raw
                        .cubes
                        .iter()
                        .flatten()
                        // The file winds a face the other way round than `load_geometry`
                        // does and stores the outward normal; the renderer expects
                        // what `load_geometry` produces.
                        .map(|face| Quad {
                            positions: std::array::from_fn(|corner| face.positions[3 - corner]),
                            uv: std::array::from_fn(|corner| [face.u[3 - corner], face.v[3 - corner]]),
                            normal: face.normal.map(|component| -component),
                        })
                        .collect(),
                });
            }
            if next.len() == before {
                return Err("missing parent or cyclic bone hierarchy".into());
            }
            pending = next;
        }
        Ok(ordered)
    }
}
impl AnimationFile {
    /// The file as a Bedrock animation document, which `ClipSet` reads.
    pub fn document(&self) -> Value {
        fn vector(values: &[Scalar; 3]) -> Value {
            values
                .iter()
                .map(|value| match value {
                    Scalar::Number(number) => Value::from(*number),
                    Scalar::Text(text) => Value::from(text.as_str()),
                })
                .collect()
        }
        fn channel(keys: &[Keyframe]) -> Value {
            if let [key] = keys
                && key.time == 0.
                && key.mode == 0
                && key.pre.is_none()
            {
                return vector(&key.post);
            }
            let mut frames = Map::new();
            for key in keys {
                let mode = match key.mode {
                    0 | 1 => None,
                    2 => Some("catmullrom".to_owned()),
                    3 => Some("bezier".to_owned()),
                    other => Some(format!("mode{other}")),
                };
                let frame = if key.mode == 0 && key.pre.is_none() {
                    vector(&key.post)
                } else {
                    let mut frame = Map::new();
                    if let Some(pre) = &key.pre {
                        frame.insert("pre".into(), vector(pre));
                    }
                    frame.insert("post".into(), vector(&key.post));
                    if let Some(mode) = mode {
                        frame.insert("lerp_mode".into(), mode.into());
                    }
                    Value::Object(frame)
                };
                frames.insert(key.time.to_string(), frame);
            }
            Value::Object(frames)
        }
        let mut animations = Map::new();
        for animation in &self.animations {
            let mut entry = Map::new();
            match animation.loop_mode {
                0 => drop(entry.insert("loop".into(), false.into())),
                1 => drop(entry.insert("loop".into(), true.into())),
                3 => drop(entry.insert("loop".into(), "hold_on_last_frame".into())),
                _ => {}
            }
            if animation.length.is_finite() {
                entry.insert("animation_length".into(), animation.length.into());
            }
            match &animation.blend_weight {
                Some(Scalar::Number(weight)) => drop(entry.insert("blend_weight".into(), (*weight).into())),
                Some(Scalar::Text(weight)) => drop(entry.insert("blend_weight".into(), weight.as_str().into())),
                None => {}
            }
            let mut bones = Map::new();
            for bone in &animation.bones {
                let mut tracks = Map::new();
                for (name, keys) in ["rotation", "position", "scale"].into_iter().zip(&bone.channels) {
                    if !keys.is_empty() {
                        tracks.insert(name.into(), channel(keys));
                    }
                }
                bones.insert(bone.bone.clone(), Value::Object(tracks));
            }
            entry.insert("bones".into(), Value::Object(bones));
            animations.insert(animation.name.clone(), Value::Object(entry));
        }
        let mut document = Map::new();
        document.insert("animations".into(), Value::Object(animations));
        Value::Object(document)
    }
}
#[cfg(test)]
mod tests {
    //! `model.ysm` is the pack under `tests/fixtures/pack`, converted by Sparkle-Morpher's
    //! folder reader, serializer and cipher; `model.json` is what its binary reader then
    //! read back, as FNV-1a digests of a canonical stream (`target/ysm-golden/Gen2.java`).
    use super::*;
    const FILE: &[u8] = include_bytes!("../tests/fixtures/model.ysm");
    const SUMMARY: &str = include_str!("../tests/fixtures/model.json");
    struct Digest(u64);
    impl Digest {
        fn new() -> Self {
            Self(0xcbf2_9ce4_8422_2325)
        }
        fn bytes(&mut self, bytes: &[u8]) {
            for byte in bytes {
                self.0 = (self.0 ^ u64::from(*byte)).wrapping_mul(0x0100_0000_01b3);
            }
        }
        fn i(&mut self, value: u32) {
            self.bytes(&value.to_le_bytes());
        }
        fn f(&mut self, value: f32) {
            self.i(value.to_bits());
        }
        fn v(&mut self, values: &[f32]) {
            values.iter().for_each(|value| self.f(*value));
        }
        fn s(&mut self, text: &str) {
            self.i(text.len() as u32);
            self.bytes(text.as_bytes());
        }
        fn value(&mut self, value: &Scalar) {
            match value {
                Scalar::Number(number) => {
                    self.bytes(&[1]);
                    self.f(*number);
                }
                Scalar::Text(text) => {
                    self.bytes(&[2]);
                    self.s(text);
                }
            }
        }
        fn hex(&self) -> String {
            format!("{:016x}", self.0)
        }
    }
    fn geometry_digest(geometry: &Geometry) -> String {
        let mut d = Digest::new();
        d.s(&geometry.identifier);
        d.f(geometry.texture_width);
        d.f(geometry.texture_height);
        d.i(geometry.bones.len() as u32);
        for bone in &geometry.bones {
            d.s(&bone.name);
            d.s(&bone.parent);
            d.v(&bone.pivot);
            d.v(&bone.rotation);
            d.i(bone.cubes.len() as u32);
            for cube in &bone.cubes {
                d.i(cube.len() as u32);
                for face in cube {
                    d.v(&face.normal);
                    for corner in 0..4 {
                        d.v(&face.positions[corner]);
                        d.f(face.u[corner]);
                        d.f(face.v[corner]);
                    }
                }
            }
        }
        d.hex()
    }
    fn animation_digest(file: &AnimationFile) -> String {
        let mut d = Digest::new();
        d.i(file.kind);
        d.i(file.animations.len() as u32);
        for animation in &file.animations {
            d.s(&animation.name);
            d.f(animation.length);
            d.i(animation.loop_mode);
            match &animation.blend_weight {
                None => d.bytes(&[0]),
                Some(weight) => d.value(weight),
            }
            d.i(animation.bones.len() as u32);
            for bone in &animation.bones {
                d.s(&bone.bone);
                for channel in &bone.channels {
                    d.i(channel.len() as u32);
                    for key in channel {
                        d.f(key.time);
                        d.i(key.mode);
                        d.bytes(&[u8::from(key.pre.is_some())]);
                        key.pre.iter().flatten().chain(&key.post).for_each(|value| d.value(value));
                    }
                }
            }
            d.i(animation.timeline.len() as u32);
            for (time, events) in &animation.timeline {
                d.f(*time);
                d.i(events.len() as u32);
                events.iter().for_each(|event| d.s(event));
            }
            d.i(animation.sounds.len() as u32);
            for (name, time) in &animation.sounds {
                d.s(name);
                d.f(*time);
            }
        }
        d.hex()
    }
    fn model() -> BinaryModel {
        parse(&crate::ysm::decrypt_file(FILE, 1 << 24).unwrap()).unwrap()
    }
    #[test]
    fn the_fixture_reads_as_the_upstream_reader_read_it() {
        let summary: Value = serde_json::from_str(SUMMARY).unwrap();
        let model = model();
        assert_eq!(model.format, summary["format"]);
        assert_eq!(model.name, summary["name"]);
        assert_eq!(model.default_texture, summary["default_texture"]);
        assert_eq!(geometry_digest(model.main.as_ref().unwrap()), summary["geometry"]["main"]);
        assert_eq!(geometry_digest(model.arm.as_ref().unwrap()), summary["geometry"]["arm"]);
        assert_eq!(model.animations.len(), summary["animations"].as_object().unwrap().len());
        for file in &model.animations {
            assert_eq!(animation_digest(file), summary["animations"][role(file.kind)], "{}", role(file.kind));
        }
        let mut d = Digest::new();
        d.i(model.textures.len() as u32);
        for texture in &model.textures {
            d.s(&texture.name);
            d.i(texture.width);
            d.i(texture.height);
            d.i(texture.format);
            d.i(texture.data.len() as u32);
            d.bytes(&texture.data);
        }
        assert_eq!(d.hex(), summary["textures"]);
        let footer = model.footer.as_ref().unwrap();
        assert_eq!(footer.version, summary["footer"]["version"]);
        assert_eq!(footer.rand, summary["footer"]["rand"]);
        assert_eq!(footer.time, summary["footer"]["time"]);
        assert_eq!(footer.extra, summary["footer"]["extra"]);
        assert_eq!(model.vehicles as u64, summary["vehicles"]);
        assert_eq!(model.projectiles as u64, summary["projectiles"]);
        assert_eq!(model.controllers as u64, summary["controllers"]);
        assert_eq!(model.sounds as u64, summary["sounds"]);
        // What the pack's own files say.
        let names: Vec<&str> = model.textures.iter().map(|texture| texture.name.as_str()).collect();
        assert_eq!(names, ["skin", "alt"]);
        assert_eq!(model.textures[1].data, include_bytes!("../tests/fixtures/pack/textures/alt.png"));
        assert_eq!((model.textures[0].width, model.textures[0].height, model.textures[0].format), (32, 16, 2));
        let main = &model.animations.iter().find(|file| file.kind == 1).unwrap().animations;
        let names: Vec<&str> = main.iter().map(|animation| animation.name.as_str()).collect();
        assert_eq!(names, ["idle", "walk", "jump", "parallel0", "empty"]);
        assert_eq!(main[0].timeline.len(), 2);
        assert_eq!(main[0].sounds, [("step".to_owned(), 0.25)]);
        assert!(main[2].length.is_infinite() && main[2].loop_mode == 2);
    }
    #[test]
    fn damaged_payloads_are_errors() {
        let payload = crate::ysm::decrypt_file(FILE, 1 << 24).unwrap();
        // Every shorter payload fails cleanly, except where only the footer is cut.
        let complete = (1..payload.len()).filter(|length| parse(&payload[..*length]).is_ok()).count();
        assert!(complete < 40, "{complete}");
        for format in [0u32, 1, 15, 33, 0xFFFF_FFFF] {
            let mut other = payload.clone();
            other[..4].copy_from_slice(&format.to_le_bytes());
            assert!(parse(&other).err().unwrap().contains("format"), "{format}");
        }
        // Changed bytes are an error or another model; never a panic or a huge allocation.
        for at in (4..payload.len()).step_by(7) {
            for change in [0x01u8, 0x80, 0xFF] {
                let mut damaged = payload.clone();
                damaged[at] ^= change;
                let _ = parse(&damaged).map(|model| model.main.as_ref().map(Geometry::bones));
            }
        }
        assert!(parse(&[]).is_err());
        assert!(parse(&[32, 0, 0, 0]).is_err());
    }
}

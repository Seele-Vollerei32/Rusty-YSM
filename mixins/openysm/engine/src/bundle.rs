//! Shared package assets; mutable geometry/animators are created per render instance.
use crate::{Bone, Clip, ClipSet, Model, load_geometry, package::{ModelPackage, PackageLimits}, ysm, ysm_binary};
use std::{collections::BTreeMap, io::Read, path::Path, sync::Arc};
/// Where geometry and texture bytes come from.
enum Source {
    Package(Arc<ModelPackage>),
    /// A `.ysm` file: baked bones, and the encoded textures in `textures` order.
    Binary { main: Vec<Bone>, arm: Option<Vec<Bone>>, textures: Vec<Vec<u8>> },
}
pub struct ModelBundle {
    source: Source,
    default_texture: Option<String>,
    pub animations: BTreeMap<String, ClipSet>,
    pub diagnostics: Vec<String>,
    /// Animation role to the file it was read from.
    files: BTreeMap<String, String>,
    main: String,
    arm: Option<String>,
    textures: Vec<String>,
}
impl ModelBundle {
    pub fn new(package: ModelPackage) -> Result<Self, String> {
        let player = &package.manifest()["files"]["player"];
        let main = player["model"]["main"]
            .as_str()
            .ok_or("missing player main model")?
            .replace('\\', "/");
        let arm = player["model"]["arm"]
            .as_str()
            .map(|s| s.replace('\\', "/"));
        // Validate geometry before publishing a new resource generation.
        load_geometry(package.read(&main)?)?;
        if let Some(path) = &arm {
            load_geometry(package.read(path)?)?;
        }
        let mut animations = BTreeMap::new();
        let mut diagnostics = Vec::new();
        let mut files = BTreeMap::new();
        if let Some(roles) = player["animation"].as_object() {
            for (role, path) in roles {
                let path = path
                    .as_str()
                    .ok_or("animation role must reference a file")?
                    .replace('\\', "/");
                let clips = ClipSet::load(package.read(&path)?)?;
                for (name, error) in &clips.rejected {
                    diagnostics.push(format!("{role}/{name}: {error}"));
                }
                animations.insert(role.clone(), clips);
                files.insert(role.clone(), path);
            }
        }
        fn texture_paths(v: &serde_json::Value, out: &mut Vec<String>) -> Result<(), String> {
            match v {
                serde_json::Value::Null => {}
                serde_json::Value::String(s) => out.push(s.replace('\\', "/")),
                serde_json::Value::Array(a) => {
                    for v in a {
                        texture_paths(v, out)?;
                    }
                }
                serde_json::Value::Object(o) => {
                    texture_paths(o.get("uv").ok_or("texture object requires uv")?, out)?
                }
                _ => return Err("invalid texture selection".into()),
            }
            Ok(())
        }
        let mut textures = Vec::new();
        texture_paths(&player["texture"], &mut textures)?;
        let default_texture = package.manifest()["properties"]["default_texture"].as_str().map(str::to_owned);
        Ok(Self {
            source: Source::Package(Arc::new(package)),
            default_texture,
            animations,
            diagnostics,
            files,
            main,
            arm,
            textures,
        })
    }
    /// A model pack directory, a ZIP of one, or an encrypted `.ysm` file.
    pub fn open(path: impl AsRef<Path>, limits: PackageLimits) -> Result<Self, String> {
        let path = path.as_ref();
        if path.is_file() {
            let mut file = std::fs::File::open(path).map_err(|e| e.to_string())?;
            let mut bytes = Vec::new();
            file.by_ref().take(7).read_to_end(&mut bytes).map_err(|e| e.to_string())?;
            if ysm::is_ysm_file(&bytes) {
                file.take(limits.max_source_bytes as u64).read_to_end(&mut bytes).map_err(|e| e.to_string())?;
                if bytes.len() > limits.max_source_bytes {
                    return Err("package source budget exceeded".into());
                }
                return Self::from_ysm(&bytes, limits);
            }
        }
        ModelPackage::open(path, limits).and_then(Self::new)
    }
    /// An encrypted `.ysm` file: its main and arm geometry, animations and textures.
    /// Vehicles, projectiles, animation controllers and sounds in it are not used.
    pub fn from_ysm(file: &[u8], limits: PackageLimits) -> Result<Self, String> {
        let model = ysm_binary::parse(&ysm::decrypt_file(file, limits.max_total_bytes)?)?;
        let main = model.main.as_ref().ok_or("the .ysm file has no main model")?.bones()?;
        let arm = model.arm.as_ref().map(ysm_binary::Geometry::bones).transpose()?;
        // Validate geometry before publishing a new resource generation.
        Model::new(main.clone())?;
        if let Some(arm) = &arm {
            Model::new(arm.clone())?;
        }
        let mut animations = BTreeMap::new();
        let mut diagnostics = Vec::new();
        let mut files = BTreeMap::new();
        for file in &model.animations {
            let role = ysm_binary::role(file.kind);
            let clips = ClipSet::from_document(&file.document())?;
            for (name, error) in &clips.rejected {
                diagnostics.push(format!("{role}/{name}: {error}"));
            }
            animations.insert(role.clone(), clips);
            files.insert(role.clone(), role);
        }
        let (names, data) = model.textures.into_iter().map(|texture| (texture.name, texture.data)).unzip();
        Ok(Self {
            source: Source::Binary { main, arm, textures: data },
            default_texture: Some(model.default_texture).filter(|name| !name.is_empty()),
            animations,
            diagnostics,
            files,
            main: String::new(),
            arm: None,
            textures: names,
        })
    }
    pub fn model(&self) -> Result<Model, String> {
        match &self.source {
            Source::Package(package) => load_geometry(package.read(&self.main)?),
            Source::Binary { main, .. } => Model::new(main.clone()),
        }
    }
    pub fn arm(&self) -> Result<Option<Model>, String> {
        match &self.source {
            Source::Package(package) => self.arm.as_ref().map(|p| package.read(p).and_then(load_geometry)).transpose(),
            Source::Binary { arm, .. } => arm.clone().map(Model::new).transpose(),
        }
    }
    /// The loaded clip `name` and its animation role. Roles reading the same
    /// file count once; a name loaded from several files is ambiguous.
    pub fn find_clip(&self, name: &str) -> Result<Option<(&str, &Clip)>, String> {
        let mut found: Option<(&str, &str, &Clip)> = None;
        for (role, set) in &self.animations {
            let Some(clip) = set.get(name) else {
                continue;
            };
            let file = self.files[role].as_str();
            match found {
                Some((_, other, _)) if other != file => {
                    return Err(format!("clip {name} is in several animation files"));
                }
                Some(_) => {}
                None => found = Some((role, file, clip)),
            }
        }
        Ok(found.map(|(role, _, clip)| (role, clip)))
    }
    /// The texture whose file stem is `wanted`; without one, the manifest's
    /// `properties.default_texture`, else the first. None: the pack has no texture.
    pub fn texture_index(&self, wanted: Option<&str>) -> Result<Option<usize>, String> {
        fn stem(path: &str) -> &str {
            let file = path.rsplit('/').next().unwrap_or(path);
            file.rsplit_once('.').map_or(file, |(stem, _)| stem)
        }
        let find = |name: &str| self.textures.iter().position(|texture| stem(texture) == name);
        if let Some(name) = wanted {
            return find(name).map(Some).ok_or_else(|| format!("unknown texture {name}"));
        }
        let default = self.default_texture.as_deref().and_then(find);
        Ok(default.or(if self.textures.is_empty() { None } else { Some(0) }))
    }
    pub fn textures(&self) -> &[String] {
        &self.textures
    }
    pub fn texture(&self, index: usize) -> Result<&[u8], String> {
        let name = self
            .textures
            .get(index)
            .ok_or("texture selection out of range")?;
        match &self.source {
            Source::Package(package) => package.read(name),
            Source::Binary { textures, .. } => Ok(&textures[index]),
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::package::PackageLimits;
    use std::io::{Cursor, Write};
    const GEOMETRY: &[u8] = br#"{"minecraft:geometry":[{"description":{"texture_width":16,"texture_height":16},"bones":[{"name":"root","pivot":[0,0,0],"cubes":[{"origin":[0,0,0],"size":[16,16,16],"uv":[0,0]}]}]}]}"#;
    fn bundle(manifest: &str, files: &[(&str, &[u8])]) -> Result<ModelBundle, String> {
        let mut zip = zip::ZipWriter::new(Cursor::new(Vec::new()));
        let manifest = [b"\xef\xbb\xbf".as_slice(), manifest.as_bytes()].concat();
        for (name, bytes) in [("ysm.json", manifest.as_slice()), ("main.json", GEOMETRY)].iter().chain(files) {
            zip.start_file(*name, zip::write::SimpleFileOptions::default()).unwrap();
            zip.write_all(bytes).unwrap();
        }
        let bytes = zip.finish().unwrap().into_inner();
        ModelBundle::new(ModelPackage::from_bytes(&bytes, PackageLimits::default())?)
    }
    /// The fixture pack as a directory and as the `.ysm` file Sparkle-Morpher made of it.
    fn fixtures() -> (ModelBundle, ModelBundle) {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures");
        let folder = ModelBundle::open(root.join("pack"), PackageLimits::default()).unwrap();
        let binary = ModelBundle::open(root.join("model.ysm"), PackageLimits::default()).unwrap();
        (folder, binary)
    }
    #[test]
    fn a_ysm_file_gives_the_geometry_of_its_pack() {
        let (folder, binary) = fixtures();
        let close = |a: &[f32], b: &[f32]| a.iter().zip(b).all(|(a, b)| (a - b).abs() < 1e-4);
        let facing = |q: &crate::Quad| {
            let p = q.positions;
            let (a, b): ([f32; 3], [f32; 3]) = (std::array::from_fn(|i| p[1][i] - p[0][i]), std::array::from_fn(|i| p[2][i] - p[0][i]));
            let cross = [a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]];
            cross.iter().zip(q.normal).map(|(c, n)| c * n).sum::<f32>() > 0.
        };
        let mut report = Vec::new();
        for (a, b) in [(folder.model().unwrap(), binary.model().unwrap()), (folder.arm().unwrap().unwrap(), binary.arm().unwrap().unwrap())] {
            assert_eq!(a.bones.len(), b.bones.len());
            for (a, b) in a.bones.iter().zip(&b.bones) {
                assert_eq!((&a.name, a.parent, a.mask), (&b.name, b.parent, b.mask));
                assert!(close(&a.pivot, &b.pivot) && close(&a.rotation, &b.rotation), "{}", a.name);
                if a.quads.len() != b.quads.len() {
                    report.push(format!("{}: {} quads from the folder, {} from the file", a.name, a.quads.len(), b.quads.len()));
                }
                for (index, quad) in a.quads.iter().enumerate() {
                    // The same corners with the same texture coordinates, facing the same way.
                    let same = b.quads.iter().any(|other| {
                        close(&quad.normal, &other.normal)
                            && facing(quad) == facing(other)
                            && (0..4).all(|i| (0..4).any(|j| close(&quad.positions[i], &other.positions[j]) && close(&quad.uv[i], &other.uv[j])))
                    });
                    if !same {
                        report.push(format!("{} quad {index}: {:?} {:?} {:?}", a.name, quad.positions, quad.uv, quad.normal));
                    }
                }
            }
        }
        assert!(report.is_empty(), "{}", report.join("\n"));
    }
    #[test]
    fn a_ysm_file_gives_the_textures_and_clips_of_its_pack() {
        let (folder, binary) = fixtures();
        assert_eq!(folder.textures(), ["textures/skin.png", "textures/alt.png"]);
        assert_eq!(binary.textures(), ["skin", "alt"]);
        for bundle in [&folder, &binary] {
            assert_eq!(bundle.texture_index(None), Ok(Some(1)));
            assert_eq!(bundle.texture_index(Some("skin")), Ok(Some(0)));
            assert!(bundle.texture_index(Some("none")).is_err());
        }
        assert_eq!(folder.texture(0).unwrap(), binary.texture(0).unwrap());
        assert_eq!(folder.texture(1).unwrap(), binary.texture(1).unwrap());
        assert!(binary.texture(2).is_err());
        assert_eq!(folder.animations.keys().collect::<Vec<_>>(), binary.animations.keys().collect::<Vec<_>>());
        assert!(folder.diagnostics.is_empty() && binary.diagnostics.is_empty());
        assert_eq!(binary.find_clip("hold").unwrap().unwrap().0, "arm");
        // Every clip poses both models alike, over time and with the same inputs.
        let query = |name: &str| match name {
            "life_time" => 3.5,
            "ground_speed" => 4.,
            name if name.starts_with("variable.") => f32::NAN,
            _ => 0.,
        };
        let (mut a, mut b) = (folder.model().unwrap(), binary.model().unwrap());
        for name in ["idle", "walk", "jump", "parallel0", "empty"] {
            let (from_folder, from_file) = (folder.animations["main"].get(name).unwrap(), binary.animations["main"].get(name).unwrap());
            assert_eq!((from_folder.looping, from_folder.length.is_finite()), (from_file.looping, from_file.length.is_finite()), "{name}");
            let (mut first, mut second) = (crate::Animator::default(), crate::Animator::default());
            for (frame, time) in [0., 0.25, 0.5, 0.75, 1., 1.3, 1.5, 2., 2.4].into_iter().enumerate() {
                first.advance(1. + frame as f32);
                second.advance(1. + frame as f32);
                first.apply(from_folder, &mut a, time, from_folder.looping, &query);
                second.apply(from_file, &mut b, time, from_file.looping, &query);
                let same = a.animation.iter().zip(&b.animation).all(|(a, b)| (a - b).abs() < 1e-3);
                assert!(same, "{name} at {time}: {:?} {:?}", a.animation, b.animation);
            }
        }
    }
    #[test]
    fn texture_selection_follows_the_manifest() {
        let files: &[(&str, &[u8])] = &[("t/red.png", b"r"), ("t/blue.v2.png", b"b")];
        let two = r#"{"properties":{"default_texture":"blue.v2"},"files":{"player":{"model":{"main":"main.json"},"texture":["t/red.png",{"uv":"t/blue.v2.png"}]}}}"#;
        let pack = bundle(two, files).unwrap();
        assert_eq!(pack.texture_index(None), Ok(Some(1)));
        assert_eq!(pack.texture_index(Some("red")), Ok(Some(0)));
        assert_eq!(pack.texture(1).unwrap(), b"b");
        assert!(pack.texture_index(Some("green")).is_err());
        // An unknown default falls back to the first texture; no texture is not an error.
        let first = two.replace("blue.v2\"}", "missing\"}");
        assert_eq!(bundle(&first, files).unwrap().texture_index(None), Ok(Some(0)));
        let none = r#"{"files":{"player":{"model":{"main":"main.json"}}}}"#;
        let pack = bundle(none, &[]).unwrap();
        assert_eq!(pack.texture_index(None), Ok(None));
        assert!(pack.arm().unwrap().is_none());
        assert_eq!(pack.model().unwrap().bones.len(), 1);
    }
    #[test]
    fn clips_are_found_by_role_and_ambiguity_is_an_error() {
        let animation: &[u8] = br#"{"animations":{"wave":{"bones":{}},"broken":{"bones":{"root":{"position":["ysm.bone_rot('a')",0,0]}}}}}"#;
        let other: &[u8] = br#"{"animations":{"wave":{"bones":{}},"bow":{"bones":{}}}}"#;
        let manifest = |extra: &str| format!(r#"{{"files":{{"player":{{"model":{{"main":"main.json"}},"animation":{{"main":"a.json","alias":"a.json","extra":"{extra}"}}}}}}}}"#);
        let pack = bundle(&manifest("b.json"), &[("a.json", animation), ("b.json", other)]).unwrap();
        // Two roles reading one file are one source; a second file makes the name ambiguous.
        assert!(pack.find_clip("wave").is_err());
        assert_eq!(pack.find_clip("bow").unwrap().unwrap().0, "extra");
        assert!(pack.find_clip("missing").unwrap().is_none());
        // A rejected clip is not found, and is reported once per role.
        assert!(pack.find_clip("broken").unwrap().is_none());
        assert_eq!(pack.diagnostics.len(), 2);
        let pack = bundle(&manifest("c.json"), &[("a.json", animation), ("c.json", br#"{"animations":{}}"#)]).unwrap();
        assert_eq!(pack.find_clip("wave").unwrap().unwrap().0, "alias");
    }
}

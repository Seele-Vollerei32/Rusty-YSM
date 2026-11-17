//! In-memory model packages. Archives are never extracted to the filesystem.
//! Directory inputs must be quiescent: canonical checks are not atomic against concurrent replacement.
use serde_json::Value;
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    io::{Cursor, Read},
    path::Path,
};

#[derive(Clone, Copy, Debug)]
pub struct PackageLimits {
    pub max_source_bytes: usize,
    pub max_entry_bytes: usize,
    pub max_total_bytes: usize,
    pub max_entries: usize,
    pub max_path_bytes: usize,
    pub max_depth: usize,
}
impl Default for PackageLimits {
    fn default() -> Self {
        Self {
            max_source_bytes: 512 * 1024 * 1024,
            max_entry_bytes: 64 * 1024 * 1024,
            max_total_bytes: 512 * 1024 * 1024,
            max_entries: 16384,
            max_path_bytes: 4096,
            max_depth: 64,
        }
    }
}
#[derive(Debug)]
pub struct ModelPackage {
    entries: BTreeMap<String, Vec<u8>>,
    manifest: Value,
    references: BTreeSet<String>,
}
pub fn normalize_resource_path(path: &str, limits: PackageLimits) -> Result<String, String> {
    let path = path.replace('\\', "/");
    if path.is_empty()
        || path.len() > limits.max_path_bytes
        || path.starts_with('/')
        || path.contains(':')
        || path.contains('\0')
    {
        return Err("invalid resource path".into());
    }
    let parts: Vec<_> = path.split('/').collect();
    if parts.len() > limits.max_depth
        || parts
            .iter()
            .any(|s| s.is_empty() || *s == "." || *s == "..")
    {
        return Err("invalid resource path component".into());
    }
    // Windows aliases must not produce different resources on different hosts.
    for part in parts {
        let stem = part.split('.').next().unwrap().to_ascii_uppercase();
        if part.ends_with(['.', ' '])
            || matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL")
            || (stem.len() == 4
                && (stem.starts_with("COM") || stem.starts_with("LPT"))
                && matches!(stem.as_bytes()[3], b'1'..=b'9'))
        {
            return Err("nonportable resource path".into());
        }
    }
    Ok(path)
}
fn bounded_read(reader: impl Read, limit: usize) -> Result<Vec<u8>, String> {
    let mut data = Vec::new();
    reader
        .take((limit as u64).saturating_add(1))
        .read_to_end(&mut data)
        .map_err(|e| e.to_string())?;
    if data.len() > limit {
        return Err("package byte budget exceeded".into());
    }
    Ok(data)
}
struct Builder {
    limits: PackageLimits,
    entries: BTreeMap<String, Vec<u8>>,
    names: BTreeSet<String>,
    total: usize,
    count: usize,
}
impl Builder {
    fn new(limits: PackageLimits) -> Self {
        Self {
            limits,
            entries: Default::default(),
            names: Default::default(),
            total: 0,
            count: 0,
        }
    }
    fn name(&mut self, name: &str) -> Result<String, String> {
        self.count += 1;
        if self.count > self.limits.max_entries {
            return Err("package entry budget exceeded".into());
        }
        let path = normalize_resource_path(name, self.limits)?;
        if !self.names.insert(path.to_lowercase()) {
            return Err(format!("duplicate resource: {path}"));
        }
        Ok(path)
    }
    fn insert(&mut self, path: String, reader: impl Read) -> Result<(), String> {
        let budget = self
            .limits
            .max_entry_bytes
            .min(self.limits.max_total_bytes.saturating_sub(self.total));
        let bytes = bounded_read(reader, budget)?;
        self.total += bytes.len();
        self.entries.insert(path, bytes);
        Ok(())
    }
    fn directory(&mut self, root: &Path, dir: &Path) -> Result<(), String> {
        for entry in fs::read_dir(dir).map_err(|e| e.to_string())? {
            let entry = entry.map_err(|e| e.to_string())?;
            let file_type = entry.file_type().map_err(|e| e.to_string())?;
            if file_type.is_symlink() {
                return Err("symlinks are not model resources".into());
            }
            let path = entry.path();
            let relative = path.strip_prefix(root).map_err(|e| e.to_string())?;
            let name = self.name(relative.to_str().ok_or("non-UTF8 resource name")?)?;
            if !path
                .canonicalize()
                .map_err(|e| e.to_string())?
                .starts_with(root)
            {
                return Err("resource escapes package".into());
            }
            if file_type.is_dir() {
                self.directory(root, &path)?;
            } else if file_type.is_file() {
                self.insert(name, fs::File::open(path).map_err(|e| e.to_string())?)?;
            } else {
                return Err("unsupported resource type".into());
            }
        }
        Ok(())
    }
    fn finish(self) -> Result<ModelPackage, String> {
        let root = if self.entries.contains_key("ysm.json") {
            String::new()
        } else {
            let candidates: Vec<_> = self
                .entries
                .keys()
                .filter_map(|p| p.strip_suffix("/ysm.json"))
                .collect();
            match candidates.as_slice() {
                [path] => format!("{path}/"),
                [] if self.entries.contains_key("main.json")
                    && self.entries.contains_key("arm.json") =>
                {
                    String::new()
                }
                [] => return Err("model manifest not found".into()),
                _ => return Err("ambiguous model roots".into()),
            }
        };
        let entries: BTreeMap<_, _> = self
            .entries
            .into_iter()
            .filter_map(|(p, b)| p.strip_prefix(&root).map(|s| (s.to_owned(), b)))
            .collect();
        let manifest = if let Some(bytes) = entries.get("ysm.json") {
            serde_json::from_slice::<Value>(bytes.strip_prefix(b"\xef\xbb\xbf").unwrap_or(bytes.as_slice()))
                .map_err(|e| format!("invalid model manifest: {e}"))?
        } else {
            serde_json::json!({"files":{"player":{"model":{"main":"main.json","arm":"arm.json"}}}})
        };
        if !manifest.is_object() {
            return Err("model manifest must be an object".into());
        }
        let mut references = BTreeSet::new();
        fn paths(
            v: &Value,
            refs: &mut BTreeSet<String>,
            limits: PackageLimits,
        ) -> Result<(), String> {
            match v {
                Value::String(s) => {
                    refs.insert(normalize_resource_path(s, limits)?);
                }
                Value::Array(a) => {
                    for v in a {
                        paths(v, refs, limits)?;
                    }
                }
                Value::Object(o) => {
                    if let Some(uv) = o.get("uv") {
                        paths(uv, refs, limits)?;
                    } else {
                        for v in o.values() {
                            paths(v, refs, limits)?;
                        }
                    }
                }
                Value::Null => {}
                _ => return Err("invalid resource reference".into()),
            }
            Ok(())
        }
        fn model_refs(
            v: &Value,
            refs: &mut BTreeSet<String>,
            limits: PackageLimits,
        ) -> Result<(), String> {
            for key in ["model", "animation", "animation_controllers", "texture"] {
                paths(&v[key], refs, limits)?;
            }
            Ok(())
        }
        model_refs(&manifest["files"]["player"], &mut references, self.limits)?;
        for kind in ["projectiles", "vehicles"] {
            if let Some(items) = manifest["files"][kind].as_array() {
                for item in items {
                    model_refs(item, &mut references, self.limits)?;
                }
            }
        }
        for reference in &references {
            if !entries.contains_key(reference) {
                return Err(format!("missing model resource: {reference}"));
            }
        }
        Ok(ModelPackage {
            entries,
            manifest,
            references,
        })
    }
}
impl ModelPackage {
    pub fn open(path: impl AsRef<Path>, limits: PackageLimits) -> Result<Self, String> {
        let path = path.as_ref();
        if fs::symlink_metadata(path)
            .map_err(|e| e.to_string())?
            .file_type()
            .is_symlink()
        {
            return Err("symlink package root".into());
        }
        if path.is_dir() {
            let root = path.canonicalize().map_err(|e| e.to_string())?;
            let mut builder = Builder::new(limits);
            builder.directory(&root, &root)?;
            builder.finish()
        } else {
            Self::from_bytes(
                &bounded_read(
                    fs::File::open(path).map_err(|e| e.to_string())?,
                    limits.max_source_bytes,
                )?,
                limits,
            )
        }
    }
    pub fn from_bytes(bytes: &[u8], limits: PackageLimits) -> Result<Self, String> {
        if bytes.len() > limits.max_source_bytes {
            return Err("package source budget exceeded".into());
        }
        if crate::ysm::is_ysm_file(bytes) {
            return Err("an encrypted .ysm file is not a package: open it with ModelBundle::open".into());
        }
        if !bytes.starts_with(b"PK") {
            return Err("unsupported encrypted or binary YSM container".into());
        }
        let mut zip = zip::ZipArchive::new(Cursor::new(bytes)).map_err(|e| e.to_string())?;
        if zip.len() > limits.max_entries {
            return Err("package entry budget exceeded".into());
        }
        let mut builder = Builder::new(limits);
        for i in 0..zip.len() {
            let mut file = zip.by_index(i).map_err(|e| e.to_string())?;
            let raw = std::str::from_utf8(file.name_raw()).map_err(|_| "non-UTF8 archive name")?;
            let name = builder.name(if file.is_dir() {
                raw.trim_end_matches('/')
            } else {
                raw
            })?;
            if let Some(mode) = file.unix_mode() {
                let kind = mode & 0o170000;
                if kind != 0 && kind != 0o100000 && kind != 0o040000 {
                    return Err("unsupported archive entry type".into());
                }
            }
            if file.is_dir() {
                continue;
            }
            if file.size() > limits.max_entry_bytes as u64 {
                return Err("package entry budget exceeded".into());
            }
            builder.insert(name, &mut file)?;
        }
        builder.finish()
    }
    pub fn entries(&self) -> &BTreeMap<String, Vec<u8>> {
        &self.entries
    }
    pub fn manifest(&self) -> &Value {
        &self.manifest
    }
    pub fn references(&self) -> &BTreeSet<String> {
        &self.references
    }
    pub fn read(&self, path: &str) -> Result<&[u8], String> {
        self.entries
            .get(path)
            .map(Vec::as_slice)
            .ok_or_else(|| format!("resource not found: {path}"))
    }
}

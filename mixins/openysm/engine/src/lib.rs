//! OpenYSM geometry and animation runtime, independent of Minecraft Java and Horizon.
pub mod molang;
use serde_json::Value;
use std::{ffi::c_void, ptr::NonNull};

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct Vertex {
    pub position: [f32; 3],
    pub uv: [f32; 2],
    pub normal: [f32; 3],
    pub light: i32,
    pub hidden: u32,
}
#[repr(C, align(16))]
#[derive(Clone, Copy)]
pub struct BoneData {
    pub transform: [f32; 16],
    pub normal: [f32; 16],
    pub light: i32,
    pub hidden: i32,
    pub padding: [i32; 2],
}
unsafe extern "C" {
    fn ysm_mesh_create(data: *const u8, length: usize) -> *mut c_void;
    fn ysm_mesh_free(handle: *mut c_void);
    fn ysm_vertex_count(handle: *const c_void) -> usize;
    fn ysm_bones(
        handle: *mut c_void,
        anim: *const f32,
        count: usize,
        light: i32,
        output: *mut c_void,
        length: usize,
    ) -> bool;
    fn ysm_skin(
        handle: *mut c_void,
        bones: *const c_void,
        bytes: usize,
        root: *const f32,
        mask: u32,
        output: *mut Vertex,
        count: usize,
    ) -> bool;
}
pub const IDENTITY: [f32; 16] = [
    1., 0., 0., 0., 0., 1., 0., 0., 0., 0., 1., 0., 0., 0., 0., 1.,
];
const MAX_QUADS: usize = 1_000_000;
#[derive(Clone, Debug)]
pub struct Bone {
    pub name: String,
    pub parent: i32,
    pub pivot: [f32; 3],
    pub rotation: [f32; 3],
    pub mask: u32,
    pub glow: bool,
    pub quads: Vec<Quad>,
}
#[derive(Clone, Debug)]
pub struct Quad {
    pub positions: [[f32; 3]; 4],
    pub uv: [[f32; 2]; 4],
    pub normal: [f32; 3],
}
pub struct Model {
    handle: NonNull<c_void>,
    pub bones: Vec<Bone>,
    matrices: Vec<BoneData>,
    animation: Vec<f32>,
    vertices: Vec<Vertex>,
}
impl Drop for Model {
    fn drop(&mut self) {
        // The handle is exclusively owned and allocated by the paired C++ constructor.
        unsafe { ysm_mesh_free(self.handle.as_ptr()) };
    }
}
fn put_i(bytes: &mut Vec<u8>, value: i32) {
    bytes.extend(value.to_le_bytes());
}
fn put_f(bytes: &mut Vec<u8>, value: f32) {
    bytes.extend(value.to_le_bytes());
}
impl Model {
    pub fn new(bones: Vec<Bone>) -> Result<Self, String> {
        if cfg!(target_endian = "big") {
            return Err("little endian host required".into());
        }
        if bones.is_empty() || bones.len() > 65535 {
            return Err("invalid bone count".into());
        }
        let mut bytes = Vec::new();
        put_i(&mut bytes, bones.len() as i32);
        let mut quads = 0;
        for (index, bone) in bones.iter().enumerate() {
            if bone.parent < -1 || bone.parent >= index as i32 {
                return Err("parents must precede children".into());
            }
            quads += bone.quads.len();
            if quads > MAX_QUADS {
                return Err("quad limit exceeded".into());
            }
            put_i(&mut bytes, bone.parent);
            put_i(&mut bytes, bone.mask as i32);
            bytes.push(u8::from(bone.glow));
            for x in bone.pivot {
                put_f(&mut bytes, x);
            }
            put_i(&mut bytes, 1);
            bytes.push(0); // Horizon handles backface policy, not the CPU projection matrix.
            put_i(&mut bytes, bone.quads.len() as i32);
            for q in &bone.quads {
                for x in q
                    .positions
                    .iter()
                    .flatten()
                    .chain(q.uv.iter().flatten())
                    .chain(q.normal.iter())
                {
                    if !x.is_finite() {
                        return Err("nonfinite geometry".into());
                    }
                    put_f(&mut bytes, *x);
                }
            }
        }
        // Constructor copies the bounded input and catches C++ exceptions.
        let handle = NonNull::new(unsafe { ysm_mesh_create(bytes.as_ptr(), bytes.len()) })
            .ok_or("C++ mesh construction failed")?;
        let count = unsafe { ysm_vertex_count(handle.as_ptr()) };
        let matrices = vec![
            BoneData {
                transform: IDENTITY,
                normal: IDENTITY,
                light: 0,
                hidden: 0,
                padding: [0; 2]
            };
            bones.len()
        ];
        let animation = vec![0.; bones.len() * 12];
        Ok(Self {
            handle,
            bones,
            matrices,
            animation,
            vertices: vec![Vertex::default(); count],
        })
    }
    /// Angles in radians; offsets in Bedrock pixels. 12 floats per bone, matching OpenYSM.cpp.
    pub fn pose(&mut self, mut pose: impl FnMut(&Bone) -> ([f32; 3], [f32; 3], [f32; 3], bool)) {
        for (bone, p) in self.bones.iter().zip(self.animation.chunks_exact_mut(12)) {
            let (rotation, position, scale, hidden) = pose(bone);
            p[0..3].copy_from_slice(&rotation);
            p[3..6].copy_from_slice(&position);
            p[6..9].copy_from_slice(&scale);
            if hidden {
                p[6..9].fill(0.);
            }
            p[10] = 0.;
        }
    }
    pub fn render(&mut self, root: &[f32; 16], light: u32, mask: u32) -> &[Vertex] {
        let light = ((light & 255) | (((light >> 8) & 255) << 16)) as i32;
        let bytes = std::mem::size_of_val(self.matrices.as_slice());
        // All buffers have exact sizes checked by the C ABI; &mut self excludes concurrent calls.
        let ok = unsafe {
            ysm_bones(
                self.handle.as_ptr(),
                self.animation.as_ptr(),
                self.animation.len(),
                light,
                self.matrices.as_mut_ptr().cast(),
                bytes,
            ) && ysm_skin(
                self.handle.as_ptr(),
                self.matrices.as_ptr().cast(),
                bytes,
                root.as_ptr(),
                mask,
                self.vertices.as_mut_ptr(),
                self.vertices.len(),
            )
        };
        if ok { &self.vertices } else { &[] }
    }
    pub fn bone_buffer(&self) -> &[BoneData] {
        &self.matrices
    }
}
fn vec3(v: &Value, default: [f32; 3]) -> Result<[f32; 3], String> {
    if v.is_null() {
        return Ok(default);
    }
    let a = v
        .as_array()
        .filter(|a| a.len() == 3)
        .ok_or("expected three numbers")?;
    let mut out = [0.; 3];
    for i in 0..3 {
        out[i] = a[i]
            .as_f64()
            .filter(|x| x.is_finite())
            .ok_or("expected finite number")? as f32;
    }
    Ok(out)
}
/// Bedrock 1.12 geometry; cube box UV and per-face UV, bone/cube pivots and rotations.
pub fn load_geometry(input: &[u8]) -> Result<Model, String> {
    let doc: Value = serde_json::from_slice(input).map_err(|e| e.to_string())?;
    let g = &doc["minecraft:geometry"][0];
    let source = g["bones"].as_array().ok_or("missing geometry bones")?;
    if source.len() > 65535 {
        return Err("bone limit exceeded".into());
    }
    let tw = g["description"]["texture_width"].as_f64().unwrap_or(64.) as f32;
    let th = g["description"]["texture_height"].as_f64().unwrap_or(64.) as f32;
    if tw <= 0. || th <= 0. {
        return Err("invalid texture dimensions".into());
    }
    let mut ordered = Vec::new();
    let mut pending: Vec<&Value> = source.iter().collect();
    while !pending.is_empty() {
        let previous = pending.len();
        let mut next = Vec::new();
        for b in pending {
            let name = b["name"].as_str().ok_or("bone without name")?;
            if ordered.iter().any(|x: &Bone| x.name == name) {
                return Err("duplicate bone name".into());
            }
            let parent = if let Some(parent) = b["parent"].as_str() {
                let Some(i) = ordered.iter().position(|x: &Bone| x.name == parent) else {
                    next.push(b);
                    continue;
                };
                i as i32
            } else {
                -1
            };
            let pivot = vec3(&b["pivot"], [0.; 3])?;
            let mut rotation = vec3(&b["rotation"], [0.; 3])?;
            for r in &mut rotation {
                *r = r.to_radians();
            }
            rotation[0] = -rotation[0];
            rotation[1] = -rotation[1];
            let mut bone = Bone {
                name: name.into(),
                parent,
                pivot: [-pivot[0], pivot[1], pivot[2]],
                rotation,
                mask: 3,
                glow: b["glow"].as_bool().unwrap_or(false),
                quads: Vec::new(),
            };
            if let Some(cubes) = b["cubes"].as_array() {
                for cube in cubes {
                    bake_cube(cube, tw, th, &mut bone.quads)?;
                }
            }
            ordered.push(bone);
        }
        if next.len() == previous {
            return Err("missing parent or cyclic bone hierarchy".into());
        }
        pending = next;
    }
    Model::new(ordered)
}
fn rotate(p: [f32; 3], pivot: [f32; 3], r: [f32; 3]) -> [f32; 3] {
    let [mut x, mut y, mut z] = std::array::from_fn(|i| p[i] - pivot[i]);
    let (s, c) = r[0].to_radians().sin_cos();
    (y, z) = (y * c - z * s, y * s + z * c);
    let (s, c) = r[1].to_radians().sin_cos();
    (x, z) = (x * c + z * s, -x * s + z * c);
    let (s, c) = r[2].to_radians().sin_cos();
    (x, y) = (x * c - y * s, x * s + y * c);
    [x + pivot[0], y + pivot[1], z + pivot[2]]
}
fn bake_cube(c: &Value, tw: f32, th: f32, out: &mut Vec<Quad>) -> Result<(), String> {
    let origin = vec3(&c["origin"], [0.; 3])?;
    let size = vec3(&c["size"], [0.; 3])?;
    let pivot = vec3(&c["pivot"], [0.; 3])?;
    let rotation = vec3(&c["rotation"], [0.; 3])?;
    let inflate = c["inflate"].as_f64().unwrap_or(0.) as f32;
    let lo: [f32; 3] = std::array::from_fn(|i| origin[i] - inflate);
    let hi: [f32; 3] = std::array::from_fn(|i| origin[i] + size[i] + inflate);
    let points: [[f32; 3]; 8] = std::array::from_fn(|i| {
        let p = std::array::from_fn(|axis| {
            if i & (1 << axis) == 0 {
                lo[axis]
            } else {
                hi[axis]
            }
        });
        let p = rotate(p, pivot, rotation);
        [-p[0] / 16., p[1] / 16., p[2] / 16.]
    });
    let [w, h, d] = size;
    let (u, v) = if let Some(a) = c["uv"].as_array() {
        (
            a.first().and_then(Value::as_f64).unwrap_or(0.) as f32,
            a.get(1).and_then(Value::as_f64).unwrap_or(0.) as f32,
        )
    } else {
        (0., 0.)
    };
    let faces = [
        ("west", [0, 4, 6, 2], [u, v + d, d, h]),
        ("east", [5, 1, 3, 7], [u + d + w, v + d, d, h]),
        ("down", [0, 1, 5, 4], [u + d + w, v, w, d]),
        ("up", [6, 7, 3, 2], [u + d, v, w, d]),
        ("north", [1, 0, 2, 3], [u + d, v + d, w, h]),
        ("south", [4, 5, 7, 6], [u + 2. * d + w, v + d, w, h]),
    ];
    for (name, indices, box_uv) in faces {
        let rect = if c["uv"].is_object() {
            let f = &c["uv"][name];
            if f.is_null() {
                continue;
            }
            let a = f["uv"].as_array().ok_or("missing face uv")?;
            let s = f["uv_size"].as_array().ok_or("missing face uv_size")?;
            if a.len() != 2 || s.len() != 2 {
                return Err("invalid face uv".into());
            }
            [
                a[0].as_f64().ok_or("invalid uv")? as f32,
                a[1].as_f64().ok_or("invalid uv")? as f32,
                s[0].as_f64().ok_or("invalid uv")? as f32,
                s[1].as_f64().ok_or("invalid uv")? as f32,
            ]
        } else {
            box_uv
        };
        let p = indices.map(|i| points[i]);
        let a: [f32; 3] = std::array::from_fn(|i| p[1][i] - p[0][i]);
        let b: [f32; 3] = std::array::from_fn(|i| p[2][i] - p[0][i]);
        let n = [
            a[1] * b[2] - a[2] * b[1],
            a[2] * b[0] - a[0] * b[2],
            a[0] * b[1] - a[1] * b[0],
        ];
        let length = n.iter().map(|x| x * x).sum::<f32>().sqrt().max(1e-8);
        let [u, v, w, h] = rect;
        let mut uv = [
            [(u + w) / tw, (v + h) / th],
            [u / tw, (v + h) / th],
            [u / tw, v / th],
            [(u + w) / tw, v / th],
        ];
        if c["mirror"].as_bool().unwrap_or(false) {
            uv.swap(0, 1);
            uv.swap(2, 3);
        }
        out.push(Quad {
            positions: p,
            uv,
            normal: n.map(|x| x / length),
        });
    }
    Ok(())
}

/// Numeric Bedrock animation tracks; unsupported Molang fails explicitly.
pub struct Clip {
    pub length: f32,
    pub looping: bool,
    tracks: Value,
    expressions: std::collections::HashMap<String, molang::Expr>,
}
impl Clip {
    pub fn load(bytes: &[u8], name: &str) -> Result<Self, String> {
        let doc: Value = serde_json::from_slice(bytes).map_err(|e| e.to_string())?;
        let a = &doc["animations"][name];
        if !a.is_object() {
            return Err("animation not found".into());
        }
        let tracks = a["bones"].clone();
        let mut expressions = std::collections::HashMap::new();
        fn compile(
            v: &Value,
            expressions: &mut std::collections::HashMap<String, molang::Expr>,
        ) -> Result<Value, String> {
            Ok(match v {
                Value::String(s) => {
                    expressions.insert(s.clone(), molang::Expr::parse(s)?);
                    Value::from(0)
                }
                Value::Array(a) => Value::Array(
                    a.iter()
                        .map(|v| compile(v, expressions))
                        .collect::<Result<_, _>>()?,
                ),
                Value::Object(o) => Value::Object(
                    o.iter()
                        .map(|(k, v)| {
                            Ok((
                                k.clone(),
                                if k == "lerp_mode" {
                                    v.clone()
                                } else {
                                    compile(v, expressions)?
                                },
                            ))
                        })
                        .collect::<Result<_, String>>()?,
                ),
                _ => v.clone(),
            })
        }
        let validated = compile(&tracks, &mut expressions)?;
        let mut inferred_length = 0.0_f32;
        for b in validated
            .as_object()
            .ok_or("missing animation bones")?
            .values()
        {
            for key in ["rotation", "position", "scale"] {
                let t = &b[key];
                if t.is_null() {
                    continue;
                }
                if t.is_array() {
                    vec3(t, [0.; 3])?;
                } else if let Some(keys) = t.as_object() {
                    for (time, v) in keys {
                        let time = time.parse::<f32>().map_err(|_| "invalid key time")?;
                        if !time.is_finite() || time < 0. {
                            return Err("invalid key time".into());
                        }
                        inferred_length = inferred_length.max(time);
                        frame(v)?;
                        if v.get("pre").is_some() {
                            vec3(&v["pre"], [0.; 3])?;
                        }
                        if let Some(mode) = v["lerp_mode"].as_str()
                            && mode != "linear"
                            && mode != "catmullrom"
                        {
                            return Err(format!("unsupported interpolation: {mode}"));
                        }
                    }
                } else if !t.is_number() {
                    return Err(format!("invalid {key} track"));
                }
            }
        }
        let length = a["animation_length"]
            .as_f64()
            .map(|v| v as f32)
            .unwrap_or(inferred_length);
        if !length.is_finite() || length < 0. {
            return Err("invalid animation length".into());
        }
        Ok(Self {
            length,
            looping: a["loop"].as_bool().unwrap_or(false),
            tracks,
            expressions,
        })
    }
    pub fn apply(&self, model: &mut Model, time: f32) {
        self.apply_with_queries(model, time, &|_| 0.);
    }
    pub fn apply_with_queries(&self, model: &mut Model, time: f32, query: &impl Fn(&str) -> f32) {
        let time = if self.looping && self.length > 0. {
            time.rem_euclid(self.length)
        } else {
            time.min(self.length)
        };
        let query = |name: &str| {
            if name == "anim_time" {
                time
            } else {
                query(name)
            }
        };
        fn evaluate(
            v: &Value,
            expressions: &std::collections::HashMap<String, molang::Expr>,
            query: &impl Fn(&str) -> f32,
        ) -> Value {
            match v {
                Value::String(s) => expressions
                    .get(s)
                    .map_or_else(|| v.clone(), |e| Value::from(e.eval(query))),
                Value::Array(a) => {
                    Value::Array(a.iter().map(|v| evaluate(v, expressions, query)).collect())
                }
                Value::Object(o) => Value::Object(
                    o.iter()
                        .map(|(k, v)| (k.clone(), evaluate(v, expressions, query)))
                        .collect(),
                ),
                _ => v.clone(),
            }
        }
        let tracks = evaluate(&self.tracks, &self.expressions, &query);
        model.pose(|bone| {
            let b = &tracks[&bone.name];
            let mut r = track(&b["rotation"], time, [0.; 3]);
            r[0] = -r[0];
            r[1] = -r[1];
            (
                std::array::from_fn(|i| bone.rotation[i] + r[i].to_radians()),
                track(&b["position"], time, [0.; 3]),
                track(&b["scale"], time, [1.; 3]),
                false,
            )
        });
    }
}
fn frame(v: &Value) -> Result<[f32; 3], String> {
    if v.is_object() && v.get("post").is_none() {
        return Err("keyframe object requires post".into());
    }
    vec3(if v.is_object() { &v["post"] } else { v }, [0.; 3])
}
fn track(v: &Value, time: f32, default: [f32; 3]) -> [f32; 3] {
    if v.is_null() {
        return default;
    }
    if let Some(n) = v.as_f64() {
        return [n as f32; 3];
    }
    if v.is_array() {
        return vec3(v, default).unwrap_or(default);
    }
    let mut keys: Vec<(f32, &Value)> = v
        .as_object()
        .into_iter()
        .flat_map(|o| o.iter())
        .filter_map(|(k, v)| Some((k.parse().ok()?, v)))
        .collect();
    keys.sort_by(|a, b| a.0.total_cmp(&b.0));
    let Some(first) = keys.first() else {
        return default;
    };
    let post = |v: &Value| frame(v).unwrap_or(default);
    let pre = |v: &Value| {
        v.get("pre")
            .map_or_else(|| post(v), |p| vec3(p, default).unwrap_or(default))
    };
    if time < first.0 {
        return pre(first.1);
    }
    for (index, w) in keys.windows(2).enumerate() {
        if time < w[1].0 {
            let t = ((time - w[0].0) / (w[1].0 - w[0].0)).clamp(0., 1.);
            let p1 = post(w[0].1);
            let p2 = pre(w[1].1);
            if w[1].1["lerp_mode"].as_str() == Some("catmullrom") {
                let p0 = post(keys[index.saturating_sub(1)].1);
                let p3 = pre(keys[(index + 2).min(keys.len() - 1)].1);
                return std::array::from_fn(|i| {
                    0.5 * ((2. * p1[i])
                        + (-p0[i] + p2[i]) * t
                        + (2. * p0[i] - 5. * p1[i] + 4. * p2[i] - p3[i]) * t * t
                        + (-p0[i] + 3. * p1[i] - 3. * p2[i] + p3[i]) * t * t * t)
                });
            }
            return std::array::from_fn(|i| p1[i] + t * (p2[i] - p1[i]));
        }
    }
    post(keys.last().unwrap().1)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn model() -> Model {
        load_geometry(br#"{"minecraft:geometry":[{"description":{"texture_width":16,"texture_height":16},"bones":[{"name":"root","pivot":[0,0,0],"cubes":[{"origin":[0,0,0],"size":[16,16,16],"uv":[0,0]}]},{"name":"child","parent":"root"}]}]}"#).unwrap()
    }
    #[test]
    fn cpp_skinning_and_hidden_inheritance() {
        let mut m = model();
        m.pose(|b| (b.rotation, [0.; 3], [1.; 3], false));
        let v = m.render(&IDENTITY, 0xf0f0, 0);
        assert_eq!(v.len(), 24);
        assert!(v.iter().all(|v| v.hidden == 0 && v.light == 0x00f000f0));
        assert!(
            v.iter()
                .all(|v| (v.normal.iter().map(|x| x * x).sum::<f32>() - 1.).abs() < 0.001)
        );
        assert!(v.iter().any(|v| v.position == [-1., 1., 1.]));
        m.pose(|_| ([0.; 3], [0.; 3], [1.; 3], true));
        assert!(m.render(&IDENTITY, 0, 0).iter().all(|v| v.hidden == 1));
        assert_eq!(m.bone_buffer()[1].hidden, 1);
    }
    #[test]
    fn numeric_animation_interpolates() {
        let mut m = model();
        let a = Clip::load(br#"{"animations":{"walk":{"animation_length":1,"loop":true,"bones":{"root":{"position":{"0":[0,0,0],"1":[16,0,0]}}}}}}"#,"walk").unwrap();
        a.apply(&mut m, 0.5);
        assert!((m.render(&IDENTITY, 0, 0)[0].position[0] + 0.5).abs() < 0.001);
    }
    #[test]
    fn rejects_cycles_and_invalid_molang() {
        assert!(load_geometry(br#"{"minecraft:geometry":[{"bones":[{"name":"a","parent":"b"},{"name":"b","parent":"a"}]}]}"#).is_err());
        assert!(
            Clip::load(
                br#"{"animations":{"a":{"bones":{"x":{"rotation":["unsupported.value",0,0]}}}}}"#,
                "a"
            )
            .is_err()
        );
    }
    #[test]
    fn discontinuous_keys_use_pre_before_and_post_at_key() {
        let track_data =
            serde_json::json!({"0":[0,0,0],"1":{"pre":[10,0,0],"post":[20,0,0]},"2":[30,0,0]});
        assert_eq!(track(&track_data, 0.5, [0.; 3]), [5., 0., 0.]);
        assert_eq!(track(&track_data, 1., [0.; 3]), [20., 0., 0.]);
        assert_eq!(track(&track_data, 1.5, [0.; 3]), [25., 0., 0.]);
    }
    #[test]
    fn duration_is_inferred_and_bad_times_are_rejected() {
        let clip = Clip::load(
            br#"{"animations":{"a":{"bones":{"root":{"rotation":{"0":[0,0,0],"2":[1,2,3]}}}}}}"#,
            "a",
        )
        .unwrap();
        assert_eq!(clip.length, 2.);
        assert!(
            Clip::load(
                br#"{"animations":{"a":{"bones":{"root":{"rotation":{"NaN":[0,0,0]}}}}}}"#,
                "a"
            )
            .is_err()
        );
    }
    #[test]
    fn catmull_rom_preserves_linear_inner_segment() {
        let data = serde_json::json!({"0":[0,0,0],"1":[10,0,0],"2":{"post":[20,0,0],"lerp_mode":"catmullrom"},"3":[30,0,0]});
        assert_eq!(track(&data, 1.5, [0.; 3]), [15., 0., 0.]);
    }
    #[test]
    fn gpu_layout_matches_upstream() {
        assert_eq!(std::mem::size_of::<BoneData>(), 144);
        assert_eq!(std::mem::size_of::<Vertex>(), 40);
    }
}

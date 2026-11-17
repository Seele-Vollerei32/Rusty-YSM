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
/// OpenYSM `HandItemRenderer`: a pack's arm model in the first-person hand pose,
/// `translate(±0.25, 1.8, 0) * scale(-1, -1, 1)` (column-major). Blocks; the
/// left arm takes +0.25. It is a rotation, so quad winding is unchanged.
pub fn arm_placement(left: bool) -> [f32; 16] {
    let x = if left { 0.25 } else { -0.25 };
    [
        -1., 0., 0., 0., 0., -1., 0., 0., 0., 0., 1., 0., x, 1.8, 0., 1.,
    ]
}
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
    /// Whether a bone with geometry belongs to exactly this part group (1 left arm, 2 right arm).
    pub fn has_part(&self, mask: u32) -> bool {
        self.bones.iter().any(|bone| bone.mask == mask && !bone.quads.is_empty())
    }
    pub fn bone_buffer(&self) -> &[BoneData] {
        &self.matrices
    }
}
/// JSON as model packs ship it: UTF-8, optionally with a byte order mark.
fn parse_json(bytes: &[u8]) -> Result<Value, String> {
    let bytes = bytes.strip_prefix(b"\xef\xbb\xbf").unwrap_or(bytes);
    serde_json::from_slice(bytes).map_err(|e| e.to_string())
}
/// What OpenYSM's main animation states test (`AnimationRegister`).
#[derive(Clone, Copy, Debug, Default)]
pub struct PlayerState {
    /// Riding a living vehicle: the main controller plays nothing.
    pub riding: bool,
    pub dead: bool,
    pub spin_attack: bool,
    pub sleeping: bool,
    /// `Player.isSwimming`, as opposed to merely being in the swimming pose.
    pub swimming: bool,
    /// `Pose.SWIMMING`, which crawling shares.
    pub swim_pose: bool,
    pub on_ladder: bool,
    /// Blocks per second, positive upwards.
    pub vertical_speed: f32,
    pub flying: bool,
    pub fall_flying: bool,
    pub in_water: bool,
    pub on_ground: bool,
    pub hurt: bool,
    pub crouching: bool,
    pub sprinting: bool,
    /// `AnimationEvent.getLimbSwingAmount`.
    pub limb_swing: f32,
}
impl PlayerState {
    /// The matching states in OpenYSM's order (priority, then registration),
    /// each with whether it plays once. `idle` always matches, so it is last.
    pub fn states(&self) -> Vec<(&'static str, bool)> {
        const MIN_SPEED: f32 = 0.05;
        if self.riding {
            return Vec::new();
        }
        let s = self;
        [
            ("death", true, s.dead),
            ("riptide", false, s.spin_attack),
            ("sleep", false, s.sleeping),
            ("swim", false, s.swimming),
            ("climb", false, s.swim_pose && s.limb_swing.abs() > MIN_SPEED),
            ("climbing", false, s.swim_pose),
            ("ladder_up", false, s.on_ladder && s.vertical_speed > 0.),
            ("ladder_stillness", false, s.on_ladder && s.vertical_speed == 0.),
            ("ladder_down", false, s.on_ladder && s.vertical_speed < 0.),
            ("fly", false, s.flying),
            ("elytra_fly", false, s.fall_flying),
            ("swim_stand", false, s.in_water && !s.on_ground),
            ("attacked", true, s.hurt),
            ("jump", false, !s.on_ground && !s.in_water),
            ("sneak", false, s.on_ground && s.crouching && s.limb_swing.abs() > MIN_SPEED),
            ("sneaking", false, s.on_ground && s.crouching),
            ("run", false, s.on_ground && s.sprinting),
            ("walk", false, s.on_ground && s.limb_swing > MIN_SPEED),
            ("idle", false, true),
        ]
        .into_iter()
        .filter(|state| state.2)
        .map(|state| (state.0, state.1))
        .collect()
    }
}
/// Every clip of one animation file that this engine can load.
pub struct ClipSet {
    clips: std::collections::HashMap<String, Clip>,
    /// Clips the file defines but the engine rejected, with the reason.
    pub rejected: Vec<(String, String)>,
}
impl ClipSet {
    pub fn load(bytes: &[u8]) -> Result<Self, String> {
        let doc = parse_json(bytes)?;
        let animations = doc["animations"].as_object().ok_or("missing animations")?;
        if animations.len() > 4096 {
            return Err("too many animations".into());
        }
        let mut set = Self {
            clips: Default::default(),
            rejected: Vec::new(),
        };
        for (name, animation) in animations {
            match Clip::from_animation(animation) {
                Ok(clip) => {
                    set.clips.insert(name.clone(), clip);
                }
                Err(error) => set.rejected.push((name.clone(), error)),
            }
        }
        Ok(set)
    }
    pub fn len(&self) -> usize {
        self.clips.len()
    }
    pub fn is_empty(&self) -> bool {
        self.clips.is_empty()
    }
    pub fn get(&self, name: &str) -> Option<&Clip> {
        self.clips.get(name)
    }
    /// The first matching state that has a loaded clip: (name, plays once, clip).
    /// OpenYSM always takes the first matching state; skipping one without a
    /// usable clip keeps the model animated where this engine rejected a clip.
    pub fn select(&self, state: &PlayerState) -> Option<(&'static str, bool, &Clip)> {
        state
            .states()
            .into_iter()
            .find_map(|(name, once)| Some((name, once, self.clips.get(name)?)))
    }
}
/// OpenYSM `ysm.head_yaw` and `ysm.head_pitch` (`AnimatableEntity`): the yaw is
/// the head relative to the body, clamped to 85 degrees; both are negated.
/// Each pair is (previous, current) in degrees, blended by `alpha` the short way.
pub fn head_look(body_yaw: (f32, f32), head_yaw: (f32, f32), pitch: (f32, f32), alpha: f32) -> (f32, f32) {
    let wrap = |degrees: f32| (degrees + 180.).rem_euclid(360.) - 180.;
    let turn = |(previous, current): (f32, f32)| previous + wrap(current - previous) * alpha;
    let yaw = -wrap(turn(head_yaw) - turn(body_yaw)).clamp(-85., 85.);
    (yaw, -(pitch.0 + (pitch.1 - pitch.0) * alpha))
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
        if !out[i].is_finite() {
            return Err("number exceeds f32 range".into());
        }
    }
    Ok(out)
}
/// Bedrock 1.12 geometry; cube box UV and per-face UV, bone/cube pivots and rotations.
pub fn load_geometry(input: &[u8]) -> Result<Model, String> {
    let doc = parse_json(input)?;
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
                // OpenYSM part groups: the arm roots and Background name a group,
                // every other bone inherits its parent's (0 for an ungrouped root).
                mask: match name {
                    "LeftArm" => 1,
                    "RightArm" => 2,
                    "Background" => 3,
                    _ if parent >= 0 => ordered[parent as usize].mask,
                    _ => 0,
                },
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
    unbounded_time: bool,
    tracks: Value,
    expressions: std::collections::HashMap<String, molang::Expr>,
    /// What this clip's expressions assigned; kept between frames.
    variables: molang::Variables,
}
impl Clip {
    pub fn load(bytes: &[u8], name: &str) -> Result<Self, String> {
        let doc = parse_json(bytes)?;
        let a = &doc["animations"][name];
        if !a.is_object() {
            return Err("animation not found".into());
        }
        Self::from_animation(a)
    }
    fn from_animation(a: &Value) -> Result<Self, String> {
        // A clip without bones is valid: OpenYSM packs use such empty clips for
        // slots like `hold_mainhand:empty`, and they leave the rest pose.
        let tracks = match &a["bones"] {
            Value::Null => Value::Object(Default::default()),
            bones if bones.is_object() => bones.clone(),
            _ => return Err("animation bones must be an object".into()),
        };
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
                            frame_vector(&v["pre"])?;
                        }
                        if let Some(mode) = v["lerp_mode"].as_str()
                            && mode != "linear"
                            && mode != "catmullrom"
                        {
                            return Err(format!("unsupported interpolation: {mode}"));
                        }
                    }
                } else if t.is_number() {
                    frame_vector(t)?;
                } else {
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
            unbounded_time: a.get("animation_length").is_none() && inferred_length == 0. && !expressions.is_empty(),
            tracks,
            expressions,
            variables: Default::default(),
        })
    }
    pub fn apply(&self, model: &mut Model, time: f32) {
        self.apply_with_queries(model, time, &|_| 0.);
    }
    pub fn apply_with_queries(&self, model: &mut Model, time: f32, query: &impl Fn(&str) -> f32) {
        self.apply_as(model, time, self.looping, query);
    }
    /// Like `apply_with_queries`, with the loop mode chosen by the caller: an
    /// OpenYSM state names its own, whatever the clip's `loop` says.
    pub fn apply_as(&self, model: &mut Model, time: f32, looping: bool, query: &impl Fn(&str) -> f32) {
        let time = if time.is_finite() { time.max(0.) } else { 0. };
        let time = if self.unbounded_time {
            time
        } else if looping && self.length > 0. {
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
            variables: &molang::Variables,
        ) -> Value {
            match v {
                Value::String(s) => expressions
                    .get(s)
                    .map_or_else(|| v.clone(), |e| Value::from(e.eval_with(query, variables))),
                Value::Array(a) => {
                    Value::Array(a.iter().map(|v| evaluate(v, expressions, query, variables)).collect())
                }
                Value::Object(o) => Value::Object(
                    o.iter()
                        .map(|(k, v)| (k.clone(), evaluate(v, expressions, query, variables)))
                        .collect(),
                ),
                _ => v.clone(),
            }
        }
        // Every expression of the clip runs each frame, keyframes at other times
        // included, in sorted bone/channel/key order. An assignment is therefore
        // seen by later expressions of this frame and by all of the next frame.
        let tracks = evaluate(&self.tracks, &self.expressions, &query, &self.variables);
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
fn frame_vector(v: &Value) -> Result<[f32; 3], String> {
    if let Some(value) = v.as_f64() {
        let value = value as f32;
        if !value.is_finite() {
            return Err("nonfinite animation sample".into());
        }
        return Ok([value; 3]);
    }
    vec3(v, [0.; 3])
}
fn frame(v: &Value) -> Result<[f32; 3], String> {
    if v.is_object() && v.get("post").is_none() {
        return Err("keyframe object requires post".into());
    }
    frame_vector(if v.is_object() { &v["post"] } else { v })
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
            .map_or_else(|| post(v), |p| frame_vector(p).unwrap_or(default))
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
    fn expression_only_animation_advances_without_declared_duration() {
        let mut m = model();
        let clip = Clip::load(br#"{"animations":{"a":{"loop":true,"bones":{"root":{"position":["query.anim_time",0,0]}}}}}"#, "a").unwrap();
        clip.apply(&mut m, 2.5);
        assert_eq!(m.animation[3], 2.5);
        clip.apply(&mut m, -1.);
        assert_eq!(m.animation[3], 0.);
        clip.apply(&mut m, f32::NAN);
        assert_eq!(m.animation[3], 0.);
        let bounded = Clip::load(br#"{"animations":{"a":{"animation_length":1,"bones":{"root":{"position":["query.anim_time",0,0]}}}}}"#, "a").unwrap();
        bounded.apply(&mut m, 2.5);
        assert_eq!(m.animation[3], 1.);
    }
    #[test]
    fn part_groups_follow_openysm_and_select_one_arm() {
        let mut m = load_geometry(br#"{"minecraft:geometry":[{"description":{"texture_width":16,"texture_height":16},"bones":[
            {"name":"LeftArm","cubes":[{"origin":[0,0,0],"size":[1,1,1],"uv":[0,0]}]},
            {"name":"LeftForeArm","parent":"LeftArm","cubes":[{"origin":[0,0,0],"size":[1,1,1],"uv":[0,0]}]},
            {"name":"RightArm","cubes":[{"origin":[0,0,0],"size":[1,1,1],"uv":[0,0]}]},
            {"name":"Background","cubes":[{"origin":[0,0,0],"size":[1,1,1],"uv":[0,0]}]},
            {"name":"Body","cubes":[{"origin":[0,0,0],"size":[1,1,1],"uv":[0,0]}]},
            {"name":"leftarm","cubes":[{"origin":[0,0,0],"size":[1,1,1],"uv":[0,0]}]}]}]}"#).unwrap();
        let masks: Vec<_> = m.bones.iter().map(|b| (b.name.as_str(), b.mask)).collect();
        assert_eq!(masks, [("LeftArm", 1), ("LeftForeArm", 1), ("RightArm", 2),
                           ("Background", 3), ("Body", 0), ("leftarm", 0)]);
        assert!(m.has_part(1) && m.has_part(2) && !m.has_part(4));
        m.pose(|b| (b.rotation, [0.; 3], [1.; 3], false));
        // 24 vertices per cube. Mask 0 draws all; a side draws its group plus Background.
        let visible = |m: &mut Model, mask| m.render(&IDENTITY, 0, mask).iter().filter(|v| v.hidden == 0).count();
        assert_eq!(visible(&mut m, 0), 6 * 24);
        assert_eq!(visible(&mut m, 1), 3 * 24);
        assert_eq!(visible(&mut m, 2), 2 * 24);
        let body = load_geometry(br#"{"minecraft:geometry":[{"bones":[{"name":"Body","cubes":[{"origin":[0,0,0],"size":[1,1,1],"uv":[0,0]}]}]}]}"#).unwrap();
        assert!(!body.has_part(1) && !body.has_part(2));
    }
    #[test]
    fn pack_arm_placement_matches_upstream_hand_transform() {
        // Bedrock pixels (x, y, z) must land at (x/16 ± 0.25, 1.8 - y/16, z/16).
        let mut m = load_geometry(br#"{"minecraft:geometry":[{"description":{"texture_width":16,"texture_height":16},"bones":[
            {"name":"LeftArm","cubes":[{"origin":[0,16,0],"size":[16,16,16],"uv":[0,0]}]}]}]}"#).unwrap();
        m.pose(|b| (b.rotation, [0.; 3], [1.; 3], false));
        for (left, offset) in [(true, 0.25_f32), (false, -0.25)] {
            let v = m.render(&arm_placement(left), 0, 1).to_vec();
            assert!(v.iter().all(|v| v.hidden == 0));
            let range = |axis: usize| {
                let values = v.iter().map(|v| v.position[axis]);
                (values.clone().fold(f32::MAX, f32::min), values.fold(f32::MIN, f32::max))
            };
            let close = |a: (f32, f32), b: (f32, f32)| (a.0 - b.0).abs() < 1e-5 && (a.1 - b.1).abs() < 1e-5;
            assert!(close(range(0), (offset, 1. + offset)), "{:?}", range(0));
            assert!(close(range(1), (-0.2, 0.8)), "{:?}", range(1));
            assert!(close(range(2), (0., 1.)), "{:?}", range(2));
        }
    }
    #[test]
    fn clip_without_bones_keeps_the_rest_pose() {
        let mut m = model();
        for source in [&br#"{"animations":{"hold_mainhand:empty":{"loop":true}}}"#[..],
                       &br#"{"animations":{"hold_mainhand:empty":{"animation_length":1,"timeline":{"0.5":"v.a=1;"}}}}"#[..]] {
            let clip = Clip::load(source, "hold_mainhand:empty").unwrap();
            m.pose(|_| ([9.; 3], [9.; 3], [9.; 3], false));
            clip.apply(&mut m, 0.25);
            // Rotation rest (0), no offset, unit scale for every bone.
            assert_eq!(&m.animation[0..9], &[0., 0., 0., 0., 0., 0., 1., 1., 1.]);
        }
        assert!(Clip::load(br#"{"animations":{"a":{"bones":[]}}}"#, "a").is_err());
        assert!(Clip::load(br#"{"animations":{"a":{"bones":3}}}"#, "a").is_err());
    }
    #[test]
    fn head_look_matches_openysm_signs_clamp_and_wraparound() {
        let close = |a: (f32, f32), b: (f32, f32)| (a.0 - b.0).abs() < 1e-3 && (a.1 - b.1).abs() < 1e-3;
        // Head 30 degrees past the body, looking 20 degrees down: both negated.
        assert!(close(head_look((10., 10.), (40., 40.), (20., 20.), 1.), (-30., -20.)));
        // Clamped to 85 on either side.
        assert!(close(head_look((0., 0.), (120., 120.), (0., 0.), 0.5), (-85., 0.)));
        assert!(close(head_look((0., 0.), (-120., -120.), (0., 0.), 0.5), (85., 0.)));
        // Interpolation crosses +-180 the short way: 170 -> -170 passes 180, which
        // is 20 degrees past a body at 160 (the long way would give 0 - 160).
        assert!(close(head_look((160., 160.), (170., -170.), (0., 10.), 0.5), (-20., -5.)), "{:?}",
                head_look((160., 160.), (170., -170.), (0., 10.), 0.5));
        // Directly behind the body is clamped like any other large angle.
        assert!(close(head_look((0., 0.), (170., -170.), (0., 0.), 0.5), (85., 0.)));
        assert!(close(head_look((175., -175.), (175., -175.), (0., 0.), 0.5), (0., 0.)));
        // Body at 350 and head at 10 differ by 20, not 340.
        assert!(close(head_look((350., 350.), (10., 10.), (0., 0.), 0.), (-20., 0.)));
    }
    #[test]
    fn clip_assignments_drive_channels_and_persist_between_frames() {
        let mut m = model();
        // `root` assigns from the animation time; `child` reads it. Bones evaluate
        // in sorted order, so `child` sees the previous frame's value.
        let clip = Clip::load(br#"{"animations":{"idle":{"animation_length":10,"bones":{
            "root":{"position":["v.bv=query.anim_time*2",0,0]},
            "child":{"position":["v.bv",0,0]}}}}}"#, "idle").unwrap();
        clip.apply(&mut m, 1.);
        assert_eq!(m.animation[3], 2.);
        assert_eq!(m.animation[12 + 3], 0.);
        clip.apply(&mut m, 3.);
        assert_eq!(m.animation[3], 6.);
        assert_eq!(m.animation[12 + 3], 2.);
        // A separately loaded clip has its own scope.
        let other = Clip::load(br#"{"animations":{"a":{"bones":{"root":{"position":["v.bv",0,0]}}}}}"#, "a").unwrap();
        other.apply(&mut m, 0.);
        assert_eq!(m.animation[3], 0.);
    }
    #[test]
    fn states_follow_openysm_priority_order() {
        let names = |s: PlayerState| s.states().into_iter().map(|x| x.0).collect::<Vec<_>>();
        let ground = PlayerState { on_ground: true, ..Default::default() };
        assert_eq!(names(ground), ["idle"]);
        assert_eq!(names(PlayerState { limb_swing: 0.2, ..ground }), ["walk", "idle"]);
        // Exactly at the threshold is not walking; walk does not take the absolute value.
        assert_eq!(names(PlayerState { limb_swing: 0.05, ..ground }), ["idle"]);
        assert_eq!(names(PlayerState { limb_swing: -0.2, ..ground }), ["idle"]);
        assert_eq!(names(PlayerState { limb_swing: 0.2, sprinting: true, ..ground }), ["run", "walk", "idle"]);
        assert_eq!(names(PlayerState { limb_swing: -0.2, crouching: true, ..ground }),
                   ["sneak", "sneaking", "idle"]);
        assert_eq!(names(PlayerState { crouching: true, sprinting: true, ..ground }),
                   ["sneaking", "run", "idle"]);
        assert_eq!(names(PlayerState::default()), ["jump", "idle"]);
        assert_eq!(names(PlayerState { in_water: true, ..Default::default() }), ["swim_stand", "idle"]);
        assert_eq!(names(PlayerState { in_water: true, swimming: true, swim_pose: true, limb_swing: 0.3, ..Default::default() }),
                   ["swim", "climb", "climbing", "swim_stand", "idle"]);
        assert_eq!(names(PlayerState { swim_pose: true, ..ground }), ["climbing", "idle"]);
        assert_eq!(names(PlayerState { on_ladder: true, vertical_speed: 2., ..Default::default() }),
                   ["ladder_up", "jump", "idle"]);
        assert_eq!(names(PlayerState { on_ladder: true, ..Default::default() })[0], "ladder_stillness");
        assert_eq!(names(PlayerState { on_ladder: true, vertical_speed: -1., ..Default::default() })[0], "ladder_down");
        assert_eq!(names(PlayerState { flying: true, fall_flying: true, ..Default::default() }),
                   ["fly", "elytra_fly", "jump", "idle"]);
        // Death outranks everything; hurt sits between swim_stand and jump.
        let hurt = PlayerState { hurt: true, limb_swing: 0.2, ..ground };
        assert_eq!(hurt.states(), [("attacked", true), ("walk", false), ("idle", false)]);
        assert_eq!(PlayerState { dead: true, sleeping: true, ..hurt }.states()[..2], [("death", true), ("sleep", false)]);
        assert!(PlayerState { riding: true, dead: true, ..ground }.states().is_empty());
    }
    #[test]
    fn clip_set_selects_by_state_and_skips_unusable_clips() {
        // UTF-8 byte order mark, as some packs ship it.
        let mut bytes = b"\xef\xbb\xbf".to_vec();
        bytes.extend_from_slice(br#"{"animations":{
            "idle":{"loop":true,"animation_length":2,"bones":{"root":{"position":{"0":[0,0,0],"2":[2,0,0]}}}},
            "walk":{"animation_length":1,"bones":{"root":{"position":[5,0,0]}}},
            "death":{"loop":true,"animation_length":1,"bones":{"root":{"position":{"0":[0,0,0],"1":[8,0,0]}}}},
            "run":{"bones":{"root":{"position":["ysm.second_order('k', 1, 1, 1, 0)",0,0]}}}}}"#);
        let set = ClipSet::load(&bytes).unwrap();
        assert_eq!(set.len(), 3);
        assert_eq!(set.rejected.len(), 1);
        assert_eq!(set.rejected[0].0, "run");
        assert!(set.get("idle").is_some() && set.get("run").is_none());
        let ground = PlayerState { on_ground: true, ..Default::default() };
        assert_eq!(set.select(&ground).unwrap().0, "idle");
        assert_eq!(set.select(&PlayerState { limb_swing: 0.3, ..ground }).unwrap().0, "walk");
        // `run` matches first but was rejected, so the next matching state plays.
        assert_eq!(set.select(&PlayerState { limb_swing: 0.3, sprinting: true, ..ground }).unwrap().0, "walk");
        // No jump clip: the airborne player falls back to idle.
        assert_eq!(set.select(&PlayerState::default()).unwrap().0, "idle");
        assert!(set.select(&PlayerState { riding: true, ..ground }).is_none());
        // The state's loop mode overrides the clip's: death holds its last frame
        // although the file says loop, and idle wraps.
        let mut m = model();
        let (name, once, clip) = set.select(&PlayerState { dead: true, ..ground }).unwrap();
        assert_eq!((name, once), ("death", true));
        clip.apply_as(&mut m, 5.5, !once, &|_| 0.);
        assert_eq!(m.animation[3], 8.);
        clip.apply_as(&mut m, 5.5, true, &|_| 0.);
        assert_eq!(m.animation[3], 4.);
        set.get("idle").unwrap().apply_as(&mut m, 2.5, true, &|_| 0.);
        assert_eq!(m.animation[3], 0.5);
        assert!(ClipSet::load(br#"{"animations":[]}"#).is_err());
        assert!(ClipSet::load(b"{}").is_err());
        assert!(ClipSet::load(br#"{"animations":{}}"#).unwrap().is_empty());
        // The mark is accepted by the other loaders too.
        let mut geometry = b"\xef\xbb\xbf".to_vec();
        geometry.extend_from_slice(br#"{"minecraft:geometry":[{"bones":[{"name":"a"}]}]}"#);
        assert!(load_geometry(&geometry).is_ok());
        assert!(Clip::load(&bytes, "walk").is_ok());
    }
    #[test]
    fn scalar_keyframes_interpolate_all_axes() {
        let data = serde_json::json!({"0":1,"1":{"pre":3,"post":5},"2":7});
        assert_eq!(track(&data, 0.5, [0.;3]), [2.;3]);
        assert_eq!(track(&data, 1., [0.;3]), [5.;3]);
        assert_eq!(track(&data, 1.5, [0.;3]), [6.;3]);
        let clip = Clip::load(br#"{"animations":{"a":{"bones":{"root":{"scale":{"0":1,"1":{"pre":2,"post":3}}}}}}}"#, "a").unwrap();
        let mut m = model();
        clip.apply(&mut m, 0.5);
        assert_eq!(&m.animation[6..9], &[1.5;3]);
        assert!(frame_vector(&serde_json::json!(1e100)).is_err());
        assert!(vec3(&serde_json::json!([1e100,0,0]), [0.;3]).is_err());
        assert!(Clip::load(br#"{"animations":{"a":{"bones":{"root":{"scale":1e100}}}}}"#, "a").is_err());
    }
    #[test]
    fn gpu_layout_matches_upstream() {
        assert_eq!(std::mem::size_of::<BoneData>(), 144);
        assert_eq!(std::mem::size_of::<Vertex>(), 40);
    }
}

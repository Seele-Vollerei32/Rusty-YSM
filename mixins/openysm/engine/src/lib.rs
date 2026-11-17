//! OpenYSM geometry and animation runtime, independent of Minecraft Java and Horizon.
pub mod bundle;
pub mod molang;
pub mod package;
#[cfg(test)]
mod package_tests;
pub mod resources;
pub mod ysm;
pub mod ysm_binary;
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
        Self::from_document(&parse_json(bytes)?)
    }
    /// The clips of a parsed Bedrock animation document.
    pub fn from_document(doc: &Value) -> Result<Self, String> {
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
    /// Removes a loaded clip, for a caller that plays it on its own.
    pub fn take(&mut self, name: &str) -> Option<Clip> {
        self.clips.remove(name)
    }
    /// OpenYSM's `pre_parallel0..7` (played before the main state) or
    /// `parallel0..7` (after it) clips that are loaded and animate a bone.
    pub fn parallel(&self, pre: bool) -> impl Iterator<Item = &Clip> {
        (0..8).filter_map(move |slot| {
            let name = if pre { format!("pre_parallel{slot}") } else { format!("parallel{slot}") };
            self.clips.get(&name).filter(|clip| !clip.is_empty())
        })
    }
    /// How many channel expressions of loaded clips evaluate as 0 because they are malformed.
    pub fn ignored(&self) -> usize {
        self.clips.values().map(|clip| clip.ignored.len()).sum()
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
            // A bone's inflate and mirror are the defaults of its cubes.
            let inflate = b["inflate"].as_f64().unwrap_or(0.) as f32;
            let mirror = b["mirror"].as_bool().unwrap_or(false);
            if let Some(cubes) = b["cubes"].as_array() {
                for cube in cubes {
                    if cube.is_object() {
                        bake_cube(cube, inflate, mirror, tw, th, &mut bone.quads)?;
                    }
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
/// One cube as OpenYSM's `YSMFolderDeserializer.bakeFaceToRaw` bakes it: in
/// blocks, X mirrored, faces in its order (north, south, east, west, up, down).
/// A cube without `uv` has no faces; a face-UV cube only those it names.
fn bake_cube(c: &Value, bone_inflate: f32, bone_mirror: bool, tw: f32, th: f32, out: &mut Vec<Quad>) -> Result<(), String> {
    let origin = vec3(&c["origin"], [0.; 3])?;
    let size = vec3(&c["size"], [0.; 3])?;
    let inflate = c["inflate"].as_f64().map_or(bone_inflate, |v| v as f32);
    let mirror = c["mirror"].as_bool().unwrap_or(bone_mirror);
    let low = [-origin[0] - size[0] - inflate, origin[1] - inflate, origin[2] - inflate];
    let lo = low.map(|v| v / 16.);
    let hi: [f32; 3] = std::array::from_fn(|i| (low[i] + (size[i] + inflate * 2.)) / 16.);
    // Corner i: bit 2 picks the high X, bit 1 the high Y, bit 0 the high Z.
    let corner = |i: usize| [if i & 4 == 0 { lo[0] } else { hi[0] }, if i & 2 == 0 { lo[1] } else { hi[1] }, if i & 1 == 0 { lo[2] } else { hi[2] }];
    // The cube's own rotation about its pivot: X, then Y, then Z, in the mirrored space.
    let turn = if c.get("rotation").is_some() || c.get("pivot").is_some() {
        let pivot = vec3(&c["pivot"], [0.; 3])?;
        let rotation = vec3(&c["rotation"], [0.; 3])?;
        Some(([-pivot[0] / 16., pivot[1] / 16., pivot[2] / 16.], [-rotation[0].to_radians(), -rotation[1].to_radians(), rotation[2].to_radians()]))
    } else {
        None
    };
    let rotated = |[mut x, mut y, mut z]: [f32; 3], angles: [f32; 3]| {
        let (s, c) = angles[0].sin_cos();
        (y, z) = (y * c - z * s, y * s + z * c);
        let (s, c) = angles[1].sin_cos();
        (x, z) = (x * c + z * s, -x * s + z * c);
        let (s, c) = angles[2].sin_cos();
        (x, y) = (x * c - y * s, x * s + y * c);
        [x, y, z]
    };
    let (dx, dy, dz) = (size[0].floor(), size[1].floor(), size[2].floor());
    let number = |v: &Value| v.as_f64().map(|v| v as f32).ok_or("invalid uv");
    let pair = |v: &Value| -> Result<[f32; 2], String> {
        match v.as_array() {
            None if v.is_null() => Ok([0.; 2]),
            Some(a) if a.len() == 2 => Ok([number(&a[0])?, number(&a[1])?]),
            _ => Err("invalid face uv".into()),
        }
    };
    let box_uv = if c["uv"].is_array() { Some(pair(&c["uv"])?) } else { None };
    let faces: [(&str, &str, [usize; 4], [f32; 3]); 6] = [
        ("north", "north", [2, 6, 4, 0], [0., 0., -1.]),
        ("south", "south", [7, 3, 1, 5], [0., 0., 1.]),
        ("east", "west", [6, 7, 5, 4], [1., 0., 0.]),
        ("west", "east", [3, 2, 0, 1], [-1., 0., 0.]),
        ("up", "up", [3, 7, 6, 2], [0., 1., 0.]),
        ("down", "down", [0, 4, 5, 1], [0., -1., 0.]),
    ];
    for (face, mirrored, corners, normal) in faces {
        // A mirrored cube takes the east face's texture from west and the reverse.
        let name = if mirror { mirrored } else { face };
        let (uv, uv_size) = if let Some([u, v]) = box_uv {
            match name {
                "north" => ([u + dz, v + dz], [dx, dy]),
                "south" => ([u + dz + dx + dz, v + dz], [dx, dy]),
                "east" => ([u, v + dz], [dz, dy]),
                "west" => ([u + dz + dx, v + dz], [dz, dy]),
                "up" => ([u + dz, v], [dx, dz]),
                _ => ([u + dz + dx, v + dz], [dx, -dz]),
            }
        } else if let Some(entry) = c["uv"].get(name) {
            (pair(&entry["uv"])?, pair(&entry["uv_size"])?)
        } else {
            continue;
        };
        let (mut u0, v0, mut u1, v1) = (uv[0] / tw, uv[1] / th, (uv[0] + uv_size[0]) / tw, (uv[1] + uv_size[1]) / th);
        if !mirror {
            std::mem::swap(&mut u0, &mut u1);
        }
        let texture = [[u0, v0], [u1, v0], [u1, v1], [u0, v1]];
        let mut positions = corners.map(corner);
        let mut normal = normal;
        if let Some((pivot, angles)) = turn {
            for position in &mut positions {
                let moved = rotated(std::array::from_fn(|i| position[i] - pivot[i]), angles);
                *position = std::array::from_fn(|i| moved[i] + pivot[i]);
            }
            normal = rotated(normal, angles);
            let length = normal.iter().map(|x| x * x).sum::<f32>().sqrt().max(1e-8);
            normal = normal.map(|x| x / length);
        }
        // The renderer takes a face wound the other way, with the normal pointing in.
        out.push(Quad {
            positions: std::array::from_fn(|i| positions[3 - i]),
            uv: std::array::from_fn(|i| texture[3 - i]),
            normal: normal.map(|x| -x),
        });
    }
    Ok(())
}
/// Mutable state belongs to an entity instance, never to shared clip assets.
#[derive(Default)]
pub struct Animator {
    pub variables: molang::Variables,
    /// What the layers of the current frame wrote, per bone.
    channels: Vec<Channels>,
}
/// Per bone: rotation in degrees, position in pixels and scale, where a layer wrote one.
type Channels = [Option<[f32; 3]>; 3];
impl Animator {
    pub fn reset(&mut self) {
        self.variables.clear();
    }
    /// Call once per evaluated frame, before `apply`, with the entity's render
    /// time in ticks: `ysm.second_order`/`ysm.first_order` values integrate here.
    pub fn advance(&mut self, render_ticks: f32) {
        self.variables.advance(render_ticks);
    }
    pub fn apply(
        &mut self,
        clip: &Clip,
        model: &mut Model,
        time: f32,
        looping: bool,
        query: &impl Fn(&str) -> f32,
    ) {
        self.begin();
        self.layer(clip, model, time, looping, false, query);
        self.finish(model);
    }
    /// Starts a frame of layered clips: `begin`, `layer` in controller order, `finish`.
    pub fn begin(&mut self) {
        self.channels.clear();
    }
    /// Samples one clip over the layers before it, as OpenYSM's controllers do
    /// once their transition has ended: a channel the clip animates replaces
    /// the bone's value. With `add_rotation` (its `parallel` controllers) the
    /// rotation is added to the bone's current one instead.
    pub fn layer(
        &mut self,
        clip: &Clip,
        model: &Model,
        time: f32,
        looping: bool,
        add_rotation: bool,
        query: &impl Fn(&str) -> f32,
    ) {
        clip.sample(model, time, looping, query, &self.variables, add_rotation, &mut self.channels);
    }
    /// Poses the model from the frame's layers; a channel no layer wrote rests.
    pub fn finish(&mut self, model: &mut Model) {
        pose_channels(model, &self.channels);
    }
}
/// Immutable Bedrock animation tracks; unsupported Molang fails explicitly.
pub struct Clip {
    pub length: f32,
    pub looping: bool,
    unbounded_time: bool,
    tracks: Value,
    expressions: std::collections::HashMap<String, molang::Expr>,
    /// `blend_weight`: scales what the clip writes; evaluated once per sample.
    blend_weight: Option<molang::Expr>,
    /// Channel text that is not Molang for OpenYSM either; it evaluates as 0 there and here.
    pub ignored: Vec<String>,
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
        let mut ignored = Vec::new();
        fn compile(
            v: &Value,
            expressions: &mut std::collections::HashMap<String, molang::Expr>,
            ignored: &mut Vec<String>,
        ) -> Result<Value, String> {
            Ok(match v {
                Value::String(s) => {
                    if !expressions.contains_key(s) {
                        let expression = match molang::Expr::parse_checked(s) {
                            Ok(expression) => expression,
                            Err(error) if error.malformed => {
                                ignored.push(s.clone());
                                molang::Expr::Number(0.)
                            }
                            Err(error) => return Err(error.message),
                        };
                        expressions.insert(s.clone(), expression);
                    }
                    Value::from(0)
                }
                Value::Array(a) => Value::Array(
                    a.iter()
                        .map(|v| compile(v, expressions, ignored))
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
                                    compile(v, expressions, ignored)?
                                },
                            ))
                        })
                        .collect::<Result<_, String>>()?,
                ),
                _ => v.clone(),
            })
        }
        let validated = compile(&tracks, &mut expressions, &mut ignored)?;
        let blend_weight = match &a["blend_weight"] {
            Value::Null => None,
            Value::Number(n) => Some(molang::Expr::Number(n.as_f64().unwrap_or(1.) as f32)),
            Value::String(s) => Some(match molang::Expr::parse_checked(s) {
                Ok(expression) => expression,
                Err(error) if error.malformed => {
                    ignored.push(s.clone());
                    molang::Expr::Number(0.)
                }
                Err(error) => return Err(error.message),
            }),
            _ => return Err("invalid blend_weight".into()),
        };
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
            blend_weight,
            ignored,
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
        self.apply_as_with_variables(model, time, looping, query, &molang::Variables::default());
    }
    /// Sample shared animation assets using the caller's entity-local variable scope.
    pub fn apply_as_with_variables(
        &self,
        model: &mut Model,
        time: f32,
        looping: bool,
        query: &impl Fn(&str) -> f32,
        variables: &molang::Variables,
    ) {
        let mut channels = Vec::new();
        self.sample(model, time, looping, query, variables, false, &mut channels);
        pose_channels(model, &channels);
    }
    /// Whether the clip animates no bone.
    pub fn is_empty(&self) -> bool {
        self.tracks.as_object().is_none_or(|bones| bones.is_empty())
    }
    #[allow(clippy::too_many_arguments)]
    fn sample(
        &self,
        model: &Model,
        time: f32,
        looping: bool,
        query: &impl Fn(&str) -> f32,
        variables: &molang::Variables,
        add_rotation: bool,
        channels: &mut Vec<Channels>,
    ) {
        let time = if time.is_finite() { time.max(0.) } else { 0. };
        let time = if self.unbounded_time {
            time
        } else if looping && self.length > 0. && time > self.length {
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
        let evaluate_selected = |v: &Value| evaluate(v, &self.expressions, &query, variables);
        let weight = self.blend_weight.as_ref().map_or(1., |weight| weight.eval_with(&query, variables));
        channels.resize(model.bones.len(), [None; 3]);
        for (bone, written) in model.bones.iter().zip(channels.iter_mut()) {
            let b = &self.tracks[&bone.name];
            if !b["rotation"].is_null() {
                let r = track_evaluated(&b["rotation"], time, [0.; 3], &evaluate_selected).map(|v| v * weight);
                written[0] = Some(match written[0] {
                    Some(current) if add_rotation => std::array::from_fn(|i| current[i] + r[i]),
                    _ => r,
                });
            }
            if !b["position"].is_null() {
                written[1] = Some(track_evaluated(&b["position"], time, [0.; 3], &evaluate_selected).map(|v| v * weight));
            }
            if !b["scale"].is_null() {
                // OpenYSM weighs a scale towards 1, not towards 0.
                written[2] = Some(track_evaluated(&b["scale"], time, [1.; 3], &evaluate_selected).map(|v| 1. + (v - 1.) * weight));
            }
        }
    }
}
fn pose_channels(model: &mut Model, channels: &[Channels]) {
    let mut index = 0;
    model.pose(|bone| {
        let written = channels.get(index).copied().unwrap_or([None; 3]);
        index += 1;
        let mut r = written[0].unwrap_or([0.; 3]);
        r[0] = -r[0];
        r[1] = -r[1];
        (
            std::array::from_fn(|i| bone.rotation[i] + r[i].to_radians()),
            written[1].unwrap_or([0.; 3]),
            written[2].unwrap_or([1.; 3]),
            false,
        )
    });
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
#[cfg(test)]
fn track(v: &Value, time: f32, default: [f32; 3]) -> [f32; 3] {
    track_evaluated(v, time, default, &Value::clone)
}
fn track_evaluated(
    v: &Value,
    time: f32,
    default: [f32; 3],
    evaluate: &impl Fn(&Value) -> Value,
) -> [f32; 3] {
    if v.is_null() {
        return default;
    }
    if let Some(n) = v.as_f64() {
        return [n as f32; 3];
    }
    if v.is_array() || v.is_string() {
        return frame_vector(&evaluate(v)).unwrap_or(default);
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
    let post = |v: &Value| frame_vector(&evaluate(v.get("post").unwrap_or(v))).unwrap_or(default);
    let pre = |v: &Value| {
        v.get("pre").map_or_else(
            || post(v),
            |p| frame_vector(&evaluate(p)).unwrap_or(default),
        )
    };
    if time < first.0 {
        return pre(first.1);
    }
    for (index, w) in keys.windows(2).enumerate() {
        if time < w[1].0 {
            let t = ((time - w[0].0) / (w[1].0 - w[0].0)).clamp(0., 1.);
            if t < 0.00001 {
                return post(w[0].1);
            }
            if t > 0.99999 {
                return post(w[1].1);
            }
            if w[1].1["lerp_mode"].as_str() == Some("catmullrom")
                || w[0].1["lerp_mode"].as_str() == Some("catmullrom")
            {
                let p0 = post(keys[index.saturating_sub(1)].1);
                let p1 = post(w[0].1);
                let p2 = pre(w[1].1);
                let p3 = pre(keys[(index + 2).min(keys.len() - 1)].1);
                return std::array::from_fn(|i| {
                    0.5 * ((2. * p1[i])
                        + (-p0[i] + p2[i]) * t
                        + (2. * p0[i] - 5. * p1[i] + 4. * p2[i] - p3[i]) * t * t
                        + (-p0[i] + 3. * p1[i] - 3. * p2[i] + p3[i]) * t * t * t)
                });
            }
            // Upstream linear evaluation reads the destination before the source.
            let p2 = pre(w[1].1);
            let p1 = post(w[0].1);
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
        a.apply(&mut m, 1.);
        assert!((m.render(&IDENTITY, 0, 0)[0].position[0] + 1.).abs() < 0.001);
        a.apply(&mut m, 1.25);
        assert!((m.render(&IDENTITY, 0, 0)[0].position[0] + 0.25).abs() < 0.001);
    }
    #[test]
    fn malformed_channels_read_zero_and_unsupported_molang_rejects_the_clip() {
        let set = ClipSet::load(br#"{"animations":{
            "stray":{"animation_length":1,"bones":{"root":{"position":["(1+2))+3","4*2",0],"rotation":["(1+2))+3",0,0]}}},
            "scope":{"bones":{"root":{"position":["{v.x=1;}",0,0]}}},
            "bone":{"bones":{"root":{"position":["ysm.bone_rot('root')",0,0]}}}}}"#).unwrap();
        assert_eq!(set.len(), 1);
        assert_eq!(set.rejected.len(), 2);
        assert_eq!(set.ignored(), 1);
        let clip = set.get("stray").unwrap();
        assert_eq!(clip.ignored, ["(1+2))+3"]);
        let mut m = model();
        clip.apply(&mut m, 0.);
        let moved = m.render(&IDENTITY, 0, 0)[0].position;
        m.pose(|b| (b.rotation, [0.; 3], [1.; 3], false));
        let rest = m.render(&IDENTITY, 0, 0)[0].position;
        // x reads 0 as upstream; y is 8 model units, half a block.
        assert!((moved[0] - rest[0]).abs() < 1e-5);
        assert!((moved[1] - rest[1] - 0.5).abs() < 1e-5, "{moved:?} {rest:?}");
    }
    #[test]
    fn layers_override_channels_and_parallel_adds_rotation() {
        let set = ClipSet::load(br#"{"animations":{
            "pre_parallel0":{"loop":true,"bones":{"root":{"rotation":[10,0,0],"position":[1,2,3],"scale":2},"child":{"rotation":[0,0,30]}}},
            "pre_parallel3":{"bones":{}},
            "idle":{"loop":true,"bones":{"root":{"rotation":[0,20,0]}}},
            "parallel1":{"bones":{"root":{"rotation":[5,5,5],"position":[7,0,0]}}},
            "parallel0":{"blend_weight":"v.w ?? 0.5","bones":{"child":{"rotation":[0,0,10],"position":[4,4,4],"scale":[3,3,3]}}},
            "parallel2":{"blend_weight":"(1))","bones":{"root":{"position":[9,9,9]}}},
            "parallel9":{"bones":{"root":{"position":[8,8,8]}}},
            "parallel4":{"blend_weight":"ysm.bone_rot('x')","bones":{"root":{"position":[8,8,8]}}}}}"#).unwrap();
        // Slot order, without the empty clip, the rejected one and names outside 0..7.
        assert_eq!(set.parallel(true).count(), 1);
        assert_eq!(set.parallel(false).count(), 3);
        assert_eq!(set.rejected.len(), 1);
        assert_eq!(set.ignored(), 1);
        let mut m = model();
        let mut animator = Animator::default();
        let absent = |_: &str| f32::NAN;
        let frame = |animator: &mut Animator, m: &mut Model, add: bool| {
            animator.begin();
            for clip in set.parallel(true) {
                animator.layer(clip, m, 0., true, false, &absent);
            }
            animator.layer(set.get("idle").unwrap(), m, 0., true, false, &absent);
            for clip in set.parallel(false) {
                animator.layer(clip, m, 0., true, add, &absent);
            }
            animator.finish(m);
            m.animation.clone()
        };
        let close = |actual: &[f32], expected: [f32; 3]| {
            assert!(actual.iter().zip(expected).all(|(a, e)| (a - e).abs() < 1e-5), "{actual:?} {expected:?}");
        };
        let pose = frame(&mut animator, &mut m, true);
        // root: idle replaces the pre layer's rotation, parallel1 adds to it;
        // x and y are negated on the way to the renderer.
        close(&pose[0..3], [(-5f32).to_radians(), (-25f32).to_radians(), 5f32.to_radians()]);
        // A malformed weight is 0: parallel2 writes a zero position over parallel1's.
        close(&pose[3..6], [0.; 3]);
        close(&pose[6..9], [2.; 3]);
        // child: half of parallel0, added to the pre layer's rotation; scale weighs towards 1.
        close(&pose[12..15], [0., 0., 35f32.to_radians()]);
        close(&pose[15..18], [2.; 3]);
        close(&pose[18..21], [2.; 3]);
        // Without the parallel mode a later layer replaces the rotation as well.
        let pose = frame(&mut animator, &mut m, false);
        close(&pose[0..3], [(-5f32).to_radians(), (-5f32).to_radians(), 5f32.to_radians()]);
        close(&pose[12..15], [0., 0., 5f32.to_radians()]);
        // No layer: the rest pose.
        animator.begin();
        animator.finish(&mut m);
        close(&m.animation[0..3], [0.; 3]);
        close(&m.animation[6..9], [1.; 3]);
        close(&m.animation[12..15], [0.; 3]);
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
        // Hierarchy order evaluates root before child; the child sees this frame's assignment.
        let clip = Clip::load(br#"{"animations":{"idle":{"animation_length":10,"bones":{
            "root":{"position":["v.bv=query.anim_time*2",0,0]},
            "child":{"position":["v.bv",0,0]}}}}}"#, "idle").unwrap();
        let mut animator = Animator::default();
        animator.apply(&clip, &mut m, 1., false, &|_| 0.);
        assert_eq!(m.animation[3], 2.);
        assert_eq!(m.animation[12 + 3], 2.);
        animator.apply(&clip, &mut m, 3., false, &|_| 0.);
        assert_eq!(m.animation[3], 6.);
        assert_eq!(m.animation[12 + 3], 6.);
        // A separately loaded clip has its own scope.
        let other = Clip::load(br#"{"animations":{"a":{"bones":{"root":{"position":["v.bv",0,0]}}}}}"#, "a").unwrap();
        other.apply(&mut m, 0.);
        assert_eq!(m.animation[3], 0.);
    }
    #[test]
    fn only_sampled_keyframes_execute_in_upstream_order() {
        let clip = Clip::load(br#"{"animations":{"a":{"bones":{"root":{"position":{"0":["v.x=v.x+1",0,0],"1":["v.x=v.x*10+2",0,0],"2":["v.x=999",0,0]}}}}}}"#, "a").unwrap();
        let mut m = model();
        let mut animator = Animator::default();
        animator.apply(&clip, &mut m, 0.5, false, &|_| 0.);
        assert_eq!(m.animation[3], 2.5);
        assert_eq!(animator.variables.number("variable.x"), Some(3.));
        animator.reset();
        animator.apply(&clip, &mut m, 0., false, &|_| 0.);
        assert_eq!(m.animation[3], 1.);
        assert_eq!(animator.variables.number("variable.x"), Some(1.));
    }
    #[test]
    fn shared_clip_instances_do_not_leak_variables() {
        fn assert_shared<T: Send + Sync>() {}
        assert_shared::<Clip>();
        assert_shared::<ClipSet>();
        let clip = Clip::load(
            br#"{"animations":{"a":{"bones":{"root":{"position":["v.x=v.x+1",0,0]}}}}}"#,
            "a",
        )
        .unwrap();
        let mut m = model();
        let mut first = Animator::default();
        let mut second = Animator::default();
        first.apply(&clip, &mut m, 0., false, &|_| 0.);
        assert_eq!(m.animation[3], 1.);
        second.apply(&clip, &mut m, 0., false, &|_| 0.);
        assert_eq!(m.animation[3], 1.);
        first.apply(&clip, &mut m, 1., false, &|_| 0.);
        assert_eq!(m.animation[3], 2.);
        second.reset();
        second.apply(&clip, &mut m, 1., false, &|_| 0.);
        assert_eq!(m.animation[3], 1.);
        clip.apply(&mut m, 0.);
        assert_eq!(m.animation[3], 1.);
        clip.apply(&mut m, 0.);
        assert_eq!(m.animation[3], 1.);
    }
    #[test]
    fn states_follow_openysm_priority_order() {
        let names = |s: PlayerState| s.states().into_iter().map(|x| x.0).collect::<Vec<_>>();

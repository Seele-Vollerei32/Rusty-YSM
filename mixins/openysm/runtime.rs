use std::cell::RefCell;
#[derive(Default)]
struct CloudState {
    disabled: bool,
    players: std::collections::BTreeMap<u32,String>,
    generation: u64,
    last_poll: Option<std::time::Instant>,
    expires: f64,
    models: std::collections::HashMap<u32,(String,Loaded)>,
}
pub fn cloud_update(directory: &std::path::Path, players: std::collections::BTreeMap<u32,String>,
                    atlas: &mut crate::hud::HudAtlas) -> Vec<crate::font::Patch> {
    CLOUD.with(|state| {
        let mut state = state.borrow_mut();
        if state.disabled {
            // Withdraw the worker's input even if an earlier request is in flight.
            let _ = std::fs::remove_file(directory.join("request.json"));
            return Vec::new();
        }
        if state.players != players {
            state.players = players;
            state.generation = state.generation.wrapping_add(1);
            state.models.clear();
            state.last_poll = None;
        }
        if state.last_poll.is_some_and(|t|t.elapsed().as_millis()<500) {return Vec::new();}
        state.last_poll = Some(std::time::Instant::now());
        let request = serde_json::json!({"generation":state.generation,"players":state.players});
        let temporary = directory.join("request.tmp");
        if std::fs::write(&temporary,request.to_string()).is_ok() {
            // Windows rename does not replace a destination. The worker tolerates
            // a missing request between removal and publication.
            let destination=directory.join("request.json");
            let _ = std::fs::remove_file(&destination);
            let _ = std::fs::rename(&temporary,destination);
        }
        let mut patches = Vec::new();
        let Ok(bytes) = std::fs::read(directory.join("state.json")) else {return patches;};
        let Ok(document) = serde_json::from_slice::<serde_json::Value>(&bytes) else {return patches;};
        if document["generation"].as_u64()!=Some(state.generation) {return patches;}
        state.expires = document["expires"].as_f64().filter(|v|v.is_finite()).unwrap_or(0.);
        let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_secs_f64();
        if state.expires<=now {state.models.clear();return patches;}
        let Some(entries)=document["players"].as_object() else {state.models.clear();return patches;};
        let ids: std::collections::HashSet<_> = state.players.keys().copied().collect();
        state.models.retain(|id,_|ids.contains(id)&&entries.contains_key(&id.to_string()));
        for (slot,(id,uuid)) in state.players.clone().iter().enumerate().take(16) {
            let row=&document["players"][id.to_string()];
            if row["uuid"].as_str()!=Some(uuid.as_str()) {state.models.remove(id);continue;}
            let Some(key)=row["key"].as_str() else {continue;};
            if let Some((old, loaded)) = state.models.get_mut(id) {
                if old == key {
                    // Dynamic inputs change without reloading geometry, clips or GPU pixels.
                    loaded.motion = row["motion"].as_object().map(|_| row["motion"].clone());
                    loaded.display_state = row["display_state"].as_object().map(|_| row["display_state"].clone());
                    continue;
                }
            }
            state.models.remove(id);
            let loaded=(|| -> Result<(Loaded,crate::font::Patch),String> {
                let model_path=row["model"].as_str().ok_or("missing model")?;
                // The sync worker already validated the PNG and produced RGBA.
                let w=row["width"].as_u64().filter(|w|*w>0&&*w<=512).ok_or("bad texture width")? as u32;
                let h=row["height"].as_u64().filter(|h|*h>0&&*h<=512).ok_or("bad texture height")? as u32;
                let expected=w as usize*h as usize*4;
                if expected>512*512*4 {return Err("cloud texture exceeds 512px slot".into());}
                let rgba_path=row["rgba"].as_str().ok_or("missing texture")?;
                let metadata=std::fs::metadata(rgba_path).map_err(|e|e.to_string())?;
                if metadata.len()!=expected as u64 {return Err("texture byte length mismatch".into());}
                let pixels=std::fs::read(rgba_path).map_err(|e|e.to_string())?;
                let model_bytes=std::fs::read(model_path).map_err(|e|e.to_string())?;
                if model_bytes.len()>64*1024*1024 {return Err("model size".into());}
                let model=openysm_runtime::load_geometry(&model_bytes)?;
                let clip = if let Some(animation) = row["animation"].as_str() {
                    let animation_path = row["animation_path"].as_str().ok_or("missing animation path")?;
                    let animation_bytes = std::fs::read(animation_path).map_err(|e|e.to_string())?;
                    Some(openysm_runtime::Clip::load(&animation_bytes, animation)?)
                } else { None };
                // Without a selected motion clip the pack's states drive the model.
                let clips = if clip.is_none() {
                    row["animation_path"].as_str().and_then(|path| load_clips(std::path::Path::new(path)))
                } else { None };
                let texture=format!("openysm/cloud{slot}");
                let original=*atlas.sprites.get(&texture).ok_or("cloud atlas slot missing")?;
                let mut rect=original;
                rect.u1=rect.u0+w as f32/atlas.width as f32;
                rect.v1=rect.v0+h as f32/atlas.height as f32;
                rect.w=w;rect.h=h;
                let patch=crate::font::Patch{x:(rect.u0*atlas.width as f32).round() as u32,
                    y:(rect.v0*atlas.height as f32).round() as u32,width:w,height:h,rgba:pixels};
                atlas.sprites.insert(texture.clone(),rect);
                Ok((Loaded{model,clip,clips,playing:Default::default(),animators:Default::default(),
                            motion:row["motion"].as_object().map(|_|row["motion"].clone()),
                            display_state:row["display_state"].as_object().map(|_|row["display_state"].clone()),
                            texture},patch))
            })();
            match loaded {
                Ok((loaded,patch))=>{state.models.insert(*id,(key.into(),loaded));patches.push(patch);},
                Err(error)=>eprintln!("[openysm] Cloud model rejected: {error}"),
            }
        }
        patches
    })
}
struct Loaded {
    model: openysm_runtime::Model,
    /// One fixed clip (`OPENYSM_CLIP`, or a Cloud motion). It takes precedence.
    clip: Option<openysm_runtime::Clip>,
    /// The pack's main animations, chosen by the player's state.
    clips: Option<openysm_runtime::ClipSet>,
    /// Per mob: the state now playing and the age (seconds) it started at.
    playing: std::collections::HashMap<u32,(&'static str,f32)>,
    animators: std::collections::HashMap<u32,(f32,openysm_runtime::Animator)>,
    motion: Option<serde_json::Value>,
    display_state: Option<serde_json::Value>,
    texture: String,
}
/// The local player's state that its model mob does not carry.
#[derive(Clone, Copy, Debug, Default)]
pub struct LocalPlayer {
    pub id: u32,
    pub on_ground: bool,
    pub sprinting: bool,
    pub flying: bool,
    pub swimming: bool,
    pub swim_pose: bool,
    pub sleeping: bool,
    pub in_water: bool,
    pub dead: bool,
    pub vertical_speed: f32,
}
/// Retire state when players leave or an entity id belongs to a different UUID.
pub fn sync_players(players: &std::collections::BTreeMap<u32,String>) {
    PLAYER_IDENTITIES.with(|identities| {
        let mut identities = identities.borrow_mut();
        MODEL.with(|slot| {
            if let Some(model) = slot.borrow_mut().as_mut() {
                model.animators.retain(|id,_| players.contains_key(id) && identities.get(id) == players.get(id));
                model.playing.retain(|id,_| players.contains_key(id) && identities.get(id) == players.get(id));
            }
        });
        *identities = players.clone();
    });
}
pub fn local_player(state: Option<LocalPlayer>) {
    LOCAL.with(|local| local.set(state));
}
fn load_clips(path: &std::path::Path) -> Option<openysm_runtime::ClipSet> {
    let result = std::fs::read(path).map_err(|e|e.to_string()).and_then(|bytes| {
        if bytes.len() > 64*1024*1024 {return Err("animation file size".into());}
        openysm_runtime::ClipSet::load(&bytes)
    });
    match result {
        Ok(set) => {
            if !set.rejected.is_empty() {
                eprintln!("[openysm] {} animation clips loaded, {} unsupported (first: {} - {})",
                          set.len(),set.rejected.len(),set.rejected[0].0,set.rejected[0].1);
            }
            if set.ignored() > 0 {
                eprintln!("[openysm] {} malformed animation expressions read as 0, as in OpenYSM",set.ignored());
            }
            Some(set)
        },
        Err(error) => {eprintln!("[openysm] animations: {error}");None}
    }
}
/// OpenYSM's main-state inputs for a player mob. A remote player is read from
/// what the server synced; `flying` is only known for the local player or from
/// a Cloud display state.
fn player_state(m: &crate::mobs::Mob, alpha: f32, flying: bool) -> openysm_runtime::PlayerState {
    use crate::pose::Pose;
    let pose = m.extra.avatar_pose;
    let in_water = m.fluids.in_water;
    let mut state = openysm_runtime::PlayerState {
        riding: m.extra.passenger,
        dead: m.health <= 0. || m.death_time > 0,
        spin_attack: m.extra.auto_spin_attack,
        sleeping: pose == Pose::Sleeping,
        // `Entity.isSwimming` is not synced separately: in water, the pose implies it.
        swimming: pose == Pose::Swimming && in_water,
        swim_pose: pose == Pose::Swimming,
        on_ladder: m.climbing,
        vertical_speed: ((m.position.y-m.previous.y)*20.) as f32,
        flying,
        fall_flying: m.extra.fall_flying,
        in_water,
        on_ground: m.on_ground,
        hurt: m.hurt_time > 0,
        crouching: m.extra.crouching || pose == Pose::Crouching,
        sprinting: m.extra.sprinting,
        limb_swing: m.previous_limb_speed+(m.limb_speed-m.previous_limb_speed)*alpha,
    };
    if let Some(local) = LOCAL.with(|local| local.get()).filter(|local| local.id == m.id) {
        state.dead = local.dead;
        state.sleeping = local.sleeping;
        state.swimming = local.swimming;
        state.swim_pose = local.swim_pose;
        state.vertical_speed = local.vertical_speed;
        state.flying = local.flying;
        state.in_water = local.in_water;
        state.on_ground = local.on_ground;
        state.sprinting = local.sprinting;
        // The local model mob has no climbing flag; ladder states stay off.
        state.on_ladder = false;
    }
    state
}
thread_local! {
    static LOCAL: std::cell::Cell<Option<LocalPlayer>> = const { std::cell::Cell::new(None) };
    static CLOUD: RefCell<CloudState> = RefCell::new(CloudState::default());
    static MODEL: RefCell<Option<Loaded>> = RefCell::new(load());
    static PLAYER_IDENTITIES: RefCell<std::collections::BTreeMap<u32,String>> = RefCell::new(Default::default());
    static PACK: RefCell<PackState> = RefCell::new(PackState::default());
    // Index 0 is the right arm. The mask selects one part group of a shared model.
    static ARMS: RefCell<[Option<(openysm_runtime::Model,u32)>;2]> = RefCell::new([
        load_arm("OPENYSM_RIGHT_ARM",2), load_arm("OPENYSM_LEFT_ARM",1)
    ]);
}
/// A per-side file is drawn whole. Otherwise `OPENYSM_ARM` names a pack's arm
/// model, where OpenYSM's `LeftArm`/`RightArm` bone groups pick the side.
fn load_arm(key: &str, part: u32) -> Option<(openysm_runtime::Model,u32)> {
    let read = |name: &str| {
        let path = std::env::var_os(name)?;
        match std::fs::read(path).map_err(|e|e.to_string()).and_then(|bytes|openysm_runtime::load_geometry(&bytes)) {
            Ok(model) => Some(model),
            Err(error) => {eprintln!("[openysm] {name}: {error}");None}
        }
    };
    if std::env::var_os(key).is_some() {
        return read(key).map(|model|(model,0));
    }
    let model = read("OPENYSM_ARM")?;
    if !model.has_part(part) {
        eprintln!("[openysm] OPENYSM_ARM has no {} geometry; keeping the original arm",
                  if part == 1 {"LeftArm"} else {"RightArm"});
        return None;
    }
    Some((model,part))
}
// Draws the first-person arm from the body model's texture; false keeps Horizon's arm.
pub fn render_arm(left: bool, matrix: glam::Mat4, atlas: crate::mobs::render::Atlas<'_>,
                  light: u32, out: &mut Vec<crate::mesher::Vertex>) -> bool {
    MODEL.with(|body| {
        let body = body.borrow();
        let Some(body) = body.as_ref() else {return false;};
        let Some(uv) = atlas(&body.texture) else {return false;};
        ARMS.with(|arms| {
            let mut arms = arms.borrow_mut();
            let Some((model,part)) = arms[usize::from(left)].as_mut() else {return false;};
            let part = *part;
            model.pose(|bone|(bone.rotation,[0.;3],[1.;3],false));
            // A pack arm keeps OpenYSM's shoulder-space geometry and hand offset;
            // a per-side file is authored around the hand origin.
            let (root,order) = if part != 0 {
                (matrix * glam::Mat4::from_cols_array(&openysm_runtime::arm_placement(left)),[0,1,2,3])
            } else {
                (matrix * glam::Mat4::from_scale(glam::Vec3::new(1.,-1.,1.)),[3,2,1,0])
            };
            for quad in model.render(&root.to_cols_array(),light,part).chunks_exact(4) {
                if quad[0].hidden != 0 {continue;}
                for i in order {
                    let v = quad[i];
                    out.push(crate::mesher::Vertex {
                        pos:v.position, uv:[uv[0]+(uv[2]-uv[0])*v.uv[0],uv[1]+(uv[3]-uv[1])*v.uv[1]],
                        layer:0,color:0xffffffff,
                        light:(v.light as u32 & 255)|((v.light as u32 >> 16 & 255)<<8),
                        normal:[(v.normal[0]*127.) as i8,(v.normal[1]*127.) as i8,(v.normal[2]*127.) as i8,0],
                        glint_uv:[0.;2],
                    });
                }
            }
            true
        })
    })
}
/// The atlas slot `texture.rs` reserves for a pack's texture.
const PACK_SLOT: &str = "openysm/pack";
/// Present only while the slot holds the selected texture. The body and arms
/// draw with it, so a rebuilt atlas keeps Horizon's own player until the upload.
const PACK_LIVE: &str = "openysm/pack_live";
struct PackTexture { width: u32, height: u32, rgba: Vec<u8> }
type BuiltPack = (Loaded,[Option<(openysm_runtime::Model,u32)>;2],Option<PackTexture>,openysm_runtime::bundle::ModelBundle);
/// A model pack read by the engine: a directory or a ZIP with `ysm.json`.
#[derive(Default)]
struct PackState {
    path: Option<std::path::PathBuf>,
    /// The selected texture's file stem; None is the manifest's default.
    wanted_texture: Option<String>,
    clip: Option<String>,
    /// What is left of the pack after the model took its main animations.
    bundle: Option<openysm_runtime::bundle::ModelBundle>,
    texture: Option<PackTexture>,
    /// The slot must be written again.
    dirty: bool,
    service: Option<openysm_runtime::resources::ResourceService>,
    /// The background load in flight: generation, path, texture, clip.
    pending: Option<(u64,std::path::PathBuf,Option<String>,Option<String>)>,
    warned: bool,
}
fn decode_pack_texture(bytes: &[u8]) -> Result<PackTexture,String> {
    // Bound dimensions before decode allocates the RGBA image.
    if bytes.len() > 32*1024*1024 {return Err("texture file exceeds 32 MiB".into());}
    if bytes.len() < 24 || &bytes[..8] != b"\x89PNG\r\n\x1a\n" || &bytes[12..16] != b"IHDR" {
        return Err("expected PNG IHDR".into());
    }
    let width = u32::from_be_bytes(bytes[16..20].try_into().unwrap());
    let height = u32::from_be_bytes(bytes[20..24].try_into().unwrap());
    if width == 0 || height == 0 || width > 4096 || height > 4096 {
        return Err("texture dimensions must be within 1..4096".into());
    }
    let (width,height,rgba) = crate::hud::decode_bytes(bytes).map_err(|e|e.to_string())?;
    if rgba.len() != width as usize*height as usize*4 {return Err("texture pixel count mismatch".into());}
    Ok(PackTexture{width,height,rgba})
}
/// Body, arms and texture of a pack. `clip` pins one clip; without it the
/// pack's main animations follow the player's state.
fn build_pack(mut bundle: openysm_runtime::bundle::ModelBundle, texture: Option<&str>, clip: Option<&str>) -> Result<BuiltPack,String> {
    let model = bundle.model()?;
    let mut arms = [None,None];
    for (slot,part) in [(0,2),(1,1)] {
        arms[slot] = bundle.arm()?.filter(|arm|arm.has_part(part)).map(|arm|(arm,part));
    }
    let pixels = match bundle.texture_index(texture)? {
        Some(index) => Some(decode_pack_texture(bundle.texture(index)?)?),
        None => None,
    };
    let (clip,clips) = match clip {
        Some(name) => {
            let role = bundle.find_clip(name)?.ok_or_else(||format!("clip {name} is not in the pack"))?.0.to_owned();
            (bundle.animations.get_mut(&role).and_then(|set|set.take(name)),None)
        },
        None => (None,bundle.animations.remove("main")),
    };
    if let Some(first) = bundle.diagnostics.first() {
        eprintln!("[openysm] {} animation clips unsupported (first: {first})",bundle.diagnostics.len());
    }
    let ignored: usize = clips.iter().map(|set|set.ignored()).sum();
    if ignored > 0 {
        eprintln!("[openysm] {ignored} malformed animation expressions read as 0, as in OpenYSM");
    }
    let texture = if pixels.is_some() {PACK_LIVE} else {"entity/player/wide/steve"}.to_owned();
    Ok((Loaded{model,clip,clips,playing:Default::default(),animators:Default::default(),motion:None,display_state:None,texture},arms,pixels,bundle))
}
/// Starts reading a pack on the resource worker; `pack_update` installs it.
fn request_pack(path: std::path::PathBuf, texture: Option<String>, clip: Option<String>) -> bool {
    // The startup pack loads with the model slot, before this state is borrowed.
    MODEL.with(|_|{});
    PACK.with(|state| {
        let mut state = state.borrow_mut();
        if state.service.is_none() {
            match openysm_runtime::resources::ResourceService::new(Default::default()) {
                Ok(service) => state.service = Some(service),
                Err(error) => {eprintln!("[openysm] resource worker: {error}");return false;}
            }
        }
        let generation = state.service.as_ref().unwrap().request(path.clone());
        state.pending = Some((generation,path,texture,clip));
        true
    })
}
fn select_pack_texture(name: &str) -> bool {
    MODEL.with(|_|{});
    PACK.with(|state| {
        let mut state = state.borrow_mut();
        let state = &mut *state;
        let Some(bundle) = state.bundle.as_ref() else {
            eprintln!("[openysm] no pack is loaded");
            return false;
        };
        let pixels = bundle.texture_index(Some(name))
            .and_then(|index|bundle.texture(index.unwrap_or(0)))
            .and_then(decode_pack_texture);
        match pixels {
            Ok(pixels) => {
                state.texture = Some(pixels);
                state.wanted_texture = Some(name.to_owned());
                state.dirty = true;
                true
            },
            Err(error) => {eprintln!("[openysm] texture change failed: {error}");false}
        }
    })
}
/// Installs a pack the worker finished and writes the selected texture into
/// its atlas slot. Called every tick; the returned patches go to the GPU atlas.
pub fn pack_update(atlas: &mut crate::hud::HudAtlas) -> Vec<crate::font::Patch> {
    MODEL.with(|_|{});
    PACK.with(|state| {
        let mut state = state.borrow_mut();
        let state = &mut *state;
        let done = state.service.as_ref().and_then(|service|service.try_recv());
        if let Some(done) = done {
            if state.pending.as_ref().is_some_and(|pending|pending.0 == done.generation) {
                let (_,path,texture,clip) = state.pending.take().unwrap();
                let built = done.result
                    .and_then(|bundle|std::sync::Arc::try_unwrap(bundle).map_err(|_|"pack is still in use".to_string()))
                    .and_then(|bundle|build_pack(bundle,texture.as_deref(),clip.as_deref()));
                match built {
                    Ok((loaded,arms,pixels,bundle)) => {
                        MODEL.with(|slot| *slot.borrow_mut() = Some(loaded));
                        ARMS.with(|slot| *slot.borrow_mut() = arms);
                        eprintln!("[openysm] pack loaded: {}",path.display());
                        state.path = Some(path);
                        state.wanted_texture = texture;
                        state.clip = clip;
                        state.texture = pixels;
                        state.bundle = Some(bundle);
                        state.dirty = true;
                    },
                    // The model in use stays.
                    Err(error) => eprintln!("[openysm] pack rejected: {error}"),
                }
            }
        }
        if state.dirty {
            atlas.sprites.remove(PACK_LIVE);
            state.dirty = false;
        }
        if atlas.sprites.contains_key(PACK_LIVE) {return Vec::new();}
        let Some(texture) = state.texture.as_ref() else {return Vec::new();};
        let Some(slot) = atlas.sprites.get(PACK_SLOT).copied() else {
            if !state.warned {
                eprintln!("[openysm] no pack texture slot: start with OPENYSM_PACK or OPENYSM_PACK_SLOT set");
                state.warned = true;
            }
            return Vec::new();
        };
        if texture.width > slot.w || texture.height > slot.h {
            eprintln!("[openysm] pack texture {}x{} exceeds the {}x{} slot; raise OPENYSM_PACK_SLOT",
                      texture.width,texture.height,slot.w,slot.h);
            state.texture = None;
            return Vec::new();
        }
        let mut rect = slot;
        rect.u1 = rect.u0+texture.width as f32/atlas.width as f32;
        rect.v1 = rect.v0+texture.height as f32/atlas.height as f32;
        rect.w = texture.width;
        rect.h = texture.height;
        atlas.sprites.insert(PACK_LIVE.into(),rect);
        eprintln!("[openysm] pack texture {}x{} uploaded",texture.width,texture.height);
        vec![crate::font::Patch{x:(slot.u0*atlas.width as f32).round() as u32,
            y:(slot.v0*atlas.height as f32).round() as u32,
            width:texture.width,height:texture.height,rgba:texture.rgba.clone()}]
    })
}
pub fn command(command: &str) -> bool {
    if let Some(path) = command.strip_prefix("pack ") {
        return request_pack(std::path::PathBuf::from(path.trim()),None,None);
    }
    if let Some(name) = command.strip_prefix("texture ") {
        return select_pack_texture(name.trim());
    }
    MODEL.with(|_|{});
    let current = PACK.with(|state| {
        let mut state = state.borrow_mut();
        match command {
            "off" => {
                if let Some(service) = state.service.as_ref() {service.cancel();}
                state.pending = None;
                state.path = None;
                state.bundle = None;
                state.texture = None;
                state.dirty = true;
                None
            },
            "reload" => state.path.clone().map(|path|(path,state.wanted_texture.clone(),state.clip.clone())),
            "list" => {
                if let (Some(path),Some(bundle)) = (state.path.as_ref(),state.bundle.as_ref()) {
                    eprintln!("[openysm] pack {}; textures: {}",path.display(),bundle.textures().join(", "));
                }
                None
            },
            _ => None,
        }
    });
    if let Some((path,texture,clip)) = current {
        return request_pack(path,texture,clip);
    }
    MODEL.with(|slot| {
        let mut slot = slot.borrow_mut();
        match command {
            "off" => { *slot = None; ARMS.with(|arms| *arms.borrow_mut() = [None,None]); true },
            "reload" => {
                if let Some(loaded) = load() {
                    ARMS.with(|arms| *arms.borrow_mut() = [load_arm("OPENYSM_RIGHT_ARM",2),load_arm("OPENYSM_LEFT_ARM",1)]);
                    *slot = Some(loaded); true
                } else { false }
            },
            "list" => {
                eprintln!("[openysm] active model: {}", slot.as_ref().map_or("off", |_| "loaded"));
                true
            },
            "cloud off" | "cloud on" => {
                CLOUD.with(|cloud| {
                    let mut cloud = cloud.borrow_mut();
                    cloud.disabled = command == "cloud off";
                    cloud.models.clear();
                    cloud.expires = 0.;
                    cloud.generation = cloud.generation.wrapping_add(1);
                    cloud.last_poll = None;
                });
                if let Some(directory) = std::env::var_os("SPM_BRIDGE_DIR") {
                    let _ = std::fs::remove_file(std::path::Path::new(&directory).join("request.json"));
                }
                true
            },
            _ => {
                if let Some(path) = command.strip_prefix("model ") {
                    let result = std::fs::read(path).map_err(|e|e.to_string())
                        .and_then(|bytes|openysm_runtime::load_geometry(&bytes));
                    match result {
                        Ok(model) => {
                            ARMS.with(|arms| *arms.borrow_mut() = [None,None]);
                            let texture = slot.as_ref().map(|m|m.texture.clone()).unwrap_or_else(||
                                if std::env::var_os("OPENYSM_TEXTURE_FILE").is_some() {"openysm/model".into()} else {"entity/player/wide/steve".into()});
                            *slot = Some(Loaded {model,clip:None,clips:None,playing:Default::default(),animators:Default::default(),motion:None,display_state:None,texture}); true
                        },
                        Err(error) => {eprintln!("[openysm] model change failed: {error}");false}
                    }
                } else {eprintln!("[openysm] commands: off, reload, list, cloud off, cloud on, model <geometry path>, pack <directory or zip>, texture <name>");false}
            }
        }
    })
}
fn load() -> Option<Loaded> {
    if let Some(path) = std::env::var_os("OPENYSM_PACK") {
        // The engine reads the pack; no other OPENYSM_* file variable applies.
        let path = std::path::PathBuf::from(path);
        let texture = std::env::var("OPENYSM_PACK_TEXTURE").ok().filter(|name|!name.is_empty());
        let clip = std::env::var("OPENYSM_CLIP").ok().filter(|name|!name.is_empty());
        let built = openysm_runtime::bundle::ModelBundle::open(&path,Default::default())
            .and_then(|bundle|build_pack(bundle,texture.as_deref(),clip.as_deref()));
        return match built {
            Ok((loaded,arms,pixels,bundle)) => {
                ARMS.with(|slot| *slot.borrow_mut() = arms);
                PACK.with(|state| {
                    let mut state = state.borrow_mut();
                    state.path = Some(path);
                    state.wanted_texture = texture;
                    state.clip = clip;
                    state.texture = pixels;
                    state.bundle = Some(bundle);
                    state.dirty = true;
                });
                Some(loaded)
            },
            Err(error) => {eprintln!("[openysm] pack load failed: {error}");None}
        };
    }
    let path = std::env::var_os("OPENYSM_MODEL")?;
    let result = (|| -> Result<Loaded,String> {
        let bytes = std::fs::read(path).map_err(|e|e.to_string())?;
        let model = openysm_runtime::load_geometry(&bytes)?;
        // OPENYSM_CLIP pins one clip; without it the file's states are chosen
        // from the player's movement.
        let (mut clip,mut clips) = (None,None);
        if let Some(path) = std::env::var_os("OPENYSM_ANIMATION") {
            if let Ok(name) = std::env::var("OPENYSM_CLIP") {
                let bytes = std::fs::read(path).map_err(|e|e.to_string())?;
                clip = Some(openysm_runtime::Clip::load(&bytes,&name)?);
            } else {
                clips = load_clips(std::path::Path::new(&path));
            }
        }
        Ok(Loaded { model, clip, clips, playing: Default::default(), animators: Default::default(), motion: None, display_state: None, texture: std::env::var("OPENYSM_TEXTURE")
            .unwrap_or_else(|_| if std::env::var_os("OPENYSM_TEXTURE_FILE").is_some() { "openysm/model".into() } else { "entity/player/wide/steve".into() }) })
    })();
    match result {
        Ok(model) => Some(model),
        Err(error) => {eprintln!("[openysm] load failed: {error}");None}
    }
}
pub fn render(m: &crate::mobs::Mob, alpha: f32, base: glam::Mat4,
              atlas: crate::mobs::render::Atlas<'_>, light: u32,
              out: &mut Vec<crate::mesher::Vertex>) -> bool {
    if std::env::var_os("SPM_BRIDGE_DIR").is_some() {
        return CLOUD.with(|cloud| {
            let mut cloud = cloud.borrow_mut();
            let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_secs_f64();
            if now >= cloud.expires {cloud.models.clear();return false;}
            let Some((_,loaded)) = cloud.models.get_mut(&m.id) else {return false;};
            draw_loaded(loaded,m,alpha,base,atlas,light,out)
        });
    }
    render_local(m,alpha,base,atlas,light,out)
}
fn render_local(m: &crate::mobs::Mob, alpha: f32, base: glam::Mat4,
              atlas: crate::mobs::render::Atlas<'_>, light: u32,
              out: &mut Vec<crate::mesher::Vertex>) -> bool {
    MODEL.with(|slot| {
        let mut slot = slot.borrow_mut();
        let Some(loaded) = slot.as_mut() else {return false;};
        draw_loaded(loaded,m,alpha,base,atlas,light,out)
    })
}
fn draw_loaded(loaded: &mut Loaded, m: &crate::mobs::Mob, alpha: f32, base: glam::Mat4,
               atlas: crate::mobs::render::Atlas<'_>, light: u32, out: &mut Vec<crate::mesher::Vertex>) -> bool {
        let Some(uv) = atlas(&loaded.texture) else {return false;};
        let age = (m.age as f32+alpha)/20.;
        if loaded.animators.len() >= 256 && !loaded.animators.contains_key(&m.id) {
            return false;
        }
        let instance = loaded.animators.entry(m.id).or_default();
        if age < instance.0 {
            instance.1.reset();
            loaded.playing.remove(&m.id);
        }
        instance.0 = age;
        let chosen = if let Some(clip) = &loaded.clip {
            let motion_time = loaded.motion.as_ref()
                .and_then(|motion| motion["started_at_unix_ms"].as_i64())
                .map(|started| ((std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_millis() as i64 - started).max(0) as f32) / 1000.)
                .unwrap_or(age);
            Some((clip,motion_time,clip.looping))
        } else if let Some(set) = &loaded.clips {
            let flying = loaded.display_state.as_ref()
                .and_then(|display| display["state"]["flying"].as_bool()).unwrap_or(false);
            set.select(&player_state(m,alpha,flying)).map(|(name,once,clip)| {
                // A state restarts its clip when entered; there is no blend between states.
                if loaded.playing.len() >= 64 && !loaded.playing.contains_key(&m.id) {loaded.playing.clear();}
                let entry = loaded.playing.entry(m.id).or_insert((name,age));
                if entry.0 != name || age < entry.1 {*entry = (name,age);}
                (clip,age-entry.1,!once)
            })
        } else {None};
        {
            let speed = ((m.position.x-m.previous.x).powi(2)+(m.position.z-m.previous.z).powi(2)).sqrt() as f32 * 20.;
            // The local model mob is not moved by the server; its speed is published each tick.
            let vertical_speed = LOCAL.with(|local| local.get()).filter(|local| local.id == m.id)
                .map_or(((m.position.y-m.previous.y)*20.) as f32, |local| local.vertical_speed);
            // Named physics values integrate once per rendered frame, before sampling.
            instance.1.advance(m.age as f32+alpha);
            let query = |name: &str| {
                let motion_value = loaded.motion.as_ref().and_then(|motion| {
                    let key = name.strip_prefix("variable.")?;
                    motion["roaming"].get(name).or_else(|| motion["roaming"].get(key))
                        .and_then(|value| value.as_f64()).map(|value| value as f32)
                        .filter(|value| value.is_finite())
                });
                let display_value = loaded.display_state.as_ref().and_then(|display| {
                    let field = match name {
                        "health" => "health",
                        "max_health" => "max_health",
                        "experience_level" => "experience_level",
                        "food_level" => "food_level",
                        "is_flying" => "flying",
                        "is_blocking" => "shield_blocking",
                        "strafe_input" => "strafe_input",
                        "vertical_input" => "vertical_input",
                        "forward_input" => "forward_input",
                        _ => return None,
                    };
                    let value = &display["state"][field];
                    value.as_bool().map(|v| f32::from(v)).or_else(||
                        value.as_f64().map(|v| v as f32).filter(|v| v.is_finite()))
                });
                motion_value.or(display_value).unwrap_or(match name {
                    "life_time" => age,
                    "ground_speed" => speed,
                    "vertical_speed" => vertical_speed,
                    // OpenYSM's remote-entity definition; its local player uses a per-frame head delta.
                    "yaw_speed" => 20.*(m.yaw-m.previous_yaw),
                    "is_moving" => f32::from(speed > 0.01),
                    "is_alive" => f32::from(m.death_time == 0),
                    "ysm.head_yaw" | "ysm.head_pitch" => {
                        let (yaw,pitch) = openysm_runtime::head_look(
                            (m.previous_body_yaw,m.body_yaw),(m.previous_head_yaw,m.head_yaw),
                            (m.previous_head_pitch,m.head_pitch),alpha);
                        if name == "ysm.head_yaw" {yaw} else {pitch}
                    },
                    // A variable nothing assigned is null: `v.x ?? fallback` takes the fallback.
                    _ if name.starts_with("variable.") => f32::NAN,
                    _ => 0.,
                })
            };
            // OpenYSM's controller order: the pre_parallel slots, the main state, the parallel slots.
            // A pinned clip plays alone. Transitions between states are not blended.
            let parallel = if loaded.clip.is_none() {loaded.clips.as_ref()} else {None};
            let animator = &mut instance.1;
            animator.begin();
            for clip in parallel.into_iter().flat_map(|set| set.parallel(true)) {
                animator.layer(clip,&loaded.model,age,true,false,&query);
            }
            if let Some((clip,time,looping)) = chosen {
                animator.layer(clip,&loaded.model,time,looping,false,&query);
            }
            for clip in parallel.into_iter().flat_map(|set| set.parallel(false)) {
                animator.layer(clip,&loaded.model,age,true,true,&query);
            }
            animator.finish(&mut loaded.model);
        }
        // Horizon's model origin is at the head and its Y axis points down;
        // Bedrock geometry uses feet origin and Y up.
        let root = base * glam::Mat4::from_translation(glam::Vec3::new(0.,1.5,0.))
            * glam::Mat4::from_scale(glam::Vec3::new(1.,-1.,1.));
        for q in loaded.model.render(&root.to_cols_array(),light,0).chunks_exact(4) {
            if q[0].hidden != 0 {continue;}
            // Reflecting Y changes winding. Keep the quad winding used by Horizon.
            for i in [3,2,1,0] {
                let v = q[i];
                out.push(crate::mesher::Vertex {
                    pos:v.position, uv:[uv[0]+(uv[2]-uv[0])*v.uv[0],uv[1]+(uv[3]-uv[1])*v.uv[1]],
                    layer:crate::mobs::render::body_overlay(m,alpha),
                    color:crate::mobs::render::body_color(m,alpha),
                    light:(v.light as u32 & 255)|((v.light as u32 >> 16 & 255)<<8),
                    normal:[(v.normal[0]*127.) as i8,(v.normal[1]*127.) as i8,(v.normal[2]*127.) as i8,0],
                    glint_uv:[0.;2],
                });
            }
        }
        true
}

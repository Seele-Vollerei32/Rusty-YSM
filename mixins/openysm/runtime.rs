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
                Ok((Loaded{model,clip,clips,playing:Default::default(),
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
pub fn command(command: &str) -> bool {
    MODEL.with(|slot| {
        let mut slot = slot.borrow_mut();
        match command {
            "off" => { *slot = None; true },
            "reload" => {
                if let Some(loaded) = load() { *slot = Some(loaded); true } else { false }
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
                            let texture = slot.as_ref().map(|m|m.texture.clone()).unwrap_or_else(||
                                if std::env::var_os("OPENYSM_TEXTURE_FILE").is_some() {"openysm/model".into()} else {"entity/player/wide/steve".into()});
                            *slot = Some(Loaded {model,clip:None,clips:None,playing:Default::default(),motion:None,display_state:None,texture}); true
                        },
                        Err(error) => {eprintln!("[openysm] model change failed: {error}");false}
                    }
                } else {eprintln!("[openysm] commands: off, reload, list, cloud off, cloud on, model <geometry path>");false}
            }
        }
    })
}
fn load() -> Option<Loaded> {
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
        Ok(Loaded { model, clip, clips, playing: Default::default(), motion: None, display_state: None, texture: std::env::var("OPENYSM_TEXTURE")
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
        if let Some((clip,time,looping)) = chosen {
            let speed = ((m.position.x-m.previous.x).powi(2)+(m.position.z-m.previous.z).powi(2)).sqrt() as f32 * 20.;
            clip.apply_as(&mut loaded.model,time,looping,&|name| {
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
                    "is_moving" => f32::from(speed > 0.01),
                    "is_alive" => f32::from(m.death_time == 0),
                    "ysm.head_yaw" | "ysm.head_pitch" => {
                        let (yaw,pitch) = openysm_runtime::head_look(
                            (m.previous_body_yaw,m.body_yaw),(m.previous_head_yaw,m.head_yaw),
                            (m.previous_head_pitch,m.head_pitch),alpha);
                        if name == "ysm.head_yaw" {yaw} else {pitch}
                    },
                    _ => 0.,
                })
            });
        } else {
            loaded.model.pose(|bone|(bone.rotation,[0.;3],[1.;3],false));
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

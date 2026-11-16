use std::cell::RefCell;
#[derive(Default)]
struct CloudState {
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
            if state.models.get(id).is_some_and(|(old,_)|old==key) {continue;}
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
                let texture=format!("openysm/cloud{slot}");
                let original=*atlas.sprites.get(&texture).ok_or("cloud atlas slot missing")?;
                let mut rect=original;
                rect.u1=rect.u0+w as f32/atlas.width as f32;
                rect.v1=rect.v0+h as f32/atlas.height as f32;
                rect.w=w;rect.h=h;
                let patch=crate::font::Patch{x:(rect.u0*atlas.width as f32).round() as u32,
                    y:(rect.v0*atlas.height as f32).round() as u32,width:w,height:h,rgba:pixels};
                atlas.sprites.insert(texture.clone(),rect);
                Ok((Loaded{model,clip,motion:row["motion"].as_object().map(|_|row["motion"].clone()),texture},patch))
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
    clip: Option<openysm_runtime::Clip>,
    motion: Option<serde_json::Value>,
    texture: String,
}
thread_local! {
    static CLOUD: RefCell<CloudState> = RefCell::new(CloudState::default());
    static MODEL: RefCell<Option<Loaded>> = RefCell::new(load());
    static ARMS: RefCell<[Option<openysm_runtime::Model>;2]> = RefCell::new([
        load_arm("OPENYSM_RIGHT_ARM"), load_arm("OPENYSM_LEFT_ARM")
    ]);
}
fn load_arm(key: &str) -> Option<openysm_runtime::Model> {
    let path = std::env::var_os(key)?;
    match std::fs::read(path).map_err(|e|e.to_string()).and_then(|bytes|openysm_runtime::load_geometry(&bytes)) {
        Ok(model) => Some(model),
        Err(error) => {eprintln!("[openysm] {key}: {error}");None}
    }
}
// Arm geometry must be authored around the hand origin, in Bedrock pixels.
pub fn render_arm(left: bool, matrix: glam::Mat4, atlas: crate::mobs::render::Atlas<'_>,
                  light: u32, out: &mut Vec<crate::mesher::Vertex>) -> bool {
    MODEL.with(|body| {
        let body = body.borrow();
        let Some(body) = body.as_ref() else {return false;};
        let Some(uv) = atlas(&body.texture) else {return false;};
        ARMS.with(|arms| {
            let mut arms = arms.borrow_mut();
            let Some(model) = arms[usize::from(left)].as_mut() else {return false;};
            model.pose(|bone|(bone.rotation,[0.;3],[1.;3],false));
            let root = matrix * glam::Mat4::from_scale(glam::Vec3::new(1.,-1.,1.));
            for quad in model.render(&root.to_cols_array(),light,0).chunks_exact(4) {
                if quad[0].hidden != 0 {continue;}
                for i in [3,2,1,0] {
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
            "cloud off" => {
                CLOUD.with(|cloud| cloud.borrow_mut().models.clear());
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
                            *slot = Some(Loaded {model,clip:None,motion:None,texture}); true
                        },
                        Err(error) => {eprintln!("[openysm] model change failed: {error}");false}
                    }
                } else {eprintln!("[openysm] commands: off, reload, list, cloud off, model <geometry path>");false}
            }
        }
    })
}
fn load() -> Option<Loaded> {
    let path = std::env::var_os("OPENYSM_MODEL")?;
    let result = (|| -> Result<Loaded,String> {
        let bytes = std::fs::read(path).map_err(|e|e.to_string())?;
        let model = openysm_runtime::load_geometry(&bytes)?;
        let clip = if let Some(path) = std::env::var_os("OPENYSM_ANIMATION") {
            let bytes = std::fs::read(path).map_err(|e|e.to_string())?;
            let name = std::env::var("OPENYSM_CLIP").map_err(|_|"OPENYSM_CLIP is required")?;
            Some(openysm_runtime::Clip::load(&bytes,&name)?)
        } else {None};
        Ok(Loaded { model, clip, motion: None, texture: std::env::var("OPENYSM_TEXTURE")
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
        if let Some(clip) = &loaded.clip {
            let age = (m.age as f32+alpha)/20.;
            let motion_time = loaded.motion.as_ref()
                .and_then(|motion| motion["started_at_unix_ms"].as_i64())
                .map(|started| ((std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_millis() as i64 - started).max(0) as f32) / 1000.)
                .unwrap_or(age);
            let speed = ((m.position.x-m.previous.x).powi(2)+(m.position.z-m.previous.z).powi(2)).sqrt() as f32 * 20.;
            clip.apply_with_queries(&mut loaded.model,motion_time,&|name| {
                let motion_value = loaded.motion.as_ref().and_then(|motion| {
                    let key = name.strip_prefix("variable.").unwrap_or(name);
                    motion["roaming"].get(key).and_then(|value| value.as_f64()).map(|value| value as f32)
                });
                motion_value.unwrap_or(match name {
                    "life_time" => age,
                    "ground_speed" => speed,
                    "is_moving" => f32::from(speed > 0.01),
                    "is_alive" => f32::from(m.death_time == 0),
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

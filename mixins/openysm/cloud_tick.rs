fn cloud_tick(game: &mut crate::game::Game<'_>) {
    // The server thread draws nothing and owns none of this mod's state.
    if game.serving.is_some() {
        return;
    }
    let mut players = std::collections::BTreeMap::new();
    if let Some(remote) = game.remote.as_ref() {
        let mut ordered: Vec<_> = remote.level_players.iter().collect();
        ordered.sort_by_key(|(id,_)|**id);
        for (id, uuid) in ordered {
            players.insert(*id as u32, format!("{:08x}-{:04x}-{:04x}-{:04x}-{:012x}",uuid>>96,(uuid>>80)&65535,(uuid>>64)&65535,(uuid>>48)&65535,uuid&0xffffffffffff));
        }
        if let Some(model) = game.player_model.as_ref() {
            let uuid = remote.profile;
            players.insert(model.id, format!("{:08x}-{:04x}-{:04x}-{:04x}-{:012x}",uuid>>96,(uuid>>80)&65535,(uuid>>64)&65535,(uuid>>48)&65535,uuid&0xffffffffffff));
        }
    } else if let Some(model) = game.player_model.as_ref() {
        // No session: the local model still owns its animation state.
        players.insert(model.id, String::new());
    }
    horizon_client::mobs::render::openysm::sync_players(&players);
    let Some(directory) = std::env::var_os("SPM_BRIDGE_DIR") else {return;};
    let directory = std::path::PathBuf::from(directory);
    // Legacy Python bridge still has sixteen texture slots.
    let local = game.player_model.as_ref().and_then(|model| players.get(&model.id).map(|uuid|(model.id,uuid.clone())));
    let mut players: std::collections::BTreeMap<_,_> = players.into_iter().take(15).collect();
    if let Some((id,uuid)) = local { players.insert(id,uuid); }
    let patches = horizon_client::mobs::render::openysm::cloud_update(&directory,players,&mut game.gui.atlas);
    game.map_patches.extend(patches);
}

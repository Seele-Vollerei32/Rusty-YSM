fn cloud_tick(game: &mut crate::game::Game<'_>) {
    let Some(directory) = std::env::var_os("SPM_BRIDGE_DIR") else {return;};
    let directory = std::path::PathBuf::from(directory);
    let mut players = std::collections::BTreeMap::new();
    if let Some(remote) = game.remote.as_ref() {
        let mut ordered: Vec<_> = remote.level_players.iter().collect();
        ordered.sort_by_key(|(id,_)|**id);
        for (id, uuid) in ordered {
            if players.len() >= 15 {break;}
            players.insert(*id as u32, format!("{:08x}-{:04x}-{:04x}-{:04x}-{:012x}",uuid>>96,(uuid>>80)&65535,(uuid>>64)&65535,(uuid>>48)&65535,uuid&0xffffffffffff));
        }
        if let Some(model) = game.player_model.as_ref() {
            let uuid = remote.profile;
            players.insert(model.id, format!("{:08x}-{:04x}-{:04x}-{:04x}-{:012x}",uuid>>96,(uuid>>80)&65535,(uuid>>64)&65535,(uuid>>48)&65535,uuid&0xffffffffffff));
        }
    }
    let patches = horizon_client::mobs::render::openysm::cloud_update(&directory,players,&mut game.gui.atlas);
    game.map_patches.extend(patches);
}

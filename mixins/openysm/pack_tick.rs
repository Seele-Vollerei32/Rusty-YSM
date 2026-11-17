// Once per client tick: install a pack the resource worker finished and write
// the selected texture into its atlas slot.
fn pack_tick(game: &mut crate::game::Game<'_>) {
    // The server thread draws nothing and owns none of this mod's state.
    if game.serving.is_some() {
        return;
    }
    let patches = horizon_client::mobs::render::openysm::pack_update(&mut game.gui.atlas);
    game.map_patches.extend(patches);
}

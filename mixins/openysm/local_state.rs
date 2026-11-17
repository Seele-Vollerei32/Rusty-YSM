// What the main animation states need from the local player. Its model mob is
// only given the pose and equipment, so the rest is published here each tick.
fn local_state(game: &crate::game::Game<'_>) {
    use horizon_client::pose::Pose;
    let state = game.player.as_ref().map(|p| horizon_client::mobs::render::openysm::LocalPlayer {
        id: game.player_model.as_ref().map_or(0, |m| m.id),
        on_ground: p.ground.on_ground,
        sprinting: p.sprinting,
        flying: p.flying,
        swimming: p.swimming,
        swim_pose: p.pose == Pose::Swimming,
        sleeping: p.sleeping || p.pose == Pose::Sleeping,
        in_water: p.fluids.was_touching_water,
        dead: p.health.dead_or_dying(),
        // Entity.yo is the position before this tick's move.
        vertical_speed: ((p.position.y - game.previous_position.y) * 20.) as f32,
    });
    horizon_client::mobs::render::openysm::local_player(state);
}

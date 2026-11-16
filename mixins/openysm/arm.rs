fn openysm_arm(game: &crate::game::Game<'_>, left: bool, matrix: glam::Mat4, light: u32,
               out: &mut Vec<horizon_client::mesher::Vertex>) -> std::ops::ControlFlow<()> {
    let atlas = &game.gui.atlas;
    let lookup = |name: &str| atlas.sprites.get(name).map(|r|[r.u0,r.v0,r.u1,r.v1]);
    if horizon_client::mobs::render::openysm::render_arm(left,matrix,&lookup,light,out) {
        std::ops::ControlFlow::Break(())
    } else { std::ops::ControlFlow::Continue(()) }
}

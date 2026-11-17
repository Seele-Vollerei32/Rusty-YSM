// Only a game that ticks its own world reaches this; with a server thread the
// window handles `/openysm` in chat.rs, where the models live.
fn openysm_command(command: &str, game: &crate::game::Game<'_>) -> std::ops::ControlFlow<bool> {
    if game.serving.is_some() {
        return std::ops::ControlFlow::Continue(());
    }
    if let Some(command) = command.strip_prefix("openysm ") {
        let ok = horizon_client::mobs::render::openysm::command(command.trim());
        return std::ops::ControlFlow::Break(ok);
    }
    std::ops::ControlFlow::Continue(())
}

// The window's game sends commands to its server thread, where this mod's state
// does not live: `/openysm ...` is handled here, on the thread that renders.
fn openysm_chat(message: &str) -> std::ops::ControlFlow<()> {
    if let Some(command) = message.trim().strip_prefix("/openysm ") {
        horizon_client::mobs::render::openysm::command(command.trim());
        return std::ops::ControlFlow::Break(());
    }
    std::ops::ControlFlow::Continue(())
}

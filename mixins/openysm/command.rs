fn openysm_command(command: &str) -> std::ops::ControlFlow<bool> {
    if let Some(command) = command.strip_prefix("openysm ") {
        let ok = horizon_client::mobs::render::openysm::command(command.trim());
        return std::ops::ControlFlow::Break(ok);
    }
    std::ops::ControlFlow::Continue(())
}

fn render(m: &crate::mobs::Mob, alpha: f32, base: glam::Mat4,
          atlas: crate::mobs::render::Atlas<'_>, light: u32, data: &crate::mobs::MobData,
          extra: &[(&crate::mobs::Part, [f32; 4], u32, u32)], body_visible: bool,
          out: &mut Vec<crate::mesher::Vertex>) -> std::ops::ControlFlow<()> {
    if body_visible && &*m.kind.id == "minecraft:player"
        && crate::mobs::render::openysm::render(m, alpha, base, atlas, light, out) {
        // Body=false prevents reentry, while retaining Horizon armour and entity layers.
        crate::mobs::render::render_visible(m, alpha, base, atlas, light, data, extra, false, out);
        return std::ops::ControlFlow::Break(());
    }
    std::ops::ControlFlow::Continue(())
}

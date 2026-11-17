use openysm_runtime::bundle::ModelBundle;
use openysm_runtime::package::{ModelPackage, PackageLimits};
use openysm_runtime::{Animator, PlayerState};
/// Poses the main model for 60 frames the way the runtime layers clips:
/// pre_parallel slots, the standing state, parallel slots. Returns the layer count.
fn pose(bundle: &ModelBundle) -> Result<usize, String> {
    let mut model = bundle.model()?;
    let Some(set) = bundle.animations.get("main") else {
        return Ok(0);
    };
    let mut animator = Animator::default();
    let query = |name: &str| if name.starts_with("variable.") { f32::NAN } else { 0. };
    let standing = PlayerState { on_ground: true, ..Default::default() };
    let mut layers = 0;
    for frame in 0..60 {
        let ticks = 1. + frame as f32 * 0.37;
        let time = ticks / 20.;
        animator.advance(ticks);
        animator.begin();
        layers = 0;
        for clip in set.parallel(true) {
            animator.layer(clip, &model, time, true, false, &query);
            layers += 1;
        }
        if let Some((_, once, clip)) = set.select(&standing) {
            animator.layer(clip, &model, time, !once, false, &query);
            layers += 1;
        }
        for clip in set.parallel(false) {
            animator.layer(clip, &model, time, true, true, &query);
            layers += 1;
        }
        animator.finish(&mut model);
        let vertices = model.render(&openysm_runtime::IDENTITY, 0xf0f0, 0);
        let finite = vertices.iter().all(|v| v.position.iter().chain(v.normal.iter()).all(|x| x.is_finite()));
        if vertices.is_empty() || !finite {
            return Err(format!("frame {frame} has no finite geometry"));
        }
    }
    Ok(layers)
}
fn main() {
    let mut failed = false;
    for path in std::env::args().skip(1) {
        let result = ModelPackage::open(&path, PackageLimits::default()).and_then(|package| {
            let entries = package.entries().len();
            let references = package.references().len();
            let bundle = ModelBundle::new(package)?;
            let layers = pose(&bundle)?;
            for line in &bundle.diagnostics {
                println!("  rejected {line}");
            }
            println!(
                "{path}: {entries} entries, {references} references, {} animation roles, {} textures, {} rejected clips, {} malformed expressions read as 0, {layers} layers posed",
                bundle.animations.len(),
                bundle.textures().len(),
                bundle.diagnostics.len(),
                bundle.animations.values().map(|set| set.ignored()).sum::<usize>()
            );
            Ok(())
        });
        if let Err(error) = result {
            eprintln!("{path}: {error}");
            failed = true;
        }
    }
    if failed {
        std::process::exit(1);
    }
}

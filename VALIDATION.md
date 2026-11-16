# Validation

Validated on Windows x64 with Rust 1.95.0, Python 3.13 and MSVC C++17.

- `cargo fmt --all --check`: passed.
- `cargo clippy --workspace --all-targets --locked --offline -- -D warnings`: passed.
- `cargo test --workspace --locked --offline`: 9 tests passed, including C++ skinning/hidden inheritance, ABI layouts, numeric animation, pre/post discontinuity, Catmull-Rom, inferred duration, bad key times, Molang precedence/degree math/conditional validation.
- `cargo run --example validate_model -- ../OpenYSM/common/src/main/resources/assets/yes_steve_model/builtin/default/models/main.json`: 64 bones, 4128 finite vertices/normals.
- Real pinned Horizon `horizon-mixins plan/apply`: passed; one bundle crate, one support module, five HEAD injections (body, atlas texture, local commands, first-person arms, Cloud player updates).
- Complete staged `horizon-client` binary `cargo check --locked --offline --no-default-features`: passed.
- Final complete staged `horizon-client` binary `cargo build --locked --offline --no-default-features --profile fast`: passed, including the texture/command/arm hooks. Latest Cloud-enabled build passed in 2m29s; log: `target/cloud-build.log`. Built client's `--help` also passed.
- Built Windows client's `--help`: exit 0.
- `python tools/test_package.py`: passed, source-only bundle reproducible and includes C++ sources/header/license/sse2neon.
- `python tools/test_launch.py`: model-pack default/explicit texture selection, missing texture and resource path rejection passed.
- `python tools/test_texture.py`: compiled the real texture helper with a mocked PNG decoder; registration and zero/oversized dimension rejection passed. This is not a real image decoding or GPU test.
- `tools/launch.py --dry-run` with the actual default OpenYSM pack resolved its model and texture correctly.
- Upstream Horizon and horizon-mixins working trees were not edited.

Build artifact: `target/horizon-cargo/fast/horizon-client.exe`.
Source bundle: `target/openysm-0.1.0.hnmod`.
Logs: `target/horizon-check.log`, `target/horizon-build.log`, `target/stage.log`, `target/client-help.log`.

SPM extension checks: six `test_spm_cloud.py` tests passed, including real HTTP against a loopback protocol fixture (not a real SPM service); one `test_cloud_watch.py` test passed for independent player models, repeated authorization and removal. The Cloud player hook passed complete Horizon `cargo check` (`target/cloud-check-final.log`).

These checks establish native compilation, link compatibility and the covered geometry/animation calculations. They do not establish rendered visual parity, FPS improvement, custom texture loading, server compatibility or full OpenYSM behavior. No interactive game session with prepared resources was run. See README for the missing functionality.

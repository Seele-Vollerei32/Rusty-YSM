# Third-party notices

This project is adapted from horizon-native-mod-template (MIT); see LICENSE.

`mixins/openysm/engine/native/renderer.cpp` derives its matrix, bone evaluation and GPU mesh baking code from `openysm.cpp/dllmain.cpp`, Copyright (c) 2026 OpenYSM, MIT. The full upstream license is in `mixins/openysm/engine/native/LICENSE` and is included in source bundles.

`mixins/openysm/engine/src/ysm.rs` is a Rust port of the `.ysm` file decryption in Sparkle-Morpher (`YsmCrypt`, `XChaCha20`, `ChaCha20Base`, `CityHash`, `MT19937`, `YsmZstd`), Copyright (c) 2026 Marciken, MIT. The files under `mixins/openysm/engine/tests/fixtures/ysm/` were produced by running those classes on generated payloads; they contain no model.

`mixins/openysm/engine/native/sse2neon.h` retains its upstream copyright and MIT license in the file header.

OpenYSM Java source in the neighboring workspace directory is used as the behavioral reference for Bedrock geometry, animation data and renderer interfaces. No Java jars, JNI binaries, Horizon sources, Minecraft assets or OpenYSM model/texture resources are distributed in this bundle. OpenYSM model resources have separate licenses; consult the original OpenYSM repository before redistributing any model.

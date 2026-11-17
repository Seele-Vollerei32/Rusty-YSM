"""Verify source bundle reproducibility and that the C++ sources survive packaging."""
import hashlib
import zipfile
from package import package

first = package()
a = hashlib.sha256(first.read_bytes()).digest()
second = package()
assert hashlib.sha256(second.read_bytes()).digest() == a
with zipfile.ZipFile(second) as archive:
    names = set(archive.namelist())
    for path in ['horizon-native.json', 'mixins/openysm/horizon-mixin.toml',
                 'mixins/openysm/engine/build.rs', 'mixins/openysm/engine/native/renderer.cpp',
                 'mixins/openysm/engine/native/renderer.h', 'mixins/openysm/engine/native/LICENSE',
                 'mixins/openysm/engine/native/sse2neon.h']:
        assert path in names, path
    assert not any(name.endswith(('.dll', '.exe', '.so')) for name in names)
print('reproducible source bundle including C++: OK')

# The native bundle intentionally omits companion tools; the distribution must
# be independently usable for Cloud launch and source builds.
first_dist = package(distribution=True)
digest = hashlib.sha256(first_dist.read_bytes()).digest()
assert hashlib.sha256(package(distribution=True).read_bytes()).digest() == digest
with zipfile.ZipFile(first_dist) as archive:
    names = set(archive.namelist())
    for path in ['tools/cloud_play.py', 'tools/cloud_watch.py', 'tools/spm_cloud.py',
                 'tools/display_state.py', 'tools/rgba.py', 'tools/launch.py',
                 'tools/horizon.py', 'Cargo.toml', 'Cargo.lock', 'compatibility.json',
                 'LICENSE', 'NOTICE.md', 'SPM_CLOUD.md']:
        assert path in names, path
    assert not any(part in ('target', '__pycache__', '.git')
                   for name in names for part in name.split('/'))
    for name in names:
        if name.endswith('.py'):
            compile(archive.read(name), name, 'exec')
print('reproducible distribution with Cloud tools and licenses: OK')

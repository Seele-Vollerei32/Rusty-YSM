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

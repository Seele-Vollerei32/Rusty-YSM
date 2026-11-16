"""Create a reproducible source bundle, including the C++ bundle crate."""
import hashlib
import json
from pathlib import Path
import tomllib
import zipfile

ROOT = Path(__file__).resolve().parents[1]


def package():
    compat = json.loads((ROOT / 'compatibility.json').read_text())
    bundle = ROOT / 'mixins/openysm'
    mod = tomllib.loads((bundle / 'horizon-mixin.toml').read_text())['mod']
    metadata = dict(schema=1, id=mod['id'], version=mod['version'], game='26.3',
                    horizon=compat['horizon_revision'], mixins=compat['horizon_mixins_revision'])
    files = {'horizon-native.json': (json.dumps(metadata, sort_keys=True, indent=2) + '\n').encode()}
    for path in sorted(bundle.rglob('*')):
        if path.is_file() and not any(p in ('target', '__pycache__', '.git') for p in path.relative_to(bundle).parts):
            files[path.relative_to(ROOT).as_posix()] = path.read_bytes()
    output = ROOT / 'target' / f"{mod['id']}-{mod['version']}.hnmod"
    output.parent.mkdir(exist_ok=True)
    with zipfile.ZipFile(output, 'w', compression=zipfile.ZIP_DEFLATED) as archive:
        for name, content in sorted(files.items()):
            info = zipfile.ZipInfo(name, (2026, 1, 1, 0, 0, 0))
            info.compress_type = zipfile.ZIP_DEFLATED
            info.external_attr = 0o100644 << 16
            archive.writestr(info, content)
    print(json.dumps(dict(file=str(output), sha256=hashlib.sha256(output.read_bytes()).hexdigest()), indent=2))
    return output


if __name__ == '__main__':
    package()

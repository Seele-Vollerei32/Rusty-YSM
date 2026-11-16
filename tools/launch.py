"""Select an unpacked OpenYSM model pack and launch the native client."""
import argparse
import json
import os
from pathlib import Path
import subprocess

ROOT = Path(__file__).resolve().parents[1]


def pack_environment(pack, texture=None, clip=None):
    pack = Path(pack).resolve()
    doc = json.loads((pack / 'ysm.json').read_text(encoding='utf-8-sig'))
    player = doc['files']['player']

    def resource(name):
        path = (pack / name).resolve()
        if not path.is_relative_to(pack) or not path.is_file():
            raise ValueError(f'invalid model resource: {name}')
        return str(path)

    textures = player['texture']
    if not isinstance(textures, list) or not textures:
        raise ValueError('expected nonempty player.texture array')
    desired = texture or doc.get('properties', {}).get('default_texture')
    selected = next((name for name in textures if Path(name).stem == desired), None)
    if texture and selected is None:
        raise ValueError(f'unknown texture {texture}; choices: {[Path(n).stem for n in textures]}')
    env = {'OPENYSM_MODEL': resource(player['model']['main']),
           'OPENYSM_TEXTURE_FILE': resource(selected or textures[0])}
    if clip:
        animations = player.get('animation', {})
        matches = []
        for name in animations.values():
            path = resource(name)
            content = json.loads(Path(path).read_text(encoding='utf-8-sig'))
            if clip in content.get('animations', {}):
                matches.append(path)
        if len(matches) != 1:
            raise ValueError(f'clip {clip} must appear in exactly one animation file, found {len(matches)}')
        env.update(OPENYSM_ANIMATION=matches[0], OPENYSM_CLIP=clip)
    return env


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--pack', type=Path, required=True)
    parser.add_argument('--texture')
    parser.add_argument('--clip')
    parser.add_argument('--resources', type=Path, required=True)
    parser.add_argument('--client', type=Path, default=ROOT / 'target/horizon-cargo/fast' /
                        ('horizon-client.exe' if os.name == 'nt' else 'horizon-client'))
    parser.add_argument('--dry-run', action='store_true')
    args, extra = parser.parse_known_args()
    config = pack_environment(args.pack, args.texture, args.clip)
    env = {key: value for key, value in os.environ.items() if not key.startswith('OPENYSM_')}
    env.update(config)
    command = [str(args.client.resolve()), '--resources', str(args.resources.resolve()), *extra]
    if args.dry_run:
        print(json.dumps({'environment': config, 'command': command}, ensure_ascii=False, indent=2))
        return 0
    return subprocess.run(command, env=env).returncode


if __name__ == '__main__':
    raise SystemExit(main())

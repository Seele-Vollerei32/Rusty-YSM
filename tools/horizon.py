"""Stage OpenYSM through the pinned horizon-mixins CLI; never edit upstream sources."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import shutil
import subprocess
import tarfile
import tempfile

ROOT = Path(__file__).resolve().parents[1]
COMPAT = json.loads((ROOT / 'compatibility.json').read_text())


def run(command, **kwargs):
    command = [str(x) for x in command]
    if command[0] == 'cargo':
        command.insert(1, '+1.95.0')
    return subprocess.run(command, check=True, **kwargs)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('action', choices=['stage', 'check', 'test', 'build'])
    parser.add_argument('--horizon', type=Path, default=ROOT.parent / 'horizon')
    parser.add_argument('--mixins', type=Path, default=ROOT.parent / 'horizon-mixins')
    args = parser.parse_args()
    horizon = args.horizon.resolve()
    mixins = args.mixins.resolve()
    target = ROOT / 'target'
    original = target / 'original'
    staged = target / 'modded'
    pin = COMPAT['horizon_revision']
    actual = subprocess.check_output(['git', '-C', str(mixins), 'rev-parse', 'HEAD'], text=True).strip()
    if actual != COMPAT['horizon_mixins_revision']:
        raise SystemExit('horizon-mixins revision mismatch')
    run(['git', '-C', mixins, 'diff', '--quiet', 'HEAD', '--', 'Cargo.toml', 'Cargo.lock', 'crates'])
    target.mkdir(exist_ok=True)
    # Re-export each time; source hashes verify the hook contract before applying anything.
    with tempfile.TemporaryDirectory(dir=target) as temp:
        archive = Path(temp) / 'source.tar'
        run(['git', '-C', horizon, 'archive', '--format=tar', '-o', archive, pin])
        export = Path(temp) / 'source'
        export.mkdir()
        with tarfile.open(archive) as stream:
            stream.extractall(export, filter='data')
        for path, expected in COMPAT['source_sha256'].items():
            if hashlib.sha256((export / path).read_bytes()).hexdigest() != expected:
                raise SystemExit(f'source hash mismatch: {path}')
        for path in export.rglob('*'):
            if path.is_symlink():
                data = path.read_bytes()
                path.unlink()
                path.write_bytes(data)
        (export / '.horizon-revision').write_text(pin + '\n')
        if original.exists():
            shutil.rmtree(original)
        shutil.move(export, original)
    run(['cargo', 'build', '--locked', '--manifest-path', mixins / 'Cargo.toml', '-p', 'horizon-mixins-cli'])
    cli = mixins / 'target/debug' / ('horizon-mixins.exe' if os.name == 'nt' else 'horizon-mixins')
    base = ['--workspace', original, '--mods', ROOT / 'mixins']
    run([cli, 'plan', *base, '--out', target / 'plan.json'])
    # Mixins can atomically replace stages only on Linux. Other hosts use immutable
    # content-addressed stage paths, keeping previous builds available for review.
    if os.name == 'nt':
        signature = hashlib.sha256((target / 'plan.json').read_bytes()).hexdigest()[:20]
        staged = target / ('modded-' + signature)
    run([cli, 'apply', *base, '--out', staged])
    if args.action != 'stage':
        run(['cargo', 'fetch', '--locked', '--manifest-path', staged / 'Cargo.toml'])
        command = ['cargo', args.action, '--locked', '--offline', '--manifest-path', staged / 'Cargo.toml',
                   '-p', 'horizon-client', '--bin', 'horizon-client', '--no-default-features',
                   '--target-dir', target / 'horizon-cargo']
        if args.action == 'build':
            command.extend(['--profile', 'fast'])
        if args.action == 'test':
            command.extend(['--no-run'])
        run(command)
    print(staged)


if __name__ == '__main__':
    main()

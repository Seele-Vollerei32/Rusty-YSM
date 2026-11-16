"""Fetch one SPM player's authorized appearance and launch Horizon with its model pack."""
import argparse
import json
import os
from pathlib import Path
import subprocess
import tempfile
from launch import ROOT, pack_environment
from spm_cloud import CloudClient


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--origin', required=True)
    parser.add_argument('--player', required=True, help='Minecraft entity UUID to query')
    parser.add_argument('--resources', type=Path, required=True)
    parser.add_argument('--cache', type=Path, default=ROOT / 'target/spm-cache')
    parser.add_argument('--include-offline', action='store_true')
    parser.add_argument('--dry-run', action='store_true')
    args, extra = parser.parse_known_args()
    client = CloudClient(args.origin, os.environ.get('SPM_ACCESS_TOKEN', ''), args.cache)
    instance = client.discover()
    entries = client.query_players([args.player], args.include_offline)
    selection = next(iter(entries.values()))['selection']
    if selection is None:
        raise SystemExit('SPM player has no accessible appearance')
    data = client.download(selection)
    from spm_cloud import extract_pack
    with tempfile.TemporaryDirectory(prefix='spm-model-') as directory:
        pack = extract_pack(data, Path(directory) / 'pack')
        texture = selection.get('texture_id') or None
        config = pack_environment(pack, texture)
        env = {k: v for k, v in os.environ.items() if not k.startswith(('OPENYSM_', 'SPM_'))}
        env.update(config)
        binary = ROOT / 'target/horizon-cargo/fast' / ('horizon-client.exe' if os.name == 'nt' else 'horizon-client')
        command = [str(binary), '--resources', str(args.resources.resolve()), *extra]
        if args.dry_run:
            print(json.dumps({'instance': instance['instance_id'], 'asset_id': selection['asset_id'],
                              'asset_revision': selection['asset_revision'], 'command': command}, indent=2))
            return 0
        return subprocess.run(command, env=env).returncode


if __name__ == '__main__':
    raise SystemExit(main())

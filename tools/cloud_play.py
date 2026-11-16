"""Run Horizon with SPM per-player polling. Credentials stay in the worker process."""
import argparse
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import time
from launch import ROOT
from spm_cloud import CloudClient


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--origin', required=True)
    parser.add_argument('--resources', type=Path, required=True)
    args, extra = parser.parse_known_args()
    token = os.environ.get('SPM_ACCESS_TOKEN')
    if not token:
        parser.error('SPM_ACCESS_TOKEN is required')
    try:
        # Fail before launching the game on a bad origin or token shape.
        CloudClient(args.origin, token, Path(tempfile.gettempdir())/'spm-horizon-probe')
    except ValueError as error:
        parser.error(str(error))
    with tempfile.TemporaryDirectory(prefix='spm-horizon-') as directory:
        worker = subprocess.Popen([sys.executable, str(ROOT/'tools/cloud_watch.py'),
                                   '--origin', args.origin, '--bridge', directory])
        game = None
        try:
            env = {k: v for k, v in os.environ.items() if not k.startswith(('SPM_', 'OPENYSM_'))}
            env['SPM_BRIDGE_DIR'] = directory
            binary = ROOT/'target/horizon-cargo/fast'/('horizon-client.exe' if os.name == 'nt' else 'horizon-client')
            game = subprocess.Popen([str(binary), '--resources', str(args.resources.resolve()), *extra], env=env)
            # A worker that dies leaves no Cloud updates; stop rather than show stale models.
            while game.poll() is None:
                if worker.poll() is not None:
                    print('SPM sync worker exited; stopping the game', file=sys.stderr)
                    game.terminate()
                    return 1
                time.sleep(0.25)
            return game.returncode
        finally:
            if game is not None and game.poll() is None:
                game.terminate()
                try:
                    game.wait(timeout=5)
                except subprocess.TimeoutExpired:
                    game.kill()
                    game.wait()
            (Path(directory)/'stop').touch()
            try:
                worker.wait(timeout=5)
            except subprocess.TimeoutExpired:
                worker.terminate()
                worker.wait()


if __name__ == '__main__':
    raise SystemExit(main())

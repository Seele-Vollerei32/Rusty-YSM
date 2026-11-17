"""SPM polling worker for Horizon's local per-player bridge. Started by cloud_play.py."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import shutil
import time
import uuid
from launch import pack_environment
from rgba import decode_png
from display_state import validate_display
from spm_cloud import CloudClient, CloudError, atomic_write, extract_pack

REQUEST_MAX_AGE = 5


def renew(client, session):
    """Rotate the session after authorization expires. The refresh token lives only
    in worker memory: the bridge directory is temporary, so persisting it would add
    a credential copy without surviving a restart. Restarts take ambient env again."""
    if not session.get('refresh'):
        raise CloudError('no refresh token available')
    session['refresh'] = client.refresh_session(session['refresh'])
    return session['refresh']


def refresh(client, request, root):
    """Resolve every visible player independently; one bad pack cannot hide the rest."""
    ids = sorted({str(uuid.UUID(value)) for value in request['players'].values()})
    if len(ids) > 16:
        raise ValueError('native bridge supports 16 visible players')
    entries = client.query_players(ids) if ids else {}
    players = {}
    for entity, identifier in request['players'].items():
        row = entries[str(uuid.UUID(identifier))]
        selection = row['selection']
        if selection is None:
            continue
        try:
            # download() reauthorizes a cached revision before returning it.
            data = client.download(selection)
            digest = selection['raw_sha256']
            destination = root / 'packs' / digest
            marker = destination / '.pack-root'
            if not marker.exists():
                if destination.exists():
                    shutil.rmtree(destination)
                pack = extract_pack(data, destination)
                marker.write_text(str(pack.relative_to(destination)), encoding='utf-8')
            pack = destination / marker.read_text(encoding='utf-8')
            motion = selection.get('motion')
            if motion is not None and not isinstance(motion, dict):
                raise ValueError('invalid Cloud motion')
            display_state = validate_display(selection.get('display_state'))
            if display_state is not None and len(json.dumps(display_state, separators=(',', ':'))) > 64 * 1024:
                raise ValueError('Cloud display state too large')
            animation_key = motion.get('animation_key', '') if motion else ''
            if not isinstance(animation_key, str) or len(animation_key) > 256:
                raise ValueError('invalid Cloud animation key')
            try:
                config = pack_environment(pack, selection.get('texture_id') or None,
                                          animation_key or None)
            except ValueError:
                # Motion may refer to an event key rather than a packaged clip.
                # Keep the model visible with its neutral pose in that case.
                config = pack_environment(pack, selection.get('texture_id') or None) 
                motion = None
                animation_key = ''
            # Decode here so the game thread only uploads validated pixels.
            width, height, pixels = decode_png(Path(config['OPENYSM_TEXTURE_FILE']).read_bytes())
            pixel_hash = hashlib.sha256(pixels).hexdigest()
            rgba_path = root / 'rgba' / f'{width}x{height}-{pixel_hash}.rgba'
            if not rgba_path.is_file() or rgba_path.stat().st_size != len(pixels):
                atomic_write(rgba_path, pixels)
            players[entity] = {'uuid': str(uuid.UUID(identifier)), 'revision': row['revision'],
                               'model': config['OPENYSM_MODEL'], 'rgba': str(rgba_path),
                               'animation': animation_key or None,
                               'animation_path': config.get('OPENYSM_ANIMATION'),
                               'width': width, 'height': height,
                               'motion': motion,
                               'display_state': display_state,
                               'key': hashlib.sha256(json.dumps(
                                   {'asset': digest, 'texture': selection.get('texture_id', ''),
                                    'animation': animation_key or None,
                                    'animation_path': config.get('OPENYSM_ANIMATION')}, 
                                   sort_keys=True, separators=(',', ':')).encode()).hexdigest()}
        except (ValueError, RuntimeError, OSError, KeyError) as error:
            if isinstance(error, CloudError) and error.status == 401:
                raise
            print(f'SPM model unavailable: {type(error).__name__}', flush=True)
    return {'generation': request['generation'], 'expires': time.time() + 15, 'players': players}


def poll_once(client, root, discovered, session):
    """Run one poll. Returns (published, discovered); a failed poll withdraws the state
    so the client falls back to vanilla models instead of showing stale ones."""
    request_path = root / 'request.json'
    request = None
    try:
        request = json.loads(request_path.read_text(encoding='utf-8'))
        if not isinstance(request, dict):
            request = None
            raise ValueError('invalid bridge request')
        players = request.get('players')
        if (type(request.get('generation')) is not int or not 0 <= request['generation'] < 2**64
                or not isinstance(players, dict) or len(players) > 16
                or any(not key.isascii() or not key.isdecimal() or len(key) > 10
                       or int(key) > 2**32 - 1 or str(int(key)) != key
                       or not isinstance(value, str) for key, value in players.items())):
            raise ValueError('invalid bridge request fields')
        if time.time() - request_path.stat().st_mtime > REQUEST_MAX_AGE:
            raise ValueError('stale game request')
        # Retry once after an expiry, including discovery and asset authorization.
        try:
            if not discovered:
                client.discover()
                discovered = True
            result = refresh(client, request, root)
        except CloudError as error:
            if error.status != 401:
                raise
            renew(client, session)
            if not discovered:
                client.discover()
                discovered = True
            result = refresh(client, request, root)
        # Players joined or left while the network request was in flight.
        if (json.loads(request_path.read_text(encoding='utf-8')) != request
                or time.time() - request_path.stat().st_mtime > REQUEST_MAX_AGE):
            raise ValueError('game changed while Cloud request was in flight')
        atomic_write(root / 'state.json', json.dumps(result).encode())
        return True, discovered
    except (OSError, ValueError, RuntimeError, KeyError):
        if request is not None:
            withdrawal = {'generation': request.get('generation'), 'expires': 0, 'players': {}}
            try:
                atomic_write(root / 'state.json', json.dumps(withdrawal).encode())
            except OSError:
                pass
        return False, False


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--origin', required=True)
    parser.add_argument('--bridge', type=Path, required=True)
    args = parser.parse_args()
    root = args.bridge.resolve()
    client = CloudClient(args.origin, os.environ.get('SPM_ACCESS_TOKEN', ''), root / 'cache')
    session = {'refresh': os.environ.get('SPM_REFRESH_TOKEN') or None}
    failures = 0
    discovered = False
    while not (root / 'stop').exists():
        published, discovered = poll_once(client, root, discovered, session)
        failures = 0 if published else min(failures + 1, 4)
        for _ in range(30 if failures == 0 else min(300, 20 * 2 ** failures)):
            if (root / 'stop').exists():
                return
            time.sleep(0.1)


if __name__ == '__main__':
    main()

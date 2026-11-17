import io
import json
import os
from pathlib import Path
import struct
import tempfile
import unittest
import uuid
import zipfile
import zlib
from cloud_watch import poll_once, refresh, renew


def chunk(kind, payload):
    return struct.pack('>I', len(payload)) + kind + payload + struct.pack('>I', zlib.crc32(kind + payload))


def tiny_png():
    """2x2 RGB PNG: the worker decodes it into 16 bytes of RGBA."""
    rows = b'\x00' + b'\x10\x20\x30' * 2 + b'\x00' + b'\x40\x50\x60' * 2
    return (b'\x89PNG\r\n\x1a\n'
            + chunk(b'IHDR', struct.pack('>IIBBBBB', 2, 2, 8, 2, 0, 0, 0))
            + chunk(b'IDAT', zlib.compress(rows)) + chunk(b'IEND', b''))


def pack_zip(animated=False):
    stream = io.BytesIO()
    with zipfile.ZipFile(stream, 'w') as archive:
        files = {'model': {'main': 'model.json'}, 'texture': ['skin.png']}
        if animated:
            files['animation'] = {'idle': 'animation.json'}
        archive.writestr('ysm.json', json.dumps({'files': {'player': files}}))
        archive.writestr('model.json', '{}')
        if animated:
            archive.writestr('animation.json', json.dumps({'animations': {
                'idle': {'animation_length': 1, 'bones': {'root': {'scale': [1, 1, 1]}}}}}))
        archive.writestr('skin.png', tiny_png())
    return stream.getvalue()


class FakeCloud:
    """Network boundary double: responses are explicit fixtures, not real HTTP."""
    def __init__(self, identifiers, *, expire_once=False, fail=False):
        self.ids = identifiers
        self.cleared = False
        self.calls = 0
        self.expire_once = expire_once
        self.fail = fail
        self.discoveries = 0
        self.refreshed = None

    def discover(self):
        self.discoveries += 1

    def query_players(self, ids):
        if self.expire_once:
            self.expire_once = False
            from spm_cloud import CloudError
            raise CloudError('expired', 401)
        if self.fail:
            raise OSError('network down')
        return {u: {'revision': 2, 'selection': None if self.cleared else {
            'asset_id': str(i), 'asset_revision': 1, 'raw_sha256': str(i) * 64,
            'texture_id': '', 'format': 'zip'}} for i, u in enumerate(self.ids)}

    def download(self, selection):
        self.calls += 1
        return pack_zip(bool(selection.get('motion')))

    def refresh_session(self, token):
        self.refreshed = token
        return 'rotated'


class PollingTests(unittest.TestCase):
    def setUp(self):
        self.directory = tempfile.TemporaryDirectory()
        self.root = Path(self.directory.name)
        self.ids = [str(uuid.uuid4()), str(uuid.uuid4())]
        self.write({'generation': 5, 'players': {'7': self.ids[0], '9': self.ids[1]}})
        self.session = {'refresh': None}

    def tearDown(self):
        # Windows can briefly retain a deleted directory entry (WinError 145).
        # Retry only that transient error; never suppress a final cleanup failure.
        import time
        for attempt in range(4):
            try:
                self.directory.cleanup()
                break
            except OSError as error:
                if getattr(error, 'winerror', None) != 145 or attempt == 3:
                    raise
                time.sleep(0.05 * (attempt + 1))

    def write(self, request):
        (self.root/'request.json').write_text(json.dumps(request), encoding='utf-8')

    def state(self):
        return json.loads((self.root/'state.json').read_text(encoding='utf-8'))

    def test_animation_selection_and_unknown_key_fallback(self):
        class AnimatedCloud(FakeCloud):
            started = 1
            pose = 2

            def query_players(self, ids):
                rows = super().query_players(ids)
                rows[self.ids[0]]['selection']['motion'] = {
                    'animation_key': 'idle', 'started_at_unix_ms': self.started,
                    'roaming': {'variable.pose': self.pose}}
                rows[self.ids[1]]['selection']['motion'] = {'animation_key': 'missing'}
                return rows
        client = AnimatedCloud(self.ids)
        self.assertTrue(poll_once(client, self.root, False, self.session)[0])
        rows = self.state()['players']
        self.assertEqual(rows['7']['animation'], 'idle')
        self.assertTrue(rows['7']['animation_path'].endswith('animation.json'))
        self.assertIsNone(rows['9']['animation'])
        self.assertIsNone(rows['9']['motion'])
        client.started = 2000
        client.pose = 3
        self.assertTrue(poll_once(client, self.root, True, self.session)[0])
        updated = self.state()['players']['7']
        self.assertEqual(rows['7']['key'], updated['key'])
        self.assertEqual(updated['motion']['started_at_unix_ms'], 2000)
        self.assertEqual(updated['motion']['roaming']['variable.pose'], 3)

    def test_independent_models_and_withdrawal(self):
        client = FakeCloud(self.ids)
        published, discovered = poll_once(client, self.root, False, self.session)
        self.assertTrue(published)
        self.assertTrue(discovered)
        result = self.state()
        self.assertEqual(result['generation'], 5)
        self.assertEqual(set(result['players']), {'7', '9'})
        self.assertNotEqual(result['players']['7']['model'], result['players']['9']['model'])
        skin = Path(result['players']['7']['rgba'])
        self.assertEqual(skin.stat().st_size, 16, 'worker decodes 2x2 RGBA')
        self.assertEqual(skin.read_bytes()[:4], b'\x10\x20\x30\xff')
        self.assertEqual((result['players']['7']['width'], result['players']['7']['height']), (2, 2))
        self.assertNotIn('texture', result['players']['7'], 'no PNG path leaks to the game thread')
        self.assertEqual(client.discoveries, 1)
        # download() owns reauthorization; a repeat poll must still ask again.
        poll_once(client, self.root, discovered, self.session)
        self.assertEqual(client.calls, 4)
        client.cleared = True
        poll_once(client, self.root, discovered, self.session)
        self.assertEqual(self.state()['players'], {})
        self.assertGreater(self.state()['expires'], 0, 'an explicit clear is a valid, live response')

    def test_transport_failure_withdraws_previous_models(self):
        client = FakeCloud(self.ids)
        poll_once(client, self.root, False, self.session)
        self.assertTrue(self.state()['players'])
        client.fail = True
        published, discovered = poll_once(client, self.root, True, self.session)
        self.assertFalse(published)
        self.assertFalse(discovered, 'rediscover after a transport reset')
        self.assertEqual(self.state(), {'generation': 5, 'expires': 0, 'players': {}})

    def test_expired_session_refreshes_and_keeps_models(self):
        session = {'refresh': 'old-refresh'}
        client = FakeCloud(self.ids, expire_once=True)
        published, _ = poll_once(client, self.root, False, session)
        self.assertTrue(published)
        self.assertEqual(client.refreshed, 'old-refresh')
        self.assertEqual(session['refresh'], 'rotated', 'rotated token kept in memory only')
        self.assertFalse((self.root/'refresh').exists())
        self.assertEqual(set(self.state()['players']), {'7', '9'})

    def test_stale_request_is_ignored(self):
        client = FakeCloud(self.ids)
        poll_once(client, self.root, False, self.session)
        age = 100
        os.utime(self.root/'request.json', (age, age))
        published, _ = poll_once(client, self.root, True, self.session)
        self.assertFalse(published)
        self.assertEqual(client.calls, 2, 'no network work for an abandoned game session')
        self.assertEqual(self.state()['expires'], 0)

    def test_player_change_during_request_is_discarded(self):
        class Changing(FakeCloud):
            def __init__(self, ids, root):
                super().__init__(ids)
                self.root = root

            def query_players(self, ids):
                self.root.joinpath('request.json').write_text(json.dumps(
                    {'generation': 6, 'players': {'7': self.ids[0]}}))
                return super().query_players(ids)

        published, _ = poll_once(Changing(self.ids, self.root), self.root, False, self.session)
        self.assertFalse(published)
        self.assertEqual(self.state()['players'], {})
        self.assertNotEqual(self.state()['generation'], 6)

    def test_invalid_request_does_not_crash_or_download(self):
        client = FakeCloud(self.ids)
        for request in [[], 'bad', {}, {'generation': True, 'players': {}},
                        {'generation': -1, 'players': {}},
                        {'generation': 2**64, 'players': {}},
                        {'generation': 1, 'players': []},
                        {'generation': 1, 'players': {'7': None}},
                        {'generation': 1, 'players': {'07': self.ids[0]}},
                        {'generation': 1, 'players': {'4294967296': self.ids[0]}}]:
            with self.subTest(request=request):
                self.write(request)
                self.assertFalse(poll_once(client, self.root, False, self.session)[0])
        self.assertEqual(client.calls, 0)
        self.assertEqual(client.discoveries, 0)
        self.write({'generation': 2, 'players': {'7': self.ids[0]}})
        self.assertTrue(poll_once(client, self.root, False, self.session)[0])

    def test_request_removed_during_download_is_discarded(self):
        root = self.root
        class Disabled(FakeCloud):
            def query_players(self, ids):
                (root / 'request.json').unlink()
                return super().query_players(ids)
        client = Disabled(self.ids)
        self.assertFalse(poll_once(client, root, False, self.session)[0])
        self.assertEqual(self.state()['players'], {})
        calls = client.calls
        self.assertFalse(poll_once(client, root, False, self.session)[0])
        self.assertEqual(client.calls, calls, 'disabled bridge performs no further downloads')

    def test_asset_expiry_retries_with_rotated_session(self):
        from spm_cloud import CloudError
        class ExpiringAsset(FakeCloud):
            expired = False
            def download(self, selection):
                if not self.expired:
                    self.expired = True
                    raise CloudError('expired', 401)
                return super().download(selection)
        session = {'refresh': 'old'}
        client = ExpiringAsset(self.ids)
        self.assertTrue(poll_once(client, self.root, False, session)[0])
        self.assertEqual(session['refresh'], 'rotated')
        self.assertEqual(len(self.state()['players']), 2)

    def test_same_size_textures_have_independent_rgba(self):
        from unittest.mock import patch
        client = FakeCloud(self.ids)
        with patch('cloud_watch.decode_png', side_effect=[
                (1, 1, bytes([255, 0, 0, 255])),
                (1, 1, bytes([0, 255, 0, 255]))]):
            self.assertTrue(poll_once(client, self.root, False, self.session)[0])
        rows = self.state()['players']
        self.assertNotEqual(rows['7']['rgba'], rows['9']['rgba'])
        self.assertNotEqual(Path(rows['7']['rgba']).read_bytes(), Path(rows['9']['rgba']).read_bytes())

    def test_display_state_updates_and_withdrawal(self):
        class DisplayCloud(FakeCloud):
            display = {'scope_id': 'test', 'world_epoch': 'epoch',
                       'dimension_id': 'minecraft:overworld',
                       'state': {'health': 12, 'flying': True}}

            def query_players(self, ids):
                rows = super().query_players(ids)
                for row in rows.values():
                    row['selection']['display_state'] = self.display
                return rows

        client = DisplayCloud(self.ids)
        self.assertTrue(poll_once(client, self.root, False, self.session)[0])
        first = self.state()['players']['7']
        self.assertEqual(first['display_state'], client.display)
        client.display = None
        self.assertTrue(poll_once(client, self.root, True, self.session)[0])
        second = self.state()['players']['7']
        self.assertIsNone(second['display_state'])
        self.assertEqual(first['key'], second['key'], 'display updates reuse geometry and texture')
        client.display = 'invalid'
        self.assertTrue(poll_once(client, self.root, True, self.session)[0])
        self.assertEqual(self.state()['players'], {})

    def test_expired_session_rotates_refresh_token(self):
        class FakeCloudRenew:
            def refresh_session(self, token):
                self.seen = token
                return 'rotated'

        client = FakeCloudRenew()
        session = {'refresh': 'old'}
        renew(client, session)
        self.assertEqual(client.seen, 'old')
        self.assertEqual(session['refresh'], 'rotated')
        with self.assertRaises(Exception):
            renew(client, {'refresh': None})


if __name__ == '__main__':
    unittest.main()

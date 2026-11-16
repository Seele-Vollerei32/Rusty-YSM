import hashlib
import io
import json
from pathlib import Path
import tempfile
import unittest
import uuid
import zipfile
from spm_cloud import CloudClient, CloudError, asset_path, extract_pack


class CloudContracts(unittest.TestCase):
    def setUp(self):
        self.client = CloudClient('https://example.org', 'test-token', 'unused-cache')
        self.player = str(uuid.uuid4())
        self.ref = dict(asset_id='a/b.c', asset_revision=2, raw_sha256='a'*64, format='zip')

    def test_http_transport_against_local_protocol_fixture(self):
        from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
        import threading
        class Handler(BaseHTTPRequestHandler):
            def log_message(self, *args):
                pass
            def do_GET(self):
                if self.headers.get('Authorization') != 'Bearer test-token':
                    self.send_error(401)
                    return
                if self.headers.get('If-None-Match'):
                    self.send_response(304)
                    self.send_header('ETag', self.headers['If-None-Match'])
                    self.end_headers()
                    return
                body = json.dumps({'protocol': 'spm.cloud.v1', 'instance_id': 'fixture'}).encode()
                self.send_response(200)
                self.send_header('Content-Length', str(len(body)))
                self.end_headers()
                self.wfile.write(body)
        server = ThreadingHTTPServer(('127.0.0.1', 0), Handler)
        thread = threading.Thread(target=server.serve_forever, daemon=True)
        thread.start()
        try:
            client = CloudClient(f'http://127.0.0.1:{server.server_port}', 'test-token', 'unused-cache', allow_loopback=True)
            self.assertEqual(client.discover()['instance_id'], 'fixture')
            self.assertTrue(client.authorize(self.ref))
        finally:
            server.shutdown()
            server.server_close()
            thread.join()

    def test_origin_and_reference(self):
        for origin in ['http://example.org', 'https://user@example.org', 'https://example.org/api']:
            with self.assertRaises(ValueError):
                CloudClient(origin, 'token', 'cache')
        self.assertIn('a%2Fb%2Ec/revisions/2/content', asset_path(self.ref))
        with self.assertRaises(ValueError):
            asset_path({**self.ref, 'asset_revision': 0})

    def test_query_contract_and_unsolicited_rejection(self):
        def request(method, path, body):
            self.assertEqual(path, '/v1/players/appearances/query')
            self.assertEqual(body['entity_uuids'], [self.player])
            return {'entries': [{'entity_uuid': self.player, 'revision': 1, 'selection': self.ref}]}
        self.client.json = request
        self.assertEqual(self.client.query_players([self.player])[self.player]['selection'], self.ref)
        self.client.json = lambda *args: {'entries': [{'entity_uuid': str(uuid.uuid4()), 'selection': None}]}
        with self.assertRaises(CloudError):
            self.client.query_players([self.player])

    def test_hash_and_reauthorization(self):
        with tempfile.TemporaryDirectory() as directory:
            self.client.cache = Path(directory)
            ref = {**self.ref, 'raw_sha256': hashlib.sha256(b'bytes').hexdigest()}
            calls = []
            def request(*args, **kwargs):
                calls.append(args)
                return b'bytes', {'X-Asset-Format': 'zip'}
            self.client.request = request
            self.client.download(ref)
            authorized = []
            self.client.authorize = lambda ref: authorized.append(ref) or True
            self.client.download(ref)
            self.assertEqual(len(calls), 1)
            self.assertEqual(len(authorized), 1)
            self.client.authorize = lambda ref: False
            with self.assertRaises(CloudError):
                self.client.download(ref)
            self.assertFalse(list(Path(directory).glob('*.zip')))
            self.client.request = lambda *a, **k: (b'corrupted', {})
            with self.assertRaises(CloudError):
                self.client.download(ref)
            self.assertFalse(list(Path(directory).glob('*.zip')))

    def test_publish_revision_and_clear(self):
        def request(method, path, body):
            self.assertEqual(method, 'PUT')
            self.assertEqual(path, '/v1/players/me/appearance')
            self.assertEqual(body['expected_revision'], 7)
            self.assertIsNone(body['asset_id'])
            return {'revision': 8}
        self.client.json = request
        self.assertEqual(self.client.publish('bound-identity', self.player, 7), 8)

    def test_zip_paths(self):
        for name, valid in [('pack/ysm.json', True), ('../ysm.json', False), ('C:/ysm.json', False)]:
            stream = io.BytesIO()
            with zipfile.ZipFile(stream, 'w') as archive:
                archive.writestr(name, json.dumps({}))
            with tempfile.TemporaryDirectory() as directory:
                destination = Path(directory) / 'fresh'
                if valid:
                    self.assertEqual(extract_pack(stream.getvalue(), destination), destination / 'pack')
                else:
                    with self.assertRaises(CloudError):
                        extract_pack(stream.getvalue(), destination)


if __name__ == '__main__':
    unittest.main()

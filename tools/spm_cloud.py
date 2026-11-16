"""SPM Cloud v1 HTTP client. No OpenYSM/Minecraft server packets are used."""
import hashlib
import io
import json
import os
from pathlib import Path, PurePosixPath
import re
import tempfile
import urllib.error
import urllib.parse
import urllib.request
import uuid
import zipfile

MAX_ASSET = 64 * 1024 * 1024


class NoRedirect(urllib.request.HTTPRedirectHandler):
    def redirect_request(self, req, fp, code, msg, headers, newurl):
        return None  # Never forward account credentials to another origin/CDN.


class CloudError(RuntimeError):
    def __init__(self, message, status=None):
        super().__init__(message)
        self.status = status


class CloudClient:
    def __init__(self, origin, token, cache, *, allow_loopback=False):
        parsed = urllib.parse.urlsplit(origin)
        local = allow_loopback and parsed.scheme == 'http' and parsed.hostname in ('localhost', '127.0.0.1', '::1')
        if (parsed.scheme != 'https' and not local) or not parsed.netloc or parsed.username or parsed.password or parsed.path not in ('', '/') or parsed.query or parsed.fragment:
            raise ValueError('Cloud origin must be bare HTTPS (HTTP loopback only for tests)')
        if not token or any(ord(c) < 32 for c in token):
            raise ValueError('invalid access token')
        self.origin = origin.rstrip('/')
        self.token = token
        self.cache = Path(cache) / hashlib.sha256(self.origin.encode()).hexdigest()
        self.opener = urllib.request.build_opener(NoRedirect())

    def request(self, method, path, body=None, limit=16 * 1024 * 1024):
        if not path.startswith('/v1/') or '..' in path or path.startswith('//'):
            raise ValueError('invalid Cloud API path')
        data = None if body is None else json.dumps(body).encode()
        request = urllib.request.Request(self.origin + path, data=data, method=method,
            headers={'Authorization': 'Bearer ' + self.token, 'Accept': 'application/json',
                     'Content-Type': 'application/json', 'User-Agent': 'openysm-horizon/0.1.0'})
        try:
            with self.opener.open(request, timeout=20) as response:
                if response.status != 200:
                    raise CloudError(f'Cloud HTTP {response.status}')
                content = response.read(limit + 1)
                if len(content) > limit:
                    raise CloudError('Cloud response exceeds byte budget')
                return content, response.headers
        except urllib.error.HTTPError as error:
            error.close()
            raise CloudError(f'Cloud HTTP {error.code}', error.code) from None

    def json(self, method, path, body=None):
        content, _ = self.request(method, path, body)
        return json.loads(content)

    def refresh_session(self, refresh_token):
        if not refresh_token or any(ord(c) < 32 for c in refresh_token):
            raise ValueError('invalid refresh token')
        session = self.json('POST', '/v1/sessions/refresh', {'refresh_token': refresh_token})
        access = session.get('access_token')
        refresh = session.get('refresh_token')
        if not isinstance(access, str) or not access or not isinstance(refresh, str) or not refresh:
            raise CloudError('malformed session refresh')
        if any(ord(c) < 32 for c in access + refresh):
            raise CloudError('malformed session token')
        self.token = access
        return refresh

    def discover(self):
        result = self.json('GET', '/v1/instance')
        if result.get('protocol') != 'spm.cloud.v1':
            raise CloudError('instance does not support spm.cloud.v1')
        return result

    def query_players(self, identifiers, include_offline=False):
        ids = {str(uuid.UUID(value)) for value in identifiers}
        if len(ids) > 64:
            raise ValueError('SPM player queries allow at most 64 UUIDs')
        result = self.json('POST', '/v1/players/appearances/query',
                           {'entity_uuids': sorted(ids), 'include_offline': include_offline})
        entries = result['entries']
        if len(entries) > 64:
            raise CloudError('oversized player response')
        output = {}
        for entry in entries:
            key = str(uuid.UUID(entry['entity_uuid']))
            if key not in ids or key in output:
                raise CloudError('duplicate or unsolicited player UUID')
            selection = entry['selection']
            if selection is not None:
                asset_path(selection)
            output[key] = entry
        if set(output) != ids:
            raise CloudError('player response UUID set mismatch')
        return output

    def publish(self, identity_id, entity_uuid, expected_revision, selection=None):
        if expected_revision < 0:
            raise ValueError('invalid appearance revision')
        body = {'identity_id': identity_id, 'entity_uuid': str(uuid.UUID(entity_uuid)),
                'expected_revision': expected_revision, 'asset_id': None, 'motion': None}
        if selection is not None:
            asset_path(selection)
            body.update({key: selection[key] for key in ('asset_id', 'asset_revision', 'raw_sha256', 'texture_id')})
        # Server verifies the bound game identity and optimistic revision. Do not retry conflicts blindly.
        return self.json('PUT', '/v1/players/me/appearance', body)['revision']

    def authorize(self, selection):
        """Match Sparkle-Morpher conditional GET authorization without buffering the body."""
        etag = '"' + selection['raw_sha256'] + '"'
        request = urllib.request.Request(self.origin + asset_path(selection), headers={
            'Authorization': 'Bearer ' + self.token, 'If-None-Match': etag})
        try:
            response = self.opener.open(request, timeout=20)
        except urllib.error.HTTPError as error:
            if error.code != 304:
                error.close()
                raise CloudError(f'Cloud HTTP {error.code}', error.code) from None
            response = error
        with response:
            return response.code in (200, 304) and response.headers.get('ETag') == etag

    def download(self, selection):
        path = asset_path(selection)
        destination = self.cache / (selection['raw_sha256'] + '.zip')
        if selection.get('format') != 'zip':
            raise CloudError('native importer currently requires ZIP format')
        if destination.is_file():
            if destination.stat().st_size <= MAX_ASSET:
                cached = destination.read_bytes()
                if hashlib.sha256(cached).hexdigest() == selection['raw_sha256']:
                    try:
                        allowed = self.authorize(selection)
                    except (CloudError, OSError):
                        destination.unlink(missing_ok=True)
                        raise
                    if allowed:
                        return cached
                    destination.unlink(missing_ok=True)
                    raise CloudError('cached model display authorization denied')
            destination.unlink(missing_ok=True)
        data, headers = self.request('GET', path, limit=MAX_ASSET)
        if hashlib.sha256(data).hexdigest() != selection['raw_sha256']:
            raise CloudError('asset SHA-256 mismatch')
        fmt = headers.get('X-Asset-Format', selection.get('format'))
        if fmt != 'zip':
            raise CloudError(f'Cloud format {fmt!r} is not yet supported by the native importer')
        self.cache.mkdir(parents=True, exist_ok=True)
        atomic_write(destination, data)
        return data


def asset_path(selection):
    asset = selection['asset_id']
    revision = selection['asset_revision']
    digest = selection['raw_sha256']
    if not isinstance(asset, str) or not asset or len(asset.encode()) > 128 or any(ord(c) < 32 for c in asset):
        raise ValueError('invalid asset ID')
    if isinstance(revision, bool) or not isinstance(revision, int) or revision <= 0:
        raise ValueError('invalid asset revision')
    if not isinstance(digest, str) or not re.fullmatch('[0-9a-f]{64}', digest):
        raise ValueError('invalid asset hash')
    encoded = urllib.parse.quote(asset, safe='').replace('.', '%2E')
    return f'/v1/assets/{encoded}/revisions/{revision}/content'


def atomic_write(path, data):
    path = Path(path)
    path.parent.mkdir(parents=True, exist_ok=True)
    fd, temporary = tempfile.mkstemp(dir=path.parent)
    try:
        with os.fdopen(fd, 'wb') as stream:
            stream.write(data)
        os.replace(temporary, path)
    finally:
        if os.path.exists(temporary):
            os.unlink(temporary)


def extract_pack(data, destination):
    """Extract a verified ZIP to a fresh directory with strict path/size budgets."""
    destination = Path(destination)
    if destination.exists():
        raise ValueError('extraction destination must be fresh')
    with zipfile.ZipFile(io.BytesIO(data)) as archive:
        entries = archive.infolist()
        if len(entries) > 10000 or sum(e.file_size for e in entries) > 256 * 1024 * 1024:
            raise CloudError('expanded model exceeds budget')
        seen = set()
        for entry in entries:
            p = PurePosixPath(entry.filename)
            if p.is_absolute() or '..' in p.parts or '\\' in entry.filename or ':' in entry.filename or not p.parts:
                raise CloudError('invalid archive path')
            key = p.as_posix().casefold()
            if key in seen or ((entry.external_attr >> 16) & 0o170000) == 0o120000:
                raise CloudError('duplicate archive path or symlink')
            seen.add(key)
        destination.mkdir(parents=True)
        for entry in entries:
            path = destination.joinpath(*PurePosixPath(entry.filename).parts)
            if entry.is_dir():
                path.mkdir(parents=True, exist_ok=True)
            else:
                path.parent.mkdir(parents=True, exist_ok=True)
                path.write_bytes(archive.read(entry))
    manifests = list(destination.rglob('ysm.json'))
    if len(manifests) != 1:
        raise CloudError('expected exactly one YSM pack manifest')
    return manifests[0].parent

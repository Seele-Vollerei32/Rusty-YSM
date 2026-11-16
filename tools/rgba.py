"""Minimal PNG decode for cloud model textures (8-bit RGB/RGBA, non-interlaced).

Decoding runs in the sync worker so the game thread only uploads validated pixels.
Anything outside this subset is rejected instead of guessed at.
"""
import struct
import zlib

MAX_SIDE = 512
CHANNELS = {2: 3, 6: 4}


def decode_png(data):
    """Return (width, height, rgba bytes) for a PNG within the slot budget."""
    if len(data) < 8 or data[:8] != b'\x89PNG\r\n\x1a\n' or len(data) > 32 * 1024 * 1024:
        raise ValueError('not a bounded PNG')
    offset = 8
    header = None
    compressed = []
    ended = False
    while offset + 12 <= len(data):
        length, kind = struct.unpack('>I4s', data[offset:offset + 8])
        offset += 8
        if length > len(data) - offset - 4:
            raise ValueError('truncated PNG chunk')
        payload = data[offset:offset + length]
        crc = struct.unpack('>I', data[offset + length:offset + length + 4])[0]
        if zlib.crc32(kind + payload) != crc:
            raise ValueError('PNG CRC mismatch')
        offset += length + 4
        if kind == b'IHDR':
            if header is not None or length != 13:
                raise ValueError('invalid PNG header')
            width, height, depth, color, compression, filtering, interlace = struct.unpack('>IIBBBBB', payload)
            if not (0 < width <= MAX_SIDE and 0 < height <= MAX_SIDE):
                raise ValueError('texture exceeds the 512 px cloud slot')
            if depth != 8 or color not in CHANNELS or compression or filtering or interlace:
                raise ValueError('unsupported PNG format')
            header = (width, height, CHANNELS[color])
        elif kind == b'IDAT':
            if header is None:
                raise ValueError('PNG chunk before header')
            compressed.append(payload)
        elif kind == b'IEND':
            if length != 0 or offset != len(data):
                raise ValueError('invalid PNG end')
            ended = True
            break
        elif kind == b'tRNS':
            raise ValueError('RGB transparency keys are not supported; use RGBA')
        elif kind != b'PLTE':
            # Ancillary chunks (gamma, text, ...) are irrelevant and skipped.
            if not kind[0] & 0x20:
                raise ValueError('unsupported critical PNG chunk')
    if header is None or not ended or not compressed:
        raise ValueError('incomplete PNG')
    width, height, channels = header
    try:
        decoder = zlib.decompressobj()
        budget = height * (width * channels + 1)
        raw = decoder.decompress(b''.join(compressed), budget + 1)
        if len(raw) > budget or not decoder.eof or decoder.unused_data:
            raise ValueError('PNG decompression exceeds budget or is incomplete')
    except zlib.error as error:
        raise ValueError('corrupt PNG data') from error
    stride = width * channels
    if len(raw) != height * (stride + 1):
        raise ValueError('PNG scanline size mismatch')
    pixels = bytearray(width * height * 4)
    previous = bytearray(stride)
    source = 0
    target = 0
    for _ in range(height):
        filter_type = raw[source]
        source += 1
        line = bytearray(raw[source:source + stride])
        source += stride
        if filter_type == 1:
            for i in range(channels, stride):
                line[i] = (line[i] + line[i - channels]) & 0xFF
        elif filter_type == 2:
            for i in range(stride):
                line[i] = (line[i] + previous[i]) & 0xFF
        elif filter_type == 3:
            for i in range(stride):
                left = line[i - channels] if i >= channels else 0
                line[i] = (line[i] + ((left + previous[i]) >> 1)) & 0xFF
        elif filter_type == 4:
            for i in range(stride):
                left = line[i - channels] if i >= channels else 0
                up = previous[i]
                corner = previous[i - channels] if i >= channels else 0
                estimate = left + up - corner
                pa, pb, pc = abs(estimate - left), abs(estimate - up), abs(estimate - corner)
                predictor = left if pa <= pb and pa <= pc else up if pb <= pc else corner
                line[i] = (line[i] + predictor) & 0xFF
        elif filter_type != 0:
            raise ValueError('invalid PNG filter')
        if channels == 4:
            pixels[target:target + stride] = line
        else:
            for x in range(width):
                base = x * 3
                pixels[target:target + 4] = line[base:base + 3] + b'\xff'
                target += 4
            previous = line
            continue
        target += stride
        previous = line
    return width, height, bytes(pixels)

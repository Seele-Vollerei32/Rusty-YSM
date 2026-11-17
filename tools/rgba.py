"""Bounded non-interlaced PNG decoder with all standard color types and bit depths.

Decoding runs in the sync worker so the game thread only uploads validated pixels.
Anything outside this subset is rejected instead of guessed at.
"""
import struct
import zlib

MAX_SIDE = 512
CHANNELS = {0: 1, 2: 3, 3: 1, 4: 2, 6: 4}


def decode_png(data):
    """Return (width, height, rgba bytes) for a PNG within the slot budget."""
    if len(data) < 8 or data[:8] != b'\x89PNG\r\n\x1a\n' or len(data) > 32 * 1024 * 1024:
        raise ValueError('not a bounded PNG')
    offset = 8
    header = None
    compressed = []
    ended = False
    palette = None
    transparency = None
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
        if header is None and kind != b'IHDR':
            raise ValueError('PNG header must be first')
        if kind == b'IHDR':
            if header is not None or length != 13:
                raise ValueError('invalid PNG header')
            width, height, depth, color, compression, filtering, interlace = struct.unpack('>IIBBBBB', payload)
            if not (0 < width <= MAX_SIDE and 0 < height <= MAX_SIDE):
                raise ValueError('texture exceeds the 512 px cloud slot')
            if (color not in CHANNELS or depth not in ((1, 2, 4, 8) if color == 3 else (1, 2, 4, 8, 16) if color == 0 else (8, 16))
                    or compression or filtering or interlace):
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
        elif kind == b'PLTE':
            if header is None or compressed or palette is not None or not length or length % 3 or length > 768 or color in (0, 4):
                raise ValueError('invalid PNG palette')
            if color == 3 and length // 3 > 1 << depth:
                raise ValueError('PNG palette exceeds bit depth')
            palette = payload
        elif kind == b'tRNS':
            if header is None or compressed or transparency is not None:
                raise ValueError('invalid PNG transparency order')
            if ((color == 0 and length == 2) or (color == 2 and length == 6)):
                transparency = struct.unpack('>' + 'H' * (length // 2), payload)
                if any(sample >= 1 << depth for sample in transparency):
                    raise ValueError('invalid PNG transparency sample')
            elif color == 3 and palette is not None and 0 < length <= len(palette) // 3:
                transparency = payload
            else:
                raise ValueError('invalid PNG transparency')
        else:
            # Ancillary chunks (gamma, text, ...) are irrelevant and skipped.
            if not kind[0] & 0x20:
                raise ValueError('unsupported critical PNG chunk')
    if header is None or not ended or not compressed:
        raise ValueError('incomplete PNG')
    if color == 3 and palette is None:
        raise ValueError('missing PNG palette')
    width, height, channels = header
    try:
        decoder = zlib.decompressobj()
        budget = height * ((width * channels * depth + 7) // 8 + 1)
        raw = decoder.decompress(b''.join(compressed), budget + 1)
        if len(raw) > budget or not decoder.eof or decoder.unused_data:
            raise ValueError('PNG decompression exceeds budget or is incomplete')
    except zlib.error as error:
        raise ValueError('corrupt PNG data') from error
    stride = (width * channels * depth + 7) // 8
    # PNG filters operate on bytes; sub-byte pixels still have a one-byte offset.
    channels = max(1, channels * depth // 8)
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
        for x in range(width):
            if depth < 8:
                bit = x * depth
                sample = bytes([(line[bit // 8] >> (8 - depth - bit % 8)) & ((1 << depth) - 1)])
            else:
                sample = line[x * channels:(x + 1) * channels]
            if depth == 16:
                # Compare tRNS before reducing precision; equal high bytes alone
                # must not make a different 16-bit sample transparent.
                original = struct.unpack('>' + 'H' * CHANNELS[color], sample)
                reduced = bytes(value >> 8 for value in original)
                if color == 6:
                    rgba = reduced
                elif color == 4:
                    rgba = bytes([reduced[0]] * 3 + [reduced[1]])
                else:
                    alpha = 0 if transparency == original else 255
                    rgb = bytes([reduced[0]] * 3) if color == 0 else reduced
                    rgba = rgb + bytes([alpha])
            elif color == 6:
                rgba = sample
            elif color == 4:
                rgba = bytes([sample[0]] * 3 + [sample[1]])
            elif color == 3:
                index = sample[0]
                if index >= len(palette) // 3:
                    raise ValueError('PNG palette index out of range')
                alpha = transparency[index] if transparency is not None and index < len(transparency) else 255
                rgba = palette[index * 3:index * 3 + 3] + bytes([alpha])
            else:
                alpha = 0 if transparency is not None and tuple(sample) == transparency else 255
                rgb = bytes([sample[0] * 255 // ((1 << depth) - 1)] * 3) if color == 0 else sample
                rgba = rgb + bytes([alpha])
            pixels[target:target + 4] = rgba
            target += 4
        previous = line
    return width, height, bytes(pixels)

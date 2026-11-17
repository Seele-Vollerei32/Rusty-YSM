import struct
import unittest
import zlib
from rgba import decode_png


def chunk(kind, payload):
    return struct.pack('>I', len(payload)) + kind + payload + struct.pack('>I', zlib.crc32(kind + payload))


def png(width, height, channels, rows, interlace=0):
    color = {3: 2, 4: 6}[channels]
    body = b''.join(b'\x00' + row for row in rows)
    return (b'\x89PNG\r\n\x1a\n' + chunk(b'IHDR', struct.pack('>IIBBBBB', width, height, 8, color, 0, 0, interlace))
            + chunk(b'IDAT', zlib.compress(body)) + chunk(b'IEND', b''))


class PngTests(unittest.TestCase):
    def test_rgb_and_rgba(self):
        self.assertEqual(decode_png(png(2, 1, 3, [b'\xff\x00\x00\x00\xff\x00'])),
                         (2, 1, b'\xff\x00\x00\xff\x00\xff\x00\xff'))
        self.assertEqual(decode_png(png(1, 1, 4, [b'\x01\x02\x03\x00'])), (1, 1, b'\x01\x02\x03\x00'))

    def test_filters_reconstruct_pixels(self):
        # Row 0 straight, rows 1-2 "Up"/"Paeth" encoded as deltas: all decode to row 0.
        first = b'\x10\x20\x30' + b'\x40\x50\x60'
        body = b'\x00' + first + b'\x02' + b'\x00' * 6 + b'\x04' + b'\x00' * 6
        data = (b'\x89PNG\r\n\x1a\n' + chunk(b'IHDR', struct.pack('>IIBBBBB', 2, 3, 8, 2, 0, 0, 0))
                + chunk(b'IDAT', zlib.compress(body)) + chunk(b'IEND', b''))
        width, height, rgba = decode_png(data)
        self.assertEqual((width, height), (2, 3))
        row = b'\x10\x20\x30\xff\x40\x50\x60\xff'
        self.assertEqual(rgba, row * 3)

    def test_rejects_oversized_and_truncated(self):
        with self.assertRaises(ValueError):
            decode_png(png(513, 1, 3, [b'\x00' * 513 * 3]))
        with self.assertRaises(ValueError):
            decode_png(png(1, 1, 3, [b'\x00\x00\x00'])[:-20])
        with self.assertRaises(ValueError):
            decode_png(b'not a png at all')
        with self.assertRaises(ValueError):
            decode_png(png(1, 1, 3, [b'\x00\x00\x00'], interlace=1))

    def test_decompression_budget_and_crc(self):
        data = (b'\x89PNG\r\n\x1a\n'
                + chunk(b'IHDR', struct.pack('>IIBBBBB', 1, 1, 8, 6, 0, 0, 0))
                + chunk(b'IDAT', zlib.compress(bytes(1024 * 1024))) + chunk(b'IEND', b''))
        with self.assertRaisesRegex(ValueError, 'budget'):
            decode_png(data)
        damaged = bytearray(png(1, 1, 4, [bytes(4)]))
        damaged[-1] ^= 1
        with self.assertRaisesRegex(ValueError, 'CRC'):
            decode_png(damaged)

    def test_palette_grayscale_and_transparency(self):
        def image(color, samples, extras=b''):
            return (b'\x89PNG\r\n\x1a\n'
                    + chunk(b'IHDR', struct.pack('>IIBBBBB', 2, 1, 8, color, 0, 0, 0))
                    + extras + chunk(b'IDAT', zlib.compress(b'\x00' + samples))
                    + chunk(b'IEND', b''))
        valid = image(0, bytes([10, 20]))
        with self.assertRaisesRegex(ValueError, 'header must be first'):
            decode_png(valid[:8] + chunk(b'tEXt', b'note') + valid[8:])
        palette = chunk(b'PLTE', bytes([255, 0, 0, 0, 255, 0]))
        self.assertEqual(decode_png(image(3, bytes([0, 1]), palette + chunk(b'tRNS', bytes([0]))))[2],
                         bytes([255, 0, 0, 0, 0, 255, 0, 255]))
        self.assertEqual(decode_png(image(0, bytes([10, 20]), chunk(b'tRNS', struct.pack('>H', 10))))[2],
                         bytes([10, 10, 10, 0, 20, 20, 20, 255]))
        self.assertEqual(decode_png(image(4, bytes([10, 50, 20, 100])))[2],
                         bytes([10, 10, 10, 50, 20, 20, 20, 100]))
        self.assertEqual(decode_png(image(2, bytes([1, 2, 3, 4, 5, 6]),
                         chunk(b'tRNS', struct.pack('>HHH', 1, 2, 3))))[2],
                         bytes([1, 2, 3, 0, 4, 5, 6, 255]))
        for invalid in [image(3, bytes([0, 1])), image(3, bytes([0, 2]), palette),
                        image(6, bytes(8), chunk(b'tRNS', bytes(6))),
                        image(0, bytes(2), chunk(b'tRNS', struct.pack('>H', 256)))]:
            with self.assertRaises(ValueError):
                decode_png(invalid)

    def test_packed_pixels_filters_and_padding(self):
        # Nine pixels cross byte boundaries at every packed depth; padding is nonzero.
        for depth, packed, values in [
                (1, bytes([0xAA, 0xFF]), [1, 0, 1, 0, 1, 0, 1, 0, 1]),
                (2, bytes([0x1B, 0x1B, 0x7F]), [0, 1, 2, 3, 0, 1, 2, 3, 1]),
                (4, bytes([0x01, 0x23, 0x45, 0x67, 0x8F]), list(range(9)))]:
            for color in (0, 3):
                with self.subTest(depth=depth, color=color):
                    maximum = (1 << depth) - 1
                    palette = b''.join(bytes([v, 0, 255 - v]) for v in range(maximum + 1))
                    extras = (chunk(b'PLTE', palette) + chunk(b'tRNS', b'\x00') if color == 3
                              else chunk(b'tRNS', struct.pack('>H', 0)))
                    # Sub-filter the second row using a one-byte left distance.
                    sub = bytes((v - (packed[i - 1] if i else 0)) & 255 for i, v in enumerate(packed))
                    data = (b'\x89PNG\r\n\x1a\n'
                            + chunk(b'IHDR', struct.pack('>IIBBBBB', 9, 2, depth, color, 0, 0, 0))
                            + extras + chunk(b'IDAT', zlib.compress(b'\x00' + packed + b'\x01' + sub))
                            + chunk(b'IEND', b''))
                    expected = b''.join(bytes(([v, 0, 255-v] if color == 3 else [v*255//maximum]*3)
                                             + [0 if v == 0 else 255]) for v in values)
                    self.assertEqual(decode_png(data), (9, 2, expected * 2))

    def test_16bit_samples_and_exact_transparency(self):
        cases = [(0, [0x1234], [0x1235], [18, 18, 18, 0, 18, 18, 18, 255]),
                 (2, [0x1234, 0xABCD, 0xFFFF], [0x1235, 0xABCD, 0xFFFF],
                  [18, 171, 255, 0, 18, 171, 255, 255]),
                 (4, [0x8080, 0x1234], [0xFFFF, 0xFFFF], [128, 128, 128, 18, 255, 255, 255, 255]),
                 (6, [0, 0x8080, 0xFFFF, 0x1234], [0xFFFF]*4, [0, 128, 255, 18, 255, 255, 255, 255])]
        for color, first, second, expected in cases:
            with self.subTest(color=color):
                row = struct.pack('>' + 'H' * (len(first)*2), *(first + second))
                distance = len(first)*2
                filtered = bytes((v - (row[i-distance] if i >= distance else 0)) & 255
                                 for i, v in enumerate(row))
                extras = chunk(b'tRNS', struct.pack('>' + 'H'*len(first), *first)) if color in (0, 2) else b''
                data = (b'\x89PNG\r\n\x1a\n'
                        + chunk(b'IHDR', struct.pack('>IIBBBBB', 2, 1, 16, color, 0, 0, 0))
                        + extras + chunk(b'IDAT', zlib.compress(b'\x01' + filtered)) + chunk(b'IEND', b''))
                self.assertEqual(decode_png(data), (2, 1, bytes(expected)))

    def test_zero_size_rejected(self):
        with self.assertRaises(ValueError):
            decode_png(png(0, 1, 3, []))


if __name__ == '__main__':
    unittest.main()

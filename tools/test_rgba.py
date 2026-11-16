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

    def test_zero_size_rejected(self):
        with self.assertRaises(ValueError):
            decode_png(png(0, 1, 3, []))


if __name__ == '__main__':
    unittest.main()

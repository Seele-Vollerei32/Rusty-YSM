import json
from pathlib import Path
import tempfile
import unittest
from launch import native_environment, pack_environment


class PackSelectionTests(unittest.TestCase):
    def test_animation_aliases(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            (root / 'main.json').write_text('{}')
            (root / 'skin.png').write_bytes(b'fixture')
            animation = json.dumps({'animations': {'idle': {'bones': {}}}})
            (root / 'animation.json').write_text(animation)
            doc = {'files': {'player': {'model': {'main': 'main.json'},
                   'texture': ['skin.png'], 'animation': {
                       'main': 'animation.json', 'alias': './animation.json'}}}}
            (root / 'ysm.json').write_text(json.dumps(doc))
            self.assertEqual(pack_environment(root, clip='idle')['OPENYSM_CLIP'], 'idle')
            (root / 'other.json').write_text(animation)
            doc['files']['player']['animation']['alias'] = 'other.json'
            (root / 'ysm.json').write_text(json.dumps(doc))
            with self.assertRaisesRegex(ValueError, 'found 2'):
                pack_environment(root, clip='idle')

    def test_arm_model_is_optional_and_confined_to_the_pack(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            for name in ['main.json', 'arm.json', 'skin.png']:
                (root / name).write_bytes(b'fixture')
            doc = {'files': {'player': {'model': {'main': 'main.json'}, 'texture': ['skin.png']}}}
            manifest = root / 'ysm.json'
            manifest.write_text(json.dumps(doc))
            self.assertNotIn('OPENYSM_ARM', pack_environment(root))
            doc['files']['player']['model']['arm'] = 'arm.json'
            manifest.write_text(json.dumps(doc))
            self.assertEqual(Path(pack_environment(root)['OPENYSM_ARM']), (root / 'arm.json').resolve())
            for invalid in ['../arm.json', 'missing.json']:
                doc['files']['player']['model']['arm'] = invalid
                manifest.write_text(json.dumps(doc))
                with self.subTest(invalid=invalid), self.assertRaises(ValueError):
                    pack_environment(root)

    def test_main_animation_file_is_passed_without_a_fixed_clip(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            for name in ['main.json', 'skin.png']:
                (root / name).write_bytes(b'fixture')
            (root / 'main.animation.json').write_text(json.dumps({'animations': {'idle': {}}}))
            (root / 'extra.animation.json').write_text(json.dumps({'animations': {'wave': {}}}))
            doc = {'files': {'player': {'model': {'main': 'main.json'}, 'texture': ['skin.png'],
                   'animation': {'extra': 'extra.animation.json'}}}}
            manifest = root / 'ysm.json'
            manifest.write_text(json.dumps(doc))
            # No main file: nothing to drive states from.
            self.assertNotIn('OPENYSM_ANIMATION', pack_environment(root))
            doc['files']['player']['animation']['main'] = 'main.animation.json'
            manifest.write_text(json.dumps(doc))
            env = pack_environment(root)
            self.assertEqual(Path(env['OPENYSM_ANIMATION']), (root / 'main.animation.json').resolve())
            self.assertNotIn('OPENYSM_CLIP', env)
            # A fixed clip still pins its own file.
            env = pack_environment(root, clip='wave')
            self.assertEqual(Path(env['OPENYSM_ANIMATION']), (root / 'extra.animation.json').resolve())
            self.assertEqual(env['OPENYSM_CLIP'], 'wave')
            for invalid in ['../main.animation.json', 'missing.json']:
                doc['files']['player']['animation']['main'] = invalid
                manifest.write_text(json.dumps(doc))
                with self.subTest(invalid=invalid), self.assertRaises(ValueError):
                    pack_environment(root)

    def test_native_environment_names_the_pack_and_sizes_the_slot(self):
        def png(width, height):
            return b'\x89PNG\r\n\x1a\n' + (13).to_bytes(4, 'big') + b'IHDR' + width.to_bytes(4, 'big') + height.to_bytes(4, 'big')
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            (root / 'skin.png').write_bytes(png(256, 512))
            self.assertEqual(native_environment(root), {'OPENYSM_PACK': str(root.resolve())})
            env = native_environment(root, 'skin', 'wave')
            self.assertEqual((env['OPENYSM_PACK_TEXTURE'], env['OPENYSM_CLIP']), ('skin', 'wave'))
            (root / 'large.png').write_bytes(png(64, 1025))
            self.assertEqual(native_environment(root)['OPENYSM_PACK_SLOT'], '2048')
            (root / 'huge.png').write_bytes(png(2049, 8))
            with self.assertRaisesRegex(ValueError, '2048'):
                native_environment(root)
            archive = root / 'pack.zip'
            archive.write_bytes(b'PK')
            self.assertEqual(native_environment(archive), {'OPENYSM_PACK': str(archive.resolve())})
            with self.assertRaisesRegex(ValueError, 'not found'):
                native_environment(root / 'missing')

    def test_selection_and_path_validation(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            for name in ['main.json', 'red.png', 'blue.png']:
                (root / name).write_bytes(b'fixture')
            doc = {'properties': {'default_texture': 'blue'}, 'files': {'player': {
                'model': {'main': 'main.json'}, 'texture': ['red.png', 'blue.png']}}}
            manifest = root / 'ysm.json'
            manifest.write_text(json.dumps(doc))
            self.assertEqual(Path(pack_environment(root)['OPENYSM_TEXTURE_FILE']).name, 'blue.png')
            self.assertEqual(Path(pack_environment(root, 'red')['OPENYSM_TEXTURE_FILE']).name, 'red.png')
            with self.assertRaises(ValueError):
                pack_environment(root, 'missing')
            doc['files']['player']['model']['main'] = '../outside.json'
            manifest.write_text(json.dumps(doc))
            with self.assertRaises(ValueError):
                pack_environment(root)


if __name__ == '__main__':
    unittest.main()

import json
from pathlib import Path
import tempfile
import unittest
from launch import pack_environment


class PackSelectionTests(unittest.TestCase):
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

"""Compile the actual texture hook against a tiny host and verify its input policy."""
from pathlib import Path
import subprocess
import tempfile

ROOT = Path(__file__).resolve().parents[1]
helper = (ROOT / 'mixins/openysm/texture.rs').read_text()
fixture = r'''
mod hud {
    pub fn decode_bytes(data: &[u8]) -> Result<(u32,u32,Vec<u8>),String> {
        Ok((u32::from_be_bytes(data[16..20].try_into().unwrap()),
            u32::from_be_bytes(data[20..24].try_into().unwrap()), vec![255;16]))
    }
}
fn main() {
    let mut images = vec![("existing".into(),1,1,vec![255;4])];
    texture(&mut images);
    let expected:usize = std::env::args().nth(1).unwrap().parse().unwrap();
    assert_eq!(images.len(),expected);
    if expected == 2 {
        let name = std::env::args().nth(2).unwrap();
        assert_eq!(images[1].0,name);
        if name == "openysm/pack" {
            let size: u32 = std::env::args().nth(3).unwrap().parse().unwrap();
            assert_eq!((images[1].1,images[1].2,images[1].3.len()),(size,size,(size*size*4) as usize));
        }
    }
}
'''
with tempfile.TemporaryDirectory() as directory:
    import os
    root = Path(directory)
    source = root / 'fixture.rs'
    source.write_text(helper + fixture)
    exe = root / ('fixture.exe' if os.name == 'nt' else 'fixture')
    subprocess.run(['rustc', '+1.95.0', '--edition=2024', str(source), '-o', str(exe)], check=True)
    clean = {key: value for key, value in os.environ.items()
             if not key.startswith('OPENYSM_') and key != 'SPM_BRIDGE_DIR'}
    for width, height, expected in [(2, 2, 2), (0, 2, 1), (4097, 2, 1)]:
        png = root / 'input.png'
        png.write_bytes(b'\x89PNG\r\n\x1a\n' + (13).to_bytes(4, 'big') + b'IHDR' +
                        width.to_bytes(4, 'big') + height.to_bytes(4, 'big'))
        subprocess.run([str(exe), str(expected), 'openysm/model'], env={**clean, 'OPENYSM_TEXTURE_FILE': str(png)}, check=True)
    # The pack slot: reserved for a pack or an explicit size, bounded to 16..2048.
    for variables, expected, size in [({}, 1, 0), ({'OPENYSM_PACK': 'unused'}, 2, 1024),
                                      ({'OPENYSM_PACK_SLOT': '64'}, 2, 64), ({'OPENYSM_PACK_SLOT': '1'}, 2, 16),
                                      ({'OPENYSM_PACK': 'unused', 'OPENYSM_PACK_SLOT': '99999'}, 2, 2048),
                                      ({'OPENYSM_PACK_SLOT': 'large'}, 1, 0)]:
        subprocess.run([str(exe), str(expected), 'openysm/pack', str(size)], env={**clean, **variables}, check=True)
print('texture hook registration and dimension bounds: OK (decoder mocked)')

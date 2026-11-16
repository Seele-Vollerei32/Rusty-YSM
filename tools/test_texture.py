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
    if expected == 2 { assert_eq!(images[1].0,"openysm/model"); }
}
'''
with tempfile.TemporaryDirectory() as directory:
    import os
    root = Path(directory)
    source = root / 'fixture.rs'
    source.write_text(helper + fixture)
    exe = root / ('fixture.exe' if os.name == 'nt' else 'fixture')
    subprocess.run(['rustc', '+1.95.0', '--edition=2024', str(source), '-o', str(exe)], check=True)
    for width, height, expected in [(2, 2, 2), (0, 2, 1), (4097, 2, 1)]:
        png = root / 'input.png'
        png.write_bytes(b'\x89PNG\r\n\x1a\n' + (13).to_bytes(4, 'big') + b'IHDR' +
                        width.to_bytes(4, 'big') + height.to_bytes(4, 'big'))
        subprocess.run([str(exe), str(expected)], env={**os.environ, 'OPENYSM_TEXTURE_FILE': str(png)}, check=True)
print('texture hook registration and dimension bounds: OK (decoder mocked)')

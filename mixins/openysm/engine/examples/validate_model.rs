fn main() {
    let mut args = std::env::args().skip(1);
    let path = args.next().expect("usage: validate_model geometry.json");
    let bytes = std::fs::read(path).expect("read model");
    let mut model = openysm_runtime::load_geometry(&bytes).expect("parse model");
    model.pose(|b| (b.rotation, [0.; 3], [1.; 3], false));
    let bones = model.bones.len();
    let vertices = model.render(&openysm_runtime::IDENTITY, 0xf0f0, 0);
    assert!(!vertices.is_empty());
    assert!(vertices.iter().all(|v| {
        v.position
            .iter()
            .chain(v.normal.iter())
            .all(|x| x.is_finite())
    }));
    println!(
        "{bones} bones, {} vertices, C++ skinning OK",
        vertices.len()
    );
}

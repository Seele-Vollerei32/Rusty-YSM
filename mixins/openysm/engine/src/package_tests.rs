use crate::package::*;
use std::io::{Cursor, Write};
fn archive(files: &[(&str, &[u8])]) -> Vec<u8> {
    let mut zip = zip::ZipWriter::new(Cursor::new(Vec::new()));
    for (name, bytes) in files {
        zip.start_file(
            *name,
            zip::write::SimpleFileOptions::default()
                .compression_method(zip::CompressionMethod::Stored),
        )
        .unwrap();
        zip.write_all(bytes).unwrap();
    }
    zip.finish().unwrap().into_inner()
}
const MANIFEST: &[u8] = br#"{"files":{"player":{"model":{"main":"main.json"},"texture":{"uv":"skin.png"}},"projectiles":[{"match":["minecraft:arrow"],"model":"arrow.json"}]}}"#;
#[test]
fn nested_pack_resolves_only_resource_fields() {
    let data = archive(&[
        ("pack/ysm.json", MANIFEST),
        ("pack/main.json", b"{}"),
        ("pack/skin.png", b"png"),
        ("pack/arrow.json", b"{}"),
    ]);
    let package = ModelPackage::from_bytes(&data, PackageLimits::default()).unwrap();
    assert_eq!(package.read("skin.png").unwrap(), b"png");
    assert_eq!(package.references().len(), 3);
    assert!(!package.references().contains("minecraft:arrow"));
}
#[test]
fn paths_collisions_and_missing_references_fail() {
    for name in [
        "../main.json",
        "a/../../main.json",
        "C:/main.json",
        "/main.json",
        "a\\..\\main.json",
        "CON.txt",
        "a./file",
        "a//b",
    ] {
        assert!(
            normalize_resource_path(name, PackageLimits::default()).is_err(),
            "{name}"
        );
    }
    let missing = archive(&[("ysm.json", MANIFEST)]);
    assert!(
        ModelPackage::from_bytes(&missing, PackageLimits::default())
            .unwrap_err()
            .contains("missing")
    );
    let collision = archive(&[
        ("main.json", b"{}"),
        ("MAIN.json", b"{}"),
        ("arm.json", b"{}"),
    ]);
    assert!(
        ModelPackage::from_bytes(&collision, PackageLimits::default())
            .unwrap_err()
            .contains("duplicate")
    );
}
#[test]
fn archive_limits_and_binary_format_fail_explicitly() {
    let data = archive(&[("main.json", b"123456"), ("arm.json", b"{}")]);
    assert!(
        ModelPackage::from_bytes(
            &data,
            PackageLimits {
                max_entry_bytes: 5,
                ..Default::default()
            }
        )
        .is_err()
    );
    assert!(
        ModelPackage::from_bytes(
            &data,
            PackageLimits {
                max_total_bytes: 7,
                ..Default::default()
            }
        )
        .is_err()
    );
    assert!(
        ModelPackage::from_bytes(
            &data,
            PackageLimits {
                max_entries: 1,
                ..Default::default()
            }
        )
        .is_err()
    );
    assert!(
        ModelPackage::from_bytes(
            &data,
            PackageLimits {
                max_source_bytes: 1,
                ..Default::default()
            }
        )
        .is_err()
    );
    assert!(
        ModelPackage::from_bytes(b"YSM encrypted", PackageLimits::default())
            .unwrap_err()
            .contains("unsupported")
    );
    assert!(ModelPackage::from_bytes(&data[..data.len() / 2], PackageLimits::default()).is_err());
    assert!(
        ModelPackage::from_bytes(&data, PackageLimits::default())
            .unwrap()
            .read("main.json")
            .is_ok()
    );
}
#[test]
fn directory_and_archive_have_identical_resources() {
    let scratch =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../target/package-tests");
    std::fs::create_dir_all(&scratch).unwrap();
    let root = scratch.join(format!(
        "openysm-package-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir(&root).unwrap();
    let files = [
        ("ysm.json", MANIFEST),
        ("main.json", b"{}".as_slice()),
        ("skin.png", b"png".as_slice()),
        ("arrow.json", b"{}".as_slice()),
    ];
    for (name, data) in files {
        std::fs::write(root.join(name), data).unwrap();
    }
    let directory = ModelPackage::open(&root, PackageLimits::default());
    std::fs::remove_dir_all(&root).unwrap();
    let archive = ModelPackage::from_bytes(&archive(&files), PackageLimits::default()).unwrap();
    let directory = directory.unwrap();
    assert_eq!(directory.entries(), archive.entries());
    assert_eq!(directory.manifest(), archive.manifest());
}
#[test]
fn corrupted_entry_and_ambiguous_roots_are_rejected() {
    let mut data = archive(&[("main.json", b"unique-model-payload"), ("arm.json", b"{}")]);
    // Default ZIP options use stored bytes; alter payload without updating its CRC.
    let offset = data
        .windows(b"unique-model-payload".len())
        .position(|w| w == b"unique-model-payload")
        .unwrap();
    data[offset] ^= 1;
    assert!(ModelPackage::from_bytes(&data, PackageLimits::default()).is_err());
    let roots = archive(&[("a/ysm.json", b"{}"), ("b/ysm.json", b"{}")]);
    assert!(
        ModelPackage::from_bytes(&roots, PackageLimits::default())
            .unwrap_err()
            .contains("ambiguous")
    );
}

fn texture(images: &mut Vec<(String, u32, u32, Vec<u8>)>) {
    if std::env::var_os("SPM_BRIDGE_DIR").is_some() {
        for i in 0..16 {
            images.push((format!("openysm/cloud{i}"),512,512,vec![0;512*512*4]));
        }
    }
    let Some(path) = std::env::var_os("OPENYSM_TEXTURE_FILE") else { return; };
    let result = (|| -> std::result::Result<(u32,u32,Vec<u8>),String> {
        let path = std::path::Path::new(&path);
        let metadata = std::fs::metadata(path).map_err(|e|e.to_string())?;
        if metadata.len() > 32 * 1024 * 1024 { return Err("texture file exceeds 32 MiB".into()); }
        let bytes = std::fs::read(path).map_err(|e|e.to_string())?;
        // Bound dimensions before decode allocates the RGBA image.
        if bytes.len() < 24 || &bytes[..8] != b"\x89PNG\r\n\x1a\n" || &bytes[12..16] != b"IHDR" {
            return Err("expected PNG IHDR".into());
        }
        let width = u32::from_be_bytes(bytes[16..20].try_into().unwrap());
        let height = u32::from_be_bytes(bytes[20..24].try_into().unwrap());
        if width == 0 || height == 0 || width > 4096 || height > 4096 {
            return Err("texture dimensions must be within 1..4096".into());
        }
        crate::hud::decode_bytes(&bytes).map_err(|e|e.to_string())
    })();
    match result {
        Ok((w,h,pixels)) => {
            images.retain(|image|image.0 != "openysm/model");
            images.push(("openysm/model".into(),w,h,pixels));
        }
        Err(error) => eprintln!("[openysm] texture load failed: {error}"),
    }
}

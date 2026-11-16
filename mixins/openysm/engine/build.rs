fn main() {
    println!("cargo:rerun-if-changed=native/renderer.cpp");
    println!("cargo:rerun-if-changed=native/renderer.h");
    println!("cargo:rerun-if-changed=native/sse2neon.h");
    cc::Build::new()
        .cpp(true)
        .std("c++17")
        .file("native/renderer.cpp")
        .compile("openysm");
}

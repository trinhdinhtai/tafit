//! Gives tafit.exe its icon on Windows. GPUI also loads the window icon from
//! this resource (id 1). A no-op when building for other platforms.

fn main() {
    println!("cargo:rerun-if-changed=tafit.rc");
    println!("cargo:rerun-if-changed=../../assets/tafit.ico");
    embed_resource::compile("tafit.rc", embed_resource::NONE)
        .manifest_optional()
        .expect("compile the Windows icon resource");
}

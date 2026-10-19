//! Emits the module definition file so the three proxy exports stay undecorated on
//! i686 (review Y1: `#[export_name]` alone may still decorate; `.def` is primary).

fn main() {
    let def = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("exports.def");
    assert!(
        !def.to_string_lossy().contains(' '),
        "link.exe needs a quote-free /DEF path"
    );
    println!("cargo:rerun-if-changed={}", def.display());
    println!("cargo:rustc-cdylib-link-arg=/DEF:{}", def.display());
}

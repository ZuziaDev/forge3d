//! Embeds the forge3d icon and version information into `forge3d.exe`.
//! Only does anything for Windows targets; works when cross-compiling with
//! mingw (`x86_64-w64-mingw32-windres`).

use std::path::PathBuf;

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=../../assets/icon.ico");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }
    let manifest = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR"));
    let out = PathBuf::from(std::env::var("OUT_DIR").expect("OUT_DIR"));
    let icon = manifest
        .join("../../assets/icon.ico")
        .canonicalize()
        .expect("assets/icon.ico is missing");
    let version = std::env::var("CARGO_PKG_VERSION").expect("CARGO_PKG_VERSION");
    let mut parts = version
        .split(|c: char| !c.is_ascii_digit())
        .filter_map(|p| p.parse::<u16>().ok());
    let (major, minor, patch) = (
        parts.next().unwrap_or(0),
        parts.next().unwrap_or(0),
        parts.next().unwrap_or(0),
    );
    let rc = format!(
        r#"1 ICON "{icon}"
1 VERSIONINFO
FILEVERSION {major},{minor},{patch},0
PRODUCTVERSION {major},{minor},{patch},0
FILEOS 0x40004
FILETYPE 0x1
BEGIN
  BLOCK "StringFileInfo"
  BEGIN
    BLOCK "040904B0"
    BEGIN
      VALUE "CompanyName", "ZuziaDev\0"
      VALUE "FileDescription", "forge3d - 3D modeler with built-in MCP server\0"
      VALUE "FileVersion", "{version}\0"
      VALUE "InternalName", "forge3d\0"
      VALUE "LegalCopyright", "Copyright (C) ZuziaDev. MIT OR Apache-2.0\0"
      VALUE "OriginalFilename", "forge3d.exe\0"
      VALUE "ProductName", "forge3d\0"
      VALUE "ProductVersion", "{version}\0"
    END
  END
  BLOCK "VarFileInfo"
  BEGIN
    VALUE "Translation", 0x409, 1200
  END
END
"#,
        icon = icon.display().to_string().replace('\\', "/"),
    );
    let rc_path = out.join("forge3d.rc");
    std::fs::write(&rc_path, rc).expect("writing forge3d.rc");
    embed_resource::compile(&rc_path, embed_resource::NONE)
        .manifest_required()
        .expect("embedding Windows resources failed");
}

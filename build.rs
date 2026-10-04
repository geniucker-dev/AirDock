// SPDX-License-Identifier: MPL-2.0
use std::{env, path::PathBuf, process::Command};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=rust/assets/icons/airdock.ico");
    if env::var("CARGO_CFG_TARGET_OS")? != "windows" {
        return Ok(());
    }
    let output = PathBuf::from(env::var_os("OUT_DIR").ok_or("missing OUT_DIR")?);
    let icon =
        PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").ok_or("missing manifest directory")?)
            .join("rust/assets/icons/airdock.ico");
    let version = env::var("CARGO_PKG_VERSION")?;
    let numeric = format!("{},0", version.replace('.', ","));
    let icon = icon
        .to_str()
        .ok_or("icon path is not valid Unicode")?
        .replace('\\', "\\\\");
    let rc = output.join("airdock.rc");
    std::fs::write(
        &rc,
        format!(
            r#"#pragma code_page(65001)
#include <windows.h>
1 ICON "{icon}"
1 VERSIONINFO
FILEVERSION {numeric}
PRODUCTVERSION {numeric}
FILEFLAGSMASK 0x3fL
FILEFLAGS 0
FILEOS 0x40004L
FILETYPE 1
FILESUBTYPE 0
BEGIN
  BLOCK "StringFileInfo"
  BEGIN
    BLOCK "040904b0"
    BEGIN
      VALUE "CompanyName", "geniucker-dev\0"
      VALUE "FileDescription", "AirDock AirPlay receiver\0"
      VALUE "FileVersion", "{version}\0"
      VALUE "InternalName", "airdock\0"
      VALUE "OriginalFilename", "airdock.exe\0"
      VALUE "ProductName", "AirDock\0"
      VALUE "ProductVersion", "{version}\0"
    END
  END
  BLOCK "VarFileInfo"
  BEGIN
    VALUE "Translation", 0x0409, 1200
  END
END
"#
        ),
    )?;
    let msvc = env::var("CARGO_CFG_TARGET_ENV")? == "msvc";
    let resource = output.join(if msvc { "airdock.res" } else { "airdock.o" });
    let status = if msvc {
        Command::new("rc.exe")
            .arg("/nologo")
            .arg("/fo")
            .arg(&resource)
            .arg(&rc)
            .status()?
    } else {
        Command::new(env::var_os("WINDRES").unwrap_or_else(|| "windres".into()))
            .arg("-i")
            .arg(&rc)
            .arg("-o")
            .arg(&resource)
            .arg("-O")
            .arg("coff")
            .status()?
    };
    if !status.success() {
        return Err("Windows icon/version resource compilation failed".into());
    }
    println!("cargo:rustc-link-arg-bin=airdock={}", resource.display());
    Ok(())
}

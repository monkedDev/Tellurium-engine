use std::{env, fs, io, path::PathBuf};

use flate2::read::GzDecoder;

fn main() -> io::Result<()> {
    let archive = "../third-party/miniaudio-0.11.23.h.gz";
    let out = PathBuf::from(env::var_os("OUT_DIR").expect("Cargo sets OUT_DIR"));
    io::copy(
        &mut GzDecoder::new(fs::File::open(archive)?),
        &mut fs::File::create(out.join("miniaudio.h"))?,
    )?;
    cc::Build::new()
        .include(&out)
        .include("include")
        .file("src/miniaudio.c")
        .warnings(false)
        .compile("tellurium_miniaudio_native");

    let target = env::var("CARGO_CFG_TARGET_OS").expect("Cargo sets target OS");
    let libraries: &[&str] = match target.as_str() {
        "windows" => &["ole32", "winmm"],
        "linux" => &["dl", "pthread", "m"],
        "macos" => &[
            "framework=CoreAudio",
            "framework=AudioToolbox",
            "framework=CoreFoundation",
        ],
        _ => &[],
    };
    for library in libraries {
        println!("cargo:rustc-link-lib={library}");
    }
    println!("cargo:include={}", out.display());
    for path in [
        archive,
        "include/tellurium/miniaudio_config.h",
        "src/miniaudio.c",
    ] {
        println!("cargo:rerun-if-changed={path}");
    }
    Ok(())
}

use std::{env, fs, path::PathBuf};

fn main() {
    let graphics = "../graphics-cpp";
    let mut build = cxx_build::bridge("src/ffi.rs");
    build
        .include("include")
        .include(env::var_os("DEP_TELLURIUM_BULLET_INCLUDE").expect("Bullet include path"))
        .include(env::var_os("DEP_TELLURIUM_MINIAUDIO_INCLUDE").expect("miniaudio include path"))
        .include("../audio-cpp/include")
        .include(format!("{graphics}/include"))
        .std("c++17")
        .file("../physics-cpp/src/bullet_world.cpp")
        .file("../audio-cpp/src/audio_device.cpp")
        .file("src/physics.cpp");
    let tracy = "../third-party/tracy-client-0.12.2.tar.gz";
    if env::var_os("CARGO_FEATURE_PROFILING").is_some() {
        let out = PathBuf::from(env::var_os("OUT_DIR").expect("Cargo sets OUT_DIR"));
        let archive = fs::File::open(tracy).expect("open vendored Tracy client");
        tar::Archive::new(flate2::read::GzDecoder::new(archive))
            .unpack(&out)
            .expect("extract vendored Tracy client");
        let public = out.join("public");
        build
            .include(&public)
            .file(public.join("TracyClient.cpp"))
            .file("src/profile.cpp")
            .define("TRACY_ENABLE", None)
            .define("TRACY_ON_DEMAND", None)
            .define("TRACY_ONLY_LOCALHOST", None)
            .define("TRACY_NO_BROADCAST", None)
            .define("TRACY_NO_SYSTEM_TRACING", None)
            .define("TRACY_NO_CALLSTACK", None);
        if env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
            for library in ["ws2_32", "dbghelp", "advapi32", "user32"] {
                println!("cargo:rustc-link-lib={library}");
            }
        }
    }
    println!("cargo:rerun-if-changed={tracy}");

    if env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        let shader = format!("{graphics}/shaders/mesh.hlsl");
        let out = PathBuf::from(env::var_os("OUT_DIR").expect("Cargo sets OUT_DIR"));
        let source = fs::read_to_string(&shader).expect("read mesh shader");
        fs::write(
            out.join("mesh_shader.hpp"),
            format!("inline constexpr char mesh_shader[] = R\"HLSL({source})HLSL\";\n"),
        )
        .expect("embed mesh shader");
        build.include(&out).file(format!("{graphics}/src/dx12.cpp"));
        for library in ["d3d12", "dxgi", "d3dcompiler", "user32"] {
            println!("cargo:rustc-link-lib={library}");
        }
        println!("cargo:rerun-if-changed={shader}");
    } else {
        build.file(format!("{graphics}/src/unsupported.cpp"));
    }

    build.compile("tellurium_bridge");
    for path in [
        "src",
        "include",
        graphics,
        "../physics-cpp/src/bullet_world.cpp",
        "../audio-cpp",
    ] {
        println!("cargo:rerun-if-changed={path}");
    }
}

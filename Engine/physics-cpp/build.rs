use std::{env, fs, io, path::Path};

use flate2::read::GzDecoder;

fn sources(directory: &Path, output: &mut Vec<std::path::PathBuf>) -> io::Result<()> {
    for entry in fs::read_dir(directory)? {
        let path = entry?.path();
        if path.is_dir() {
            sources(&path, output)?;
        } else if path.extension().is_some_and(|ext| ext == "cpp") {
            output.push(path);
        }
    }
    Ok(())
}

fn main() -> io::Result<()> {
    let source = "../third-party/bullet-cpu-3.25.tar.gz";
    let root = std::path::PathBuf::from(env::var_os("OUT_DIR").expect("Cargo sets OUT_DIR"));
    tar::Archive::new(GzDecoder::new(fs::File::open(source)?)).unpack(&root)?;
    let include = root.join("src");

    let mut files = Vec::new();
    for library in ["LinearMath", "BulletCollision", "BulletDynamics"] {
        sources(&include.join(library), &mut files)?;
    }
    files.sort();
    cc::Build::new()
        .cpp(true)
        .std("c++17")
        .include(&include)
        .warnings(false)
        .files(files)
        .compile("tellurium_bullet_cpu");

    println!("cargo:include={}", include.display());
    println!("cargo:rerun-if-changed={source}");
    Ok(())
}

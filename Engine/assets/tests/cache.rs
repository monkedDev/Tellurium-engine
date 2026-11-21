use std::fs;
use std::io;

use tellurium_assets::{Assets, AudioClip, Handle, Mesh, ScriptAsset, Texture};

fn in_cache<T>(handle: &Handle<T>) {
    assert!(handle.cache_path().is_file());
    assert_eq!(
        handle.cache_path().parent().unwrap().file_name().unwrap(),
        ".telcache"
    );
}

#[test]
fn imports_demo_sources_reuses_live_handles_and_recovers_damaged_cache() -> io::Result<()> {
    let sandbox = tempfile::Builder::new()
        .prefix("tellurium-assets-")
        .tempdir_in(env!("CARGO_TARGET_TMPDIR"))?;
    let fixtures = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../runtime/demo");
    for name in [
        "grid.png",
        "box.fbx",
        "impact.wav",
        "orbit.ogg",
        "physics.luau",
    ] {
        fs::copy(fixtures.join(name), sandbox.path().join(name))?;
    }
    let assets = Assets::new(sandbox.path())?;
    let texture = assets.request::<Texture>("grid.png")?;
    let duplicate = assets.request::<Texture>("grid.png")?;
    let mesh = assets.request::<Mesh>("box.fbx")?;
    let wav = assets.request::<AudioClip>("impact.wav")?;
    let ogg = assets.request::<AudioClip>("orbit.ogg")?;
    let script = assets.request::<ScriptAsset>("physics.luau")?;
    let texture = texture.wait()?;
    let duplicate = duplicate.wait()?;
    let mesh = mesh.wait()?;
    let wav = wav.wait()?;
    let ogg = ogg.wait()?;
    let script = script.wait()?;
    assert!(std::ptr::eq(&*texture, &*duplicate));
    assert!(assets.request::<Mesh>("grid.png")?.wait().is_err());
    assert_eq!((texture.width, texture.height), (64, 64));
    assert_eq!((mesh.vertices.len(), mesh.indices.len()), (24, 36));
    assert!(wav.samples.len() > 1000 && ogg.samples.len() > 1000);
    assert!(!script.bytecode().is_empty());
    for name in [
        "grid.png",
        "box.fbx",
        "impact.wav",
        "orbit.ogg",
        "physics.luau",
    ] {
        assert!(sandbox.path().join(format!("{name}.telmeta")).is_file());
    }
    in_cache(&texture);
    in_cache(&mesh);
    in_cache(&wav);
    in_cache(&ogg);
    in_cache(&script);

    let path = texture.cache_path().to_owned();
    let revision = texture.revision();
    let expected = texture.rgba8.clone();
    drop((texture, duplicate));
    fs::write(&path, b"invalid cache")?;
    let recovered = assets.request::<Texture>("grid.png")?.wait()?;
    assert_eq!(recovered.revision(), revision);
    assert_eq!(recovered.rgba8, expected);
    assert_eq!(recovered.cache_path(), path);

    let meta = sandbox.path().join("grid.png.telmeta");
    let data = fs::read_to_string(&meta)?.replace("flip_vertical = false", "flip_vertical = true");
    fs::write(&meta, data)?;
    let flipped = assets.request::<Texture>("grid.png")?.wait()?;
    assert_eq!(flipped.revision().id, revision.id);
    assert_ne!(flipped.revision().digest, revision.digest);
    assert_ne!(flipped.rgba8, expected);
    Ok(())
}

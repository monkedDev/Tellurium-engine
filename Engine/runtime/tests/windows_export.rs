#![cfg(all(windows, feature = "editor"))]

use std::fs::{self, File};
use std::io::Write;
use std::os::windows::ffi::OsStrExt;
use std::path::Path;
use std::process::{Child, Command};
use std::thread;
use std::time::{Duration, Instant};

use tellurium_builder::export_windows;
use tellurium_serialization::{
    AudioSourceData, BodyType, ColliderData, ColliderShape, DocumentFormat, Node, NodeKind,
    PackedScene, Project, ProjectConfig, RigidbodyData, ScriptData, WorldSnapshot,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{FindWindowW, PostMessageW, WM_CLOSE};

struct Game(Child);

impl Drop for Game {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

#[test]
#[ignore = "launches the real DirectX 12 game on an interactive Windows desktop"]
fn windows_export_runs_without_the_project_or_toolchain() -> Result<(), Box<dyn std::error::Error>>
{
    let sandbox = tempfile::tempdir_in(env!("CARGO_TARGET_TMPDIR"))?;
    let root = sandbox.path().join("source");
    let output = sandbox.path().join("release");
    fs::create_dir_all(root.join("assets"))?;
    fs::create_dir_all(root.join("scenes"))?;
    fs::create_dir(root.join("plugins"))?;
    fs::create_dir(&output)?;
    let fixtures = Path::new(env!("CARGO_MANIFEST_DIR")).join("demo");
    for name in ["grid.png", "box.fbx", "impact.wav", "orbit.ogg"] {
        fs::copy(fixtures.join(name), root.join("assets").join(name))?;
    }
    let script = r#"
-- This source-only comment must be stripped.
local frames = 0
function on_update()
    frames += 1
    transform.set_position(frames, 2, 3)
    if frames == 2 then
        error("TELLURIUM_PACKAGED_FRAME_OK")
    end
end
"#;
    fs::write(root.join("assets/start.luau"), script)?;
    fs::write(root.join("outside_assets.luau"), "return 42")?;
    // A malformed editor plugin must never reach the game's compiler or package.
    fs::write(root.join("plugins/editor_only.luau"), "local =")?;
    let name = format!("Тест экспорта {}", std::process::id());
    let mut config = ProjectConfig {
        name: name.clone(),
        ..Default::default()
    };
    config.export.console = true;
    config.export.icon = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../assets/icons/tellurium-editor.ico")
        .to_string_lossy()
        .into_owned();
    let project = Project::new(root.join("test.tprj"), config)?;
    project.save()?;
    fs::OpenOptions::new()
        .append(true)
        .open(&project.file)?
        .write_all(&fs::read(fixtures.join("project.toml"))?)?;
    let mut scene = PackedScene::default();
    let mut cube = Node::new("Cube", NodeKind::MeshInstance);
    let mesh = cube.mesh_renderer.as_mut().unwrap();
    mesh.mesh = Some("assets\\box.fbx".into());
    mesh.texture = Some("assets/grid.png".into());
    cube.script = Some(ScriptData {
        path: "assets/start.luau".into(),
        state: None,
    });
    let cube = scene.insert(cube, None)?;
    for clip in ["impact.wav", "orbit.ogg"] {
        let mut audio = Node::new(clip, NodeKind::AudioStreamPlayer);
        audio.audio_source = Some(AudioSourceData {
            clip: format!("assets/{clip}"),
            volume: 0.0,
            looping: true,
            spatial: true,
        });
        scene.insert(audio, None)?;
    }
    let mut body = Node::new("Falling body", NodeKind::Node3D);
    body.transform.position[1] = 2.0;
    body.rigidbody = Some(RigidbodyData {
        kind: BodyType::Dynamic,
        mass: 1.0,
        linear_velocity: [0.0; 3],
    });
    body.collider = Some(ColliderData {
        shape: ColliderShape::Box,
        dimensions: [0.5; 3],
        trigger: false,
    });
    scene.insert(body, None)?;
    let mut listener = Node::new("Listener", NodeKind::Node3D);
    listener.audio_listener = true;
    scene.insert(listener, None)?;
    scene.save(&root.join("scenes/main.telsc"), DocumentFormat::Text)?;
    PackedScene::default().save(&root.join("scenes/extra.telsc"), DocumentFormat::Binary)?;

    let source = root.join("assets/start.luau");
    fs::write(&source, "local =")?;
    let error = export_windows(&project, &output).unwrap_err();
    assert!(error.to_string().contains("assets/start.luau"), "{error}");
    assert_eq!(fs::read_dir(&output)?.count(), 0);
    fs::write(&source, script)?;

    let project = Project::load(&project.file)?;
    let exe = export_windows(&project, &output)?;
    let archive = exe.with_extension("telpak");
    let packed = fs::read(&archive)?;
    for path in [
        "scenes/main.telsc",
        "scenes/extra.telsc",
        "assets/grid.png",
        "assets/box.fbx",
        "assets/impact.wav",
        "assets/orbit.ogg",
        "assets/start.luau",
        "outside_assets.luau",
        "GameplayContext",
    ] {
        assert!(
            packed
                .windows(path.len())
                .any(|bytes| bytes == path.as_bytes()),
            "{path}"
        );
    }
    let comment = b"This source-only comment must be stripped.";
    assert!(!packed.windows(comment.len()).any(|bytes| bytes == comment));
    let plugin = b"plugins/editor_only.luau";
    assert!(!packed.windows(plugin.len()).any(|bytes| bytes == plugin));
    for entry in fs::read_dir(exe.parent().unwrap())? {
        let path = entry?.path();
        assert!(path.is_file());
        assert!(matches!(
            path.extension().and_then(|ext| ext.to_str()),
            Some("exe" | "telpak" | "dll" | "txt")
        ));
    }
    assert_ne!(
        export_windows(&project, &output)?,
        exe,
        "repeat exports use separate folders"
    );
    fs::rename(&root, sandbox.path().join("unavailable"))?;

    let snapshot = sandbox.path().join("play.telsave");
    let errors = sandbox.path().join("stderr.txt");
    let clean = |exe: &Path| {
        let mut command = Command::new(exe);
        command.current_dir(sandbox.path()).env(
            "PATH",
            std::env::var_os("SystemRoot")
                .map(|path| std::path::PathBuf::from(path).join("System32"))
                .unwrap_or_default(),
        );
        command
    };
    let mut game = Game(
        clean(&exe)
            .env("RUST_LOG", "error")
            .arg("--save")
            .arg(&snapshot)
            .stderr(File::create(&errors)?)
            .spawn()?,
    );
    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        let stderr = fs::read_to_string(&errors)?;
        assert!(
            game.0.try_wait()?.is_none(),
            "exported game stopped early: {stderr}"
        );
        if stderr.contains("TELLURIUM_PACKAGED_FRAME_OK") {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "packaged script did not run: {stderr}"
        );
        thread::sleep(Duration::from_millis(50));
    }
    let title: Vec<u16> = std::ffi::OsStr::new(&name)
        .encode_wide()
        .chain(Some(0))
        .collect();
    // SAFETY: The title buffer is live and NUL-terminated. WM_CLOSE takes no pointers.
    unsafe {
        let window = FindWindowW(std::ptr::null(), title.as_ptr());
        assert!(!window.is_null());
        assert_ne!(PostMessageW(window, WM_CLOSE, 0, 0), 0);
    }
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        if let Some(status) = game.0.try_wait()? {
            assert!(status.success(), "{}", fs::read_to_string(&errors)?);
            break;
        }
        assert!(Instant::now() < deadline, "game did not close");
        thread::sleep(Duration::from_millis(50));
    }
    let saved = WorldSnapshot::load(&snapshot)?;
    assert!(saved.scene.node(cube).unwrap().transform.position[0] >= 2.0);

    fs::OpenOptions::new()
        .append(true)
        .open(&archive)?
        .write_all(b"changed")?;
    let rejected = clean(&exe).output()?;
    assert!(!rejected.status.success());
    assert!(String::from_utf8_lossy(&rejected.stderr).contains("does not match the executable"));
    Ok(())
}

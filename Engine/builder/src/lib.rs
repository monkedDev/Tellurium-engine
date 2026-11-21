//! Bake a Windows distribution from the standalone runtime template.

#![cfg(windows)]

mod executable;

use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, File, OpenOptions};
use std::io::{self, BufWriter};
use std::path::{Path, PathBuf};

use tellurium_assets::package::{self, Kind, Writer, CONFIG};
use tellurium_assets::{Asset, Assets, AudioClip, Mesh, ScriptAsset, Texture};
use tellurium_input::ActionMap;
use tellurium_serialization::{
    is_project_file, DocumentFormat, GraphicsBackend, PackedScene, Project,
};

const TEMPLATE: &str = "tellurium_runtime.exe";

/// Write into a private staging directory, then publish the complete distribution.
pub fn export_windows(project: &Project, output: &Path) -> io::Result<PathBuf> {
    if !is_project_file(&project.file) || !fs::metadata(&project.file)?.is_file() {
        return Err(invalid(
            "Save the project as project.toml or .tprj before export",
        ));
    }
    if project.config.graphics_backend != GraphicsBackend::Dx12 {
        return Err(invalid("The Windows runtime template supports DirectX 12"));
    }
    let config = project.export_document()?;
    ActionMap::from_project_toml(&config).map_err(invalid)?;
    let template = template_directory()?;
    let output = output.canonicalize()?;
    let stage = tempfile::Builder::new()
        .prefix(".tellurium-export-")
        .tempdir_in(&output)?;
    let name = game_name(&project.config.name);
    let exe_name = format!("{name}.exe");
    let exe = stage.path().join(&exe_name);
    let archive = stage.path().join(format!("{name}.telpak"));
    let mut writer = Writer::new(BufWriter::new(File::create(&archive)?))?;
    writer.entry(Kind::Project, CONFIG, config.as_bytes())?;
    bake(project, &mut writer)?;
    let digest = writer.finish()?;

    fs::copy(template.join(TEMPLATE), &exe)?;
    if !project.config.export.icon.is_empty() {
        let icon = Path::new(&project.config.export.icon);
        let icon = if icon.is_absolute() {
            icon.to_owned()
        } else {
            project.path(&project.config.export.icon)?
        };
        executable::set_icon(&exe, &icon)?;
    }
    executable::set_console(&exe, project.config.export.console)?;
    package::write_binding(OpenOptions::new().append(true).open(&exe)?, digest)?;
    for dependency in fs::read_dir(template.join("redistributables"))? {
        let dependency = dependency?;
        let path = dependency.path();
        if dependency.file_type()?.is_file()
            && path.extension().is_some_and(|ext| {
                ext.eq_ignore_ascii_case("dll") || ext.eq_ignore_ascii_case("txt")
            })
        {
            fs::copy(&path, stage.path().join(dependency.file_name()))?;
        }
    }

    let folder = format!("{name}-Windows");
    for number in 1.. {
        let destination = output.join(if number == 1 {
            folder.clone()
        } else {
            format!("{folder}-{number}")
        });
        if destination.exists() {
            continue;
        }
        match fs::rename(stage.path(), &destination) {
            Ok(()) => {
                let _ = stage.keep();
                return Ok(destination.join(exe_name));
            }
            Err(_) if destination.exists() => continue,
            Err(error) => return Err(error),
        }
    }
    unreachable!()
}

fn bake(project: &Project, writer: &mut Writer<impl io::Write>) -> io::Result<()> {
    let mut scenes = BTreeSet::from([package::key(&project.config.main_scene)?]);
    let mut assets = BTreeMap::new();
    for path in files(project, Some(&project.config.scenes))? {
        if Path::new(&path)
            .extension()
            .is_some_and(|ext| ext.eq_ignore_ascii_case("telsc"))
        {
            scenes.insert(path);
        }
    }
    if project.path(&project.config.assets)?.try_exists()? {
        for path in files(project, Some(&project.config.assets))? {
            if let Some(kind) = Kind::asset(&path).filter(|_| !is_editor_plugin(&path)) {
                assets.insert(path, kind);
            }
        }
    }
    for path in files(project, None)? {
        if Kind::asset(&path) == Some(Kind::Script) && !is_editor_plugin(&path) {
            assets.insert(path, Kind::Script);
        }
    }

    for path in scenes {
        let result = (|| {
            let scene = PackedScene::load(&project.path(&path)?)?;
            for node in &scene.nodes {
                if let Some(mesh) = &node.mesh_renderer {
                    if let Some(path) = &mesh.mesh {
                        add_reference(&mut assets, path, Kind::Mesh)?;
                    }
                    if let Some(path) = &mesh.texture {
                        add_reference(&mut assets, path, Kind::Texture)?;
                    }
                }
                if let Some(script) = &node.script {
                    add_reference(&mut assets, &script.path, Kind::Script)?;
                }
                if let Some(sound) = &node.audio_source {
                    add_reference(&mut assets, &sound.clip, Kind::Audio)?;
                }
            }
            writer.entry(Kind::Scene, &path, &scene.to_bytes(DocumentFormat::Binary)?)
        })();
        result.map_err(|error| with_path(&path, error))?;
    }

    let importer = Assets::new(project.root())?;
    for (path, kind) in assets {
        let result = (|| {
            project.path(&path)?;
            let source = Path::new(&path);
            let bytes = match kind {
                Kind::Mesh => baked::<Mesh>(&importer, source)?,
                Kind::Texture => baked::<Texture>(&importer, source)?,
                Kind::Audio => baked::<AudioClip>(&importer, source)?,
                Kind::Script => importer
                    .request::<ScriptAsset>(source)?
                    .wait()?
                    .bytecode()
                    .to_vec(),
                _ => unreachable!(),
            };
            writer.entry(kind, &path, &bytes)
        })();
        result.map_err(|error| with_path(&path, error))?;
    }
    Ok(())
}

fn baked<T: Asset>(assets: &Assets, path: &Path) -> io::Result<Vec<u8>> {
    assets.request::<T>(path)?.wait()?.baked_bytes()
}

fn add_reference(assets: &mut BTreeMap<String, Kind>, path: &str, kind: Kind) -> io::Result<()> {
    if Kind::asset(path) != Some(kind) {
        return Err(invalid(format!("Expected a {kind:?} asset at {path:?}")));
    }
    let key = package::key(path)?;
    if is_editor_plugin(&key) {
        return Err(invalid(format!(
            "The scene references an editor plugin at {path:?}"
        )));
    }
    assets.insert(key, kind);
    Ok(())
}

fn is_editor_plugin(path: &str) -> bool {
    path.split_once('/')
        .is_some_and(|(folder, _)| folder.eq_ignore_ascii_case("plugins"))
}

fn files(project: &Project, directory: Option<&str>) -> io::Result<Vec<String>> {
    let mut files = Vec::new();
    let root = match directory {
        Some(path) => (project.path(path)?, package::key(path)?),
        None => (project.root().to_owned(), String::new()),
    };
    let mut pending = vec![root];
    while let Some((directory, relative)) = pending.pop() {
        for entry in fs::read_dir(&directory).map_err(|error| with_path(&relative, error))? {
            let entry = entry?;
            let kind = entry.file_type()?;
            let name = entry.file_name();
            let name = name
                .to_str()
                .ok_or_else(|| invalid("Project file name is not Unicode"))?;
            let path = if relative.is_empty() {
                name.to_owned()
            } else {
                format!("{relative}/{name}")
            };
            // Only explicit references follow links, through Project::path.
            if kind.is_dir() && !matches!(name, ".telcache" | ".git") {
                pending.push((entry.path(), path));
            } else if kind.is_file() {
                files.push(path);
            }
        }
    }
    Ok(files)
}

fn template_directory() -> io::Result<PathBuf> {
    let local = std::env::current_exe()?
        .with_file_name("templates")
        .join("windows");
    let workspace = Path::new(env!("CARGO_MANIFEST_DIR")).join("../templates/windows");
    [local, workspace].into_iter()
        .find(|path| path.join(TEMPLATE).is_file() && path.join("redistributables").is_dir())
        .ok_or_else(|| invalid("Windows runtime template is missing. Run Engine/templates/windows/build-template.ps1"))
}

fn game_name(name: &str) -> String {
    let name: String = name
        .chars()
        .take(80)
        .map(|ch| {
            if ch.is_control() || "<>:\"/\\|?*.".contains(ch) {
                '_'
            } else {
                ch
            }
        })
        .collect();
    let name = name.trim();
    let upper = name.to_ascii_uppercase();
    if name.is_empty() {
        "Game".into()
    } else if matches!(upper.as_str(), "CON" | "PRN" | "AUX" | "NUL")
        || upper
            .strip_prefix("COM")
            .or_else(|| upper.strip_prefix("LPT"))
            .is_some_and(|suffix| "123456789¹²³".contains(suffix) && suffix.chars().count() == 1)
    {
        format!("{name}_")
    } else {
        name.into()
    }
}

fn invalid(error: impl ToString) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, error.to_string())
}

fn with_path(path: &str, error: io::Error) -> io::Error {
    io::Error::new(error.kind(), format!("{path}: {error}"))
}

use std::collections::{HashMap, HashSet};
use std::fs::File;
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver};
use std::time::{Duration, Instant};

use notify::{Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use tellurium_assets::{AssetRequest, Assets, ScriptAsset, Texture};
use tellurium_editor::{EditorEvent, EditorResult, ResourceKind};
use tellurium_serialization::Project;

use crate::editor_host::Game;

const DEBOUNCE: Duration = Duration::from_millis(150);
const EMBEDDED_SHADER: &str = include_str!("../../graphics-cpp/shaders/mesh.hlsl");

pub(crate) struct ProjectWatch {
    _watcher: RecommendedWatcher,
    incoming: Receiver<notify::Result<Event>>,
    delayed: HashMap<PathBuf, Instant>,
    root: PathBuf,
    shader: Option<PathBuf>,
    assets: Assets,
    scripts: HashMap<String, AssetRequest<ScriptAsset>>,
    textures: HashMap<String, AssetRequest<Texture>>,
}

impl ProjectWatch {
    pub(crate) fn new(project: &Project) -> EditorResult<Self> {
        let root = project.root().canonicalize()?;
        let assets = Assets::new(&root)?;
        let (events, incoming) = mpsc::channel();
        let mut watcher = notify::recommended_watcher(move |event| {
            let _ = events.send(event);
        })?;
        watcher.watch(&root, RecursiveMode::Recursive)?;
        let mut shader = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../graphics-cpp/shaders/mesh.hlsl")
            .canonicalize()
            .ok();
        if let Some(path) = &shader {
            if !path.starts_with(&root) {
                if let Some(folder) = path.parent() {
                    if let Err(error) = watcher.watch(folder, RecursiveMode::NonRecursive) {
                        log::warn!("Cannot watch the engine shader: {error}");
                        shader = None;
                    }
                }
            }
        }
        Ok(Self {
            _watcher: watcher,
            incoming,
            delayed: HashMap::new(),
            root,
            shader,
            assets,
            scripts: HashMap::new(),
            textures: HashMap::new(),
        })
    }

    pub(crate) fn poll(&mut self, mut game: Option<&mut Game>, loading: bool) -> Vec<EditorEvent> {
        let watch_runtime_assets = game.is_some() || loading;
        let now = Instant::now();
        for event in self.incoming.try_iter().take(2048) {
            match event {
                Ok(event) if !matches!(event.kind, EventKind::Access(_)) => {
                    for path in event.paths {
                        if matches!(extension(&path).as_str(), "luau" | "png" | "telsc" | "hlsl") {
                            self.delayed.insert(path, now);
                        }
                    }
                }
                Err(error) => log::warn!("File watcher: {error}"),
                _ => {}
            }
        }
        let mut due = HashSet::new();
        self.delayed.retain(|path, seen| {
            if now.duration_since(*seen) >= DEBOUNCE {
                due.insert(path.clone());
                false
            } else {
                true
            }
        });
        let mut events = Vec::new();
        for path in due {
            // Canonicalize the parent so replace-by-rename and removal notifications
            // retain their authored filename, even when the file briefly disappears.
            let Some(path) = path
                .parent()
                .zip(path.file_name())
                .and_then(|(parent, name)| {
                    parent.canonicalize().ok().map(|parent| parent.join(name))
                })
            else {
                continue;
            };
            if self.shader.as_ref() == Some(&path) {
                if let Some(game) = game.as_deref_mut() {
                    match self.reload_shader(game) {
                        Ok(()) => events.push(EditorEvent::ResourceReloaded(
                            ResourceKind::Shader,
                            "mesh.hlsl".into(),
                        )),
                        Err(error) => log::warn!("Shader reload: {error}"),
                    }
                }
                continue;
            }
            let Ok(relative) = path.strip_prefix(&self.root) else {
                continue;
            };
            let Some(relative) = relative.to_str() else {
                continue;
            };
            let name = relative.replace('\\', "/");
            match extension(&path).as_str() {
                "telsc" => events.push(EditorEvent::SceneChanged(name)),
                "luau"
                    if watch_runtime_assets
                        && !name.split_once('/').is_some_and(|(directory, _)| {
                            directory.eq_ignore_ascii_case("plugins")
                        }) =>
                {
                    match self.assets.request(&name) {
                        Ok(request) => {
                            self.scripts.insert(name, request);
                        }
                        Err(error) => log::warn!("Script import {name}: {error}"),
                    }
                }
                "png" if watch_runtime_assets => match self.assets.request(&name) {
                    Ok(request) => {
                        self.textures.insert(name, request);
                    }
                    Err(error) => log::warn!("Texture import {name}: {error}"),
                },
                _ => {}
            }
        }
        if let Some(game) = game {
            self.scripts.retain(|name, request| match request.poll() {
                Ok(None) => true,
                Ok(Some(asset)) => {
                    let count = game.runtime.scripting.reload_asset(name, asset);
                    if count > 0 {
                        log::info!("Reloaded {name} for {count} script instance(s)");
                        events.push(EditorEvent::ResourceReloaded(
                            ResourceKind::Script,
                            name.clone(),
                        ));
                    }
                    false
                }
                Err(error) => {
                    log::warn!("Script reload {name}: {error}");
                    false
                }
            });
            self.textures.retain(|name, request| match request.poll() {
                Ok(None) => true,
                Ok(Some(asset)) => {
                    match game.renderer.reload_texture(name, asset) {
                        Ok(true) => events.push(EditorEvent::ResourceReloaded(
                            ResourceKind::Texture,
                            name.clone(),
                        )),
                        Ok(false) => {}
                        Err(error) => log::warn!("Texture reload {name}: {error}"),
                    }
                    false
                }
                Err(error) => {
                    log::warn!("Texture import {name}: {error}");
                    false
                }
            });
        } else if !loading {
            self.scripts.clear();
            self.textures.clear();
        }
        events
    }

    pub(crate) fn reload_shader(&self, game: &mut Game) -> EditorResult<()> {
        if let Some(shader) = &self.shader {
            game.renderer
                .reload_shader(&read_text(shader, 1024 * 1024)?)?;
        }
        Ok(())
    }

    pub(crate) fn initial_shader(&self, game: &mut Game) -> EditorResult<()> {
        if let Some(shader) = &self.shader {
            let source = read_text(shader, 1024 * 1024)?;
            if source != EMBEDDED_SHADER {
                game.renderer.reload_shader(&source)?;
            }
        }
        Ok(())
    }
}

pub(crate) fn read_text(path: &Path, limit: u64) -> io::Result<String> {
    let mut text = String::new();
    File::open(path)?
        .take(limit + 1)
        .read_to_string(&mut text)?;
    if text.len() as u64 > limit {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "source file exceeds the size limit",
        ));
    }
    Ok(text)
}

fn extension(path: &Path) -> String {
    path.extension()
        .and_then(|extension| extension.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tellurium_serialization::ProjectConfig;

    #[test]
    fn native_watch_reports_atomic_scene_saves() {
        let folder = tempfile::tempdir_in(env!("CARGO_MANIFEST_DIR")).unwrap();
        let project =
            Project::new(folder.path().join("project.toml"), ProjectConfig::default()).unwrap();
        let scenes = folder.path().join("scenes");
        std::fs::create_dir(&scenes).unwrap();
        let mut watch = ProjectWatch::new(&project).unwrap();
        let mut save = tempfile::NamedTempFile::new_in(&scenes).unwrap();
        save.write_all(b"scene").unwrap();
        save.persist(scenes.join("main.telsc")).unwrap();
        let deadline = Instant::now() + Duration::from_secs(4);
        loop {
            if watch.poll(None, false).into_iter().any(|event| {
                matches!(event, EditorEvent::SceneChanged(path) if path == "scenes/main.telsc")
            }) {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "native scene notification did not arrive"
            );
            std::thread::sleep(Duration::from_millis(25));
        }
    }
}

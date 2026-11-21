use std::error::Error;
use std::path::PathBuf;
use std::sync::Arc;

use tellurium_assets::package::{Kind, Package, CONFIG};
use tellurium_core::Transform;
use tellurium_platform::{fs, Platform, RawInputEvent};
use tellurium_scripting::LuauConfig;
use tellurium_serialization::{ComponentRegistry, PackedScene, Project, WorldSnapshot};

use crate::demo::{self, DemoLoading};
use crate::scene::{self, PreparedScene};
use crate::{Runtime, SceneRenderer};

type RunResult = Result<(), Box<dyn Error>>;

#[derive(Default)]
struct Options {
    project: Option<PathBuf>,
    scene: Option<PathBuf>,
    load: Option<PathBuf>,
    save: Option<PathBuf>,
    script: Option<PathBuf>,
    demo: bool,
    runtime: bool,
}

impl Options {
    fn parse() -> Result<Option<Self>, Box<dyn Error>> {
        let mut options = Self::default();
        let mut args = std::env::args_os().skip(1);
        while let Some(arg) = args.next() {
            let slot = match arg.to_str() {
                Some("--project") => &mut options.project,
                Some("--scene") => &mut options.scene,
                Some("--load") => &mut options.load,
                Some("--save") => &mut options.save,
                Some("--runtime") => {
                    options.runtime = true;
                    continue;
                }
                Some("--demo") => {
                    options.demo = true;
                    continue;
                }
                Some("-h" | "--help") => {
                    println!("tellurium_runtime [--runtime] [--project <project.toml|.tprj|directory>] [--scene <.telsc> | --load <.telsave>] [--save <.telsave>]");
                    println!("tellurium_runtime --demo | <script.luau>");
                    return Ok(None);
                }
                _ if std::path::Path::new(&arg)
                    .extension()
                    .is_some_and(|ext| ext.eq_ignore_ascii_case("luau")) =>
                {
                    if options.script.replace(arg.into()).is_some() {
                        return Err("Specify one Luau script".into());
                    }
                    continue;
                }
                _ => return Err(format!("Unknown argument: {}", arg.to_string_lossy()).into()),
            };
            if slot.is_some() {
                return Err(format!("Repeated argument {:?}", arg).into());
            }
            *slot = Some(
                args.next()
                    .ok_or_else(|| format!("Missing path after {:?}", arg))?
                    .into(),
            );
        }
        if (options.scene.is_some() && options.load.is_some())
            || ((options.demo || options.script.is_some())
                && (options.project.is_some()
                    || options.scene.is_some()
                    || options.load.is_some()
                    || options.save.is_some()))
            || (options.demo && options.script.is_some())
        {
            return Err(
                "Choose a project scene, a snapshot, the demo, or a standalone script".into(),
            );
        }
        Ok(Some(options))
    }

    #[cfg(all(feature = "editor", windows))]
    fn open_editor(&self) -> bool {
        !self.runtime
            && !self.demo
            && self.script.is_none()
            && self.scene.is_none()
            && self.load.is_none()
            && self.save.is_none()
    }
}

pub fn run() -> RunResult {
    let Some(options) = Options::parse()? else {
        return Ok(());
    };
    let package = Package::open_game()?;
    #[cfg(all(feature = "editor", windows))]
    if package.is_none() && options.open_editor() {
        crate::editor_host::run(options.project)?;
        return Ok(());
    }
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();
    run_runtime(options, package)
}

fn run_runtime(options: Options, package: Option<Package>) -> RunResult {
    let registry = ComponentRegistry::default();
    let mut loading = if options.demo {
        Some(DemoLoading::request()?)
    } else {
        None
    };
    let mut project = if let Some(package) = &package {
        if options.project.is_some()
            || options.scene.is_some()
            || options.demo
            || options.script.is_some()
        {
            return Err("The exported game uses the project and scenes in its .telpak".into());
        }
        let document = std::str::from_utf8(package.bytes(Kind::Project, CONFIG)?)?;
        Some(Project::from_document(
            std::env::current_exe()?.with_extension("telpak"),
            document,
        )?)
    } else {
        options.project.as_deref().map(Project::load).transpose()?
    };
    if project.is_none() && !options.demo && options.script.is_none() {
        for folder in [
            std::env::current_dir()?,
            std::env::current_exe()?.parent().unwrap().to_path_buf(),
        ] {
            if folder.join("project.toml").is_file()
                || std::fs::read_dir(&folder)?
                    .filter_map(Result::ok)
                    .any(|entry| tellurium_serialization::is_project_file(&entry.path()))
            {
                project = Some(Project::load(&folder)?);
                break;
            }
        }
    }
    if project.is_none()
        && (options.scene.is_some() || options.load.is_some() || options.save.is_some())
    {
        return Err("Specify --project when opening a scene or snapshot".into());
    }
    let snapshot = options
        .load
        .as_deref()
        .map(WorldSnapshot::load)
        .transpose()?;
    let prepared = project
        .as_ref()
        .map(|project| {
            let scene = match &snapshot {
                Some(snapshot) => snapshot.scene.clone(),
                None => match &package {
                    Some(package) => PackedScene::from_bytes(
                        package.bytes(Kind::Scene, &project.config.main_scene)?,
                    )?,
                    None => scene::load_project_scene(project, options.scene.as_deref())?,
                },
            };
            match &package {
                Some(package) => PreparedScene::load_packaged(project, scene, package),
                None => PreparedScene::load(project, scene),
            }
        })
        .transpose()?;
    drop(package);
    let platform = Platform::new()?;
    if let Some(project) = &project {
        platform.window.set_title(&project.config.name);
    }
    let mut renderer = SceneRenderer::new(Arc::clone(&platform.window), platform.inner_size())?;
    let mut runtime = if let Some((prepared, project)) = prepared.zip(project.as_ref()) {
        prepared.install(project, &mut renderer, &registry)?
    } else {
        let mut runtime = Runtime::new(LuauConfig::default())?;
        if options.demo || options.script.is_some() {
            runtime.input = demo::controls()?;
        }
        runtime
    };
    if let Some(snapshot) = &snapshot {
        runtime.restore_clock(snapshot);
    }
    let mut watch_position = if let Some(path) = options.script {
        let name = path.to_string_lossy();
        let source = fs::read_to_string(&path)?;
        let entity = runtime.spawn(&name, &source)?;
        runtime
            .world
            .borrow_mut()
            .add_component(entity, renderer.demo_triangle()?);
        Some(entity)
    } else {
        None
    };
    let mut runtime_error = None;
    let restored_time = snapshot.as_ref().map_or(0.0, |value| value.elapsed);
    let mut elapsed = restored_time;

    let event_loop_result = platform.run(|ctx| {
        let input = ctx.input.drain();
        if input.contains(&RawInputEvent::WindowClosed) {
            return false;
        }
        if loading.is_some() {
            match DemoLoading::poll_install(&mut loading, &mut runtime, &mut renderer) {
                Ok(Some(scene)) => runtime.demo = Some(scene),
                Ok(None) => {}
                Err(error) => {
                    runtime_error = Some(error);
                    return false;
                }
            }
        }
        let delta = ctx.timer.tick();
        elapsed = restored_time + ctx.timer.elapsed();
        let frame_result = runtime.frame(input, delta, elapsed, |world| {
            if ctx.window.is_minimized() == Some(true) {
                return Ok(());
            }
            if project.is_some() {
                scene::camera(world, &mut renderer);
            }
            let size = ctx.window.inner_size();
            renderer.render(world, (size.width, size.height), || {
                ctx.window.pre_present_notify()
            })
        });
        if let Err(error) = frame_result.and_then(|result| result.map_err(Into::into)) {
            runtime_error = Some(error);
            return false;
        }
        if let Some(entity) = watch_position {
            let world = runtime.world.borrow();
            if let Some(transform) = world.get_component::<Transform>(entity) {
                if transform.position != [0.0; 3] {
                    log::info!(
                        "Entity {} position changed by Luau: {:?}",
                        entity.0,
                        transform.position
                    );
                    watch_position = None;
                }
            }
        }
        true
    });

    let save = if let Some((path, project)) = options.save.zip(project) {
        if runtime_error.is_none() && event_loop_result.is_ok() {
            runtime.snapshot(elapsed, &registry).and_then(|snapshot| {
                snapshot
                    .save(&path, project.config.scene_format)
                    .map_err(Into::into)
            })
        } else {
            Ok(())
        }
    } else {
        Ok(())
    };
    runtime.shutdown();
    event_loop_result?;
    if let Some(error) = runtime_error {
        return Err(error);
    }
    save
}

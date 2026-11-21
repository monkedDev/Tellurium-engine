//! Editable projects shipped with the editor.

use std::fs;
use std::io;
use std::path::Path;

use tellurium_serialization::Project;

pub struct Sample {
    pub name: &'static str,
    pub description: &'static str,
    pub controls: &'static str,
    pub(crate) icon: &'static str,
    folder: &'static str,
    files: &'static [(&'static str, &'static [u8])],
}

macro_rules! files {
    ($sample:literal, [$($path:literal),+ $(,)?] $(, $extra:expr)* $(,)?) => {
        &[$(($path, include_bytes!(concat!("../samples/", $sample, "/", $path)).as_slice()),)+ $($extra,)*]
    };
}

pub const CATALOG: &[Sample] = &[
    Sample {
        name: "Neon Courier",
        description: "A 2D checkpoint circuit with a dash, a moving beacon, and lap tracking.",
        controls: "WASD / arrows or left stick · Space / A to dash",
        icon: "node-sprite",
        folder: "neon-courier",
        files: files!("neon-courier", [
            "project.toml", "scenes/main.telsc", "assets/courier.luau", "assets/beacon.luau",
        ]),
    },
    Sample {
        name: "Gravity Foundry",
        description: "A Bullet physics test with falling crates, a domino run, and a sensor gate.",
        controls: "Space / A sends the shuttle through the sensor",
        icon: "component-rigidbody",
        folder: "gravity-foundry",
        files: files!("gravity-foundry", [
            "project.toml", "scenes/main.telsc", "assets/shuttle.luau", "assets/sensor.luau", "assets/indicator.luau",
        ],
            ("assets/crate.fbx", include_bytes!("../../runtime/demo/box.fbx")),
            ("assets/grid.png", include_bytes!("../../runtime/demo/grid.png")),
        ),
    },
    Sample {
        name: "Orbit Soundstage",
        description: "Two moving audio emitters around a fixed listener. Headphones make the stereo movement easier to hear.",
        controls: "T / X toggles the orbit speed",
        icon: "node-audio",
        folder: "orbit-soundstage",
        files: files!("orbit-soundstage", [
            "project.toml", "scenes/main.telsc", "assets/orbit.luau", "assets/pulse.luau", "assets/beacon.wav", "plugins/stage_notes.luau",
        ],
            ("assets/orbit.ogg", include_bytes!("../../runtime/demo/orbit.ogg")),
        ),
    },
];

impl Sample {
    pub(crate) fn has_copy(&self, directory: &Path) -> bool {
        directory.join(self.folder).is_dir()
    }

    /// Materialize a complete copy once. Future visits reuse the editable project.
    pub fn open(&self, directory: &Path) -> io::Result<Project> {
        fs::create_dir_all(directory)?;
        let copy = directory.join(self.folder);
        let project_file = copy.join("project.toml");
        if copy.try_exists()? {
            return Project::load(&project_file);
        }
        let stage = tempfile::tempdir_in(directory)?;
        for (relative, contents) in self.files {
            let target = stage.path().join(relative);
            if let Some(parent) = target.parent() {
                fs::create_dir_all(parent)?;
            }
            fs::write(target, contents)?;
        }
        match fs::rename(stage.path(), &copy) {
            Ok(()) => {
                let _ = stage.keep();
            }
            Err(error) if !project_file.is_file() => return Err(error),
            Err(_) => {}
        }
        Project::load(&project_file)
    }
}

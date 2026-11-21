use std::fs;
use std::io;
use std::path::{Component, Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::{atomic_write, invalid};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DocumentFormat {
    #[default]
    Text,
    Binary,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum GraphicsBackend {
    #[default]
    Dx12,
    Vulkan,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ExportConfig {
    pub release: bool,
    pub target: String,
    pub icon: String,
    pub console: bool,
}

impl Default for ExportConfig {
    fn default() -> Self {
        Self {
            release: true,
            target: String::new(),
            icon: String::new(),
            console: false,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ProjectConfig {
    pub name: String,
    pub description: String,
    pub engine_version: String,
    pub graphics_backend: GraphicsBackend,
    pub assets: String,
    pub scenes: String,
    pub main_scene: String,
    pub scene_format: DocumentFormat,
    pub recent_scenes: Vec<String>,
    pub export: ExportConfig,
}

impl Default for ProjectConfig {
    fn default() -> Self {
        Self {
            name: "Untitled".into(),
            description: String::new(),
            engine_version: env!("CARGO_PKG_VERSION").into(),
            graphics_backend: GraphicsBackend::Dx12,
            assets: "assets".into(),
            scenes: "scenes".into(),
            main_scene: "scenes/main.telsc".into(),
            scene_format: DocumentFormat::Text,
            recent_scenes: Vec::new(),
            export: ExportConfig::default(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct Project {
    pub file: PathBuf,
    pub config: ProjectConfig,
    document: toml::Table,
}

impl Project {
    pub fn load(path: &Path) -> io::Result<Self> {
        let file = if path.is_dir() {
            let toml = path.join("project.toml");
            if toml.is_file() {
                toml
            } else {
                let files = fs::read_dir(path)?
                    .filter_map(Result::ok)
                    .map(|entry| entry.path())
                    .filter(|path| is_project_file(path))
                    .collect::<Vec<_>>();
                if files.len() != 1 {
                    return Err(invalid("Select a project.toml or a specific .tprj file"));
                }
                files.into_iter().next().unwrap()
            }
        } else {
            path.to_path_buf()
        }
        .canonicalize()?;
        if !is_project_file(&file) {
            return Err(invalid("The project must use project.toml or .tprj"));
        }
        let source = fs::read_to_string(&file)?;
        let project = Self::from_document(file, &source)?;
        project.validate()?;
        Ok(project)
    }

    pub fn from_document(file: PathBuf, source: &str) -> io::Result<Self> {
        let document: toml::Table = source.parse().map_err(invalid)?;
        let info = document
            .get("project")
            .cloned()
            .ok_or_else(|| invalid("Missing [project] section"))?;
        let config: ProjectConfig = info.try_into().map_err(invalid)?;
        let project = Self {
            file,
            config,
            document,
        };
        project.validate_paths()?;
        Ok(project)
    }

    pub fn new(file: PathBuf, config: ProjectConfig) -> io::Result<Self> {
        let project = Self {
            file,
            config,
            document: toml::Table::default(),
        };
        project.validate()?;
        Ok(project)
    }

    pub fn root(&self) -> &Path {
        self.file
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
            .unwrap_or(Path::new("."))
    }

    pub fn path(&self, relative: &str) -> io::Result<PathBuf> {
        confined(self.root(), relative)
    }

    pub fn input_config(&self) -> String {
        self.document.to_string()
    }

    pub fn export_document(&self) -> io::Result<String> {
        self.validate()?;
        let mut config = self.config.clone();
        config.recent_scenes.clear();
        let mut config = toml::Value::try_from(config).map_err(invalid)?;
        config.as_table_mut().unwrap().remove("export");
        let mut document = toml::Table::new();
        document.insert("project".into(), config);
        if let Some(input) = self.document.get("input") {
            document.insert("input".into(), input.clone());
        }
        toml::to_string(&document).map_err(invalid)
    }

    pub fn save(&self) -> io::Result<()> {
        self.validate()?;
        let mut document = self.document.clone();
        let info = toml::Value::try_from(&self.config).map_err(invalid)?;
        let entry = document
            .entry("project")
            .or_insert_with(|| toml::Value::Table(toml::Table::new()));
        if let (Some(existing), toml::Value::Table(fields)) = (entry.as_table_mut(), info) {
            existing.extend(fields);
        }
        atomic_write(
            &self.file,
            toml::to_string_pretty(&document)
                .map_err(invalid)?
                .as_bytes(),
        )
    }

    fn validate(&self) -> io::Result<()> {
        self.validate_paths()?;
        self.path(&self.config.assets)?;
        let scenes = self.path(&self.config.scenes)?;
        if !self.path(&self.config.main_scene)?.starts_with(scenes) {
            return Err(invalid(
                "Main scene must be inside the project scenes folder",
            ));
        }
        Ok(())
    }

    fn validate_paths(&self) -> io::Result<()> {
        if self.config.name.trim().is_empty() {
            return Err(invalid("Project name is required"));
        }
        relative_path(&self.config.assets)?;
        relative_path(&self.config.scenes)?;
        if !relative_path(&self.config.main_scene)?
            .extension()
            .is_some_and(|ext| ext.eq_ignore_ascii_case("telsc"))
        {
            return Err(invalid(
                "Main scene must be a .telsc file in the project scenes folder",
            ));
        }
        Ok(())
    }
}

pub fn is_project_file(path: &Path) -> bool {
    path.file_name()
        .is_some_and(|name| name.eq_ignore_ascii_case("project.toml"))
        || path
            .extension()
            .is_some_and(|ext| ext.eq_ignore_ascii_case("tprj"))
}

pub fn relative_path(value: &str) -> io::Result<&Path> {
    let path = Path::new(value);
    if value.is_empty()
        || value.contains([':', '\0'])
        || path
            .components()
            .any(|part| !matches!(part, Component::Normal(_)))
        || value
            .split(['/', '\\'])
            .any(|part| part == ".." || part == "." || part.is_empty())
    {
        return Err(invalid(format!(
            "Expected a project-relative path, got {value:?}"
        )));
    }
    Ok(path)
}

/// Resolve existing symlinks and prevent leaving the project, including for a new file.
pub fn confined(root: &Path, value: &str) -> io::Result<PathBuf> {
    let relative = relative_path(value)?;
    let canonical_root = root.canonicalize()?;
    let mut existing = canonical_root.clone();
    for part in relative {
        existing.push(part);
        match fs::symlink_metadata(&existing) {
            Ok(_) => {
                existing = existing.canonicalize()?;
                if !existing.starts_with(&canonical_root) {
                    return Err(invalid("Project path points outside the project"));
                }
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }
    }
    Ok(existing)
}

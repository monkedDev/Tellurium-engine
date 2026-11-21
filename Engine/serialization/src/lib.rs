//! Versioned scene and world formats. Native resource handles are rebuilt on load.

mod hierarchy;
mod project;
mod registry;
mod script_state;

pub use hierarchy::{compose, relative, Hierarchy, SceneNode};
pub use project::{
    confined, is_project_file, relative_path, DocumentFormat, GraphicsBackend, Project,
    ProjectConfig,
};
pub use registry::ComponentRegistry;
pub use tellurium_core::{EntityId, Transform};

use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs::{self, File};
use std::io::{self, Read, Write};
use std::path::Path;

use bincode::Options;
use glam::{Quat, Vec3};
use serde::{de::DeserializeOwned, Deserialize, Serialize};

pub const FORMAT_VERSION: u32 = 1;
const SCENE_MAGIC: &[u8] = b"TELSC\x01\0\0";
const SAVE_MAGIC: &[u8] = b"TELSAVE\x01";
const MAX_DOCUMENT_BYTES: u64 = 32 * 1024 * 1024;
const MAX_NODES: usize = 50_000;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum NodeKind {
    Node2D,
    #[default]
    Node3D,
    Camera,
    Light,
    MeshInstance,
    Sprite,
    AudioStreamPlayer,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum BodyType {
    Static,
    Dynamic,
    Kinematic,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RigidbodyData {
    pub kind: BodyType,
    pub mass: f32,
    pub linear_velocity: [f32; 3],
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ColliderShape {
    Box,
    Sphere,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ColliderData {
    pub shape: ColliderShape,
    pub dimensions: [f32; 3],
    pub trigger: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MeshRendererData {
    /// A missing mesh uses the engine's unit cube, or a quad for Sprite.
    pub mesh: Option<String>,
    pub texture: Option<String>,
    pub tint: [f32; 4],
    pub visible: bool,
}

impl Default for MeshRendererData {
    fn default() -> Self {
        Self {
            mesh: None,
            texture: None,
            tint: [1.0; 4],
            visible: true,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AudioSourceData {
    pub clip: String,
    pub volume: f32,
    pub looping: bool,
    pub spatial: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CameraData {
    pub half_height: f32,
    pub active: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum ScriptValue {
    Bool(bool),
    Integer(i64),
    Number(f64),
    String(String),
    Table(Vec<(ScriptValue, ScriptValue)>),
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ScriptState {
    pub started: bool,
    pub failed: bool,
    pub globals: BTreeMap<String, ScriptValue>,
    #[serde(default)]
    pub payload: Option<ScriptValue>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScriptData {
    pub path: String,
    #[serde(default, with = "script_state")]
    pub state: Option<ScriptState>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Node {
    pub id: EntityId,
    pub parent: Option<EntityId>,
    pub name: String,
    pub kind: NodeKind,
    pub transform: Transform,
    pub mesh_renderer: Option<MeshRendererData>,
    pub rigidbody: Option<RigidbodyData>,
    pub collider: Option<ColliderData>,
    pub audio_source: Option<AudioSourceData>,
    pub audio_listener: bool,
    pub camera: Option<CameraData>,
    pub script: Option<ScriptData>,
    /// Component names map to serde JSON payloads. Strings keep the binary schema portable.
    pub custom: BTreeMap<String, String>,
}

impl Default for Node {
    fn default() -> Self {
        Self {
            id: EntityId::NULL,
            parent: None,
            name: "Node".into(),
            kind: NodeKind::default(),
            transform: Transform::default(),
            mesh_renderer: None,
            rigidbody: None,
            collider: None,
            audio_source: None,
            audio_listener: false,
            camera: None,
            script: None,
            custom: BTreeMap::new(),
        }
    }
}

impl Node {
    pub fn new(name: impl Into<String>, kind: NodeKind) -> Self {
        let mut node = Self {
            name: name.into(),
            kind,
            ..Self::default()
        };
        match kind {
            NodeKind::Camera => {
                node.camera = Some(CameraData {
                    half_height: 2.5,
                    active: true,
                })
            }
            NodeKind::MeshInstance | NodeKind::Sprite => {
                node.mesh_renderer = Some(MeshRendererData::default())
            }
            _ => {}
        }
        node
    }

    fn validate(&self) -> io::Result<()> {
        let pose = self.transform;
        let rotation = Quat::from_array(pose.rotation);
        if self.name.trim().is_empty()
            || !Vec3::from_array(pose.position).is_finite()
            || !Vec3::from_array(pose.scale).is_finite()
            || pose.scale.iter().any(|scale| scale.abs() < 0.00001)
            || !rotation.is_finite()
            || !rotation.is_normalized()
        {
            return Err(invalid(format!(
                "Invalid name or transform on node {}",
                self.id.0
            )));
        }
        if let Some(mesh) = &self.mesh_renderer {
            for path in mesh.mesh.iter().chain(&mesh.texture) {
                relative_path(path)?;
            }
            if mesh
                .tint
                .iter()
                .any(|value| !value.is_finite() || !(0.0..=1.0).contains(value))
            {
                return Err(invalid("Mesh tint must be between zero and one"));
            }
        }
        if let Some(script) = &self.script {
            relative_path(&script.path)?;
        }
        if let Some(audio) = &self.audio_source {
            relative_path(&audio.clip)?;
            if !audio.volume.is_finite() || audio.volume < 0.0 {
                return Err(invalid("Audio volume must be finite and nonnegative"));
            }
        }
        if let Some(camera) = &self.camera {
            if !camera.half_height.is_finite() || camera.half_height < 0.01 {
                return Err(invalid("Camera half height must be at least 0.01"));
            }
        }
        if let Some(body) = &self.rigidbody {
            if !body.mass.is_finite()
                || body.mass < 0.0
                || (body.kind == BodyType::Dynamic && body.mass <= 0.0)
                || !Vec3::from_array(body.linear_velocity).is_finite()
            {
                return Err(invalid("Invalid rigidbody mass or velocity"));
            }
        }
        if let Some(collider) = &self.collider {
            if collider
                .dimensions
                .iter()
                .any(|value| !value.is_finite() || *value <= 0.0)
            {
                return Err(invalid("Collider dimensions must be finite and positive"));
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PackedScene {
    pub version: u32,
    pub nodes: Vec<Node>,
    #[serde(default)]
    pub last_entity_id: u64,
}

impl Default for PackedScene {
    fn default() -> Self {
        Self {
            version: FORMAT_VERSION,
            nodes: Vec::new(),
            last_entity_id: 0,
        }
    }
}

impl PackedScene {
    pub fn load(path: &Path) -> io::Result<Self> {
        let scene: Self = read_document(path, SCENE_MAGIC)?;
        scene.order()?;
        Ok(scene)
    }

    pub fn from_bytes(bytes: &[u8]) -> io::Result<Self> {
        let scene: Self = decode_document(bytes, SCENE_MAGIC)?;
        scene.order()?;
        Ok(scene)
    }

    pub fn to_bytes(&self, format: DocumentFormat) -> io::Result<Vec<u8>> {
        self.order()?;
        encode_document(SCENE_MAGIC, format, self)
    }

    pub fn save(&self, path: &Path, format: DocumentFormat) -> io::Result<()> {
        self.order()?;
        write_document(path, SCENE_MAGIC, format, self)
    }

    /// Stable parent-before-child order without recursion. Validates the entire document.
    pub fn order(&self) -> io::Result<Vec<usize>> {
        if self.version != FORMAT_VERSION || self.nodes.len() > MAX_NODES {
            return Err(invalid("Unsupported scene version or too many nodes"));
        }
        let mut ids = HashSet::with_capacity(self.nodes.len());
        let mut children: HashMap<Option<EntityId>, Vec<usize>> = HashMap::new();
        for (index, node) in self.nodes.iter().enumerate() {
            if node.id == EntityId::NULL || node.id.0 == u64::MAX || !ids.insert(node.id) {
                return Err(invalid(format!(
                    "Invalid or duplicate entity ID {}",
                    node.id.0
                )));
            }
            node.validate()?;
            children.entry(node.parent).or_default().push(index);
        }
        if self.last_entity_id == u64::MAX {
            return Err(invalid("Entity ID space exhausted"));
        }
        let mut stack = children.remove(&None).unwrap_or_default();
        stack.reverse();
        let mut order = Vec::with_capacity(self.nodes.len());
        while let Some(index) = stack.pop() {
            order.push(index);
            if let Some(child_indices) = children.remove(&Some(self.nodes[index].id)) {
                stack.extend(child_indices.into_iter().rev());
            }
        }
        if order.len() != self.nodes.len() {
            return Err(invalid(
                "Scene contains a missing parent or a hierarchy cycle",
            ));
        }
        Ok(order)
    }

    pub fn insert(&mut self, mut node: Node, parent: Option<EntityId>) -> io::Result<EntityId> {
        if self.nodes.len() >= MAX_NODES || parent.is_some_and(|id| self.node(id).is_none()) {
            return Err(invalid("Cannot add a node to this scene"));
        }
        let id = next_id(self.last_id())?;
        node.id = id;
        node.parent = parent;
        self.nodes.push(node);
        self.last_entity_id = id.0;
        Ok(id)
    }

    pub fn node(&self, id: EntityId) -> Option<&Node> {
        self.nodes.iter().find(|node| node.id == id)
    }

    pub fn node_mut(&mut self, id: EntityId) -> Option<&mut Node> {
        self.nodes.iter_mut().find(|node| node.id == id)
    }

    pub fn last_id(&self) -> u64 {
        self.nodes
            .iter()
            .map(|node| node.id.0)
            .fold(self.last_entity_id, u64::max)
    }

    /// Copy a scene under one instance root. Every occurrence gets independent IDs.
    pub fn instance(
        &mut self,
        name: &str,
        scene: &Self,
        parent: Option<EntityId>,
    ) -> io::Result<EntityId> {
        let order = scene.order()?;
        if name.trim().is_empty() || self.nodes.len() + order.len() + 1 > MAX_NODES {
            return Err(invalid("Invalid instance name or too many nodes"));
        }
        let root = next_id(self.last_id())?;
        let mut last_id = root;
        let mut ids = HashMap::new();
        let mut nodes = Vec::with_capacity(order.len());
        for index in order {
            let mut node = scene.nodes[index].clone();
            last_id = next_id(last_id.0)?;
            ids.insert(node.id, last_id);
            node.id = last_id;
            node.parent = Some(node.parent.map_or(root, |id| ids[&id]));
            nodes.push(node);
        }
        self.insert(Node::new(name, NodeKind::Node3D), parent)?;
        self.nodes.extend(nodes);
        self.last_entity_id = last_id.0;
        Ok(root)
    }

    pub fn reparent(&mut self, child: EntityId, parent: Option<EntityId>) -> io::Result<()> {
        let parents: HashMap<_, _> = self
            .nodes
            .iter()
            .map(|node| (node.id, node.parent))
            .collect();
        let mut ancestor = parent;
        let mut remaining = self.nodes.len();
        while let Some(id) = ancestor {
            if id == child || remaining == 0 {
                return Err(invalid("A node cannot be parented to its own subtree"));
            }
            ancestor = *parents
                .get(&id)
                .ok_or_else(|| invalid("Parent does not exist"))?;
            remaining -= 1;
        }
        self.node_mut(child)
            .ok_or_else(|| invalid("Node does not exist"))?
            .parent = parent;
        Ok(())
    }

    pub fn remove_subtree(&mut self, root: EntityId) {
        let mut removed = HashSet::from([root]);
        let mut children: HashMap<EntityId, Vec<EntityId>> = HashMap::new();
        self.last_entity_id = self.last_id();
        for node in &self.nodes {
            if let Some(parent) = node.parent {
                children.entry(parent).or_default().push(node.id);
            }
        }
        let mut stack = vec![root];
        while let Some(id) = stack.pop() {
            if let Some(ids) = children.remove(&id) {
                stack.extend(ids.iter().copied().filter(|id| removed.insert(*id)));
            }
        }
        self.nodes.retain(|node| !removed.contains(&node.id));
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorldSnapshot {
    pub elapsed: f64,
    pub fixed_accumulator: f64,
    pub scene: PackedScene,
}

impl WorldSnapshot {
    pub fn load(path: &Path) -> io::Result<Self> {
        let snapshot: Self = read_document(path, SAVE_MAGIC)?;
        snapshot.validate()?;
        Ok(snapshot)
    }

    pub fn save(&self, path: &Path, format: DocumentFormat) -> io::Result<()> {
        self.validate()?;
        write_document(path, SAVE_MAGIC, format, self)
    }

    fn validate(&self) -> io::Result<()> {
        self.scene.order()?;
        if !self.elapsed.is_finite()
            || self.elapsed < 0.0
            || !self.fixed_accumulator.is_finite()
            || !(0.0..1.0 / 60.0).contains(&self.fixed_accumulator)
        {
            return Err(invalid("Invalid game clock in snapshot"));
        }
        Ok(())
    }
}

fn next_id(last: u64) -> io::Result<EntityId> {
    last.checked_add(1)
        .filter(|id| *id < u64::MAX)
        .map(EntityId)
        .ok_or_else(|| invalid("Entity ID space exhausted"))
}

fn binary() -> impl Options {
    bincode::DefaultOptions::new()
        .with_fixint_encoding()
        .with_limit(MAX_DOCUMENT_BYTES)
}

fn read_document<T: DeserializeOwned>(path: &Path, magic: &[u8]) -> io::Result<T> {
    let mut bytes = Vec::new();
    File::open(path)?
        .take(MAX_DOCUMENT_BYTES + 1)
        .read_to_end(&mut bytes)?;
    decode_document(&bytes, magic)
}

fn decode_document<T: DeserializeOwned>(bytes: &[u8], magic: &[u8]) -> io::Result<T> {
    if bytes.len() as u64 > MAX_DOCUMENT_BYTES {
        return Err(invalid("Document exceeds 32 MiB"));
    }
    if let Some(payload) = bytes.strip_prefix(magic) {
        return binary().deserialize(payload).map_err(invalid);
    }
    serde_json::from_slice(bytes).map_err(invalid)
}

fn write_document<T: Serialize>(
    path: &Path,
    magic: &[u8],
    format: DocumentFormat,
    value: &T,
) -> io::Result<()> {
    atomic_write(path, &encode_document(magic, format, value)?)
}

fn encode_document(
    magic: &[u8],
    format: DocumentFormat,
    value: &impl Serialize,
) -> io::Result<Vec<u8>> {
    let bytes = match format {
        DocumentFormat::Text => serde_json::to_vec_pretty(value).map_err(invalid)?,
        DocumentFormat::Binary => {
            let mut bytes = magic.to_vec();
            binary()
                .serialize_into(&mut bytes, value)
                .map_err(invalid)?;
            bytes
        }
    };
    if bytes.len() as u64 > MAX_DOCUMENT_BYTES {
        return Err(invalid("Document exceeds 32 MiB"));
    }
    Ok(bytes)
}

pub fn atomic_write(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let parent = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    fs::create_dir_all(parent)?;
    let mut file = tempfile::NamedTempFile::new_in(parent)?;
    file.write_all(bytes)?;
    file.as_file().sync_all()?;
    file.persist(path)?;
    Ok(())
}

pub(crate) fn invalid(error: impl std::fmt::Display) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, error.to_string())
}

impl tellurium_core::Component for MeshRendererData {}
impl tellurium_core::Component for CameraData {}

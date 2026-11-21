//! Background import, on-disk asset cache and typed, reference-counted handles.

mod formats;
pub mod package;

use std::any::Any;
use std::collections::HashMap;
use std::fs::{self, File};
use std::io::{self, Read, Write};
use std::ops::Deref;
use std::path::{Path, PathBuf};
use std::sync::{mpsc, Arc, Weak};
use std::thread::{self, JoinHandle};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

pub use formats::{AudioClip, Mesh, MeshVertex, ScriptAsset, Texture};
pub use uuid::Uuid as AssetId;

type Result<T> = io::Result<T>;
type Erased = dyn Any + Send + Sync;
type Job = Box<dyn FnOnce(&mut Importer) + Send>;

const MAGIC: &[u8] = b"TELLURIUM-ASSET\x01";
pub(crate) const MAX_BYTES: usize = 128 * 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct AssetRevision {
    pub id: AssetId,
    pub digest: [u8; 32],
}

pub trait Asset: formats::Codec + Send + Sync + 'static {}
impl<T: formats::Codec + Send + Sync + 'static> Asset for T {}

struct Entry<T> {
    revision: AssetRevision,
    cache_path: PathBuf,
    data: T,
}

pub struct Handle<T>(Arc<Entry<T>>);

impl<T> Clone for Handle<T> {
    fn clone(&self) -> Self {
        Self(Arc::clone(&self.0))
    }
}

impl<T> Handle<T> {
    pub fn revision(&self) -> AssetRevision {
        self.0.revision
    }

    pub fn cache_path(&self) -> &Path {
        &self.0.cache_path
    }
}

impl<T: Asset> Handle<T> {
    /// Return the current optimized payload, checking its cache revision and checksum.
    pub fn baked_bytes(&self) -> io::Result<Vec<u8>> {
        read_cache(self.cache_path(), &self.revision())
    }
}

impl<T> Deref for Handle<T> {
    type Target = T;

    fn deref(&self) -> &T {
        &self.0.data
    }
}

pub struct AssetRequest<T> {
    receiver: mpsc::Receiver<Result<Handle<T>>>,
    ready: Option<Handle<T>>,
}

impl<T> AssetRequest<T> {
    pub fn poll(&mut self) -> Result<Option<&Handle<T>>> {
        if self.ready.is_none() {
            match self.receiver.try_recv() {
                Ok(result) => self.ready = Some(result?),
                Err(mpsc::TryRecvError::Empty) => {}
                Err(mpsc::TryRecvError::Disconnected) => return Err(disconnected()),
            }
        }
        Ok(self.ready.as_ref())
    }

    pub fn wait(self) -> Result<Handle<T>> {
        match self.ready {
            Some(ready) => Ok(ready),
            None => self.receiver.recv().map_err(|_| disconnected())?,
        }
    }
}

pub struct Assets {
    jobs: Option<mpsc::Sender<Job>>,
    worker: Option<JoinHandle<()>>,
}

impl Assets {
    /// Start one worker for the canonical project root and its .telcache directory.
    pub fn new(root: impl AsRef<Path>) -> Result<Self> {
        let root = root.as_ref().canonicalize()?;
        let cache = root.join(".telcache");
        fs::create_dir_all(&cache)?;
        let cache = cache.canonicalize()?;
        if !cache.starts_with(&root) {
            return Err(invalid(".telcache must be inside the project"));
        }
        let (jobs, requests) = mpsc::channel::<Job>();
        let worker = thread::Builder::new()
            .name("tellurium-assets".into())
            .spawn(move || {
                let mut importer = Importer {
                    root,
                    cache,
                    live: HashMap::new(),
                };
                for job in requests {
                    job(&mut importer);
                }
            })?;
        Ok(Self {
            jobs: Some(jobs),
            worker: Some(worker),
        })
    }

    /// Queue a project-relative source path and return immediately.
    pub fn request<T: Asset>(&self, path: impl Into<PathBuf>) -> Result<AssetRequest<T>> {
        let path = path.into();
        let (complete, request) = mpsc::channel();
        let job: Job = Box::new(move |importer| {
            let _ = complete.send(importer.load::<T>(&path));
        });
        self.jobs
            .as_ref()
            .ok_or_else(disconnected)?
            .send(job)
            .map_err(|_| disconnected())?;
        Ok(AssetRequest {
            receiver: request,
            ready: None,
        })
    }
}

impl Drop for Assets {
    fn drop(&mut self) {
        self.jobs.take();
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

fn disconnected() -> io::Error {
    io::Error::new(io::ErrorKind::BrokenPipe, "asset worker is unavailable")
}

pub(crate) fn invalid(error: impl ToString) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, error.to_string())
}

#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ImportSettings {
    /// Reverse image rows before storing RGBA8.
    pub flip_vertical: bool,
    /// Convert an audio asset to one spatializable channel.
    pub audio_mono: bool,
}

#[derive(Serialize, Deserialize)]
struct Meta {
    id: AssetId,
    source_hash: String,
    #[serde(default)]
    settings: ImportSettings,
}

struct Importer {
    root: PathBuf,
    cache: PathBuf,
    live: HashMap<PathBuf, Weak<Erased>>,
}

impl Importer {
    fn load<T: Asset>(&mut self, path: &Path) -> Result<Handle<T>> {
        let source_path = self.root.join(path).canonicalize()?;
        if !source_path.starts_with(&self.root) || source_path.starts_with(&self.cache) {
            return Err(invalid(
                "asset source must be inside the project and outside .telcache",
            ));
        }
        let extension = source_path
            .extension()
            .and_then(|ext| ext.to_str())
            .unwrap_or_default()
            .to_ascii_lowercase();
        if !T::EXTENSIONS.contains(&extension.as_str()) {
            return Err(invalid(format!(
                "asset type does not match {}",
                path.display()
            )));
        }

        let source = read_limited(&source_path, MAX_BYTES)?;
        let source_hash = Sha256::digest(&source);
        let meta = metadata(&source_path, &format!("{source_hash:x}"))?;
        let mut digest = Sha256::new();
        digest.update(MAGIC);
        digest.update(source_hash);
        digest.update(extension.as_bytes());
        digest.update([
            meta.settings.flip_vertical as u8,
            meta.settings.audio_mono as u8,
        ]);
        let hash = digest.finalize();
        let revision = AssetRevision {
            id: meta.id,
            digest: hash.into(),
        };

        if let Some(existing) = self
            .live
            .get(&source_path)
            .and_then(Weak::upgrade)
            .and_then(|asset| asset.downcast::<Entry<T>>().ok())
            .filter(|entry| entry.revision == revision)
        {
            return Ok(Handle(existing));
        }

        let cache_path = self.cache.join(format!("{}-{hash:x}.telbin", meta.id));
        let verified = T::verify_source(&source)?;
        let data = match read_cache(&cache_path, &revision)
            .and_then(|payload| T::decode(payload, verified.as_ref()))
        {
            Ok(data) => data,
            Err(error)
                if matches!(
                    error.kind(),
                    io::ErrorKind::NotFound | io::ErrorKind::InvalidData
                ) =>
            {
                let payload = T::import(&source, &extension, &meta.settings, verified.as_ref())?;
                write_cache(&cache_path, &revision, &payload)?;
                T::decode(read_cache(&cache_path, &revision)?, verified.as_ref())?
            }
            Err(error) => return Err(error),
        };

        let entry = Arc::new(Entry {
            revision,
            cache_path,
            data,
        });
        let erased: Arc<Erased> = entry.clone();
        self.live.retain(|_, weak| weak.strong_count() != 0);
        self.live.insert(source_path, Arc::downgrade(&erased));
        Ok(Handle(entry))
    }
}

fn read_limited(path: &Path, max: usize) -> Result<Vec<u8>> {
    let mut output = Vec::new();
    File::open(path)?
        .take(max as u64 + 1)
        .read_to_end(&mut output)?;
    if output.len() > max {
        return Err(invalid(format!(
            "{} exceeds the asset size limit",
            path.display()
        )));
    }
    Ok(output)
}

fn metadata(source: &Path, hash: &str) -> Result<Meta> {
    let mut name = source.as_os_str().to_owned();
    name.push(".telmeta");
    let path = PathBuf::from(name);
    match fs::symlink_metadata(&path) {
        Ok(info) if !info.is_file() => return Err(invalid(".telmeta must be a regular file")),
        Ok(_) => {}
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            let meta = Meta {
                id: AssetId::new_v4(),
                source_hash: hash.to_owned(),
                settings: ImportSettings::default(),
            };
            let bytes = toml::to_string_pretty(&meta).map_err(invalid)?;
            match write_atomic(&path, bytes.as_bytes(), false) {
                Ok(()) => return Ok(meta),
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
                Err(error) => return Err(error),
            }
        }
        Err(error) => return Err(error),
    }

    let source = read_limited(&path, 64 * 1024)?;
    let mut meta: Meta =
        toml::from_str(std::str::from_utf8(&source).map_err(invalid)?).map_err(invalid)?;
    if meta.source_hash != hash {
        meta.source_hash = hash.to_owned();
        let bytes = toml::to_string_pretty(&meta).map_err(invalid)?;
        write_atomic(&path, bytes.as_bytes(), true)?;
    }
    Ok(meta)
}

fn read_cache(path: &Path, revision: &AssetRevision) -> Result<Vec<u8>> {
    let bytes = read_limited(path, MAX_BYTES + MAGIC.len() + 64)?;
    let header = MAGIC.len() + 64;
    if bytes.len() < header
        || !bytes.starts_with(MAGIC)
        || bytes[MAGIC.len()..MAGIC.len() + 32] != revision.digest
        || bytes[MAGIC.len() + 32..header] != Sha256::digest(&bytes[header..])[..]
    {
        return Err(invalid("invalid asset cache header or checksum"));
    }
    let mut bytes = bytes;
    bytes.drain(..header);
    Ok(bytes)
}

fn write_cache(path: &Path, revision: &AssetRevision, payload: &[u8]) -> Result<()> {
    if payload.len() > MAX_BYTES {
        return Err(invalid("imported asset exceeds the cache size limit"));
    }
    let mut bytes = Vec::with_capacity(MAGIC.len() + 64 + payload.len());
    bytes.extend_from_slice(MAGIC);
    bytes.extend_from_slice(&revision.digest);
    bytes.extend_from_slice(&Sha256::digest(payload));
    bytes.extend_from_slice(payload);
    write_atomic(path, &bytes, true)
}

fn write_atomic(path: &Path, bytes: &[u8], replace: bool) -> Result<()> {
    let parent = path
        .parent()
        .ok_or_else(|| invalid("asset output has no parent directory"))?;
    let mut temp = tempfile::NamedTempFile::new_in(parent)?;
    temp.write_all(bytes)?;
    if replace {
        temp.persist(path).map_err(|error| error.error)?;
    } else {
        temp.persist_noclobber(path).map_err(|error| error.error)?;
    }
    Ok(())
}

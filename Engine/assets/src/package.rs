//! Versioned game archive shared by the exporter and the standalone runtime.

use std::collections::BTreeMap;
use std::fs::File;
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::PathBuf;
use std::sync::Arc;

use sha2::{Digest, Sha256};

use crate::formats::Codec;
use crate::{
    invalid, Asset, AssetId, AssetRevision, AudioClip, Entry, Handle, Mesh, ScriptAsset, Texture,
    MAX_BYTES,
};

const MAGIC: &[u8] = b"TELLURIUM-PAK\x01";
const FOOTER: &[u8] = b"TELLURIUM-GAME\x01";
const MAX_PACKAGE: usize = 512 * 1024 * 1024;
const MAX_ENTRIES: usize = 65_536;
pub const CONFIG: &str = "project.toml";

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
#[repr(u8)]
pub enum Kind {
    Project,
    Scene,
    Texture,
    Mesh,
    Audio,
    Script,
}

impl Kind {
    pub fn asset(path: &str) -> Option<Self> {
        let ext = path.rsplit_once('.')?.1.to_ascii_lowercase();
        [
            (Texture::EXTENSIONS, Self::Texture),
            (Mesh::EXTENSIONS, Self::Mesh),
            (AudioClip::EXTENSIONS, Self::Audio),
            (ScriptAsset::EXTENSIONS, Self::Script),
        ]
        .into_iter()
        .find_map(|(extensions, kind)| extensions.contains(&ext.as_str()).then_some(kind))
    }

    fn read(value: u8) -> io::Result<Self> {
        [
            Self::Project,
            Self::Scene,
            Self::Texture,
            Self::Mesh,
            Self::Audio,
            Self::Script,
        ]
        .get(usize::from(value))
        .copied()
        .ok_or_else(|| invalid("unknown .telpak entry type"))
    }
}

pub fn key(path: &str) -> io::Result<String> {
    let normalized = path.replace('\\', "/");
    if normalized.len() > 4096
        || normalized.contains([':', '\0'])
        || normalized
            .split('/')
            .any(|part| matches!(part, "" | "." | ".."))
    {
        return Err(invalid(format!("invalid package path {path:?}")));
    }
    Ok(normalized)
}

pub struct Writer<W> {
    output: W,
    hash: Sha256,
    size: usize,
    count: usize,
}

impl<W: Write> Writer<W> {
    pub fn new(output: W) -> io::Result<Self> {
        let mut writer = Self {
            output,
            hash: Sha256::new(),
            size: 0,
            count: 0,
        };
        writer.write(MAGIC)?;
        Ok(writer)
    }

    pub fn entry(&mut self, kind: Kind, path: &str, data: &[u8]) -> io::Result<()> {
        let name = key(path)?;
        if data.len() > MAX_BYTES || self.count == MAX_ENTRIES {
            return Err(invalid(format!("package entry limit exceeded at {name}")));
        }
        self.count += 1;
        self.write(&[kind as u8])?;
        self.write(&(name.len() as u16).to_le_bytes())?;
        self.write(&(data.len() as u32).to_le_bytes())?;
        self.write(name.as_bytes())?;
        self.write(data)
    }

    fn write(&mut self, bytes: &[u8]) -> io::Result<()> {
        self.size += bytes.len();
        if self.size > MAX_PACKAGE {
            return Err(invalid(".telpak exceeds 512 MiB"));
        }
        self.output.write_all(bytes)?;
        self.hash.update(bytes);
        Ok(())
    }

    pub fn finish(mut self) -> io::Result<[u8; 32]> {
        self.output.flush()?;
        Ok(self.hash.finalize().into())
    }
}

/// Bind an exported executable to the archive produced by the builder.
pub fn write_binding(mut exe: impl Write, digest: [u8; 32]) -> io::Result<()> {
    exe.write_all(&digest)?;
    exe.write_all(FOOTER)
}

/// Only the package matching the digest embedded in the running executable
/// can provide Luau bytecode. A loose archive is never a source of bytecode.
pub struct Package {
    data: Vec<u8>,
    entries: BTreeMap<(Kind, String), std::ops::Range<usize>>,
}

impl Package {
    pub fn open_game() -> io::Result<Option<Self>> {
        let exe_path = std::env::current_exe()?;
        let mut exe = File::open(&exe_path)?;
        let mut binding = [0; 32 + FOOTER.len()];
        exe.seek(SeekFrom::End(-(binding.len() as i64)))?;
        exe.read_exact(&mut binding)?;
        if &binding[32..] != FOOTER {
            return Ok(None);
        }

        let path = exe_path.with_extension("telpak");
        let mut data = Vec::new();
        File::open(&path)
            .map_err(|error| io::Error::new(error.kind(), format!("{}: {error}", path.display())))?
            .take(MAX_PACKAGE as u64 + 1)
            .read_to_end(&mut data)?;
        if data.len() > MAX_PACKAGE || Sha256::digest(&data)[..] != binding[..32] {
            return Err(invalid(format!(
                "{} does not match the executable",
                path.display()
            )));
        }
        Self::parse(data).map(Some)
    }

    fn parse(data: Vec<u8>) -> io::Result<Self> {
        let mut rest = data
            .strip_prefix(MAGIC)
            .ok_or_else(|| invalid("unsupported .telpak version"))?;
        let mut entries = BTreeMap::new();
        while !rest.is_empty() {
            let Some((header, body)) = rest.split_at_checked(7) else {
                return Err(invalid("truncated .telpak entry header"));
            };
            let kind = Kind::read(header[0])?;
            let name_len = u16::from_le_bytes(header[1..3].try_into().unwrap()) as usize;
            let size = u32::from_le_bytes(header[3..7].try_into().unwrap()) as usize;
            if size > MAX_BYTES || entries.len() == MAX_ENTRIES {
                return Err(invalid(".telpak entry limit exceeded"));
            }
            let Some((name, body)) = body.split_at_checked(name_len) else {
                return Err(invalid("truncated .telpak entry path"));
            };
            let name = key(std::str::from_utf8(name).map_err(invalid)?)?;
            let start = data.len() - body.len();
            rest = body
                .get(size..)
                .ok_or_else(|| invalid("truncated .telpak entry data"))?;
            if entries.insert((kind, name), start..start + size).is_some() {
                return Err(invalid("duplicate .telpak entry"));
            }
        }
        Ok(Self { data, entries })
    }

    pub fn bytes(&self, kind: Kind, path: &str) -> io::Result<&[u8]> {
        let name = key(path)?;
        self.entries
            .get(&(kind, name))
            .map(|range| &self.data[range.clone()])
            .ok_or_else(|| invalid(format!("missing {kind:?} {path:?} in .telpak")))
    }

    pub fn asset<T: Asset>(&self, kind: Kind, path: &str) -> io::Result<Handle<T>> {
        let extension = path.rsplit_once('.').map_or("", |(_, ext)| ext);
        if kind == Kind::Script
            || Kind::asset(path) != Some(kind)
            || !T::EXTENSIONS
                .iter()
                .any(|ext| ext.eq_ignore_ascii_case(extension))
        {
            return Err(invalid(format!("asset type does not match {path}")));
        }
        let payload = self.bytes(kind, path)?;
        Ok(handle(payload, T::decode(payload.to_vec(), None)?))
    }

    pub fn script(&self, path: &str) -> io::Result<Handle<ScriptAsset>> {
        let payload = self.bytes(Kind::Script, path)?;
        Ok(handle(
            payload,
            ScriptAsset {
                bytecode: payload.to_vec(),
            },
        ))
    }
}

fn handle<T>(payload: &[u8], data: T) -> Handle<T> {
    let digest: [u8; 32] = Sha256::digest(payload).into();
    Handle(Arc::new(Entry {
        revision: AssetRevision {
            id: AssetId::from_bytes(digest[..16].try_into().unwrap()),
            digest,
        },
        cache_path: PathBuf::new(),
        data,
    }))
}

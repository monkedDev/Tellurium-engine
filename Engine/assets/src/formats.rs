use std::collections::HashMap;
use std::io::{self, Cursor};

use bincode::Options;
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use symphonia::core::audio::SampleBuffer;
use symphonia::core::codecs::DecoderOptions;
use symphonia::core::errors::Error as AudioError;
use symphonia::core::formats::FormatOptions;
use symphonia::core::io::{MediaSourceStream, MediaSourceStreamOptions};
use symphonia::core::meta::MetadataOptions;
use symphonia::core::probe::Hint;

use crate::{invalid, ImportSettings, MAX_BYTES};

// Only this module can produce proof that a bytecode buffer was locally compiled.
pub struct VerifiedScript(Vec<u8>);

pub trait Codec: Sized {
    const EXTENSIONS: &'static [&'static str];

    fn verify_source(_source: &[u8]) -> io::Result<Option<VerifiedScript>> {
        Ok(None)
    }

    fn import(
        source: &[u8],
        extension: &str,
        settings: &ImportSettings,
        verified: Option<&VerifiedScript>,
    ) -> io::Result<Vec<u8>>;

    fn decode(payload: Vec<u8>, verified: Option<&VerifiedScript>) -> io::Result<Self>;
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Texture {
    pub width: u32,
    pub height: u32,
    pub rgba8: Vec<u8>,
}

#[derive(Debug, Copy, Clone, Serialize, Deserialize)]
pub struct MeshVertex {
    pub position: [f32; 3],
    pub color: [f32; 4],
    pub uv: [f32; 2],
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Mesh {
    pub vertices: Vec<MeshVertex>,
    pub indices: Vec<u32>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct AudioClip {
    pub sample_rate: u32,
    pub channels: u32,
    pub samples: Vec<f32>,
}

/// Bytecode from the local compiler or a package bound to the current executable.
pub struct ScriptAsset {
    pub(crate) bytecode: Vec<u8>,
}

impl ScriptAsset {
    pub fn bytecode(&self) -> &[u8] {
        &self.bytecode
    }
}

fn options() -> impl Options {
    bincode::DefaultOptions::new()
        .with_fixint_encoding()
        .with_little_endian()
        .with_limit(MAX_BYTES as u64)
        .reject_trailing_bytes()
}

fn encode(data: &impl Serialize) -> io::Result<Vec<u8>> {
    options().serialize(data).map_err(invalid)
}

fn decode<T: DeserializeOwned>(data: &[u8]) -> io::Result<T> {
    options().deserialize(data).map_err(invalid)
}

impl Codec for Texture {
    const EXTENSIONS: &'static [&'static str] = &["png"];

    fn import(
        source: &[u8],
        _: &str,
        settings: &ImportSettings,
        _: Option<&VerifiedScript>,
    ) -> io::Result<Vec<u8>> {
        let mut decoder =
            png::Decoder::new_with_limits(Cursor::new(source), png::Limits { bytes: MAX_BYTES });
        decoder.set_transformations(png::Transformations::EXPAND | png::Transformations::STRIP_16);
        let mut reader = decoder.read_info().map_err(invalid)?;
        let width = reader.info().width;
        let height = reader.info().height;
        let pixels = u64::from(width) * u64::from(height);
        if pixels > (MAX_BYTES / 4 - 8) as u64 || width > 16384 || height > 16384 {
            return Err(invalid("PNG dimensions exceed the asset limit"));
        }
        let mut decoded = vec![0; reader.output_buffer_size()];
        let frame = reader.next_frame(&mut decoded).map_err(invalid)?;
        let channels = frame.color_type.samples();
        let row_size = frame.width as usize * channels;

        let mut rgba8 = Vec::with_capacity(pixels as usize * 4);
        for row in 0..frame.height as usize {
            let index = if settings.flip_vertical {
                frame.height as usize - 1 - row
            } else {
                row
            };
            for pixel in decoded[index * row_size..(index + 1) * row_size].chunks_exact(channels) {
                let color = match frame.color_type {
                    png::ColorType::Grayscale => [pixel[0], pixel[0], pixel[0], 255],
                    png::ColorType::GrayscaleAlpha => [pixel[0], pixel[0], pixel[0], pixel[1]],
                    png::ColorType::Rgb => [pixel[0], pixel[1], pixel[2], 255],
                    png::ColorType::Rgba => [pixel[0], pixel[1], pixel[2], pixel[3]],
                    png::ColorType::Indexed => return Err(invalid("PNG palette was not expanded")),
                };
                rgba8.extend_from_slice(&color);
            }
        }
        encode(&Self {
            width: frame.width,
            height: frame.height,
            rgba8,
        })
    }

    fn decode(payload: Vec<u8>, _: Option<&VerifiedScript>) -> io::Result<Self> {
        let texture: Self = decode(&payload)?;
        if texture.width == 0
            || texture.height == 0
            || texture.width > 16384
            || texture.height > 16384
            || texture.rgba8.len() as u64
                != u64::from(texture.width) * u64::from(texture.height) * 4
        {
            return Err(invalid("invalid cached RGBA8 texture"));
        }
        Ok(texture)
    }
}

impl Codec for Mesh {
    const EXTENSIONS: &'static [&'static str] = &["fbx"];

    fn import(
        source: &[u8],
        _: &str,
        _: &ImportSettings,
        _: Option<&VerifiedScript>,
    ) -> io::Result<Vec<u8>> {
        let scene = ufbx::load_memory(
            source,
            ufbx::LoadOpts {
                ignore_animation: true,
                ignore_embedded: true,
                target_axes: ufbx::CoordinateAxes::left_handed_y_up(),
                target_unit_meters: 1.0,
                temp_allocator: ufbx::AllocatorOpts {
                    memory_limit: MAX_BYTES,
                    ..Default::default()
                },
                result_allocator: ufbx::AllocatorOpts {
                    memory_limit: MAX_BYTES,
                    ..Default::default()
                },
                ..Default::default()
            },
        )
        .map_err(|error| invalid(format!("FBX import: {error:?}")))?;

        let mut vertices = Vec::new();
        let mut indices = Vec::new();
        let mut corners = Vec::new();
        let mut unique = HashMap::new();
        for node in &scene.nodes {
            let Some(mesh) = node.mesh.as_ref() else {
                continue;
            };
            if !mesh.vertex_position.exists {
                continue;
            }
            for &face in &mesh.faces {
                if face.num_indices < 3 {
                    continue;
                }
                corners.resize((face.num_indices as usize - 2) * 3, 0);
                let count = ufbx::triangulate_face(&mut corners, mesh, face) as usize * 3;
                for &corner in &corners[..count] {
                    let corner = corner as usize;
                    let p = ufbx::transform_position(
                        &node.geometry_to_world,
                        mesh.vertex_position[corner],
                    );
                    let uv = if mesh.vertex_uv.exists {
                        let tex = mesh.vertex_uv[corner];
                        [tex.x as f32, 1.0 - tex.y as f32]
                    } else {
                        [0.0; 2]
                    };
                    let color = if mesh.vertex_color.exists {
                        let color = mesh.vertex_color[corner];
                        [
                            color.x as f32,
                            color.y as f32,
                            color.z as f32,
                            color.w as f32,
                        ]
                    } else {
                        [1.0; 4]
                    };
                    let vertex = MeshVertex {
                        position: [p.x as f32, p.y as f32, p.z as f32],
                        color,
                        uv,
                    };
                    let key = [
                        vertex.position[0],
                        vertex.position[1],
                        vertex.position[2],
                        color[0],
                        color[1],
                        color[2],
                        color[3],
                        uv[0],
                        uv[1],
                    ]
                    .map(f32::to_bits);
                    let index = *unique.entry(key).or_insert_with(|| {
                        let index = vertices.len() as u32;
                        vertices.push(vertex);
                        index
                    });
                    indices.push(index);
                    if vertices.len() * 36 + indices.len() * 4 > MAX_BYTES - 16 {
                        return Err(invalid("imported mesh exceeds the asset limit"));
                    }
                }
            }
        }
        if indices.is_empty() {
            return Err(invalid("FBX contains no mesh triangles"));
        }
        encode(&Self { vertices, indices })
    }

    fn decode(payload: Vec<u8>, _: Option<&VerifiedScript>) -> io::Result<Self> {
        let mesh: Self = decode(&payload)?;
        if mesh.indices.is_empty()
            || !mesh.indices.len().is_multiple_of(3)
            || mesh
                .indices
                .iter()
                .any(|index| *index as usize >= mesh.vertices.len())
            || mesh.vertices.iter().any(|vertex| {
                vertex
                    .position
                    .iter()
                    .chain(&vertex.color)
                    .chain(&vertex.uv)
                    .any(|v| !v.is_finite())
            })
        {
            return Err(invalid("invalid cached indexed mesh"));
        }
        Ok(mesh)
    }
}

impl Codec for AudioClip {
    const EXTENSIONS: &'static [&'static str] = &["wav", "ogg"];

    fn import(
        source: &[u8],
        extension: &str,
        settings: &ImportSettings,
        _: Option<&VerifiedScript>,
    ) -> io::Result<Vec<u8>> {
        let stream = MediaSourceStream::new(
            Box::new(Cursor::new(source.to_vec())),
            MediaSourceStreamOptions::default(),
        );
        let mut hint = Hint::new();
        hint.with_extension(extension);
        let mut format = symphonia::default::get_probe()
            .format(
                &hint,
                stream,
                &FormatOptions::default(),
                &MetadataOptions::default(),
            )
            .map_err(invalid)?
            .format;
        let track = format
            .default_track()
            .ok_or_else(|| invalid("audio has no default track"))?;
        let track_id = track.id;
        let mut decoder = symphonia::default::get_codecs()
            .make(&track.codec_params, &DecoderOptions::default())
            .map_err(invalid)?;
        let mut samples = Vec::new();
        let mut spec = None;
        loop {
            let packet = match format.next_packet() {
                Ok(packet) => packet,
                Err(AudioError::IoError(error)) if error.kind() == io::ErrorKind::UnexpectedEof => {
                    break
                }
                Err(error) => return Err(invalid(error)),
            };
            if packet.track_id() != track_id {
                continue;
            }
            let decoded = decoder.decode(&packet).map_err(invalid)?;
            let format = *decoded.spec();
            if spec.is_some_and(|previous| previous != format) {
                return Err(invalid("audio format changes between packets"));
            }
            spec = Some(format);
            let channels = format.channels.count();
            let mut buffer = SampleBuffer::<f32>::new(decoded.capacity() as u64, format);
            buffer.copy_interleaved_ref(decoded);
            let added = if settings.audio_mono {
                buffer.samples().len() / channels
            } else {
                buffer.samples().len()
            };
            if samples.len() + added > MAX_BYTES / 4 - 8 {
                return Err(invalid("decoded audio exceeds the asset limit"));
            }
            if settings.audio_mono {
                samples.extend(
                    buffer
                        .samples()
                        .chunks_exact(channels)
                        .map(|frame| frame.iter().sum::<f32>() / channels as f32),
                );
            } else {
                samples.extend_from_slice(buffer.samples());
            }
        }
        let spec = spec.ok_or_else(|| invalid("audio had no decodable samples"))?;
        let channels = if settings.audio_mono {
            1
        } else {
            spec.channels.count() as u32
        };
        encode(&Self {
            sample_rate: spec.rate,
            channels,
            samples,
        })
    }

    fn decode(payload: Vec<u8>, _: Option<&VerifiedScript>) -> io::Result<Self> {
        let clip: Self = decode(&payload)?;
        if clip.channels == 0
            || clip.channels > 32
            || clip.sample_rate == 0
            || clip.sample_rate > 384000
            || clip.samples.is_empty()
            || !clip.samples.len().is_multiple_of(clip.channels as usize)
            || clip.samples.iter().any(|sample| !sample.is_finite())
        {
            return Err(invalid("invalid cached interleaved PCM"));
        }
        Ok(clip)
    }
}

impl Codec for ScriptAsset {
    const EXTENSIONS: &'static [&'static str] = &["luau"];

    fn verify_source(source: &[u8]) -> io::Result<Option<VerifiedScript>> {
        if source.len() > 1024 * 1024 {
            return Err(invalid("Luau source exceeds 1 MiB"));
        }
        let source = std::str::from_utf8(source).map_err(invalid)?;
        let bytecode = mlua::Compiler::new()
            .set_optimization_level(2)
            .compile(source)
            .map_err(invalid)?;
        Ok(Some(VerifiedScript(bytecode)))
    }

    fn import(
        _: &[u8],
        _: &str,
        _: &ImportSettings,
        verified: Option<&VerifiedScript>,
    ) -> io::Result<Vec<u8>> {
        Ok(verified
            .ok_or_else(|| invalid("missing verified Luau bytecode"))?
            .0
            .clone())
    }

    fn decode(payload: Vec<u8>, verified: Option<&VerifiedScript>) -> io::Result<Self> {
        // Luau only accepts trusted bytecode. A cache edited on disk must match
        // the output of this process's compiler for the same source.
        if verified.map(|proof| proof.0.as_slice()) != Some(payload.as_slice()) {
            return Err(invalid(
                "cached Luau bytecode does not match the local compiler",
            ));
        }
        Ok(Self { bytecode: payload })
    }
}

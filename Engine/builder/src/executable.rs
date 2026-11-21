use std::fs::{self, OpenOptions};
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::os::windows::ffi::OsStrExt;
use std::path::Path;

use windows_sys::Win32::System::LibraryLoader::{
    BeginUpdateResourceW, EndUpdateResourceW, UpdateResourceW,
};

use crate::invalid;

pub(super) fn set_console(path: &Path, console: bool) -> io::Result<()> {
    let mut exe = OpenOptions::new().read(true).write(true).open(path)?;
    let mut dos = [0; 64];
    exe.read_exact(&mut dos)?;
    if &dos[..2] != b"MZ" {
        return Err(invalid("Invalid Windows runtime template"));
    }
    let pe = u32::from_le_bytes(dos[60..64].try_into().unwrap()) as u64;
    exe.seek(SeekFrom::Start(pe))?;
    let mut headers = [0; 26];
    exe.read_exact(&mut headers)?;
    let optional_size = u16::from_le_bytes(headers[20..22].try_into().unwrap());
    let magic = u16::from_le_bytes(headers[24..26].try_into().unwrap());
    if &headers[..4] != b"PE\0\0" || optional_size < 70 || !matches!(magic, 0x10b | 0x20b) {
        return Err(invalid("Invalid runtime PE header"));
    }
    // IMAGE_OPTIONAL_HEADER.Subsystem has the same offset in PE32 and PE32+.
    let subsystem: u16 = if console { 3 } else { 2 };
    exe.seek(SeekFrom::Start(pe + 24 + 68))?;
    exe.write_all(&subsystem.to_le_bytes())
}

const RT_ICON: usize = 3;
const RT_GROUP_ICON: usize = 14;

pub(super) fn set_icon(exe: &Path, icon: &Path) -> io::Result<()> {
    let mut bytes = Vec::new();
    fs::File::open(icon)?
        .take(16 * 1024 * 1024 + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() > 16 * 1024 * 1024 || bytes.get(..4) != Some(&[0, 0, 1, 0]) {
        return Err(invalid("Expected a Windows .ico file up to 16 MiB"));
    }
    let header = bytes
        .get(..6)
        .ok_or_else(|| invalid("Truncated ICO header"))?;
    let count = u16::from_le_bytes(header[4..6].try_into().unwrap());
    let table_end = 6 + usize::from(count) * 16;
    let directory = bytes
        .get(6..table_end)
        .filter(|_| count > 0)
        .ok_or_else(|| invalid("Invalid ICO directory"))?;
    let mut images = Vec::with_capacity(count.into());
    let mut group = header.to_vec();
    for (entry, id) in directory.chunks_exact(16).zip(1..=count) {
        let size = u32::from_le_bytes(entry[8..12].try_into().unwrap()) as usize;
        let offset = u32::from_le_bytes(entry[12..16].try_into().unwrap()) as usize;
        let image = bytes
            .get(offset..)
            .and_then(|rest| rest.get(..size))
            .filter(|_| size > 0 && offset >= table_end)
            .ok_or_else(|| invalid("Invalid ICO image size or offset"))?;
        images.push((id, image));
        group.extend_from_slice(&entry[..12]);
        group.extend_from_slice(&id.to_le_bytes());
    }

    let path: Vec<u16> = exe.as_os_str().encode_wide().chain(Some(0)).collect();
    // SAFETY: The path is a live NUL-terminated UTF-16 string. The returned
    // handle belongs to this transaction and is closed exactly once below.
    let update = unsafe { BeginUpdateResourceW(path.as_ptr(), 0) };
    if update.is_null() {
        return Err(io::Error::last_os_error());
    }
    let add = |kind: usize, id: u16, data: &[u8]| {
        // SAFETY: Windows interprets type and name as integer resource IDs.
        // The data slice is valid for the entire call.
        let result = unsafe {
            UpdateResourceW(
                update,
                kind as _,
                usize::from(id) as _,
                0,
                data.as_ptr().cast(),
                data.len() as u32,
            )
        };
        if result == 0 {
            Err(io::Error::last_os_error())
        } else {
            Ok(())
        }
    };
    let result = images
        .into_iter()
        .try_for_each(|(id, image)| add(RT_ICON, id, image))
        .and_then(|_| add(RT_GROUP_ICON, 1, &group));
    // SAFETY: This consumes the valid update handle. Failed edits are discarded.
    let ended = unsafe { EndUpdateResourceW(update, i32::from(result.is_err())) };
    result?;
    if ended == 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

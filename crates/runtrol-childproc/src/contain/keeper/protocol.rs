//! Small private structural frames. No provider argument or terminal byte enters this lane.

use std::io::{BufRead, Read, Write};
use std::os::windows::ffi::{OsStrExt as _, OsStringExt as _};

use super::{KeeperLimits, KeeperTarget};
use crate::SpawnError;
use crate::contain::job::failure;

pub(super) const MAX_TARGET_UNITS: usize = crate::argv::MAX_ARGUMENT_LEN;
const MAX_FRAME_BYTES: usize = 256;

pub(super) fn line(reader: &mut impl BufRead) -> Result<String, SpawnError> {
    let mut line = String::with_capacity(MAX_FRAME_BYTES);
    let count = reader
        .take((MAX_FRAME_BYTES + 1) as u64)
        .read_line(&mut line)
        .map_err(|error| failure("reading private keeper control", error.to_string()))?;
    if count == 0 || count > MAX_FRAME_BYTES || !line.ends_with('\n') {
        return Err(failure(
            "reading private keeper control",
            "incomplete bounded frame",
        ));
    }
    Ok(line.trim_end().to_owned())
}

pub(super) fn answer(writer: &mut impl Write, text: &str) -> Result<(), SpawnError> {
    if text.len() >= MAX_FRAME_BYTES {
        return Err(failure(
            "writing private keeper control",
            "structural frame exceeded its bound",
        ));
    }
    writeln!(writer, "{text}")
        .and_then(|()| writer.flush())
        .map_err(|error| failure("writing private keeper control", error.to_string()))
}

pub(super) fn number<T: std::str::FromStr>(word: Option<&str>) -> Result<T, SpawnError> {
    word.ok_or_else(|| {
        failure(
            "reading private keeper control",
            "missing structural number",
        )
    })?
    .parse()
    .map_err(|_| {
        failure(
            "reading private keeper control",
            "invalid structural number",
        )
    })
}

pub(super) fn write_target(
    writer: &mut impl Write,
    target: &KeeperTarget,
    limits: KeeperLimits,
) -> Result<(), SpawnError> {
    let path: Vec<u16> = target.directory.as_os_str().encode_wide().collect();
    let units = u32::try_from(path.len())
        .map_err(|_| failure("registering keeper target", "directory is too long"))?;
    let mut frame = Vec::with_capacity(32 + path.len() * 2);
    frame.extend_from_slice(&units.to_le_bytes());
    frame.extend_from_slice(&limits.terminals.to_le_bytes());
    frame.extend_from_slice(&limits.commands.to_le_bytes());
    frame.extend_from_slice(&target.identity);
    for unit in path {
        frame.extend_from_slice(&unit.to_le_bytes());
    }
    writer
        .write_all(&frame)
        .and_then(|()| writer.flush())
        .map_err(|error| failure("registering keeper target", error.to_string()))
}

pub(super) fn read_target(
    reader: &mut impl Read,
) -> Result<(KeeperTarget, KeeperLimits), SpawnError> {
    let mut size = [0; 4];
    let mut terminals = [0; 2];
    let mut commands = [0; 2];
    let mut identity = [0; 24];
    for field in [
        size.as_mut_slice(),
        terminals.as_mut_slice(),
        commands.as_mut_slice(),
        identity.as_mut_slice(),
    ] {
        reader
            .read_exact(field)
            .map_err(|error| failure("reading keeper target", error.to_string()))?;
    }
    let units = u32::from_le_bytes(size) as usize;
    if units == 0 || units > MAX_TARGET_UNITS {
        return Err(failure(
            "reading keeper target",
            "directory exceeded its bound",
        ));
    }
    let limits = KeeperLimits::new(u16::from_le_bytes(terminals), u16::from_le_bytes(commands))?;
    let mut bytes = vec![0; units * 2];
    reader
        .read_exact(&mut bytes)
        .map_err(|error| failure("reading keeper target", error.to_string()))?;
    let path: Vec<_> = bytes
        .chunks_exact(2)
        .map(|pair| {
            // chunks_exact establishes the two-byte frame. No external length indexes this slice.
            let mut unit = [0; 2];
            unit.copy_from_slice(pair);
            u16::from_le_bytes(unit)
        })
        .collect();
    let target = KeeperTarget::new(std::ffi::OsString::from_wide(&path).into(), identity)?;
    Ok((target, limits))
}

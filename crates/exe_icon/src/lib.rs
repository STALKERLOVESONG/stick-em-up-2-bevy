//! Build helper for the game's and the launcher's `build.rs`: give the programs the original
//! game's icon. It is read from the player's own copy of `SEU2.exe` (the icon is the game's art,
//! so it is not in the repository) and written as a compiled resource file (`.res`), which the
//! MSVC linker and `rust-lld` both take as an input. Without the original, the remake's own
//! drawn icon (`packaging/icons/*.res`) is used.

use std::io;
use std::path::{Path, PathBuf};

const RT_ICON: u32 = 3;
const RT_GROUP_ICON: u32 = 14;
const LANGUAGE_EN_US: u16 = 0x0409;

/// Where the original is: `STICK_ORIGINAL_EXE`, else Steam's default library folders and the
/// libraries `libraryfolders.vdf` lists.
pub fn find_original_exe() -> Option<PathBuf> {
    if let Some(p) = std::env::var_os("STICK_ORIGINAL_EXE").map(PathBuf::from) {
        return p.is_file().then_some(p);
    }
    let mut steams: Vec<PathBuf> = ["C:\\Program Files (x86)\\Steam", "C:\\Program Files\\Steam"].iter().map(PathBuf::from).collect();
    if let Some(p) = std::env::var_os("STEAM_DIR") {
        steams.insert(0, PathBuf::from(p));
    }
    let mut libraries = Vec::new();
    for steam in &steams {
        libraries.push(steam.clone());
        if let Ok(text) = std::fs::read_to_string(steam.join("steamapps").join("libraryfolders.vdf")) {
            libraries.extend(library_paths(&text));
        }
    }
    libraries.into_iter().map(|l| l.join("steamapps").join("common").join("Stick 'Em Up 2").join("SEU2.exe")).find(|p| p.is_file())
}

/// `"path"  "D:\\SteamLibrary"` lines of `libraryfolders.vdf`.
fn library_paths(vdf: &str) -> Vec<PathBuf> {
    vdf.lines()
        .filter_map(|line| {
            let mut quoted = line.split('"').skip(1).step_by(2);
            (quoted.next()? == "path").then(|| PathBuf::from(quoted.next().unwrap_or("").replace("\\\\", "\\")))
        })
        .collect()
}

fn u16_at(b: &[u8], at: usize) -> io::Result<u16> {
    b.get(at..at + 2).map(|s| u16::from_le_bytes([s[0], s[1]])).ok_or_else(truncated)
}

fn u32_at(b: &[u8], at: usize) -> io::Result<u32> {
    b.get(at..at + 4).map(|s| u32::from_le_bytes([s[0], s[1], s[2], s[3]])).ok_or_else(truncated)
}

fn truncated() -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, "not a Windows program (truncated)")
}

/// One resource: type, id (named resources are skipped) and its bytes.
struct Resource {
    kind: u32,
    id: u32,
    data: Vec<u8>,
}

/// Every id-named resource of a PE file (first language of each).
fn resources(pe: &[u8]) -> io::Result<Vec<Resource>> {
    if pe.get(..2) != Some(b"MZ") {
        return Err(truncated());
    }
    let header = u32_at(pe, 0x3C)? as usize;
    if pe.get(header..header + 4) != Some(b"PE\0\0") {
        return Err(truncated());
    }
    let coff = header + 4;
    let sections = u16_at(pe, coff + 2)? as usize;
    let optional = coff + 20;
    let optional_size = u16_at(pe, coff + 16)? as usize;
    let directories = match u16_at(pe, optional)? {
        0x10b => optional + 96,
        0x20b => optional + 112,
        _ => return Err(truncated()),
    };
    let resource_rva = u32_at(pe, directories + 2 * 8)?;
    if resource_rva == 0 {
        return Ok(Vec::new());
    }
    let table = optional + optional_size;
    let section_of = |rva: u32| -> io::Result<usize> {
        for i in 0..sections {
            let s = table + i * 40;
            let size = u32_at(pe, s + 8)?.max(u32_at(pe, s + 16)?);
            let address = u32_at(pe, s + 12)?;
            if rva >= address && rva < address + size {
                return Ok((rva - address + u32_at(pe, s + 20)?) as usize);
            }
        }
        Err(truncated())
    };
    let base = section_of(resource_rva)?;
    let entries = |dir: usize| -> io::Result<Vec<(u32, u32)>> {
        let at = base + dir;
        let count = u16_at(pe, at + 12)? as usize + u16_at(pe, at + 14)? as usize;
        (0..count).map(|i| Ok((u32_at(pe, at + 16 + i * 8)?, u32_at(pe, at + 20 + i * 8)?))).collect()
    };
    let mut out = Vec::new();
    for (kind, types) in entries(0)? {
        if kind & 0x8000_0000 != 0 || types & 0x8000_0000 == 0 {
            continue;
        }
        for (id, names) in entries((types & 0x7FFF_FFFF) as usize)? {
            if id & 0x8000_0000 != 0 || names & 0x8000_0000 == 0 {
                continue;
            }
            let Some(&(_, leaf)) = entries((names & 0x7FFF_FFFF) as usize)?.first() else { continue };
            if leaf & 0x8000_0000 != 0 {
                continue;
            }
            let entry = base + leaf as usize;
            let start = section_of(u32_at(pe, entry)?)?;
            let size = u32_at(pe, entry + 4)? as usize;
            let data = pe.get(start..start + size).ok_or_else(truncated)?.to_vec();
            out.push(Resource { kind, id, data });
        }
    }
    Ok(out)
}

/// The program's icon (its first icon group with the images it lists) as a `.res` file.
pub fn icon_res_from_exe(exe: &Path) -> io::Result<Option<Vec<u8>>> {
    let all = resources(&std::fs::read(exe)?)?;
    let Some(group) = all.iter().find(|r| r.kind == RT_GROUP_ICON) else { return Ok(None) };
    let count = u16_at(&group.data, 4)? as usize;
    let mut images = Vec::new();
    for i in 0..count {
        let id = u16_at(&group.data, 6 + i * 14 + 12)? as u32;
        let image = all.iter().find(|r| r.kind == RT_ICON && r.id == id).ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, format!("icon image {id} missing")))?;
        images.push((id as u16, image.data.as_slice()));
    }
    Ok(Some(res_file(&group.data, &images)))
}

fn res_entry(out: &mut Vec<u8>, kind: u16, id: u16, data: &[u8], flags: u16) {
    out.extend_from_slice(&(data.len() as u32).to_le_bytes());
    out.extend_from_slice(&32u32.to_le_bytes());
    for v in [0xFFFF, kind, 0xFFFF, id] {
        out.extend_from_slice(&v.to_le_bytes());
    }
    out.extend_from_slice(&0u32.to_le_bytes()); // data version
    out.extend_from_slice(&flags.to_le_bytes());
    out.extend_from_slice(&LANGUAGE_EN_US.to_le_bytes());
    out.extend_from_slice(&[0; 8]); // version, characteristics
    out.extend_from_slice(data);
    while out.len() % 4 != 0 {
        out.push(0);
    }
}

/// A compiled resource file: the empty leading entry, the images (`RT_ICON`) and the group
/// (`RT_GROUP_ICON` 1, the icon Explorer shows).
fn res_file(group: &[u8], images: &[(u16, &[u8])]) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(&0u32.to_le_bytes());
    out.extend_from_slice(&32u32.to_le_bytes());
    for v in [0xFFFFu16, 0, 0xFFFF, 0] {
        out.extend_from_slice(&v.to_le_bytes());
    }
    out.extend_from_slice(&[0; 16]);
    for (id, data) in images {
        res_entry(&mut out, RT_ICON as u16, *id, data, 0x1010);
    }
    res_entry(&mut out, RT_GROUP_ICON as u16, 1, group, 0x1030);
    out
}

/// For a `build.rs`: link the original's icon, else `fallback` (a `.res` file); MSVC targets only.
pub fn link_icon(fallback: &Path) {
    if std::env::var("CARGO_CFG_TARGET_ENV").as_deref() != Ok("msvc") {
        return;
    }
    println!("cargo::rerun-if-env-changed=STICK_ORIGINAL_EXE");
    println!("cargo::rerun-if-changed={}", fallback.display());
    let out_dir = PathBuf::from(std::env::var_os("OUT_DIR").expect("OUT_DIR"));
    let original = find_original_exe().and_then(|exe| match icon_res_from_exe(&exe) {
        Ok(res) => res,
        Err(e) => {
            println!("cargo::warning=icon from {}: {e}", exe.display());
            None
        }
    });
    let res = match original {
        Some(bytes) => {
            let path = out_dir.join("original_icon.res");
            if std::fs::write(&path, bytes).is_err() {
                return;
            }
            path
        }
        None if fallback.is_file() => fallback.to_path_buf(),
        None => return,
    };
    println!("cargo::rustc-link-arg-bins={}", res.display());
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn library_folders() {
        let vdf = "\"libraryfolders\"\n{\n\t\"0\"\n\t{\n\t\t\"path\"\t\t\"C:\\\\Program Files (x86)\\\\Steam\"\n\t}\n\t\"1\"\n\t{\n\t\t\"path\"\t\t\"D:\\\\SteamLibrary\"\n\t}\n}";
        assert_eq!(library_paths(vdf), vec![PathBuf::from("C:\\Program Files (x86)\\Steam"), PathBuf::from("D:\\SteamLibrary")]);
    }

    #[test]
    fn not_a_program() {
        assert!(resources(b"hello").is_err());
    }
}

//! Read host Mach-O load commands without requiring Xcode tools or running the binary.
use std::{
    collections::BTreeSet,
    fs::File,
    io::{Read, Seek, SeekFrom},
    path::{Path, PathBuf},
};

const MAX_COMMAND_BYTES: usize = 1024 * 1024;
const MAX_IMAGES: usize = 128;

#[derive(Default)]
struct Image {
    libraries: Vec<String>,
    rpaths: Vec<String>,
}

/// Return only existing dependency files, never their containing directories.
pub(super) fn libraries(executable: &Path) -> Vec<PathBuf> {
    let Some(executable_directory) = executable.parent() else {
        return Vec::new();
    };
    let mut pending = vec![(executable.to_path_buf(), Vec::<PathBuf>::new())];
    let mut visited = BTreeSet::new();
    let mut libraries = BTreeSet::new();
    while let Some((path, inherited)) = pending.pop() {
        if visited.len() >= MAX_IMAGES {
            break;
        }
        if !visited.insert(path.clone()) {
            continue;
        }
        let Some(image) = read_image(&path) else {
            continue;
        };
        let loader = path.parent().unwrap_or(executable_directory);
        let mut rpaths = image
            .rpaths
            .iter()
            .filter_map(|path| expand(path, loader, executable_directory))
            .collect::<Vec<_>>();
        rpaths.extend(inherited);
        for name in image.libraries {
            if libraries.len() >= MAX_IMAGES {
                break;
            }
            let candidates = if let Some(relative) = name.strip_prefix("@rpath/") {
                rpaths.iter().map(|root| root.join(relative)).collect()
            } else {
                expand(&name, loader, executable_directory)
                    .into_iter()
                    .collect::<Vec<_>>()
            };
            if let Some(library) = candidates
                .into_iter()
                .find_map(|candidate| candidate.canonicalize().ok().filter(|path| path.is_file()))
            {
                if library != executable
                    && !libraries.contains(&library)
                    && read_image(&library).is_some()
                    && libraries.insert(library.clone())
                {
                    pending.push((library, rpaths.clone()));
                }
            }
        }
    }
    libraries.into_iter().collect()
}

fn expand(name: &str, loader: &Path, executable: &Path) -> Option<PathBuf> {
    if name == "@loader_path" {
        Some(loader.to_path_buf())
    } else if name == "@executable_path" {
        Some(executable.to_path_buf())
    } else if let Some(relative) = name.strip_prefix("@loader_path/") {
        Some(loader.join(relative))
    } else if let Some(relative) = name.strip_prefix("@executable_path/") {
        Some(executable.join(relative))
    } else {
        let path = PathBuf::from(name);
        path.is_absolute().then_some(path)
    }
}

fn integer(bytes: &[u8], little: bool) -> Option<u32> {
    let bytes = bytes.get(..4)?.try_into().ok()?;
    Some(if little {
        u32::from_le_bytes(bytes)
    } else {
        u32::from_be_bytes(bytes)
    })
}

fn read_image(path: &Path) -> Option<Image> {
    let mut file = File::open(path).ok()?;
    let length = file.metadata().ok()?.len();
    let mut magic = [0; 8];
    file.read_exact(&mut magic).ok()?;
    let mut offset = 0;
    let mut image_end = length;
    let fat = match magic[..4] {
        [0xca, 0xfe, 0xba, 0xbe] => Some((false, false)),
        [0xbe, 0xba, 0xfe, 0xca] => Some((true, false)),
        [0xca, 0xfe, 0xba, 0xbf] => Some((false, true)),
        [0xbf, 0xba, 0xfe, 0xca] => Some((true, true)),
        _ => None,
    };
    if let Some((little, wide)) = fat {
        let count = integer(&magic[4..], little)? as usize;
        if !(1..=64).contains(&count) {
            return None;
        }
        let size = if wide { 32 } else { 20 };
        let mut table = vec![0; count * size];
        file.read_exact(&mut table).ok()?;
        let cpu = if cfg!(target_arch = "aarch64") {
            0x0100_000c
        } else if cfg!(target_arch = "x86_64") {
            0x0100_0007
        } else {
            return None;
        };
        let arch = table
            .chunks_exact(size)
            .find(|arch| integer(arch, little) == Some(cpu))?;
        let (start, size) = if wide {
            let read = |bytes: &[u8]| -> Option<u64> {
                let bytes = bytes.try_into().ok()?;
                Some(if little {
                    u64::from_le_bytes(bytes)
                } else {
                    u64::from_be_bytes(bytes)
                })
            };
            (read(&arch[8..16])?, read(&arch[16..24])?)
        } else {
            (
                integer(&arch[8..], little)? as u64,
                integer(&arch[12..], little)? as u64,
            )
        };
        offset = start;
        image_end = start.checked_add(size)?;
        if image_end > length || size < 28 {
            return None;
        }
    }
    file.seek(SeekFrom::Start(offset)).ok()?;
    let mut header = [0; 28];
    file.read_exact(&mut header).ok()?;
    let (little, wide) = match header[..4] {
        [0xce, 0xfa, 0xed, 0xfe] => (true, false),
        [0xcf, 0xfa, 0xed, 0xfe] => (true, true),
        [0xfe, 0xed, 0xfa, 0xce] => (false, false),
        [0xfe, 0xed, 0xfa, 0xcf] => (false, true),
        _ => return None,
    };
    if wide {
        file.seek(SeekFrom::Current(4)).ok()?;
    }
    let count = integer(&header[16..], little)? as usize;
    let size = integer(&header[20..], little)? as usize;
    if count > 4096
        || size > MAX_COMMAND_BYTES
        || file.stream_position().ok()?.checked_add(size as u64)? > image_end
    {
        return None;
    }
    let mut commands = vec![0; size];
    file.read_exact(&mut commands).ok()?;
    let mut image = Image::default();
    let mut position: usize = 0;
    for _ in 0..count {
        let rest = commands.get(position..)?;
        let command = integer(rest, little)?;
        let size = integer(rest.get(4..)?, little)? as usize;
        if size < 8 {
            return None;
        }
        let bytes = rest.get(..size)?;
        let minimum = match command {
            0xc | 0x8000_0018 | 0x8000_001f | 0x20 | 0x8000_0023 => Some(24),
            0x8000_001c => Some(12),
            _ => None,
        };
        if let Some(minimum) = minimum {
            let start = integer(bytes.get(8..)?, little)? as usize;
            if start < minimum {
                return None;
            }
            let value = bytes.get(start..)?;
            let end = value.iter().position(|&byte| byte == 0)?;
            let value = std::str::from_utf8(&value[..end]).ok()?.to_owned();
            if command == 0x8000_001c {
                image.rpaths.push(value);
            } else {
                image.libraries.push(value);
            }
        }
        position = position.checked_add(size)?;
    }
    (position == commands.len()).then_some(image)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn image(commands: &[(u32, &str)]) -> Vec<u8> {
        let mut body = Vec::new();
        for &(command, name) in commands {
            let start = if command == 0x8000_001c { 12 } else { 24 };
            let size = (start + name.len() + 1).next_multiple_of(8);
            let mut bytes = vec![0; size];
            bytes[..4].copy_from_slice(&command.to_le_bytes());
            bytes[4..8].copy_from_slice(&(size as u32).to_le_bytes());
            bytes[8..12].copy_from_slice(&(start as u32).to_le_bytes());
            bytes[start..start + name.len()].copy_from_slice(name.as_bytes());
            body.extend(bytes);
        }
        let mut header = vec![0; 32];
        header[..4].copy_from_slice(&0xfeed_facfu32.to_le_bytes());
        header[16..20].copy_from_slice(&(commands.len() as u32).to_le_bytes());
        header[20..24].copy_from_slice(&(body.len() as u32).to_le_bytes());
        header.extend(body);
        header
    }

    #[test]
    fn symlinked_transitive_libraries_are_exact_files_not_package_roots() {
        let root = tempfile::tempdir().unwrap();
        let root = root.path().canonicalize().unwrap();
        let bin = root.join("bin");
        let lib = root.join("Cellar/pcre2/1/lib");
        std::fs::create_dir_all(&bin).unwrap();
        std::fs::create_dir_all(&lib).unwrap();
        let alias = root.join("opt");
        std::os::unix::fs::symlink(root.join("Cellar/pcre2/1"), &alias).unwrap();
        let first = lib.join("first.dylib");
        let second = lib.join("second.dylib");
        let binary = bin.join("rg");
        std::fs::write(
            &binary,
            image(&[(0xc, alias.join("lib/first.dylib").to_str().unwrap())]),
        )
        .unwrap();
        std::fs::write(&first, image(&[(0xc, "@loader_path/second.dylib")])).unwrap();
        std::fs::write(
            &second,
            image(&[
                (0xc, "@loader_path/first.dylib"),
                (0xc, "@loader_path/unrelated-secret"),
            ]),
        )
        .unwrap();
        std::fs::write(lib.join("unrelated-secret"), "private").unwrap();
        assert_eq!(libraries(&binary), vec![first, second]);
    }

    #[test]
    fn rpaths_are_inherited_from_the_executable_for_transitive_dependencies() {
        let root = tempfile::tempdir().unwrap();
        let root = root.path().canonicalize().unwrap();
        let bin = root.join("rg");
        let lib = root.join("lib");
        std::fs::create_dir(&lib).unwrap();
        let first = lib.join("first.dylib");
        let second = lib.join("second.dylib");
        std::fs::write(
            &bin,
            image(&[
                (0x8000_001c, "@executable_path/lib"),
                (0xc, "@rpath/first.dylib"),
            ]),
        )
        .unwrap();
        std::fs::write(&first, image(&[(0xc, "@rpath/second.dylib")])).unwrap();
        std::fs::write(&second, image(&[])).unwrap();
        assert_eq!(libraries(&bin), vec![first, second]);
    }

    #[test]
    fn universal_images_select_host_architecture_and_invalid_commands_fail_closed() {
        let root = tempfile::tempdir().unwrap();
        let binary = root.path().join("rg");
        let lib = root.path().join("needed.dylib");
        std::fs::write(&lib, image(&[])).unwrap();
        let thin = image(&[(0xc, lib.to_str().unwrap())]);
        let cpu: u32 = if cfg!(target_arch = "aarch64") {
            0x0100_000c
        } else {
            0x0100_0007
        };
        let mut fat = vec![0; 64];
        fat[..4].copy_from_slice(&0xcafe_babeu32.to_be_bytes());
        fat[4..8].copy_from_slice(&1u32.to_be_bytes());
        fat[8..12].copy_from_slice(&cpu.to_be_bytes());
        fat[16..20].copy_from_slice(&64u32.to_be_bytes());
        fat[20..24].copy_from_slice(&(thin.len() as u32).to_be_bytes());
        fat.extend(thin.clone());
        std::fs::write(&binary, fat).unwrap();
        assert_eq!(libraries(&binary), vec![lib.canonicalize().unwrap()]);
        let mut malformed = thin;
        malformed[36..40].copy_from_slice(&u32::MAX.to_le_bytes());
        std::fs::write(&binary, malformed).unwrap();
        assert!(libraries(&binary).is_empty());
        std::fs::write(&binary, b"#!/bin/sh\n").unwrap();
        assert!(libraries(&binary).is_empty());
    }
}

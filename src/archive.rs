//! Extract finished archives: ZIP, RAR (including multi-part sets), 7z and
//! tar (plain, .tar.gz / .tgz, .tar.xz / .txz).
//!
//! Extraction only ever writes inside the folder it was given:
//!
//! * an entry name that is absolute, has a drive letter or climbs with `..`
//!   is refused (and counted), never written;
//! * a file that is already there is kept, not overwritten;
//! * an archive that would unpack past the size or entry limits stops with a
//!   plain reason, so a "zip bomb" cannot fill the disk;
//! * nothing that comes out is ever run.
//!
//! A password-protected archive is tried without a password, then with each
//! saved password (Settings, Archive passwords); the one that opens it is
//! reported so it can move to the top. None fitting: reported as protected.

use std::io::{Read, Write};
use std::path::{Component, Path, PathBuf};

/// Refuse to write more than this in total (a real archive this big is rare;
/// a bomb is not).
const MAX_TOTAL: u64 = 64 * 1024 * 1024 * 1024;
const MAX_ENTRIES: usize = 200_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Zip,
    Rar,
    SevenZ,
    Tar,
    TarGz,
    TarXz,
}

impl Kind {
    /// The word Settings uses for this kind's "extract it" switch.
    pub fn key(self) -> &'static str {
        match self {
            Kind::Zip => "zip",
            Kind::Rar => "rar",
            Kind::SevenZ => "7z",
            Kind::Tar | Kind::TarGz | Kind::TarXz => "tar",
        }
    }
}

/// Which archive a file name is, and for RAR sets, where it sits in the set.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Volume {
    pub kind: Kind,
    /// File name of the volume extraction starts from.
    pub first: String,
    /// Shared name of every volume of a multi-part set (lowercase); `None`
    /// for a single file.
    pub set: Option<String>,
}

fn split_part(stem: &str) -> Option<(&str, u32)> {
    let lower = stem.to_ascii_lowercase();
    let i = lower.rfind(".part")?;
    let digits = &stem[i + 5..];
    if digits.is_empty() || digits.len() > 4 || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    Some((&stem[..i], digits.parse().ok()?))
}

/// Read an archive file name. `None` when it is not one this app extracts.
pub fn volume_of(name: &str) -> Option<Volume> {
    let lower = name.to_ascii_lowercase();
    let (stem, ext) = lower.rsplit_once('.')?;
    let orig_stem = &name[..stem.len()];
    let single = |kind| Some(Volume { kind, first: name.to_string(), set: None });
    match ext {
        "tar" => single(Kind::Tar),
        "tgz" => single(Kind::TarGz),
        "txz" => single(Kind::TarXz),
        "gz" if stem.ends_with(".tar") => single(Kind::TarGz),
        "xz" if stem.ends_with(".tar") => single(Kind::TarXz),
        "zip" => Some(Volume { kind: Kind::Zip, first: name.to_string(), set: None }),
        "7z" => Some(Volume { kind: Kind::SevenZ, first: name.to_string(), set: None }),
        "rar" => match split_part(orig_stem) {
            // name.part1.rar, name.part01.rar, ...: the set starts at part 1
            // written with the same number of digits.
            Some((base, _)) => {
                let width = orig_stem.len() - base.len() - 5;
                Some(Volume {
                    kind: Kind::Rar,
                    first: format!("{base}.part{:0width$}.rar", 1),
                    set: Some(format!("{}.part", base.to_ascii_lowercase())),
                })
            }
            // name.rar, possibly followed by name.r00, name.r01, ...
            None => Some(Volume { kind: Kind::Rar, first: name.to_string(), set: Some(format!("{stem}.r")) }),
        },
        // Old-style later volumes: name.r00 .. name.r99
        e if e.len() == 3 && e.starts_with('r') && e[1..].bytes().all(|b| b.is_ascii_digit()) => Some(Volume {
            kind: Kind::Rar,
            first: format!("{orig_stem}.rar"),
            set: Some(format!("{stem}.r")),
        }),
        _ => None,
    }
}

/// Does file `name` belong to the multi-part set `set` (see [`Volume::set`])?
pub fn in_set(name: &str, set: &str) -> bool {
    volume_of(name).and_then(|v| v.set).is_some_and(|s| s == set)
}

/// Where an archive's files go: next to it, or in a new folder named after
/// it (`name`, `name (2)`, ... when that is taken).
pub fn destination(archive: &Path, into_folder: bool) -> PathBuf {
    let parent = archive.parent().map(Path::to_path_buf).unwrap_or_default();
    if !into_folder {
        return parent;
    }
    let file = archive.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let mut stem = file.rsplit_once('.').map(|(s, _)| s.to_string()).unwrap_or(file.clone());
    // "name.tar.gz" -> "name".
    if stem.to_ascii_lowercase().ends_with(".tar") {
        stem.truncate(stem.len() - 4);
    }
    if let Some((base, _)) = split_part(&stem) {
        stem = base.to_string();
    }
    let stem = stem.trim().trim_end_matches('.').to_string();
    let stem = if stem.is_empty() { "extracted".to_string() } else { stem };
    let mut dir = parent.join(&stem);
    let mut n = 2;
    while dir.exists() {
        dir = parent.join(format!("{stem} ({n})"));
        n += 1;
    }
    dir
}

/// The path an entry name maps to under `root`, or `None` when the name
/// would land outside it.
pub fn safe_join(root: &Path, name: &str) -> Option<PathBuf> {
    let name = name.replace('\\', "/");
    if name.starts_with('/') || name.contains(':') || name.contains('\0') {
        return None;
    }
    let mut out = root.to_path_buf();
    let mut parts = 0;
    for piece in name.split('/') {
        match piece {
            "" | "." => continue,
            ".." => return None,
            p => {
                // Each piece must be one plain path component here too.
                let mut c = Path::new(p).components();
                match (c.next(), c.next()) {
                    (Some(Component::Normal(_)), None) => {}
                    _ => return None,
                }
                // Windows drops trailing dots and spaces: "a. " would be "a".
                let p = p.trim_end_matches(['.', ' ']);
                if p.is_empty() {
                    return None;
                }
                out.push(p);
                parts += 1;
            }
        }
    }
    (parts > 0).then_some(out)
}

#[derive(Debug)]
pub enum Error {
    Password,
    Limit(String),
    Broken(String),
    Io(String),
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::Password => write!(f, "it is password-protected and none of the saved passwords opens it (Settings, Archives, Archive passwords)"),
            Error::Limit(s) => write!(f, "{s}"),
            Error::Broken(s) => write!(f, "the archive could not be read ({s})"),
            Error::Io(s) => write!(f, "{s}"),
        }
    }
}

impl From<std::io::Error> for Error {
    fn from(e: std::io::Error) -> Self {
        Error::Io(e.to_string())
    }
}

#[derive(Debug, Default, Clone)]
pub struct Outcome {
    pub dest: PathBuf,
    pub files: usize,
    pub bytes: u64,
    /// Files already there, kept as they were.
    pub skipped: usize,
    /// Entries whose names tried to leave the folder.
    pub refused: usize,
}

impl Outcome {
    /// Everything in the archive is now on disk as extracted.
    pub fn complete(&self) -> bool {
        self.skipped == 0 && self.refused == 0
    }
}

struct Budget {
    out: Outcome,
    entries: usize,
    /// Files this attempt wrote, removed again when its password was wrong.
    written: Vec<PathBuf>,
}

impl Budget {
    fn entry(&mut self) -> Result<(), Error> {
        self.entries += 1;
        if self.entries > MAX_ENTRIES {
            return Err(Error::Limit(format!("it holds more than {MAX_ENTRIES} entries")));
        }
        Ok(())
    }

    fn declared(&self, size: u64) -> Result<(), Error> {
        if self.out.bytes.saturating_add(size) > MAX_TOTAL {
            return Err(Error::Limit("it would unpack to more than 64 GB".into()));
        }
        Ok(())
    }

    /// Where an entry goes, or `None` (counted) when it is refused or the
    /// file already exists.
    fn target(&mut self, name: &str) -> Result<Option<PathBuf>, Error> {
        self.entry()?;
        let Some(path) = safe_join(&self.out.dest, name) else {
            self.out.refused += 1;
            return Ok(None);
        };
        if path.exists() {
            self.out.skipped += 1;
            return Ok(None);
        }
        if let Some(p) = path.parent() {
            std::fs::create_dir_all(p)?;
        }
        Ok(Some(path))
    }

    /// Copy one entry's bytes, never past the total limit. A partly written
    /// file is removed on failure.
    fn write(&mut self, path: &Path, reader: &mut dyn Read) -> Result<(), Error> {
        let res = (|| -> Result<u64, Error> {
            let mut file = std::fs::File::create(path)?;
            let mut buf = vec![0u8; 256 * 1024];
            let mut n = 0u64;
            loop {
                let k = reader.read(&mut buf).map_err(|e| Error::Broken(e.to_string()))?;
                if k == 0 {
                    break;
                }
                n += k as u64;
                if self.out.bytes + n > MAX_TOTAL {
                    return Err(Error::Limit("it would unpack to more than 64 GB".into()));
                }
                file.write_all(&buf[..k])?;
            }
            Ok(n)
        })();
        match res {
            Ok(n) => {
                self.out.bytes += n;
                self.out.files += 1;
                self.written.push(path.to_path_buf());
                Ok(())
            }
            Err(e) => {
                let _ = std::fs::remove_file(path);
                Err(e)
            }
        }
    }
}

/// Extract `archive` into `dest` (created when missing), without a password.
pub fn extract(archive: &Path, dest: &Path) -> Result<Outcome, Error> {
    extract_with(archive, dest, &[]).map(|(out, _)| out)
}

/// Extract `archive` into `dest`, trying no password first and then each of
/// `passwords` in order. Returns what came out and the password that fitted
/// (`None`: none was needed).
pub fn extract_with(archive: &Path, dest: &Path, passwords: &[String]) -> Result<(Outcome, Option<String>), Error> {
    let name = archive.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let vol = volume_of(&name).ok_or_else(|| Error::Broken("not a ZIP, RAR, 7z or tar file".into()))?;
    let start = archive.with_file_name(&vol.first);
    if !start.is_file() {
        return Err(Error::Io(format!("{} is missing", vol.first)));
    }
    std::fs::create_dir_all(dest)?;
    let tries = std::iter::once(None).chain(
        passwords.iter().map(|p| p.trim()).filter(|p| !p.is_empty()).map(Some),
    );
    for pw in tries {
        let mut b = Budget { out: Outcome { dest: dest.to_path_buf(), ..Outcome::default() }, entries: 0, written: Vec::new() };
        let res = match vol.kind {
            Kind::Zip => zip_extract(&start, &mut b, pw),
            Kind::SevenZ => sevenz_extract(&start, &mut b, pw),
            Kind::Rar => rar_extract(&start, &mut b, pw),
            // A tar has no password: one try.
            Kind::Tar | Kind::TarGz | Kind::TarXz => return tar_extract(&start, vol.kind, &mut b).map(|()| (b.out, None)),
        };
        match res {
            Ok(()) => return Ok((b.out, pw.map(str::to_string))),
            Err(Error::Password) => {
                // A wrong password may have let the unencrypted entries out:
                // they go, or the next try would find them "already there".
                for f in b.written.iter().rev() {
                    let _ = std::fs::remove_file(f);
                }
                continue;
            }
            Err(e) => return Err(e),
        }
    }
    Err(Error::Password)
}

fn zip_extract(path: &Path, b: &mut Budget, pw: Option<&str>) -> Result<(), Error> {
    let file = std::fs::File::open(path)?;
    let mut zip = zip::ZipArchive::new(std::io::BufReader::new(file)).map_err(|e| Error::Broken(e.to_string()))?;
    for i in 0..zip.len() {
        let opened = match pw {
            Some(p) => zip.by_index_decrypt(i, p.as_bytes()),
            None => zip.by_index(i),
        };
        let mut entry = match opened {
            Ok(e) => e,
            Err(zip::result::ZipError::InvalidPassword) => return Err(Error::Password),
            Err(zip::result::ZipError::UnsupportedArchive(m)) if m.contains("ncrypt") || m.contains("assword") => {
                return Err(Error::Password)
            }
            Err(e) => return Err(Error::Broken(e.to_string())),
        };
        if entry.encrypted() && pw.is_none() {
            return Err(Error::Password);
        }
        let name = entry.name().to_string();
        if entry.is_dir() {
            b.entry()?;
            if let Some(dir) = safe_join(&b.out.dest, &name) {
                std::fs::create_dir_all(dir)?;
            } else {
                b.out.refused += 1;
            }
            continue;
        }
        b.declared(entry.size())?;
        if let Some(target) = b.target(&name)? {
            b.write(&target, &mut entry)?;
        }
    }
    Ok(())
}

fn sevenz_extract(path: &Path, b: &mut Budget, pw: Option<&str>) -> Result<(), Error> {
    use sevenz_rust2::{ArchiveReader, Error as SzError, Password};
    let map = |e: SzError| match e {
        SzError::PasswordRequired | SzError::MaybeBadPassword(_) => Error::Password,
        e => Error::Broken(e.to_string()),
    };
    let password = pw.map(Password::from).unwrap_or_else(Password::empty);
    let mut reader = ArchiveReader::open(path, password).map_err(map)?;
    let total: u64 = reader.archive().files.iter().map(|f| f.size).sum();
    b.declared(total)?;
    let mut failure: Option<Error> = None;
    let res = reader.for_each_entries(|entry, data| {
        let step = (|| -> Result<(), Error> {
            if entry.is_anti_item {
                return Ok(());
            }
            if entry.is_directory {
                b.entry()?;
                match safe_join(&b.out.dest, &entry.name) {
                    Some(dir) => std::fs::create_dir_all(dir)?,
                    None => b.out.refused += 1,
                }
                return Ok(());
            }
            match b.target(&entry.name)? {
                Some(target) => b.write(&target, data),
                // Skipped entries still have to be read through: in a solid
                // archive the next file's bytes come after them.
                None => std::io::copy(data, &mut std::io::sink()).map(|_| ()).map_err(|e| Error::Broken(e.to_string())),
            }
        })();
        match step {
            Ok(()) => Ok(true),
            Err(e) => {
                failure = Some(e);
                Ok(false)
            }
        }
    });
    if let Some(e) = failure {
        return Err(e);
    }
    res.map_err(map)
}

fn rar_extract(path: &Path, b: &mut Budget, pw: Option<&str>) -> Result<(), Error> {
    let broken = |e: unrar::error::UnrarError| {
        if matches!(e.code, unrar::error::Code::MissingPassword | unrar::error::Code::BadPassword) {
            Error::Password
        } else {
            Error::Broken(e.to_string())
        }
    };
    let opened = match pw {
        Some(p) => unrar::Archive::with_password(path, p).open_for_processing(),
        None => unrar::Archive::new(path).open_for_processing(),
    };
    let mut arc = opened.map_err(broken)?;
    if arc.has_encrypted_headers() && pw.is_none() {
        return Err(Error::Password);
    }
    while let Some(header) = arc.read_header().map_err(broken)? {
        let entry = header.entry();
        if entry.is_encrypted() && pw.is_none() {
            return Err(Error::Password);
        }
        let name = entry.filename.to_string_lossy().into_owned();
        arc = if entry.is_directory() {
            b.entry()?;
            match safe_join(&b.out.dest, &name) {
                Some(dir) => std::fs::create_dir_all(dir)?,
                None => b.out.refused += 1,
            }
            header.skip().map_err(broken)?
        } else {
            let size = entry.unpacked_size;
            b.declared(size)?;
            match b.target(&name)? {
                Some(target) => {
                    let next = header.extract_to(&target).map_err(|e| {
                        let _ = std::fs::remove_file(&target);
                        broken(e)
                    })?;
                    b.out.bytes += std::fs::metadata(&target).map(|m| m.len()).unwrap_or(size);
                    b.out.files += 1;
                    next
                }
                None => header.skip().map_err(broken)?,
            }
        };
    }
    Ok(())
}

/// A tar, plain or compressed. Only files and folders come out: links (a
/// way to write outside the folder) are refused and counted.
fn tar_extract(path: &Path, kind: Kind, b: &mut Budget) -> Result<(), Error> {
    let file = std::io::BufReader::new(std::fs::File::open(path)?);
    // .xz has no streaming reader here: it is unpacked to a hidden file next
    // to the output first, read as a tar, then removed.
    let mut unpacked: Option<PathBuf> = None;
    let reader: Box<dyn Read> = match kind {
        Kind::TarGz => Box::new(flate2::read::GzDecoder::new(file)),
        Kind::TarXz => {
            let tmp = b.out.dest.join(".download_craft_untar.tmp");
            let mut out = std::io::BufWriter::new(std::fs::File::create(&tmp)?);
            let mut input = file;
            lzma_rs::xz_decompress(&mut input, &mut out).map_err(|e| Error::Broken(e.to_string()))?;
            out.flush()?;
            drop(out);
            unpacked = Some(tmp.clone());
            Box::new(std::io::BufReader::new(std::fs::File::open(&tmp)?))
        }
        _ => Box::new(file),
    };
    let res = (|| -> Result<(), Error> {
        let mut tar = tar::Archive::new(reader);
        for entry in tar.entries().map_err(|e| Error::Broken(e.to_string()))? {
            let mut entry = entry.map_err(|e| Error::Broken(e.to_string()))?;
            let name = entry.path().map_err(|e| Error::Broken(e.to_string()))?.to_string_lossy().into_owned();
            match entry.header().entry_type() {
                tar::EntryType::Directory => {
                    b.entry()?;
                    match safe_join(&b.out.dest, &name) {
                        Some(dir) => std::fs::create_dir_all(dir)?,
                        None => b.out.refused += 1,
                    }
                }
                tar::EntryType::Regular | tar::EntryType::Continuous => {
                    b.declared(entry.header().size().unwrap_or(0))?;
                    if let Some(target) = b.target(&name)? {
                        b.write(&target, &mut entry)?;
                    }
                }
                // Links, devices, metadata records: never written.
                _ => {
                    b.entry()?;
                    b.out.refused += 1;
                }
            }
        }
        Ok(())
    })();
    if let Some(tmp) = unpacked {
        let _ = std::fs::remove_file(tmp);
    }
    res
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn archive_names() {
        assert_eq!(volume_of("a.zip").unwrap().kind, Kind::Zip);
        assert_eq!(volume_of("A.7Z").unwrap().kind, Kind::SevenZ);
        let one = volume_of("Movie.rar").unwrap();
        assert_eq!((one.first.as_str(), one.set.as_deref()), ("Movie.rar", Some("movie.r")));
        let later = volume_of("Movie.r07").unwrap();
        assert_eq!((later.first.as_str(), later.set.as_deref()), ("Movie.rar", Some("movie.r")));
        let p3 = volume_of("Big.Set.part03.rar").unwrap();
        assert_eq!((p3.first.as_str(), p3.set.as_deref()), ("Big.Set.part01.rar", Some("big.set.part")));
        assert_eq!(volume_of("x.part2.rar").unwrap().first, "x.part1.rar");
        assert!(in_set("big.set.part1.rar", "big.set.part"));
        assert!(!in_set("other.part1.rar", "big.set.part"));
        assert_eq!(volume_of("a.tar").unwrap().kind, Kind::Tar);
        assert_eq!(volume_of("A.TAR.GZ").unwrap().kind, Kind::TarGz);
        assert_eq!(volume_of("a.tgz").unwrap().kind, Kind::TarGz);
        assert_eq!(volume_of("a.tar.xz").unwrap().kind, Kind::TarXz);
        assert_eq!(destination(Path::new("/d/Pack.tar.gz"), true).file_name().unwrap(), "Pack");
        for n in ["a.zipx", "a", "notes.r", "a.rev", "a.gz", "a.xz"] {
            assert_eq!(volume_of(n), None, "{n}");
        }
    }

    #[test]
    fn entry_names_stay_inside() {
        let root = Path::new("/out");
        assert_eq!(safe_join(root, "a/b.txt"), Some(root.join("a").join("b.txt")));
        assert_eq!(safe_join(root, "a\\b.txt"), Some(root.join("a").join("b.txt")));
        assert_eq!(safe_join(root, "./a.txt"), Some(root.join("a.txt")));
        for bad in ["../x", "a/../../x", "/etc/passwd", "\\\\server\\share\\x", "C:\\x", "C:x", "", "./", "a/ . /b"] {
            assert_eq!(safe_join(root, bad), None, "{bad:?}");
        }
    }

    fn tmp(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("dc_archive_{tag}_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn make_zip(path: &Path, entries: &[(&str, &[u8])]) {
        let mut w = zip::ZipWriter::new(std::fs::File::create(path).unwrap());
        for (n, data) in entries {
            w.start_file(*n, zip::write::SimpleFileOptions::default()).unwrap();
            w.write_all(data).unwrap();
        }
        w.finish().unwrap();
    }

    #[test]
    fn zip_into_its_own_folder_and_keeps_existing_files() {
        let d = tmp("zip");
        let z = d.join("Pack.zip");
        make_zip(&z, &[("readme.txt", b"hello"), ("sub/data.bin", &[7u8; 5000]), ("../evil.txt", b"no")]);
        let dest = destination(&z, true);
        assert_eq!(dest, d.join("Pack"));
        let out = extract(&z, &dest).unwrap();
        assert_eq!((out.files, out.refused, out.bytes), (2, 1, 5005));
        assert_eq!(std::fs::read(dest.join("sub").join("data.bin")).unwrap().len(), 5000);
        assert!(!d.join("evil.txt").exists());
        // A second run into the same folder keeps what is there.
        std::fs::write(dest.join("readme.txt"), b"mine").unwrap();
        let again = extract(&z, &dest).unwrap();
        assert_eq!((again.files, again.skipped), (0, 2));
        assert_eq!(std::fs::read(dest.join("readme.txt")).unwrap(), b"mine");
        // The folder name is taken now: the next one gets a number.
        assert_eq!(destination(&z, true), d.join("Pack (2)"));
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn sevenz_extracts() {
        let d = tmp("7z");
        let src = d.join("src");
        std::fs::create_dir_all(src.join("inner")).unwrap();
        std::fs::write(src.join("a.txt"), b"alpha").unwrap();
        std::fs::write(src.join("inner").join("b.txt"), vec![b'b'; 70_000]).unwrap();
        let archive = d.join("set.7z");
        let mut w = sevenz_rust2::ArchiveWriter::create(&archive).unwrap();
        w.push_source_path(&src, |_| true).unwrap();
        w.finish().unwrap();
        let out = extract(&archive, &d.join("out")).unwrap();
        assert_eq!(out.files, 2);
        assert_eq!(std::fs::read(d.join("out").join("a.txt")).unwrap(), b"alpha");
        assert_eq!(std::fs::read(d.join("out").join("inner").join("b.txt")).unwrap().len(), 70_000);
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn tar_gz_extracts_and_refuses_links() {
        let d = tmp("targz");
        let archive = d.join("Pack.tar.gz");
        {
            let gz = flate2::write::GzEncoder::new(std::fs::File::create(&archive).unwrap(), flate2::Compression::fast());
            let mut t = tar::Builder::new(gz);
            let data = b"hello tar";
            let mut h = tar::Header::new_gnu();
            h.set_size(data.len() as u64);
            h.set_mode(0o644);
            h.set_cksum();
            t.append_data(&mut h, "inner/hello.txt", &data[..]).unwrap();
            let mut l = tar::Header::new_gnu();
            l.set_entry_type(tar::EntryType::Symlink);
            l.set_size(0);
            l.set_cksum();
            t.append_link(&mut l, "escape", "../../outside").unwrap();
            t.into_inner().unwrap().finish().unwrap();
        }
        let out = extract(&archive, &d.join("o")).unwrap();
        assert_eq!((out.files, out.refused), (1, 1), "{out:?}");
        assert_eq!(std::fs::read(d.join("o").join("inner").join("hello.txt")).unwrap(), b"hello tar");
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn saved_passwords_open_a_protected_zip() {
        let d = tmp("zippw");
        let z = d.join("Secret.zip");
        {
            let mut w = zip::ZipWriter::new(std::fs::File::create(&z).unwrap());
            let opts = zip::write::SimpleFileOptions::default().with_aes_encryption(zip::AesMode::Aes256, "letmein");
            w.start_file("note.txt", opts).unwrap();
            w.write_all(b"secret").unwrap();
            w.finish().unwrap();
        }
        assert!(matches!(extract(&z, &d.join("a")), Err(Error::Password)));
        let list = vec!["wrong".to_string(), "letmein".to_string()];
        let (out, used) = extract_with(&z, &d.join("b"), &list).unwrap();
        assert_eq!((out.files, used.as_deref()), (1, Some("letmein")));
        assert_eq!(std::fs::read(d.join("b").join("note.txt")).unwrap(), b"secret");
        let _ = std::fs::remove_dir_all(&d);
    }

    fn fixture(name: &str) -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests").join("fixtures").join("archive").join(name)
    }

    #[test]
    fn rar_extracts_and_reports_passwords() {
        let d = tmp("rar");
        let r = d.join("version.rar");
        std::fs::copy(fixture("version.rar"), &r).unwrap();
        let out = extract(&r, &d.join("o")).unwrap();
        assert_eq!(out.files, 1, "{out:?}");
        assert!(std::fs::read_dir(d.join("o")).unwrap().count() == 1);
        let c = d.join("crypted.rar");
        std::fs::copy(fixture("crypted.rar"), &c).unwrap();
        assert!(matches!(extract(&c, &d.join("c")), Err(Error::Password)));
        std::fs::write(&r, b"not a rar at all").unwrap();
        assert!(matches!(extract(&r, &d.join("o2")), Err(Error::Broken(_))));
        let _ = std::fs::remove_dir_all(&d);
    }
}

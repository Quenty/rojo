use std::collections::{HashMap, HashSet};
use std::ffi::{OsStr, OsString};
use std::io;
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

use crossbeam_channel::Receiver;
use notify::{watcher, DebouncedEvent, RecommendedWatcher, RecursiveMode, Watcher};

use crate::{DirEntry, Metadata, ReadDir, VfsBackend, VfsEvent};

/// `VfsBackend` that uses `std::fs` and the `notify` crate.
pub struct StdBackend {
    watcher: RecommendedWatcher,
    watcher_receiver: Receiver<VfsEvent>,
    watches: HashSet<PathBuf>,
    cache: ListingCache,
}

impl StdBackend {
    pub fn new() -> StdBackend {
        let (notify_tx, notify_rx) = mpsc::channel();
        let watcher = watcher(notify_tx, Duration::from_millis(50)).unwrap();

        let (tx, rx) = crossbeam_channel::unbounded();

        thread::spawn(move || {
            for event in notify_rx {
                match event {
                    DebouncedEvent::Create(path) => {
                        tx.send(VfsEvent::Create(path))?;
                    }
                    DebouncedEvent::Write(path) => {
                        tx.send(VfsEvent::Write(path))?;
                    }
                    DebouncedEvent::Remove(path) => {
                        tx.send(VfsEvent::Remove(path))?;
                    }
                    DebouncedEvent::Rename(from, to) => {
                        tx.send(VfsEvent::Remove(from))?;
                        tx.send(VfsEvent::Create(to))?;
                    }
                    _ => {}
                }
            }

            Result::<(), crossbeam_channel::SendError<VfsEvent>>::Ok(())
        });

        Self {
            watcher,
            watcher_receiver: rx,
            watches: HashSet::new(),
            cache: ListingCache::default(),
        }
    }

    /// Reads a directory from disk, records its listing in the cache, and
    /// returns the entries sorted by file name.
    fn list_dir(&mut self, path: &Path) -> io::Result<Vec<fs_err::DirEntry>> {
        let entries: Result<Vec<_>, _> = fs_err::read_dir(path)?.collect();
        let mut entries = entries?;
        entries.sort_by_cached_key(|entry| entry.file_name());

        self.cache.insert_listing(path, &entries);

        Ok(entries)
    }

    /// Answers "what is at `path`?" from the listing cache when `path` is a
    /// child of a directory we have already listed, or of a directory we know
    /// exists and are about to traverse anyway. Returns `None` when the cache
    /// cannot answer and the caller must ask the filesystem.
    fn cached_lookup(&mut self, path: &Path) -> Option<Option<Metadata>> {
        if let Some(hit) = self.cache.lookup(path) {
            return Some(hit);
        }

        let parent = path.parent()?;
        if !self.cache.is_known_dir(parent) {
            return None;
        }

        // Listing the parent costs one syscall and replaces every probe that
        // will be made against it (init.* files, *.meta.json, and the type
        // check on each child), so it pays for itself immediately.
        if self.list_dir(parent).is_err() {
            return None;
        }

        self.cache.lookup(path)
    }
}

impl VfsBackend for StdBackend {
    fn read(&mut self, path: &Path) -> io::Result<Vec<u8>> {
        if let Some(None) = self.cached_lookup(path) {
            return Err(not_found(path));
        }

        fs_err::read(path)
    }

    fn write(&mut self, path: &Path, data: &[u8]) -> io::Result<()> {
        self.cache.invalidate(path);
        fs_err::write(path, data)
    }

    fn exists(&mut self, path: &Path) -> io::Result<bool> {
        // `std::fs::exists` follows symlinks, so only a plain file or
        // directory (or a definite absence) can be answered from the cache.
        match self.cached_lookup(path) {
            Some(None) => return Ok(false),
            Some(Some(meta)) if !meta.is_symlink() => return Ok(true),
            _ => {}
        }

        std::fs::exists(path)
    }

    fn read_dir(&mut self, path: &Path) -> io::Result<ReadDir> {
        let paths = match self.cache.child_paths(path) {
            Some(paths) => paths,
            None => self
                .list_dir(path)?
                .into_iter()
                .map(|entry| entry.path())
                .collect(),
        };

        let inner = paths.into_iter().map(|path| Ok(DirEntry { path }));

        Ok(ReadDir {
            inner: Box::new(inner),
        })
    }

    fn create_dir(&mut self, path: &Path) -> io::Result<()> {
        self.cache.invalidate(path);
        fs_err::create_dir(path)
    }

    fn create_dir_all(&mut self, path: &Path) -> io::Result<()> {
        self.cache.invalidate(path);
        fs_err::create_dir_all(path)
    }

    fn remove_file(&mut self, path: &Path) -> io::Result<()> {
        self.cache.invalidate(path);
        fs_err::remove_file(path)
    }

    fn remove_dir_all(&mut self, path: &Path) -> io::Result<()> {
        self.cache.invalidate(path);
        fs_err::remove_dir_all(path)
    }

    fn metadata(&mut self, path: &Path) -> io::Result<Metadata> {
        if let Some(hit) = self.cached_lookup(path) {
            return hit.ok_or_else(|| not_found(path));
        }

        let filetype = fs_err::symlink_metadata(path)?.file_type();

        if filetype.is_dir() {
            self.cache.note_dir(path);
        }

        Ok(Metadata {
            is_file: filetype.is_file(),
            is_symlink: filetype.is_symlink(),
        })
    }

    fn canonicalize(&mut self, path: &Path) -> io::Result<PathBuf> {
        if let Some(canonical) = self.cache.canonical(path) {
            return Ok(canonical);
        }

        let canonical = fs_err::canonicalize(path)?;
        self.cache.note_canonical(path, &canonical);
        Ok(canonical)
    }

    fn read_link(&mut self, path: &Path) -> io::Result<PathBuf> {
        fs_err::read_link(path)
    }

    fn event_receiver(&self) -> crossbeam_channel::Receiver<VfsEvent> {
        self.watcher_receiver.clone()
    }

    fn watch(&mut self, path: &Path) -> io::Result<()> {
        if self.watches.contains(path)
            || path
                .ancestors()
                .any(|ancestor| self.watches.contains(ancestor))
        {
            Ok(())
        } else {
            self.watches.insert(path.to_path_buf());
            self.watcher
                .watch(path, RecursiveMode::Recursive)
                .map_err(io::Error::other)
        }
    }

    fn unwatch(&mut self, path: &Path) -> io::Result<()> {
        self.watches.remove(path);
        self.watcher.unwatch(path).map_err(io::Error::other)
    }

    fn invalidate(&mut self, path: &Path) {
        self.cache.invalidate(path);
    }
}

impl Default for StdBackend {
    fn default() -> Self {
        Self::new()
    }
}

/// Remembers the contents of directories that have been listed so that
/// questions about their children ("is there an `init.lua` here?", "is this
/// entry a directory?") can be answered without going back to the filesystem.
///
/// Rojo asks around ten such questions per directory entry, and every one of
/// them is a syscall that the directory listing already answered. Entries are
/// forgotten whenever the backend writes to a path or is told about a change
/// through `invalidate`, which the `Vfs` calls for every filesystem event.
#[derive(Default)]
struct ListingCache {
    /// Directory path, exactly as it was passed to `read_dir`, mapped to its
    /// children keyed by file name (see `name_key`).
    listings: HashMap<PathBuf, HashMap<OsString, ListingEntry>>,

    /// Paths we have learned are directories, either because they appeared in
    /// a listing or because a `metadata` call said so. Only children of these
    /// paths are eligible for a speculative listing, which keeps the cache
    /// from listing arbitrary directories on behalf of one-off lookups.
    known_dirs: HashSet<PathBuf>,

    /// Results of `canonicalize`. Symlink targets repeat many times in a
    /// pnpm tree, and each resolution is otherwise a fresh syscall.
    canonical: HashMap<PathBuf, PathBuf>,
}

struct ListingEntry {
    /// The file name as the filesystem reported it.
    name: OsString,
    kind: EntryKind,
}

#[derive(Clone, Copy)]
enum EntryKind {
    File,
    Dir,
    Symlink,
}

impl EntryKind {
    fn from_file_type(file_type: std::fs::FileType) -> Self {
        if file_type.is_symlink() {
            EntryKind::Symlink
        } else if file_type.is_dir() {
            EntryKind::Dir
        } else {
            EntryKind::File
        }
    }

    /// Mirrors what `symlink_metadata` reports for each kind of entry.
    fn metadata(self) -> Metadata {
        match self {
            EntryKind::File => Metadata {
                is_file: true,
                is_symlink: false,
            },
            EntryKind::Dir => Metadata {
                is_file: false,
                is_symlink: false,
            },
            EntryKind::Symlink => Metadata {
                is_file: false,
                is_symlink: true,
            },
        }
    }
}

impl ListingCache {
    fn insert_listing(&mut self, dir: &Path, entries: &[fs_err::DirEntry]) {
        let mut listing = HashMap::with_capacity(entries.len());

        for entry in entries {
            let kind = match entry.file_type() {
                Ok(file_type) => EntryKind::from_file_type(file_type),
                // Without a reliable type for every child, the listing cannot
                // stand in for the filesystem, so don't cache this directory.
                Err(_) => return,
            };

            if let EntryKind::Dir = kind {
                self.known_dirs.insert(entry.path());
            }

            let name = entry.file_name();
            listing.insert(name_key(&name), ListingEntry { name, kind });
        }

        self.known_dirs.insert(dir.to_path_buf());
        self.listings.insert(dir.to_path_buf(), listing);
    }

    /// Returns `Some(Some(_))` if `path` is a known child of a listed
    /// directory, `Some(None)` if the directory is listed but has no such
    /// child, and `None` if `path`'s parent has not been listed.
    fn lookup(&self, path: &Path) -> Option<Option<Metadata>> {
        let parent = path.parent()?;
        let name = path.file_name()?;
        let listing = self.listings.get(parent)?;

        Some(
            listing
                .get(&name_key(name))
                .map(|entry| entry.kind.metadata()),
        )
    }

    /// Returns the children of `dir` sorted by file name, if `dir` has been
    /// listed.
    fn child_paths(&self, dir: &Path) -> Option<Vec<PathBuf>> {
        let listing = self.listings.get(dir)?;

        let mut names: Vec<&OsString> = listing.values().map(|entry| &entry.name).collect();
        names.sort();

        Some(names.into_iter().map(|name| dir.join(name)).collect())
    }

    fn is_known_dir(&self, path: &Path) -> bool {
        self.known_dirs.contains(path)
    }

    fn note_dir(&mut self, path: &Path) {
        self.known_dirs.insert(path.to_path_buf());
    }

    fn canonical(&self, path: &Path) -> Option<PathBuf> {
        self.canonical.get(path).cloned()
    }

    fn note_canonical(&mut self, path: &Path, canonical: &Path) {
        self.canonical
            .insert(path.to_path_buf(), canonical.to_path_buf());
    }

    /// Forgets everything that a change at `path` could have affected: the
    /// listings of `path` and everything under it, the listings of all of
    /// its ancestors, and any canonical path that starts at or resolves to
    /// somewhere under `path`.
    ///
    /// Ancestors are included rather than just the parent because change
    /// events for a deleted tree arrive child-first: by the time the first
    /// one is handled, the parent directory may already be gone, and the
    /// grandparent's listing would still claim it exists.
    fn invalidate(&mut self, path: &Path) {
        for ancestor in path.ancestors() {
            self.listings.remove(ancestor);
        }

        self.listings.retain(|dir, _| !dir.starts_with(path));
        self.known_dirs.retain(|dir| !dir.starts_with(path));
        self.canonical
            .retain(|from, to| !from.starts_with(path) && !to.starts_with(path));
    }
}

/// Windows and macOS filesystems are case-insensitive by default, and Rojo
/// probes for fixed lowercase names like `init.lua`, so the cache must match
/// the way the filesystem would.
#[cfg(any(windows, target_os = "macos"))]
fn name_key(name: &OsStr) -> OsString {
    name.to_string_lossy().to_lowercase().into()
}

#[cfg(not(any(windows, target_os = "macos")))]
fn name_key(name: &OsStr) -> OsString {
    name.to_os_string()
}

fn not_found(path: &Path) -> io::Error {
    io::Error::new(
        io::ErrorKind::NotFound,
        format!("path {} not found", path.display()),
    )
}

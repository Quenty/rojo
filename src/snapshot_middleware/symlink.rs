//! Handles symlink resolution

use crate::{
    snapshot::{InstanceContext, InstanceMetadata, InstanceSnapshot, Symlink, Symlinks},
    snapshot_middleware::snapshot_from_vfs,
};
use memofs::Vfs;
use rbx_dom_weak::{types::Ref, ustr};
use std::path::{Path, PathBuf};

/// Returns the canonical path a symlink points at. This is used both as the
/// identity of the link (two links with the same target become one instance
/// plus ObjectValues pointing at it) and as the location to snapshot from.
///
/// Canonicalizing the link's target rather than the link itself lets the
/// `Vfs` reuse the result: in a pnpm tree the same absolute target appears
/// behind hundreds of different links, so only the first costs a full
/// resolution. Relative targets are resolved through the link, since the
/// same relative text under different directories can name different places.
pub fn symlink_target(vfs: &Vfs, path: &Path) -> anyhow::Result<PathBuf> {
    let target = vfs.read_link(path)?;

    let canonical = if target.is_absolute() {
        vfs.canonicalize(&target)?
    } else {
        vfs.canonicalize(path)?
    };

    Ok(strip_windows_long_file_path(&canonical).to_path_buf())
}

pub fn snapshot_symlink(
    symlinks: &mut Symlinks,
    context: &InstanceContext,
    vfs: &Vfs,
    path: &Path,
    canonical: &Path,
) -> anyhow::Result<Option<InstanceSnapshot>> {
    if let Some(symlink) = symlinks.get(canonical) {
        if symlink.reifed_path == path {
            return build_and_update(symlinks, context, vfs, path, canonical);
        } else {
            log::trace!("Pointing {} to symlink id {}", symlink.name, symlink.id,);

            return Ok(Some(
                InstanceSnapshot::new()
                    .name(symlink.name.to_owned())
                    .class_name("ObjectValue")
                    .property(ustr("Value"), symlink.id)
                    .metadata(
                        InstanceMetadata::new()
                            .instigating_source(path)
                            .relevant_paths(vec![canonical.to_owned()])
                            .context(context),
                    ),
            ));
        }
    }

    let id: Ref = Ref::new();
    let symlink = Symlink {
        id: id,
        point_to_id: id,
        name: canonical.file_name().unwrap().to_str().unwrap().to_owned(), // get directory name
        reifed_path: path.to_owned(),
    };

    log::trace!(
        "Building new symlink with id {} pointed at {}",
        id,
        path.display()
    );

    // Add before we can recurse/query this again
    symlinks.insert(canonical.to_owned(), symlink);
    build_and_update(symlinks, context, vfs, path, canonical)
}

fn strip_windows_long_file_path(canonical: &Path) -> &Path {
    // Strip out the canonicalized windows prefix
    if canonical.to_str().unwrap().starts_with("\\\\?\\") {
        // This is bad, but I don't know a better way to do this
        Path::new(&canonical.to_str().unwrap()[4..])
    } else {
        canonical
    }
}

fn build_and_update(
    symlinks: &mut Symlinks,
    context: &InstanceContext,
    vfs: &Vfs,
    path: &Path,
    canonical: &Path,
) -> anyhow::Result<Option<InstanceSnapshot>> {
    log::trace!("Building data from {}", canonical.display());

    let result = snapshot_from_vfs(symlinks, context, vfs, canonical);

    match result {
        Ok(Some(found_snapshot)) => {
            let mut snapshot: InstanceSnapshot = found_snapshot;
            if snapshot.snapshot_id.is_none() {
                snapshot.snapshot_id = Ref::new();
            }

            if let Some(symlink) = symlinks.get_mut(canonical) {
                symlink.point_to_id = snapshot.snapshot_id;

                log::trace!(
                    "Set {} to point to instance id {}",
                    symlink.name,
                    snapshot.snapshot_id
                );
            } else {
                log::error!("Failed to update symlink {}", path.display());
            }

            return Ok(Some(snapshot));
        }
        Ok(None) => {
            log::trace!("Failed to get snapshot result from {}", path.display());

            return Ok(None);
        }
        Err(e) => {
            log::error!(
                "Failed to get snapshot result from {}: {}",
                path.display(),
                e
            );

            return Ok(None);
        }
    }
}

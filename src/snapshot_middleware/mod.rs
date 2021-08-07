//! Defines the semantics that Rojo uses to turn entries on the filesystem into
//! Roblox instances using the instance snapshot subsystem.

#![allow(dead_code)]

use crate::snapshot::{InstanceMetadata, InstanceSnapshot};
use maplit::hashmap;
use rbx_dom_weak::types::Ref;
use std::{
    borrow::Cow,
    collections::HashMap,
    fs,
    path::{Path, PathBuf},
};

mod csv;
mod dir;
mod json;
mod json_model;
mod lua;
mod meta_file;
mod middleware;
mod project;
mod rbxm;
mod rbxmx;
mod txt;
mod util;

use memofs::{IoResultExt, Vfs};

use crate::snapshot::InstanceContext;

use self::{
    csv::snapshot_csv,
    dir::snapshot_dir,
    json::snapshot_json,
    json_model::snapshot_json_model,
    lua::{snapshot_lua, snapshot_lua_init},
    middleware::SnapshotInstanceResult,
    project::snapshot_project,
    rbxm::snapshot_rbxm,
    rbxmx::snapshot_rbxmx,
    txt::snapshot_txt,
    util::match_file_name,
};

pub use self::project::snapshot_project_node;

// TODO: Use vfs.metadata
fn is_symlink(path: &Path) -> std::io::Result<bool> {
    let metadata = fs::symlink_metadata(path)?;
    let file_type = metadata.file_type();

    let is_symlink = file_type.is_symlink();

    Ok(is_symlink)
}

fn snapshot_dir_complete(
    context: &InstanceContext,
    vfs: &Vfs,
    path: &Path,
    symlinks: &mut HashMap<PathBuf, Symlink>,
) -> SnapshotInstanceResult {
    let project_path = path.join("default.project.json");
    if vfs.metadata(&project_path).with_not_found()?.is_some() {
        return snapshot_project(context, vfs, &project_path, symlinks);
    }

    let init_path = path.join("init.lua");
    if vfs.metadata(&init_path).with_not_found()?.is_some() {
        return snapshot_lua_init(context, vfs, &init_path, symlinks);
    }

    let init_path = path.join("init.server.lua");
    if vfs.metadata(&init_path).with_not_found()?.is_some() {
        return snapshot_lua_init(context, vfs, &init_path, symlinks);
    }

    let init_path = path.join("init.client.lua");
    if vfs.metadata(&init_path).with_not_found()?.is_some() {
        return snapshot_lua_init(context, vfs, &init_path, symlinks);
    }

    snapshot_dir(context, vfs, path, symlinks)
}

/// Attempts to create a dirsymlink
fn snapshot_dirsymlink(
    context: &InstanceContext,
    vfs: &Vfs,
    path: &Path,
    symlinks: &mut HashMap<PathBuf, Symlink>,
) -> SnapshotInstanceResult {
    let canonical_path = fs::canonicalize(path)?;
    if symlinks.contains_key(&canonical_path) {
        log::warn!(
            "Making link to {0}",
            match path.to_owned().to_str() {
                Some(canonical_path) => canonical_path.to_owned(),
                _ => "bad path".to_owned(),
            }
        );

        match canonical_path.to_str() {
            Some(_canon_path) => {
                let properties = hashmap! {
                    "Value".to_owned() => symlinks[&canonical_path].0.into(),
                };

                Ok(Some(
                    InstanceSnapshot::new()
                        .name(symlinks[&canonical_path].1.to_owned())
                        .class_name("ObjectValue")
                        .properties(properties)
                        .metadata(
                            InstanceMetadata::new()
                                .instigating_source(path)
                                .relevant_paths(vec![canonical_path]) // TODO: Proper value here
                                .context(context),
                        ),
                ))
            }
            None => Ok(None),
        }
    } else {
        log::warn!(
            "Unlinked directory to {0}",
            match path.to_owned().to_str() {
                Some(canonical_path) => canonical_path.to_owned(),
                _ => "bad path".to_owned(),
            }
        );

        match snapshot_dir_complete(context, vfs, path, symlinks)? {
            Some(snapshot) => {
                // Add the modified symlink
                let mut modified_snapshot =
                    snapshot.symlink_canonical(Some(canonical_path.clone()));

                match modified_snapshot.snapshot_id {
                    Some(snapshot_id) => {
                        symlinks
                            .entry(canonical_path)
                            .or_insert(Symlink(snapshot_id, modified_snapshot.name.to_owned()));
                    }
                    _ => {
                        let snapshot_id = Ref::new();
                        modified_snapshot = modified_snapshot.snapshot_id(Some(snapshot_id));

                        symlinks
                            .entry(canonical_path)
                            .or_insert(Symlink(snapshot_id, modified_snapshot.name.to_owned()));
                    }
                }

                Ok(Some(modified_snapshot))
            }
            None => Ok(None),
        }
    }
}

/// ref, name
pub struct Symlink(Ref, Cow<'static, str>);

impl Symlink {
    pub fn new(reference: Ref, name: Cow<'static, str>) -> Self {
        Symlink(reference, name)
    }
}

pub fn recurse_generate_hashmap(
    mut symlinks: HashMap<PathBuf, Symlink>,
    snapshot_instance_result: SnapshotInstanceResult,
) -> () {
    match snapshot_instance_result {
        Ok(Some(InstanceSnapshot {
            symlink_canonical: Some(symlink_canonical_value),
            snapshot_id: Some(snapshot_id_value),
            name: name_value,
            ..
        })) => {
            symlinks
                .entry(symlink_canonical_value)
                .or_insert(Symlink(snapshot_id_value, name_value));
            ()
        }
        _ => (),
    }

    ()
}

/// The main entrypoint to the snapshot function. This function can be pointed
/// at any path and will return something if Rojo knows how to deal with it.
pub fn snapshot_from_vfs(
    context: &InstanceContext,
    vfs: &Vfs,
    path: &Path,
    symlinks: &mut HashMap<PathBuf, Symlink>,
) -> SnapshotInstanceResult {
    let meta = match vfs.metadata(path).with_not_found()? {
        Some(meta) => meta,
        None => return Ok(None),
    };

    if meta.is_dir() {
        // TODO: use vfs.metadata
        match is_symlink(path) {
            Ok(true) => snapshot_dirsymlink(context, vfs, path, symlinks),
            _ => snapshot_dir_complete(context, vfs, path, symlinks),
        }
    } else {
        if let Some(name) = match_file_name(path, ".lua") {
            match name {
                // init scripts are handled elsewhere and should not turn into
                // their own children.
                "init" | "init.client" | "init.server" => return Ok(None),

                _ => return snapshot_lua(context, vfs, path),
            }
        } else if let Some(_name) = match_file_name(path, ".project.json") {
            return snapshot_project(context, vfs, path, symlinks);
        } else if let Some(name) = match_file_name(path, ".model.json") {
            return snapshot_json_model(context, vfs, path, name);
        } else if let Some(_name) = match_file_name(path, ".meta.json") {
            // .meta.json files do not turn into their own instances.
            return Ok(None);
        } else if let Some(name) = match_file_name(path, ".json") {
            return snapshot_json(context, vfs, path, name);
        } else if let Some(name) = match_file_name(path, ".csv") {
            return snapshot_csv(context, vfs, path, name);
        } else if let Some(name) = match_file_name(path, ".txt") {
            return snapshot_txt(context, vfs, path, name);
        } else if let Some(name) = match_file_name(path, ".rbxmx") {
            return snapshot_rbxmx(context, vfs, path, name);
        } else if let Some(name) = match_file_name(path, ".rbxm") {
            return snapshot_rbxm(context, vfs, path, name);
        }

        Ok(None)
    }
}

//! Symlink snapshot data

use rbx_dom_weak::types::Ref;
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, HashMap},
    path::PathBuf,
};

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
pub struct Symlink {
    // Reference id
    pub id: Ref,

    pub point_to_id: Ref,

    // Instance name
    pub name: String,

    pub reifed_path: PathBuf,
}

// Represents a set of symlinks
pub type Symlinks = BTreeMap<PathBuf, Symlink>;

pub fn link_symlink_id_to_point_to_id(
    snapshot_id_to_instance_id: &mut HashMap<Ref, Ref>,
    symlinks: &mut crate::snapshot::Symlinks,
) {
    for entry in symlinks {
        // Ensure we can look up the symlink ref id
        snapshot_id_to_instance_id.insert(entry.1.id, entry.1.point_to_id);
    }
}

/// Translate symlink ids to be instance ids
pub fn link_old_ids_to_new_ids_and_update_symlink(
    snapshot_id_to_instance_id: &mut HashMap<Ref, Ref>,
    symlinks: &mut crate::snapshot::Symlinks,
) {
    for entry in symlinks {
        if let Some(&instance_referent) = snapshot_id_to_instance_id.get(&entry.1.point_to_id) {
            // Link old id to our new instance
            snapshot_id_to_instance_id.insert(entry.1.point_to_id, instance_referent);
            snapshot_id_to_instance_id.insert(entry.1.id, instance_referent);

            log::trace!(
                "Symlink {} now points to {}",
                entry.1.name,
                instance_referent
            );

            entry.1.point_to_id = instance_referent;
        } else {
            log::trace!(
                "Failed to find snapshot_id_to_instance_id {} for {}",
                entry.1.point_to_id,
                entry.1.name,
            );
        }
    }
}

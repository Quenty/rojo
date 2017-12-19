use std::collections::HashMap;

use core::Route;
use plugin::{Plugin, PluginChain, TransformFileResult, TransformRbxResult, FileChangeResult};
use rbx::{RbxItem, RbxValue};
use vfs::VfsItem;

/// A plugin with simple transforms:
/// * Directories become Folder instances
/// * Files become StringValue objects with 'Value' as their contents
pub struct DefaultPlugin;

impl DefaultPlugin {
    pub fn new() -> DefaultPlugin {
        DefaultPlugin
    }
}

impl Plugin for DefaultPlugin {
    fn transform_result(&self, plugins: &PluginChain, rbx_item: &RbxItem) -> TransformRbxResult {
        match rbx_itme {
            // does pattern matching even work like this?
            &RbxItem { children: NOT_EMPTY_RIP_DONT_KNOW_RUST } =>
            &RbxItem { class_name: "Folder".to_string(), ref name, ref children, .. } => {
                // handle folder case
                let mut vfs_children = HashMap::new();

                for child_item in (children) {
                    match plugins.transform_rbx(child_item) {
                        Some(vfs_item) => {
                            vfs_children.insert(child_item.name, vfs_item);
                        },
                        _ => {},
                    }
                }

                TransformRbxResult::Value(Some(VfsItem::Folder {
                    name: name.clone(),
                    children: vfs_children,
                }))
            },
            &RbxItem {ref class_name, ref name, ref children, ref properties} => {
                // handle default case (string value/general serialization)
                TransformRbxResult::Value(Some(VfsItem::File {
                    name: name.clone(),

                }))
            },
        }
    }

    fn transform_file(&self, plugins: &PluginChain, vfs_item: &VfsItem) -> TransformFileResult {
        match vfs_item {
            &VfsItem::File { ref contents, ref name } => {
                let mut properties = HashMap::new();

                properties.insert("Value".to_string(), RbxValue::String {
                    value: contents.clone(),
                });

                TransformFileResult::Value(Some(RbxItem {
                    name: name.clone(),
                    class_name: "StringValue".to_string(),
                    children: Vec::new(),
                    properties,
                }))
            },
            &VfsItem::Dir { ref children, ref name } => {
                let mut rbx_children = Vec::new();

                for (_, child_item) in children {
                    match plugins.transform_file(child_item) {
                        Some(rbx_item) => {
                            rbx_children.push(rbx_item);
                        },
                        _ => {},
                    }
                }

                TransformResult::Value(Some(RbxItem {
                    name: name.clone(),
                    class_name: "Folder".to_string(),
                    children: rbx_children,
                    properties: HashMap::new(),
                }))
            },
        }
    }

    fn handle_file_change(&self, route: &Route) -> FileChangeResult {
        FileChangeResult::MarkChanged(Some(vec![route.clone()]))
    }
}

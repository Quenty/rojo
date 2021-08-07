# Goals
Support node_modules in 2 ways.

1. Support dependency searching recursively, supporting multiple versions of the same dependency
2. Avoid having to publish a package to download/test upstream dependencies
3. Allow us to ignore node_modules that don't exist (folder-wise)

# Design notes

1. Want to avoid having to modify the plugin
# How to do this

1. De-duplicate symlinked folders such that the first instance points to the original structure and the second is an object value


We need to take `snapshot_middleware::snapshot_from_vfs` and have to detect symlinks
We need a global data structure that is effectively a hashmap of the canonical symlinks and use that to lookup the other location of the symliink
    If snapshot_from_vfs does not not know about the symlink's final location, we need to store it as a reference, and do a second pass to reify this correctly
    This may break aggressively

We need to modify snapshot_from_vfs to allow $ignore property to be enabled

# What we're doing

1. We might need to modify memofs (https://docs.rs/memofs/0.1.3/memofs/) to support symlinks
2. Try to detect symlinks and add a name or something to the end


------

`InstanceContext` is cloned all the way down recursively for each project. Currently this is not mutable, and holds ignore files, and is recursively inherited.

We can inherit our existing symlinks in this and use it to resolve to an object value pointing to this value. (Although rojo sync doesn't support refs, I believe, so we will be hacking this in).

OR: We can just make these into a fake module script... Hehehehe. (Bounce like wally does)
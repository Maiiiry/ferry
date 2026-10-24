//! Recursive tree cloning shared by the copy and Linux reflink backends:
//! walks the base target, recreates directories and symlinks, clones or
//! copies regular files, preserves hard links within the tree, and restores
//! mtimes and permissions.

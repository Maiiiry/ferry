//! Linux overlayfs backend: the base target is the read-only lower layer,
//! each agent gets its own upper and work dirs. The mount itself happens in
//! the agent's namespace sandbox (see `sandbox::linux`).

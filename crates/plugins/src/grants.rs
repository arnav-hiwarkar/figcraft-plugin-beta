//! Fail closed if the shared store cannot be refreshed.
use craft_ext_core::{id::ExtensionId, permission::Permission, version::Version};
use craft_ext_host::api::Grants;
use craft_ext_store::view::GrantView;

pub struct LiveGrants {
    pub root: std::path::PathBuf,
    pub app: craft_ext_core::id::AppId,
}
impl LiveGrants {
    fn read<T>(&self, fallback: T, f: impl FnOnce(&GrantView) -> T) -> T {
        // Read a complete snapshot on every check: a failed refresh must never leave cached
        // permissions usable, including within the metadata probe throttle window.
        let Ok(view) = GrantView::open(&self.root, self.app.clone()) else { return fallback };
        f(&view)
    }
}
impl Grants for LiveGrants {
    fn generation(&self, id: &ExtensionId) -> u64 {
        self.read(u64::MAX, |v| v.generation(id))
    }
    fn allowed(&self, id: &ExtensionId, p: &Permission) -> bool {
        self.read(false, |v| v.allowed(id, p))
    }
    fn stopped(&self, id: &ExtensionId, version: Version) -> bool {
        super::safe_mode() || self.read(true, |v| v.stopped(id, version))
    }
    fn full_access(&self, id: &ExtensionId) -> bool {
        self.read(false, |v| v.full_access(id))
    }
}

//! Fail closed if the shared store cannot be refreshed.
use craft_ext_core::{id::ExtensionId, permission::Permission, version::Version};
use craft_ext_host::api::Grants;
use craft_ext_store::view::GrantView;
use std::sync::Mutex;

pub struct LiveGrants(pub Mutex<GrantView>);
impl LiveGrants {
    fn read<T>(&self, fallback: T, f: impl FnOnce(&GrantView) -> T) -> T {
        let Ok(mut view) = self.0.lock() else { return fallback };
        if view.refresh_if_changed().is_err() {
            return fallback;
        }
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

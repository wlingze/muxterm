//! Settings 页的 FFI-backed 配置操作模型。

use std::cell::RefCell;
use std::rc::Rc;

use crate::ffi_client::{ClientConfigSnapshot, ClientJsonPatchOperation, FfiClient};

/// FFI-backed configuration operations used by the GTK settings views.
///
/// GTK callbacks outlive the stack frame that opened the window, so they
/// receive operations rather than a borrowed `FfiClient`. The production
/// window resolves the client from `EventPump` at call time; tests can keep a
/// dedicated catalog client in an `Rc<RefCell<_>>`.
type ConfigDescribeFn = dyn Fn() -> anyhow::Result<ClientConfigSnapshot>;
type ConfigApplyFn = dyn Fn(&[ClientJsonPatchOperation]) -> anyhow::Result<ClientConfigSnapshot>;
type ConfigReloadFn = dyn Fn() -> anyhow::Result<ClientConfigSnapshot>;

#[derive(Clone)]
pub struct ConfigApi {
    describe: Rc<ConfigDescribeFn>,
    apply: Rc<ConfigApplyFn>,
    reload: Rc<ConfigReloadFn>,
}

impl ConfigApi {
    pub fn from_callbacks(
        describe: impl Fn() -> anyhow::Result<ClientConfigSnapshot> + 'static,
        apply: impl Fn(&[ClientJsonPatchOperation]) -> anyhow::Result<ClientConfigSnapshot> + 'static,
        reload: impl Fn() -> anyhow::Result<ClientConfigSnapshot> + 'static,
    ) -> Self {
        Self {
            describe: Rc::new(describe),
            apply: Rc::new(apply),
            reload: Rc::new(reload),
        }
    }

    pub fn from_client(client: Rc<RefCell<FfiClient>>) -> Self {
        let describe_client = client.clone();
        let apply_client = client.clone();
        let reload_client = client;
        Self::from_callbacks(
            move || describe_client.borrow().config_describe(),
            move |patch| apply_client.borrow().config_apply(patch),
            move || {
                reload_client.borrow().config_reload()?;
                reload_client.borrow().config_describe()
            },
        )
    }

    pub fn describe(&self) -> anyhow::Result<ClientConfigSnapshot> {
        (self.describe)()
    }

    pub fn apply(
        &self,
        patch: &[ClientJsonPatchOperation],
    ) -> anyhow::Result<ClientConfigSnapshot> {
        (self.apply)(patch)
    }

    pub fn reload(&self) -> anyhow::Result<ClientConfigSnapshot> {
        (self.reload)()
    }
}

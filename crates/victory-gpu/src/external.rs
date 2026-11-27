//! Host work that a host library records into a title's command list in
//! place of a console one (the host's FidelityFX upscaler, say): the host
//! device, a title texture's host copy in the state the library is told,
//! and the host list itself.
//!
//! Everything is handed out as a raw COM pointer, so that the caller needs
//! no D3D12 bindings of its own. The device lives for the run; a texture
//! while its title resource does (the host list holds it through
//! submission once it names it); the list while the title's list is open.
//!
//! Ported from Belobug/VIctory-Belobug-AMD cd8dfc06
//! (crates/victory-gpu/src/external.rs) and adapted to our host: a title
//! list has its host list from the start, and the title's draw state is
//! replayed after the call, as [`crate::list::upscale_stretch`] does. With
//! lists written down (`VICTORY_THIN=1`, `crate::list::thin`) a title list
//! has no host list: the work is written down at its place and done on the
//! list of ours its calls are translated on ([`later`], [`record_later`]),
//! with what it reads copied at the call.

use std::ffi::c_void;

use windows::core::Interface;

use crate::object::This;

/// The host D3D12 device (`ID3D12Device`), created on first use; `None`
/// without a host GPU.
pub fn host_device() -> Option<*mut c_void> {
    crate::host::host().map(|h| h.device.as_raw())
}

/// A title texture's host copy, as a library is to see it.
#[derive(Clone, Copy, Debug)]
pub struct Texture {
    /// The host `ID3D12Resource`.
    pub raw: *mut c_void,
    /// The host state it is in (`D3D12_RESOURCE_STATES`), what was asked
    /// for less the bits its flags forbid.
    pub state: u32,
    pub width: u64,
    pub height: u32,
    /// Its `DXGI_FORMAT`.
    pub format: i32,
}

/// The host list behind a title list, for one synchronous external
/// recording call.
pub struct Recording<'a>(&'a crate::host::HostList);

impl Recording<'_> {
    /// Moves a title texture's host copy to host state `state` in the list
    /// (as a draw would: the list's first use of it is left to its
    /// prelude).
    pub fn texture(&self, resource: *const c_void, state: u32) -> Option<Texture> {
        let resource: This = resource.cast();
        let texture = crate::resource::host_resource(resource)?;
        crate::list::ensure_state(self.0, resource, state);
        let state = crate::host::legal_state(texture, state);
        // SAFETY: a live resource.
        let desc = unsafe { texture.GetDesc() };
        Some(Texture {
            raw: texture.as_raw(),
            state,
            width: desc.Width,
            height: desc.Height,
            format: desc.Format.0,
        })
    }

    /// Brings a texture's host copy up to date as for a shader read
    /// (`crate::draw`'s shader resource views): an alias's source texels, a
    /// texture the title fills uploaded, a depth texture's colour twin.
    pub fn prepare_input(&self, resource: *const c_void) -> Option<()> {
        let resource: This = resource.cast();
        let texture = crate::resource::host_resource(resource)?;
        let aliased = crate::resource::alias_of(resource)
            .is_some_and(|source| crate::texture::alias(self.0, resource, source.get()));
        if !aliased && crate::texture::title_written(texture) {
            crate::texture::upload(self.0, resource, texture);
        }
        self.0.twin_use(resource, false, false);
        Some(())
    }

    /// The host `ID3D12GraphicsCommandList`, open for recording.
    pub fn commands(&self) -> *mut c_void {
        self.0.list.as_raw()
    }

    /// The title's colour texture `colour` stretched over `output` from its
    /// top-left `render` texels, as `crate::list::upscale_stretch` does:
    /// whether it was recorded.
    pub fn stretch(&self, colour: *const c_void, output: *const c_void, render: [u32; 2]) -> bool {
        crate::list::stretch_on(self.0, colour.cast(), output.cast(), render) == Some(true)
    }
}

/// Whether work recorded into title list `list` is done later
/// ([`record_later`]): its calls are written down (`VICTORY_THIN=1`) and
/// translated by threads of ours, in their order.
pub fn later(list: *const c_void) -> bool {
    !list.is_null() && crate::list::thin::on()
}

/// Has `f` record at this place of title list `list`, when the list's calls
/// are translated (now, if they are not written down): on a thread of ours,
/// with the host list there. `f` says whether it recorded into it (then the
/// title's own state is set again). Whatever `f` reads of the title's must be
/// copied into it now. False if the list records nowhere (`f` is dropped).
pub fn record_later(list: *const c_void, f: impl FnOnce(&Recording<'_>) -> bool + Send + 'static) -> bool {
    crate::list::external(list.cast(), Box::new(move |host: &crate::host::HostList| f(&Recording(host))))
}

/// The library recorded `resource`'s new contents on the host list: they
/// are on the host, not in title memory.
pub fn output_written(resource: *const c_void) {
    crate::texture::host_written(resource.cast());
}

/// Settles submitted title work before a library releases GPU resources
/// of its own.
pub fn finish_submitted() {
    crate::queue::before_sync();
    crate::host::settle_for_fence();
}

/// Runs `f` with the host list behind the title list `list` (open for
/// recording); `None` without one. What `f` binds (descriptor heaps, root
/// signature, pipeline) is replaced at once by the title's own state.
pub fn record<R>(list: *const c_void, f: impl FnOnce(&Recording<'_>) -> R) -> Option<R> {
    let list: This = list.cast();
    if list.is_null() {
        return None;
    }
    let mut out = None;
    crate::list::with_state(list, |host, state| {
        out = Some(f(&Recording(host)));
        state.replay(host);
    });
    out
}
